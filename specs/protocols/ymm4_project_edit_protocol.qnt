// YMM4 project-, scene-, character-, and template-edit semantics.
//
// This model deliberately starts with every broad project mutation family
// unavailable. A future adapter may advertise one family only by advancing a
// versioned edit surface. Every preview seals that surface, the active source,
// the resource revision, and any descriptor/dependent evidence it needs.
//
// Generic writer exclusion, bridge WAL, rollback, and receipt replay remain in
// ymm4_apply_protocol.qnt and ymm4_change_set_protocol.qnt. This module models
// the domain rules that those execution protocols must preserve. A green run
// does not prove YMM4 private APIs, UI behavior, filesystem publication, or
// render output.
module ymm4ProjectEditProtocol {
  type ProjectEditStatus =
    | ProjectIdle
    | ProjectPreviewable
    | ProjectApproved
    | ProjectConflicted
    | ProjectRejected
    | ProjectCommitted
    | ProjectNoOp

  type ProjectEditKind =
    | NoProjectEdit
    | ProjectSettingsEdit
    | SceneCreate
    | SceneUpdate
    | SceneDelete
    | TimelineCreate
    | TimelineUpdate
    | TimelineDelete
    | CharacterCreate
    | CharacterUpdate
    | CharacterDelete
    | NativeTemplateInstantiate
    | NativeTemplateDefinitionEdit

  // Finite representatives for project-wide settings that have different
  // concrete YMM4 members but the same approval/invalidation semantics.
  type ProjectSettingClass =
    | NoProjectSetting
    | CanvasSize
    | FrameRate
    | AudioSampleRate
    | BackgroundColor
    | TimelineLayerPolicy

  // CharacterUpdate is always a partial update of exactly one known setting
  // class. CharacterCreate and CharacterDelete operate on the whole resource,
  // so they carry NoCharacterSetting rather than pretending to be partial.
  type CharacterSettingClass =
    | NoCharacterSetting
    | CharacterName
    | CharacterGroup
    | CharacterVoiceType
    | CharacterVoiceParameters
    | CharacterPortraitConfiguration

  type DependentPolicy =
    | NoDependentPolicy
    | RejectWhenReferenced
    | RewriteManagedDependents

  type ProjectEditPlan = {
    kind: ProjectEditKind,
    settingClass: ProjectSettingClass,
    characterSettingClass: CharacterSettingClass,
    characterNamePreimageDigest: int,
    characterGroupPreimageDigest: int,
    characterVoiceTypePreimageDigest: int,
    characterVoiceParametersPreimageDigest: int,
    characterPortraitConfigurationPreimageDigest: int,
    unknownCharacterConfigurationPreimageDigest: int,
    digest: int,
    baseRevision: int,
    sourceFingerprint: int,
    editSurfaceRevision: int,
    capabilityRevision: int,
    targetResourceRevision: int,
    descriptorCatalogDigest: int,
    dependentSetDigest: int,
    dependentCount: int,
    dependentPolicy: DependentPolicy,
    templateContentDigest: int,
    expansionDigest: int,
    predictedChangedEntities: int,
    normalizedNoOp: bool,
  }

  type ProjectEditState = {
    status: ProjectEditStatus,

    headRevision: int,
    sourceFingerprint: int,
    editSurfaceRevision: int,
    capabilityRevision: int,
    descriptorCatalogRevision: int,

    settingsMutationSupported: bool,
    sceneMutationSupported: bool,
    timelineMutationSupported: bool,
    characterMutationSupported: bool,
    templateMutationSupported: bool,

    settingsRevision: int,
    sceneTopologyRevision: int,
    characterCatalogRevision: int,
    templateCatalogRevision: int,

    sceneCount: int,
    referencedSceneCount: int,
    sceneReferenceSetDigest: int,
    timelineCount: int,
    referencedTimelineCount: int,
    timelineReferenceSetDigest: int,
    characterCount: int,
    characterDependentCount: int,
    managedCharacterDependentCount: int,
    dependentSetDigest: int,

    // These finite digests witness both the selected known character setting
    // and every known/unknown setting that a partial update must preserve.
    characterNameDigest: int,
    characterGroupDigest: int,
    characterVoiceTypeDigest: int,
    characterVoiceParametersDigest: int,
    characterPortraitConfigurationDigest: int,
    unknownCharacterConfigurationDigest: int,

    templateDescriptorAvailable: bool,
    templateContentDigest: int,
    templateExpansionDigest: int,
    templateProducedKindsSupported: bool,
    instantiatedTemplateItems: int,

    derivedEpoch: int,
    checkpointEpoch: int,
    inspectionEpoch: int,
    renderEpoch: int,
    compositionEpoch: int,
    timelinePlanEpoch: int,

    projectMutationCount: int,
    canonicalCommitCount: int,
    externalCommitCount: int,
    observationCount: int,
    rejectedAttemptCount: int,
    preservedDependentItems: int,
    lastRejectedKind: ProjectEditKind,

    plan: ProjectEditPlan,
    approvedPlan: ProjectEditPlan,
    committedPlan: ProjectEditPlan,
  }

  val missing = -1
  val initialHeadRevision = 20

  val emptyPlan = {
    kind: NoProjectEdit,
    settingClass: NoProjectSetting,
    characterSettingClass: NoCharacterSetting,
    characterNamePreimageDigest: missing,
    characterGroupPreimageDigest: missing,
    characterVoiceTypePreimageDigest: missing,
    characterVoiceParametersPreimageDigest: missing,
    characterPortraitConfigurationPreimageDigest: missing,
    unknownCharacterConfigurationPreimageDigest: missing,
    digest: 0,
    baseRevision: missing,
    sourceFingerprint: missing,
    editSurfaceRevision: missing,
    capabilityRevision: missing,
    targetResourceRevision: missing,
    descriptorCatalogDigest: missing,
    dependentSetDigest: missing,
    dependentCount: 0,
    dependentPolicy: NoDependentPolicy,
    templateContentDigest: missing,
    expansionDigest: missing,
    predictedChangedEntities: 0,
    normalizedNoOp: false,
  }

  var state: ProjectEditState

  pure def approvalMustRemainBound(status: ProjectEditStatus): bool =
    any {
      status == ProjectApproved,
      status == ProjectConflicted,
      status == ProjectCommitted,
      status == ProjectNoOp,
    }

  pure def targetRevisionFor(s: ProjectEditState, kind: ProjectEditKind): int =
    if (kind == ProjectSettingsEdit) s.settingsRevision
    else if (kind == SceneCreate or kind == SceneUpdate or kind == SceneDelete
      or kind == TimelineCreate or kind == TimelineUpdate or kind == TimelineDelete)
      s.sceneTopologyRevision
    else if (kind == CharacterCreate or kind == CharacterUpdate or kind == CharacterDelete)
      s.characterCatalogRevision
    else if (kind == NativeTemplateInstantiate or kind == NativeTemplateDefinitionEdit)
      s.templateCatalogRevision
    else missing

