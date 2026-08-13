module ymm4ApplyProtocol {
  type Status =
    | Draft
    | Previewable
    | Approved
    | Applying
    | Applied
    | Verified
    | Committed
    | Conflicted
    | Failed
    | Crashed
    | Recovering
    | RolledBack

  type RequestBinding = {
    operationId: int,
    requestDigest: int,
    projectId: int,
    preconditionDigest: int,
    baseRevision: int,
    externalFingerprint: int,
    capabilityRevision: int,
  }

  var headRevision: int
  var externalFingerprint: int
  var activeProjectId: int
  var capabilityRevision: int
  var requestBinding: RequestBinding
  var approvedBinding: RequestBinding
  var status: Status
  var applyCount: int
  var mutatedSteps: int
  var walPrepared: bool
  var walBinding: RequestBinding
  var hasReceipt: bool
  var receiptBinding: RequestBinding
  var receiptVerified: bool

  action init = all {
    headRevision' = 0,
    externalFingerprint' = 0,
    activeProjectId' = 1,
    capabilityRevision' = 1,
    requestBinding' = {
      operationId: 1,
      requestDigest: 1,
      projectId: 1,
      preconditionDigest: 1,
      baseRevision: 0,
      externalFingerprint: 0,
      capabilityRevision: 1,
    },
    approvedBinding' = {
      operationId: -1,
      requestDigest: -1,
      projectId: -1,
      preconditionDigest: -1,
      baseRevision: -1,
      externalFingerprint: -1,
      capabilityRevision: -1,
    },
    status' = Draft,
    applyCount' = 0,
    mutatedSteps' = 0,
    walPrepared' = false,
    walBinding' = {
      operationId: -1,
      requestDigest: -1,
      projectId: -1,
      preconditionDigest: -1,
      baseRevision: -1,
      externalFingerprint: -1,
      capabilityRevision: -1,
    },
    hasReceipt' = false,
    receiptBinding' = {
      operationId: -1,
      requestDigest: -1,
      projectId: -1,
      preconditionDigest: -1,
      baseRevision: -1,
      externalFingerprint: -1,
      capabilityRevision: -1,
    },
    receiptVerified' = false,
  }

  action stage = all {
    status == Draft,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding
      .with("projectId", activeProjectId)
      .with("preconditionDigest", requestBinding.preconditionDigest + 1)
      .with("baseRevision", headRevision)
      .with("externalFingerprint", externalFingerprint)
      .with("capabilityRevision", capabilityRevision),
    approvedBinding' = approvedBinding,
    status' = Previewable,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action refreshPreview = all {
    status == Previewable,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding
      .with("requestDigest", requestBinding.requestDigest + 1)
      .with("projectId", activeProjectId)
      .with("preconditionDigest", requestBinding.preconditionDigest + 1)
      .with("baseRevision", headRevision)
      .with("externalFingerprint", externalFingerprint)
      .with("capabilityRevision", capabilityRevision),
    approvedBinding' = approvedBinding,
    status' = Previewable,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action approve = all {
    status == Previewable,
    requestBinding.projectId == activeProjectId,
    requestBinding.baseRevision == headRevision,
    requestBinding.externalFingerprint == externalFingerprint,
    requestBinding.capabilityRevision == capabilityRevision,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = requestBinding,
    status' = Approved,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action mutateRequestAfterApproval = all {
    status == Approved,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding
      .with("requestDigest", requestBinding.requestDigest + 1)
      .with("preconditionDigest", requestBinding.preconditionDigest + 1),
    approvedBinding' = approvedBinding,
    status' = Previewable,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action rotateOperationAfterApproval = all {
    status == Approved,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding
      .with("operationId", requestBinding.operationId + 1)
      .with("requestDigest", requestBinding.requestDigest + 1),
    approvedBinding' = approvedBinding,
    status' = Previewable,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action retargetProjectAfterApproval = all {
    status == Approved,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId + 1,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding
      .with("requestDigest", requestBinding.requestDigest + 1)
      .with("projectId", activeProjectId + 1)
      .with("preconditionDigest", requestBinding.preconditionDigest + 1),
    approvedBinding' = approvedBinding,
    status' = Previewable,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action capabilityChangeBeforeApply = all {
    any { status == Previewable, status == Approved },
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision + 1,
    requestBinding' = requestBinding
      .with("requestDigest", requestBinding.requestDigest + 1)
      .with("preconditionDigest", requestBinding.preconditionDigest + 1)
      .with("capabilityRevision", capabilityRevision + 1),
    approvedBinding' = approvedBinding,
    status' = Previewable,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action externalEdit = all {
    any { status == Draft, status == Previewable, status == Approved },
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint + 1,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = status,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action concurrentCommit = all {
    any { status == Previewable, status == Approved },
    headRevision' = headRevision + 1,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = status,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action switchActiveProject = all {
    status == Approved,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId + 1,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Approved,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  // This transition durably prepares the WAL; it performs no YMM4 mutation.
  action beginApply = all {
    status == Approved,
    approvedBinding == requestBinding,
    requestBinding.projectId == activeProjectId,
    requestBinding.baseRevision == headRevision,
    requestBinding.externalFingerprint == externalFingerprint,
    requestBinding.capabilityRevision == capabilityRevision,
    not(walPrepared),
    not(hasReceipt),
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Applying,
    applyCount' = applyCount,
    mutatedSteps' = 0,
    walPrepared' = true,
    walBinding' = requestBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action markConflict = all {
    status == Approved,
    any {
      requestBinding.projectId != activeProjectId,
      requestBinding.baseRevision != headRevision,
      requestBinding.externalFingerprint != externalFingerprint,
    },
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Conflicted,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  // Two durable steps stand in for a multi-item batch and expose partial failure.
  action applyOneStep = all {
    status == Applying,
    walPrepared,
    walBinding == approvedBinding,
    approvedBinding == requestBinding,
    walBinding.projectId == activeProjectId,
    walBinding.capabilityRevision == capabilityRevision,
    externalFingerprint == walBinding.externalFingerprint + mutatedSteps,
    mutatedSteps < 2,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint + 1,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Applying,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps + 1,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action finishApply = all {
    status == Applying,
    walPrepared,
    walBinding == approvedBinding,
    approvedBinding == requestBinding,
    mutatedSteps == 2,
    externalFingerprint == walBinding.externalFingerprint + mutatedSteps,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Applied,
    applyCount' = applyCount + 1,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = true,
    receiptBinding' = walBinding,
    receiptVerified' = false,
  }

  action verifyReadback = all {
    status == Applied,
    hasReceipt,
    receiptBinding == walBinding,
    externalFingerprint == walBinding.externalFingerprint + mutatedSteps,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Verified,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = true,
  }

  action commit = all {
    status == Verified,
    hasReceipt,
    receiptVerified,
    receiptBinding == approvedBinding,
    headRevision' = headRevision + 1,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Committed,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action replayOperation = all {
    hasReceipt,
    receiptBinding == requestBinding,
    any { status == Applied, status == Verified, status == Committed },
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = status,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action crashDuringApply = all {
    status == Applying,
    walPrepared,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Crashed,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action failApply = all {
    status == Applying,
    walPrepared,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Failed,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action capabilityChangeDuringApply = all {
    status == Applying,
    walPrepared,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision + 1,
    requestBinding' = requestBinding
      .with("requestDigest", requestBinding.requestDigest + 1)
      .with("preconditionDigest", requestBinding.preconditionDigest + 1)
      .with("capabilityRevision", capabilityRevision + 1),
    approvedBinding' = approvedBinding,
    status' = Failed,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action beginRecovery = all {
    any { status == Crashed, status == Failed },
    walPrepared,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Recovering,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  // WAL progress is reduced one step at a time, so recovery can itself crash.
  action rollbackOneStep = all {
    status == Recovering,
    walPrepared,
    mutatedSteps > 0,
    externalFingerprint == walBinding.externalFingerprint + mutatedSteps,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint - 1,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Recovering,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps - 1,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action crashDuringRecovery = all {
    status == Recovering,
    walPrepared,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = Crashed,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action finishRollback = all {
    status == Recovering,
    walPrepared,
    mutatedSteps == 0,
    externalFingerprint == walBinding.externalFingerprint,
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = RolledBack,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = false,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = false,
  }

  action stutter = all {
    any { status == Committed, status == Conflicted, status == RolledBack },
    headRevision' = headRevision,
    externalFingerprint' = externalFingerprint,
    activeProjectId' = activeProjectId,
    capabilityRevision' = capabilityRevision,
    requestBinding' = requestBinding,
    approvedBinding' = approvedBinding,
    status' = status,
    applyCount' = applyCount,
    mutatedSteps' = mutatedSteps,
    walPrepared' = walPrepared,
    walBinding' = walBinding,
    hasReceipt' = hasReceipt,
    receiptBinding' = receiptBinding,
    receiptVerified' = receiptVerified,
  }

  action step = any {
    stage,
    refreshPreview,
    approve,
    mutateRequestAfterApproval,
    rotateOperationAfterApproval,
    retargetProjectAfterApproval,
    capabilityChangeBeforeApply,
    externalEdit,
    concurrentCommit,
    switchActiveProject,
    beginApply,
    markConflict,
    applyOneStep,
    finishApply,
    verifyReadback,
    commit,
    replayOperation,
    crashDuringApply,
    failApply,
    capabilityChangeDuringApply,
    beginRecovery,
    rollbackOneStep,
    crashDuringRecovery,
    finishRollback,
    stutter,
  }

  val approvalIsRequestBound =
    status != Approved or approvedBinding == requestBinding

  val capabilityRevisionIsFresh =
    status != Approved
      or approvedBinding.capabilityRevision == capabilityRevision

  val walPrecedesMutation =
    mutatedSteps == 0
      or and { walPrepared, walBinding == approvedBinding }

  val applyingUsesBoundWal =
    status != Applying
      or and {
        walPrepared,
        walBinding == approvedBinding,
        approvedBinding == requestBinding,
        walBinding.projectId == activeProjectId,
        walBinding.capabilityRevision == capabilityRevision,
      }

  val recoveryHasWal =
    not(any { status == Crashed, status == Failed, status == Recovering })
      or walPrepared

  val receiptIsRequestBound =
    not(hasReceipt)
      or and {
        receiptBinding == walBinding,
        receiptBinding == approvedBinding,
      }

  val applyIsAtMostOnce = applyCount <= 1

  val rollbackRestoresExternalState =
    status != RolledBack
      or and {
        mutatedSteps == 0,
        externalFingerprint == walBinding.externalFingerprint,
        not(receiptVerified),
      }

  val committedOnlyAfterVerifiedReceipt =
    status != Committed
      or and {
        hasReceipt,
        receiptVerified,
        receiptBinding == approvedBinding,
        applyCount == 1,
        mutatedSteps == 2,
      }

  val safety = and {
    approvalIsRequestBound,
    capabilityRevisionIsFresh,
    walPrecedesMutation,
    applyingUsesBoundWal,
    recoveryHasWal,
    receiptIsRequestBound,
    applyIsAtMostOnce,
    rollbackRestoresExternalState,
    committedOnlyAfterVerifiedReceipt,
  }
}

// Phase 3-5 extension: bounded native apply, explicit manual recovery, and
// source-bound Tier-2 output tasks. This is a separate module so the deployed
// v2 apply protocol above and its existing safety suite remain stable while the
// planned lifecycle contracts are refined and implemented.
module ymm4LifecycleProtocol {
  type ApplyStatus =
    | ApplyIdle
    | ApplyApplying
    | ApplyAppliedUnverified
    | ApplyVerified
    | ApplyCommitted
    | ApplyFailed
    | ApplyRecovering
    | ApplyRolledBack
    | ApplyRecoveryRequired

  type CheckpointStatus =
    | CheckpointIdle
    | CheckpointSaving
    | CheckpointVerified
    | CheckpointFailed
    | CheckpointStale

  type InspectionStatus =
    | InspectionIdle
    | InspectionStaged
    | InspectionCapturing
    | InspectionCaptured
    | InspectionAccepted
    | InspectionRejected
    | InspectionFailed
    | InspectionStale

  type RenderStatus =
    | RenderIdle
    | RenderStaged
    | RenderRunning
    | RenderCompleted
    | RenderCancelled
    | RenderFailed
    | RenderStale
    | RenderRecoveryRequired

  type CoreState = {
    applyStatus: ApplyStatus,
    headRevision: int,
    committedApplyCount: int,
    sceneEpoch: int,
    approvedChangeBudget: int,
    actualManagedChanges: int,
    walPrepared: bool,
    readbackDurable: bool,
    rollbackFailed: bool,
    recoveryRequiredObserved: bool,
    nativeUndoCount: int,
    unmanagedState: int,
    unmanagedEditCount: int,
    hasExistingPath: bool,
  }

  type CheckpointTask = {
    status: CheckpointStatus,
    sourceRevision: int,
    sourceSceneEpoch: int,
  }

  type InspectionTask = {
    status: InspectionStatus,
    sourceRevision: int,
    sourceSceneEpoch: int,
    sourceProfileRevision: int,
    activeProfileRevision: int,
  }

  type RenderTask = {
    status: RenderStatus,
    sourceRevision: int,
    sourceSceneEpoch: int,
    sourceProfileRevision: int,
    activeProfileRevision: int,
    checkpointBound: bool,
    checkpointBytesFresh: bool,
    encoderBindingFresh: bool,
    encoderFilesLeased: bool,
    overwriteWalPrepared: bool,
    candidateNamespaceLeased: bool,
    candidateVerified: bool,
    candidatePublished: bool,
    originalRestored: bool,
  }

  var core: CoreState
  var checkpoint: CheckpointTask
  var inspection: InspectionTask
  var render: RenderTask

  pure def checkpointHasFreshnessContract(status: CheckpointStatus): bool =
    any { status == CheckpointSaving, status == CheckpointVerified }

  pure def inspectionHasFreshnessContract(status: InspectionStatus): bool =
    any {
      status == InspectionStaged,
      status == InspectionCapturing,
      status == InspectionCaptured,
      status == InspectionAccepted,
      status == InspectionRejected,
    }

  pure def renderHasFreshnessContract(status: RenderStatus): bool =
    any {
      status == RenderStaged,
      status == RenderRunning,
      status == RenderCompleted,
    }

  // The 4.55.1.1 resource list is not a proven exhaustive contract for every
  // asset/configuration the child encoder may read. This gate stays false until
  // a SHA-256-bound, no-follow, whole-process-leased dependency closure exists.
  pure def authoritativeRenderSourceBindingAvailable: bool = false

  pure def invalidateCheckpoint(task: CheckpointTask): CheckpointTask =
    if (checkpointHasFreshnessContract(task.status))
      task.with("status", CheckpointStale)
    else
      task

  pure def invalidateInspection(task: InspectionTask): InspectionTask =
    if (inspectionHasFreshnessContract(task.status))
      task.with("status", InspectionStale)
    else
      task

  pure def invalidateRender(task: RenderTask): RenderTask =
    if (renderHasFreshnessContract(task.status))
      task.with("status", RenderStale).with("originalRestored", true)
    else
      task

  pure def projectStateIsUnambiguous(status: ApplyStatus): bool =
    not(any {
      status == ApplyApplying,
      status == ApplyAppliedUnverified,
      status == ApplyFailed,
      status == ApplyRecovering,
      status == ApplyRecoveryRequired,
    })

  pure def applyOwnsProjectGate(status: ApplyStatus): bool =
    any {
      status == ApplyApplying,
      status == ApplyAppliedUnverified,
      status == ApplyFailed,
      status == ApplyRecovering,
    }

  action init = all {
    core' = {
      applyStatus: ApplyIdle,
      headRevision: 0,
      committedApplyCount: 0,
      sceneEpoch: 0,
      approvedChangeBudget: 2,
      actualManagedChanges: 0,
      walPrepared: false,
      readbackDurable: false,
      rollbackFailed: false,
      recoveryRequiredObserved: false,
      nativeUndoCount: 0,
      unmanagedState: 100,
      unmanagedEditCount: 0,
      hasExistingPath: false,
    },
    checkpoint' = {
      status: CheckpointIdle,
      sourceRevision: -1,
      sourceSceneEpoch: -1,
    },
    inspection' = {
      status: InspectionIdle,
      sourceRevision: -1,
      sourceSceneEpoch: -1,
      sourceProfileRevision: -1,
      activeProfileRevision: 1,
    },
    render' = {
      status: RenderIdle,
      sourceRevision: -1,
      sourceSceneEpoch: -1,
      sourceProfileRevision: -1,
      activeProfileRevision: 1,
      checkpointBound: false,
      checkpointBytesFresh: false,
      encoderBindingFresh: false,
      encoderFilesLeased: false,
      overwriteWalPrepared: false,
      candidateNamespaceLeased: false,
      candidateVerified: false,
      candidatePublished: false,
      originalRestored: true,
    },
  }

  // Budget selection is a preview/approval concern and is frozen by beginApply.
  action approveOneChangeBudget = all {
    core.applyStatus == ApplyIdle,
    core.actualManagedChanges == 0,
    core' = core.with("approvedChangeBudget", 1),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action approveTwoChangeBudget = all {
    core.applyStatus == ApplyIdle,
    core.actualManagedChanges == 0,
    core' = core.with("approvedChangeBudget", 2),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action beginApply = all {
    core.applyStatus == ApplyIdle,
    core.hasExistingPath,
    core.actualManagedChanges == 0,
    core.approvedChangeBudget > 0,
    checkpoint.status != CheckpointSaving,
    inspection.status != InspectionCapturing,
    render.status != RenderRunning,
    core' = core
      .with("applyStatus", ApplyApplying)
      .with("walPrepared", true)
      .with("readbackDurable", false)
      .with("rollbackFailed", false),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  // Up to three concrete changes expose both in-budget and over-budget traces.
  action applyOneManagedChange = all {
    core.applyStatus == ApplyApplying,
    core.walPrepared,
    core.actualManagedChanges < 3,
    core' = core
      .with("actualManagedChanges", core.actualManagedChanges + 1)
      .with("sceneEpoch", core.sceneEpoch + 1),
    checkpoint' = invalidateCheckpoint(checkpoint),
    inspection' = invalidateInspection(inspection),
    render' = invalidateRender(render),
  }

  // `applied_unverified` is written only after semantic readback succeeds.
  // Once this recovery fact is durable, compensating rollback would destroy
  // an already-observed approved result; recovery may only promote the exact
  // result to Verified or stop in RecoveryRequired.
  action persistSuccessfulReadback = all {
    core.applyStatus == ApplyApplying,
    core.walPrepared,
    core.actualManagedChanges > 0,
    core.actualManagedChanges <= core.approvedChangeBudget,
    core' = core
      .with("applyStatus", ApplyAppliedUnverified)
      .with("readbackDurable", true),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action verifyInBudgetReadback = all {
    core.applyStatus == ApplyAppliedUnverified,
    core.actualManagedChanges <= core.approvedChangeBudget,
    core' = core.with("applyStatus", ApplyVerified),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action rejectOverBudgetReadback = all {
    core.applyStatus == ApplyApplying,
    core.actualManagedChanges > core.approvedChangeBudget,
    core' = core.with("applyStatus", ApplyFailed),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action failApply = all {
    core.applyStatus == ApplyApplying,
    core.walPrepared,
    core' = core.with("applyStatus", ApplyFailed),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action commitVerifiedApply = all {
    core.applyStatus == ApplyVerified,
    core.actualManagedChanges <= core.approvedChangeBudget,
    core' = core
      .with("applyStatus", ApplyCommitted)
      .with("headRevision", core.headRevision + 1)
      .with("committedApplyCount", core.committedApplyCount + 1),
    checkpoint' = invalidateCheckpoint(checkpoint),
    inspection' = invalidateInspection(inspection),
    render' = invalidateRender(render),
  }

  action failAppliedUnverifiedRecovery = all {
    core.applyStatus == ApplyAppliedUnverified,
    core.readbackDurable,
    core.walPrepared,
    core' = core
      .with("applyStatus", ApplyRecoveryRequired)
      .with("recoveryRequiredObserved", true),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action beginRecovery = all {
    core.applyStatus == ApplyFailed,
    core.walPrepared,
    core' = core.with("applyStatus", ApplyRecovering),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action rollbackOneManagedChange = all {
    core.applyStatus == ApplyRecovering,
    core.walPrepared,
    core.actualManagedChanges > 0,
    core' = core
      .with("actualManagedChanges", core.actualManagedChanges - 1)
      .with("sceneEpoch", core.sceneEpoch + 1),
    checkpoint' = invalidateCheckpoint(checkpoint),
    inspection' = invalidateInspection(inspection),
    render' = invalidateRender(render),
  }

  action finishRollback = all {
    core.applyStatus == ApplyRecovering,
    core.actualManagedChanges == 0,
    core' = core
      .with("applyStatus", ApplyRolledBack)
      .with("walPrepared", false),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  // A failed compensating write leaves a durable, terminal manual-recovery state.
  action failRollback = all {
    core.applyStatus == ApplyRecovering,
    core.walPrepared,
    core.actualManagedChanges > 0,
    core' = core
      .with("applyStatus", ApplyRecoveryRequired)
      .with("rollbackFailed", true)
      .with("recoveryRequiredObserved", true),
    checkpoint' = invalidateCheckpoint(checkpoint),
    inspection' = invalidateInspection(inspection),
    render' = invalidateRender(render),
  }

  action prepareNextApply = all {
    any {
      core.applyStatus == ApplyCommitted,
      core.applyStatus == ApplyRolledBack,
    },
    core' = core
      .with("applyStatus", ApplyIdle)
      .with("actualManagedChanges", 0)
      .with("walPrepared", false)
      .with("readbackDurable", false)
      .with("rollbackFailed", false),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  // This represents an out-of-scope host edit. TakeGraph never rewrites it.
  action externalUnmanagedEdit = all {
    core' = core
      .with("unmanagedState", core.unmanagedState + 1)
      .with("unmanagedEditCount", core.unmanagedEditCount + 1),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  // A host-native Undo changes the observed scene, but never canonical history.
  action manualNativeUndo = all {
    core.headRevision > 0,
    projectStateIsUnambiguous(core.applyStatus),
    core' = core
      .with("nativeUndoCount", core.nativeUndoCount + 1)
      .with("sceneEpoch", core.sceneEpoch + 1),
    checkpoint' = invalidateCheckpoint(checkpoint),
    inspection' = invalidateInspection(inspection),
    render' = invalidateRender(render),
  }

  action externalRelevantSceneEdit = all {
    projectStateIsUnambiguous(core.applyStatus),
    core' = core.with("sceneEpoch", core.sceneEpoch + 1),
    checkpoint' = invalidateCheckpoint(checkpoint),
    inspection' = invalidateInspection(inspection),
    render' = invalidateRender(render),
  }

  action beginCheckpoint = all {
    projectStateIsUnambiguous(core.applyStatus),
    core.hasExistingPath,
    not(checkpoint.status == CheckpointSaving),
    inspection.status != InspectionCapturing,
    render.status != RenderRunning,
    core' = core,
    checkpoint' = checkpoint
      .with("status", CheckpointSaving)
      .with("sourceRevision", core.headRevision)
      .with("sourceSceneEpoch", core.sceneEpoch),
    inspection' = inspection,
    render' = render,
  }

  action completeCheckpoint = all {
    checkpoint.status == CheckpointSaving,
    checkpoint.sourceRevision == core.headRevision,
    checkpoint.sourceSceneEpoch == core.sceneEpoch,
    core' = core,
    checkpoint' = checkpoint.with("status", CheckpointVerified),
    inspection' = inspection,
    render' = render,
  }

  action failCheckpoint = all {
    checkpoint.status == CheckpointSaving,
    core' = core,
    checkpoint' = checkpoint.with("status", CheckpointFailed),
    inspection' = inspection,
    render' = render,
  }

  action stageInspection = all {
    projectStateIsUnambiguous(core.applyStatus),
    not(any {
      inspection.status == InspectionStaged,
      inspection.status == InspectionCapturing,
    }),
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection
      .with("status", InspectionStaged)
      .with("sourceRevision", core.headRevision)
      .with("sourceSceneEpoch", core.sceneEpoch)
      .with("sourceProfileRevision", inspection.activeProfileRevision),
    render' = render,
  }

  action beginInspectionCapture = all {
    inspection.status == InspectionStaged,
    projectStateIsUnambiguous(core.applyStatus),
    checkpoint.status != CheckpointSaving,
    render.status != RenderRunning,
    inspection.sourceRevision == core.headRevision,
    inspection.sourceSceneEpoch == core.sceneEpoch,
    inspection.sourceProfileRevision == inspection.activeProfileRevision,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection.with("status", InspectionCapturing),
    render' = render,
  }

  action completeInspectionCapture = all {
    inspection.status == InspectionCapturing,
    inspection.sourceRevision == core.headRevision,
    inspection.sourceSceneEpoch == core.sceneEpoch,
    inspection.sourceProfileRevision == inspection.activeProfileRevision,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection.with("status", InspectionCaptured),
    render' = render,
  }

  action acceptInspection = all {
    inspection.status == InspectionCaptured,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection.with("status", InspectionAccepted),
    render' = render,
  }

  action rejectInspection = all {
    inspection.status == InspectionCaptured,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection.with("status", InspectionRejected),
    render' = render,
  }

  action failInspection = all {
    inspection.status == InspectionCapturing,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection.with("status", InspectionFailed),
    render' = render,
  }

  action changeCaptureProfile = all {
    core' = core,
    checkpoint' = checkpoint,
    inspection' = invalidateInspection(inspection)
      .with("activeProfileRevision", inspection.activeProfileRevision + 1),
    render' = render,
  }

  action stageRender = all {
    authoritativeRenderSourceBindingAvailable,
    projectStateIsUnambiguous(core.applyStatus),
    checkpoint.status == CheckpointVerified,
    checkpoint.sourceRevision == core.headRevision,
    checkpoint.sourceSceneEpoch == core.sceneEpoch,
    not(any { render.status == RenderStaged, render.status == RenderRunning }),
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render
      .with("status", RenderStaged)
      .with("sourceRevision", core.headRevision)
      .with("sourceSceneEpoch", core.sceneEpoch)
      .with("sourceProfileRevision", render.activeProfileRevision)
      .with("checkpointBound", true)
      .with("checkpointBytesFresh", true)
      .with("encoderBindingFresh", true)
      .with("encoderFilesLeased", false)
      .with("overwriteWalPrepared", true)
      .with("candidateNamespaceLeased", true)
      .with("candidateVerified", false)
      .with("candidatePublished", false)
      .with("originalRestored", false),
  }

  action beginRender = all {
    render.status == RenderStaged,
    projectStateIsUnambiguous(core.applyStatus),
    checkpoint.status != CheckpointSaving,
    inspection.status != InspectionCapturing,
    render.sourceRevision == core.headRevision,
    render.sourceSceneEpoch == core.sceneEpoch,
    render.sourceProfileRevision == render.activeProfileRevision,
    render.checkpointBound,
    render.checkpointBytesFresh,
    render.encoderBindingFresh,
    render.overwriteWalPrepared,
    render.candidateNamespaceLeased,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render
      .with("status", RenderRunning)
      .with("encoderFilesLeased", true),
  }

  action completeRender = all {
    render.status == RenderRunning,
    render.sourceRevision == core.headRevision,
    render.sourceSceneEpoch == core.sceneEpoch,
    render.sourceProfileRevision == render.activeProfileRevision,
    render.encoderBindingFresh,
    render.encoderFilesLeased,
    render.candidateNamespaceLeased,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render
      .with("status", RenderCompleted)
      .with("candidateVerified", true)
      .with("candidatePublished", true),
  }

  action cancelRender = all {
    any { render.status == RenderStaged, render.status == RenderRunning },
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render.with("status", RenderCancelled).with("originalRestored", true),
  }

  action failRender = all {
    any { render.status == RenderStaged, render.status == RenderRunning },
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render.with("status", RenderFailed).with("originalRestored", true),
  }

  action driftRenderCheckpointBytes = all {
    any { render.status == RenderStaged, render.status == RenderRunning },
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render
      .with("status", RenderStale)
      .with("checkpointBytesFresh", false)
      .with("originalRestored", true),
  }

  // The selected writer, resolved arguments, setting bytes, or encoder
  // binaries changed after approval. No child encoder may start from the
  // stale binding, and the overwrite WAL restores the original output.
  action driftRenderEncoderBinding = all {
    any { render.status == RenderStaged, render.status == RenderRunning },
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render
      .with("status", RenderStale)
      .with("encoderBindingFresh", false)
      .with("originalRestored", true),
  }

  action failRenderRecovery = all {
    any { render.status == RenderStaged, render.status == RenderRunning },
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render.with("status", RenderRecoveryRequired),
  }

  action changeRenderProfile = all {
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = invalidateRender(render)
      .with("activeProfileRevision", render.activeProfileRevision + 1)
      .with("encoderBindingFresh", false),
  }

  action recoveryRequiredStutter = all {
    core.applyStatus == ApplyRecoveryRequired,
    core' = core,
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  // An operator may release the write gate without claiming rollback
  // succeeded. Leftover target items become unmanaged drift.
  action gainExistingPath = all {
    not(core.hasExistingPath),
    core' = core.with("hasExistingPath", true),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action loseExistingPath = all {
    any {
      core.applyStatus == ApplyIdle,
      core.applyStatus == ApplyRolledBack,
    },
    core.hasExistingPath,
    core' = core.with("hasExistingPath", false),
    checkpoint' = invalidateCheckpoint(checkpoint),
    inspection' = inspection,
    render' = render,
  }

  action acknowledgeRecoveryRequired = all {
    core.applyStatus == ApplyRecoveryRequired,
    core.recoveryRequiredObserved,
    core' = core
      .with("applyStatus", ApplyIdle)
      .with("actualManagedChanges", 0)
      .with("walPrepared", false)
      .with("readbackDurable", false)
      .with("rollbackFailed", false)
      .with("recoveryRequiredObserved", false),
    checkpoint' = checkpoint,
    inspection' = inspection,
    render' = render,
  }

  action step = any {
    approveOneChangeBudget,
    approveTwoChangeBudget,
    beginApply,
    applyOneManagedChange,
    persistSuccessfulReadback,
    verifyInBudgetReadback,
    rejectOverBudgetReadback,
    failApply,
    commitVerifiedApply,
    failAppliedUnverifiedRecovery,
    beginRecovery,
    rollbackOneManagedChange,
    finishRollback,
    failRollback,
    prepareNextApply,
    externalUnmanagedEdit,
    manualNativeUndo,
    externalRelevantSceneEdit,
    beginCheckpoint,
    completeCheckpoint,
    failCheckpoint,
    stageInspection,
    beginInspectionCapture,
    completeInspectionCapture,
    acceptInspection,
    rejectInspection,
    failInspection,
    changeCaptureProfile,
    stageRender,
    beginRender,
    completeRender,
    cancelRender,
    failRender,
    driftRenderCheckpointBytes,
    driftRenderEncoderBinding,
    failRenderRecovery,
    changeRenderProfile,
    recoveryRequiredStutter,
    gainExistingPath,
    loseExistingPath,
    acknowledgeRecoveryRequired,
  }

  val recoveryRequiredHasDurableCause =
    core.applyStatus != ApplyRecoveryRequired
      or and {
        core.walPrepared,
        core.actualManagedChanges > 0,
        core.recoveryRequiredObserved,
        any { core.rollbackFailed, core.readbackDurable },
      }

  val durableSuccessfulReadbackCannotRollBack =
    not(core.readbackDurable)
      or not(any {
        core.applyStatus == ApplyFailed,
        core.applyStatus == ApplyRecovering,
        core.applyStatus == ApplyRolledBack,
      })

  val recoveryRequiredIsTerminal =
    core.recoveryRequiredObserved == (core.applyStatus == ApplyRecoveryRequired)

  val verifiedApplyWithinApprovedBudget =
    not(any {
      core.applyStatus == ApplyVerified,
      core.applyStatus == ApplyCommitted,
    })
      or and {
        core.actualManagedChanges > 0,
        core.actualManagedChanges <= core.approvedChangeBudget,
      }

  val unmanagedStateIsPreserved =
    core.unmanagedState == 100 + core.unmanagedEditCount

  // Canonical revision writes are exactly verified TakeGraph commits. Native
  // Undo, external drift, capture, save and render never contribute to this count.
  val nativeUndoDoesNotAdvanceRevision =
    core.headRevision == core.committedApplyCount

  val checkpointLifecycleIsSourceBound =
    not(checkpointHasFreshnessContract(checkpoint.status))
      or and {
        checkpoint.sourceRevision == core.headRevision,
        checkpoint.sourceSceneEpoch == core.sceneEpoch,
      }

  val inspectionLifecycleIsSourceBound =
    not(inspectionHasFreshnessContract(inspection.status))
      or and {
        inspection.sourceRevision == core.headRevision,
        inspection.sourceSceneEpoch == core.sceneEpoch,
        inspection.sourceProfileRevision == inspection.activeProfileRevision,
      }

  val renderLifecycleIsSourceBound =
    and {
      implies(not(authoritativeRenderSourceBindingAvailable), render.status == RenderIdle),
      not(renderHasFreshnessContract(render.status))
        or and {
          render.sourceRevision == core.headRevision,
          render.sourceSceneEpoch == core.sceneEpoch,
          render.sourceProfileRevision == render.activeProfileRevision,
          render.checkpointBound,
          render.checkpointBytesFresh,
          render.encoderBindingFresh,
          render.overwriteWalPrepared,
          render.candidateNamespaceLeased,
          implies(render.status == RenderRunning, render.encoderFilesLeased),
          implies(render.status == RenderCompleted, and {
            render.candidateVerified,
            render.candidatePublished,
          }),
        },
    }

  val nonSuccessfulRenderRestoresOriginal =
    not(any {
      render.status == RenderCancelled,
      render.status == RenderFailed,
      render.status == RenderStale,
    })
      or and {
        render.overwriteWalPrepared,
        render.originalRestored,
      }

  val tier2TasksDoNotAdvanceRevision =
    core.headRevision == core.committedApplyCount

  val applyAndCheckpointRequireExistingPath =
    not(any {
      applyOwnsProjectGate(core.applyStatus),
      core.applyStatus == ApplyVerified,
      core.applyStatus == ApplyCommitted,
      checkpoint.status == CheckpointSaving,
      checkpoint.status == CheckpointVerified,
    })
      or core.hasExistingPath

  val projectOperationGateIsExclusive =
    not(any {
      and {
        applyOwnsProjectGate(core.applyStatus),
        checkpoint.status == CheckpointSaving,
      },
      and {
        applyOwnsProjectGate(core.applyStatus),
        inspection.status == InspectionCapturing,
      },
      and {
        applyOwnsProjectGate(core.applyStatus),
        render.status == RenderRunning,
      },
      and {
        checkpoint.status == CheckpointSaving,
        inspection.status == InspectionCapturing,
      },
      and {
        checkpoint.status == CheckpointSaving,
        render.status == RenderRunning,
      },
      and {
        inspection.status == InspectionCapturing,
        render.status == RenderRunning,
      },
    })

  val lifecycleSafety = and {
    recoveryRequiredHasDurableCause,
    durableSuccessfulReadbackCannotRollBack,
    recoveryRequiredIsTerminal,
    verifiedApplyWithinApprovedBudget,
    unmanagedStateIsPreserved,
    nativeUndoDoesNotAdvanceRevision,
    checkpointLifecycleIsSourceBound,
    inspectionLifecycleIsSourceBound,
    renderLifecycleIsSourceBound,
    nonSuccessfulRenderRestoresOriginal,
    tier2TasksDoNotAdvanceRevision,
    projectOperationGateIsExclusive,
    applyAndCheckpointRequireExistingPath,
  }

  // Witnesses make the important failure and independent-lifecycle paths
  // visible in randomized traces without weakening the safety invariants.
  val recoveryRequiredReachable =
    core.applyStatus == ApplyRecoveryRequired

  val overBudgetRecoveryReachable =
    core.applyStatus == ApplyRecovering
      and core.actualManagedChanges > core.approvedChangeBudget

  val independentTaskStatesReachable =
    and {
      checkpoint.status == CheckpointSaving,
      inspection.status == InspectionStaged,
      render.status == RenderStaged,
    }

  val acceptedInspectionReachable =
    inspection.status == InspectionAccepted

  val staleAllOutputsReachable =
    and {
      checkpoint.status == CheckpointStale,
      inspection.status == InspectionStale,
      render.status == RenderStale,
    }
}

// Phase 5 reconciliation execution. Acceptance materializes inert children;
// metadata detach and canonical re-export each require their own approval.
// The detach mutation is Remark-only, WAL-bound, project-scoped and verified
// by a fresh read-back. Re-export dispatch can only create an unapproved
// downstream preview.
module ymm4ReconciliationProtocol {
  type DetachStatus =
    | DetachAbsent
    | DetachPreview
    | DetachApproved
    | DetachApplying
    | DetachVerified
    | DetachRolledBack
    | DetachRecoveryRequired

  type ReExportStatus =
    | ReExportAbsent
    | ReExportAwaitingManifest
    | ReExportPreview
    | ReExportApproved
    | ReExportApplied

  type ReconciliationState = {
    parentAccepted: bool,
    headRevision: int,
    importApproved: bool,
    detachStatus: DetachStatus,
    activeProject: int,
    requestProject: int,
    requestDigest: int,
    approvedDetachDigest: int,
    walDigest: int,
    receiptDigest: int,
    remarkPresent: bool,
    beforeRemarkSetDigest: int,
    expectedRemarkSetDigest: int,
    currentRemarkSetDigest: int,
    expectedRemarkWitnessCount: int,
    currentRemarkWitnessCount: int,
    remarkWitnessesUnique: bool,
    beforeNonRemarkDigest: int,
    currentNonRemarkDigest: int,
    receiptVerified: bool,
    responseSuccess: bool,
    detachMutationCount: int,
    detachCanonicalCommitCount: int,
    recoveryGate: bool,
    reExportStatus: ReExportStatus,
    incomingManifestDigest: int,
    dispatchedManifestDigest: int,
    reExportPreviewDigest: int,
    sealedReExportPreviewDigest: int,
    approvedReExportDigest: int,
    reExportMutationCount: int,
  }

  var state: ReconciliationState

  action init = state' = {
    parentAccepted: false,
    headRevision: 7,
    importApproved: false,
    detachStatus: DetachAbsent,
    activeProject: 1,
    requestProject: 1,
    requestDigest: 10,
    approvedDetachDigest: -1,
    walDigest: -1,
    receiptDigest: -1,
    remarkPresent: true,
    beforeRemarkSetDigest: 200,
    expectedRemarkSetDigest: 201,
    currentRemarkSetDigest: 200,
    expectedRemarkWitnessCount: 2,
    currentRemarkWitnessCount: 2,
    remarkWitnessesUnique: true,
    beforeNonRemarkDigest: 100,
    currentNonRemarkDigest: 100,
    receiptVerified: false,
    responseSuccess: false,
    detachMutationCount: 0,
    detachCanonicalCommitCount: 0,
    recoveryGate: false,
    reExportStatus: ReExportAbsent,
    incomingManifestDigest: 30,
    dispatchedManifestDigest: -1,
    reExportPreviewDigest: 20,
    sealedReExportPreviewDigest: -1,
    approvedReExportDigest: -1,
    reExportMutationCount: 0,
  }

  // Reconciliation approval creates children but approves none of them.
  action acceptChoices = all {
    not(state.parentAccepted),
    state' = state
      .with("parentAccepted", true)
      .with("detachStatus", DetachPreview)
      .with("requestProject", state.activeProject)
      .with("reExportStatus", ReExportAwaitingManifest),
  }

  action approveDetach = all {
    state.detachStatus == DetachPreview,
    not(state.recoveryGate),
    state' = state
      .with("approvedDetachDigest", state.requestDigest)
      .with("detachStatus", DetachApproved),
  }

  // Editing a preview invalidates any prior approval binding.
  action replaceDetachPreview = all {
    any {
      state.detachStatus == DetachPreview,
      state.detachStatus == DetachApproved,
    },
    state' = state
      .with("requestDigest", state.requestDigest + 1)
      .with("requestProject", state.activeProject)
      .with("detachStatus", DetachPreview),
  }

  action switchProjectBeforeDetach = all {
    state.detachStatus == DetachApproved,
    state' = state.with("activeProject", state.activeProject + 1),
  }

  // This is the durable WAL transition. It performs no YMM4 mutation.
  action beginDetach = all {
    state.detachStatus == DetachApproved,
    not(state.recoveryGate),
    state.approvedDetachDigest == state.requestDigest,
    state.requestProject == state.activeProject,
    state' = state
      .with("walDigest", state.requestDigest)
      .with("detachStatus", DetachApplying),
  }

  // The only mutation removes the identity Remark. Non-Remark content is not
  // assigned and therefore remains exactly at its sealed digest.
  action removeIdentityRemark = all {
    state.detachStatus == DetachApplying,
    state.walDigest == state.requestDigest,
    state.walDigest == state.approvedDetachDigest,
    state.requestProject == state.activeProject,
    state.remarkPresent,
    state.currentRemarkSetDigest == state.beforeRemarkSetDigest,
    state.currentRemarkWitnessCount == state.expectedRemarkWitnessCount,
    state.remarkWitnessesUnique,
    state.detachMutationCount == 0,
    state' = state
      .with("remarkPresent", false)
      .with("currentRemarkSetDigest", state.expectedRemarkSetDigest)
      .with("detachMutationCount", 1),
  }

  // A concurrent user edit to any unrelated Remark, or a duplicated stable
  // item witness, invalidates the sealed exact-scene postcondition.
  action driftUnrelatedRemarkAfterMutation = all {
    state.detachStatus == DetachApplying,
    state.detachMutationCount == 1,
    state' = state.with("currentRemarkSetDigest", state.currentRemarkSetDigest + 10),
  }

  action duplicateRemarkWitnessAfterMutation = all {
    state.detachStatus == DetachApplying,
    state.detachMutationCount == 1,
    state' = state
      .with("currentRemarkWitnessCount", state.currentRemarkWitnessCount + 1)
      .with("remarkWitnessesUnique", false),
  }

  // This represents a fresh project read-back after the bridge mutation.
  action verifyDetachReadback = all {
    state.detachStatus == DetachApplying,
    not(state.remarkPresent),
    state.currentRemarkSetDigest == state.expectedRemarkSetDigest,
    state.currentRemarkWitnessCount == state.expectedRemarkWitnessCount,
    state.remarkWitnessesUnique,
    state.currentNonRemarkDigest == state.beforeNonRemarkDigest,
    state.detachMutationCount == 1,
    state' = state
      .with("receiptDigest", state.walDigest)
      .with("receiptVerified", true)
      .with("responseSuccess", true)
      .with("detachCanonicalCommitCount", 1)
      .with("headRevision", state.headRevision + 1)
      .with("detachStatus", DetachVerified),
  }

  // An ambiguous read-back closes the global recovery gate. No new detach can
  // begin until the exact WAL preimage has been restored or verified.
  action ambiguousDetachReadback = all {
    state.detachStatus == DetachApplying,
    not(state.remarkPresent),
    state.detachMutationCount == 1,
    state' = state
      .with("recoveryGate", true)
      .with("detachStatus", DetachRecoveryRequired),
  }

  action rollbackDetach = all {
    state.detachStatus == DetachRecoveryRequired,
    state.recoveryGate,
    state.walDigest == state.approvedDetachDigest,
    state' = state
      .with("remarkPresent", true)
      .with("currentRemarkSetDigest", state.beforeRemarkSetDigest)
      .with("currentRemarkWitnessCount", state.expectedRemarkWitnessCount)
      .with("remarkWitnessesUnique", true)
      .with("receiptVerified", false)
      .with("responseSuccess", false)
      .with("recoveryGate", false)
      .with("detachStatus", DetachRolledBack),
  }

  // Authenticated replay is a pure lookup of the same operation/request
  // binding. It cannot repeat the mutation.
  action replayVerifiedDetach = all {
    state.detachStatus == DetachVerified,
    state.receiptVerified,
    state.receiptDigest == state.requestDigest,
    state.receiptDigest == state.walDigest,
    state' = state,
  }

  // Dispatcher output is an ordinary preview and deliberately has no approval.
  action dispatchReExport = all {
    state.reExportStatus == ReExportAwaitingManifest,
    state' = state
      .with("reExportPreviewDigest", state.reExportPreviewDigest + 1)
      .with("sealedReExportPreviewDigest", state.reExportPreviewDigest + 1)
      .with("dispatchedManifestDigest", state.incomingManifestDigest)
      .with("reExportStatus", ReExportPreview),
  }

  // An exact retry returns the same durable child and exporter task. It does
  // not stage a second preview or change the downstream digest.
  action replayReExportExact = all {
    state.reExportStatus == ReExportPreview,
    state.incomingManifestDigest == state.dispatchedManifestDigest,
    state' = state,
  }

  // A caller may present another manifest, but the durable child is already
  // bound. No dispatcher transition accepts it or replaces the saved preview.
  action presentDifferentReExportManifest = all {
    state.reExportStatus == ReExportPreview,
    state' = state.with("incomingManifestDigest", state.incomingManifestDigest + 1),
  }

  action approveReExport = all {
    state.reExportStatus == ReExportPreview,
    state' = state
      .with("approvedReExportDigest", state.reExportPreviewDigest)
      .with("reExportStatus", ReExportApproved),
  }

  action applyReExport = all {
    state.reExportStatus == ReExportApproved,
    state.approvedReExportDigest == state.reExportPreviewDigest,
    state.reExportMutationCount == 0,
    state' = state
      .with("reExportMutationCount", 1)
      .with("reExportStatus", ReExportApplied),
  }

  action stutter = state' = state

  action step = any {
    acceptChoices,
    approveDetach,
    replaceDetachPreview,
    switchProjectBeforeDetach,
    beginDetach,
    removeIdentityRemark,
    driftUnrelatedRemarkAfterMutation,
    duplicateRemarkWitnessAfterMutation,
    verifyDetachReadback,
    ambiguousDetachReadback,
    rollbackDetach,
    replayVerifiedDetach,
    dispatchReExport,
    replayReExportExact,
    presentDifferentReExportManifest,
    approveReExport,
    applyReExport,
    stutter,
  }

  val importRemainsUnapproved = not(state.importApproved)

  val detachWalPrecedesMutation =
    state.detachMutationCount == 0
      or and {
        state.walDigest == state.requestDigest,
        state.walDigest == state.approvedDetachDigest,
      }

  val detachIsProjectScoped =
    state.detachStatus != DetachApplying
      or state.requestProject == state.activeProject

  val detachPreservesNonRemarkContent =
    state.currentNonRemarkDigest == state.beforeNonRemarkDigest

  val verifiedDetachHasFreshBoundReceipt =
    state.detachStatus != DetachVerified
      or and {
        not(state.remarkPresent),
        state.receiptVerified,
        state.responseSuccess,
        state.receiptDigest == state.requestDigest,
        state.receiptDigest == state.walDigest,
        state.currentRemarkSetDigest == state.expectedRemarkSetDigest,
        state.currentRemarkWitnessCount == state.expectedRemarkWitnessCount,
        state.remarkWitnessesUnique,
        state.detachMutationCount == 1,
      }

  val detachIsAtMostOnce = state.detachMutationCount <= 1

  val detachOwnershipCommitIsExactlyOnce = and {
    state.detachCanonicalCommitCount <= 1,
    state.detachStatus != DetachVerified
      or and {
        state.detachCanonicalCommitCount == 1,
        state.headRevision == 8,
      },
  }

  val recoveryRequiredClosesGate =
    state.recoveryGate == (state.detachStatus == DetachRecoveryRequired)

  val reExportPreviewIsUnapproved =
    state.reExportStatus != ReExportPreview
      or state.approvedReExportDigest != state.reExportPreviewDigest

  val reExportDispatchDoesNotMutate =
    not(any {
      state.reExportStatus == ReExportAwaitingManifest,
      state.reExportStatus == ReExportPreview,
    })
      or state.reExportMutationCount == 0

  val reExportManifestReplayIsBound =
    state.reExportStatus == ReExportAwaitingManifest
      or state.reExportStatus == ReExportAbsent
      or and {
        state.dispatchedManifestDigest != -1,
        state.sealedReExportPreviewDigest == state.reExportPreviewDigest,
      }

  val reExportApplyRequiresSeparateApproval =
    state.reExportStatus != ReExportApplied
      or and {
        state.approvedReExportDigest == state.reExportPreviewDigest,
        state.reExportMutationCount == 1,
      }

  val onlyVerifiedDetachAdvancesCanonicalRevision =
    state.headRevision == 7 + state.detachCanonicalCommitCount

  val reconciliationSafety = and {
    importRemainsUnapproved,
    detachWalPrecedesMutation,
    detachIsProjectScoped,
    detachPreservesNonRemarkContent,
    verifiedDetachHasFreshBoundReceipt,
    detachIsAtMostOnce,
    detachOwnershipCommitIsExactlyOnce,
    recoveryRequiredClosesGate,
    reExportPreviewIsUnapproved,
    reExportDispatchDoesNotMutate,
    reExportManifestReplayIsBound,
    reExportApplyRequiresSeparateApproval,
    onlyVerifiedDetachAdvancesCanonicalRevision,
  }
}

// Shared canonical-to-YMM exclusion for every ordinary exporter route. Owner
// 1 and owner 2 stand for any two distinct routes among portable export,
// native voice create, native voice mutation, and native extension. The OS
// fence is transient; the durable reservation remains after an ambiguous
// bridge outcome and is the authority that excludes every other route.
module ymm4ExternalMutationFenceProtocol {
  type FencePhase =
    | FenceIdle
    | FenceReserved
    | FenceMutating
    | FenceAmbiguous
    | FenceAppliedUnverified
    | FenceVerified
    | FencePublished
    | FenceRecoveryRequired
    | FenceNotStarted
    | FenceRolledBack

  type FenceState = {
    phase: FencePhase,
    lockOwner: int,
    reservationOwner: int,
    reservationEvidence: int,
    mutationCountOne: int,
    mutationCountTwo: int,
    verifiedOne: bool,
    verifiedTwo: bool,
    canonicalCommitOne: int,
    canonicalCommitTwo: int,
    committedEvidenceOne: int,
    committedEvidenceTwo: int,
    durableReadbackOwner: int,
    rollbackProofOwner: int,
    notStartedProofOwner: int,
    lastAbortOwner: int,
    operatorAckProofOwner: int,
  }

  var fence: FenceState

  action init = fence' = {
    phase: FenceIdle,
    lockOwner: 0,
    reservationOwner: 0,
    reservationEvidence: 0,
    mutationCountOne: 0,
    mutationCountTwo: 0,
    verifiedOne: false,
    verifiedTwo: false,
    canonicalCommitOne: 0,
    canonicalCommitTwo: 0,
    committedEvidenceOne: 0,
    committedEvidenceTwo: 0,
    durableReadbackOwner: 0,
    rollbackProofOwner: 0,
    notStartedProofOwner: 0,
    lastAbortOwner: 0,
    operatorAckProofOwner: 0,
  }

  action reserveOne = all {
    fence.lockOwner == 0,
    fence.reservationOwner == 0,
    fence.mutationCountOne == 0,
    fence.mutationCountTwo == 0,
    fence.canonicalCommitOne == 0,
    fence' = fence
      .with("phase", FenceReserved)
      .with("lockOwner", 1)
      .with("reservationOwner", 1)
      .with("reservationEvidence", 101)
      .with("durableReadbackOwner", 0)
      .with("rollbackProofOwner", 0)
      .with("notStartedProofOwner", 0)
      .with("lastAbortOwner", 0)
      .with("operatorAckProofOwner", if (fence.operatorAckProofOwner == 1) 0 else fence.operatorAckProofOwner),
  }

  action reserveTwo = all {
    fence.lockOwner == 0,
    fence.reservationOwner == 0,
    fence.mutationCountTwo == 0,
    fence.mutationCountOne == 0,
    fence.canonicalCommitTwo == 0,
    fence' = fence
      .with("phase", FenceReserved)
      .with("lockOwner", 2)
      .with("reservationOwner", 2)
      .with("reservationEvidence", 202)
      .with("durableReadbackOwner", 0)
      .with("rollbackProofOwner", 0)
      .with("notStartedProofOwner", 0)
      .with("lastAbortOwner", 0)
      .with("operatorAckProofOwner", if (fence.operatorAckProofOwner == 2) 0 else fence.operatorAckProofOwner),
  }

  action mutateOne = all {
    fence.phase == FenceReserved,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence.mutationCountOne == 0,
    fence' = fence
      .with("phase", FenceMutating)
      .with("mutationCountOne", 1),
  }

  action mutateTwo = all {
    fence.phase == FenceReserved,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence.mutationCountTwo == 0,
    fence' = fence
      .with("phase", FenceMutating)
      .with("mutationCountTwo", 1),
  }

  // Transport loss releases only the process fence. The durable reservation
  // survives restart and keeps every other route out.
  action ambiguousOne = all {
    fence.phase == FenceMutating,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence' = fence
      .with("phase", FenceAmbiguous)
      .with("lockOwner", 0),
  }

  action ambiguousTwo = all {
    fence.phase == FenceMutating,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence' = fence
      .with("phase", FenceAmbiguous)
      .with("lockOwner", 0),
  }

  // A request may be delayed before the bridge has published its WAL. The
  // durable canonical reservation remains, but no target mutation is counted.
  action ambiguousBeforeMutationOne = all {
    fence.phase == FenceReserved,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence.mutationCountOne == 0,
    fence' = fence
      .with("phase", FenceAmbiguous)
      .with("lockOwner", 0),
  }

  action ambiguousBeforeMutationTwo = all {
    fence.phase == FenceReserved,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence.mutationCountTwo == 0,
    fence' = fence
      .with("phase", FenceAmbiguous)
      .with("lockOwner", 0),
  }

  action retryOne = all {
    fence.phase == FenceAmbiguous,
    fence.lockOwner == 0,
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence' = fence
      .with("phase", FenceMutating)
      .with("lockOwner", 1)
      .with("mutationCountOne", 1),
  }

  action retryTwo = all {
    fence.phase == FenceAmbiguous,
    fence.lockOwner == 0,
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence' = fence
      .with("phase", FenceMutating)
      .with("lockOwner", 2)
      .with("mutationCountTwo", 1),
  }

  // Successful semantic readback is itself durable before the terminal
  // verified receipt. A crash in this window must never trigger rollback.
  action persistSuccessfulReadbackOne = all {
    fence.phase == FenceMutating,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence.mutationCountOne == 1,
    fence' = fence
      .with("phase", FenceAppliedUnverified)
      .with("durableReadbackOwner", 1)
      .with("rollbackProofOwner", 0),
  }

  action persistSuccessfulReadbackTwo = all {
    fence.phase == FenceMutating,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence.mutationCountTwo == 1,
    fence' = fence
      .with("phase", FenceAppliedUnverified)
      .with("durableReadbackOwner", 2)
      .with("rollbackProofOwner", 0),
  }

  action verifyOne = all {
    fence.phase == FenceAppliedUnverified,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence.durableReadbackOwner == 1,
    fence' = fence
      .with("phase", FenceVerified)
      .with("verifiedOne", true),
  }

  action verifyTwo = all {
    fence.phase == FenceAppliedUnverified,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence.durableReadbackOwner == 2,
    fence' = fence
      .with("phase", FenceVerified)
      .with("verifiedTwo", true),
  }

  action crashAfterReadbackOne = all {
    fence.phase == FenceAppliedUnverified,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence.durableReadbackOwner == 1,
    fence' = fence.with("lockOwner", 0),
  }

  action crashAfterReadbackTwo = all {
    fence.phase == FenceAppliedUnverified,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence.durableReadbackOwner == 2,
    fence' = fence.with("lockOwner", 0),
  }

  action recoverVerifiedReadbackOne = all {
    fence.phase == FenceAppliedUnverified,
    fence.lockOwner == 0,
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence.durableReadbackOwner == 1,
    fence' = fence
      .with("phase", FenceVerified)
      .with("lockOwner", 1)
      .with("verifiedOne", true),
  }

  action recoverVerifiedReadbackTwo = all {
    fence.phase == FenceAppliedUnverified,
    fence.lockOwner == 0,
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence.durableReadbackOwner == 2,
    fence' = fence
      .with("phase", FenceVerified)
      .with("lockOwner", 2)
      .with("verifiedTwo", true),
  }

  action requireManualReadbackRecoveryOne = all {
    fence.phase == FenceAppliedUnverified,
    any { fence.lockOwner == 0, fence.lockOwner == 1 },
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence.durableReadbackOwner == 1,
    fence' = fence
      .with("phase", FenceRecoveryRequired)
      .with("lockOwner", 0),
  }

  action requireManualReadbackRecoveryTwo = all {
    fence.phase == FenceAppliedUnverified,
    any { fence.lockOwner == 0, fence.lockOwner == 2 },
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence.durableReadbackOwner == 2,
    fence' = fence
      .with("phase", FenceRecoveryRequired)
      .with("lockOwner", 0),
  }

  action publishOne = all {
    fence.phase == FenceVerified,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence.verifiedOne,
    fence.canonicalCommitOne == 0,
    fence' = fence
      .with("phase", FencePublished)
      .with("lockOwner", 0)
      .with("reservationOwner", 0)
      .with("reservationEvidence", 0)
      .with("canonicalCommitOne", 1)
      .with("committedEvidenceOne", 101),
  }

  action publishTwo = all {
    fence.phase == FenceVerified,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence.verifiedTwo,
    fence.canonicalCommitTwo == 0,
    fence' = fence
      .with("phase", FencePublished)
      .with("lockOwner", 0)
      .with("reservationOwner", 0)
      .with("reservationEvidence", 0)
      .with("canonicalCommitTwo", 1)
      .with("committedEvidenceTwo", 202),
  }

  // A task-file save crash replays the exact committed evidence without a
  // second target mutation or canonical commit.
  action replayPublished = all {
    fence.phase == FencePublished,
    any { fence.canonicalCommitOne == 1, fence.canonicalCommitTwo == 1 },
    fence' = fence,
  }

  action proveRollbackOne = all {
    fence.phase == FenceMutating,
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence' = fence.with("rollbackProofOwner", 1),
  }

  action proveRollbackTwo = all {
    fence.phase == FenceMutating,
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence' = fence.with("rollbackProofOwner", 2),
  }

  // The bridge seals this request-bound tombstone under the same apply gate.
  // Once published, a delayed exact POST replays it and cannot begin mutation.
  action sealNotStartedOne = all {
    any { fence.phase == FenceReserved, fence.phase == FenceAmbiguous },
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence.mutationCountOne == 0,
    any { fence.lockOwner == 0, fence.lockOwner == 1 },
    fence' = fence
      .with("phase", FenceNotStarted)
      .with("lockOwner", 1)
      .with("notStartedProofOwner", 1),
  }

  action sealNotStartedTwo = all {
    any { fence.phase == FenceReserved, fence.phase == FenceAmbiguous },
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence.mutationCountTwo == 0,
    any { fence.lockOwner == 0, fence.lockOwner == 2 },
    fence' = fence
      .with("phase", FenceNotStarted)
      .with("lockOwner", 2)
      .with("notStartedProofOwner", 2),
  }

  action abortOne = all {
    any {
      all {
        fence.phase == FenceMutating,
        fence.rollbackProofOwner == 1,
      },
      all {
        fence.phase == FenceNotStarted,
        fence.notStartedProofOwner == 1,
      },
    },
    fence.lockOwner == 1,
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence' = fence
      .with("phase", FenceRolledBack)
      .with("lockOwner", 0)
      .with("reservationOwner", 0)
      .with("reservationEvidence", 0)
      .with("lastAbortOwner", 1),
  }

  action abortTwo = all {
    any {
      all {
        fence.phase == FenceMutating,
        fence.rollbackProofOwner == 2,
      },
      all {
        fence.phase == FenceNotStarted,
        fence.notStartedProofOwner == 2,
      },
    },
    fence.lockOwner == 2,
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence' = fence
      .with("phase", FenceRolledBack)
      .with("lockOwner", 0)
      .with("reservationOwner", 0)
      .with("reservationEvidence", 0)
      .with("lastAbortOwner", 2),
  }

  // An operator may release an unresolved reservation without claiming
  // rollback, verification, or a canonical commit. Leftover target items
  // become unmanaged drift. This is not lastAbortOwner: successful
  // durable read-back still cannot roll back.
  action acknowledgeRecoveryRequiredOne = all {
    any {
      fence.phase == FenceAmbiguous,
      fence.phase == FenceRecoveryRequired,
    },
    fence.reservationOwner == 1,
    fence.reservationEvidence == 101,
    fence' = fence
      .with("phase", FenceIdle)
      .with("lockOwner", 0)
      .with("reservationOwner", 0)
      .with("reservationEvidence", 0)
      .with("operatorAckProofOwner", 1),
  }

  action acknowledgeRecoveryRequiredTwo = all {
    any {
      fence.phase == FenceAmbiguous,
      fence.phase == FenceRecoveryRequired,
    },
    fence.reservationOwner == 2,
    fence.reservationEvidence == 202,
    fence' = fence
      .with("phase", FenceIdle)
      .with("lockOwner", 0)
      .with("reservationOwner", 0)
      .with("reservationEvidence", 0)
      .with("operatorAckProofOwner", 2),
  }

  action stutter = fence' = fence

  action step = any {
    reserveOne,
    reserveTwo,
    mutateOne,
    mutateTwo,
    ambiguousOne,
    ambiguousTwo,
    ambiguousBeforeMutationOne,
    ambiguousBeforeMutationTwo,
    retryOne,
    retryTwo,
    persistSuccessfulReadbackOne,
    persistSuccessfulReadbackTwo,
    verifyOne,
    verifyTwo,
    crashAfterReadbackOne,
    crashAfterReadbackTwo,
    recoverVerifiedReadbackOne,
    recoverVerifiedReadbackTwo,
    requireManualReadbackRecoveryOne,
    requireManualReadbackRecoveryTwo,
    publishOne,
    publishTwo,
    replayPublished,
    proveRollbackOne,
    proveRollbackTwo,
    sealNotStartedOne,
    sealNotStartedTwo,
    abortOne,
    abortTwo,
    acknowledgeRecoveryRequiredOne,
    acknowledgeRecoveryRequiredTwo,
    stutter,
  }

  val mutationRequiresExactReservation = and {
    fence.phase != FenceMutating
      or and {
        fence.lockOwner == fence.reservationOwner,
        any {
          and { fence.lockOwner == 1, fence.reservationEvidence == 101 },
          and { fence.lockOwner == 2, fence.reservationEvidence == 202 },
        },
      },
    fence.mutationCountOne == 0
      or fence.committedEvidenceOne == 101
      or fence.lastAbortOwner == 1
      or fence.operatorAckProofOwner == 1
      or and {
        fence.reservationOwner == 1,
        fence.reservationEvidence == 101,
      },
    fence.mutationCountTwo == 0
      or fence.committedEvidenceTwo == 202
      or fence.lastAbortOwner == 2
      or fence.operatorAckProofOwner == 2
      or and {
        fence.reservationOwner == 2,
        fence.reservationEvidence == 202,
      },
  }

  val externalReservationIsExclusive = and {
    fence.reservationOwner >= 0,
    fence.reservationOwner <= 2,
    fence.lockOwner == 0 or fence.lockOwner == fence.reservationOwner,
  }

  val ambiguousOutcomeRetainsReservation =
    fence.phase != FenceAmbiguous
      or and {
        fence.lockOwner == 0,
        any {
          and { fence.reservationOwner == 1, fence.reservationEvidence == 101 },
          and { fence.reservationOwner == 2, fence.reservationEvidence == 202 },
        },
      }

  val publicationIsVerifiedAndAtMostOnce = and {
    fence.canonicalCommitOne <= 1,
    fence.canonicalCommitTwo <= 1,
    fence.canonicalCommitOne == 0
      or and { fence.verifiedOne, fence.committedEvidenceOne == 101 },
    fence.canonicalCommitTwo == 0
      or and { fence.verifiedTwo, fence.committedEvidenceTwo == 202 },
  }

  val reservationAbortRequiresExactTerminalProof =
    fence.lastAbortOwner == 0
      or any {
        fence.lastAbortOwner == fence.rollbackProofOwner,
        fence.lastAbortOwner == fence.notStartedProofOwner,
      }

  val notStartedTombstonePreventsMutation =
    fence.phase != FenceNotStarted
      or any {
        all {
          fence.reservationOwner == 1,
          fence.notStartedProofOwner == 1,
          fence.mutationCountOne == 0,
        },
        all {
          fence.reservationOwner == 2,
          fence.notStartedProofOwner == 2,
          fence.mutationCountTwo == 0,
        },
      }

  val durableSuccessfulReadbackCannotRollBack = and {
    fence.durableReadbackOwner == 0
      or fence.phase != FenceRolledBack,
    fence.durableReadbackOwner == 0
      or fence.lastAbortOwner != fence.durableReadbackOwner,
  }

  val unresolvedDurableReadbackRetainsReservation =
    not(any {
      fence.phase == FenceAppliedUnverified,
      fence.phase == FenceRecoveryRequired,
    })
      or any {
        all {
          fence.durableReadbackOwner == 1,
          fence.reservationOwner == 1,
          fence.reservationEvidence == 101,
        },
        all {
          fence.durableReadbackOwner == 2,
          fence.reservationOwner == 2,
          fence.reservationEvidence == 202,
        },
      }

  val operatorAckDoesNotKeepOwnReservation =
    fence.operatorAckProofOwner == 0
      or fence.reservationOwner != fence.operatorAckProofOwner

  val operatorAckIsNotRollback =
    fence.operatorAckProofOwner == 0
      or fence.lastAbortOwner != fence.operatorAckProofOwner

  val externalMutationFenceSafety = and {
    mutationRequiresExactReservation,
    externalReservationIsExclusive,
    ambiguousOutcomeRetainsReservation,
    publicationIsVerifiedAndAtMostOnce,
    reservationAbortRequiresExactTerminalProof,
    notStartedTombstonePreventsMutation,
    durableSuccessfulReadbackCannotRollBack,
    unresolvedDurableReadbackRetainsReservation,
    operatorAckDoesNotKeepOwnReservation,
    operatorAckIsNotRollback,
  }
}
