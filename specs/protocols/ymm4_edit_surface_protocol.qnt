// YMM4 field-level edit-surface protocol.
//
// This model defines the common admission and approval boundary for field
// edits. It intentionally leaves graph references, paint order, keyframes,
// scenes, project settings, characters, and templates to their own protocols.
//
// Every request is bound to an advertised capability, a schema, a typed item
// descriptor, an authenticated observation, ownership, and a source snapshot.
// Unsupported, unobserved, unmanaged, locked, or stale requests fail closed.
// Partial patches normalize absent/reset/value/explicit-no-op forms before the
// plan is sealed. Updates preserve every untouched known field and every
// unknown YMM field, and a canonical commit requires exact readback of each
// touched field. Expanding the surface changes its semantic binding and makes
// an older approval unusable until fresh observation, staging, and approval.
module ymm4EditSurfaceProtocol {
  type EditStatus =
    | Draft
    | Normalized
    | Admitted
    | Previewable
    | Approved
    | Verifying
    | Verified
    | Committed
    | Rejected
    | VerificationFailed
    | Conflicted

  type CrudKind =
    | Create
    | Update
    | Delete

  // Field-editable item families. Structural containers and project-level
  // objects are outside this module.
  type ItemFamily =
    | ManagedVoiceItem
    | ManagedAudioItem
    | ManagedTextItem
    | ShapeItem
    | EffectItem
    | FrameBufferItem
    | GenericYmmItem

  // The supported field catalog is semantic, not a reflection field bag.
  // UnknownField is observable and preservable, but never directly writable.
  type FieldClass =
    | TimingField
    | TransformPositionField
    | TransformScaleField
    | TransformRotationField
    | OpacityVisibilityField
    | VisualStyleField
    | TextContentField
    | TextStyleField
    | AudioSourceField
    | AudioParametersField
    | MaskField
    | LockField
    | TypedEffectParameterField
    | KnownExtensionField
    | UnknownField

  type PatchInput =
    | PatchAbsent
    | PatchReset
    | PatchValue
    | PatchExplicitNoOp

  type NormalizedPatch =
    | NotNormalized
    | FieldUntouched
    | ResetToDefault
    | SetCanonicalValue
    | NormalizedNoOp

  type Availability =
    | Available
    | Unavailable

  type Ownership =
    | Managed
    | Unmanaged
    | OwnershipNotApplicable

  type TargetLock =
    | Unlocked
    | Locked
    | LockNotApplicable

  type RejectReason =
    | NoRejection
    | UnsupportedCapability
    | MissingSchema
    | SchemaDoesNotCoverField
    | MissingDescriptor
    | DescriptorFamilyMismatch
    | DescriptorDoesNotCoverField
    | ObservationUnavailable
    | SourceBindingStale
    | TargetExistenceMismatch
    | UnmanagedTarget
    | LockedTarget
    | ApprovalDigestMismatch

  type PlanBinding = {
    requestId: int,
    family: ItemFamily,
    crud: CrudKind,
    fieldClass: FieldClass,
    normalizedPatch: NormalizedPatch,
    requestDigest: int,
    planDigest: int,
    canonicalBaseRevision: int,
    sourceId: int,
    sourceRevision: int,
    sourceFingerprint: int,
    targetFingerprint: int,
    surfaceRevision: int,
    capabilityDigest: int,
    schemaDigest: int,
    descriptorDigest: int,
  }

  type ApplicationEvidence = {
    capabilityAccepted: bool,
    schemaAccepted: bool,
    descriptorAccepted: bool,
    observationAccepted: bool,
    sourceBindingFresh: bool,
    existenceAccepted: bool,
    ownershipAccepted: bool,
    lockAccepted: bool,
    approvalExact: bool,
    planBindingFresh: bool,
  }

  type EditSurfaceState = {
    status: EditStatus,
    rejection: RejectReason,

    canonicalHeadRevision: int,
    externalCommitCount: int,
    sourceId: int,
    sourceRevision: int,
    sourceFingerprint: int,
    targetFingerprint: int,
    targetExists: bool,

    surfaceRevision: int,
    capability: Availability,
    capabilityDigest: int,
    capabilityFamily: ItemFamily,
    capabilityCrud: CrudKind,
    capabilitySupportsField: bool,
    schema: Availability,
    schemaDigest: int,
    schemaFamily: ItemFamily,
    schemaCoversField: bool,
    descriptor: Availability,
    descriptorDigest: int,
    descriptorFamily: ItemFamily,
    descriptorCoversField: bool,

    observation: Availability,
    observedCanonicalHeadRevision: int,
    observedSourceId: int,
    observedSourceRevision: int,
    observedSourceFingerprint: int,
    observedTargetFingerprint: int,
    observedTargetExists: bool,
    observedSurfaceRevision: int,
    observationCount: int,

    ownership: Ownership,
    targetLock: TargetLock,

    patchInput: PatchInput,
    suppliedValue: int,
    currentFieldValue: int,
    defaultFieldValue: int,
    normalizedPatch: NormalizedPatch,
    expectedFieldValue: int,

    // These witnesses model a known but untouched field and an opaque field
    // introduced by YMM or another plugin. Neither may be reconstructed away.
    untouchedKnownFieldValue: int,
    unknownFieldDigest: int,

    planClock: int,
    task: PlanBinding,
    approvedBinding: PlanBinding,
    invalidatedBinding: PlanBinding,
    appliedBinding: PlanBinding,
    commitBinding: PlanBinding,
    applicationEvidence: ApplicationEvidence,

    applicationCount: int,
    targetMutationCount: int,
    readBackCount: int,
    readBackFieldValue: int,
    readBackTargetExists: bool,
    touchedFieldReadBackExact: bool,
    targetExistenceReadBackExact: bool,
    canonicalCommitCount: int,
    replayCount: int,
    approvalInvalidationCount: int,
  }