  pure def kindIsSupported(s: ProjectEditState, kind: ProjectEditKind): bool =
    if (kind == ProjectSettingsEdit) s.settingsMutationSupported
    else if (kind == SceneCreate or kind == SceneUpdate or kind == SceneDelete)
      s.sceneMutationSupported
    else if (kind == TimelineCreate or kind == TimelineUpdate or kind == TimelineDelete)
      s.timelineMutationSupported
    else if (kind == CharacterCreate or kind == CharacterUpdate or kind == CharacterDelete)
      s.characterMutationSupported
    else if (kind == NativeTemplateInstantiate) s.templateMutationSupported
    else false

  pure def descriptorDigestFor(s: ProjectEditState, kind: ProjectEditKind): int =
    if (kind == NoProjectEdit) missing else s.descriptorCatalogRevision

  pure def dependentDigestFor(s: ProjectEditState, kind: ProjectEditKind): int =
    if (kind == SceneDelete) s.sceneReferenceSetDigest
    else if (kind == TimelineDelete) s.timelineReferenceSetDigest
    else if (kind == CharacterUpdate or kind == CharacterDelete) s.dependentSetDigest
    else 0

  pure def dependentCountFor(s: ProjectEditState, kind: ProjectEditKind): int =
    if (kind == SceneDelete) s.referencedSceneCount
    else if (kind == TimelineDelete) s.referencedTimelineCount
    else if (kind == CharacterUpdate or kind == CharacterDelete)
      s.characterDependentCount
    else 0

  pure def sealedPlan(
    s: ProjectEditState,
    kind: ProjectEditKind,
    settingClass: ProjectSettingClass,
    policy: DependentPolicy,
    changedEntities: int,
    normalizedNoOp: bool
  ): ProjectEditPlan = {
    kind: kind,
    settingClass: settingClass,
    characterSettingClass: NoCharacterSetting,
    characterNamePreimageDigest: missing,
    characterGroupPreimageDigest: missing,
    characterVoiceTypePreimageDigest: missing,
    characterVoiceParametersPreimageDigest: missing,
    characterPortraitConfigurationPreimageDigest: missing,
    unknownCharacterConfigurationPreimageDigest: missing,
    digest: s.plan.digest + 1,
    baseRevision: s.headRevision,
    sourceFingerprint: s.sourceFingerprint,
    editSurfaceRevision: s.editSurfaceRevision,
    capabilityRevision: s.capabilityRevision,
    targetResourceRevision: targetRevisionFor(s, kind),
    descriptorCatalogDigest: descriptorDigestFor(s, kind),
    dependentSetDigest: dependentDigestFor(s, kind),
    dependentCount: dependentCountFor(s, kind),
    dependentPolicy: policy,
    templateContentDigest:
      if (kind == NativeTemplateInstantiate) s.templateContentDigest else 0,
    expansionDigest:
      if (kind == NativeTemplateInstantiate) s.templateExpansionDigest else 0,
    predictedChangedEntities: changedEntities,
    normalizedNoOp: normalizedNoOp,
  }

  pure def sealedCharacterUpdatePlan(
    s: ProjectEditState,
    settingClass: CharacterSettingClass
  ): ProjectEditPlan =
    sealedPlan(
      s,
      CharacterUpdate,
      NoProjectSetting,
      RewriteManagedDependents,
      s.characterDependentCount + 1,
      false)
      .with("characterSettingClass", settingClass)
      .with("characterNamePreimageDigest", s.characterNameDigest)
      .with("characterGroupPreimageDigest", s.characterGroupDigest)
      .with("characterVoiceTypePreimageDigest", s.characterVoiceTypeDigest)
      .with(
        "characterVoiceParametersPreimageDigest",
        s.characterVoiceParametersDigest)
      .with(
        "characterPortraitConfigurationPreimageDigest",
        s.characterPortraitConfigurationDigest)
      .with(
        "unknownCharacterConfigurationPreimageDigest",
        s.unknownCharacterConfigurationDigest)

  pure def planIsFresh(s: ProjectEditState): bool = all {
    s.plan.baseRevision == s.headRevision,
    s.plan.sourceFingerprint == s.sourceFingerprint,
    s.plan.editSurfaceRevision == s.editSurfaceRevision,
    s.plan.capabilityRevision == s.capabilityRevision,
    s.plan.targetResourceRevision == targetRevisionFor(s, s.plan.kind),
    s.plan.descriptorCatalogDigest == descriptorDigestFor(s, s.plan.kind),
    s.plan.dependentSetDigest == dependentDigestFor(s, s.plan.kind),
    s.plan.dependentCount == dependentCountFor(s, s.plan.kind),
    s.plan.kind != CharacterUpdate
      or all {
        s.plan.characterSettingClass != NoCharacterSetting,
        s.plan.characterNamePreimageDigest == s.characterNameDigest,
        s.plan.characterGroupPreimageDigest == s.characterGroupDigest,
        s.plan.characterVoiceTypePreimageDigest == s.characterVoiceTypeDigest,
        s.plan.characterVoiceParametersPreimageDigest
          == s.characterVoiceParametersDigest,
        s.plan.characterPortraitConfigurationPreimageDigest
          == s.characterPortraitConfigurationDigest,
        s.plan.unknownCharacterConfigurationPreimageDigest
          == s.unknownCharacterConfigurationDigest,
      },
    s.plan.kind != NativeTemplateInstantiate
      or all {
        s.templateDescriptorAvailable,
        s.templateProducedKindsSupported,
        s.plan.templateContentDigest == s.templateContentDigest,
        s.plan.expansionDigest == s.templateExpansionDigest,
      },
  }

  pure def characterPlanPreservesDependents(s: ProjectEditState): bool =
    not(s.plan.kind == CharacterUpdate or s.plan.kind == CharacterDelete)
      or s.plan.dependentCount == 0
      or all {
        s.plan.dependentPolicy == RewriteManagedDependents,
        s.managedCharacterDependentCount == s.plan.dependentCount,
        s.characterDependentCount == s.plan.dependentCount,
        s.plan.dependentSetDigest == s.dependentSetDigest,
      }

