// YMM4 composition-graph edit semantics.
//
// This finite model describes the domain contract for item/entity edits that
// are not yet exposed uniformly by TakeGraph. It deliberately does not model
// approval, WAL, rollback, or recovery; an admitted semantic edit is intended
// to be carried by the generic change-set/apply protocols.
//
// The fixed graph is the smallest representative shape that exercises stable
// entity identity, hierarchy and mask references, an ordered effect chain, a
// framebuffer/scene/transition chain, and reference-safe deletion. The focus
// slot can contain every known YMM4 item family. Unknown item kinds fail closed.
//
// A green Quint run proves only these abstract transitions and invariants. It
// does not prove YMM4 private APIs, serialization, UI gestures, or OS behavior.
module ymm4CompositionGraphProtocol {
  type ItemKind =
    | AudioItem
    | VoiceItem
    | AssetItem
    | TextItem
    | ShapeItem
    | EffectItem
    | FrameBufferItem
    | GroupItem
    | SceneItem
    | TransitionItem
    | UnknownItem

  type Interpolation =
    | Hold
    | Linear
    | EaseIn
    | EaseOut

  type EditStatus =
    | Idle
    | Staged
    | Applied
    | NormalizedNoOp
    | Rejected

  type EditIntent =
    | NoEdit
    | CreateEntity
    | UpdateGeometry
    | UpdateKeyframes
    | UpdatePaintOrder
    | AttachFocusReferences
    | DetachFocusReferences
    | AttachRootParent
    | AppendFirstEffect
    | AppendSecondEffect
    | ReorderEffects
    | DetachEffects
    | BindFrameBuffer
    | DetachFrameBuffer
    | BindScene
    | BindTransition
    | DeleteEntity

  type EditBinding = {
    requestDigest: int,
    sourceGraphId: int,
    targetGraphId: int,
    baseRevision: int,
    targetFingerprint: int,
    descriptorDigest: int,
    targetEntityId: int,
    targetKind: ItemKind,
  }

  type CompositionGraphState = {
    status: EditStatus,
    intent: EditIntent,

    sourceGraphId: int,
    targetGraphId: int,
    targetFingerprint: int,
    descriptorDigest: int,
    revision: int,
    semanticMutationCount: int,
    rejectionCount: int,
    requestCounter: int,
    outcomeBaseRevision: int,

    binding: EditBinding,
    lastAppliedBinding: EditBinding,
    stagedTargetId: int,
    stagedKind: ItemKind,

    candidateStartFrame: int,
    candidateLayer: int,
    candidateDuration: int,
    candidateKeyframeA: int,
    candidateKeyframeB: int,
    candidateInterpolationA: Interpolation,
    candidateInterpolationB: Interpolation,
    candidateFocusPaintSlot: int,

    // The focus slot is a finite representative for generic CRUD. Identity is
    // never reassigned by update; deletion tombstones it and creation allocates
    // strictly above the high-water mark.
    focusExists: bool,
    focusId: int,
    focusBirthId: int,
    lastDeletedFocusId: int,
    lastAllocatedId: int,
    focusKind: ItemKind,
    focusStartFrame: int,
    focusLayer: int,
    focusDuration: int,
    focusParentId: int,
    focusGroupId: int,
    focusMaskId: int,

    // Paint order is a separate sibling-list axis. It is intentionally not
    // derived from timeline layer and is unrelated to effect-chain order.
    paintSiblingExists: bool,
    paintSiblingId: int,
    paintSlotOneId: int,
    paintSlotTwoId: int,

    keyframeCount: int,
    keyframeOneFrame: int,
    keyframeTwoFrame: int,
    keyframeOneInterpolation: Interpolation,
    keyframeTwoInterpolation: Interpolation,

    rootGroupExists: bool,
    rootGroupParentId: int,
    maskShapeExists: bool,

    firstEffectExists: bool,
    secondEffectExists: bool,
    effectSlotOneId: int,
    effectSlotTwoId: int,
    firstEffectOwnerId: int,
    secondEffectOwnerId: int,

    frameBufferExists: bool,
    frameBufferSourceId: int,
    sceneAExists: bool,
    sceneBExists: bool,
    sceneAFrameBufferId: int,
    sceneBFrameBufferId: int,
    transitionExists: bool,
    transitionSourceSceneId: int,
    transitionTargetSceneId: int,
  }

  val missing = -1
  val initialRevision = 7
  val initialFingerprint = 700

  val rootGroupId = 10
  val maskShapeId = 11
  val initialFocusId = 20
  val stablePaintSiblingId = 21
  val firstEffectId = 30
  val secondEffectId = 31
  val frameBufferId = 40
  val sceneAId = 50
  val sceneBId = 51
  val transitionId = 60

  val emptyBinding = {
    requestDigest: missing,
    sourceGraphId: missing,
    targetGraphId: missing,
    baseRevision: missing,
    targetFingerprint: missing,
    descriptorDigest: missing,
    targetEntityId: missing,
    targetKind: UnknownItem,
  }

  var state: CompositionGraphState

  pure def isKnownKind(kind: ItemKind): bool =
    kind != UnknownItem

  pure def isVisualKind(kind: ItemKind): bool =
    any {
      kind == AssetItem,
      kind == TextItem,
      kind == ShapeItem,
      kind == EffectItem,
      kind == FrameBufferItem,
      kind == GroupItem,
      kind == SceneItem,
      kind == TransitionItem,
    }

  pure def captureBinding(
    s: CompositionGraphState,
    targetEntityId: int,
    targetKind: ItemKind
  ): EditBinding = {
    requestDigest: s.requestCounter + 1,
    sourceGraphId: s.sourceGraphId,
    targetGraphId: s.targetGraphId,
    baseRevision: s.revision,
    targetFingerprint: s.targetFingerprint,
    descriptorDigest: s.descriptorDigest,
    targetEntityId: targetEntityId,
    targetKind: targetKind,
  }