  val missing = -1
  val initialCanonicalRevision = 13
  val initialSourceId = 71
  val initialSourceRevision = 5
  val initialSourceFingerprint = 500
  val initialTargetFingerprint = 700
  val initialSurfaceRevision = 4
  val initialCapabilityDigest = 410
  val initialSchemaDigest = 420
  val initialDescriptorDigest = 430
  val initialFieldValue = 20
  val defaultFieldValue = 0
  val initialUntouchedKnownFieldValue = 31
  val initialUnknownFieldDigest = 991

  pure def emptyBinding: PlanBinding = {
    requestId: missing,
    family: ManagedVoiceItem,
    crud: Update,
    fieldClass: AudioParametersField,
    normalizedPatch: NotNormalized,
    requestDigest: missing,
    planDigest: missing,
    canonicalBaseRevision: missing,
    sourceId: missing,
    sourceRevision: missing,
    sourceFingerprint: missing,
    targetFingerprint: missing,
    surfaceRevision: missing,
    capabilityDigest: missing,
    schemaDigest: missing,
    descriptorDigest: missing,
  }

  pure def emptyApplicationEvidence: ApplicationEvidence = {
    capabilityAccepted: false,
    schemaAccepted: false,
    descriptorAccepted: false,
    observationAccepted: false,
    sourceBindingFresh: false,
    existenceAccepted: false,
    ownershipAccepted: false,
    lockAccepted: false,
    approvalExact: false,
    planBindingFresh: false,
  }

  var state: EditSurfaceState

  pure def fieldIsKnown(fieldClass: FieldClass): bool =
    fieldClass != UnknownField

  pure def normalizedForm(
    input: PatchInput,
    currentValue: int,
    defaultValue: int,
    supplied: int
  ): NormalizedPatch =
    if (input == PatchAbsent)
      FieldUntouched
    else if (input == PatchExplicitNoOp)
      NormalizedNoOp
    else if (input == PatchReset and currentValue == defaultValue)
      NormalizedNoOp
    else if (input == PatchReset)
      ResetToDefault
    else if (supplied == currentValue)
      NormalizedNoOp
    else
      SetCanonicalValue

  pure def expectedValueFor(
    normalized: NormalizedPatch,
    currentValue: int,
    defaultValue: int,
    supplied: int
  ): int =
    if (normalized == ResetToDefault)
      defaultValue
    else if (normalized == SetCanonicalValue)
      supplied
    else
      currentValue

  pure def patchTouchesField(normalized: NormalizedPatch): bool =
    normalized == ResetToDefault or normalized == SetCanonicalValue

  pure def requestMutatesTarget(s: EditSurfaceState): bool =
    any {
      s.task.crud == Create,
      s.task.crud == Delete,
      s.task.crud == Update and patchTouchesField(s.task.normalizedPatch),
    }

  pure def expectedExistenceAfterApply(crud: CrudKind): bool =
    crud != Delete

  pure def ownershipAccepts(s: EditSurfaceState): bool =
    if (s.task.crud == Create)
      s.ownership == OwnershipNotApplicable
    else
      s.ownership == Managed

  pure def lockAccepts(s: EditSurfaceState): bool =
    if (s.task.crud == Create)
      s.targetLock == LockNotApplicable
    else
      s.targetLock == Unlocked

  pure def targetExistenceAccepts(s: EditSurfaceState): bool =
    if (s.task.crud == Create)
      not(s.observedTargetExists)
    else
      s.observedTargetExists

  pure def observationIsFresh(s: EditSurfaceState): bool =
    all {
      s.observation == Available,
      s.observedCanonicalHeadRevision == s.canonicalHeadRevision,
      s.observedSourceId == s.sourceId,
      s.observedSourceRevision == s.sourceRevision,
      s.observedSourceFingerprint == s.sourceFingerprint,
      s.observedTargetFingerprint == s.targetFingerprint,
      s.observedTargetExists == s.targetExists,
      s.observedSurfaceRevision == s.surfaceRevision,
    }

  pure def capabilityAccepts(s: EditSurfaceState): bool =
    all {
      s.capability == Available,
      s.capabilityFamily == s.task.family,
      s.capabilityCrud == s.task.crud,
      s.capabilitySupportsField,
    }