  pure def advanceActualEdit(s: ProjectEditState): ProjectEditState =
    s
      .with("status", ProjectCommitted)
      .with("headRevision", s.headRevision + 1)
      .with("settingsRevision",
        if (s.plan.kind == ProjectSettingsEdit) s.settingsRevision + 1 else s.settingsRevision)
      .with("sceneTopologyRevision",
        if (s.plan.kind == SceneCreate or s.plan.kind == SceneUpdate
          or s.plan.kind == SceneDelete or s.plan.kind == TimelineCreate
          or s.plan.kind == TimelineUpdate or s.plan.kind == TimelineDelete)
          s.sceneTopologyRevision + 1 else s.sceneTopologyRevision)
      .with("characterCatalogRevision",
        if (s.plan.kind == CharacterCreate or s.plan.kind == CharacterUpdate
          or s.plan.kind == CharacterDelete)
          s.characterCatalogRevision + 1 else s.characterCatalogRevision)
      .with("characterNameDigest",
        if (s.plan.kind == CharacterUpdate
          and s.plan.characterSettingClass == CharacterName)
          s.characterNameDigest + 1 else s.characterNameDigest)
      .with("characterGroupDigest",
        if (s.plan.kind == CharacterUpdate
          and s.plan.characterSettingClass == CharacterGroup)
          s.characterGroupDigest + 1 else s.characterGroupDigest)
      .with("characterVoiceTypeDigest",
        if (s.plan.kind == CharacterUpdate
          and s.plan.characterSettingClass == CharacterVoiceType)
          s.characterVoiceTypeDigest + 1 else s.characterVoiceTypeDigest)
      .with("characterVoiceParametersDigest",
        if (s.plan.kind == CharacterUpdate
          and s.plan.characterSettingClass == CharacterVoiceParameters)
          s.characterVoiceParametersDigest + 1
        else s.characterVoiceParametersDigest)
      .with("characterPortraitConfigurationDigest",
        if (s.plan.kind == CharacterUpdate
          and s.plan.characterSettingClass == CharacterPortraitConfiguration)
          s.characterPortraitConfigurationDigest + 1
        else s.characterPortraitConfigurationDigest)
      .with("sceneCount",
        if (s.plan.kind == SceneCreate) s.sceneCount + 1
        else if (s.plan.kind == SceneDelete) s.sceneCount - 1
        else s.sceneCount)
      .with("timelineCount",
        if (s.plan.kind == TimelineCreate) s.timelineCount + 1
        else if (s.plan.kind == TimelineDelete) s.timelineCount - 1
        else s.timelineCount)
      .with("characterCount",
        if (s.plan.kind == CharacterCreate) s.characterCount + 1
        else if (s.plan.kind == CharacterDelete) s.characterCount - 1
        else s.characterCount)
      .with("characterDependentCount",
        if (s.plan.kind == CharacterDelete) 0 else s.characterDependentCount)
      .with("managedCharacterDependentCount",
        if (s.plan.kind == CharacterDelete) 0 else s.managedCharacterDependentCount)
      .with("dependentSetDigest",
        if (s.plan.kind == CharacterDelete) s.dependentSetDigest + 1
        else s.dependentSetDigest)
      .with("instantiatedTemplateItems",
        if (s.plan.kind == NativeTemplateInstantiate)
          s.instantiatedTemplateItems + s.plan.predictedChangedEntities
        else s.instantiatedTemplateItems)
      .with("preservedDependentItems",
        if (s.plan.kind == CharacterUpdate or s.plan.kind == CharacterDelete)
          s.preservedDependentItems + s.plan.dependentCount
        else s.preservedDependentItems)
      .with("derivedEpoch", s.derivedEpoch + 1)
      .with("projectMutationCount", s.projectMutationCount + 1)
      .with("canonicalCommitCount", s.canonicalCommitCount + 1)
      .with("committedPlan", s.plan)

  action init =
    state' = {
      status: ProjectIdle,

      headRevision: initialHeadRevision,
      sourceFingerprint: 100,
      editSurfaceRevision: 1,
      capabilityRevision: 1,
      descriptorCatalogRevision: 1,

      settingsMutationSupported: false,
      sceneMutationSupported: false,
      timelineMutationSupported: false,
      characterMutationSupported: false,
      templateMutationSupported: false,

      settingsRevision: 5,
      sceneTopologyRevision: 6,
      characterCatalogRevision: 7,
      templateCatalogRevision: 8,

      sceneCount: 2,
      referencedSceneCount: 1,
      sceneReferenceSetDigest: 60,
      timelineCount: 2,
      referencedTimelineCount: 1,
      timelineReferenceSetDigest: 70,
      characterCount: 2,
      characterDependentCount: 2,
      managedCharacterDependentCount: 2,
      dependentSetDigest: 90,

      characterNameDigest: 101,
      characterGroupDigest: 102,
      characterVoiceTypeDigest: 103,
      characterVoiceParametersDigest: 104,
      characterPortraitConfigurationDigest: 105,
      unknownCharacterConfigurationDigest: 199,

      templateDescriptorAvailable: false,
      templateContentDigest: 0,
      templateExpansionDigest: 0,
      templateProducedKindsSupported: false,
      instantiatedTemplateItems: 0,

      derivedEpoch: 0,
      checkpointEpoch: 0,
      inspectionEpoch: 0,
      renderEpoch: 0,
      compositionEpoch: 0,
      timelinePlanEpoch: 0,

      projectMutationCount: 0,
      canonicalCommitCount: 0,
      externalCommitCount: 0,
      observationCount: 0,
      rejectedAttemptCount: 0,
      preservedDependentItems: 0,
      lastRejectedKind: NoProjectEdit,

