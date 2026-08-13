[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'Medium')]
param(
    [Parameter()]
    [string] $Ymm4Path = $env:YMM4_PATH,

    [Parameter()]
    [ValidateRange(5, 120)]
    [int] $ShutdownTimeoutSeconds = 30,

    [Parameter()]
    [ValidateRange(5, 120)]
    [int] $StartupTimeoutSeconds = 45,

    [Parameter()]
    [string[]] $Ymm4Arguments = @(),

    [Parameter()]
    [string[]] $RequiredCapability = @()
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$installer = Join-Path $PSScriptRoot 'Install-Ymm4Bridge.ps1'
$takeGraphLocalDirectory = Join-Path $env:LOCALAPPDATA 'TakeGraph'
$credentialPath = Join-Path $takeGraphLocalDirectory 'ymm4-bridge.json'
$updateLockPath = Join-Path $takeGraphLocalDirectory 'ymm4-bridge-update.lock'
$bridgeEndpoint = 'http://127.0.0.1:8766'

if ([string]::IsNullOrWhiteSpace($Ymm4Path)) {
    throw 'Ymm4Path is required. Pass -Ymm4Path or set YMM4_PATH.'
}
$resolvedYmm4Path = (Resolve-Path -LiteralPath $Ymm4Path).Path
$ymm4Executable = Join-Path $resolvedYmm4Path 'YukkuriMovieMaker.exe'
if (-not (Test-Path -LiteralPath $ymm4Executable -PathType Leaf)) {
    throw "YukkuriMovieMaker.exe was not found under $resolvedYmm4Path"
}
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "Bridge installer was not found: $installer"
}

function Get-Ymm4Processes {
    @(
        Get-Process -ErrorAction SilentlyContinue | Where-Object {
            $_.ProcessName -eq 'YukkuriMovieMaker' -or
            $_.ProcessName -eq 'YukkuriMovieMaker.Win32Service'
        }
    )
}

function Assert-Ymm4ProcessesMatchTarget {
    $expectedMainExecutable = [System.IO.Path]::GetFullPath($ymm4Executable)
    foreach ($process in @(Get-Ymm4Processes)) {
        $processPath = [string] $process.Path
        if ([string]::IsNullOrWhiteSpace($processPath)) {
            throw "Cannot verify the executable path for $($process.ProcessName)#$($process.Id). Close YMM4 manually and retry."
        }
        $fullProcessPath = [System.IO.Path]::GetFullPath($processPath)
        if ($process.ProcessName -eq 'YukkuriMovieMaker') {
            if (-not $fullProcessPath.Equals(
                    $expectedMainExecutable,
                    [StringComparison]::OrdinalIgnoreCase)) {
                throw "A YMM4 main process from another executable is running: $fullProcessPath"
            }
            continue
        }

        # The Win32 service lives below the selected YMM4 installation, but
        # its exact relative location is owned by YMM4 and may change.
        $relativePath = [System.IO.Path]::GetRelativePath($resolvedYmm4Path, $fullProcessPath)
        if ([System.IO.Path]::IsPathRooted($relativePath) -or
            $relativePath -eq '..' -or
            $relativePath.StartsWith("..$([System.IO.Path]::DirectorySeparatorChar)", [StringComparison]::Ordinal)) {
            throw "A YMM4 process from another installation is running: $fullProcessPath"
        }
    }
}

function Read-BridgeCredentials {
    if (-not (Test-Path -LiteralPath $credentialPath -PathType Leaf)) {
        return $null
    }
    try {
        $credentials = Get-Content -LiteralPath $credentialPath -Raw | ConvertFrom-Json
        if ([string]::IsNullOrWhiteSpace([string] $credentials.token)) {
            return $null
        }
        [pscustomobject]@{
            token = [string] $credentials.token
        }
    }
    catch {
        return $null
    }
}

function Read-ActiveNamedProjectPath {
    $credentials = Read-BridgeCredentials
    if ($null -eq $credentials) {
        return $null
    }
    try {
        $snapshot = Invoke-RestMethod `
            -Method Get `
            -Uri "$bridgeEndpoint/v1/project/snapshot" `
            -Headers @{ 'x-takegraph-token' = $credentials.token } `
            -TimeoutSec 3
        $projectPath = [string] $snapshot.projectPath
        if ([string]::IsNullOrWhiteSpace($projectPath)) {
            return $null
        }
        $fullProjectPath = [System.IO.Path]::GetFullPath($projectPath)
        if (-not (Test-Path -LiteralPath $fullProjectPath -PathType Leaf)) {
            throw "The active named YMM4 project does not exist on disk: $fullProjectPath"
        }
        return $fullProjectPath
    }
    catch {
        if ($_.Exception.Message -like 'The active named YMM4 project does not exist*') {
            throw
        }
        # An old/unreachable bridge cannot provide restoration evidence. The
        # normal close remains safe, but this updater will not guess a path.
        return $null
    }
}

