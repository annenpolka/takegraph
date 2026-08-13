// Universal YMM4 edit-transaction protocol.
//
// `ymm4ChangeSetProtocol` proves the minimal two-operation apply/rollback
// mechanics. This model adds the envelope needed before every future YMM edit
// kind can share that machinery instead of growing one endpoint per kind:
//
// - operation descriptors are opaque to the transaction lifecycle and may
//   represent item lifecycle, transforms, text, audio, effects, characters,
//   timeline structure, or a future extension family;
// - one digest seals the ordered heterogeneous descriptor list, source,
//   capability revision, and the single transaction endpoint contract;
// - preparation may finish in any order, but apply and rollback order are
//   derived only from the sealed descriptor order;
// - one project queue, writer gate, whole-batch preimage, and durable WAL cover
//   the complete transaction;
// - every operation is read back before exactly one canonical revision is
//   published; and
// - any already-staged later task is invalidated by the preceding commit and
//   must be re-previewed against the new source.
//
// Global project settings are deliberately outside this ordinary edit
// transaction. They require a separately modeled project operation. A caller
// also cannot emulate a transaction by chaining legacy kind-specific
// endpoints: such a pseudo-batch is rejected before the first mutation.
module ymm4EditTransactionProtocol {
  type TransactionStatus =
    | Draft
    | Preparing
    | Previewable
    | Approved
    | Queued
    | Applying
    | Verifying
    | Verified
    | RollingBack
    | RecoveryRequired
    | Committed
    | RolledBack
    | RepreviewRequired
    | Rejected

  type LaterStatus =
    | LaterAbsent
    | LaterPreviewable
    | LaterApproved
    | LaterQueued
    | LaterStale

  type RecoveryMode =
    | NoRecovery
    | ResumeApply
    | ResumeRollback

  type EndpointMode =
    | UnifiedTransactionEndpoint
    | LegacyKindEndpoints

  type RejectionReason =
    | NoRejection
    | UnmanagedTarget
    | LockedTarget
    | GlobalSettingTarget
    | OperationCountOutOfBounds
    | OverlappingWriteFootprint
    | IllegalDependencyOrder
    | PseudoBatchAcrossEndpoints

  // The lifecycle never branches on these variants. Adding another concrete
  // edit kind means adding a descriptor/driver mapping, not another lifecycle
  // or endpoint.
  type SemanticFamily =
    | ItemLifecycleFamily
    | TransformVisibilityFamily
    | TextStyleFamily
    | AudioPlaybackFamily
    | EffectChainFamily
    | CharacterVoiceFamily
    | TimelineStructureFamily
    | FutureExtensionFamily

  type TargetScope =
    | ProjectLocalTarget
    | GlobalProjectSettingTarget

  type ReadbackEvidence =
    | ReadbackPending
    | ReadbackExact
    | ReadbackMismatch

  type OperationDescriptor = {
    index: int,
    family: SemanticFamily,
    kindDigest: int,
    targetManaged: bool,
    targetUnlocked: bool,
    targetScope: TargetScope,
    requiredCapability: int,
  }

  type TransactionBinding = {
    taskId: int,
    requestDigest: int,
    projectId: int,
    sceneId: int,
    baseRevision: int,
    sourceFingerprint: int,
    capabilityRevision: int,
    descriptorListDigest: int,
    operationCount: int,
    operationOrderDigest: int,
    semanticFamilyDigest: int,
    endpointRevision: int,
  }

  type LaterTask = {
    status: LaterStatus,
    binding: TransactionBinding,
    approvedBinding: TransactionBinding,
    queuePosition: int,
    repreviewCount: int,
  }

  type TransactionState = {
    status: TransactionStatus,
    rejectionReason: RejectionReason,
    recoveryMode: RecoveryMode,

    headRevision: int,
    externalCommitCount: int,
    canonicalCommitCount: int,
    projectId: int,
    sceneId: int,
    sourceFingerprint: int,
    capabilityRevision: int,
    endpointMode: EndpointMode,
    requestedOperationCount: int,
    writeFootprintsDisjoint: bool,
    dependenciesRespectCanonicalOrder: bool,
    writeFootprintEvidenceDigest: int,
    dependencyEvidenceDigest: int,

    operationOne: OperationDescriptor,
    operationTwo: OperationDescriptor,
    operationThree: OperationDescriptor,
    preparationClock: int,
    operationOnePrepared: bool,
    operationTwoPrepared: bool,
    operationThreePrepared: bool,
    operationOnePreparationOrder: int,
    operationTwoPreparationOrder: int,
    operationThreePreparationOrder: int,

    task: TransactionBinding,
    approvedBinding: TransactionBinding,
    walBinding: TransactionBinding,
    receiptBinding: TransactionBinding,
    recoveryBinding: TransactionBinding,
    terminalBinding: TransactionBinding,

    queueHead: int,
    mainQueuePosition: int,
    writerOwner: int,
    reservationOwner: int,
    gateRejectCount: int,

    wholePreimageCaptured: bool,
    wholePreimageDigest: int,
    preimageCoverageDigest: int,
    walPresent: bool,
    targetMatchesWholePreimage: bool,
    appliedPrefix: int,
    operationOneWrites: int,
    operationTwoWrites: int,
    operationThreeWrites: int,
    operationOneApplyOrder: int,
    operationTwoApplyOrder: int,
    operationThreeApplyOrder: int,
    rollbackWrites: int,
    firstRollbackIndex: int,
    lastRollbackIndex: int,

    operationOneReadBack: ReadbackEvidence,
    operationTwoReadBack: ReadbackEvidence,
    operationThreeReadBack: ReadbackEvidence,
    postimageDigest: int,

    unifiedEndpointDispatchCount: int,
    legacyEndpointDispatchAttemptCount: int,
    legacyEndpointMutationCount: int,
    atomicRollbackClaimCount: int,
    terminalReplayCount: int,
    mismatchedReplayRejectCount: int,
    mismatchedRecoveryRejectCount: int,
    presentedRecoveryBinding: TransactionBinding,
    lastRejectedRecoveryBinding: TransactionBinding,
    presentedTerminalBinding: TransactionBinding,
    lastRejectedTerminalBinding: TransactionBinding,

    later: LaterTask,
    laterWasStaged: bool,
  }

  val missing = -1
  val noOwner = 0
  val mainTaskOwner = 1
  val laterTaskOwner = 2
  val initialRevision = 10
  val initialSourceFingerprint = 100
  val canonicalOperationCount = 3
  val canonicalOperationOrderDigest = 123
  val canonicalSemanticFamilyDigest = 456
  val canonicalPreimageCoverageDigest = 789
  val transactionEndpointRevision = 2

  val emptyBinding = {
    taskId: missing,
    requestDigest: missing,
    projectId: missing,
    sceneId: missing,
    baseRevision: missing,
    sourceFingerprint: missing,
    capabilityRevision: missing,
    descriptorListDigest: missing,
    operationCount: missing,
    operationOrderDigest: missing,
    semanticFamilyDigest: missing,
    endpointRevision: missing,
  }

  var state: TransactionState

  pure def familyCode(family: SemanticFamily): int =
    if (family == ItemLifecycleFamily) 1
    else if (family == TransformVisibilityFamily) 2
    else if (family == TextStyleFamily) 3
    else if (family == AudioPlaybackFamily) 4
    else if (family == EffectChainFamily) 5
    else if (family == CharacterVoiceFamily) 6
    else if (family == TimelineStructureFamily) 7
    else 8

  pure def scopeCode(scope: TargetScope): int =
    if (scope == ProjectLocalTarget) 1 else 2

  pure def boolCode(value: bool): int = if (value) 1 else 0

  pure def operationDescriptorDigest(operation: OperationDescriptor): int =
    operation.index * 10000000
      + familyCode(operation.family) * 1000000
      + operation.kindDigest * 1000
      + boolCode(operation.targetManaged) * 100
      + boolCode(operation.targetUnlocked) * 10
      + scopeCode(operation.targetScope) * 3
      + operation.requiredCapability

  // Abstract canonical serialization of the complete ordered descriptor list.
  // Each operation contributes its index, family, kind, ownership/lock flags,
  // scope, and required capability. Conflict/dependency decisions contribute
  // both their exact evidence digests and their legality result. The production
  // representation is SHA-256; arithmetic here only keeps every input visible.
  pure def computeDescriptorListDigest(s: TransactionState): int =
    operationDescriptorDigest(s.operationOne) * 1000000
      + operationDescriptorDigest(s.operationTwo) * 1000
      + operationDescriptorDigest(s.operationThree)
      + s.requestedOperationCount * 100000000
      + s.writeFootprintEvidenceDigest * 10000
      + s.dependencyEvidenceDigest * 100
      + boolCode(s.writeFootprintsDisjoint) * 10
      + boolCode(s.dependenciesRespectCanonicalOrder)

