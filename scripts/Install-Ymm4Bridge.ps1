[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter()]
    [string] $Ymm4Path = $env:YMM4_PATH,

    [Parameter()]
    [string] $BridgeDll,

    [Parameter()]
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($Ymm4Path)) {
    throw 'Ymm4Path is required. Pass -Ymm4Path or set YMM4_PATH.'
}

$resolvedYmm4Path = (Resolve-Path -LiteralPath $Ymm4Path).Path
$ymm4Executable = Join-Path $resolvedYmm4Path 'YukkuriMovieMaker.exe'
if (-not (Test-Path -LiteralPath $ymm4Executable -PathType Leaf)) {
    throw "YukkuriMovieMaker.exe was not found under $resolvedYmm4Path"
}

function Assert-Ymm4Stopped {
    $runningYmm4 = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
            $_.ProcessName -eq 'YukkuriMovieMaker' -or
            $_.ProcessName -eq 'YukkuriMovieMaker.Win32Service'
        })
    if ($runningYmm4.Count -ne 0) {
        $processSummary = ($runningYmm4 | ForEach-Object { "$($_.ProcessName)#$($_.Id)" }) -join ', '
        throw "YMM4 must be closed before plugin installation. Running: $processSummary"
    }
}
Assert-Ymm4Stopped

$project = Join-Path $repositoryRoot 'bridges\ymm4\TakeGraph.Ymm4Bridge\TakeGraph.Ymm4Bridge.csproj'
if (-not $SkipBuild) {
    # Keep human-facing build output out of the success pipeline so wrappers
    # can consume the final JSON receipt without scraping localized log text.
    & dotnet build $project -c Release -p:UseYmm4ContractStub=false ("-p:Ymm4Path={0}" -f $resolvedYmm4Path) | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "Bridge build failed with exit code $LASTEXITCODE"
    }
}

if ([string]::IsNullOrWhiteSpace($BridgeDll)) {
    $BridgeDll = Join-Path $repositoryRoot 'bridges\ymm4\TakeGraph.Ymm4Bridge\bin\Release\net10.0-windows\TakeGraph.Ymm4Bridge.dll'
}
$source = (Resolve-Path -LiteralPath $BridgeDll).Path
$sourceReferences = [Reflection.Assembly]::LoadFile($source).GetReferencedAssemblies().Name
if ('YukkuriMovieMaker.Plugin' -notin $sourceReferences) {
    throw 'Bridge DLL is not a production YMM4-contract build; contract-stub artifacts cannot be installed.'
}
$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
Assert-Ymm4Stopped

$pluginDirectory = Join-Path $resolvedYmm4Path 'user\plugin\TakeGraph.Ymm4Bridge'
$target = Join-Path $pluginDirectory 'TakeGraph.Ymm4Bridge.dll'
if ([System.IO.Path]::GetFullPath($source).Equals(
        [System.IO.Path]::GetFullPath($target),
        [StringComparison]::OrdinalIgnoreCase)) {
    throw 'BridgeDll must be a build artifact, not the currently installed target.'
}
# YMM4 recursively probes DLLs below user\plugin. Backups must live outside
# that tree and must not retain a .dll suffix, otherwise an old bridge can win
# plugin discovery before the installed target.
$backupDirectory = Join-Path $resolvedYmm4Path 'user\TakeGraph\plugin-backups\TakeGraph.Ymm4Bridge'
$legacyBackupDirectory = Join-Path $pluginDirectory 'backups'
$incoming = Join-Path $pluginDirectory (".TakeGraph.Ymm4Bridge.{0}.incoming" -f [guid]::NewGuid().ToString('N'))

if (-not $PSCmdlet.ShouldProcess($target, "install bridge SHA-256 $sourceHash")) {
    return
}

[System.IO.Directory]::CreateDirectory($pluginDirectory) | Out-Null
[System.IO.Directory]::CreateDirectory($backupDirectory) | Out-Null