  pure def stageFor(
    s: CompositionGraphState,
    intent: EditIntent,
    targetEntityId: int,
    targetKind: ItemKind
  ): CompositionGraphState =
    s
      .with("status", Staged)
      .with("intent", intent)
      .with("requestCounter", s.requestCounter + 1)
      .with("binding", captureBinding(s, targetEntityId, targetKind))
      .with("stagedTargetId", targetEntityId)
      .with("stagedKind", targetKind)
      .with("outcomeBaseRevision", s.revision)

  pure def markApplied(s: CompositionGraphState): CompositionGraphState =
    s
      .with("status", Applied)
      .with("revision", s.revision + 1)
      .with("targetFingerprint", s.targetFingerprint + 1)
      .with("semanticMutationCount", s.semanticMutationCount + 1)
      .with("lastAppliedBinding", s.binding)
      .with("outcomeBaseRevision", s.binding.baseRevision)

  pure def markNoOp(s: CompositionGraphState): CompositionGraphState =
    s
      .with("status", NormalizedNoOp)
      .with("outcomeBaseRevision", s.binding.baseRevision)

  pure def markRejected(s: CompositionGraphState): CompositionGraphState =
    s
      .with("status", Rejected)
      .with("rejectionCount", s.rejectionCount + 1)
      .with("outcomeBaseRevision", s.revision)

  pure def bindingMatchesCurrentGraph(s: CompositionGraphState): bool =
    all {
      s.status == Staged,
      s.binding.requestDigest == s.requestCounter,
      s.binding.sourceGraphId == s.sourceGraphId,
      s.binding.targetGraphId == s.targetGraphId,
      s.binding.baseRevision == s.revision,
      s.binding.targetFingerprint == s.targetFingerprint,
      s.binding.descriptorDigest == s.descriptorDigest,
      s.binding.targetEntityId == s.stagedTargetId,
      s.binding.targetKind == s.stagedKind,
      isKnownKind(s.binding.targetKind),
    }

  pure def existingTargetBindingIsExact(s: CompositionGraphState): bool =
    all {
      bindingMatchesCurrentGraph(s),
      any {
        all {
          s.focusExists,
          s.stagedTargetId == s.focusId,
          s.stagedKind == s.focusKind,
        },
        all {
          s.rootGroupExists,
          s.stagedTargetId == rootGroupId,
          s.stagedKind == GroupItem,
        },
        all {
          s.frameBufferExists,
          s.stagedTargetId == frameBufferId,
          s.stagedKind == FrameBufferItem,
        },
        all {
          s.sceneAExists,
          s.stagedTargetId == sceneAId,
          s.stagedKind == SceneItem,
        },
        all {
          s.transitionExists,
          s.stagedTargetId == transitionId,
          s.stagedKind == TransitionItem,
        },
      },
    }

  pure def candidateGeometryIsLegal(s: CompositionGraphState): bool =
    all {
      s.candidateStartFrame >= 0,
      s.candidateLayer >= 0,
      s.candidateDuration > 0,
      s.keyframeCount == 0
        or s.keyframeOneFrame < s.candidateDuration,
      s.keyframeCount < 2
        or s.keyframeTwoFrame < s.candidateDuration,
    }

  pure def candidateGeometryEqualsStored(s: CompositionGraphState): bool =
    all {
      s.candidateStartFrame == s.focusStartFrame,
      s.candidateLayer == s.focusLayer,
      s.candidateDuration == s.focusDuration,
    }

  pure def keyframeCandidateNormalizesToStored(s: CompositionGraphState): bool =
    all {
      s.keyframeCount == 2,
      s.candidateKeyframeA == s.keyframeTwoFrame,
      s.candidateKeyframeB == s.keyframeOneFrame,
      s.candidateInterpolationA == s.keyframeTwoInterpolation,
      s.candidateInterpolationB == s.keyframeOneInterpolation,
    }

  pure def focusHasIncomingReferences(s: CompositionGraphState): bool =
    all {
      s.focusExists,
      any {
        s.rootGroupParentId == s.focusId,
        s.firstEffectOwnerId == s.focusId,
        s.secondEffectOwnerId == s.focusId,
        s.frameBufferSourceId == s.focusId,
      },
    }

  pure def focusHasOutgoingReferences(s: CompositionGraphState): bool =
    any {
      s.focusParentId != missing,
      s.focusGroupId != missing,
      s.focusMaskId != missing,
    }

  // The policy is deliberately strict: deletion never hides an implicit
  // cascade. Every incoming and outgoing reference must be detached by an
  // explicit, separately previewable edit before the entity may be removed.
  pure def focusIsReferenceSafeToDelete(s: CompositionGraphState): bool =
    all {
      s.focusExists,
      not(focusHasIncomingReferences(s)),
      not(focusHasOutgoingReferences(s)),
    }

  action init =
    state' = {
      status: Idle,
      intent: NoEdit,

      sourceGraphId: 100,
      targetGraphId: 200,
      targetFingerprint: initialFingerprint,
      descriptorDigest: 900,
      revision: initialRevision,
      semanticMutationCount: 0,
      rejectionCount: 0,
      requestCounter: 0,
      outcomeBaseRevision: initialRevision,

      binding: emptyBinding,
      lastAppliedBinding: emptyBinding,
      stagedTargetId: missing,
      stagedKind: UnknownItem,

      candidateStartFrame: 0,
      candidateLayer: 0,
      candidateDuration: 1,
      candidateKeyframeA: missing,
      candidateKeyframeB: missing,
      candidateInterpolationA: Hold,
      candidateInterpolationB: Hold,
      candidateFocusPaintSlot: 1,

      focusExists: true,
      focusId: initialFocusId,
      focusBirthId: initialFocusId,
      lastDeletedFocusId: missing,
      lastAllocatedId: transitionId,
      focusKind: TextItem,
      focusStartFrame: 0,
      focusLayer: 1,
      focusDuration: 10,
      focusParentId: missing,
      focusGroupId: missing,
      focusMaskId: missing,

      paintSiblingExists: true,
      paintSiblingId: stablePaintSiblingId,
      paintSlotOneId: initialFocusId,
      paintSlotTwoId: stablePaintSiblingId,

      keyframeCount: 0,
      keyframeOneFrame: missing,
      keyframeTwoFrame: missing,
      keyframeOneInterpolation: Hold,
      keyframeTwoInterpolation: Hold,

