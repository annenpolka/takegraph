[CmdletBinding()]
param(
    [Parameter()]
    [string] $CliPath,

    [Parameter()]
    [string] $ExpectedProjectPattern = '\.ymmp$',

    [Parameter()]
    [int] $MinimumManagedItems = 0,

    [Parameter()]
    [switch] $RunSceneCapture,

    [Parameter()]
    [string] $SceneTaskPath,

    [Parameter()]
    [string] $ApprovedSceneDigest,

    [Parameter()]
    [ValidateRange(1, 16384)]
    [int] $ExpectedWidth = 1920,

    [Parameter()]
    [ValidateRange(1, 16384)]
    [int] $ExpectedHeight = 1080,

    [Parameter()]
    [ValidateRange(0, [int]::MaxValue)]
    [int[]] $Frame = @(0),

    [Parameter()]
    [ValidateRange(0, [int]::MaxValue)]
    [int] $Head = 0,

    [Parameter()]
    [string] $ProjectStateRoot,

    [Parameter()]
    [string] $EvidenceRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([string]::IsNullOrWhiteSpace($ProjectStateRoot)) {
    $ProjectStateRoot = Join-Path $repositoryRoot '.takegraph\project-store'
}
$ProjectStateRoot = [System.IO.Path]::GetFullPath($ProjectStateRoot)
if ([string]::IsNullOrWhiteSpace($CliPath)) {
    $CliPath = Join-Path $repositoryRoot 'target\debug\takegraph.exe'
}
if (-not (Test-Path -LiteralPath $CliPath -PathType Leaf)) {
    & cargo build -p takegraph-cli
    if ($LASTEXITCODE -ne 0) {
        throw "takegraph CLI build failed with exit code $LASTEXITCODE"
    }
}
$resolvedCli = (Resolve-Path -LiteralPath $CliPath).Path

if ([string]::IsNullOrWhiteSpace($EvidenceRoot)) {
    $EvidenceRoot = Join-Path $repositoryRoot '.takegraph\live-ymm4'
}
$runDirectory = Join-Path $EvidenceRoot ((Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
[System.IO.Directory]::CreateDirectory($runDirectory) | Out-Null

function Invoke-TakeGraphJson {
    param([Parameter(Mandatory)][string[]] $Arguments)

    $output = & $resolvedCli @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "takegraph $($Arguments -join ' ') failed:`n$($output -join [Environment]::NewLine)"
    }
    try {
        return ($output -join [Environment]::NewLine) | ConvertFrom-Json -Depth 100
    }
    catch {
        throw "takegraph returned invalid JSON for $($Arguments -join ' '):`n$($output -join [Environment]::NewLine)"
    }
}

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw $Message }
}

$health = Invoke-TakeGraphJson @('ymm4', 'health')
Assert-True ($health.status -eq 'running') 'YMM4 bridge health is not running.'
Assert-True ($health.protocolVersion -eq 2) "Expected protocol 2, got $($health.protocolVersion)."
Assert-True ($health.yMm4Version -eq '4.55.1.1' -or $health.ymm4Version -eq '4.55.1.1') "Expected YMM4 4.55.1.1."