function Request-Ymm4Close {
    $mainProcesses = @(Get-Process -Name 'YukkuriMovieMaker' -ErrorAction SilentlyContinue)
    if ($mainProcesses.Count -gt 1) {
        throw 'More than one YMM4 main process is running; close them manually before updating.'
    }
    $credentials = Read-BridgeCredentials
    if ($null -ne $credentials) {
        try {
            # If the authenticated bridge is reachable, its close result is
            # authoritative. Do not bypass a recovery/write-gate rejection by
            # falling back to CloseMainWindow.
            Invoke-RestMethod `
                -Method Get `
                -Uri "$bridgeEndpoint/v1/health" `
                -Headers @{ 'x-takegraph-token' = $credentials.token } `
                -TimeoutSec 2 | Out-Null
            Invoke-RestMethod `
                -Method Post `
                -Uri "$bridgeEndpoint/v1/application/close" `
                -Headers @{ 'x-takegraph-token' = $credentials.token } `
                -TimeoutSec 5 | Out-Null
            return
        }
        catch {
            $statusCode = $null
            $responseProperty = $_.Exception.PSObject.Properties['Response']
            if ($null -ne $responseProperty -and $null -ne $responseProperty.Value) {
                $statusProperty = $responseProperty.Value.PSObject.Properties['StatusCode']
                if ($null -ne $statusProperty) {
                    $statusCode = [int] $statusProperty.Value
                }
            }
            if ($null -ne $statusCode -and $statusCode -notin @(404, 405)) {
                throw "The authenticated bridge refused the normal close request (HTTP $statusCode). Resolve its recovery state before updating."
            }
            # An absent/old route or connection failure may fall back to the
            # application's normal close request, never a force-stop.
        }
    }

    $closeRequested = $false
    foreach ($process in $mainProcesses) {
        if ($process.CloseMainWindow()) {
            $closeRequested = $true
        }
    }
    if ($mainProcesses.Count -gt 0 -and -not $closeRequested) {
        throw 'YMM4 did not expose a normal close action. Close it manually; the updater will not force-stop it.'
    }
}

function Wait-Ymm4Stopped {
    $deadline = [DateTime]::UtcNow.AddSeconds($ShutdownTimeoutSeconds)
    while (@(Get-Ymm4Processes).Count -gt 0 -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    $remaining = @(Get-Ymm4Processes)
    if ($remaining.Count -gt 0) {
        $summary = ($remaining | ForEach-Object { "$($_.ProcessName)#$($_.Id)" }) -join ', '
        throw "YMM4 did not close within $ShutdownTimeoutSeconds seconds. Resolve any save confirmation and retry. Still running: $summary"
    }
}

function Wait-BridgeReady(
    [System.Diagnostics.Process] $StartedProcess,
    [string] $ExpectedPluginVersion,
    [string] $ExpectedProjectPath) {
    $deadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    $lastHealth = $null
    $lastCapabilities = $null
    $lastProjectPath = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($StartedProcess.HasExited) {
            throw "YMM4 exited before the bridge became healthy (exit code $($StartedProcess.ExitCode))."
        }
        $credentials = Read-BridgeCredentials
        if ($null -ne $credentials) {
            try {
                $headers = @{ 'x-takegraph-token' = $credentials.token }
                $lastHealth = Invoke-RestMethod `
                    -Method Get `
                    -Uri "$bridgeEndpoint/v1/health" `
                    -Headers $headers `
                    -TimeoutSec 2
                if ($lastHealth.status -eq 'running' -and
                    [int] $lastHealth.protocolVersion -eq 2 -and
                    [string] $lastHealth.pluginVersion -eq $ExpectedPluginVersion) {
                    $lastCapabilities = Invoke-RestMethod `
                        -Method Get `
                        -Uri "$bridgeEndpoint/v1/capabilities" `
                        -Headers $headers `
                        -TimeoutSec 2
                    if ([int] $lastCapabilities.protocolVersion -ne 2) {
                        throw 'Updated bridge returned an unsupported capability protocol version.'
                    }
                    $capabilitySet = [System.Collections.Generic.HashSet[string]]::new(
                        [StringComparer]::Ordinal)
                    foreach ($capability in @($lastCapabilities.capabilities)) {
                        [void] $capabilitySet.Add([string] $capability)
                    }
                    $missing = @(
                        $RequiredCapability | Where-Object {
                            -not $capabilitySet.Contains([string] $_)
                        }
                    )
                    if ($missing.Count -gt 0) {
                        throw "Updated bridge is running but required capabilities are absent: $($missing -join ', ')"
                    }
                    if (-not [string]::IsNullOrWhiteSpace($ExpectedProjectPath)) {
                        $snapshot = Invoke-RestMethod `
                            -Method Get `
                            -Uri "$bridgeEndpoint/v1/project/snapshot" `
                            -Headers $headers `
                            -TimeoutSec 2
                        $lastProjectPath = [string] $snapshot.projectPath
                        if ([string]::IsNullOrWhiteSpace($lastProjectPath) -or
                            -not [System.IO.Path]::GetFullPath($lastProjectPath).Equals(
                                [System.IO.Path]::GetFullPath($ExpectedProjectPath),
                                [StringComparison]::OrdinalIgnoreCase)) {
                            Start-Sleep -Milliseconds 300
                            continue
                        }
                    }
                    return [pscustomobject]@{
                        health = $lastHealth
                        capabilities = $lastCapabilities
                    }
                }
            }
            catch {
                if ($_.Exception.Message -like 'Updated bridge is running but required capabilities*' -or
                    $_.Exception.Message -eq 'Updated bridge returned an unsupported capability protocol version.') {
                    throw
                }
            }
        }
        Start-Sleep -Milliseconds 300
    }
    $observed = if ($null -eq $lastHealth) {
        'no authenticated health response'
    }
    else {
        "status=$($lastHealth.status), protocolVersion=$($lastHealth.protocolVersion), pluginVersion=$($lastHealth.pluginVersion), projectPath=$lastProjectPath"
    }
    throw "YMM4 started, but the expected TakeGraph bridge did not become healthy within $StartupTimeoutSeconds seconds ($observed). Check the newest YMM4 user log."
}