  pure def schemaAccepts(s: EditSurfaceState): bool =
    all {
      s.schema == Available,
      s.schemaFamily == s.task.family,
      s.schemaCoversField,
      fieldIsKnown(s.task.fieldClass),
    }

  pure def descriptorAccepts(s: EditSurfaceState): bool =
    all {
      s.descriptor == Available,
      s.descriptorFamily == s.task.family,
      s.descriptorCoversField,
      fieldIsKnown(s.task.fieldClass),
    }

  pure def admissionAccepts(s: EditSurfaceState): bool =
    all {
      s.normalizedPatch != NotNormalized,
      capabilityAccepts(s),
      schemaAccepts(s),
      descriptorAccepts(s),
      observationIsFresh(s),
      targetExistenceAccepts(s),
      ownershipAccepts(s),
      lockAccepts(s),
    }

  // Stable rejection priority avoids ambiguous behavior when several pieces
  // of evidence are missing at once.
  pure def admissionFailure(s: EditSurfaceState): RejectReason =
    if (s.capability != Available
      or s.capabilityFamily != s.task.family
      or s.capabilityCrud != s.task.crud
      or not(s.capabilitySupportsField))
      UnsupportedCapability
    else if (s.schema != Available)
      MissingSchema
    else if (s.schemaFamily != s.task.family
      or not(s.schemaCoversField)
      or not(fieldIsKnown(s.task.fieldClass)))
      SchemaDoesNotCoverField
    else if (s.descriptor != Available)
      MissingDescriptor
    else if (s.descriptorFamily != s.task.family)
      DescriptorFamilyMismatch
    else if (not(s.descriptorCoversField)
      or not(fieldIsKnown(s.task.fieldClass)))
      DescriptorDoesNotCoverField
    else if (s.observation != Available)
      ObservationUnavailable
    else if (not(observationIsFresh(s)))
      SourceBindingStale
    else if (not(targetExistenceAccepts(s)))
      TargetExistenceMismatch
    else if (not(ownershipAccepts(s)))
      UnmanagedTarget
    else if (not(lockAccepts(s)))
      LockedTarget
    else
      NoRejection

  pure def planBindingIsFresh(s: EditSurfaceState): bool =
    all {
      s.task.canonicalBaseRevision == s.canonicalHeadRevision,
      s.task.sourceId == s.sourceId,
      s.task.sourceRevision == s.sourceRevision,
      s.task.sourceFingerprint == s.sourceFingerprint,
      s.task.targetFingerprint == s.targetFingerprint,
      s.task.surfaceRevision == s.surfaceRevision,
      s.task.capabilityDigest == s.capabilityDigest,
      s.task.schemaDigest == s.schemaDigest,
      s.task.descriptorDigest == s.descriptorDigest,
    }

  pure def configuredRequest(
    s: EditSurfaceState,
    family: ItemFamily,
    crud: CrudKind,
    fieldClass: FieldClass
  ): EditSurfaceState =
    s
      .with("task", s.task
        .with("family", family)
        .with("crud", crud)
        .with("fieldClass", fieldClass)
        .with("requestDigest", s.task.requestDigest + 1)
        .with("normalizedPatch", NotNormalized))
      .with("capabilityFamily", family)
      .with("capabilityCrud", crud)
      .with("schemaFamily", family)
      .with("descriptorFamily", family)
      .with("ownership", if (crud == Create) OwnershipNotApplicable else Managed)
      .with("targetLock", if (crud == Create) LockNotApplicable else Unlocked)
      .with("targetExists", crud != Create)
      .with("normalizedPatch", NotNormalized)

  action init =
    state' = {
      status: Draft,
      rejection: NoRejection,

      canonicalHeadRevision: initialCanonicalRevision,
      externalCommitCount: 0,
      sourceId: initialSourceId,
      sourceRevision: initialSourceRevision,
      sourceFingerprint: initialSourceFingerprint,
      targetFingerprint: initialTargetFingerprint,
      targetExists: true,

      surfaceRevision: initialSurfaceRevision,
      capability: Available,
      capabilityDigest: initialCapabilityDigest,
      capabilityFamily: ManagedVoiceItem,
      capabilityCrud: Update,
      capabilitySupportsField: true,
      schema: Available,
      schemaDigest: initialSchemaDigest,
      schemaFamily: ManagedVoiceItem,
      schemaCoversField: true,
      descriptor: Available,
      descriptorDigest: initialDescriptorDigest,
      descriptorFamily: ManagedVoiceItem,
      descriptorCoversField: true,

      observation: Unavailable,
      observedCanonicalHeadRevision: missing,
      observedSourceId: missing,
      observedSourceRevision: missing,
      observedSourceFingerprint: missing,
      observedTargetFingerprint: missing,
      observedTargetExists: false,
      observedSurfaceRevision: missing,
      observationCount: 0,

      ownership: Managed,
      targetLock: Unlocked,

      patchInput: PatchValue,
      suppliedValue: 25,
      currentFieldValue: initialFieldValue,
      defaultFieldValue: defaultFieldValue,
      normalizedPatch: NotNormalized,
      expectedFieldValue: missing,

