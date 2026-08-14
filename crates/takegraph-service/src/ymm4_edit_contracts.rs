//! YMM4 admission for the Quint edit-contract families.
//!
//! These helpers run the portable core only after the live structured
//! capability catalog advertises the matching feature. Current YMM4 4.55.1.1
//! does not advertise them, so every public path fail-closes.

use takegraph_core::{
    CharacterSettingClass, CompositionError, CompositionGraph, CompositionIntent,
    CompositionOutcome, CrudKind, DependentPolicy, EditSurfaceError, EditSurfaceSession,
    EditTransactionError, EditTransactionPlan, EditTransactionRequest, EntityId, ItemKind,
    ProjectEditCapability, ProjectEditError, ProjectEditKind, ProjectEditPlan, ProjectEditSession,
    ProjectSettingClass,
};
use takegraph_node::StructuredYmm4Capabilities;
use thiserror::Error;

/// Feature names sealed into the YMM4 structured capability catalog.
pub const EDIT_SURFACE_FEATURE: &str = "editSurface.admit";
pub const COMPOSITION_GRAPH_FEATURE: &str = "compositionGraph.apply";
pub const PROJECT_SETTINGS_FEATURE: &str = "projectEdit.settings";
pub const PROJECT_SCENE_FEATURE: &str = "projectEdit.scene";
pub const PROJECT_TIMELINE_FEATURE: &str = "projectEdit.timeline";
pub const PROJECT_CHARACTER_FEATURE: &str = "projectEdit.character";
pub const PROJECT_TEMPLATE_DEFINITION_FEATURE: &str = "projectEdit.templateDefinition";
pub const EDIT_TRANSACTION_FEATURE: &str = "editTransaction.apply";