$capabilities = Invoke-TakeGraphJson @('ymm4', 'capabilities')
$capabilitySet = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($capability in $capabilities.capabilities) { [void] $capabilitySet.Add([string] $capability) }
$requiredCapabilities = @(
    'readback_verification',
    'request_bound_receipts',
    'write_ahead_apply',
    'recovery_readback',
    'managed_audio',
    'managed_caption',
    'idempotent_apply',
    'undo_batch',
    'mutation_profile_ymm4_4_55_1_1',
    'unified_target_plan',
    'metadata_remark_detach',
    'project_checkpoint_verified',
    'native_voice_create',
    'native_voice_update_replace_preserving_user_state',
    'native_voice_delete',
    'native_voice_exact_wav_export',
    'native_voice_host_bound_provenance',
    'native_voice_remark_identity',
    'native_voice_bounded_duration',
    'scene_capture_native_png',
    'scene_capture_playhead_restore',
    'scene_capture_content_hash',
    'native_portrait_upsert',
    'native_face_upsert',
    'native_image_upsert',
    'native_video_upsert',
    'native_audio_upsert',
    'native_effect_typed_mutation',
    'native_template_instantiate'
)
$missingCapabilities = @($requiredCapabilities | Where-Object { -not $capabilitySet.Contains($_) })
Assert-True ($missingCapabilities.Count -eq 0) ("Missing required live capabilities: " + ($missingCapabilities -join ', '))
$renderCapabilityNames = @(
    'project_render',
    'project_render_cancel',
    'project_render_media_receipt'
)
$unexpectedRenderCapabilities = @($renderCapabilityNames | Where-Object { $capabilitySet.Contains($_) })
Assert-True ($unexpectedRenderCapabilities.Count -eq 0) (
    'Authoritative render must remain fail-closed without an exhaustive source dependency manifest: ' +
    ($unexpectedRenderCapabilities -join ', '))
$renderProfiles = Invoke-TakeGraphJson @('ymm4', 'render-profiles')
$bindableRenderProfiles = @($renderProfiles.profiles | Where-Object { $_.bindable -eq $true })
Assert-True ($bindableRenderProfiles.Count -eq 0) 'Render profile became bindable without exhaustive source dependency evidence.'
$unboundRenderProfiles = @($renderProfiles.profiles | Where-Object { $_.bindable -eq $false })
Assert-True ($unboundRenderProfiles.Count -eq 1) 'Expected exactly one explicit unbindable render profile.'
Assert-True (
    -not [string]::IsNullOrWhiteSpace([string] $unboundRenderProfiles[0].bindingError)) (
    'Unbindable render profile did not expose a fail-closed binding error.')

$snapshot = Invoke-TakeGraphJson @('ymm4', 'snapshot')
Assert-True (-not [string]::IsNullOrWhiteSpace([string] $snapshot.projectId)) 'Snapshot projectId is missing.'
Assert-True (-not [string]::IsNullOrWhiteSpace([string] $snapshot.sceneId)) 'Snapshot sceneId is missing.'
Assert-True ([string] $snapshot.fingerprint -match '^[0-9a-f]{64}$') 'Snapshot fingerprint is not SHA-256 shaped.'
Assert-True ([string] $snapshot.projectPath -match $ExpectedProjectPattern) "Project path did not match $ExpectedProjectPattern."
Assert-True (@($snapshot.managedItems).Count -ge $MinimumManagedItems) "Snapshot has fewer than $MinimumManagedItems managed items."
$canonical = Invoke-TakeGraphJson @('ymm4', 'canonical-head', '--state-root', $ProjectStateRoot)
Assert-True ($canonical.projectId -eq $snapshot.projectId) 'Canonical head is bound to another project.'
if ($PSBoundParameters.ContainsKey('Head')) {
    Assert-True ($Head -eq [int] $canonical.revision) "Requested head $Head does not match canonical head $($canonical.revision)."
}
$effectiveHead = [int] $canonical.revision
$expectedProjectArguments = @('--expected-project-id', [string] $canonical.projectId)

$controls = Invoke-TakeGraphJson @('ymm4', 'controls')
$descriptors = Invoke-TakeGraphJson (@('ymm4', 'native-extension-descriptors') + $expectedProjectArguments)
$targetCatalog = $descriptors.targetCatalog
Assert-True ([string] $targetCatalog.catalogDigest -match '^(sha256:)?[0-9a-f]{64}$') 'Descriptor catalog digest is invalid.'
Assert-True ($targetCatalog.projectId -eq $snapshot.projectId) 'Descriptor catalog is bound to another project.'
Assert-True ($targetCatalog.sceneId -eq $snapshot.sceneId) 'Descriptor catalog is bound to another scene.'

