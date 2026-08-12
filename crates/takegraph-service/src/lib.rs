//! Canonical project service façade.
//!
//! In-memory staging remains available for the portable core guard. External
//! editor workflows use the append-only, hash-chained [`DurableProjectStore`]
//! and source-bound task journals before advancing canonical state.

use takegraph_core::{Patch, PatchError, PatchId, RevisionId};

mod external_mutation;
mod managed_projection;
pub mod native_extension_plan;
pub mod project_operations;
pub mod project_store;
pub mod scene_inspection;
pub mod ymm4_export;
pub mod ymm4_native_extension;
pub mod ymm4_native_voice_export;
pub mod ymm4_native_voice_mutation;
pub mod ymm4_realization;
pub mod ymm4_target_plan;

pub use external_mutation::DurableExternalMutationOutcome;
pub use native_extension_plan::{
    ExistingNativeExtension, ExistingNativeExtensionKind, ExistingUpdateMode,
    NativeExtensionCapabilities, NativeExtensionFeature, NativeExtensionObservation,
    NativeExtensionPlanContext, NativeExtensionPlanError, plan_native_extensions,
    plan_native_extensions_with_identity_overrides,
};
pub use project_operations::{
    CheckpointTaskRecord, CheckpointTaskStatus, DurableTaskRecord, ProjectOperationError,
    ProjectOperationStore, ReconciliationBridgeWireStatus, ReconciliationChildKind,
    ReconciliationChildReference, ReconciliationChildTask, ReconciliationDetachContract,
    ReconciliationDetachDraft, ReconciliationDetachRequirement, ReconciliationDetachStatus,
    ReconciliationDownstreamPreview, ReconciliationExporterRoute, ReconciliationImportPatchDraft,
    ReconciliationReExportManifest, ReconciliationReExportStatus, ReconciliationReExportTask,
    ReconciliationTaskRecord, ReconciliationTaskStatus, RenderTaskRecord, RenderTaskStatus,
};
pub use project_store::{
    DurableProjectState, DurableProjectStore, ExternalCommitRecord, ExternalMutationFence,
    ManagedTargetState, PendingExternalCommit, PendingExternalCommitKind, ProjectStoreError,
    TargetLink, VerifiedTargetBinding,
};
pub use scene_inspection::{
    ChangedCueFrameRange, SceneCaptureEvidence, SceneCaptureProfile, SceneCaptureSamplePlan,
    SceneHumanReview, SceneInspectionError, SceneInspectionPlan, SceneInspectionReceipt,
    SceneInspectionSource, SceneInspectionStatus, SceneInspectionTask, SceneReviewDecision,
    sample_changed_cue_frames,
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
