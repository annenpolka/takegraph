//! Canonical project service façade.
//!
//! In-memory staging remains available for the portable core guard. External
//! editor workflows use the append-only, hash-chained [`DurableProjectStore`]
//! and source-bound task journals before advancing canonical state.

use takegraph_core::{Patch, PatchError, PatchId, RevisionId};

pub mod annotation_derive;
pub mod annotation_store;
mod external_mutation;
pub mod interpretation;
mod managed_projection;
pub mod native_extension_plan;
pub mod project_operations;
pub mod project_store;
pub mod promotion;
pub mod scene_inspection;
pub mod transcription_config;
pub mod transcription_jobs;
pub mod ymm4_edit_contracts;
pub mod ymm4_export;
pub mod ymm4_native_extension;
pub mod ymm4_native_voice_export;
pub mod ymm4_native_voice_mutation;
pub mod ymm4_realization;
pub mod ymm4_target_plan;
pub mod ymm4_timeline_edit;

pub use annotation_derive::{
    AnnotationDeriveError, AnnotationDeriveMode, AnnotationDerivePhase, AnnotationDerivePlan,
    AnnotationDeriveReport, AnnotationDeriveStore, run_annotation_derive,
};
pub use external_mutation::{
    DurableExternalMutationOutcome, UnsavedProjectError, require_existing_project_path,
};
pub use interpretation::{InterpretCaptureError, attach_human_interpretation, interpret_capture};
pub use native_extension_plan::{
    ExistingNativeExtension, ExistingNativeExtensionKind, ExistingUpdateMode,
    NativeExtensionCapabilities, NativeExtensionFeature, NativeExtensionObservation,
    NativeExtensionPlanContext, NativeExtensionPlanError, plan_native_extensions,
    plan_native_extensions_with_identity_overrides,
};
pub use project_operations::{
    CheckpointTaskRecord, CheckpointTaskStatus, DurableTaskRecord, ProjectInitializationTaskRecord,
    ProjectInitializationTaskStatus, ProjectOperationError, ProjectOperationStore,
    ReconciliationBridgeWireStatus, ReconciliationChildKind, ReconciliationChildReference,
    ReconciliationChildTask, ReconciliationDetachContract, ReconciliationDetachDraft,
    ReconciliationDetachRequirement, ReconciliationDetachStatus, ReconciliationDownstreamPreview,
    ReconciliationExporterRoute, ReconciliationImportPatchDraft, ReconciliationReExportManifest,
    ReconciliationReExportStatus, ReconciliationReExportTask, ReconciliationTaskRecord,
    ReconciliationTaskStatus, RenderTaskRecord, RenderTaskStatus,
};
pub use project_store::{
    DurableProjectState, DurableProjectStore, ExternalCommitRecord, ExternalMutationFence,
    ManagedTargetState, PendingExternalCommit, PendingExternalCommitKind,
    ProjectInitializationReservation, ProjectInitializationReservationOutcome,
    ProjectInitializationReservationStatus, ProjectStoreError, TargetLink, VerifiedTargetBinding,
};
pub use promotion::{
    PromotionError, StagedNarrationPromotion, annotation_pin_entity_id, assert_pin_project,
    assert_promotion_target, commit_promotions_from_plan, find_pin_item,
    narration_promotion_operations, pin_promotion_operations, record_committed_promotion,
    record_staged_promotion, stage_narration_promotion, stage_pin_promotion, stage_unpin_promotion,
    unpin_promotion_operations,
};
pub use scene_inspection::{
    ChangedCueFrameRange, SceneCaptureEvidence, SceneCaptureProfile, SceneCaptureSamplePlan,
    SceneHumanReview, SceneInspectionError, SceneInspectionPlan, SceneInspectionReceipt,
    SceneInspectionSource, SceneInspectionStatus, SceneInspectionTask, SceneReviewDecision,
    sample_changed_cue_frames,
};
pub use transcription_config::{
    TranscriptionConfigError, TranscriptionHostConfig, resolve_transcription_host,
    whisper_host_configured,
};
pub use transcription_jobs::{
    TranscriptionJob, TranscriptionJobError, TranscriptionJobStatus, TranscriptionJobStore,
    attach_human_transcript, transcribe_capture,
};
pub use ymm4_edit_contracts::{
    COMPOSITION_GRAPH_FEATURE, EDIT_SURFACE_FEATURE, EDIT_TRANSACTION_FEATURE,
    PROJECT_CHARACTER_FEATURE, PROJECT_SCENE_FEATURE, PROJECT_SETTINGS_FEATURE,
    PROJECT_TEMPLATE_DEFINITION_FEATURE, PROJECT_TIMELINE_FEATURE, ProjectEditStageRequest,
    Ymm4EditContractError, admit_field_edit, apply_composition_intent, project_edit_feature,
    require_advertised_edit_feature, seal_edit_transaction, stage_project_edit,
};
pub use ymm4_export::{Ymm4ExportError, Ymm4ExportPatch};
pub use ymm4_native_extension::{
    NativeExtensionArtifactSource, NativeExtensionStageManifest, Ymm4NativeExtensionError,
    Ymm4NativeExtensionTask,
};
pub use ymm4_native_voice_export::{Ymm4NativeVoiceExportError, Ymm4NativeVoiceExportPatch};
pub use ymm4_native_voice_mutation::{Ymm4NativeVoiceMutationError, Ymm4NativeVoiceMutationPatch};
pub use ymm4_realization::{RealizationReadbackError, normalize_realizations};
pub use ymm4_target_plan::{
    TargetPlanBuildError, native_voice_target_plan, portable_pair_target_plan,
};
pub use ymm4_timeline_edit::{
    TimelineEditStageManifest, TimelineEditStageOperation, Ymm4TimelineEditError,
    Ymm4TimelineEditTask,
};

#[derive(Debug, Default)]
pub struct ProjectService {
    head: RevisionId,
    patches: Vec<Patch>,
}

impl ProjectService {
    /// Returns the current canonical project revision.
    #[must_use]
    pub fn head(&self) -> RevisionId {
        self.head
    }

    /// Stores a reviewable patch without applying it.
    pub fn stage(&mut self, patch: Patch) {
        self.patches.push(patch);
    }

    /// Commits a staged patch through the portable core guard.
    ///
    /// # Errors
    ///
    /// Returns an error if the patch does not exist or fails core validation.
    pub fn commit(&mut self, patch_id: PatchId) -> Result<RevisionId, ServiceError> {
        let patch = self
            .patches
            .iter_mut()
            .find(|patch| patch.id == patch_id)
            .ok_or(ServiceError::PatchNotFound(patch_id))?;
        let next = patch.commit(self.head)?;
        self.head = next;
        Ok(next)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("patch not found: {0:?}")]
    PatchNotFound(PatchId),
    #[error(transparent)]
    Patch(#[from] PatchError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_commits_through_core_guard() {
        let mut service = ProjectService::default();
        let mut patch = Patch::draft(service.head(), "digest-a");
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();
        patch.approve().unwrap();
        let id = patch.id;
        service.stage(patch);

        assert_eq!(service.commit(id).unwrap(), RevisionId(1));
    }
}