# Migrate backups created by older installers out of YMM4's recursive plugin
# discovery root. Copy+hash+delete makes the move auditable across volumes and
# leaves the original intact if verification fails.
if (Test-Path -LiteralPath $legacyBackupDirectory -PathType Container) {
    foreach ($legacyBackup in @(Get-ChildItem -LiteralPath $legacyBackupDirectory -File -Filter '*.dll')) {
        $legacyHash = (Get-FileHash -LiteralPath $legacyBackup.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $legacyDestination = Join-Path $backupDirectory ("{0}.{1}.backup" -f $legacyBackup.BaseName, $legacyHash.Substring(0, 12))
        if (Test-Path -LiteralPath $legacyDestination -PathType Leaf) {
            $existingHash = (Get-FileHash -LiteralPath $legacyDestination -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($existingHash -ne $legacyHash) {
                throw "Legacy backup migration destination conflicts: $legacyDestination"
            }
        }
        else {
            Copy-Item -LiteralPath $legacyBackup.FullName -Destination $legacyDestination
            $copiedHash = (Get-FileHash -LiteralPath $legacyDestination -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($copiedHash -ne $legacyHash) {
                throw "Legacy backup migration hash mismatch: $($legacyBackup.FullName)"
            }
        }
        Remove-Item -LiteralPath $legacyBackup.FullName -Force
    }
}

$backup = $null
try {
    Copy-Item -LiteralPath $source -Destination $incoming
    $incomingHash = (Get-FileHash -LiteralPath $incoming -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($incomingHash -ne $sourceHash) {
        throw 'The staged bridge copy does not match the built DLL hash.'
    }

    $targetExisted = Test-Path -LiteralPath $target -PathType Leaf
    if ($targetExisted) {
        $oldHash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
        $stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
        $backup = Join-Path $backupDirectory ("TakeGraph.Ymm4Bridge.{0}.{1}.dll.backup" -f $stamp, $oldHash.Substring(0, 12))
    }

    Assert-Ymm4Stopped
    if ($targetExisted) {
        [System.IO.File]::Replace($incoming, $target, $backup, $true)
        $backupHash = (Get-FileHash -LiteralPath $backup -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($backupHash -ne $oldHash) {
            throw 'The atomic bridge backup could not be verified.'
        }
    }
    else {
        [System.IO.File]::Move($incoming, $target)
    }
    $installedHash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($installedHash -ne $sourceHash) {
        if ($targetExisted -and (Test-Path -LiteralPath $backup -PathType Leaf)) {
            $restoreIncoming = Join-Path $pluginDirectory (".TakeGraph.Ymm4Bridge.{0}.restore" -f [guid]::NewGuid().ToString('N'))
            try {
                Copy-Item -LiteralPath $backup -Destination $restoreIncoming
                $restoreHash = (Get-FileHash -LiteralPath $restoreIncoming -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($restoreHash -ne $oldHash) {
                    throw 'The verified bridge backup could not be staged for restoration.'
                }
                [System.IO.File]::Replace($restoreIncoming, $target, $null, $true)
                $restoredHash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($restoredHash -ne $oldHash) {
                    throw 'The previous bridge could not be restored after verification failure.'
                }
            }
            finally {
                if (Test-Path -LiteralPath $restoreIncoming -PathType Leaf) {
                    Remove-Item -LiteralPath $restoreIncoming -Force
                }
            }
            throw 'The new bridge failed post-install verification; the previous bridge was restored.'
        }
        if (Test-Path -LiteralPath $target -PathType Leaf) {
            Remove-Item -LiteralPath $target -Force
        }
        throw 'The new bridge failed post-install verification and was removed.'
    }

    [pscustomobject]@{
        status = 'installed'
        target = $target
        sha256 = $installedHash
        byteLength = (Get-Item -LiteralPath $target).Length
        backup = $backup
        ymm4Executable = $ymm4Executable
    } | ConvertTo-Json -Depth 4
}
finally {
    if (Test-Path -LiteralPath $incoming -PathType Leaf) {
        Remove-Item -LiteralPath $incoming -Force
    }
}