      plan: emptyPlan,
      approvedPlan: emptyPlan,
      committedPlan: emptyPlan,
    }

  // Capability discovery is read-only with respect to the project. It still
  // advances the edit-surface binding so an older preview cannot be reused.
  action enableSettingsMutation = all {
    state.status == ProjectIdle,
    not(state.settingsMutationSupported),
    state' = state
      .with("settingsMutationSupported", true)
      .with("editSurfaceRevision", state.editSurfaceRevision + 1)
      .with("capabilityRevision", state.capabilityRevision + 1)
      .with("descriptorCatalogRevision", state.descriptorCatalogRevision + 1),
  }

  action enableSceneMutation = all {
    state.status == ProjectIdle,
    not(state.sceneMutationSupported),
    state' = state
      .with("sceneMutationSupported", true)
      .with("editSurfaceRevision", state.editSurfaceRevision + 1)
      .with("capabilityRevision", state.capabilityRevision + 1)
      .with("descriptorCatalogRevision", state.descriptorCatalogRevision + 1),
  }

  action enableTimelineMutation = all {
    state.status == ProjectIdle,
    not(state.timelineMutationSupported),
    state' = state
      .with("timelineMutationSupported", true)
      .with("editSurfaceRevision", state.editSurfaceRevision + 1)
      .with("capabilityRevision", state.capabilityRevision + 1)
      .with("descriptorCatalogRevision", state.descriptorCatalogRevision + 1),
  }

  action enableCharacterMutation = all {
    state.status == ProjectIdle,
    not(state.characterMutationSupported),
    state' = state
      .with("characterMutationSupported", true)
      .with("editSurfaceRevision", state.editSurfaceRevision + 1)
      .with("capabilityRevision", state.capabilityRevision + 1)
      .with("descriptorCatalogRevision", state.descriptorCatalogRevision + 1),
  }

  action enableTemplateMutation = all {
    state.status == ProjectIdle,
    not(state.templateMutationSupported),
    state' = state
      .with("templateMutationSupported", true)
      .with("templateDescriptorAvailable", true)
      .with("templateProducedKindsSupported", true)
      .with("templateCatalogRevision", state.templateCatalogRevision + 1)
      .with("templateContentDigest", 81)
      .with("templateExpansionDigest", 82)
      .with("editSurfaceRevision", state.editSurfaceRevision + 1)
      .with("capabilityRevision", state.capabilityRevision + 1)
      .with("descriptorCatalogRevision", state.descriptorCatalogRevision + 1),
  }

  action observeProject = all {
    state.status == ProjectIdle,
    state' = state.with("observationCount", state.observationCount + 1),
  }

  action rejectUnsupportedSettingsEdit = all {
    state.status == ProjectIdle,
    not(state.settingsMutationSupported),
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", ProjectSettingsEdit),
  }

  action rejectUnsupportedTimelineEdit = all {
    state.status == ProjectIdle,
    not(state.timelineMutationSupported),
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", TimelineUpdate),
  }

  action stageSettingsEdit = all {
    state.status == ProjectIdle,
    state.settingsMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, ProjectSettingsEdit, CanvasSize, NoDependentPolicy, 1, false)),
  }

  action stageFrameRateEdit = all {
    state.status == ProjectIdle,
    state.settingsMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, ProjectSettingsEdit, FrameRate, NoDependentPolicy, 1, false)),
  }

  action stageAudioSampleRateEdit = all {
    state.status == ProjectIdle,
    state.settingsMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, ProjectSettingsEdit, AudioSampleRate, NoDependentPolicy, 1, false)),
  }

  action stageBackgroundColorEdit = all {
    state.status == ProjectIdle,
    state.settingsMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, ProjectSettingsEdit, BackgroundColor, NoDependentPolicy, 1, false)),
  }

  action stageTimelineLayerPolicyEdit = all {
    state.status == ProjectIdle,
    state.settingsMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, ProjectSettingsEdit, TimelineLayerPolicy, NoDependentPolicy, 1, false)),
  }

  action stageNormalizedSettingsNoOp = all {
    state.status == ProjectIdle,
    state.settingsMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, ProjectSettingsEdit, CanvasSize, NoDependentPolicy, 0, true)),
  }

  action stageSceneCreate = all {
    state.status == ProjectIdle,
    state.sceneMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, SceneCreate, NoProjectSetting, NoDependentPolicy, 1, false)),
  }

  action stageSceneUpdate = all {
    state.status == ProjectIdle,
    state.sceneMutationSupported,
    state.sceneCount > 0,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, SceneUpdate, NoProjectSetting, NoDependentPolicy, 1, false)),
  }

  action rejectReferencedSceneDelete = all {
    state.status == ProjectIdle,
    state.sceneMutationSupported,
    state.referencedSceneCount > 0,
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", SceneDelete),
  }

  // These selection actions abstract choosing another target from the same
  // observed topology. They change query evidence, not canonical project data.
  action selectUnreferencedSceneTarget = all {
    state.status == ProjectIdle,
    state.referencedSceneCount > 0,
    state' = state
      .with("referencedSceneCount", 0)
      .with("sceneReferenceSetDigest", state.sceneReferenceSetDigest + 1)
      .with("observationCount", state.observationCount + 1),
  }

  action stageUnreferencedSceneDelete = all {
    state.status == ProjectIdle,
    state.sceneMutationSupported,
    state.sceneCount > 1,
    state.referencedSceneCount == 0,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, SceneDelete, NoProjectSetting, RejectWhenReferenced, 1, false)),
  }

  action stageTimelineCreate = all {
    state.status == ProjectIdle,
    state.timelineMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, TimelineCreate, NoProjectSetting, NoDependentPolicy, 1, false)),
  }

  action stageTimelineUpdate = all {
    state.status == ProjectIdle,
    state.timelineMutationSupported,
    state.timelineCount > 0,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, TimelineUpdate, NoProjectSetting, NoDependentPolicy, 1, false)),
  }

  action rejectReferencedTimelineDelete = all {
    state.status == ProjectIdle,
    state.timelineMutationSupported,
    state.referencedTimelineCount > 0,
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", TimelineDelete),
  }

  action selectUnreferencedTimelineTarget = all {
    state.status == ProjectIdle,
    state.referencedTimelineCount > 0,
    state' = state
      .with("referencedTimelineCount", 0)
      .with("timelineReferenceSetDigest", state.timelineReferenceSetDigest + 1)
      .with("observationCount", state.observationCount + 1),
  }

  action stageUnreferencedTimelineDelete = all {
    state.status == ProjectIdle,
    state.timelineMutationSupported,
    state.timelineCount > 1,
    state.referencedTimelineCount == 0,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, TimelineDelete, NoProjectSetting, RejectWhenReferenced, 1, false)),
  }

  action rejectCharacterDeleteWithUnknownDependents = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterDependentCount > state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", CharacterDelete),
  }

  action rejectCharacterUpdateWithUnknownDependents = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterDependentCount > state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", CharacterUpdate),
  }

  action selectCharacterWithUnknownDependent = all {
    state.status == ProjectIdle,
    state.characterDependentCount == state.managedCharacterDependentCount,
    state' = state
      .with("characterDependentCount", state.characterDependentCount + 1)
      .with("dependentSetDigest", state.dependentSetDigest + 1)
      .with("observationCount", state.observationCount + 1),
  }

  action selectCharacterWithoutDependents = all {
    state.status == ProjectIdle,
    state.characterDependentCount > 0,
    state' = state
      .with("characterDependentCount", 0)
      .with("managedCharacterDependentCount", 0)
      .with("dependentSetDigest", state.dependentSetDigest + 1)
      .with("observationCount", state.observationCount + 1),
  }

  action stageCharacterCreate = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, CharacterCreate, NoProjectSetting, NoDependentPolicy, 1, false)),
  }

  action stageCharacterUpdateWithManagedRewrite = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterDependentCount == state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedCharacterUpdatePlan(state, CharacterName)),
  }

  action stageCharacterGroupUpdateWithManagedRewrite = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterDependentCount == state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedCharacterUpdatePlan(state, CharacterGroup)),
  }

  action stageCharacterVoiceTypeUpdateWithManagedRewrite = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterDependentCount == state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedCharacterUpdatePlan(state, CharacterVoiceType)),
  }

  action stageCharacterVoiceParametersUpdateWithManagedRewrite = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterDependentCount == state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectPreviewable)
      .with(
        "plan",
        sealedCharacterUpdatePlan(state, CharacterVoiceParameters)),
  }

  action stageCharacterPortraitConfigurationUpdateWithManagedRewrite = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterDependentCount == state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectPreviewable)
      .with(
        "plan",
        sealedCharacterUpdatePlan(state, CharacterPortraitConfiguration)),
  }

  action stageCharacterDeleteWithManagedRewrite = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterCount > 1,
    state.characterDependentCount > 0,
    state.characterDependentCount == state.managedCharacterDependentCount,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state,
        CharacterDelete,
        NoProjectSetting,
        RewriteManagedDependents,
        state.characterDependentCount + 1,
        false)),
  }

  action stageCharacterDeleteWithoutDependents = all {
    state.status == ProjectIdle,
    state.characterMutationSupported,
    state.characterCount > 1,
    state.characterDependentCount == 0,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state, CharacterDelete, NoProjectSetting, RejectWhenReferenced, 1, false)),
  }

  action rejectUnavailableTemplate = all {
    state.status == ProjectIdle,
    any {
      not(state.templateMutationSupported),
      not(state.templateDescriptorAvailable),
      not(state.templateProducedKindsSupported),
    },
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", NativeTemplateInstantiate),
  }

  action rejectTemplateDefinitionMutation = all {
    state.status == ProjectIdle,
    state' = state
      .with("status", ProjectRejected)
      .with("rejectedAttemptCount", state.rejectedAttemptCount + 1)
      .with("lastRejectedKind", NativeTemplateDefinitionEdit),
  }

  action stageTemplateInstantiation = all {
    state.status == ProjectIdle,
    state.templateMutationSupported,
    state.templateDescriptorAvailable,
    state.templateProducedKindsSupported,
    state.templateContentDigest > 0,
    state.templateExpansionDigest > 0,
    state' = state
      .with("status", ProjectPreviewable)
      .with("plan", sealedPlan(
        state,
        NativeTemplateInstantiate,
        NoProjectSetting,
        NoDependentPolicy,
        2,
        false)),
  }

  action approve = all {
    state.status == ProjectPreviewable,
    state.plan.kind != NoProjectEdit,
    state.plan.digest > 0,
    kindIsSupported(state, state.plan.kind),
    characterPlanPreservesDependents(state),
    planIsFresh(state),
    state' = state
      .with("status", ProjectApproved)
      .with("approvedPlan", state.plan),
  }

  action changeSourceAfterApproval = all {
    state.status == ProjectApproved,
    state' = state
      .with("headRevision", state.headRevision + 1)
      .with("sourceFingerprint", state.sourceFingerprint + 1)
      .with("externalCommitCount", state.externalCommitCount + 1),
  }

  action changeSurfaceAfterApproval = all {
    state.status == ProjectApproved,
    state' = state
      .with("editSurfaceRevision", state.editSurfaceRevision + 1)
      .with("capabilityRevision", state.capabilityRevision + 1),
  }

  action changeCapabilityAfterApproval = all {
    state.status == ProjectApproved,
    state' = state.with("capabilityRevision", state.capabilityRevision + 1),
  }

  action changeDescriptorAfterApproval = all {
    state.status == ProjectApproved,
    state' = state.with(
      "descriptorCatalogRevision", state.descriptorCatalogRevision + 1),
  }

  action changeCharacterDependentsAfterApproval = all {
    state.status == ProjectApproved,
    any {
      state.plan.kind == CharacterUpdate,
      state.plan.kind == CharacterDelete,
    },
    state' = state
      .with("characterDependentCount", state.characterDependentCount + 1)
      .with("managedCharacterDependentCount", state.managedCharacterDependentCount + 1)
      .with("dependentSetDigest", state.dependentSetDigest + 1)
      .with("observationCount", state.observationCount + 1),
  }

  action changeTemplateAfterApproval = all {
    state.status == ProjectApproved,
    state.plan.kind == NativeTemplateInstantiate,
    state' = state
      .with("templateCatalogRevision", state.templateCatalogRevision + 1)
      .with("templateContentDigest", state.templateContentDigest + 1)
      .with("templateExpansionDigest", state.templateExpansionDigest + 1),
  }

  action markConflict = all {
    state.status == ProjectApproved,
    not(planIsFresh(state)),
    state' = state.with("status", ProjectConflicted),
  }

  action commitApproved = all {
    state.status == ProjectApproved,
    state.plan == state.approvedPlan,
    kindIsSupported(state, state.plan.kind),
    planIsFresh(state),
    characterPlanPreservesDependents(state),
    not(state.plan.normalizedNoOp),
    state.plan.predictedChangedEntities > 0,
    state' = advanceActualEdit(state),
  }

  action finishNormalizedNoOp = all {
    state.status == ProjectApproved,
    state.plan == state.approvedPlan,
    planIsFresh(state),
    state.plan.normalizedNoOp,
    state.plan.predictedChangedEntities == 0,
    state' = state
      .with("status", ProjectNoOp)
      .with("committedPlan", state.plan),
  }

  action stutterTerminal = all {
    any {
      state.status == ProjectRejected,
      state.status == ProjectConflicted,
      state.status == ProjectCommitted,
      state.status == ProjectNoOp,
    },
    state' = state,
  }

  action step = any {
    enableSettingsMutation,
    enableSceneMutation,
    enableTimelineMutation,
    enableCharacterMutation,
    enableTemplateMutation,
    observeProject,
    rejectUnsupportedSettingsEdit,
    rejectUnsupportedTimelineEdit,
    stageSettingsEdit,
    stageFrameRateEdit,
    stageAudioSampleRateEdit,
    stageBackgroundColorEdit,
    stageTimelineLayerPolicyEdit,
    stageNormalizedSettingsNoOp,
    stageSceneCreate,
    stageSceneUpdate,
    rejectReferencedSceneDelete,
    selectUnreferencedSceneTarget,
    stageUnreferencedSceneDelete,
    stageTimelineCreate,
    stageTimelineUpdate,
    rejectReferencedTimelineDelete,
    selectUnreferencedTimelineTarget,
    stageUnreferencedTimelineDelete,
    rejectCharacterDeleteWithUnknownDependents,
    rejectCharacterUpdateWithUnknownDependents,
    selectCharacterWithUnknownDependent,
    selectCharacterWithoutDependents,
    stageCharacterCreate,
    stageCharacterUpdateWithManagedRewrite,
    stageCharacterGroupUpdateWithManagedRewrite,
    stageCharacterVoiceTypeUpdateWithManagedRewrite,
    stageCharacterVoiceParametersUpdateWithManagedRewrite,
    stageCharacterPortraitConfigurationUpdateWithManagedRewrite,
    stageCharacterDeleteWithManagedRewrite,
    stageCharacterDeleteWithoutDependents,
    rejectUnavailableTemplate,
    rejectTemplateDefinitionMutation,
    stageTemplateInstantiation,
    approve,
    changeSourceAfterApproval,
    changeSurfaceAfterApproval,
    changeCapabilityAfterApproval,
    changeDescriptorAfterApproval,
    changeCharacterDependentsAfterApproval,
    changeTemplateAfterApproval,
    markConflict,
    commitApproved,
    finishNormalizedNoOp,
    stutterTerminal,
  }

  val projectApprovalIsExact =
    not(approvalMustRemainBound(state.status))
      or state.approvedPlan == state.plan

  val unsupportedProjectEditNeverStages =
    state.status != ProjectRejected
      or all {
        state.plan == emptyPlan,
        state.projectMutationCount == 0,
        state.canonicalCommitCount == 0,
      }

  val projectEditIsSourceAndSurfaceBound =
    state.status != ProjectCommitted
      or all {
        state.committedPlan == state.approvedPlan,
        state.committedPlan.sourceFingerprint == state.sourceFingerprint,
        state.committedPlan.editSurfaceRevision == state.editSurfaceRevision,
        state.committedPlan.capabilityRevision == state.capabilityRevision,
        state.committedPlan.descriptorCatalogDigest
          == state.descriptorCatalogRevision,
      }

  val characterDependentsArePreservedOrRejected =
    state.status != ProjectCommitted
      or not(any {
        state.committedPlan.kind == CharacterUpdate,
        state.committedPlan.kind == CharacterDelete,
      })
      or all {
        state.committedPlan.dependentCount == 0
          or state.committedPlan.dependentPolicy == RewriteManagedDependents,
        state.preservedDependentItems == state.committedPlan.dependentCount,
        state.committedPlan.kind != CharacterUpdate
          or all {
            state.characterDependentCount == state.committedPlan.dependentCount,
            state.managedCharacterDependentCount == state.characterDependentCount,
          },
        state.committedPlan.kind != CharacterDelete
          or all {
            state.characterDependentCount == 0,
            state.managedCharacterDependentCount == 0,
          },
      }

  val templateExpansionIsApprovalBound =
    state.status != ProjectCommitted
      or state.committedPlan.kind != NativeTemplateInstantiate
      or all {
        state.committedPlan.templateContentDigest == state.templateContentDigest,
        state.committedPlan.expansionDigest == state.templateExpansionDigest,
        state.templateDescriptorAvailable,
        state.templateProducedKindsSupported,
        state.instantiatedTemplateItems == state.committedPlan.predictedChangedEntities,
      }

  val sceneDeleteNeverLeavesDanglingReferences =
    state.status != ProjectCommitted
      or state.committedPlan.kind != SceneDelete
      or all {
        state.referencedSceneCount == 0,
        state.sceneCount >= 1,
      }

  val timelineDeleteNeverLeavesDanglingReferences =
    state.status != ProjectCommitted
      or state.committedPlan.kind != TimelineDelete
      or all {
        state.referencedTimelineCount == 0,
        state.timelineCount >= 1,
      }

  val projectSettingTaxonomyIsSealed =
    state.plan.kind == NoProjectEdit
      or if (state.plan.kind == ProjectSettingsEdit)
        state.plan.settingClass != NoProjectSetting
      else state.plan.settingClass == NoProjectSetting

  val characterSettingTaxonomyIsSealed =
    state.plan.kind == NoProjectEdit
      or if (state.plan.kind == CharacterUpdate)
        all {
          state.plan.characterSettingClass != NoCharacterSetting,
          state.plan.characterNamePreimageDigest != missing,
          state.plan.characterGroupPreimageDigest != missing,
          state.plan.characterVoiceTypePreimageDigest != missing,
          state.plan.characterVoiceParametersPreimageDigest != missing,
          state.plan.characterPortraitConfigurationPreimageDigest != missing,
          state.plan.unknownCharacterConfigurationPreimageDigest != missing,
        }
      else all {
        state.plan.characterSettingClass == NoCharacterSetting,
        state.plan.characterNamePreimageDigest == missing,
        state.plan.characterGroupPreimageDigest == missing,
        state.plan.characterVoiceTypePreimageDigest == missing,
        state.plan.characterVoiceParametersPreimageDigest == missing,
        state.plan.characterPortraitConfigurationPreimageDigest == missing,
        state.plan.unknownCharacterConfigurationPreimageDigest == missing,
      }

  val partialCharacterUpdatePreservesConfiguration =
    state.status != ProjectCommitted
      or state.committedPlan.kind != CharacterUpdate
      or all {
        state.characterNameDigest
          == if (state.committedPlan.characterSettingClass == CharacterName)
            state.committedPlan.characterNamePreimageDigest + 1
          else state.committedPlan.characterNamePreimageDigest,
        state.characterGroupDigest
          == if (state.committedPlan.characterSettingClass == CharacterGroup)
            state.committedPlan.characterGroupPreimageDigest + 1
          else state.committedPlan.characterGroupPreimageDigest,
        state.characterVoiceTypeDigest
          == if (state.committedPlan.characterSettingClass == CharacterVoiceType)
            state.committedPlan.characterVoiceTypePreimageDigest + 1
          else state.committedPlan.characterVoiceTypePreimageDigest,
        state.characterVoiceParametersDigest
          == if (
            state.committedPlan.characterSettingClass == CharacterVoiceParameters)
            state.committedPlan.characterVoiceParametersPreimageDigest + 1
          else state.committedPlan.characterVoiceParametersPreimageDigest,
        state.characterPortraitConfigurationDigest
          == if (
            state.committedPlan.characterSettingClass
              == CharacterPortraitConfiguration)
            state.committedPlan.characterPortraitConfigurationPreimageDigest + 1
          else state.committedPlan.characterPortraitConfigurationPreimageDigest,
        state.unknownCharacterConfigurationDigest
          == state.committedPlan.unknownCharacterConfigurationPreimageDigest,
      }

  val normalizationIsCanonical =
    state.plan.kind == NoProjectEdit
      or state.plan.normalizedNoOp
        == (state.plan.predictedChangedEntities == 0)

  val normalizedNoOpDoesNotAdvanceRevision =
    state.status != ProjectNoOp
      or all {
        state.projectMutationCount == 0,
        state.canonicalCommitCount == 0,
        state.headRevision == initialHeadRevision,
        state.derivedEpoch == 0,
      }

  val actualProjectEditInvalidatesDerivedTasks =
    state.projectMutationCount == 0
      or all {
        state.derivedEpoch > state.checkpointEpoch,
        state.derivedEpoch > state.inspectionEpoch,
        state.derivedEpoch > state.renderEpoch,
        state.derivedEpoch > state.compositionEpoch,
        state.derivedEpoch > state.timelinePlanEpoch,
      }

  val resourceCardinalityIsSafe = all {
    state.sceneCount >= 1,
    state.timelineCount >= 1,
    state.characterCount >= 1,
    state.referencedSceneCount >= 0,
    state.referencedTimelineCount >= 0,
    state.characterDependentCount >= 0,
    state.managedCharacterDependentCount >= 0,
    state.managedCharacterDependentCount <= state.characterDependentCount,
  }

  val oneProjectEditCreatesOneRevision = all {
    state.projectMutationCount == state.canonicalCommitCount,
    state.canonicalCommitCount <= 1,
    state.headRevision
      == initialHeadRevision + state.canonicalCommitCount + state.externalCommitCount,
  }

  val projectObservationDoesNotMutate =
    all {
      state.observationCount >= 0,
      state.projectMutationCount == state.canonicalCommitCount,
      state.headRevision
        == initialHeadRevision + state.canonicalCommitCount + state.externalCommitCount,
    }

  val projectEditSafety = and {
    projectApprovalIsExact,
    unsupportedProjectEditNeverStages,
    projectEditIsSourceAndSurfaceBound,
    characterDependentsArePreservedOrRejected,
    templateExpansionIsApprovalBound,
    sceneDeleteNeverLeavesDanglingReferences,
    timelineDeleteNeverLeavesDanglingReferences,
    projectSettingTaxonomyIsSealed,
    characterSettingTaxonomyIsSealed,
    partialCharacterUpdatePreservesConfiguration,
    normalizationIsCanonical,
    normalizedNoOpDoesNotAdvanceRevision,
    actualProjectEditInvalidatesDerivedTasks,
    resourceCardinalityIsSafe,
    oneProjectEditCreatesOneRevision,
    projectObservationDoesNotMutate,
  }

  run unsupportedSettingsRequestFailsClosedTest = init
    .then(rejectUnsupportedSettingsEdit)
    .expect(all {
      projectEditSafety,
      state.status == ProjectRejected,
      state.lastRejectedKind == ProjectSettingsEdit,
      state.projectMutationCount == 0,
    })

  run unsupportedTimelineCrudFailsClosedTest = init
    .then(rejectUnsupportedTimelineEdit)
    .expect(all {
      projectEditSafety,
      state.status == ProjectRejected,
      state.lastRejectedKind == TimelineUpdate,
      state.projectMutationCount == 0,
    })

  run settingsEditInvalidatesDerivedTasksTest = init
    .then(enableSettingsMutation)
    .then(stageSettingsEdit)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.status == ProjectCommitted,
      state.settingsRevision == 6,
      state.derivedEpoch == 1,
      state.headRevision == initialHeadRevision + 1,
    })

  run normalizedSettingsNoOpCreatesNoRevisionTest = init
    .then(enableSettingsMutation)
    .then(stageNormalizedSettingsNoOp)
    .then(approve)
    .then(finishNormalizedNoOp)
    .expect(all {
      projectEditSafety,
      state.status == ProjectNoOp,
      state.projectMutationCount == 0,
      state.headRevision == initialHeadRevision,
    })

  run referencedSceneDeleteIsRejectedTest = init
    .then(enableSceneMutation)
    .then(rejectReferencedSceneDelete)
    .expect(all {
      projectEditSafety,
      state.status == ProjectRejected,
      state.lastRejectedKind == SceneDelete,
      state.sceneCount == 2,
    })

  run managedCharacterDependentsAreRewrittenTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterUpdateWithManagedRewrite)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.status == ProjectCommitted,
      state.committedPlan.kind == CharacterUpdate,
      state.committedPlan.characterSettingClass == CharacterName,
      state.preservedDependentItems == 2,
      state.characterCount == 2,
      state.characterNameDigest == 102,
      state.characterGroupDigest == 102,
      state.characterVoiceTypeDigest == 103,
      state.characterVoiceParametersDigest == 104,
      state.characterPortraitConfigurationDigest == 105,
      state.unknownCharacterConfigurationDigest == 199,
    })

  run characterGroupUpdatePreservesOtherConfigurationTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterGroupUpdateWithManagedRewrite)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.characterSettingClass == CharacterGroup,
      state.characterNameDigest == 101,
      state.characterGroupDigest == 103,
      state.characterVoiceTypeDigest == 103,
      state.characterVoiceParametersDigest == 104,
      state.characterPortraitConfigurationDigest == 105,
      state.unknownCharacterConfigurationDigest == 199,
    })

  run characterVoiceTypeUpdatePreservesOtherConfigurationTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterVoiceTypeUpdateWithManagedRewrite)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.characterSettingClass == CharacterVoiceType,
      state.characterNameDigest == 101,
      state.characterGroupDigest == 102,
      state.characterVoiceTypeDigest == 104,
      state.characterVoiceParametersDigest == 104,
      state.characterPortraitConfigurationDigest == 105,
      state.unknownCharacterConfigurationDigest == 199,
    })

  run characterVoiceParametersUpdatePreservesOtherConfigurationTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterVoiceParametersUpdateWithManagedRewrite)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.characterSettingClass == CharacterVoiceParameters,
      state.characterNameDigest == 101,
      state.characterGroupDigest == 102,
      state.characterVoiceTypeDigest == 103,
      state.characterVoiceParametersDigest == 105,
      state.characterPortraitConfigurationDigest == 105,
      state.unknownCharacterConfigurationDigest == 199,
    })

  run characterPortraitUpdatePreservesOtherConfigurationTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterPortraitConfigurationUpdateWithManagedRewrite)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.characterSettingClass
        == CharacterPortraitConfiguration,
      state.characterNameDigest == 101,
      state.characterGroupDigest == 102,
      state.characterVoiceTypeDigest == 103,
      state.characterVoiceParametersDigest == 104,
      state.characterPortraitConfigurationDigest == 106,
      state.unknownCharacterConfigurationDigest == 199,
    })

  run templateDescriptorDriftConflictsTest = init
    .then(enableTemplateMutation)
    .then(stageTemplateInstantiation)
    .then(approve)
    .then(changeTemplateAfterApproval)
    .then(markConflict)
    .expect(all {
      projectEditSafety,
      state.status == ProjectConflicted,
      state.instantiatedTemplateItems == 0,
      state.canonicalCommitCount == 0,
    })

  run exactTemplateExpansionCommitsTest = init
    .then(enableTemplateMutation)
    .then(stageTemplateInstantiation)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.status == ProjectCommitted,
      state.instantiatedTemplateItems == 2,
      state.committedPlan.expansionDigest == 82,
      state.canonicalCommitCount == 1,
    })

  run templateDefinitionsRemainOpaqueTest = init
    .then(enableTemplateMutation)
    .then(rejectTemplateDefinitionMutation)
    .expect(all {
      projectEditSafety,
      state.status == ProjectRejected,
      state.lastRejectedKind == NativeTemplateDefinitionEdit,
      state.instantiatedTemplateItems == 0,
    })

  run editSurfaceChangeInvalidatesApprovalTest = init
    .then(enableSettingsMutation)
    .then(stageSettingsEdit)
    .then(approve)
    .then(changeSurfaceAfterApproval)
    .then(markConflict)
    .expect(all {
      projectEditSafety,
      state.status == ProjectConflicted,
      state.projectMutationCount == 0,
      state.canonicalCommitCount == 0,
    })

  run projectObservationIsReadOnlyTest = init
    .then(observeProject)
    .then(observeProject)
    .expect(all {
      projectEditSafety,
      state.observationCount == 2,
      state.projectMutationCount == 0,
      state.headRevision == initialHeadRevision,
    })

  run frameRateSettingIsSealedTest = init
    .then(enableSettingsMutation)
    .then(stageFrameRateEdit)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.settingClass == FrameRate,
      state.settingsRevision == 6,
    })

  run audioSampleRateSettingIsSealedTest = init
    .then(enableSettingsMutation)
    .then(stageAudioSampleRateEdit)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.settingClass == AudioSampleRate,
      state.settingsRevision == 6,
    })

  run backgroundColorSettingIsSealedTest = init
    .then(enableSettingsMutation)
    .then(stageBackgroundColorEdit)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.settingClass == BackgroundColor,
      state.settingsRevision == 6,
    })

  run timelineLayerPolicySettingIsSealedTest = init
    .then(enableSettingsMutation)
    .then(stageTimelineLayerPolicyEdit)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.settingClass == TimelineLayerPolicy,
      state.settingsRevision == 6,
    })

  run sceneCreateCommitsTest = init
    .then(enableSceneMutation)
    .then(stageSceneCreate)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == SceneCreate,
      state.sceneCount == 3,
    })

  run sceneUpdateCommitsTest = init
    .then(enableSceneMutation)
    .then(stageSceneUpdate)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == SceneUpdate,
      state.sceneCount == 2,
    })

  run unreferencedSceneDeleteCommitsTest = init
    .then(enableSceneMutation)
    .then(selectUnreferencedSceneTarget)
    .then(stageUnreferencedSceneDelete)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == SceneDelete,
      state.sceneCount == 1,
      state.referencedSceneCount == 0,
    })

  run timelineCreateCommitsTest = init
    .then(enableTimelineMutation)
    .then(stageTimelineCreate)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == TimelineCreate,
      state.timelineCount == 3,
    })

  run timelineUpdateCommitsTest = init
    .then(enableTimelineMutation)
    .then(stageTimelineUpdate)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == TimelineUpdate,
      state.timelineCount == 2,
    })

  run referencedTimelineDeleteIsRejectedTest = init
    .then(enableTimelineMutation)
    .then(rejectReferencedTimelineDelete)
    .expect(all {
      projectEditSafety,
      state.status == ProjectRejected,
      state.lastRejectedKind == TimelineDelete,
      state.timelineCount == 2,
    })

  run unreferencedTimelineDeleteCommitsTest = init
    .then(enableTimelineMutation)
    .then(selectUnreferencedTimelineTarget)
    .then(stageUnreferencedTimelineDelete)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == TimelineDelete,
      state.timelineCount == 1,
      state.referencedTimelineCount == 0,
    })

  run characterCreateCommitsTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterCreate)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == CharacterCreate,
      state.committedPlan.characterSettingClass == NoCharacterSetting,
      state.characterCount == 3,
      state.preservedDependentItems == 0,
    })

  run characterDeleteRewritesManagedDependentsTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterDeleteWithManagedRewrite)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == CharacterDelete,
      state.committedPlan.characterSettingClass == NoCharacterSetting,
      state.committedPlan.dependentCount == 2,
      state.characterCount == 1,
      state.characterDependentCount == 0,
      state.preservedDependentItems == 2,
    })

  run characterDeleteWithoutDependentsCommitsTest = init
    .then(enableCharacterMutation)
    .then(selectCharacterWithoutDependents)
    .then(stageCharacterDeleteWithoutDependents)
    .then(approve)
    .then(commitApproved)
    .expect(all {
      projectEditSafety,
      state.committedPlan.kind == CharacterDelete,
      state.committedPlan.characterSettingClass == NoCharacterSetting,
      state.committedPlan.dependentCount == 0,
      state.characterCount == 1,
      state.preservedDependentItems == 0,
    })

  run unknownCharacterDependentRejectsUpdateTest = init
    .then(enableCharacterMutation)
    .then(selectCharacterWithUnknownDependent)
    .then(rejectCharacterUpdateWithUnknownDependents)
    .expect(all {
      projectEditSafety,
      state.status == ProjectRejected,
      state.lastRejectedKind == CharacterUpdate,
      state.characterCount == 2,
    })

  run unknownCharacterDependentRejectsDeleteTest = init
    .then(enableCharacterMutation)
    .then(selectCharacterWithUnknownDependent)
    .then(rejectCharacterDeleteWithUnknownDependents)
    .expect(all {
      projectEditSafety,
      state.status == ProjectRejected,
      state.lastRejectedKind == CharacterDelete,
      state.characterCount == 2,
      state.preservedDependentItems == 0,
    })

  run sourceDriftInvalidatesApprovalTest = init
    .then(enableSettingsMutation)
    .then(stageSettingsEdit)
    .then(approve)
    .then(changeSourceAfterApproval)
    .then(markConflict)
    .expect(all {
      projectEditSafety,
      state.status == ProjectConflicted,
      state.externalCommitCount == 1,
      state.canonicalCommitCount == 0,
    })

  run capabilityDriftInvalidatesApprovalTest = init
    .then(enableSettingsMutation)
    .then(stageSettingsEdit)
    .then(approve)
    .then(changeCapabilityAfterApproval)
    .then(markConflict)
    .expect(all {
      projectEditSafety,
      state.status == ProjectConflicted,
      state.canonicalCommitCount == 0,
    })

  run descriptorDriftInvalidatesApprovalTest = init
    .then(enableSettingsMutation)
    .then(stageSettingsEdit)
    .then(approve)
    .then(changeDescriptorAfterApproval)
    .then(markConflict)
    .expect(all {
      projectEditSafety,
      state.status == ProjectConflicted,
      state.canonicalCommitCount == 0,
    })

  run characterDependentDriftInvalidatesApprovalTest = init
    .then(enableCharacterMutation)
    .then(stageCharacterUpdateWithManagedRewrite)
    .then(approve)
    .then(changeCharacterDependentsAfterApproval)
    .then(markConflict)
    .expect(all {
      projectEditSafety,
      state.status == ProjectConflicted,
      state.canonicalCommitCount == 0,
      state.preservedDependentItems == 0,
    })
}