  pure def makeBinding(
    taskId: int,
    requestDigest: int,
    projectId: int,
    sceneId: int,
    baseRevision: int,
    sourceFingerprint: int,
    capabilityRevision: int,
    descriptorListDigest: int,
  ): TransactionBinding = {
    taskId: taskId,
    requestDigest: requestDigest,
    projectId: projectId,
    sceneId: sceneId,
    baseRevision: baseRevision,
    sourceFingerprint: sourceFingerprint,
    capabilityRevision: capabilityRevision,
    descriptorListDigest: descriptorListDigest,
    operationCount: canonicalOperationCount,
    operationOrderDigest: canonicalOperationOrderDigest,
    semanticFamilyDigest: canonicalSemanticFamilyDigest,
    endpointRevision: transactionEndpointRevision,
  }

  // This stands for canonical serialization and hashing. The concrete hash
  // function remains an implementation boundary; every semantic input is a
  // term here so changing one creates a different approved request.
  pure def sealedDigest(
    taskId: int,
    projectId: int,
    sceneId: int,
    baseRevision: int,
    sourceFingerprint: int,
    capabilityRevision: int,
    descriptorListDigest: int,
  ): int =
    taskId * 1000000
      + projectId * 100000
      + sceneId * 10000
      + baseRevision * 1000
      + sourceFingerprint * 100
      + capabilityRevision * 10
      + descriptorListDigest
      + canonicalOperationOrderDigest
      + canonicalSemanticFamilyDigest
      + transactionEndpointRevision

  pure def operationIsEligible(operation: OperationDescriptor): bool = and {
    operation.targetManaged,
    operation.targetUnlocked,
    operation.targetScope == ProjectLocalTarget,
    operation.requiredCapability > 0,
  }

  pure def everyOperationIsEligible(s: TransactionState): bool = and {
    operationIsEligible(s.operationOne),
    operationIsEligible(s.operationTwo),
    operationIsEligible(s.operationThree),
  }

  pure def descriptorIsHeterogeneous(s: TransactionState): bool = any {
    s.operationOne.family != s.operationTwo.family,
    s.operationOne.family != s.operationThree.family,
    s.operationTwo.family != s.operationThree.family,
  }

  pure def everyOperationPrepared(s: TransactionState): bool = and {
    s.operationOnePrepared,
    s.operationTwoPrepared,
    s.operationThreePrepared,
  }

  pure def everyOperationReadBack(s: TransactionState): bool = and {
    s.operationOneReadBack != ReadbackPending,
    s.operationTwoReadBack != ReadbackPending,
    s.operationThreeReadBack != ReadbackPending,
  }

  pure def everyOperationReadBackExact(s: TransactionState): bool = and {
    s.operationOneReadBack == ReadbackExact,
    s.operationTwoReadBack == ReadbackExact,
    s.operationThreeReadBack == ReadbackExact,
  }

  pure def anyOperationReadBackMismatch(s: TransactionState): bool = any {
    s.operationOneReadBack == ReadbackMismatch,
    s.operationTwoReadBack == ReadbackMismatch,
    s.operationThreeReadBack == ReadbackMismatch,
  }

  pure def bindingMatchesActiveSource(
    binding: TransactionBinding,
    s: TransactionState,
  ): bool = and {
    binding.projectId == s.projectId,
    binding.sceneId == s.sceneId,
    binding.baseRevision == s.headRevision,
    binding.sourceFingerprint == s.sourceFingerprint,
    binding.capabilityRevision == s.capabilityRevision,
    binding.descriptorListDigest == computeDescriptorListDigest(s),
    binding.operationCount == canonicalOperationCount,
    binding.operationOrderDigest == canonicalOperationOrderDigest,
    binding.semanticFamilyDigest == canonicalSemanticFamilyDigest,
    binding.endpointRevision == transactionEndpointRevision,
  }

  pure def hasSealedMainTask(s: TransactionState): bool = and {
    s.task.taskId == mainTaskOwner,
    s.task.requestDigest
      == sealedDigest(
        mainTaskOwner,
        s.task.projectId,
        s.task.sceneId,
        s.task.baseRevision,
        s.task.sourceFingerprint,
        s.task.capabilityRevision,
        s.task.descriptorListDigest
      ),
    s.task.operationCount == canonicalOperationCount,
    s.task.descriptorListDigest == computeDescriptorListDigest(s),
    s.task.operationOrderDigest == canonicalOperationOrderDigest,
    s.task.semanticFamilyDigest == canonicalSemanticFamilyDigest,
    s.task.endpointRevision == transactionEndpointRevision,
  }

  pure def approvalMustRemainExact(status: TransactionStatus): bool = any {
    status == Approved,
    status == Queued,
    status == Applying,
    status == Verifying,
    status == Verified,
    status == RollingBack,
    status == RecoveryRequired,
    status == Committed,
    status == RolledBack,
  }

  action init = state' = {
    status: Draft,
    rejectionReason: NoRejection,
    recoveryMode: NoRecovery,

    headRevision: initialRevision,
    externalCommitCount: 0,
    canonicalCommitCount: 0,
    projectId: 4,
    sceneId: 5,
    sourceFingerprint: initialSourceFingerprint,
    capabilityRevision: 7,
    endpointMode: UnifiedTransactionEndpoint,
    requestedOperationCount: canonicalOperationCount,
    writeFootprintsDisjoint: true,
    dependenciesRespectCanonicalOrder: true,
    writeFootprintEvidenceDigest: 41,
    dependencyEvidenceDigest: 42,

    operationOne: {
      index: 1,
      family: ItemLifecycleFamily,
      kindDigest: 101,
      targetManaged: true,
      targetUnlocked: true,
      targetScope: ProjectLocalTarget,
      requiredCapability: 11,
    },
    operationTwo: {
      index: 2,
      family: TransformVisibilityFamily,
      kindDigest: 202,
      targetManaged: true,
      targetUnlocked: true,
      targetScope: ProjectLocalTarget,
      requiredCapability: 22,
    },
    operationThree: {
      index: 3,
      family: AudioPlaybackFamily,
      kindDigest: 303,
      targetManaged: true,
      targetUnlocked: true,
      targetScope: ProjectLocalTarget,
      requiredCapability: 33,
    },
    preparationClock: 0,
    operationOnePrepared: false,
    operationTwoPrepared: false,
    operationThreePrepared: false,
    operationOnePreparationOrder: 0,
    operationTwoPreparationOrder: 0,
    operationThreePreparationOrder: 0,

    task: emptyBinding,
    approvedBinding: emptyBinding,
    walBinding: emptyBinding,
    receiptBinding: emptyBinding,
    recoveryBinding: emptyBinding,
    terminalBinding: emptyBinding,

    queueHead: noOwner,
    mainQueuePosition: 0,
    writerOwner: noOwner,
    reservationOwner: noOwner,
    gateRejectCount: 0,

    wholePreimageCaptured: false,
    wholePreimageDigest: missing,
    preimageCoverageDigest: missing,
    walPresent: false,
    targetMatchesWholePreimage: true,
    appliedPrefix: 0,
    operationOneWrites: 0,
    operationTwoWrites: 0,
    operationThreeWrites: 0,
    operationOneApplyOrder: 0,
    operationTwoApplyOrder: 0,
    operationThreeApplyOrder: 0,
    rollbackWrites: 0,
    firstRollbackIndex: 0,
    lastRollbackIndex: 0,

    operationOneReadBack: ReadbackPending,
    operationTwoReadBack: ReadbackPending,
    operationThreeReadBack: ReadbackPending,
    postimageDigest: missing,

    unifiedEndpointDispatchCount: 0,
    legacyEndpointDispatchAttemptCount: 0,
    legacyEndpointMutationCount: 0,
    atomicRollbackClaimCount: 0,
    terminalReplayCount: 0,
    mismatchedReplayRejectCount: 0,
    mismatchedRecoveryRejectCount: 0,
    presentedRecoveryBinding: emptyBinding,
    lastRejectedRecoveryBinding: emptyBinding,
    presentedTerminalBinding: emptyBinding,
    lastRejectedTerminalBinding: emptyBinding,