$credentialPath = if ($env:TAKEGRAPH_YMM4_CREDENTIALS) {
    $env:TAKEGRAPH_YMM4_CREDENTIALS
} else {
    Join-Path $env:LOCALAPPDATA 'TakeGraph\ymm4-bridge.json'
}
$credential = Get-Content -LiteralPath $credentialPath -Raw | ConvertFrom-Json
$headers = @{ 'x-takegraph-token' = [string] $credential.token }
$endpoint = ([string] $credential.endpoint).TrimEnd('/')
$recovery = Invoke-RestMethod -Uri ($endpoint + '/v2/recovery') -Headers $headers -TimeoutSec 10
Assert-True ($recovery.pending -eq 0) "Bridge has $($recovery.pending) pending recovery journal(s)."
Assert-True ($recovery.recoveryRequired -eq 0) "Bridge has $($recovery.recoveryRequired) recovery-required journal(s)."

$sceneEvidence = $null
$overallStatus = 'passed'
if ($RunSceneCapture) {
    if ([string]::IsNullOrWhiteSpace($SceneTaskPath)) {
        $profilePath = Join-Path $runDirectory 'scene-profile.json'
        $taskPath = Join-Path $runDirectory 'scene-inspection.json'
        @{
            expectedWidth = $ExpectedWidth
            expectedHeight = $ExpectedHeight
            blackLumaThreshold = 16
            blackPixelRatioPpm = 990000
            blankChannelSpanThreshold = 0
            safeArea = $null
            regions = @()
        } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $profilePath -Encoding utf8NoBOM

        $stageArguments = @('ymm4', 'scene-stage', '--profile', $profilePath, '--profile-id', 'live-driver-smoke', '--task', $taskPath, '--head', [string] $effectiveHead) + $expectedProjectArguments
        foreach ($requestedFrame in $Frame) { $stageArguments += @('--frame', [string] $requestedFrame) }
        $staged = Invoke-TakeGraphJson $stageArguments
        Assert-True ([string] $staged.digest -match '^[0-9a-f]{64}$') 'Scene stage digest is invalid.'
        $overallStatus = 'approval_required'
        $sceneEvidence = [pscustomobject]@{
            status = 'approval_required'
            taskPath = $taskPath
            profilePath = $profilePath
            digest = $staged.digest
            frames = $Frame
            expectedWidth = $ExpectedWidth
            expectedHeight = $ExpectedHeight
        }
    }
    else {
        Assert-True ([string] $ApprovedSceneDigest -match '^[0-9a-f]{64}$') 'ApprovedSceneDigest must be the exact 64-character staged digest.'
        $taskPath = (Resolve-Path -LiteralPath $SceneTaskPath).Path
        $profilePath = Join-Path (Split-Path -Parent $taskPath) 'scene-profile.json'
        Assert-True (Test-Path -LiteralPath $profilePath -PathType Leaf) "Scene profile is missing beside $taskPath."
        $task = Get-Content -LiteralPath $taskPath -Raw | ConvertFrom-Json -Depth 100
        Assert-True ($task.plan.digest -eq $ApprovedSceneDigest) 'ApprovedSceneDigest does not match the persisted scene plan.'
        $expectedCaptureCount = @($task.plan.samples).Count
        $taskStatus = [string] $task.receipt.status
        if ($taskStatus -eq 'staged') {
            $approved = Invoke-TakeGraphJson (@('ymm4', 'scene-approve', '--task', $taskPath, '--digest', $ApprovedSceneDigest, '--current-profile', $profilePath, '--head', [string] $effectiveHead) + $expectedProjectArguments)
            $approvalStatus = [string] $approved.receipt.status
            $captured = Invoke-TakeGraphJson (@('ymm4', 'scene-capture', '--task', $taskPath, '--current-profile', $profilePath, '--head', [string] $effectiveHead) + $expectedProjectArguments)
            $replayed = Invoke-TakeGraphJson (@('ymm4', 'scene-replay', '--task', $taskPath, '--current-profile', $profilePath, '--head', [string] $effectiveHead) + $expectedProjectArguments)
        }
        elseif ($taskStatus -eq 'approved') {
            $approvalStatus = 'approved'
            $captured = Invoke-TakeGraphJson (@('ymm4', 'scene-capture', '--task', $taskPath, '--current-profile', $profilePath, '--head', [string] $effectiveHead) + $expectedProjectArguments)
            $replayed = Invoke-TakeGraphJson (@('ymm4', 'scene-replay', '--task', $taskPath, '--current-profile', $profilePath, '--head', [string] $effectiveHead) + $expectedProjectArguments)
        }
        elseif ($taskStatus -in @('captured', 'reviewed', 'accepted', 'rejected')) {
            $approvalStatus = 'approved'
            $replayed = Invoke-TakeGraphJson (@('ymm4', 'scene-replay', '--task', $taskPath, '--current-profile', $profilePath, '--head', [string] $effectiveHead) + $expectedProjectArguments)
            $captured = $replayed
        }
        else {
            throw "Scene task cannot be captured or replayed from status $taskStatus."
        }
        Assert-True ($captured.receipt.status -eq 'captured') "Scene capture status is $($captured.receipt.status)."
        Assert-True ($captured.receipt.captureEvidence.transientStateRestored -eq $true) 'Scene capture did not prove transient-state restoration.'
        Assert-True ($captured.receipt.captureEvidence.projectDirtyBefore -eq $captured.receipt.captureEvidence.projectDirtyAfter) 'Scene capture changed project dirty state.'
        Assert-True (@($captured.images).Count -eq $expectedCaptureCount) 'Scene capture returned an unexpected image count.'
        Assert-True ($replayed.authenticatedReplayPerformed -eq $true) 'Scene receipt was not authenticated by replay.'
        $sceneEvidence = [pscustomobject]@{
            status = 'captured'
            taskPath = $taskPath
            profilePath = $profilePath
            digest = $ApprovedSceneDigest
            approvalStatus = $approvalStatus
            captureStatus = $captured.receipt.status
            transientStateRestored = $captured.receipt.captureEvidence.transientStateRestored
            images = $captured.images
            replayed = $replayed.authenticatedReplayPerformed
        }
    }
}