      rootGroupExists: true,
      rootGroupParentId: missing,
      maskShapeExists: true,

      firstEffectExists: true,
      secondEffectExists: true,
      effectSlotOneId: missing,
      effectSlotTwoId: missing,
      firstEffectOwnerId: missing,
      secondEffectOwnerId: missing,

      frameBufferExists: true,
      frameBufferSourceId: missing,
      sceneAExists: true,
      sceneBExists: true,
      sceneAFrameBufferId: missing,
      sceneBFrameBufferId: missing,
      transitionExists: true,
      transitionSourceSceneId: missing,
      transitionTargetSceneId: missing,
    }

  action beginNextEdit = all {
    any {
      state.status == Applied,
      state.status == NormalizedNoOp,
      state.status == Rejected,
    },
    state' = state
      .with("status", Idle)
      .with("intent", NoEdit)
      .with("binding", emptyBinding)
      .with("stagedTargetId", missing)
      .with("stagedKind", UnknownItem),
  }

  pure def stagedKnownCreate(
    s: CompositionGraphState,
    kind: ItemKind
  ): CompositionGraphState =
    stageFor(s, CreateEntity, s.lastAllocatedId + 1, kind)
      .with("candidateStartFrame", 0)
      .with("candidateLayer", 0)
      .with("candidateDuration", 10)

  action stageCreateAudio = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, AudioItem),
  }

  action stageCreateVoice = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, VoiceItem),
  }

  action stageCreateAsset = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, AssetItem),
  }

  action stageCreateText = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, TextItem),
  }

  action stageCreateShape = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, ShapeItem),
  }

  action stageCreateEffect = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, EffectItem),
  }

  action stageCreateFrameBuffer = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, FrameBufferItem),
  }

  action stageCreateGroup = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, GroupItem),
  }

  action stageCreateScene = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, SceneItem),
  }

  action stageCreateTransition = all {
    state.status == Idle,
    not(state.focusExists),
    state' = stagedKnownCreate(state, TransitionItem),
  }

  // Unknown descriptors never reach Staged. No target identity is allocated.
  action rejectUnknownCreate = all {
    state.status == Idle,
    not(state.focusExists),
    state' = markRejected(state)
      .with("intent", CreateEntity)
      .with("stagedTargetId", state.lastAllocatedId + 1)
      .with("stagedKind", UnknownItem),
  }

  action rejectCreateOverExisting = all {
    state.status == Idle,
    state.focusExists,
    state' = markRejected(state)
      .with("intent", CreateEntity)
      .with("stagedTargetId", state.focusId)
      .with("stagedKind", state.focusKind),
  }

  action applyCreate = all {
    state.intent == CreateEntity,
    bindingMatchesCurrentGraph(state),
    not(state.focusExists),
    state.stagedTargetId == state.lastAllocatedId + 1,
    isKnownKind(state.stagedKind),
    state.candidateStartFrame >= 0,
    state.candidateLayer >= 0,
    state.candidateDuration > 0,
    state' = markApplied(state)
      .with("focusExists", true)
      .with("focusId", state.stagedTargetId)
      .with("focusBirthId", state.stagedTargetId)
      .with("lastAllocatedId", state.stagedTargetId)
      .with("focusKind", state.stagedKind)
      .with("focusStartFrame", state.candidateStartFrame)
      .with("focusLayer", state.candidateLayer)
      .with("focusDuration", state.candidateDuration)
      .with("focusParentId", missing)
      .with("focusGroupId", missing)
      .with("focusMaskId", missing)
      .with("paintSlotOneId", state.paintSiblingId)
      .with("paintSlotTwoId", state.stagedTargetId)
      .with("keyframeCount", 0)
      .with("keyframeOneFrame", missing)
      .with("keyframeTwoFrame", missing),
  }

  action stageGeometryUpdate = all {
    state.status == Idle,
    state.focusExists,
    state' = stageFor(state, UpdateGeometry, state.focusId, state.focusKind)
      .with("candidateStartFrame", state.focusStartFrame + 2)
      .with("candidateLayer", state.focusLayer + 1)
      .with("candidateDuration", state.focusDuration + 2),
  }

  action stageGeometryNoOp = all {
    state.status == Idle,
    state.focusExists,
    state' = stageFor(state, UpdateGeometry, state.focusId, state.focusKind)
      .with("candidateStartFrame", state.focusStartFrame)
      .with("candidateLayer", state.focusLayer)
      .with("candidateDuration", state.focusDuration),
  }

  action stageInvalidGeometry = all {
    state.status == Idle,
    state.focusExists,
    state' = stageFor(state, UpdateGeometry, state.focusId, state.focusKind)
      .with("candidateStartFrame", state.focusStartFrame)
      .with("candidateLayer", state.focusLayer)
      .with("candidateDuration", 0),
  }

  action applyGeometryUpdate = all {
    state.intent == UpdateGeometry,
    existingTargetBindingIsExact(state),
    candidateGeometryIsLegal(state),
    not(candidateGeometryEqualsStored(state)),
    state' = markApplied(state)
      .with("focusStartFrame", state.candidateStartFrame)
      .with("focusLayer", state.candidateLayer)
      .with("focusDuration", state.candidateDuration),
  }

  action finishGeometryNoOp = all {
    state.intent == UpdateGeometry,
    existingTargetBindingIsExact(state),
    candidateGeometryIsLegal(state),
    candidateGeometryEqualsStored(state),
    state' = markNoOp(state),
  }

  action rejectInvalidGeometry = all {
    state.intent == UpdateGeometry,
    existingTargetBindingIsExact(state),
    not(candidateGeometryIsLegal(state)),
    state' = markRejected(state),
  }

  // The staged order is intentionally late-then-early. Application stores a
  // unique canonical frame order and carries interpolation with its keyframe.
  action stageReverseKeyframes = all {
    state.status == Idle,
    state.focusExists,
    state.focusDuration > 6,
    state' = stageFor(state, UpdateKeyframes, state.focusId, state.focusKind)
      .with("candidateKeyframeA", 6)
      .with("candidateKeyframeB", 2)
      .with("candidateInterpolationA", Linear)
      .with("candidateInterpolationB", EaseIn),
  }

  action applyCanonicalKeyframes = all {
    state.intent == UpdateKeyframes,
    existingTargetBindingIsExact(state),
    state.candidateKeyframeA == 6,
    state.candidateKeyframeB == 2,
    state.focusDuration > 6,
    not(keyframeCandidateNormalizesToStored(state)),
    state' = markApplied(state)
      .with("keyframeCount", 2)
      .with("keyframeOneFrame", 2)
      .with("keyframeTwoFrame", 6)
      .with("keyframeOneInterpolation", state.candidateInterpolationB)
      .with("keyframeTwoInterpolation", state.candidateInterpolationA),
  }

  action finishCanonicalKeyframesNoOp = all {
    state.intent == UpdateKeyframes,
    existingTargetBindingIsExact(state),
    keyframeCandidateNormalizesToStored(state),
    state' = markNoOp(state),
  }

  // Slot two is the visual front in this finite sibling list. Moving an item
  // changes only paint order; timeline layer remains an independent field.
  action stageMoveFocusToFront = all {
    state.status == Idle,
    state.focusExists,
    state.paintSiblingExists,
    state' = stageFor(
      state,
      UpdatePaintOrder,
      state.focusId,
      state.focusKind
    ).with("candidateFocusPaintSlot", 2),
  }

  action stageMoveFocusToBack = all {
    state.status == Idle,
    state.focusExists,
    state.paintSiblingExists,
    state' = stageFor(
      state,
      UpdatePaintOrder,
      state.focusId,
      state.focusKind
    ).with("candidateFocusPaintSlot", 1),
  }

  action applyPaintOrderMove = all {
    state.intent == UpdatePaintOrder,
    existingTargetBindingIsExact(state),
    state.paintSiblingExists,
    any {
      all {
        state.candidateFocusPaintSlot == 1,
        state.paintSlotOneId != state.focusId,
      },
      all {
        state.candidateFocusPaintSlot == 2,
        state.paintSlotTwoId != state.focusId,
      },
    },
    state' = markApplied(state)
      .with(
        "paintSlotOneId",
        if (state.candidateFocusPaintSlot == 1)
          state.focusId
        else
          state.paintSiblingId
      )
      .with(
        "paintSlotTwoId",
        if (state.candidateFocusPaintSlot == 1)
          state.paintSiblingId
        else
          state.focusId
      ),
  }

  action finishPaintOrderNoOp = all {
    state.intent == UpdatePaintOrder,
    existingTargetBindingIsExact(state),
    any {
      all {
        state.candidateFocusPaintSlot == 1,
        state.paintSlotOneId == state.focusId,
      },
      all {
        state.candidateFocusPaintSlot == 2,
        state.paintSlotTwoId == state.focusId,
      },
    },
    state' = markNoOp(state),
  }

  action stageAttachFocusReferences = all {
    state.status == Idle,
    state.focusExists,
    isVisualKind(state.focusKind),
    state' = stageFor(
      state,
      AttachFocusReferences,
      state.focusId,
      state.focusKind
    ),
  }

  action applyAttachFocusReferences = all {
    state.intent == AttachFocusReferences,
    existingTargetBindingIsExact(state),
    state.rootGroupExists,
    state.maskShapeExists,
    isVisualKind(state.focusKind),
    state.rootGroupParentId != state.focusId,
    state' = markApplied(state)
      .with("focusParentId", rootGroupId)
      .with("focusGroupId", rootGroupId)
      .with("focusMaskId", maskShapeId),
  }

  action stageDetachFocusReferences = all {
    state.status == Idle,
    state.focusExists,
    focusHasOutgoingReferences(state),
    state' = stageFor(
      state,
      DetachFocusReferences,
      state.focusId,
      state.focusKind
    ),
  }

  action applyDetachFocusReferences = all {
    state.intent == DetachFocusReferences,
    existingTargetBindingIsExact(state),
    state' = markApplied(state)
      .with("focusParentId", missing)
      .with("focusGroupId", missing)
      .with("focusMaskId", missing),
  }

  action stageAttachRootParent = all {
    state.status == Idle,
    state.focusExists,
    state.focusKind == GroupItem,
    state.rootGroupExists,
    state' = stageFor(state, AttachRootParent, rootGroupId, GroupItem),
  }

  action applyAttachRootParent = all {
    state.intent == AttachRootParent,
    existingTargetBindingIsExact(state),
    state.focusKind == GroupItem,
    state.focusParentId != rootGroupId,
    state.focusGroupId != rootGroupId,
    state.rootGroupParentId == missing,
    state' = markApplied(state)
      .with("rootGroupParentId", state.focusId),
  }

  action rejectParentCycle = all {
    state.intent == AttachRootParent,
    existingTargetBindingIsExact(state),
    state.focusKind == GroupItem,
    any {
      state.focusParentId == rootGroupId,
      state.focusGroupId == rootGroupId,
    },
    state' = markRejected(state),
  }

  action stageAppendFirstEffect = all {
    state.status == Idle,
    state.focusExists,
    state.effectSlotOneId == missing,
    state' = stageFor(
      state,
      AppendFirstEffect,
      state.focusId,
      state.focusKind
    ),
  }

  action applyAppendFirstEffect = all {
    state.intent == AppendFirstEffect,
    existingTargetBindingIsExact(state),
    state.firstEffectExists,
    state.effectSlotOneId == missing,
    state.effectSlotTwoId == missing,
    state' = markApplied(state)
      .with("effectSlotOneId", firstEffectId)
      .with("firstEffectOwnerId", state.focusId),
  }

  action stageAppendSecondEffect = all {
    state.status == Idle,
    state.focusExists,
    state.effectSlotOneId != missing,
    state.effectSlotTwoId == missing,
    state' = stageFor(
      state,
      AppendSecondEffect,
      state.focusId,
      state.focusKind
    ),
  }

  action applyAppendSecondEffect = all {
    state.intent == AppendSecondEffect,
    existingTargetBindingIsExact(state),
    state.secondEffectExists,
    state.effectSlotOneId == firstEffectId,
    state.effectSlotTwoId == missing,
    state' = markApplied(state)
      .with("effectSlotTwoId", secondEffectId)
      .with("secondEffectOwnerId", state.focusId),
  }

  action stageReorderEffects = all {
    state.status == Idle,
    state.focusExists,
    state.effectSlotOneId != missing,
    state.effectSlotTwoId != missing,
    state' = stageFor(
      state,
      ReorderEffects,
      state.focusId,
      state.focusKind
    ),
  }

  action applyReorderEffects = all {
    state.intent == ReorderEffects,
    existingTargetBindingIsExact(state),
    state.effectSlotOneId == firstEffectId,
    state.effectSlotTwoId == secondEffectId,
    state' = markApplied(state)
      .with("effectSlotOneId", secondEffectId)
      .with("effectSlotTwoId", firstEffectId),
  }

  action stageDetachEffects = all {
    state.status == Idle,
    state.focusExists,
    state.effectSlotOneId != missing,
    state' = stageFor(state, DetachEffects, state.focusId, state.focusKind),
  }

  action applyDetachEffects = all {
    state.intent == DetachEffects,
    existingTargetBindingIsExact(state),
    state' = markApplied(state)
      .with("effectSlotOneId", missing)
      .with("effectSlotTwoId", missing)
      .with("firstEffectOwnerId", missing)
      .with("secondEffectOwnerId", missing),
  }

  action stageBindFrameBuffer = all {
    state.status == Idle,
    state.focusExists,
    isVisualKind(state.focusKind),
    state.frameBufferSourceId == missing,
    state' = stageFor(
      state,
      BindFrameBuffer,
      frameBufferId,
      FrameBufferItem
    ),
  }

  action applyBindFrameBuffer = all {
    state.intent == BindFrameBuffer,
    existingTargetBindingIsExact(state),
    state.frameBufferExists,
    state.focusExists,
    isVisualKind(state.focusKind),
    state.frameBufferSourceId == missing,
    state' = markApplied(state)
      .with("frameBufferSourceId", state.focusId),
  }

  action stageDetachFrameBuffer = all {
    state.status == Idle,
    state.frameBufferExists,
    state.frameBufferSourceId != missing,
    state' = stageFor(
      state,
      DetachFrameBuffer,
      frameBufferId,
      FrameBufferItem
    ),
  }

  action applyDetachFrameBuffer = all {
    state.intent == DetachFrameBuffer,
    existingTargetBindingIsExact(state),
    state' = markApplied(state)
      .with("frameBufferSourceId", missing),
  }

  action stageBindScene = all {
    state.status == Idle,
    state.sceneAExists,
    state.frameBufferExists,
    state.sceneAFrameBufferId == missing,
    state' = stageFor(state, BindScene, sceneAId, SceneItem),
  }

  action applyBindScene = all {
    state.intent == BindScene,
    existingTargetBindingIsExact(state),
    state.sceneAExists,
    state.frameBufferExists,
    state' = markApplied(state)
      .with("sceneAFrameBufferId", frameBufferId),
  }

  action stageBindTransition = all {
    state.status == Idle,
    state.transitionExists,
    state.sceneAExists,
    state.sceneBExists,
    state.transitionSourceSceneId == missing,
    state.transitionTargetSceneId == missing,
    state' = stageFor(
      state,
      BindTransition,
      transitionId,
      TransitionItem
    ),
  }

  action applyBindTransition = all {
    state.intent == BindTransition,
    existingTargetBindingIsExact(state),
    state.sceneAExists,
    state.sceneBExists,
    sceneAId != sceneBId,
    state' = markApplied(state)
      .with("transitionSourceSceneId", sceneAId)
      .with("transitionTargetSceneId", sceneBId),
  }

  action stageDeleteFocus = all {
    state.status == Idle,
    state.focusExists,
    state' = stageFor(state, DeleteEntity, state.focusId, state.focusKind),
  }

  action applyDeleteFocus = all {
    state.intent == DeleteEntity,
    existingTargetBindingIsExact(state),
    focusIsReferenceSafeToDelete(state),
    state' = markApplied(state)
      .with("lastDeletedFocusId", state.focusId)
      .with("focusExists", false)
      .with("focusId", missing)
      .with("focusBirthId", missing)
      .with("focusKind", UnknownItem)
      .with("focusStartFrame", 0)
      .with("focusLayer", 0)
      .with("focusDuration", 1)
      .with("focusParentId", missing)
      .with("focusGroupId", missing)
      .with("focusMaskId", missing)
      // Removing the deleted item from its owned sibling list is structural
      // compaction, not an implicit cascade through a cross-entity reference.
      .with("paintSlotOneId", state.paintSiblingId)
      .with("paintSlotTwoId", missing)
      .with("keyframeCount", 0)
      .with("keyframeOneFrame", missing)
      .with("keyframeTwoFrame", missing),
  }

  action rejectReferencedDelete = all {
    state.intent == DeleteEntity,
    existingTargetBindingIsExact(state),
    not(focusIsReferenceSafeToDelete(state)),
    state' = markRejected(state),
  }

  // Environment drift after staging cannot be silently retargeted.
  action switchTargetGraph = all {
    state.status == Staged,
    state' = state.with("targetGraphId", state.targetGraphId + 1),
  }

  action changeDescriptor = all {
    state.status == Staged,
    state' = state.with("descriptorDigest", state.descriptorDigest + 1),
  }

  action rejectStaleBinding = all {
    state.status == Staged,
    not(bindingMatchesCurrentGraph(state)),
    state' = markRejected(state),
  }

  action stutterTerminal = all {
    any {
      state.status == Applied,
      state.status == NormalizedNoOp,
      state.status == Rejected,
    },
    state' = state,
  }

  action step = any {
    beginNextEdit,
    stageCreateAudio,
    stageCreateVoice,
    stageCreateAsset,
    stageCreateText,
    stageCreateShape,
    stageCreateEffect,
    stageCreateFrameBuffer,
    stageCreateGroup,
    stageCreateScene,
    stageCreateTransition,
    rejectUnknownCreate,
    rejectCreateOverExisting,
    applyCreate,
    stageGeometryUpdate,
    stageGeometryNoOp,
    stageInvalidGeometry,
    applyGeometryUpdate,
    finishGeometryNoOp,
    rejectInvalidGeometry,
    stageReverseKeyframes,
    applyCanonicalKeyframes,
    finishCanonicalKeyframesNoOp,
    stageMoveFocusToFront,
    stageMoveFocusToBack,
    applyPaintOrderMove,
    finishPaintOrderNoOp,
    stageAttachFocusReferences,
    applyAttachFocusReferences,
    stageDetachFocusReferences,
    applyDetachFocusReferences,
    stageAttachRootParent,
    applyAttachRootParent,
    rejectParentCycle,
    stageAppendFirstEffect,
    applyAppendFirstEffect,
    stageAppendSecondEffect,
    applyAppendSecondEffect,
    stageReorderEffects,
    applyReorderEffects,
    stageDetachEffects,
    applyDetachEffects,
    stageBindFrameBuffer,
    applyBindFrameBuffer,
    stageDetachFrameBuffer,
    applyDetachFrameBuffer,
    stageBindScene,
    applyBindScene,
    stageBindTransition,
    applyBindTransition,
    stageDeleteFocus,
    applyDeleteFocus,
    rejectReferencedDelete,
    switchTargetGraph,
    changeDescriptor,
    rejectStaleBinding,
    stutterTerminal,
  }

  val stableEntityIdentity = all {
    state.lastAllocatedId >= transitionId,
    state.paintSiblingExists,
    state.paintSiblingId == stablePaintSiblingId,
    state.focusExists
      == all {
        state.focusId != missing,
        state.focusBirthId != missing,
      },
    not(state.focusExists)
      or all {
        state.focusId == state.focusBirthId,
        state.focusId <= state.lastAllocatedId,
        state.focusId != rootGroupId,
        state.focusId != maskShapeId,
        state.focusId != state.paintSiblingId,
        state.focusId != firstEffectId,
        state.focusId != secondEffectId,
        state.focusId != frameBufferId,
        state.focusId != sceneAId,
        state.focusId != sceneBId,
        state.focusId != transitionId,
        state.focusId != state.lastDeletedFocusId,
      },
  }

  // Occupied paint slots are contiguous and are an exact permutation of the
  // existing siblings. There is never a duplicate, an omitted item, or a gap.
  val siblingPaintOrderIsCanonical =
    if (state.focusExists)
      all {
        state.paintSiblingExists,
        state.paintSlotOneId != missing,
        state.paintSlotTwoId != missing,
        state.paintSlotOneId != state.paintSlotTwoId,
        any {
          all {
            state.paintSlotOneId == state.focusId,
            state.paintSlotTwoId == state.paintSiblingId,
          },
          all {
            state.paintSlotOneId == state.paintSiblingId,
            state.paintSlotTwoId == state.focusId,
          },
        },
      }
    else
      all {
        state.paintSiblingExists,
        state.paintSlotOneId == state.paintSiblingId,
        state.paintSlotTwoId == missing,
      }

  val storedGeometryIsLegal =
    not(state.focusExists)
      or all {
        isKnownKind(state.focusKind),
        state.focusStartFrame >= 0,
        state.focusLayer >= 0,
        state.focusDuration > 0,
      }

  val keyframesAreCanonical = all {
    state.keyframeCount >= 0,
    state.keyframeCount <= 2,
    state.keyframeCount != 0
      or all {
        state.keyframeOneFrame == missing,
        state.keyframeTwoFrame == missing,
      },
    state.keyframeCount != 1
      or all {
        state.focusExists,
        state.keyframeOneFrame >= 0,
        state.keyframeOneFrame < state.focusDuration,
        state.keyframeTwoFrame == missing,
      },
    state.keyframeCount != 2
      or all {
        state.focusExists,
        state.keyframeOneFrame >= 0,
        state.keyframeOneFrame < state.keyframeTwoFrame,
        state.keyframeTwoFrame < state.focusDuration,
      },
  }

  val hierarchyMaskAndGroupReferencesAreSafe = all {
    state.focusParentId == missing
      or all {
        state.focusExists,
        state.rootGroupExists,
        state.focusParentId == rootGroupId,
        state.focusId != rootGroupId,
      },
    state.focusGroupId == missing
      or all {
        state.focusExists,
        state.rootGroupExists,
        state.focusGroupId == rootGroupId,
        state.focusId != rootGroupId,
      },
    state.focusMaskId == missing
      or all {
        state.focusExists,
        state.maskShapeExists,
        isVisualKind(state.focusKind),
        state.focusMaskId == maskShapeId,
        state.focusId != maskShapeId,
      },
    state.rootGroupParentId == missing
      or all {
        state.rootGroupExists,
        state.focusExists,
        state.focusKind == GroupItem,
        state.rootGroupParentId == state.focusId,
        state.focusId != rootGroupId,
      },
    not(all {
      any {
        state.focusParentId == rootGroupId,
        state.focusGroupId == rootGroupId,
      },
      state.rootGroupParentId == state.focusId,
    }),
  }

  val effectChainIsCanonical = all {
    state.effectSlotOneId == missing
      or any {
        state.effectSlotOneId == firstEffectId,
        state.effectSlotOneId == secondEffectId,
      },
    state.effectSlotTwoId == missing
      or any {
        state.effectSlotTwoId == firstEffectId,
        state.effectSlotTwoId == secondEffectId,
      },
    state.effectSlotOneId == missing
      == all {
        state.effectSlotTwoId == missing,
        state.firstEffectOwnerId == missing,
        state.secondEffectOwnerId == missing,
      },
    state.effectSlotTwoId == missing
      or all {
        state.effectSlotOneId != missing,
        state.effectSlotOneId != state.effectSlotTwoId,
      },
    state.firstEffectOwnerId == missing
      or all {
        state.firstEffectExists,
        state.focusExists,
        state.firstEffectOwnerId == state.focusId,
        any {
          state.effectSlotOneId == firstEffectId,
          state.effectSlotTwoId == firstEffectId,
        },
      },
    state.secondEffectOwnerId == missing
      or all {
        state.secondEffectExists,
        state.focusExists,
        state.secondEffectOwnerId == state.focusId,
        any {
          state.effectSlotOneId == secondEffectId,
          state.effectSlotTwoId == secondEffectId,
        },
      },
    state.effectSlotOneId != firstEffectId
      or state.firstEffectOwnerId == state.focusId,
    state.effectSlotTwoId != firstEffectId
      or state.firstEffectOwnerId == state.focusId,
    state.effectSlotOneId != secondEffectId
      or state.secondEffectOwnerId == state.focusId,
    state.effectSlotTwoId != secondEffectId
      or state.secondEffectOwnerId == state.focusId,
  }

  val renderGraphReferencesAreTyped = all {
    state.frameBufferSourceId == missing
      or all {
        state.frameBufferExists,
        state.focusExists,
        isVisualKind(state.focusKind),
        state.frameBufferSourceId == state.focusId,
      },
    state.sceneAFrameBufferId == missing
      or all {
        state.sceneAExists,
        state.frameBufferExists,
        state.sceneAFrameBufferId == frameBufferId,
      },
    state.sceneBFrameBufferId == missing
      or all {
        state.sceneBExists,
        state.frameBufferExists,
        state.sceneBFrameBufferId == frameBufferId,
      },
    state.transitionSourceSceneId == missing
      == (state.transitionTargetSceneId == missing),
    state.transitionSourceSceneId == missing
      or all {
        state.transitionExists,
        state.sceneAExists,
        state.sceneBExists,
        state.transitionSourceSceneId == sceneAId,
        state.transitionTargetSceneId == sceneBId,
        state.transitionSourceSceneId != state.transitionTargetSceneId,
      },
  }

  val noDanglingReferences = all {
    hierarchyMaskAndGroupReferencesAreSafe,
    effectChainIsCanonical,
    renderGraphReferencesAreTyped,
    state.focusExists
      or all {
        state.focusParentId == missing,
        state.focusGroupId == missing,
        state.focusMaskId == missing,
        state.rootGroupParentId == missing,
        state.effectSlotOneId == missing,
        state.effectSlotTwoId == missing,
        state.firstEffectOwnerId == missing,
        state.secondEffectOwnerId == missing,
        state.frameBufferSourceId == missing,
      },
  }

  val stagedTargetsAreKnownAndExactlyBound =
    state.status != Staged
      or all {
        isKnownKind(state.stagedKind),
        state.binding.targetKind == state.stagedKind,
        state.binding.targetEntityId == state.stagedTargetId,
        state.binding.requestDigest == state.requestCounter,
      }

  val mutationRequiresExactBinding =
    state.status != Applied
      or all {
        state.lastAppliedBinding == state.binding,
        state.binding.sourceGraphId == state.sourceGraphId,
        state.binding.targetGraphId == state.targetGraphId,
        state.binding.baseRevision + 1 == state.revision,
        state.binding.targetFingerprint + 1 == state.targetFingerprint,
        state.binding.descriptorDigest == state.descriptorDigest,
        isKnownKind(state.binding.targetKind),
      }

  val revisionTracksOnlySemanticMutation = all {
    state.revision == initialRevision + state.semanticMutationCount,
    state.targetFingerprint
      == initialFingerprint + state.semanticMutationCount,
    state.status != NormalizedNoOp
      or all {
        state.revision == state.outcomeBaseRevision,
        state.targetFingerprint == state.binding.targetFingerprint,
        state.lastAppliedBinding != state.binding,
      },
    state.status != Rejected
      or state.revision == state.outcomeBaseRevision,
  }

  val referenceSafeDeletePolicy =
    state.focusExists
      or all {
        not(focusHasIncomingReferences(state)),
        not(focusHasOutgoingReferences(state)),
      }

  val compositionGraphSafety = and {
    stableEntityIdentity,
    siblingPaintOrderIsCanonical,
    storedGeometryIsLegal,
    keyframesAreCanonical,
    noDanglingReferences,
    stagedTargetsAreKnownAndExactlyBound,
    mutationRequiresExactBinding,
    revisionTracksOnlySemanticMutation,
    referenceSafeDeletePolicy,
  }

  run geometryAndCanonicalKeyframeTest = init
    .then(stageGeometryUpdate)
    .then(applyGeometryUpdate)
    .then(beginNextEdit)
    .then(stageReverseKeyframes)
    .then(applyCanonicalKeyframes)
    .expect(all {
      compositionGraphSafety,
      state.status == Applied,
      state.focusId == initialFocusId,
      state.focusBirthId == initialFocusId,
      state.focusStartFrame == 2,
      state.focusLayer == 2,
      state.focusDuration == 12,
      state.keyframeCount == 2,
      state.keyframeOneFrame == 2,
      state.keyframeTwoFrame == 6,
      state.keyframeOneInterpolation == EaseIn,
      state.keyframeTwoInterpolation == Linear,
      state.revision == initialRevision + 2,
    })

  run normalizedGeometryNoOpDoesNotAdvanceRevisionTest = init
    .then(stageGeometryNoOp)
    .then(finishGeometryNoOp)
    .expect(all {
      compositionGraphSafety,
      state.status == NormalizedNoOp,
      state.revision == initialRevision,
      state.semanticMutationCount == 0,
      state.focusId == initialFocusId,
    })

  run reverseKeyframeInputCanNormalizeToNoOpTest = init
    .then(stageReverseKeyframes)
    .then(applyCanonicalKeyframes)
    .then(beginNextEdit)
    .then(stageReverseKeyframes)
    .then(finishCanonicalKeyframesNoOp)
    .expect(all {
      compositionGraphSafety,
      state.status == NormalizedNoOp,
      state.keyframeOneFrame == 2,
      state.keyframeTwoFrame == 6,
      state.revision == initialRevision + 1,
      state.semanticMutationCount == 1,
    })

  run paintOrderMoveAndNormalizedNoOpTest = init
    .then(stageMoveFocusToFront)
    .then(applyPaintOrderMove)
    .then(beginNextEdit)
    .then(stageMoveFocusToFront)
    .then(finishPaintOrderNoOp)
    .then(beginNextEdit)
    .then(stageMoveFocusToBack)
    .then(applyPaintOrderMove)
    .expect(all {
      compositionGraphSafety,
      state.status == Applied,
      state.paintSlotOneId == state.focusId,
      state.paintSlotTwoId == state.paintSiblingId,
      state.focusLayer == 1,
      state.effectSlotOneId == missing,
      state.effectSlotTwoId == missing,
      state.revision == initialRevision + 2,
      state.semanticMutationCount == 2,
    })

  run paintOrderBindingDriftFailsClosedTest = init
    .then(stageMoveFocusToFront)
    .then(switchTargetGraph)
    .then(rejectStaleBinding)
    .expect(all {
      compositionGraphSafety,
      state.status == Rejected,
      state.targetGraphId != state.binding.targetGraphId,
      state.paintSlotOneId == state.focusId,
      state.paintSlotTwoId == state.paintSiblingId,
      state.focusLayer == 1,
      state.revision == initialRevision,
      state.semanticMutationCount == 0,
      state.rejectionCount == 1,
    })

  run safeDeleteThenCreateUsesNewIdentityTest = init
    .then(stageDeleteFocus)
    .then(applyDeleteFocus)
    .then(beginNextEdit)
    .then(stageCreateShape)
    .then(applyCreate)
    .expect(all {
      compositionGraphSafety,
      state.status == Applied,
      state.lastDeletedFocusId == initialFocusId,
      state.focusExists,
      state.focusKind == ShapeItem,
      state.focusId == transitionId + 1,
      state.focusBirthId == state.focusId,
      state.revision == initialRevision + 2,
    })

  run unknownItemKindFailsClosedTest = init
    .then(stageDeleteFocus)
    .then(applyDeleteFocus)
    .then(beginNextEdit)
    .then(rejectUnknownCreate)
    .expect(all {
      compositionGraphSafety,
      state.status == Rejected,
      state.stagedKind == UnknownItem,
      not(state.focusExists),
      state.lastAllocatedId == transitionId,
      state.revision == initialRevision + 1,
      state.semanticMutationCount == 1,
      state.rejectionCount == 1,
    })

  run referencedDeleteIsRejectedTest = init
    .then(stageBindFrameBuffer)
    .then(applyBindFrameBuffer)
    .then(beginNextEdit)
    .then(stageDeleteFocus)
    .then(rejectReferencedDelete)
    .expect(all {
      compositionGraphSafety,
      state.status == Rejected,
      state.focusExists,
      state.frameBufferSourceId == state.focusId,
      state.revision == initialRevision + 1,
      state.semanticMutationCount == 1,
      state.rejectionCount == 1,
    })

  run explicitDetachAllowsDeleteTest = init
    .then(stageBindFrameBuffer)
    .then(applyBindFrameBuffer)
    .then(beginNextEdit)
    .then(stageDetachFrameBuffer)
    .then(applyDetachFrameBuffer)
    .then(beginNextEdit)
    .then(stageDeleteFocus)
    .then(applyDeleteFocus)
    .expect(all {
      compositionGraphSafety,
      state.status == Applied,
      not(state.focusExists),
      state.frameBufferSourceId == missing,
      state.revision == initialRevision + 3,
    })

  run hierarchyCycleFailsClosedTest = init
    .then(stageDeleteFocus)
    .then(applyDeleteFocus)
    .then(beginNextEdit)
    .then(stageCreateGroup)
    .then(applyCreate)
    .then(beginNextEdit)
    .then(stageAttachFocusReferences)
    .then(applyAttachFocusReferences)
    .then(beginNextEdit)
    .then(stageAttachRootParent)
    .then(rejectParentCycle)
    .expect(all {
      compositionGraphSafety,
      state.status == Rejected,
      state.focusKind == GroupItem,
      state.focusParentId == rootGroupId,
      state.rootGroupParentId == missing,
      state.revision == initialRevision + 3,
      state.rejectionCount == 1,
    })

  run effectOrderIsExplicitAndContiguousTest = init
    .then(stageAppendFirstEffect)
    .then(applyAppendFirstEffect)
    .then(beginNextEdit)
    .then(stageAppendSecondEffect)
    .then(applyAppendSecondEffect)
    .then(beginNextEdit)
    .then(stageReorderEffects)
    .then(applyReorderEffects)
    .expect(all {
      compositionGraphSafety,
      state.status == Applied,
      state.effectSlotOneId == secondEffectId,
      state.effectSlotTwoId == firstEffectId,
      state.firstEffectOwnerId == state.focusId,
      state.secondEffectOwnerId == state.focusId,
      state.revision == initialRevision + 3,
    })

  run typedRenderReferenceChainTest = init
    .then(stageBindFrameBuffer)
    .then(applyBindFrameBuffer)
    .then(beginNextEdit)
    .then(stageBindScene)
    .then(applyBindScene)
    .then(beginNextEdit)
    .then(stageBindTransition)
    .then(applyBindTransition)
    .expect(all {
      compositionGraphSafety,
      state.status == Applied,
      state.frameBufferSourceId == state.focusId,
      state.sceneAFrameBufferId == frameBufferId,
      state.transitionSourceSceneId == sceneAId,
      state.transitionTargetSceneId == sceneBId,
      state.revision == initialRevision + 3,
    })

  run switchedTargetRejectsExactBindingTest = init
    .then(stageGeometryUpdate)
    .then(switchTargetGraph)
    .then(rejectStaleBinding)
    .expect(all {
      compositionGraphSafety,
      state.status == Rejected,
      state.targetGraphId != state.binding.targetGraphId,
      state.focusStartFrame == 0,
      state.revision == initialRevision,
      state.semanticMutationCount == 0,
      state.rejectionCount == 1,
    })

  run descriptorDriftRejectsBeforeMutationTest = init
    .then(stageGeometryUpdate)
    .then(changeDescriptor)
    .then(rejectStaleBinding)
    .expect(all {
      compositionGraphSafety,
      state.status == Rejected,
      state.descriptorDigest != state.binding.descriptorDigest,
      state.focusStartFrame == 0,
      state.revision == initialRevision,
      state.semanticMutationCount == 0,
      state.rejectionCount == 1,
    })
}