$action = 'normally close YMM4, install the production bridge, restart YMM4, and verify bridge health'
if (-not $PSCmdlet.ShouldProcess($resolvedYmm4Path, $action)) {
    return
}

[System.IO.Directory]::CreateDirectory($takeGraphLocalDirectory) | Out-Null
$updateLock = $null
try {
    try {
        $updateLock = [System.IO.File]::Open(
            $updateLockPath,
            [System.IO.FileMode]::OpenOrCreate,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None)
    }
    catch [System.IO.IOException] {
        throw 'Another TakeGraph YMM4 bridge update is already running.'
    }

    Assert-Ymm4ProcessesMatchTarget
    $projectPathToRestore = $null
    if ($Ymm4Arguments.Count -eq 0 -and @(Get-Ymm4Processes).Count -gt 0) {
        $projectPathToRestore = Read-ActiveNamedProjectPath
    }
    if (@(Get-Ymm4Processes).Count -gt 0) {
        Request-Ymm4Close
        Wait-Ymm4Stopped
    }

    # The wrapper intentionally offers no arbitrary DLL or skip-build option.
    # Every normal update recompiles against the real YMM4 plugin contract.
    $installOutput = & $installer -Ymm4Path $resolvedYmm4Path -Confirm:$false
    $installation = ($installOutput | Out-String) | ConvertFrom-Json

    $installedHash = (Get-FileHash -LiteralPath $installation.target -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($installedHash -ne [string] $installation.sha256) {
        throw 'Installed bridge hash changed after the installer verification.'
    }
    $pluginRoot = Join-Path $resolvedYmm4Path 'user\plugin'
    $duplicateBridges = @(
        Get-ChildItem -LiteralPath $pluginRoot -Recurse -File -Filter 'TakeGraph.Ymm4Bridge*.dll' |
            Where-Object {
                -not [System.IO.Path]::GetFullPath($_.FullName).Equals(
                    [System.IO.Path]::GetFullPath([string] $installation.target),
                    [StringComparison]::OrdinalIgnoreCase)
            }
    )
    if ($duplicateBridges.Count -gt 0) {
        throw "Another TakeGraph.Ymm4Bridge.dll exists under YMM4's plugin discovery tree: $($duplicateBridges[0].FullName)"
    }
    $expectedPluginVersion = (Get-Item -LiteralPath $installation.target).VersionInfo.ProductVersion
    if ([string]::IsNullOrWhiteSpace($expectedPluginVersion)) {
        throw 'Installed bridge has no informational product version.'
    }

    $startParameters = @{
        FilePath = $ymm4Executable
        PassThru = $true
        WindowStyle = 'Normal'
    }
    if ($Ymm4Arguments.Count -gt 0) {
        $startParameters.ArgumentList = $Ymm4Arguments
    }
    elseif (-not [string]::IsNullOrWhiteSpace($projectPathToRestore)) {
        # Start-Process joins ArgumentList values; retain quotes so paths with
        # spaces reach YMM4 as one project argument.
        $startParameters.ArgumentList = @("`"$projectPathToRestore`"")
    }
    $process = Start-Process @startParameters
    $ready = Wait-BridgeReady $process $expectedPluginVersion $projectPathToRestore

    [pscustomobject]@{
        status = 'updated_and_restarted'
        processId = $process.Id
        installedSha256 = $installedHash
        pluginVersion = $ready.health.pluginVersion
        ymm4Version = $ready.health.ymm4Version
        requiredCapabilities = $RequiredCapability
        autoRestoredNamedProject = -not [string]::IsNullOrWhiteSpace($projectPathToRestore)
        backup = $installation.backup
    } | ConvertTo-Json -Depth 4
}
finally {
    if ($null -ne $updateLock) {
        $updateLock.Dispose()
    }
}