      untouchedKnownFieldValue: initialUntouchedKnownFieldValue,
      unknownFieldDigest: initialUnknownFieldDigest,

      planClock: 0,
      task: {
        requestId: 1,
        family: ManagedVoiceItem,
        crud: Update,
        fieldClass: AudioParametersField,
        normalizedPatch: NotNormalized,
        requestDigest: 100,
        planDigest: missing,
        canonicalBaseRevision: missing,
        sourceId: missing,
        sourceRevision: missing,
        sourceFingerprint: missing,
        targetFingerprint: missing,
        surfaceRevision: missing,
        capabilityDigest: missing,
        schemaDigest: missing,
        descriptorDigest: missing,
      },
      approvedBinding: emptyBinding,
      invalidatedBinding: emptyBinding,
      appliedBinding: emptyBinding,
      commitBinding: emptyBinding,
      applicationEvidence: emptyApplicationEvidence,

      applicationCount: 0,
      targetMutationCount: 0,
      readBackCount: 0,
      readBackFieldValue: missing,
      readBackTargetExists: false,
      touchedFieldReadBackExact: false,
      targetExistenceReadBackExact: false,
      canonicalCommitCount: 0,
      replayCount: 0,
      approvalInvalidationCount: 0,
    }

  // Representative family/field selectors make the catalog reachable during
  // randomized exploration without pulling structural protocols into scope.
  action selectManagedAudioUpdate = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(state, ManagedAudioItem, Update, AudioParametersField),
  }

  action selectManagedTextUpdate = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(state, ManagedTextItem, Update, TextStyleField),
  }

  action selectShapeTransformUpdate = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(state, ShapeItem, Update, TransformPositionField),
  }

  action selectTypedEffectParameterUpdate = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(state, EffectItem, Update, TypedEffectParameterField),
  }

  action selectFrameBufferMaskUpdate = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(state, FrameBufferItem, Update, MaskField),
  }

  action selectGenericKnownExtensionUpdate = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(state, GenericYmmItem, Update, KnownExtensionField),
  }

  action selectCreate = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(
      state,
      state.task.family,
      Create,
      state.task.fieldClass
    ),
  }

  action selectDelete = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = configuredRequest(
      state,
      state.task.family,
      Delete,
      state.task.fieldClass
    ),
  }

  action selectAbsentPatch = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state.with("patchInput", PatchAbsent),
  }

  action selectResetPatch = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state.with("patchInput", PatchReset),
  }

  action selectValueEqualToCurrentPatch = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state
      .with("patchInput", PatchValue)
      .with("suppliedValue", state.currentFieldValue),
  }

  action selectExplicitNoOpPatch = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state.with("patchInput", PatchExplicitNoOp),
  }

  // Hostile fixtures choose an unsupported or unsafe request target. These
  // fixture actions do not represent writes to YMM4.
  action chooseUnsupportedCapabilityFixture = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state.with("capability", Unavailable),
  }

  action chooseMissingSchemaFixture = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state.with("schema", Unavailable),
  }

  action chooseMissingDescriptorFixture = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state.with("descriptor", Unavailable),
  }

  action chooseDescriptorMismatchFixture = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state.with("descriptorFamily", ShapeItem),
  }

  action chooseUnknownFieldFixture = all {
    state.status == Draft,
    state.observationCount == 0,
    state' = state
      .with("task", state.task.with("fieldClass", UnknownField))
      .with("schemaCoversField", false)
      .with("descriptorCoversField", false),
  }

  action chooseUnmanagedTargetFixture = all {
    state.status == Draft,
    state.observationCount == 0,
    state.task.crud != Create,
    state' = state.with("ownership", Unmanaged),
  }

  action chooseLockedTargetFixture = all {
    state.status == Draft,
    state.observationCount == 0,
    state.task.crud != Create,
    state' = state.with("targetLock", Locked),
  }

  action normalizePatch = all {
    state.status == Draft,
    state' = state
      .with("status", Normalized)
      .with("normalizedPatch", normalizedForm(
        state.patchInput,
        state.currentFieldValue,
        state.defaultFieldValue,
        state.suppliedValue
      ))
      .with("expectedFieldValue", expectedValueFor(
        normalizedForm(
          state.patchInput,
          state.currentFieldValue,
          state.defaultFieldValue,
          state.suppliedValue
        ),
        state.currentFieldValue,
        state.defaultFieldValue,
        state.suppliedValue
      ))
      .with("task", state.task.with(
        "normalizedPatch",
        normalizedForm(
          state.patchInput,
          state.currentFieldValue,
          state.defaultFieldValue,
          state.suppliedValue
        )
      )),
  }

  action observeReadOnly = all {
    any { state.status == Draft, state.status == Normalized },
    state' = state
      .with("observation", Available)
      .with("observedCanonicalHeadRevision", state.canonicalHeadRevision)
      .with("observedSourceId", state.sourceId)
      .with("observedSourceRevision", state.sourceRevision)
      .with("observedSourceFingerprint", state.sourceFingerprint)
      .with("observedTargetFingerprint", state.targetFingerprint)
      .with("observedTargetExists", state.targetExists)
      .with("observedSurfaceRevision", state.surfaceRevision)
      .with("observationCount", state.observationCount + 1),
  }

  action loseObservation = all {
    state.status == Normalized,
    state.observation == Available,
    state' = state.with("observation", Unavailable),
  }

  action admit = all {
    state.status == Normalized,
    admissionAccepts(state),
    state' = state
      .with("status", Admitted)
      .with("rejection", NoRejection),
  }

  action rejectAdmission = all {
    state.status == Normalized,
    not(admissionAccepts(state)),
    admissionFailure(state) != NoRejection,
    state' = state
      .with("status", Rejected)
      .with("rejection", admissionFailure(state)),
  }

  // One exact plan binds request normalization, source identity/revision/hash,
  // target observation, and all three edit-surface advertisements.
  action stagePreview = all {
    state.status == Admitted,
    admissionAccepts(state),
    state' = state
      .with("status", Previewable)
      .with("planClock", state.planClock + 1)
      .with("task", state.task
        .with("normalizedPatch", state.normalizedPatch)
        .with("planDigest", state.planClock + 1)
        .with("canonicalBaseRevision", state.canonicalHeadRevision)
        .with("sourceId", state.sourceId)
        .with("sourceRevision", state.sourceRevision)
        .with("sourceFingerprint", state.sourceFingerprint)
        .with("targetFingerprint", state.targetFingerprint)
        .with("surfaceRevision", state.surfaceRevision)
        .with("capabilityDigest", state.capabilityDigest)
        .with("schemaDigest", state.schemaDigest)
        .with("descriptorDigest", state.descriptorDigest)),
  }

  action approveExact = all {
    state.status == Previewable,
    planBindingIsFresh(state),
    state' = state
      .with("status", Approved)
      .with("approvedBinding", state.task),
  }

  action rejectMismatchedApprovalDigest = all {
    state.status == Previewable,
    state' = state
      .with("status", Rejected)
      .with("rejection", ApprovalDigestMismatch),
  }

  action revisePatchAfterApproval = all {
    state.status == Approved,
    state' = state
      .with("status", Draft)
      .with("patchInput", PatchReset)
      .with("normalizedPatch", NotNormalized)
      .with("task", state.task
        .with("requestDigest", state.task.requestDigest + 1)
        .with("normalizedPatch", NotNormalized)),
  }

  // A wider field/schema/descriptor surface changes plan semantics. The old
  // approved binding becomes explicit invalidation evidence, and observation
  // must be refreshed before the request can be admitted again.
  action expandSurfaceAfterApproval = all {
    state.status == Approved,
    state' = state
      .with("status", Draft)
      .with("surfaceRevision", state.surfaceRevision + 1)
      .with("capabilityDigest", state.capabilityDigest + 1)
      .with("schemaDigest", state.schemaDigest + 1)
      .with("descriptorDigest", state.descriptorDigest + 1)
      .with("observation", Unavailable)
      .with("normalizedPatch", NotNormalized)
      .with("task", state.task.with("normalizedPatch", NotNormalized))
      .with("invalidatedBinding", state.approvedBinding)
      .with("approvalInvalidationCount", state.approvalInvalidationCount + 1),
  }

  action externalSourceDriftAfterApproval = all {
    state.status == Approved,
    state' = state
      .with("canonicalHeadRevision", state.canonicalHeadRevision + 1)
      .with("externalCommitCount", state.externalCommitCount + 1)
      .with("sourceRevision", state.sourceRevision + 1)
      .with("sourceFingerprint", state.sourceFingerprint + 1)
      .with("targetFingerprint", state.targetFingerprint + 1),
  }

  action markConflict = all {
    state.status == Approved,
    not(planBindingIsFresh(state)),
    state' = state.with("status", Conflicted),
  }

  action applyApproved = all {
    state.status == Approved,
    state.task == state.approvedBinding,
    planBindingIsFresh(state),
    admissionAccepts(state),
    state.applicationCount == 0,
    state' = state
      .with("status", Verifying)
      .with("appliedBinding", state.task)
      .with("applicationEvidence", {
        capabilityAccepted: capabilityAccepts(state),
        schemaAccepted: schemaAccepts(state),
        descriptorAccepted: descriptorAccepts(state),
        observationAccepted: observationIsFresh(state),
        sourceBindingFresh: all {
          state.task.sourceId == state.sourceId,
          state.task.sourceRevision == state.sourceRevision,
          state.task.sourceFingerprint == state.sourceFingerprint,
        },
        existenceAccepted: targetExistenceAccepts(state),
        ownershipAccepted: ownershipAccepts(state),
        lockAccepted: lockAccepts(state),
        approvalExact: state.task == state.approvedBinding,
        planBindingFresh: planBindingIsFresh(state),
      })
      .with("applicationCount", state.applicationCount + 1)
      .with("targetMutationCount",
        state.targetMutationCount + (if (requestMutatesTarget(state)) 1 else 0))
      .with("targetFingerprint",
        state.targetFingerprint + (if (requestMutatesTarget(state)) 1 else 0))
      .with("targetExists", expectedExistenceAfterApply(state.task.crud))
      .with("currentFieldValue",
        if (state.task.crud == Delete)
          state.currentFieldValue
        else
          state.expectedFieldValue),
  }

  action recordExactReadBack = all {
    state.status == Verifying,
    state.readBackCount == 0,
    state' = state
      .with("readBackCount", 1)
      .with("readBackFieldValue", state.expectedFieldValue)
      .with("readBackTargetExists", expectedExistenceAfterApply(state.task.crud))
      .with("touchedFieldReadBackExact", true)
      .with("targetExistenceReadBackExact", true),
  }

  action recordMismatchedTouchedFieldReadBack = all {
    state.status == Verifying,
    state.readBackCount == 0,
    patchTouchesField(state.task.normalizedPatch),
    state' = state
      .with("status", VerificationFailed)
      .with("readBackCount", 1)
      .with("readBackFieldValue", state.expectedFieldValue + 1)
      .with("readBackTargetExists", expectedExistenceAfterApply(state.task.crud))
      .with("touchedFieldReadBackExact", false)
      .with("targetExistenceReadBackExact", true),
  }

  action acceptExactReadBack = all {
    state.status == Verifying,
    state.readBackCount == 1,
    state.touchedFieldReadBackExact,
    state.targetExistenceReadBackExact,
    state.readBackFieldValue == state.expectedFieldValue,
    state.readBackTargetExists == expectedExistenceAfterApply(state.task.crud),
    state' = state.with("status", Verified),
  }

  action commit = all {
    state.status == Verified,
    state.task == state.approvedBinding,
    state.appliedBinding == state.approvedBinding,
    state.touchedFieldReadBackExact,
    state.targetExistenceReadBackExact,
    state.canonicalCommitCount == 0,
    state' = state
      .with("status", Committed)
      .with("canonicalHeadRevision",
        state.canonicalHeadRevision + (if (requestMutatesTarget(state)) 1 else 0))
      .with("canonicalCommitCount",
        state.canonicalCommitCount + (if (requestMutatesTarget(state)) 1 else 0))
      .with("commitBinding", state.task),
  }

  action replayCommitted = all {
    state.status == Committed,
    state.commitBinding == state.task,
    state' = state.with("replayCount", state.replayCount + 1),
  }

  action stutterTerminal = all {
    any {
      state.status == Rejected,
      state.status == VerificationFailed,
      state.status == Conflicted,
    },
    state' = state,
  }

  action step = any {
    selectManagedAudioUpdate,
    selectManagedTextUpdate,
    selectShapeTransformUpdate,
    selectTypedEffectParameterUpdate,
    selectFrameBufferMaskUpdate,
    selectGenericKnownExtensionUpdate,
    selectCreate,
    selectDelete,
    selectAbsentPatch,
    selectResetPatch,
    selectValueEqualToCurrentPatch,
    selectExplicitNoOpPatch,
    chooseUnsupportedCapabilityFixture,
    chooseMissingSchemaFixture,
    chooseMissingDescriptorFixture,
    chooseDescriptorMismatchFixture,
    chooseUnknownFieldFixture,
    chooseUnmanagedTargetFixture,
    chooseLockedTargetFixture,
    normalizePatch,
    observeReadOnly,
    loseObservation,
    admit,
    rejectAdmission,
    stagePreview,
    approveExact,
    rejectMismatchedApprovalDigest,
    revisePatchAfterApproval,
    expandSurfaceAfterApproval,
    externalSourceDriftAfterApproval,
    markConflict,
    applyApproved,
    recordExactReadBack,
    recordMismatchedTouchedFieldReadBack,
    acceptExactReadBack,
    commit,
    replayCommitted,
    stutterTerminal,
  }

  pure def approvalMustRemainExact(status: EditStatus): bool =
    any {
      status == Approved,
      status == Verifying,
      status == Verified,
      status == Committed,
    }

  val normalizedPatchIsCanonical =
    state.status == Draft
      or state.normalizedPatch
        == normalizedForm(
          state.patchInput,
          if (state.applicationCount == 0) state.currentFieldValue else initialFieldValue,
          state.defaultFieldValue,
          state.suppliedValue
        )
      or state.applicationCount == 1

  val approvalIsExact =
    not(approvalMustRemainExact(state.status))
      or state.approvedBinding == state.task

  val rejectedRequestsFailClosed =
    state.status != Rejected
      or all {
        state.applicationCount == 0,
        state.targetMutationCount == 0,
        state.canonicalCommitCount == 0,
        state.appliedBinding == emptyBinding,
      }

  val unsupportedUnobservedUnmanagedOrLockedNeverApplies =
    state.applicationCount == 0
      or all {
        state.applicationEvidence.capabilityAccepted,
        state.applicationEvidence.schemaAccepted,
        state.applicationEvidence.descriptorAccepted,
        state.applicationEvidence.observationAccepted,
        state.applicationEvidence.sourceBindingFresh,
        state.applicationEvidence.existenceAccepted,
        state.applicationEvidence.ownershipAccepted,
        state.applicationEvidence.lockAccepted,
      }

  val everyApplicationHasExactFreshApproval =
    state.applicationCount == 0
      or all {
        state.applicationCount == 1,
        state.applicationEvidence.approvalExact,
        state.applicationEvidence.planBindingFresh,
        state.appliedBinding == state.approvedBinding,
        state.appliedBinding.planDigest >= 0,
      }

  val surfaceExpansionInvalidatesApproval =
    state.approvalInvalidationCount == 0
      or all {
        state.invalidatedBinding != emptyBinding,
        state.invalidatedBinding.surfaceRevision < state.surfaceRevision,
        state.invalidatedBinding.capabilityDigest < state.capabilityDigest,
        state.invalidatedBinding.schemaDigest < state.schemaDigest,
        state.invalidatedBinding.descriptorDigest < state.descriptorDigest,
        state.applicationCount == 0
          or state.appliedBinding != state.invalidatedBinding,
        state.status != Approved
          or state.approvedBinding.surfaceRevision == state.surfaceRevision,
      }

  // Only the selected field may change during Update. The opaque digest is a
  // preservation witness for every field this schema version does not know.
  val partialUpdatePreservesUntouchedAndUnknownFields = all {
    state.untouchedKnownFieldValue == initialUntouchedKnownFieldValue,
    state.unknownFieldDigest == initialUnknownFieldDigest,
  }

  val touchedFieldsRequireExactReadBack =
    state.status != Committed
      or all {
        state.readBackCount == 1,
        state.touchedFieldReadBackExact,
        state.targetExistenceReadBackExact,
        state.readBackFieldValue == state.expectedFieldValue,
        state.readBackTargetExists
          == expectedExistenceAfterApply(state.task.crud),
        state.appliedBinding == state.commitBinding,
      }

  val mismatchedReadBackNeverCommits =
    state.status != VerificationFailed
      or all {
        state.canonicalCommitCount == 0,
        state.commitBinding == emptyBinding,
      }

  val readOnlyObservationDoesNotMutate = all {
    state.targetMutationCount
      == (if (state.applicationCount == 1
        and requestMutatesTarget(state)) 1 else 0),
    state.canonicalHeadRevision
      == initialCanonicalRevision
        + state.externalCommitCount
        + state.canonicalCommitCount,
    state.observationCount >= 0,
  }

  val exactReplayIsAtMostOnce = all {
    state.applicationCount <= 1,
    state.targetMutationCount <= 1,
    state.canonicalCommitCount <= 1,
    state.replayCount == 0
      or all {
        state.status == Committed,
        state.commitBinding == state.task,
      },
  }

  val editSurfaceSafety = and {
    normalizedPatchIsCanonical,
    approvalIsExact,
    rejectedRequestsFailClosed,
    unsupportedUnobservedUnmanagedOrLockedNeverApplies,
    everyApplicationHasExactFreshApproval,
    surfaceExpansionInvalidatesApproval,
    partialUpdatePreservesUntouchedAndUnknownFields,
    touchedFieldsRequireExactReadBack,
    mismatchedReadBackNeverCommits,
    readOnlyObservationDoesNotMutate,
    exactReplayIsAtMostOnce,
  }

  run valuePatchHappyPathTest = init
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordExactReadBack)
    .then(acceptExactReadBack)
    .then(commit)
    .then(replayCommitted)
    .expect(all {
      editSurfaceSafety,
      state.status == Committed,
      state.normalizedPatch == SetCanonicalValue,
      state.currentFieldValue == 25,
      state.targetMutationCount == 1,
      state.canonicalCommitCount == 1,
      state.canonicalHeadRevision == initialCanonicalRevision + 1,
      state.replayCount == 1,
    })

  run absentPatchIsUntouchedNoOpTest = init
    .then(selectAbsentPatch)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordExactReadBack)
    .then(acceptExactReadBack)
    .then(commit)
    .expect(all {
      editSurfaceSafety,
      state.status == Committed,
      state.normalizedPatch == FieldUntouched,
      state.currentFieldValue == initialFieldValue,
      state.targetMutationCount == 0,
      state.canonicalCommitCount == 0,
      state.canonicalHeadRevision == initialCanonicalRevision,
    })

  run resetPatchUsesDescriptorDefaultTest = init
    .then(selectResetPatch)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordExactReadBack)
    .then(acceptExactReadBack)
    .then(commit)
    .expect(all {
      editSurfaceSafety,
      state.status == Committed,
      state.normalizedPatch == ResetToDefault,
      state.currentFieldValue == defaultFieldValue,
      state.targetMutationCount == 1,
      state.canonicalCommitCount == 1,
    })

  run equalValueNormalizesToNoOpTest = init
    .then(selectValueEqualToCurrentPatch)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordExactReadBack)
    .then(acceptExactReadBack)
    .then(commit)
    .expect(all {
      editSurfaceSafety,
      state.status == Committed,
      state.normalizedPatch == NormalizedNoOp,
      state.targetMutationCount == 0,
      state.canonicalCommitCount == 0,
    })

  run explicitNoOpRemainsNoOpTest = init
    .then(selectExplicitNoOpPatch)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordExactReadBack)
    .then(acceptExactReadBack)
    .then(commit)
    .expect(all {
      editSurfaceSafety,
      state.status == Committed,
      state.patchInput == PatchExplicitNoOp,
      state.normalizedPatch == NormalizedNoOp,
      state.targetMutationCount == 0,
      state.canonicalCommitCount == 0,
    })

  run unsupportedCapabilityFailsClosedTest = init
    .then(chooseUnsupportedCapabilityFixture)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(rejectAdmission)
    .expect(all {
      editSurfaceSafety,
      state.status == Rejected,
      state.rejection == UnsupportedCapability,
      state.applicationCount == 0,
    })

  run missingSchemaFailsClosedTest = init
    .then(chooseMissingSchemaFixture)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(rejectAdmission)
    .expect(all {
      editSurfaceSafety,
      state.status == Rejected,
      state.rejection == MissingSchema,
      state.applicationCount == 0,
    })

  run missingDescriptorFailsClosedTest = init
    .then(chooseMissingDescriptorFixture)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(rejectAdmission)
    .expect(all {
      editSurfaceSafety,
      state.status == Rejected,
      state.rejection == MissingDescriptor,
      state.applicationCount == 0,
    })

  run unknownFieldIsPreservedButNotWritableTest = init
    .then(chooseUnknownFieldFixture)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(rejectAdmission)
    .expect(all {
      editSurfaceSafety,
      state.status == Rejected,
      state.rejection == SchemaDoesNotCoverField,
      state.unknownFieldDigest == initialUnknownFieldDigest,
      state.applicationCount == 0,
    })

  run unobservedTargetFailsClosedTest = init
    .then(normalizePatch)
    .then(rejectAdmission)
    .expect(all {
      editSurfaceSafety,
      state.status == Rejected,
      state.rejection == ObservationUnavailable,
      state.applicationCount == 0,
    })

  run unmanagedTargetFailsClosedTest = init
    .then(chooseUnmanagedTargetFixture)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(rejectAdmission)
    .expect(all {
      editSurfaceSafety,
      state.status == Rejected,
      state.rejection == UnmanagedTarget,
      state.applicationCount == 0,
    })

  run lockedTargetFailsClosedTest = init
    .then(chooseLockedTargetFixture)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(rejectAdmission)
    .expect(all {
      editSurfaceSafety,
      state.status == Rejected,
      state.rejection == LockedTarget,
      state.applicationCount == 0,
    })

  run mismatchedReadBackCannotCommitTest = init
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordMismatchedTouchedFieldReadBack)
    .expect(all {
      editSurfaceSafety,
      state.status == VerificationFailed,
      not(state.touchedFieldReadBackExact),
      state.canonicalCommitCount == 0,
      state.commitBinding == emptyBinding,
    })

  run sourceDriftConflictsBeforeApplyTest = init
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(externalSourceDriftAfterApproval)
    .then(markConflict)
    .expect(all {
      editSurfaceSafety,
      state.status == Conflicted,
      state.externalCommitCount == 1,
      state.applicationCount == 0,
      state.canonicalCommitCount == 0,
    })

  run surfaceExpansionInvalidatesApprovalTest = init
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(expandSurfaceAfterApproval)
    .expect(all {
      editSurfaceSafety,
      state.status == Draft,
      state.observation == Unavailable,
      state.invalidatedBinding == state.approvedBinding,
      state.invalidatedBinding.surfaceRevision < state.surfaceRevision,
      state.applicationCount == 0,
    })

  run expandedSurfaceNeedsFreshApprovalTest = init
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(expandSurfaceAfterApproval)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordExactReadBack)
    .then(acceptExactReadBack)
    .then(commit)
    .expect(all {
      editSurfaceSafety,
      state.status == Committed,
      state.appliedBinding != state.invalidatedBinding,
      state.appliedBinding.surfaceRevision == state.surfaceRevision,
      state.canonicalCommitCount == 1,
    })

  run readOnlyObservationDoesNotMutateTest = init
    .then(observeReadOnly)
    .then(observeReadOnly)
    .expect(all {
      editSurfaceSafety,
      state.status == Draft,
      state.observationCount == 2,
      state.sourceFingerprint == initialSourceFingerprint,
      state.targetFingerprint == initialTargetFingerprint,
      state.targetMutationCount == 0,
      state.canonicalHeadRevision == initialCanonicalRevision,
    })

  run createUsesExplicitAbsenceAdmissionTest = init
    .then(selectCreate)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .expect(all {
      editSurfaceSafety,
      state.status == Approved,
      state.task.crud == Create,
      not(state.observedTargetExists),
      state.ownership == OwnershipNotApplicable,
      state.targetLock == LockNotApplicable,
    })

  run deleteUsesManagedExistingTargetAdmissionTest = init
    .then(selectDelete)
    .then(normalizePatch)
    .then(observeReadOnly)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .expect(all {
      editSurfaceSafety,
      state.status == Approved,
      state.task.crud == Delete,
      state.observedTargetExists,
      state.ownership == Managed,
      state.targetLock == Unlocked,
    })
}