$report = [pscustomobject]@{
    status = $overallStatus
    testedAt = [DateTimeOffset]::Now.ToString('o')
    health = $health
    capabilityCount = $capabilitySet.Count
    requiredCapabilities = $requiredCapabilities
    project = [pscustomobject]@{
        projectId = $snapshot.projectId
        projectName = $snapshot.projectName
        projectPath = $snapshot.projectPath
        sceneId = $snapshot.sceneId
        fps = $snapshot.fps
        fingerprint = $snapshot.fingerprint
        managedItemCount = @($snapshot.managedItems).Count
        unmanagedContextCount = $snapshot.unmanagedContextCount
    }
    controlCount = @($controls.commands).Count
    descriptorCount = @($targetCatalog.descriptors).Count
    descriptorCatalogDigest = $targetCatalog.catalogDigest
    render = [pscustomobject]@{
        capabilityAdvertised = $capabilitySet.Contains('project_render')
        bindableProfileCount = $bindableRenderProfiles.Count
        profiles = $renderProfiles.profiles
        descriptorSetDigest = $renderProfiles.descriptorSetDigest
    }
    recovery = [pscustomobject]@{
        pending = $recovery.pending
        recoveryRequired = $recovery.recoveryRequired
    }
    scene = $sceneEvidence
}
$reportPath = Join-Path $runDirectory 'live-report.json'
$report | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $reportPath -Encoding utf8NoBOM
$report | Add-Member -NotePropertyName evidencePath -NotePropertyValue $reportPath
$report | ConvertTo-Json -Depth 100