    later: {
      status: LaterAbsent,
      binding: emptyBinding,
      approvedBinding: emptyBinding,
      queuePosition: 0,
      repreviewCount: 0,
    },
    laterWasStaged: false,
  }

  action stageTransaction = all {
    state.status == Draft,
    state.endpointMode == UnifiedTransactionEndpoint,
    state.requestedOperationCount >= 1,
    state.requestedOperationCount <= 128,
    state.requestedOperationCount == canonicalOperationCount,
    state.writeFootprintsDisjoint,
    state.dependenciesRespectCanonicalOrder,
    everyOperationIsEligible(state),
    descriptorIsHeterogeneous(state),
    state' = state
      .with("status", Preparing)
      .with("task", makeBinding(
        mainTaskOwner,
        missing,
        state.projectId,
        state.sceneId,
        state.headRevision,
        state.sourceFingerprint,
        state.capabilityRevision,
        computeDescriptorListDigest(state)
      )),
  }

  action prepareOperationOne = all {
    state.status == Preparing,
    not(state.operationOnePrepared),
    state' = state
      .with("preparationClock", state.preparationClock + 1)
      .with("operationOnePrepared", true)
      .with("operationOnePreparationOrder", state.preparationClock + 1),
  }

  action prepareOperationTwo = all {
    state.status == Preparing,
    not(state.operationTwoPrepared),
    state' = state
      .with("preparationClock", state.preparationClock + 1)
      .with("operationTwoPrepared", true)
      .with("operationTwoPreparationOrder", state.preparationClock + 1),
  }

  action prepareOperationThree = all {
    state.status == Preparing,
    not(state.operationThreePrepared),
    state' = state
      .with("preparationClock", state.preparationClock + 1)
      .with("operationThreePrepared", true)
      .with("operationThreePreparationOrder", state.preparationClock + 1),
  }

  // The digest is independent of preparation completion order.
  action sealPreview = all {
    state.status == Preparing,
    everyOperationPrepared(state),
    state' = state
      .with("status", Previewable)
      .with("task", state.task.with(
        "requestDigest",
        sealedDigest(
          mainTaskOwner,
          state.task.projectId,
          state.task.sceneId,
          state.task.baseRevision,
          state.task.sourceFingerprint,
          state.task.capabilityRevision,
          state.task.descriptorListDigest
        )
      )),
  }

  action approve = all {
    state.status == Previewable,
    everyOperationPrepared(state),
    hasSealedMainTask(state),
    bindingMatchesActiveSource(state.task, state),
    state' = state
      .with("status", Approved)
      .with("approvedBinding", state.task),
  }

  action enqueueMain = all {
    state.status == Approved,
    state.task == state.approvedBinding,
    bindingMatchesActiveSource(state.task, state),
    state.queueHead == noOwner,
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("status", Queued)
      .with("queueHead", mainTaskOwner)
      .with("mainQueuePosition", 1),
  }

  // This later transaction is already fully materialized. Its internal
  // preparation is abstracted because this model uses it to expose queue and
  // freshness behavior, while the main transaction exposes operation order.
  action stageLaterTask = all {
    state.status == Queued,
    state.later.status == LaterAbsent,
    state' = state
      .with("laterWasStaged", true)
      .with("later", state.later
        .with("status", LaterPreviewable)
        .with("binding", makeBinding(
          laterTaskOwner,
          sealedDigest(
            laterTaskOwner,
            state.projectId,
            state.sceneId,
            state.headRevision,
            state.sourceFingerprint,
            state.capabilityRevision,
            computeDescriptorListDigest(state)
          ),
          state.projectId,
          state.sceneId,
          state.headRevision,
          state.sourceFingerprint,
          state.capabilityRevision,
          computeDescriptorListDigest(state)
        ))),
  }

  action approveLaterTask = all {
    state.later.status == LaterPreviewable,
    bindingMatchesActiveSource(state.later.binding, state),
    state' = state.with("later", state.later
      .with("status", LaterApproved)
      .with("approvedBinding", state.later.binding)),
  }

  action enqueueLaterTask = all {
    state.later.status == LaterApproved,
    state.later.binding == state.later.approvedBinding,
    state.queueHead == mainTaskOwner,
    state.mainQueuePosition == 1,
    state' = state.with("later", state.later
      .with("status", LaterQueued)
      .with("queuePosition", 2)),
  }

  // A queued later task can never bypass the current queue head or writer.
  action rejectLaterGateBypass = all {
    state.later.status == LaterQueued,
    any {
      state.queueHead == mainTaskOwner,
      state.writerOwner == mainTaskOwner,
      state.reservationOwner == mainTaskOwner,
    },
    state' = state.with("gateRejectCount", state.gateRejectCount + 1),
  }

  // Source, capability, and descriptor drift all invalidate the complete
  // binding before the writer is acquired. None can be patched in-place under
  // the old approval.
  action externalCommitBeforeAcquire = all {
    any { state.status == Approved, state.status == Queued },
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("status", RepreviewRequired)
      .with("headRevision", state.headRevision + 1)
      .with("externalCommitCount", state.externalCommitCount + 1)
      .with("sourceFingerprint", state.sourceFingerprint + 1)
      .with("queueHead", noOwner)
      .with("mainQueuePosition", 0)
      .with("later", if (state.later.status == LaterAbsent)
        state.later
      else
        state.later.with("status", LaterStale).with("queuePosition", 0)),
  }

  action capabilityDriftBeforeAcquire = all {
    any { state.status == Approved, state.status == Queued },
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("status", RepreviewRequired)
      .with("capabilityRevision", state.capabilityRevision + 1)
      .with("queueHead", noOwner)
      .with("mainQueuePosition", 0)
      .with("later", if (state.later.status == LaterAbsent)
        state.later
      else
        state.later.with("status", LaterStale).with("queuePosition", 0)),
  }

  action descriptorDriftBeforeAcquire = all {
    any { state.status == Approved, state.status == Queued },
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("status", RepreviewRequired)
      .with("operationTwo", state.operationTwo.with(
        "kindDigest",
        state.operationTwo.kindDigest + 1
      ))
      .with("queueHead", noOwner)
      .with("mainQueuePosition", 0)
      .with("later", if (state.later.status == LaterAbsent)
        state.later
      else
        state.later.with("status", LaterStale).with("queuePosition", 0)),
  }

  action writeFootprintEvidenceDriftBeforeAcquire = all {
    any { state.status == Approved, state.status == Queued },
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("status", RepreviewRequired)
      .with(
        "writeFootprintEvidenceDigest",
        state.writeFootprintEvidenceDigest + 1
      )
      .with("queueHead", noOwner)
      .with("mainQueuePosition", 0)
      .with("later", if (state.later.status == LaterAbsent)
        state.later
      else
        state.later.with("status", LaterStale).with("queuePosition", 0)),
  }

  action dependencyEvidenceDriftBeforeAcquire = all {
    any { state.status == Approved, state.status == Queued },
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("status", RepreviewRequired)
      .with("dependencyEvidenceDigest", state.dependencyEvidenceDigest + 1)
      .with("queueHead", noOwner)
      .with("mainQueuePosition", 0)
      .with("later", if (state.later.status == LaterAbsent)
        state.later
      else
        state.later.with("status", LaterStale).with("queuePosition", 0)),
  }

  action repreviewMain = all {
    state.status == RepreviewRequired,
    state.appliedPrefix == 0,
    state.canonicalCommitCount == 0,
    state' = state
      .with("status", Preparing)
      .with("rejectionReason", NoRejection)
      .with("preparationClock", 0)
      .with("operationOnePrepared", false)
      .with("operationTwoPrepared", false)
      .with("operationThreePrepared", false)
      .with("operationOnePreparationOrder", 0)
      .with("operationTwoPreparationOrder", 0)
      .with("operationThreePreparationOrder", 0)
      .with("approvedBinding", emptyBinding)
      .with("task", makeBinding(
        mainTaskOwner,
        missing,
        state.projectId,
        state.sceneId,
        state.headRevision,
        state.sourceFingerprint,
        state.capabilityRevision,
        computeDescriptorListDigest(state)
      )),
  }

  // Reservation, complete preimage capture, WAL publication, and logical
  // endpoint dispatch happen before the first target mutation.
  action acquireUnifiedWriter = all {
    state.status == Queued,
    state.queueHead == mainTaskOwner,
    state.mainQueuePosition == 1,
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state.task == state.approvedBinding,
    bindingMatchesActiveSource(state.task, state),
    state.endpointMode == UnifiedTransactionEndpoint,
    state.unifiedEndpointDispatchCount == 0,
    state' = state
      .with("status", Applying)
      .with("writerOwner", mainTaskOwner)
      .with("reservationOwner", mainTaskOwner)
      .with("wholePreimageCaptured", true)
      .with("wholePreimageDigest", state.sourceFingerprint)
      .with("preimageCoverageDigest", canonicalPreimageCoverageDigest)
      .with("walPresent", true)
      .with("walBinding", state.task)
      .with("unifiedEndpointDispatchCount", 1),
  }

  action applyOperationOne = all {
    state.status == Applying,
    state.writerOwner == mainTaskOwner,
    state.reservationOwner == mainTaskOwner,
    state.walPresent,
    state.walBinding == state.task,
    state.wholePreimageCaptured,
    state.preimageCoverageDigest == canonicalPreimageCoverageDigest,
    state.appliedPrefix == 0,
    state.operationOneWrites == 0,
    state' = state
      .with("appliedPrefix", 1)
      .with("operationOneWrites", 1)
      .with("operationOneApplyOrder", 1)
      .with("targetMatchesWholePreimage", false),
  }

  action applyOperationTwo = all {
    state.status == Applying,
    state.writerOwner == mainTaskOwner,
    state.reservationOwner == mainTaskOwner,
    state.walBinding == state.task,
    state.appliedPrefix == 1,
    state.operationOneWrites == 1,
    state.operationTwoWrites == 0,
    state' = state
      .with("appliedPrefix", 2)
      .with("operationTwoWrites", 1)
      .with("operationTwoApplyOrder", 2),
  }

  action applyOperationThree = all {
    state.status == Applying,
    state.writerOwner == mainTaskOwner,
    state.reservationOwner == mainTaskOwner,
    state.walBinding == state.task,
    state.appliedPrefix == 2,
    state.operationTwoWrites == 1,
    state.operationThreeWrites == 0,
    state' = state
      .with("status", Verifying)
      .with("appliedPrefix", 3)
      .with("operationThreeWrites", 1)
      .with("operationThreeApplyOrder", 3),
  }

  action readBackOperationOne = all {
    state.status == Verifying,
    state.appliedPrefix == canonicalOperationCount,
    state.operationOneReadBack == ReadbackPending,
    state' = state.with("operationOneReadBack", ReadbackExact),
  }

  action readBackOperationTwo = all {
    state.status == Verifying,
    state.appliedPrefix == canonicalOperationCount,
    state.operationTwoReadBack == ReadbackPending,
    state' = state.with("operationTwoReadBack", ReadbackExact),
  }

  action readBackOperationThree = all {
    state.status == Verifying,
    state.appliedPrefix == canonicalOperationCount,
    state.operationThreeReadBack == ReadbackPending,
    state' = state.with("operationThreeReadBack", ReadbackExact),
  }

  // A mismatch is recorded only after the other two operation receipts are
  // exact. Once present, acceptance is disabled and the only transaction-state
  // transition is beginRollbackAfterReadbackMismatch.
  action readBackOperationOneMismatch = all {
    state.status == Verifying,
    state.operationOneReadBack == ReadbackPending,
    state.operationTwoReadBack == ReadbackExact,
    state.operationThreeReadBack == ReadbackExact,
    state' = state.with("operationOneReadBack", ReadbackMismatch),
  }

  action readBackOperationTwoMismatch = all {
    state.status == Verifying,
    state.operationOneReadBack == ReadbackExact,
    state.operationTwoReadBack == ReadbackPending,
    state.operationThreeReadBack == ReadbackExact,
    state' = state.with("operationTwoReadBack", ReadbackMismatch),
  }

  action readBackOperationThreeMismatch = all {
    state.status == Verifying,
    state.operationOneReadBack == ReadbackExact,
    state.operationTwoReadBack == ReadbackExact,
    state.operationThreeReadBack == ReadbackPending,
    state' = state.with("operationThreeReadBack", ReadbackMismatch),
  }

  action acceptWholeReadback = all {
    state.status == Verifying,
    everyOperationReadBackExact(state),
    state.appliedPrefix == canonicalOperationCount,
    state' = state
      .with("status", Verified)
      .with("receiptBinding", state.task)
      .with("postimageDigest", state.wholePreimageDigest + 1),
  }

  // One verified heterogeneous transaction publishes one canonical revision.
  action commitTransaction = all {
    state.status == Verified,
    everyOperationReadBackExact(state),
    state.receiptBinding == state.task,
    state.task == state.approvedBinding,
    state.walBinding == state.task,
    state.writerOwner == mainTaskOwner,
    state.reservationOwner == mainTaskOwner,
    state.canonicalCommitCount == 0,
    state.headRevision == state.task.baseRevision,
    state' = state
      .with("status", Committed)
      .with("headRevision", state.headRevision + 1)
      .with("sourceFingerprint", state.sourceFingerprint + 1)
      .with("canonicalCommitCount", 1)
      .with("terminalBinding", state.task)
      .with("writerOwner", noOwner)
      .with("reservationOwner", noOwner)
      .with("queueHead", noOwner)
      .with("mainQueuePosition", 0)
      .with("later", if (state.later.status == LaterAbsent)
        state.later
      else
        state.later.with("status", LaterStale).with("queuePosition", 0)),
  }

  action beginRollbackAfterPartialApply = all {
    state.status == Applying,
    state.appliedPrefix > 0,
    state.appliedPrefix < canonicalOperationCount,
    state.writerOwner == mainTaskOwner,
    state' = state.with("status", RollingBack),
  }

  // Readback mismatch is assessed only after all operations were observed.
  action beginRollbackAfterReadbackMismatch = all {
    state.status == Verifying,
    everyOperationReadBack(state),
    anyOperationReadBackMismatch(state),
    state.appliedPrefix == canonicalOperationCount,
    state.writerOwner == mainTaskOwner,
    state' = state.with("status", RollingBack),
  }

  action rollBackOperationThree = all {
    state.status == RollingBack,
    state.writerOwner == mainTaskOwner,
    state.appliedPrefix == 3,
    state' = state
      .with("appliedPrefix", 2)
      .with("rollbackWrites", state.rollbackWrites + 1)
      .with("firstRollbackIndex", if (state.rollbackWrites == 0) 3 else state.firstRollbackIndex)
      .with("lastRollbackIndex", 3),
  }

  action rollBackOperationTwo = all {
    state.status == RollingBack,
    state.writerOwner == mainTaskOwner,
    state.appliedPrefix == 2,
    state' = state
      .with("appliedPrefix", 1)
      .with("rollbackWrites", state.rollbackWrites + 1)
      .with("firstRollbackIndex", if (state.rollbackWrites == 0) 2 else state.firstRollbackIndex)
      .with("lastRollbackIndex", 2),
  }

  action rollBackOperationOne = all {
    state.status == RollingBack,
    state.writerOwner == mainTaskOwner,
    state.appliedPrefix == 1,
    state' = state
      .with("appliedPrefix", 0)
      .with("rollbackWrites", state.rollbackWrites + 1)
      .with("firstRollbackIndex", if (state.rollbackWrites == 0) 1 else state.firstRollbackIndex)
      .with("lastRollbackIndex", 1)
      .with("targetMatchesWholePreimage", true),
  }

  action finishRollback = all {
    state.status == RollingBack,
    state.appliedPrefix == 0,
    state.targetMatchesWholePreimage,
    state.writerOwner == mainTaskOwner,
    state.reservationOwner == mainTaskOwner,
    state' = state
      .with("status", RolledBack)
      .with("recoveryMode", NoRecovery)
      .with("terminalBinding", state.task)
      .with("writerOwner", noOwner)
      .with("reservationOwner", noOwner)
      .with("queueHead", noOwner)
      .with("mainQueuePosition", 0)
      .with("later", if (state.later.status == LaterAbsent)
        state.later
      else
        state.later.with("status", LaterStale).with("queuePosition", 0)),
  }

  // A crash may resume at the first unapplied operation. A failed rollback is
  // fail-closed and can only resume from the same approved WAL binding.
  action crashAfterPartialApply = all {
    state.status == Applying,
    state.appliedPrefix > 0,
    state.appliedPrefix < canonicalOperationCount,
    state.writerOwner == mainTaskOwner,
    state.reservationOwner == mainTaskOwner,
    state' = state
      .with("status", RecoveryRequired)
      .with("recoveryMode", ResumeApply)
      .with("recoveryBinding", state.task)
      .with("writerOwner", noOwner),
  }

  action failRollback = all {
    state.status == RollingBack,
    state.appliedPrefix > 0,
    state.writerOwner == mainTaskOwner,
    state.reservationOwner == mainTaskOwner,
    state' = state
      .with("status", RecoveryRequired)
      .with("recoveryMode", ResumeRollback)
      .with("recoveryBinding", state.task)
      .with("writerOwner", noOwner),
  }

  action resumeExactApply = all {
    state.status == RecoveryRequired,
    state.recoveryMode == ResumeApply,
    state.recoveryBinding == state.task,
    state.approvedBinding == state.task,
    state.walBinding == state.task,
    any {
      state.presentedRecoveryBinding == emptyBinding,
      state.presentedRecoveryBinding == state.recoveryBinding,
    },
    state.writerOwner == noOwner,
    state.reservationOwner == mainTaskOwner,
    state' = state
      .with("status", Applying)
      .with("recoveryMode", NoRecovery)
      .with("writerOwner", mainTaskOwner),
  }

  action resumeExactRollback = all {
    state.status == RecoveryRequired,
    state.recoveryMode == ResumeRollback,
    state.recoveryBinding == state.task,
    state.approvedBinding == state.task,
    state.walBinding == state.task,
    any {
      state.presentedRecoveryBinding == emptyBinding,
      state.presentedRecoveryBinding == state.recoveryBinding,
    },
    state.writerOwner == noOwner,
    state.reservationOwner == mainTaskOwner,
    state' = state
      .with("status", RollingBack)
      .with("recoveryMode", NoRecovery)
      .with("writerOwner", mainTaskOwner),
  }

  action presentMismatchedRecoveryAttempt = all {
    state.status == RecoveryRequired,
    state.recoveryBinding == state.task,
    state.presentedRecoveryBinding == emptyBinding,
    state.recoveryBinding.requestDigest + 1 != state.recoveryBinding.requestDigest,
    state' = state.with("presentedRecoveryBinding", state.recoveryBinding.with(
      "requestDigest",
      state.recoveryBinding.requestDigest + 1
    )),
  }

  action rejectMismatchedRecovery = all {
    state.status == RecoveryRequired,
    state.presentedRecoveryBinding != emptyBinding,
    state.presentedRecoveryBinding != state.recoveryBinding,
    state.presentedRecoveryBinding.requestDigest
      != state.recoveryBinding.requestDigest,
    state' = state
      .with("mismatchedRecoveryRejectCount", state.mismatchedRecoveryRejectCount + 1)
      .with("lastRejectedRecoveryBinding", state.presentedRecoveryBinding)
      .with("presentedRecoveryBinding", emptyBinding),
  }

  // Terminal retries are receipt lookups. They neither re-dispatch the bridge
  // endpoint nor mutate YMM/canonical state.
  action replayCommitted = all {
    state.status == Committed,
    state.terminalBinding == state.task,
    any {
      state.presentedTerminalBinding == emptyBinding,
      state.presentedTerminalBinding == state.terminalBinding,
    },
    state' = state.with("terminalReplayCount", state.terminalReplayCount + 1),
  }

  action replayRolledBack = all {
    state.status == RolledBack,
    state.terminalBinding == state.task,
    any {
      state.presentedTerminalBinding == emptyBinding,
      state.presentedTerminalBinding == state.terminalBinding,
    },
    state' = state.with("terminalReplayCount", state.terminalReplayCount + 1),
  }

  action presentMismatchedTerminalReplay = all {
    any { state.status == Committed, state.status == RolledBack },
    state.terminalBinding == state.task,
    state.presentedTerminalBinding == emptyBinding,
    state.terminalBinding.requestDigest + 1 != state.terminalBinding.requestDigest,
    state' = state.with("presentedTerminalBinding", state.terminalBinding.with(
      "requestDigest",
      state.terminalBinding.requestDigest + 1
    )),
  }

  action rejectMismatchedTerminalReplay = all {
    any { state.status == Committed, state.status == RolledBack },
    state.presentedTerminalBinding != emptyBinding,
    state.presentedTerminalBinding != state.terminalBinding,
    state.presentedTerminalBinding.requestDigest
      != state.terminalBinding.requestDigest,
    state' = state
      .with("mismatchedReplayRejectCount", state.mismatchedReplayRejectCount + 1)
      .with("lastRejectedTerminalBinding", state.presentedTerminalBinding)
      .with("presentedTerminalBinding", emptyBinding),
  }

  // A later task is not silently rebased. Repreview creates a new digest from
  // the new head/source/capability tuple, after which a new approval is needed.
  action repreviewLaterTask = all {
    state.later.status == LaterStale,
    any { state.status == Committed, state.status == RolledBack },
    state' = state.with("later", state.later
      .with("status", LaterPreviewable)
      .with("repreviewCount", state.later.repreviewCount + 1)
      .with("approvedBinding", emptyBinding)
      .with("binding", makeBinding(
        laterTaskOwner,
        sealedDigest(
          laterTaskOwner,
          state.projectId,
          state.sceneId,
          state.headRevision,
          state.sourceFingerprint,
          state.capabilityRevision,
          computeDescriptorListDigest(state)
        ),
        state.projectId,
        state.sceneId,
        state.headRevision,
        state.sourceFingerprint,
        state.capabilityRevision,
        computeDescriptorListDigest(state)
      ))),
  }

  // Invalid ordinary-edit targets fail at staging, before preparation, WAL,
  // endpoint dispatch, or mutation.
  action rejectUnmanagedTargetAtStage = all {
    state.status == Draft,
    state' = state
      .with("status", Rejected)
      .with("rejectionReason", UnmanagedTarget)
      .with("operationOne", state.operationOne.with("targetManaged", false)),
  }

  action rejectLockedTargetAtStage = all {
    state.status == Draft,
    state' = state
      .with("status", Rejected)
      .with("rejectionReason", LockedTarget)
      .with("operationTwo", state.operationTwo.with("targetUnlocked", false)),
  }

  action rejectGlobalSettingAtStage = all {
    state.status == Draft,
    state' = state
      .with("status", Rejected)
      .with("rejectionReason", GlobalSettingTarget)
      .with("operationThree", state.operationThree.with(
        "targetScope",
        GlobalProjectSettingTarget
      )),
  }

  // Production accepts one through 128 operations. Three is the smallest
  // representative batch that exposes multiple semantic families, arbitrary
  // preparation order, and a dependency spanning a middle operation.
  action rejectOperationCountAtStage = all {
    state.status == Draft,
    state' = state
      .with("status", Rejected)
      .with("rejectionReason", OperationCountOutOfBounds)
      .with("requestedOperationCount", 129),
  }

  // Two descriptors may not claim the same field/artifact write footprint.
  // A future commutative merge rule must become an explicit descriptor rule;
  // ordinary overlap fails closed rather than relying on apply timing.
  action rejectOverlappingWriteFootprintAtStage = all {
    state.status == Draft,
    state' = state
      .with("status", Rejected)
      .with("rejectionReason", OverlappingWriteFootprint)
      .with("writeFootprintsDisjoint", false)
      .with("writeFootprintEvidenceDigest", state.writeFootprintEvidenceDigest + 1),
  }

  // References may depend on an earlier create in the sealed canonical order,
  // never on a later operation or on preparation completion order.
  action rejectIllegalDependencyOrderAtStage = all {
    state.status == Draft,
    state' = state
      .with("status", Rejected)
      .with("rejectionReason", IllegalDependencyOrder)
      .with("dependenciesRespectCanonicalOrder", false)
      .with("dependencyEvidenceDigest", state.dependencyEvidenceDigest + 1),
  }

  // A caller already chained two legacy endpoints and observed the first
  // mutation before presenting the request as if it were atomic. The atomic
  // facade rejects that history: it records the outside partial effect but
  // neither dispatches a unified transaction nor claims it rolled back an
  // operation that happened outside its WAL/gate.
  action rejectPseudoBatchAtStage = all {
    state.status == Draft,
    state' = state
      .with("status", Rejected)
      .with("rejectionReason", PseudoBatchAcrossEndpoints)
      .with("endpointMode", LegacyKindEndpoints)
      .with("legacyEndpointDispatchAttemptCount", 2)
      .with("legacyEndpointMutationCount", 1)
      .with("atomicRollbackClaimCount", 0),
  }

  action terminalStutter = all {
    any {
      state.status == Committed,
      state.status == RolledBack,
      state.status == Rejected,
    },
    state' = state,
  }

  action step = any {
    stageTransaction,
    prepareOperationOne,
    prepareOperationTwo,
    prepareOperationThree,
    sealPreview,
    approve,
    enqueueMain,
    stageLaterTask,
    approveLaterTask,
    enqueueLaterTask,
    rejectLaterGateBypass,
    externalCommitBeforeAcquire,
    capabilityDriftBeforeAcquire,
    descriptorDriftBeforeAcquire,
    writeFootprintEvidenceDriftBeforeAcquire,
    dependencyEvidenceDriftBeforeAcquire,
    repreviewMain,
    acquireUnifiedWriter,
    applyOperationOne,
    applyOperationTwo,
    applyOperationThree,
    readBackOperationOne,
    readBackOperationTwo,
    readBackOperationThree,
    readBackOperationOneMismatch,
    readBackOperationTwoMismatch,
    readBackOperationThreeMismatch,
    acceptWholeReadback,
    commitTransaction,
    beginRollbackAfterPartialApply,
    beginRollbackAfterReadbackMismatch,
    rollBackOperationThree,
    rollBackOperationTwo,
    rollBackOperationOne,
    finishRollback,
    crashAfterPartialApply,
    failRollback,
    resumeExactApply,
    resumeExactRollback,
    presentMismatchedRecoveryAttempt,
    rejectMismatchedRecovery,
    replayCommitted,
    replayRolledBack,
    presentMismatchedTerminalReplay,
    rejectMismatchedTerminalReplay,
    repreviewLaterTask,
    rejectUnmanagedTargetAtStage,
    rejectLockedTargetAtStage,
    rejectGlobalSettingAtStage,
    rejectOperationCountAtStage,
    rejectOverlappingWriteFootprintAtStage,
    rejectIllegalDependencyOrderAtStage,
    rejectPseudoBatchAtStage,
    terminalStutter,
  }

  val stagedPreviewContainsWholeOrderedHeterogeneousBatch =
    not(any {
      state.status == Previewable,
      approvalMustRemainExact(state.status),
    })
      or and {
        everyOperationPrepared(state),
        hasSealedMainTask(state),
        descriptorIsHeterogeneous(state),
        state.operationOnePreparationOrder > 0,
        state.operationTwoPreparationOrder > 0,
        state.operationThreePreparationOrder > 0,
        state.operationOnePreparationOrder != state.operationTwoPreparationOrder,
        state.operationOnePreparationOrder != state.operationThreePreparationOrder,
        state.operationTwoPreparationOrder != state.operationThreePreparationOrder,
      }

  val approvalIsDescriptorCapabilityAndSourceBound =
    not(approvalMustRemainExact(state.status))
      or and {
        state.approvedBinding == state.task,
        state.approvedBinding.requestDigest == state.task.requestDigest,
        state.approvedBinding.descriptorListDigest == state.task.descriptorListDigest,
        state.task.descriptorListDigest == computeDescriptorListDigest(state),
        state.approvedBinding.capabilityRevision == state.task.capabilityRevision,
        state.approvedBinding.projectId == state.task.projectId,
        state.approvedBinding.sceneId == state.task.sceneId,
        state.approvedBinding.baseRevision == state.task.baseRevision,
        state.approvedBinding.sourceFingerprint == state.task.sourceFingerprint,
      }

  val preparationCompletionDoesNotChooseApplyOrder = and {
    state.operationOneWrites <= 1,
    state.operationTwoWrites <= 1,
    state.operationThreeWrites <= 1,
    state.operationOneApplyOrder == 0 or state.operationOneApplyOrder == 1,
    state.operationTwoApplyOrder == 0 or state.operationTwoApplyOrder == 2,
    state.operationThreeApplyOrder == 0 or state.operationThreeApplyOrder == 3,
    state.operationTwoWrites == 0
      or and {
        state.operationOneWrites == 1,
        state.operationOneApplyOrder == 1,
        state.operationTwoApplyOrder == 2,
      },
    state.operationThreeWrites == 0
      or and {
        state.operationOneWrites == 1,
        state.operationTwoWrites == 1,
        state.operationThreeApplyOrder == 3,
      },
  }

  val singleWriterQueueAndGateAreExclusive = and {
    state.writerOwner == noOwner or state.writerOwner == mainTaskOwner,
    state.reservationOwner == noOwner or state.reservationOwner == mainTaskOwner,
    state.writerOwner == noOwner or state.writerOwner == state.reservationOwner,
    state.mainQueuePosition == 0 or state.mainQueuePosition == 1,
    state.later.queuePosition == 0 or state.later.queuePosition == 2,
    state.later.status != LaterQueued
      or and {
        state.later.queuePosition == 2,
        state.queueHead == mainTaskOwner,
      },
    state.writerOwner != mainTaskOwner
      or state.queueHead == mainTaskOwner,
  }

  val wholePreimageAndWalPrecedeEveryMutation =
    state.operationOneWrites
      + state.operationTwoWrites
      + state.operationThreeWrites
      + state.rollbackWrites
      == 0
      or and {
        state.wholePreimageCaptured,
        state.wholePreimageDigest == state.task.sourceFingerprint,
        state.preimageCoverageDigest == canonicalPreimageCoverageDigest,
        state.walPresent,
        state.walBinding == state.task,
        state.approvedBinding == state.task,
      }

  val rollbackUsesReverseOrderAndWholePreimage = and {
    state.rollbackWrites <= canonicalOperationCount,
    state.firstRollbackIndex == 0
      or state.firstRollbackIndex >= state.lastRollbackIndex,
    state.status != RolledBack
      or and {
        state.appliedPrefix == 0,
        state.targetMatchesWholePreimage,
        state.canonicalCommitCount == 0,
        state.writerOwner == noOwner,
        state.reservationOwner == noOwner,
      },
  }

  val unresolvedPartialApplyFailsClosed =
    state.status != RecoveryRequired
      or and {
        state.appliedPrefix > 0,
        state.appliedPrefix <= canonicalOperationCount,
        state.writerOwner == noOwner,
        state.reservationOwner == mainTaskOwner,
        state.walPresent,
        state.walBinding == state.task,
        state.recoveryBinding == state.task,
        state.canonicalCommitCount == 0,
      }

  val allOperationsReadBackBeforeOneRevision =
    state.canonicalCommitCount == 0
      or and {
        everyOperationReadBackExact(state),
        state.appliedPrefix == canonicalOperationCount,
        state.operationOneWrites == 1,
        state.operationTwoWrites == 1,
        state.operationThreeWrites == 1,
        state.receiptBinding == state.task,
        state.terminalBinding == state.task,
      }

  val readbackMismatchCanOnlyRollBack = and {
    not(anyOperationReadBackMismatch(state))
      or and {
        not(any { state.status == Verified, state.status == Committed }),
        state.receiptBinding == emptyBinding,
        any {
          state.status == Verifying,
          state.status == RollingBack,
          state.status == RecoveryRequired,
          state.status == RolledBack,
        },
      },
    not(any { state.status == Verified, state.status == Committed })
      or and {
        everyOperationReadBackExact(state),
        not(anyOperationReadBackMismatch(state)),
      },
  }

  val durableBindingsSealDescriptorList = and {
    state.task.taskId != mainTaskOwner
      or state.task.requestDigest == missing
      or state.task.requestDigest
        == sealedDigest(
          state.task.taskId,
          state.task.projectId,
          state.task.sceneId,
          state.task.baseRevision,
          state.task.sourceFingerprint,
          state.task.capabilityRevision,
          state.task.descriptorListDigest
        ),
    not(state.walPresent)
      or and {
        state.walBinding == state.approvedBinding,
        state.walBinding.descriptorListDigest == state.task.descriptorListDigest,
      },
    state.status != RecoveryRequired
      or and {
        state.recoveryBinding == state.walBinding,
        state.recoveryBinding.descriptorListDigest == state.task.descriptorListDigest,
      },
    not(any { state.status == Committed, state.status == RolledBack })
      or and {
        state.terminalBinding == state.task,
        state.terminalBinding.descriptorListDigest == state.task.descriptorListDigest,
      },
    state.receiptBinding == emptyBinding
      or and {
        state.receiptBinding == state.task,
        state.receiptBinding.descriptorListDigest == state.task.descriptorListDigest,
      },
    state.later.binding == emptyBinding
      or state.later.binding.requestDigest
        == sealedDigest(
          state.later.binding.taskId,
          state.later.binding.projectId,
          state.later.binding.sceneId,
          state.later.binding.baseRevision,
          state.later.binding.sourceFingerprint,
          state.later.binding.capabilityRevision,
          state.later.binding.descriptorListDigest
        ),
  }

  val oneTransactionCreatesOneCanonicalRevision = and {
    state.canonicalCommitCount <= 1,
    state.headRevision
      == initialRevision + state.externalCommitCount + state.canonicalCommitCount,
    state.status != Committed
      or and {
        state.canonicalCommitCount == 1,
        state.headRevision == state.task.baseRevision + 1,
      },
  }

  val exactTerminalReplayIsMutationFree = and {
    state.operationOneWrites <= 1,
    state.operationTwoWrites <= 1,
    state.operationThreeWrites <= 1,
    state.canonicalCommitCount <= 1,
    state.terminalReplayCount == 0
      or and {
        any { state.status == Committed, state.status == RolledBack },
        state.terminalBinding == state.task,
      },
  }

  val mismatchedAttemptsAreRejectedWithEvidence = and {
    state.presentedRecoveryBinding == emptyBinding
      or and {
        state.status == RecoveryRequired,
        state.presentedRecoveryBinding != state.recoveryBinding,
        state.presentedRecoveryBinding.requestDigest
          != state.recoveryBinding.requestDigest,
      },
    state.mismatchedRecoveryRejectCount == 0
      or and {
        state.lastRejectedRecoveryBinding != emptyBinding,
        state.lastRejectedRecoveryBinding != state.recoveryBinding,
        state.lastRejectedRecoveryBinding.requestDigest
          != state.recoveryBinding.requestDigest,
      },
    state.presentedTerminalBinding == emptyBinding
      or and {
        any { state.status == Committed, state.status == RolledBack },
        state.presentedTerminalBinding != state.terminalBinding,
        state.presentedTerminalBinding.requestDigest
          != state.terminalBinding.requestDigest,
      },
    state.mismatchedReplayRejectCount == 0
      or and {
        state.lastRejectedTerminalBinding != emptyBinding,
        state.lastRejectedTerminalBinding != state.terminalBinding,
        state.lastRejectedTerminalBinding.requestDigest
          != state.terminalBinding.requestDigest,
      },
  }

  val laterStagedTasksBecomeStaleAndRequireRepreview = and {
    not(and {
      state.laterWasStaged,
      any { state.status == Committed, state.status == RolledBack },
      state.later.repreviewCount == 0,
    })
      or state.later.status == LaterStale,
    state.later.repreviewCount == 0
      or and {
        any {
          state.later.status == LaterPreviewable,
          state.later.status == LaterApproved,
        },
        bindingMatchesActiveSource(state.later.binding, state),
        state.later.binding.requestDigest
          == sealedDigest(
            laterTaskOwner,
            state.later.binding.projectId,
            state.later.binding.sceneId,
            state.later.binding.baseRevision,
            state.later.binding.sourceFingerprint,
            state.later.binding.capabilityRevision,
            state.later.binding.descriptorListDigest
          ),
      },
  }

  val invalidTargetsFailBeforePreparationOrMutation =
    state.status != Rejected
      or and {
        state.rejectionReason != NoRejection,
        not(state.operationOnePrepared),
        not(state.operationTwoPrepared),
        not(state.operationThreePrepared),
        not(state.walPresent),
        not(state.wholePreimageCaptured),
        state.appliedPrefix == 0,
        state.canonicalCommitCount == 0,
        state.unifiedEndpointDispatchCount == 0,
        state.rejectionReason == PseudoBatchAcrossEndpoints
          or state.legacyEndpointMutationCount == 0,
        any {
          and {
            state.rejectionReason == UnmanagedTarget,
            not(state.operationOne.targetManaged),
          },
          and {
            state.rejectionReason == LockedTarget,
            not(state.operationTwo.targetUnlocked),
          },
          and {
            state.rejectionReason == GlobalSettingTarget,
            state.operationThree.targetScope == GlobalProjectSettingTarget,
          },
          and {
            state.rejectionReason == OperationCountOutOfBounds,
            any {
              state.requestedOperationCount < 1,
              state.requestedOperationCount > 128,
            },
          },
          and {
            state.rejectionReason == OverlappingWriteFootprint,
            not(state.writeFootprintsDisjoint),
          },
          and {
            state.rejectionReason == IllegalDependencyOrder,
            not(state.dependenciesRespectCanonicalOrder),
          },
          and {
            state.rejectionReason == PseudoBatchAcrossEndpoints,
            state.endpointMode == LegacyKindEndpoints,
          },
        },
      }

  val noPseudoBatchAcrossEndpoints = and {
    state.unifiedEndpointDispatchCount <= 1,
    state.legacyEndpointDispatchAttemptCount == 0
      or and {
        state.status == Rejected,
        state.rejectionReason == PseudoBatchAcrossEndpoints,
        state.endpointMode == LegacyKindEndpoints,
        state.legacyEndpointDispatchAttemptCount == 2,
        state.legacyEndpointMutationCount == 1,
        state.atomicRollbackClaimCount == 0,
        state.unifiedEndpointDispatchCount == 0,
        state.operationOneWrites == 0,
        state.operationTwoWrites == 0,
        state.operationThreeWrites == 0,
        state.canonicalCommitCount == 0,
      },
    state.operationOneWrites + state.operationTwoWrites + state.operationThreeWrites == 0
      or and {
        state.endpointMode == UnifiedTransactionEndpoint,
        state.unifiedEndpointDispatchCount == 1,
        state.legacyEndpointDispatchAttemptCount == 0,
        state.legacyEndpointMutationCount == 0,
      },
    state.canonicalCommitCount == 0
      or and {
        state.endpointMode == UnifiedTransactionEndpoint,
        state.unifiedEndpointDispatchCount == 1,
      },
  }

  val acceptedDescriptorListIsBoundedConflictFreeAndDependencyLegal =
    state.status == Rejected
      or and {
        state.requestedOperationCount >= 1,
        state.requestedOperationCount <= 128,
        state.requestedOperationCount == canonicalOperationCount,
        state.writeFootprintsDisjoint,
        state.dependenciesRespectCanonicalOrder,
      }

  val staleApprovedTransactionNeverMutates =
    state.status != RepreviewRequired
      or and {
        state.appliedPrefix == 0,
        state.operationOneWrites == 0,
        state.operationTwoWrites == 0,
        state.operationThreeWrites == 0,
        state.canonicalCommitCount == 0,
        state.writerOwner == noOwner,
        state.reservationOwner == noOwner,
      }

  val editTransactionSafety = and {
    stagedPreviewContainsWholeOrderedHeterogeneousBatch,
    approvalIsDescriptorCapabilityAndSourceBound,
    preparationCompletionDoesNotChooseApplyOrder,
    singleWriterQueueAndGateAreExclusive,
    wholePreimageAndWalPrecedeEveryMutation,
    rollbackUsesReverseOrderAndWholePreimage,
    unresolvedPartialApplyFailsClosed,
    allOperationsReadBackBeforeOneRevision,
    readbackMismatchCanOnlyRollBack,
    durableBindingsSealDescriptorList,
    oneTransactionCreatesOneCanonicalRevision,
    exactTerminalReplayIsMutationFree,
    mismatchedAttemptsAreRejectedWithEvidence,
    laterStagedTasksBecomeStaleAndRequireRepreview,
    invalidTargetsFailBeforePreparationOrMutation,
    noPseudoBatchAcrossEndpoints,
    acceptedDescriptorListIsBoundedConflictFreeAndDependencyLegal,
    staleApprovedTransactionNeverMutates,
  }

  run reversePreparationCommitsHeterogeneousBatchOnceTest = init
    .then(stageTransaction)
    .then(prepareOperationThree)
    .then(prepareOperationTwo)
    .then(prepareOperationOne)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(applyOperationThree)
    .then(readBackOperationThree)
    .then(readBackOperationOne)
    .then(readBackOperationTwo)
    .then(acceptWholeReadback)
    .then(commitTransaction)
    .then(replayCommitted)
    .then(replayCommitted)
    .expect(and {
      editTransactionSafety,
      state.status == Committed,
      descriptorIsHeterogeneous(state),
      state.operationThreePreparationOrder == 1,
      state.operationTwoPreparationOrder == 2,
      state.operationOnePreparationOrder == 3,
      state.operationOneApplyOrder == 1,
      state.operationTwoApplyOrder == 2,
      state.operationThreeApplyOrder == 3,
      state.canonicalCommitCount == 1,
      state.headRevision == initialRevision + 1,
      state.terminalReplayCount == 2,
      state.unifiedEndpointDispatchCount == 1,
      state.task.descriptorListDigest == computeDescriptorListDigest(state),
      state.walBinding.descriptorListDigest == state.task.descriptorListDigest,
      state.receiptBinding.descriptorListDigest == state.task.descriptorListDigest,
      state.terminalBinding.descriptorListDigest == state.task.descriptorListDigest,
    })

  run laterQueuedTaskBecomesStaleAndRepreviewsTest = init
    .then(stageTransaction)
    .then(prepareOperationTwo)
    .then(prepareOperationOne)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(stageLaterTask)
    .then(approveLaterTask)
    .then(enqueueLaterTask)
    .then(rejectLaterGateBypass)
    .then(acquireUnifiedWriter)
    .then(rejectLaterGateBypass)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(applyOperationThree)
    .then(readBackOperationOne)
    .then(readBackOperationTwo)
    .then(readBackOperationThree)
    .then(acceptWholeReadback)
    .then(commitTransaction)
    .then(repreviewLaterTask)
    .then(approveLaterTask)
    .expect(and {
      editTransactionSafety,
      state.status == Committed,
      state.gateRejectCount == 2,
      state.later.repreviewCount == 1,
      state.later.status == LaterApproved,
      state.later.binding.baseRevision == state.headRevision,
      state.later.binding.sourceFingerprint == state.sourceFingerprint,
      state.later.approvedBinding == state.later.binding,
    })

  run partialFailureRollsBackWholePreimageTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationThree)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(beginRollbackAfterPartialApply)
    .then(rollBackOperationTwo)
    .then(rollBackOperationOne)
    .then(finishRollback)
    .then(replayRolledBack)
    .expect(and {
      editTransactionSafety,
      state.status == RolledBack,
      state.appliedPrefix == 0,
      state.targetMatchesWholePreimage,
      state.firstRollbackIndex == 2,
      state.lastRollbackIndex == 1,
      state.rollbackWrites == 2,
      state.canonicalCommitCount == 0,
      state.headRevision == initialRevision,
      state.terminalReplayCount == 1,
    })

  run fullReadbackMismatchRollsBackInReverseOrderTest = init
    .then(stageTransaction)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(prepareOperationOne)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(applyOperationThree)
    .then(readBackOperationOne)
    .then(readBackOperationThree)
    .then(readBackOperationTwoMismatch)
    .then(beginRollbackAfterReadbackMismatch)
    .then(rollBackOperationThree)
    .then(rollBackOperationTwo)
    .then(rollBackOperationOne)
    .then(finishRollback)
    .expect(and {
      editTransactionSafety,
      state.status == RolledBack,
      state.firstRollbackIndex == 3,
      state.lastRollbackIndex == 1,
      state.rollbackWrites == 3,
      state.targetMatchesWholePreimage,
      state.operationTwoReadBack == ReadbackMismatch,
      state.canonicalCommitCount == 0,
    })

  run rollbackFailureRetainsExactReservationTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(beginRollbackAfterPartialApply)
    .then(rollBackOperationTwo)
    .then(failRollback)
    .then(presentMismatchedRecoveryAttempt)
    .then(rejectMismatchedRecovery)
    .expect(and {
      editTransactionSafety,
      state.status == RecoveryRequired,
      state.recoveryMode == ResumeRollback,
      state.writerOwner == noOwner,
      state.reservationOwner == mainTaskOwner,
      state.recoveryBinding == state.task,
      state.mismatchedRecoveryRejectCount == 1,
      state.presentedRecoveryBinding == emptyBinding,
      state.lastRejectedRecoveryBinding != state.recoveryBinding,
      state.lastRejectedRecoveryBinding.requestDigest
        != state.recoveryBinding.requestDigest,
      state.canonicalCommitCount == 0,
    })

  run failedRollbackResumesExactPreimageRestoreTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(beginRollbackAfterPartialApply)
    .then(rollBackOperationTwo)
    .then(failRollback)
    .then(resumeExactRollback)
    .then(rollBackOperationOne)
    .then(finishRollback)
    .expect(and {
      editTransactionSafety,
      state.status == RolledBack,
      state.firstRollbackIndex == 2,
      state.lastRollbackIndex == 1,
      state.rollbackWrites == 2,
      state.targetMatchesWholePreimage,
      state.canonicalCommitCount == 0,
    })

  run exactCrashResumeDoesNotReplayPrefixTest = init
    .then(stageTransaction)
    .then(prepareOperationThree)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(crashAfterPartialApply)
    .then(resumeExactApply)
    .then(applyOperationTwo)
    .then(applyOperationThree)
    .then(readBackOperationTwo)
    .then(readBackOperationThree)
    .then(readBackOperationOne)
    .then(acceptWholeReadback)
    .then(commitTransaction)
    .expect(and {
      editTransactionSafety,
      state.status == Committed,
      state.operationOneWrites == 1,
      state.operationTwoWrites == 1,
      state.operationThreeWrites == 1,
      state.canonicalCommitCount == 1,
      state.unifiedEndpointDispatchCount == 1,
    })

  run oneMissingReadbackCannotPublishTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(applyOperationThree)
    .then(readBackOperationOne)
    .then(readBackOperationThree)
    .expect(and {
      editTransactionSafety,
      state.status == Verifying,
      state.operationOneReadBack == ReadbackExact,
      state.operationTwoReadBack == ReadbackPending,
      state.operationThreeReadBack == ReadbackExact,
      state.canonicalCommitCount == 0,
      state.headRevision == initialRevision,
    })

  run mismatchedTerminalReplayIsRejectedBeforeExactReplayTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(acquireUnifiedWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(applyOperationThree)
    .then(readBackOperationOne)
    .then(readBackOperationTwo)
    .then(readBackOperationThree)
    .then(acceptWholeReadback)
    .then(commitTransaction)
    .then(presentMismatchedTerminalReplay)
    .then(rejectMismatchedTerminalReplay)
    .then(replayCommitted)
    .expect(and {
      editTransactionSafety,
      state.status == Committed,
      state.mismatchedReplayRejectCount == 1,
      state.presentedTerminalBinding == emptyBinding,
      state.lastRejectedTerminalBinding != state.terminalBinding,
      state.lastRejectedTerminalBinding.requestDigest
        != state.terminalBinding.requestDigest,
      state.terminalReplayCount == 1,
      state.canonicalCommitCount == 1,
    })

  run sourceDriftRequiresWholeRepreviewTest = init
    .then(stageTransaction)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(prepareOperationOne)
    .then(sealPreview)
    .then(approve)
    .then(enqueueMain)
    .then(externalCommitBeforeAcquire)
    .then(repreviewMain)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .expect(and {
      editTransactionSafety,
      state.status == Approved,
      state.task.baseRevision == initialRevision + 1,
      state.task.sourceFingerprint == initialSourceFingerprint + 1,
      state.task == state.approvedBinding,
      state.operationOneWrites == 0,
      state.canonicalCommitCount == 0,
    })

  run capabilityDriftInvalidatesApprovalTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(capabilityDriftBeforeAcquire)
    .expect(and {
      editTransactionSafety,
      state.status == RepreviewRequired,
      state.task.capabilityRevision != state.capabilityRevision,
      state.operationOneWrites == 0,
      state.unifiedEndpointDispatchCount == 0,
    })

  run descriptorDriftInvalidatesApprovalTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(descriptorDriftBeforeAcquire)
    .expect(and {
      editTransactionSafety,
      state.status == RepreviewRequired,
      state.task.descriptorListDigest != computeDescriptorListDigest(state),
      state.operationOneWrites == 0,
      state.unifiedEndpointDispatchCount == 0,
    })

  run writeFootprintEvidenceDriftInvalidatesApprovalTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(writeFootprintEvidenceDriftBeforeAcquire)
    .expect(and {
      editTransactionSafety,
      state.status == RepreviewRequired,
      state.task.descriptorListDigest != computeDescriptorListDigest(state),
      state.operationOneWrites == 0,
      state.unifiedEndpointDispatchCount == 0,
    })

  run dependencyEvidenceDriftInvalidatesApprovalTest = init
    .then(stageTransaction)
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(prepareOperationThree)
    .then(sealPreview)
    .then(approve)
    .then(dependencyEvidenceDriftBeforeAcquire)
    .expect(and {
      editTransactionSafety,
      state.status == RepreviewRequired,
      state.task.descriptorListDigest != computeDescriptorListDigest(state),
      state.operationOneWrites == 0,
      state.unifiedEndpointDispatchCount == 0,
    })

  run unmanagedTargetFailsBeforePreparationTest = init
    .then(rejectUnmanagedTargetAtStage)
    .expect(and {
      editTransactionSafety,
      state.status == Rejected,
      state.rejectionReason == UnmanagedTarget,
    })

  run lockedTargetFailsBeforePreparationTest = init
    .then(rejectLockedTargetAtStage)
    .expect(and {
      editTransactionSafety,
      state.status == Rejected,
      state.rejectionReason == LockedTarget,
    })

  run globalSettingFailsBeforePreparationTest = init
    .then(rejectGlobalSettingAtStage)
    .expect(and {
      editTransactionSafety,
      state.status == Rejected,
      state.rejectionReason == GlobalSettingTarget,
    })

  run operationCountAboveBoundFailsBeforePreparationTest = init
    .then(rejectOperationCountAtStage)
    .expect(and {
      editTransactionSafety,
      state.status == Rejected,
      state.rejectionReason == OperationCountOutOfBounds,
      state.requestedOperationCount == 129,
    })

  run overlappingWriteFootprintFailsBeforePreparationTest = init
    .then(rejectOverlappingWriteFootprintAtStage)
    .expect(and {
      editTransactionSafety,
      state.status == Rejected,
      state.rejectionReason == OverlappingWriteFootprint,
      not(state.writeFootprintsDisjoint),
    })

  run illegalDependencyOrderFailsBeforePreparationTest = init
    .then(rejectIllegalDependencyOrderAtStage)
    .expect(and {
      editTransactionSafety,
      state.status == Rejected,
      state.rejectionReason == IllegalDependencyOrder,
      not(state.dependenciesRespectCanonicalOrder),
    })

  run pseudoBatchAcrossLegacyEndpointsIsRejectedTest = init
    .then(rejectPseudoBatchAtStage)
    .expect(and {
      editTransactionSafety,
      state.status == Rejected,
      state.rejectionReason == PseudoBatchAcrossEndpoints,
      state.legacyEndpointDispatchAttemptCount == 2,
      state.legacyEndpointMutationCount == 1,
      state.atomicRollbackClaimCount == 0,
      state.unifiedEndpointDispatchCount == 0,
      state.canonicalCommitCount == 0,
    })
}