/// Fail-closed YMM4 edit-contract errors.
#[derive(Debug, Error)]
pub enum Ymm4EditContractError {
    #[error("YMM4 does not advertise {0}")]
    Unsupported(&'static str),
    #[error(transparent)]
    EditSurface(#[from] EditSurfaceError),
    #[error(transparent)]
    Composition(#[from] CompositionError),
    #[error(transparent)]
    ProjectEdit(#[from] ProjectEditError),
    #[error(transparent)]
    Transaction(#[from] EditTransactionError),
}

/// Requires an advertised feature before any portable mutation is attempted.
///
/// # Errors
///
/// Returns [`Ymm4EditContractError::Unsupported`] when the feature is missing
/// or `available: false`.
pub fn require_advertised_edit_feature(
    capabilities: &StructuredYmm4Capabilities,
    feature: &'static str,
) -> Result<(), Ymm4EditContractError> {
    if capabilities
        .feature(feature)
        .is_some_and(|descriptor| descriptor.available)
    {
        Ok(())
    } else {
        Err(Ymm4EditContractError::Unsupported(feature))
    }
}

/// Admits a normalized, observed field edit only when `editSurface.admit` is on.
///
/// # Errors
///
/// Returns [`Ymm4EditContractError::Unsupported`] without touching the session
/// when the bridge does not advertise the feature.
pub fn admit_field_edit(
    capabilities: &StructuredYmm4Capabilities,
    session: &mut EditSurfaceSession,
) -> Result<(), Ymm4EditContractError> {
    require_advertised_edit_feature(capabilities, EDIT_SURFACE_FEATURE)?;
    session.admit()?;
    Ok(())
}

/// Stages and applies one composition-graph intent when advertised.
///
/// # Errors
///
/// Returns [`Ymm4EditContractError::Unsupported`] before `stage` when the
/// bridge does not advertise `compositionGraph.apply`.
pub fn apply_composition_intent(
    capabilities: &StructuredYmm4Capabilities,
    graph: &mut CompositionGraph,
    intent: CompositionIntent,
    target: EntityId,
    kind: ItemKind,
) -> Result<CompositionOutcome, Ymm4EditContractError> {
    require_advertised_edit_feature(capabilities, COMPOSITION_GRAPH_FEATURE)?;
    graph.stage(intent, target, kind)?;
    Ok(graph.apply_staged()?)
}

/// Feature that must be advertised for `kind` to leave the unavailable default.
#[must_use]
pub fn project_edit_feature(kind: ProjectEditKind) -> &'static str {
    match kind {
        ProjectEditKind::ProjectSettingsEdit => PROJECT_SETTINGS_FEATURE,
        ProjectEditKind::SceneCreate
        | ProjectEditKind::SceneUpdate
        | ProjectEditKind::SceneDelete => PROJECT_SCENE_FEATURE,
        ProjectEditKind::TimelineCreate
        | ProjectEditKind::TimelineUpdate
        | ProjectEditKind::TimelineDelete => PROJECT_TIMELINE_FEATURE,
        ProjectEditKind::CharacterCreate
        | ProjectEditKind::CharacterUpdate
        | ProjectEditKind::CharacterDelete => PROJECT_CHARACTER_FEATURE,
        ProjectEditKind::NativeTemplateDefinitionEdit => PROJECT_TEMPLATE_DEFINITION_FEATURE,
        ProjectEditKind::NativeTemplateInstantiate | ProjectEditKind::NoProjectEdit => {
            PROJECT_TEMPLATE_DEFINITION_FEATURE
        }
    }
}

fn enable_if_advertised(
    capabilities: &StructuredYmm4Capabilities,
    session: &mut ProjectEditSession,
    feature: &'static str,
    capability: ProjectEditCapability,
) -> Result<(), Ymm4EditContractError> {
    if capabilities
        .feature(feature)
        .is_some_and(|descriptor| descriptor.available)
    {
        session.enable(capability)?;
    }
    Ok(())
}

/// Portable project-edit stage request after capability gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectEditStageRequest {
    pub kind: ProjectEditKind,
    pub setting_class: ProjectSettingClass,
    pub character_setting_class: CharacterSettingClass,
    pub policy: DependentPolicy,
    pub changed_entities: i64,
    pub normalized_no_op: bool,
}

/// Stages a project-edit family after enabling only advertised capabilities.
///
/// Unadvertised families stay off, so [`ProjectEditSession::stage`] fail-closes
/// the same way the Quint model starts every family unavailable.
///
/// # Errors
///
/// Returns the portable project-edit reject, or
/// [`Ymm4EditContractError::Unsupported`] for template-definition edits.
pub fn stage_project_edit(
    capabilities: &StructuredYmm4Capabilities,
    session: &mut ProjectEditSession,
    request: ProjectEditStageRequest,
) -> Result<ProjectEditPlan, Ymm4EditContractError> {
    if request.kind == ProjectEditKind::NativeTemplateDefinitionEdit {
        require_advertised_edit_feature(capabilities, PROJECT_TEMPLATE_DEFINITION_FEATURE)?;
    }
    enable_if_advertised(
        capabilities,
        session,
        PROJECT_SETTINGS_FEATURE,
        ProjectEditCapability::SettingsMutation,
    )?;
    enable_if_advertised(
        capabilities,
        session,
        PROJECT_SCENE_FEATURE,
        ProjectEditCapability::SceneMutation,
    )?;
    enable_if_advertised(
        capabilities,
        session,
        PROJECT_TIMELINE_FEATURE,
        ProjectEditCapability::TimelineMutation,
    )?;
    enable_if_advertised(
        capabilities,
        session,
        PROJECT_CHARACTER_FEATURE,
        ProjectEditCapability::CharacterMutation,
    )?;
    enable_if_advertised(
        capabilities,
        session,
        PROJECT_TEMPLATE_DEFINITION_FEATURE,
        ProjectEditCapability::TemplateMutation,
    )?;
    Ok(session.stage(
        request.kind,
        request.setting_class,
        request.character_setting_class,
        request.policy,
        request.changed_entities,
        request.normalized_no_op,
    )?)
}

/// Seals a heterogeneous transaction only on the unified advertised endpoint.
///
/// # Errors
///
/// Returns [`Ymm4EditContractError::Unsupported`] before seal when the feature
/// is off.
pub fn seal_edit_transaction(
    capabilities: &StructuredYmm4Capabilities,
    request: EditTransactionRequest,
) -> Result<EditTransactionPlan, Ymm4EditContractError> {
    require_advertised_edit_feature(capabilities, EDIT_TRANSACTION_FEATURE)?;
    Ok(EditTransactionPlan::seal(request)?)
}

/// Helper for tests: whether a create/update/delete field edit is a mutating crud.
#[must_use]
pub fn field_edit_mutates(session: &EditSurfaceSession) -> bool {
    !matches!(session.task.crud, CrudKind::Update)
        || !matches!(
            session.normalized_patch,
            takegraph_core::NormalizedPatch::FieldUntouched
                | takegraph_core::NormalizedPatch::NormalizedNoOp
                | takegraph_core::NormalizedPatch::NotNormalized
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        CompositionIntent, EndpointMode, FieldClass, INITIAL_FOCUS_ID, ItemFamily,
        representative_operations,
    };
    use takegraph_node::{
        StructuredYmm4Capabilities, Ymm4Capabilities, Ymm4Capability, Ymm4Health,
    };

    fn health() -> Ymm4Health {
        Ymm4Health {
            status: "running".into(),
            protocol_version: 2,
            plugin_version: "0.2.0".into(),
            ymm4_version: "4.55.1.1".into(),
        }
    }

    fn structured(tokens: Vec<Ymm4Capability>) -> StructuredYmm4Capabilities {
        StructuredYmm4Capabilities::from_bridge(
            &health(),
            &Ymm4Capabilities {
                protocol_version: 2,
                capabilities: tokens,
            },
        )
        .expect("structured")
    }

    fn current_bridge() -> StructuredYmm4Capabilities {
        structured(vec![
            Ymm4Capability::MutationProfileYmm4_4_55_1_1,
            Ymm4Capability::UnifiedTargetPlan,
            Ymm4Capability::TimelineEditManagedCueMixed,
            Ymm4Capability::NativeTemplateInstantiate,
        ])
    }

    #[test]
    fn current_ymm4_driver_rejects_every_new_edit_family_before_portable_apply() {
        let capabilities = current_bridge();
        let mut surface = EditSurfaceSession::initial();
        surface.normalize().expect("normalize");
        surface.observe_read_only().expect("observe");
        let err = admit_field_edit(&capabilities, &mut surface).expect_err("surface");
        assert!(matches!(
            err,
            Ymm4EditContractError::Unsupported(EDIT_SURFACE_FEATURE)
        ));
        assert_ne!(surface.status, takegraph_core::EditSurfaceStatus::Admitted);

        let mut graph = CompositionGraph::representative();
        let err = apply_composition_intent(
            &capabilities,
            &mut graph,
            CompositionIntent::UpdateGeometry {
                start_frame: 2,
                layer: 2,
                duration: 12,
            },
            INITIAL_FOCUS_ID,
            ItemKind::TextItem,
        )
        .expect_err("composition");
        assert!(matches!(
            err,
            Ymm4EditContractError::Unsupported(COMPOSITION_GRAPH_FEATURE)
        ));
        assert_eq!(graph.revision, takegraph_core::INITIAL_REVISION);
        assert!(graph.focus().is_some());

        let mut project = ProjectEditSession::initial();
        let err = stage_project_edit(
            &capabilities,
            &mut project,
            ProjectEditStageRequest {
                kind: ProjectEditKind::ProjectSettingsEdit,
                setting_class: ProjectSettingClass::CanvasSize,
                character_setting_class: CharacterSettingClass::NoCharacterSetting,
                policy: DependentPolicy::NoDependentPolicy,
                changed_entities: 1,
                normalized_no_op: false,
            },
        )
        .expect_err("settings");
        assert!(matches!(
            err,
            Ymm4EditContractError::ProjectEdit(ProjectEditError::UnsupportedFamily(
                ProjectEditKind::ProjectSettingsEdit
            ))
        ));
        let err = stage_project_edit(
            &capabilities,
            &mut project,
            ProjectEditStageRequest {
                kind: ProjectEditKind::NativeTemplateDefinitionEdit,
                setting_class: ProjectSettingClass::NoProjectSetting,
                character_setting_class: CharacterSettingClass::NoCharacterSetting,
                policy: DependentPolicy::NoDependentPolicy,
                changed_entities: 1,
                normalized_no_op: false,
            },
        )
        .expect_err("template definition");
        assert!(matches!(
            err,
            Ymm4EditContractError::Unsupported(PROJECT_TEMPLATE_DEFINITION_FEATURE)
        ));

        let err = seal_edit_transaction(
            &capabilities,
            EditTransactionRequest {
                task_id: "t".into(),
                project_id: "p".into(),
                scene_id: "s".into(),
                base_revision: 1,
                source_fingerprint: "fp".into(),
                capability_revision: 1,
                endpoint_mode: EndpointMode::UnifiedTransactionEndpoint,
                operations: representative_operations(),
            },
        )
        .expect_err("transaction");
        assert!(matches!(
            err,
            Ymm4EditContractError::Unsupported(EDIT_TRANSACTION_FEATURE)
        ));
    }

    #[test]
    fn advertised_features_reach_portable_admission_and_unknown_kind_still_fails() {
        let capabilities = structured(vec![
            Ymm4Capability::EditSurfaceAdmit,
            Ymm4Capability::CompositionGraphApply,
            Ymm4Capability::ProjectSettingsMutation,
            Ymm4Capability::EditTransactionApply,
        ]);

        let mut surface = EditSurfaceSession::initial();
        surface.normalize().expect("normalize");
        surface.observe_read_only().expect("observe");
        admit_field_edit(&capabilities, &mut surface).expect("admit");
        assert_eq!(surface.status, takegraph_core::EditSurfaceStatus::Admitted);
        assert_eq!(surface.task.family, ItemFamily::ManagedVoiceItem);
        assert_eq!(surface.task.field_class, FieldClass::AudioParametersField);

        let mut graph = CompositionGraph::representative();
        apply_composition_intent(
            &capabilities,
            &mut graph,
            CompositionIntent::UpdateGeometry {
                start_frame: 2,
                layer: 2,
                duration: 12,
            },
            INITIAL_FOCUS_ID,
            ItemKind::TextItem,
        )
        .expect("geometry");
        assert_eq!(graph.focus().expect("focus").start_frame, 2);
        let err = apply_composition_intent(
            &capabilities,
            &mut graph,
            CompositionIntent::CreateEntity {
                kind: ItemKind::UnknownItem,
                start_frame: 0,
                layer: 0,
                duration: 10,
            },
            99,
            ItemKind::UnknownItem,
        )
        .expect_err("unknown");
        assert!(matches!(
            err,
            Ymm4EditContractError::Composition(CompositionError::UnknownKind)
        ));

        let mut project = ProjectEditSession::initial();
        let plan = stage_project_edit(
            &capabilities,
            &mut project,
            ProjectEditStageRequest {
                kind: ProjectEditKind::ProjectSettingsEdit,
                setting_class: ProjectSettingClass::FrameRate,
                character_setting_class: CharacterSettingClass::NoCharacterSetting,
                policy: DependentPolicy::NoDependentPolicy,
                changed_entities: 1,
                normalized_no_op: false,
            },
        )
        .expect("settings advertised");
        assert_eq!(plan.kind, ProjectEditKind::ProjectSettingsEdit);
        assert_eq!(plan.setting_class, ProjectSettingClass::FrameRate);

        let sealed = seal_edit_transaction(
            &capabilities,
            EditTransactionRequest {
                task_id: "task-main".into(),
                project_id: "project".into(),
                scene_id: "scene".into(),
                base_revision: 10,
                source_fingerprint: "sha256:source".into(),
                capability_revision: 7,
                endpoint_mode: EndpointMode::UnifiedTransactionEndpoint,
                operations: representative_operations(),
            },
        )
        .expect("seal");
        assert_eq!(sealed.operations.len(), 3);
        assert!(!sealed.plan_digest.is_empty());
    }
}
