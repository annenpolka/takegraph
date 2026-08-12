//! Durable, source-bound checkpoint, render, and reconciliation workflows.
//!
//! Checkpoint/render and reconciliation planning read but never advance the
//! canonical revision. A separately approved, receipt-verified permanent
//! metadata detach is the exception: it CAS-removes canonical ownership and
//! advances exactly one revision. Every transition is an immutable generation
//! published by atomic rename; corrupt or discontinuous journals fail closed.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use takegraph_core::{
    AssetKind, CanonicalError, EffectOperation, ManagedSemanticIdentity, ManagedSemanticItem,
    ManagedSemanticValue, NativeExtensionIntent, Patch, PatchError, PatchId, PatchStatus,
    PortraitPresentation, ReconciliationAction, ReconciliationDecision, ReconciliationError,
    ReconciliationPreview, ReconciliationSource, RevisionId, SemanticDriftReport, canonical_sha256,
};
use takegraph_node::{
    CapabilityRequirement, ManagedUtterance, MetadataDetachNodeError, ProjectOperationNodeError,
    RenderOverwritePolicy, VerifiedMediaProbe, Ymm4BridgeClient, Ymm4CheckpointProfile,
    Ymm4CheckpointReceipt, Ymm4CheckpointRequest, Ymm4CheckpointRequestInput, Ymm4CheckpointStatus,
    Ymm4Error, Ymm4MetadataDetachReceipt, Ymm4MetadataDetachRequest,
    Ymm4MetadataDetachRequestInput, Ymm4MetadataDetachStatus, Ymm4NativeVoiceMutation,
    Ymm4ProjectSnapshot, Ymm4RenderProfileDescriptor, Ymm4RenderRequest, Ymm4RenderRequestInput,
    Ymm4RenderStatus, Ymm4RenderTask, validate_render_task, verify_checkpoint,
    verify_metadata_detach, verify_metadata_detach_not_started, verify_render_checkpoint_file,
    verify_rendered_media,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{DurableProjectStore, ProjectStoreError};
use crate::{
    NativeExtensionStageManifest, Ymm4ExportPatch, Ymm4NativeExtensionTask,
    Ymm4NativeVoiceMutationPatch,
};

const JOURNAL_SCHEMA_VERSION: u32 = 1;
const RECONCILIATION_STATE_PROFILE: &str = "takegraph-ymm4-managed-semantic-projection/v2";

/// Persisted service checkpoint state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTaskStatus {
    Staged,
    Executing,
    Verified,
    Failed,
    Stale,
    RecoveryRequired,
}

fn reconciliation_action_entry_id(action: &ReconciliationAction) -> &str {
    match action {
        ReconciliationAction::ProposeImportPatch { entry_id, .. }
        | ReconciliationAction::DetachManagedIdentity { entry_id, .. }
        | ReconciliationAction::ReExportCanonicalState { entry_id, .. } => entry_id,
    }
}

fn previewable_patch(base: RevisionId, digest: String) -> Result<Patch, ProjectOperationError> {
    let mut patch = Patch::draft(base, digest);
    patch.validate()?;
    patch.materialize_preview()?;
    Ok(patch)
}

fn build_reconciliation_child(
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    action: &ReconciliationAction,
) -> Result<ReconciliationChildTask, ProjectOperationError> {
    match action {
        ReconciliationAction::ProposeImportPatch {
            entry_id,
            identity,
            observed,
        } => build_import_child(
            child_task_id,
            action_digest,
            report,
            entry_id,
            identity,
            observed,
        ),
        ReconciliationAction::DetachManagedIdentity {
            entry_id,
            identity,
            observed,
        } => build_detach_child(
            child_task_id,
            action_digest,
            report,
            entry_id,
            identity,
            observed,
        ),
        ReconciliationAction::ReExportCanonicalState {
            entry_id,
            identity,
            canonical,
        } => build_re_export_child(
            child_task_id,
            action_digest,
            report,
            entry_id,
            identity,
            canonical,
        ),
    }
}

fn build_import_child(
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    observed: &[ManagedSemanticItem],
) -> Result<ReconciliationChildTask, ProjectOperationError> {
    let digest = import_patch_digest(
        child_task_id,
        action_digest,
        report,
        entry_id,
        identity,
        observed,
    )?;
    Ok(ReconciliationChildTask::ImportPatch(Box::new(
        ReconciliationImportPatchDraft {
            child_task_id: child_task_id.into(),
            report_digest: report.report_digest.clone(),
            entry_id: entry_id.into(),
            source: report.source.clone(),
            identity: identity.clone(),
            observed: observed.to_vec(),
            patch: previewable_patch(report.source.source_revision, digest)?,
        },
    )))
}

fn build_detach_child(
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    observed: &[ManagedSemanticItem],
) -> Result<ReconciliationChildTask, ProjectOperationError> {
    let operation_id = Uuid::new_v4();
    let contract = reconciliation_detach_contract();
    let digest = detach_patch_digest(
        child_task_id,
        operation_id,
        action_digest,
        report,
        entry_id,
        identity,
        observed,
        &contract,
    )?;
    Ok(ReconciliationChildTask::MetadataDetach(Box::new(
        ReconciliationDetachDraft {
            child_task_id: child_task_id.into(),
            operation_id,
            report_digest: report.report_digest.clone(),
            entry_id: entry_id.into(),
            source: report.source.clone(),
            identity: identity.clone(),
            observed: observed.to_vec(),
            contract,
            patch: previewable_patch(report.source.source_revision, digest)?,
            status: ReconciliationDetachStatus::PreviewReady,
            request: None,
            receipt: None,
            committed_revision: None,
            error: None,
        },
    )))
}

fn reissue_detach_attempt(
    draft: &mut ReconciliationDetachDraft,
    action_digest: &str,
    report: &SemanticDriftReport,
) -> Result<(), ProjectOperationError> {
    let operation_id = Uuid::new_v4();
    let digest = detach_patch_digest(
        &draft.child_task_id,
        operation_id,
        action_digest,
        report,
        &draft.entry_id,
        &draft.identity,
        &draft.observed,
        &draft.contract,
    )?;
    draft.operation_id = operation_id;
    draft.patch = previewable_patch(draft.source.source_revision, digest)?;
    draft.status = ReconciliationDetachStatus::PreviewReady;
    draft.request = None;
    draft.receipt = None;
    draft.committed_revision = None;
    draft.error = None;
    Ok(())
}

fn build_re_export_child(
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    canonical: &[ManagedSemanticItem],
) -> Result<ReconciliationChildTask, ProjectOperationError> {
    let downstream_task_id = Uuid::new_v4();
    let exporter_route = reconciliation_exporter_route(report, entry_id)?;
    let handoff_digest = re_export_handoff_digest(
        child_task_id,
        downstream_task_id,
        action_digest,
        report,
        entry_id,
        identity,
        canonical,
        exporter_route,
    )?;
    Ok(ReconciliationChildTask::CanonicalReExport(Box::new(
        ReconciliationReExportTask {
            child_task_id: child_task_id.into(),
            downstream_task_id,
            report_digest: report.report_digest.clone(),
            entry_id: entry_id.into(),
            source: report.source.clone(),
            identity: identity.clone(),
            canonical: canonical.to_vec(),
            exporter_route,
            downstream_preview_required: true,
            downstream_approval_required: true,
            handoff_digest,
            dispatch_manifest_digest: None,
            status: ReconciliationReExportStatus::AwaitingManifest,
            downstream_preview: None,
            error: None,
        },
    )))
}

fn import_patch_digest(
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    observed: &[ManagedSemanticItem],
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-reconciliation-import-patch-v1",
        &(
            child_task_id,
            action_digest,
            &report.report_digest,
            &report.source,
            entry_id,
            identity,
            observed,
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn detach_patch_digest(
    child_task_id: &str,
    operation_id: Uuid,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    observed: &[ManagedSemanticItem],
    contract: &ReconciliationDetachContract,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-reconciliation-detach-plan-v1",
        &(
            child_task_id,
            operation_id,
            action_digest,
            &report.report_digest,
            &report.source,
            entry_id,
            identity,
            observed,
            contract,
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn re_export_handoff_digest(
    child_task_id: &str,
    downstream_task_id: Uuid,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    canonical: &[ManagedSemanticItem],
    exporter_route: ReconciliationExporterRoute,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-reconciliation-re-export-handoff-v1",
        &(
            child_task_id,
            downstream_task_id,
            action_digest,
            &report.report_digest,
            &report.source,
            entry_id,
            identity,
            canonical,
            exporter_route,
            true,
            true,
        ),
    )
}

fn reconciliation_detach_contract() -> ReconciliationDetachContract {
    ReconciliationDetachContract {
        identity_carrier: "takegraph_remark_v2".into(),
        permitted_mutation: "remove_takegraph_identity_remark_only".into(),
        requirements: vec![
            ReconciliationDetachRequirement::PreserveAllNonRemarkContent,
            ReconciliationDetachRequirement::WriteAheadLog,
            ReconciliationDetachRequirement::AuthenticatedReplay,
            ReconciliationDetachRequirement::FreshReadback,
            ReconciliationDetachRequirement::RemarkAbsent,
            ReconciliationDetachRequirement::ContentDigestUnchanged,
        ],
        bridge_wire_status: ReconciliationBridgeWireStatus::Available,
    }
}

fn reconciliation_exporter_route(
    report: &SemanticDriftReport,
    entry_id: &str,
) -> Result<ReconciliationExporterRoute, ProjectOperationError> {
    let entry = report
        .entries
        .iter()
        .find(|entry| entry.entry_id == entry_id)
        .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;
    let kinds = entry
        .expected
        .iter()
        .chain(&entry.actual)
        .map(|item| item.realization_kind.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if kinds
        .iter()
        .all(|kind| matches!(*kind, "portable_audio" | "portable_caption"))
        && !kinds.is_empty()
    {
        return Ok(ReconciliationExporterRoute::PortablePair);
    }
    if kinds.len() == 1 && kinds.contains("ymm4_native_voice") {
        return Ok(ReconciliationExporterRoute::NativeVoiceMutation);
    }
    if !kinds.is_empty() && kinds.iter().all(|kind| kind.starts_with("ymm4_native_")) {
        return Ok(ReconciliationExporterRoute::NativeExtension);
    }
    Err(ProjectOperationError::UnsupportedReconciliationExporter(
        kinds.into_iter().map(str::to_owned).collect(),
    ))
}

fn validate_reconciliation_child(
    task: &ReconciliationChildTask,
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    action: &ReconciliationAction,
) -> Result<ReconciliationChildReference, ProjectOperationError> {
    match (task, action) {
        (
            ReconciliationChildTask::ImportPatch(draft),
            ReconciliationAction::ProposeImportPatch {
                entry_id,
                identity,
                observed,
            },
        ) => validate_import_child(
            draft,
            child_task_id,
            action_digest,
            report,
            entry_id,
            identity,
            observed,
        ),
        (
            ReconciliationChildTask::MetadataDetach(draft),
            ReconciliationAction::DetachManagedIdentity {
                entry_id,
                identity,
                observed,
            },
        ) => validate_detach_child(
            draft,
            child_task_id,
            action_digest,
            report,
            entry_id,
            identity,
            observed,
        ),
        (
            ReconciliationChildTask::CanonicalReExport(handoff),
            ReconciliationAction::ReExportCanonicalState {
                entry_id,
                identity,
                canonical,
            },
        ) => validate_re_export_child(
            handoff,
            child_task_id,
            action_digest,
            report,
            entry_id,
            identity,
            canonical,
        ),
        _ => Err(ProjectOperationError::InvalidReconciliationChildren),
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_import_child(
    draft: &ReconciliationImportPatchDraft,
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    observed: &[ManagedSemanticItem],
) -> Result<ReconciliationChildReference, ProjectOperationError> {
    let digest = import_patch_digest(
        child_task_id,
        action_digest,
        report,
        entry_id,
        identity,
        observed,
    )?;
    if draft.child_task_id != child_task_id
        || draft.report_digest != report.report_digest
        || draft.entry_id != entry_id
        || draft.source != report.source
        || draft.identity != *identity
        || draft.observed != observed
        || !valid_unapproved_child_patch(&draft.patch, &report.source, &digest)
    {
        return Err(ProjectOperationError::InvalidReconciliationChildren);
    }
    Ok(ReconciliationChildReference {
        entry_id: entry_id.into(),
        child_task_id: child_task_id.into(),
        kind: ReconciliationChildKind::ImportPatch,
        patch_id: Some(draft.patch.id),
        downstream_task_id: None,
        action_digest: action_digest.into(),
    })
}

#[allow(clippy::too_many_arguments)]
fn validate_detach_child(
    draft: &ReconciliationDetachDraft,
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    observed: &[ManagedSemanticItem],
) -> Result<ReconciliationChildReference, ProjectOperationError> {
    let digest = detach_patch_digest(
        child_task_id,
        draft.operation_id,
        action_digest,
        report,
        entry_id,
        identity,
        observed,
        &draft.contract,
    )?;
    if draft.child_task_id != child_task_id
        || draft.report_digest != report.report_digest
        || draft.entry_id != entry_id
        || draft.source != report.source
        || draft.identity != *identity
        || draft.observed != observed
        || !valid_detach_child_lifecycle(draft, &report.source, &digest)
        || draft.contract != reconciliation_detach_contract()
    {
        return Err(ProjectOperationError::InvalidReconciliationChildren);
    }
    Ok(ReconciliationChildReference {
        entry_id: entry_id.into(),
        child_task_id: child_task_id.into(),
        kind: ReconciliationChildKind::MetadataDetach,
        patch_id: Some(draft.patch.id),
        downstream_task_id: Some(draft.operation_id),
        action_digest: action_digest.into(),
    })
}

fn valid_detach_child_lifecycle(
    draft: &ReconciliationDetachDraft,
    source: &ReconciliationSource,
    digest: &str,
) -> bool {
    if draft.patch.base != source.source_revision || draft.patch.digest != digest {
        return false;
    }
    let approved = draft.patch.status == PatchStatus::Approved
        && draft.patch.approved_digest.as_deref() == Some(digest);
    match draft.status {
        ReconciliationDetachStatus::PreviewReady => {
            draft.patch.status == PatchStatus::Previewable
                && draft.patch.approved_digest.is_none()
                && draft.request.is_none()
                && draft.receipt.is_none()
                && draft.committed_revision.is_none()
                && draft.error.is_none()
        }
        ReconciliationDetachStatus::Approved => {
            approved
                && draft.request.is_none()
                && draft.receipt.is_none()
                && draft.committed_revision.is_none()
                && draft.error.is_none()
        }
        ReconciliationDetachStatus::Applying => {
            approved
                && draft.request.is_some()
                && draft.committed_revision.is_none()
                && draft.receipt.as_ref().is_none_or(|receipt| {
                    receipt.status == Ymm4MetadataDetachStatus::Verified
                        && receipt.verified
                        && receipt.error.is_none()
                        && draft.error.is_some()
                })
        }
        ReconciliationDetachStatus::Verified => {
            approved
                && draft.request.is_some()
                && draft.receipt.as_ref().is_some_and(|receipt| {
                    receipt.status == Ymm4MetadataDetachStatus::Verified
                        && receipt.verified
                        && receipt.error.is_none()
                })
                && draft.committed_revision == source.source_revision.checked_next()
                && draft.error.is_none()
        }
        ReconciliationDetachStatus::RolledBack
        | ReconciliationDetachStatus::Failed
        | ReconciliationDetachStatus::RecoveryRequired => {
            approved
                && draft.request.is_some()
                && draft.committed_revision.is_none()
                && draft.error.is_some()
        }
        ReconciliationDetachStatus::Stale => approved && draft.error.is_some(),
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_re_export_child(
    handoff: &ReconciliationReExportTask,
    child_task_id: &str,
    action_digest: &str,
    report: &SemanticDriftReport,
    entry_id: &str,
    identity: &ManagedSemanticIdentity,
    canonical: &[ManagedSemanticItem],
) -> Result<ReconciliationChildReference, ProjectOperationError> {
    let exporter_route = reconciliation_exporter_route(report, entry_id)?;
    let digest = re_export_handoff_digest(
        child_task_id,
        handoff.downstream_task_id,
        action_digest,
        report,
        entry_id,
        identity,
        canonical,
        exporter_route,
    )?;
    if handoff.child_task_id != child_task_id
        || handoff.report_digest != report.report_digest
        || handoff.entry_id != entry_id
        || handoff.source != report.source
        || handoff.identity != *identity
        || handoff.canonical != canonical
        || handoff.exporter_route != exporter_route
        || !handoff.downstream_preview_required
        || !handoff.downstream_approval_required
        || handoff.handoff_digest != digest
        || !valid_re_export_lifecycle(handoff)
    {
        return Err(ProjectOperationError::InvalidReconciliationChildren);
    }
    Ok(ReconciliationChildReference {
        entry_id: entry_id.into(),
        child_task_id: child_task_id.into(),
        kind: ReconciliationChildKind::CanonicalReExport,
        patch_id: None,
        downstream_task_id: Some(handoff.downstream_task_id),
        action_digest: action_digest.into(),
    })
}

fn valid_re_export_lifecycle(handoff: &ReconciliationReExportTask) -> bool {
    match handoff.status {
        ReconciliationReExportStatus::AwaitingManifest => {
            handoff.dispatch_manifest_digest.is_none()
                && handoff.downstream_preview.is_none()
                && handoff.error.is_none()
        }
        ReconciliationReExportStatus::PreviewReady => {
            handoff.dispatch_manifest_digest.is_some()
                && handoff.downstream_preview.as_ref().is_some_and(|preview| {
                    downstream_preview_route(preview) == handoff.exporter_route
                        && downstream_preview_patch(preview).status == PatchStatus::Previewable
                        && downstream_preview_patch(preview).approved_digest.is_none()
                        && downstream_preview_patch(preview).base == handoff.source.source_revision
                        && downstream_preview_operation_id(preview) == handoff.downstream_task_id
                })
                && handoff.error.is_none()
        }
        ReconciliationReExportStatus::Stale | ReconciliationReExportStatus::Failed => {
            handoff.downstream_preview.is_none() && handoff.error.is_some()
        }
    }
}

const fn downstream_preview_route(
    preview: &ReconciliationDownstreamPreview,
) -> ReconciliationExporterRoute {
    match preview {
        ReconciliationDownstreamPreview::PortablePair(_) => {
            ReconciliationExporterRoute::PortablePair
        }
        ReconciliationDownstreamPreview::NativeVoiceMutation(_) => {
            ReconciliationExporterRoute::NativeVoiceMutation
        }
        ReconciliationDownstreamPreview::NativeExtension(_) => {
            ReconciliationExporterRoute::NativeExtension
        }
    }
}

fn downstream_preview_patch(preview: &ReconciliationDownstreamPreview) -> &Patch {
    match preview {
        ReconciliationDownstreamPreview::PortablePair(task) => &task.patch,
        ReconciliationDownstreamPreview::NativeVoiceMutation(task) => &task.patch,
        ReconciliationDownstreamPreview::NativeExtension(task) => &task.patch,
    }
}

fn downstream_preview_operation_id(preview: &ReconciliationDownstreamPreview) -> Uuid {
    match preview {
        ReconciliationDownstreamPreview::PortablePair(task) => task.operation_id,
        ReconciliationDownstreamPreview::NativeVoiceMutation(task) => task.operation_id,
        ReconciliationDownstreamPreview::NativeExtension(task) => task.operation_id,
    }
}

fn valid_unapproved_child_patch(
    patch: &Patch,
    source: &ReconciliationSource,
    digest: &str,
) -> bool {
    patch.base == source.source_revision
        && patch.digest == digest
        && patch.status == PatchStatus::Previewable
        && patch.approved_digest.is_none()
}

/// One verified-existing-path save workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointTaskRecord {
    pub request: Ymm4CheckpointRequest,
    pub profile: Ymm4CheckpointProfile,
    pub expected_project_path: String,
    pub status: CheckpointTaskStatus,
    pub receipt: Option<Ymm4CheckpointReceipt>,
    pub error: Option<String>,
}

/// Persisted service render state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderTaskStatus {
    Staged,
    Submitting,
    Running,
    Finalizing,
    Cancelling,
    Cancelled,
    Verified,
    Failed,
    Stale,
    RecoveryRequired,
}

impl RenderTaskStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Verified | Self::Failed | Self::Stale | Self::RecoveryRequired
        )
    }
}

/// One authoritative native render workflow and its final local evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderTaskRecord {
    pub request: Ymm4RenderRequest,
    pub profile: Ymm4RenderProfileDescriptor,
    pub status: RenderTaskStatus,
    pub bridge_task: Option<Ymm4RenderTask>,
    pub verified_media: Option<VerifiedMediaProbe>,
    pub error: Option<String>,
}

/// Persisted reconciliation state. Accepted actions still use their normal
/// patch/adapter approval path; acceptance never performs silent sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationTaskStatus {
    ReportReady,
    DecisionPreviewReady,
    ActionsMaterialized,
    /// Legacy v1 journal state produced before durable child materialization.
    ActionsAccepted,
    Stale,
}

/// A managed-subset drift report with optional explicit decision preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationTaskRecord {
    pub report: SemanticDriftReport,
    pub expected_managed_state: Vec<ManagedSemanticItem>,
    pub preview: Option<ReconciliationPreview>,
    #[serde(default)]
    pub materialized_children: Vec<ReconciliationChildReference>,
    pub status: ReconciliationTaskStatus,
}

/// Downstream workflow selected by one accepted reconciliation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationChildKind {
    ImportPatch,
    MetadataDetach,
    CanonicalReExport,
}

/// Stable pointer returned from reconciliation acceptance. Import and detach
/// expose their normal approval patch IDs; every action has a durable child ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationChildReference {
    pub entry_id: String,
    pub child_task_id: String,
    pub kind: ReconciliationChildKind,
    pub patch_id: Option<PatchId>,
    pub downstream_task_id: Option<Uuid>,
    pub action_digest: String,
}

fn same_stable_child_reference(
    current: &ReconciliationChildReference,
    stored: &ReconciliationChildReference,
) -> bool {
    current.entry_id == stored.entry_id
        && current.child_task_id == stored.child_task_id
        && current.kind == stored.kind
        && current.action_digest == stored.action_digest
        && (current.kind == ReconciliationChildKind::MetadataDetach
            || (current.patch_id == stored.patch_id
                && current.downstream_task_id == stored.downstream_task_id))
}

fn metadata_detach_operation_id(task: &ReconciliationChildTask) -> Uuid {
    let ReconciliationChildTask::MetadataDetach(draft) = task else {
        unreachable!("metadata detach reissue returned another child kind");
    };
    draft.operation_id
}

fn reissuable_detach_terminal(task: &ReconciliationChildTask) -> bool {
    let ReconciliationChildTask::MetadataDetach(draft) = task else {
        return false;
    };
    matches!(
        (
            draft.status,
            draft.receipt.as_ref().map(|receipt| receipt.status)
        ),
        (
            ReconciliationDetachStatus::RolledBack,
            Some(Ymm4MetadataDetachStatus::RolledBack)
        ) | (
            ReconciliationDetachStatus::Failed,
            Some(Ymm4MetadataDetachStatus::NotStarted)
        )
    )
}

fn metadata_detach_terminal_error(task: &ReconciliationChildTask) -> Option<String> {
    let ReconciliationChildTask::MetadataDetach(draft) = task else {
        return None;
    };
    draft.error.clone()
}

/// Durable child handoff. These records are inert: materialization never
/// mutates canonical state or YMM4 and never counts as downstream approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "type",
    content = "task",
    deny_unknown_fields
)]
pub enum ReconciliationChildTask {
    ImportPatch(Box<ReconciliationImportPatchDraft>),
    MetadataDetach(Box<ReconciliationDetachDraft>),
    CanonicalReExport(Box<ReconciliationReExportTask>),
}

/// Semantic import proposal in the ordinary core Patch lifecycle. The patch is
/// previewable, has no approved digest, and therefore cannot commit yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationImportPatchDraft {
    pub child_task_id: String,
    pub report_digest: String,
    pub entry_id: String,
    pub source: ReconciliationSource,
    pub identity: ManagedSemanticIdentity,
    pub observed: Vec<ManagedSemanticItem>,
    pub patch: Patch,
}

/// One mandatory safety property of metadata-only detach execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationDetachRequirement {
    PreserveAllNonRemarkContent,
    WriteAheadLog,
    AuthenticatedReplay,
    FreshReadback,
    RemarkAbsent,
    ContentDigestUnchanged,
}

/// Whether the adapter has an executable route satisfying the whole contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationBridgeWireStatus {
    Unavailable,
    Available,
}

/// Metadata-only detach contract. The bridge route may remove only the
/// `TakeGraph` identity Remark; it must WAL the prior Remark and prove by fresh
/// read-back that all non-Remark content is byte/semantic-digest unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationDetachContract {
    pub identity_carrier: String,
    pub permitted_mutation: String,
    pub requirements: Vec<ReconciliationDetachRequirement>,
    pub bridge_wire_status: ReconciliationBridgeWireStatus,
}

/// Independently approval-waiting adapter request for an executable detach.
/// The exact request is persisted before bridge I/O and its receipt is retained
/// for authenticated replay and recovery-visible status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationDetachDraft {
    pub child_task_id: String,
    pub operation_id: Uuid,
    pub report_digest: String,
    pub entry_id: String,
    pub source: ReconciliationSource,
    pub identity: ManagedSemanticIdentity,
    pub observed: Vec<ManagedSemanticItem>,
    pub contract: ReconciliationDetachContract,
    pub patch: Patch,
    #[serde(default)]
    pub status: ReconciliationDetachStatus,
    #[serde(default)]
    pub request: Option<Ymm4MetadataDetachRequest>,
    #[serde(default)]
    pub receipt: Option<Ymm4MetadataDetachReceipt>,
    /// Canonical CAS revision that permanently removed this ownership identity.
    #[serde(default)]
    pub committed_revision: Option<RevisionId>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Durable lifecycle for the independently-approved metadata detach child.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationDetachStatus {
    #[default]
    PreviewReady,
    Approved,
    Applying,
    Verified,
    RolledBack,
    Failed,
    RecoveryRequired,
    Stale,
}

/// Existing-exporter route for a canonical re-export handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationExporterRoute {
    PortablePair,
    NativeVoiceMutation,
    NativeExtension,
}

/// Staged handoff to an existing exporter. The exporter must still create its
/// own target plan/preview and obtain ordinary approval before any mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationReExportTask {
    pub child_task_id: String,
    pub downstream_task_id: Uuid,
    pub report_digest: String,
    pub entry_id: String,
    pub source: ReconciliationSource,
    pub identity: ManagedSemanticIdentity,
    pub canonical: Vec<ManagedSemanticItem>,
    pub exporter_route: ReconciliationExporterRoute,
    pub downstream_preview_required: bool,
    pub downstream_approval_required: bool,
    pub handoff_digest: String,
    /// Exact route-tagged manifest bound to the generated downstream preview.
    /// A retry must present the same digest or it is rejected.
    #[serde(default)]
    pub dispatch_manifest_digest: Option<String>,
    #[serde(default)]
    pub status: ReconciliationReExportStatus,
    #[serde(default)]
    pub downstream_preview: Option<ReconciliationDownstreamPreview>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Re-export dispatch remains a preview-only lifecycle. No state represents
/// automatic approval or apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationReExportStatus {
    #[default]
    AwaitingManifest,
    PreviewReady,
    Stale,
    Failed,
}

/// Route-specific exporter input supplied only after the reconciliation child
/// exists. The dispatcher checks its semantic projection against canonical
/// state before invoking an existing exporter staging path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "route", deny_unknown_fields)]
pub enum ReconciliationReExportManifest {
    PortablePair {
        utterances: Vec<ManagedUtterance>,
    },
    NativeVoiceMutation {
        mutations: Vec<Ymm4NativeVoiceMutation>,
    },
    NativeExtension {
        manifest: NativeExtensionStageManifest,
        artifact_root: PathBuf,
    },
}

/// Ordinary existing-exporter preview persisted inside the reconciliation
/// child. Each inner patch remains previewable and unapproved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "route", content = "preview")]
pub enum ReconciliationDownstreamPreview {
    PortablePair(Ymm4ExportPatch),
    NativeVoiceMutation(Ymm4NativeVoiceMutationPatch),
    NativeExtension(Box<Ymm4NativeExtensionTask>),
}

/// One immutable, hash-chained task generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableTaskRecord<T> {
    pub schema_version: u32,
    pub generation: u64,
    pub previous_record_digest: Option<String>,
    pub payload: T,
    pub record_digest: String,
}

/// Append-only external-operation store beneath a service-owned root.
#[derive(Debug, Clone)]
pub struct ProjectOperationStore {
    root: PathBuf,
}

impl ProjectOperationStore {
    #[must_use]
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Stages a save bound to current target identity/state and canonical head.
    ///
    /// # Errors
    ///
    /// Returns an error for stale/corrupt canonical state, an unsaved project,
    /// bridge failure, or corrupt task journal data.
    pub async fn stage_checkpoint(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        source_revision: RevisionId,
    ) -> Result<DurableTaskRecord<CheckpointTaskRecord>, ProjectOperationError> {
        require_revision(canonical, source_revision)?;
        require_bridge_feature(client, "project.checkpoint").await?;
        let health = client.health().await?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != canonical.project_id() {
            return Err(ProjectOperationError::CanonicalProjectMismatch {
                expected: canonical.project_id().into(),
                actual: snapshot.project_id,
            });
        }
        if snapshot.project_path.trim().is_empty() {
            return Err(ProjectOperationError::UnsavedProject);
        }
        let profile = client.checkpoint_profile().await?;
        if !profile.existing_path_only || profile.profile_digest.trim().is_empty() {
            return Err(ProjectOperationError::InvalidCheckpointProfile);
        }
        let target_identity_digest = target_identity_digest(&health.ymm4_version, &snapshot)?;
        let request = Ymm4CheckpointRequest::try_new(Ymm4CheckpointRequestInput {
            operation_id: Uuid::new_v4(),
            project_id: snapshot.project_id,
            scene_id: snapshot.scene_id,
            source_revision: source_revision.0,
            target_identity_digest,
            expected_state_digest: snapshot.fingerprint,
            checkpoint_profile_digest: profile.profile_digest.clone(),
        })?;
        let payload = CheckpointTaskRecord {
            request,
            profile,
            expected_project_path: snapshot.project_path,
            status: CheckpointTaskStatus::Staged,
            receipt: None,
            error: None,
        };
        self.append_checkpoint(&payload, None)
    }

    /// Executes/replays a staged save, re-hashes the project file, and proves
    /// that neither target state nor canonical revision changed.
    ///
    /// # Errors
    ///
    /// Returns an error for stale bindings, bridge/save failure, receipt/file
    /// mismatch, invalid lifecycle state, or corrupt persistence.
    pub async fn execute_checkpoint(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        operation_id: Uuid,
    ) -> Result<DurableTaskRecord<CheckpointTaskRecord>, ProjectOperationError> {
        let mut durable = self.checkpoint_status(operation_id)?;
        if durable.payload.status == CheckpointTaskStatus::Verified {
            require_revision(
                canonical,
                RevisionId(durable.payload.request.source_revision),
            )?;
            let receipt = durable
                .payload
                .receipt
                .as_ref()
                .ok_or(ProjectOperationError::MissingCheckpointReceipt)?;
            verify_checkpoint(&durable.payload.request, &durable.payload.profile, receipt)?;
            return Ok(durable);
        }
        if let Err(error) = self
            .require_checkpoint_fresh(canonical, client, &durable.payload)
            .await
        {
            self.record_checkpoint_stale(&mut durable, &error);
            return Err(error);
        }
        if durable.payload.status != CheckpointTaskStatus::Staged
            && durable.payload.status != CheckpointTaskStatus::Executing
        {
            return Err(ProjectOperationError::InvalidCheckpointTransition(
                durable.payload.status,
            ));
        }
        if durable.payload.status == CheckpointTaskStatus::Staged {
            durable.payload.status = CheckpointTaskStatus::Executing;
            durable.payload.error = None;
            durable = self.append_checkpoint(&durable.payload, Some(durable.generation))?;
        }
        let receipt = match client.create_checkpoint(&durable.payload.request).await {
            Ok(receipt) => receipt,
            Err(error) => {
                durable.payload.status = CheckpointTaskStatus::Failed;
                durable.payload.error = Some(error.to_string());
                let _ = self.append_checkpoint(&durable.payload, Some(durable.generation));
                return Err(error.into());
            }
        };
        let verification = if receipt.project_path == durable.payload.expected_project_path {
            verify_checkpoint(&durable.payload.request, &durable.payload.profile, &receipt)
        } else {
            Err(ProjectOperationNodeError::OutputPathMismatch)
        };
        if let Err(error) = verification {
            durable.payload.status = match receipt.status {
                Ymm4CheckpointStatus::Stale => CheckpointTaskStatus::Stale,
                Ymm4CheckpointStatus::RecoveryRequired => CheckpointTaskStatus::RecoveryRequired,
                _ => CheckpointTaskStatus::Failed,
            };
            durable.payload.receipt = Some(receipt);
            durable.payload.error = Some(error.to_string());
            let _ = self.append_checkpoint(&durable.payload, Some(durable.generation));
            return Err(error.into());
        }
        if let Err(error) = self
            .require_checkpoint_fresh(canonical, client, &durable.payload)
            .await
        {
            self.record_checkpoint_stale(&mut durable, &error);
            return Err(error);
        }
        durable.payload.status = CheckpointTaskStatus::Verified;
        durable.payload.receipt = Some(receipt);
        durable.payload.error = None;
        self.append_checkpoint(&durable.payload, Some(durable.generation))
    }

    /// Reads the latest validated checkpoint generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is missing or its journal is corrupt.
    pub fn checkpoint_status(
        &self,
        operation_id: Uuid,
    ) -> Result<DurableTaskRecord<CheckpointTaskRecord>, ProjectOperationError> {
        self.load_latest(TaskKind::Checkpoint, &operation_id.to_string())?
            .ok_or(ProjectOperationError::TaskNotFound(operation_id))
    }

    /// Stages a render after resolving an exact read-only profile descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error for stale/corrupt canonical state, descriptor failure,
    /// invalid output policy, bridge failure, or corrupt task persistence.
    #[allow(clippy::too_many_arguments)]
    pub async fn stage_render(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        source_revision: RevisionId,
        checkpoint_operation_id: Uuid,
        profile_id_or_digest: &str,
        output_path: PathBuf,
        overwrite_policy: RenderOverwritePolicy,
    ) -> Result<DurableTaskRecord<RenderTaskRecord>, ProjectOperationError> {
        require_revision(canonical, source_revision)?;
        let health = client.health().await?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != canonical.project_id() {
            return Err(ProjectOperationError::CanonicalProjectMismatch {
                expected: canonical.project_id().into(),
                actual: snapshot.project_id,
            });
        }
        let profiles = client.render_profiles().await?;
        let profile = profiles
            .profiles
            .into_iter()
            .find(|candidate| {
                candidate.descriptor_id == profile_id_or_digest
                    || candidate.profile_digest == profile_id_or_digest
            })
            .ok_or_else(|| {
                ProjectOperationError::RenderProfileNotFound(profile_id_or_digest.into())
            })?;
        if !profile.bindable {
            return Err(ProjectOperationError::RenderProfileNotBindable(
                profile.binding_error.clone().unwrap_or_else(|| {
                    "bridge did not provide an exact encoder configuration binding".into()
                }),
            ));
        }
        require_bridge_feature(client, "project.render").await?;
        let checkpoint = self.checkpoint_status(checkpoint_operation_id)?;
        if checkpoint.payload.status != CheckpointTaskStatus::Verified {
            return Err(ProjectOperationError::RenderCheckpointNotVerified(
                checkpoint_operation_id,
            ));
        }
        let checkpoint_receipt = checkpoint
            .payload
            .receipt
            .as_ref()
            .ok_or(ProjectOperationError::MissingCheckpointReceipt)?;
        verify_checkpoint(
            &checkpoint.payload.request,
            &checkpoint.payload.profile,
            checkpoint_receipt,
        )?;
        let target_identity_digest = target_identity_digest(&health.ymm4_version, &snapshot)?;
        if checkpoint.payload.request.project_id != snapshot.project_id
            || checkpoint.payload.request.scene_id != snapshot.scene_id
            || checkpoint.payload.request.source_revision != source_revision.0
            || checkpoint.payload.request.target_identity_digest != target_identity_digest
            || checkpoint.payload.request.expected_state_digest != snapshot.fingerprint
            || checkpoint_receipt.project_path != snapshot.project_path
        {
            return Err(ProjectOperationError::StaleTargetState);
        }
        let request = Ymm4RenderRequest::try_new(Ymm4RenderRequestInput {
            task_id: Uuid::new_v4(),
            project_id: snapshot.project_id,
            scene_id: snapshot.scene_id,
            source_revision: source_revision.0,
            target_identity_digest,
            expected_state_digest: snapshot.fingerprint,
            checkpoint_operation_id,
            checkpoint_request_digest: checkpoint.payload.request.request_digest.clone(),
            checkpoint_project_path: checkpoint_receipt.project_path.clone(),
            checkpoint_file_sha256: checkpoint_receipt
                .post_file_sha256
                .clone()
                .ok_or(ProjectOperationError::MissingCheckpointReceipt)?,
            checkpoint_file_bytes: checkpoint_receipt
                .post_file_bytes
                .ok_or(ProjectOperationError::MissingCheckpointReceipt)?,
            render_profile_digest: profile.profile_digest.clone(),
            output_path,
            overwrite_policy,
        })?;
        let payload = RenderTaskRecord {
            request,
            profile,
            status: RenderTaskStatus::Staged,
            bridge_task: None,
            verified_media: None,
            error: None,
        };
        self.append_render(&payload, None)
    }

    /// Starts or resumes a render without blocking for completion.
    ///
    /// # Errors
    ///
    /// Returns an error for stale state, invalid lifecycle/task binding, bridge
    /// failure, final-media mismatch, or corrupt persistence.
    pub async fn execute_render(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        task_id: Uuid,
    ) -> Result<DurableTaskRecord<RenderTaskRecord>, ProjectOperationError> {
        let mut durable = self.render_status(task_id)?;
        if durable.payload.status == RenderTaskStatus::Verified {
            require_revision(
                canonical,
                RevisionId(durable.payload.request.source_revision),
            )?;
            let bridge_task = durable
                .payload
                .bridge_task
                .as_ref()
                .ok_or(ProjectOperationError::MissingBridgeTask)?;
            let measured = verify_rendered_media(&durable.payload.request, bridge_task)?;
            verify_media_profile(&durable.payload.profile, &measured)?;
            return Ok(durable);
        }
        if durable.payload.status.is_terminal() {
            return Ok(durable);
        }
        if let Err(error) = self
            .require_render_fresh(canonical, client, &durable.payload)
            .await
        {
            self.record_render_stale(&mut durable, &error);
            return Err(error);
        }
        match durable.payload.status {
            RenderTaskStatus::Staged => {
                durable.payload.status = RenderTaskStatus::Submitting;
                durable.payload.error = None;
                durable = self.append_render(&durable.payload, Some(durable.generation))?;
                let task = match client.start_render(&durable.payload.request).await {
                    Ok(task) => task,
                    Err(error) => {
                        durable.payload.status = RenderTaskStatus::Failed;
                        durable.payload.error = Some(error.to_string());
                        let _ = self.append_render(&durable.payload, Some(durable.generation));
                        return Err(error.into());
                    }
                };
                self.record_render_task(canonical, client, durable, task)
                    .await
            }
            RenderTaskStatus::Submitting
            | RenderTaskStatus::Running
            | RenderTaskStatus::Finalizing
            | RenderTaskStatus::Cancelling => self.poll_render(canonical, client, task_id).await,
            status => Err(ProjectOperationError::InvalidRenderTransition(status)),
        }
    }

    /// Polls a persisted render, enforcing monotonic integer progress.
    ///
    /// # Errors
    ///
    /// Returns an error for stale source state, regressed/invalid progress,
    /// bridge failure, final-media mismatch, or corrupt persistence.
    pub async fn poll_render(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        task_id: Uuid,
    ) -> Result<DurableTaskRecord<RenderTaskRecord>, ProjectOperationError> {
        let mut durable = self.render_status(task_id)?;
        if durable.payload.status.is_terminal() {
            return Ok(durable);
        }
        if let Err(error) = self
            .require_render_fresh(canonical, client, &durable.payload)
            .await
        {
            self.record_render_stale(&mut durable, &error);
            return Err(error);
        }
        let task = client.render_task(task_id).await?;
        if let Some(previous) = &durable.payload.bridge_task
            && task.progress_basis_points < previous.progress_basis_points
        {
            return Err(ProjectOperationError::ProgressRegressed {
                previous: previous.progress_basis_points,
                current: task.progress_basis_points,
            });
        }
        self.record_render_task(canonical, client, durable, task)
            .await
    }

    /// Requests cooperative cancellation using all original source bindings.
    /// Cancellation remains available after staleness to stop external work.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/corrupt task, invalid binding, bridge
    /// cancellation failure, or persistence failure.
    pub async fn cancel_render(
        &self,
        client: &Ymm4BridgeClient,
        task_id: Uuid,
    ) -> Result<DurableTaskRecord<RenderTaskRecord>, ProjectOperationError> {
        require_bridge_feature(client, "project.render").await?;
        let mut durable = self.render_status(task_id)?;
        if durable.payload.status.is_terminal() {
            return Ok(durable);
        }
        durable.payload.status = RenderTaskStatus::Cancelling;
        durable = self.append_render(&durable.payload, Some(durable.generation))?;
        let request = (&durable.payload.request).into();
        let task = client.cancel_render(&request).await?;
        validate_render_task(&durable.payload.request, &task)?;
        durable.payload.status = map_bridge_render_status(task.status);
        durable.payload.error.clone_from(&task.error);
        durable.payload.bridge_task = Some(task);
        self.append_render(&durable.payload, Some(durable.generation))
    }

    /// Reads the latest validated render generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is missing or its journal is corrupt.
    pub fn render_status(
        &self,
        task_id: Uuid,
    ) -> Result<DurableTaskRecord<RenderTaskRecord>, ProjectOperationError> {
        self.load_latest(TaskKind::Render, &task_id.to_string())?
            .ok_or(ProjectOperationError::TaskNotFound(task_id))
    }

    /// Compares canonical expected items with a fresh managed-only projection.
    ///
    /// # Errors
    ///
    /// Returns an error for stale/corrupt canonical state, bridge read failure,
    /// invalid projections, or persistence failure.
    pub async fn stage_reconciliation_from_durable(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        source_revision: RevisionId,
    ) -> Result<DurableTaskRecord<ReconciliationTaskRecord>, ProjectOperationError> {
        require_revision(canonical, source_revision)?;
        let capabilities = client.structured_capabilities().await?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != canonical.project_id() {
            return Err(ProjectOperationError::CanonicalProjectMismatch {
                expected: canonical.project_id().into(),
                actual: snapshot.project_id,
            });
        }
        let source = reconciliation_source(source_revision, &capabilities, &snapshot)?;
        let expected_managed_state = canonical.managed_state_for_linked_target(
            &snapshot.project_id,
            &snapshot.scene_id,
            &source.target_identity_digest,
        )?;
        let actual = crate::managed_projection::project_snapshot(&snapshot);
        let report = SemanticDriftReport::build(source, expected_managed_state.clone(), actual)?;
        let payload = ReconciliationTaskRecord {
            report,
            expected_managed_state,
            preview: None,
            materialized_children: Vec::new(),
            status: ReconciliationTaskStatus::ReportReady,
        };
        self.append_reconciliation(&payload, None)
    }

    /// Binds exactly one import/detach/re-export choice to every drift entry.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/corrupt report, incomplete decisions, or
    /// invalid lifecycle/persistence state.
    pub fn preview_reconciliation(
        &self,
        report_digest: &str,
        decisions: Vec<ReconciliationDecision>,
    ) -> Result<DurableTaskRecord<ReconciliationTaskRecord>, ProjectOperationError> {
        let mut durable = self.reconciliation_status(report_digest)?;
        if durable.payload.status != ReconciliationTaskStatus::ReportReady {
            return Err(ProjectOperationError::InvalidReconciliationTransition(
                durable.payload.status,
            ));
        }
        durable.payload.preview = Some(ReconciliationPreview::build(
            &durable.payload.report,
            decisions,
        )?);
        durable.payload.status = ReconciliationTaskStatus::DecisionPreviewReady;
        self.append_reconciliation(&durable.payload, Some(durable.generation))
    }

    /// Re-hashes approval and re-reads source state, then materializes inert,
    /// durable child workflows. Import becomes an approval-waiting core patch;
    /// detach becomes an independently approvable metadata-only adapter plan;
    /// re-export becomes a staged handoff that still requires an explicit
    /// manifest and the existing exporter's own preview and approval. This
    /// method never performs any of those mutations.
    ///
    /// # Errors
    ///
    /// Returns an error for approval mutation, stale source/target state, bridge
    /// failure, invalid lifecycle state, or corrupt persistence.
    pub async fn accept_reconciliation(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        report_digest: &str,
        approved_digest: &str,
    ) -> Result<DurableTaskRecord<ReconciliationTaskRecord>, ProjectOperationError> {
        let mut durable = self.reconciliation_status(report_digest)?;
        let preview = durable
            .payload
            .preview
            .clone()
            .ok_or(ProjectOperationError::MissingReconciliationPreview)?;
        preview.verify_approval_digest(approved_digest)?;
        if durable.payload.status == ReconciliationTaskStatus::ActionsMaterialized {
            self.verify_materialized_children(&durable.payload, &preview)?;
            return Ok(durable);
        }
        if !matches!(
            durable.payload.status,
            ReconciliationTaskStatus::DecisionPreviewReady
                | ReconciliationTaskStatus::ActionsAccepted
        ) {
            return Err(ProjectOperationError::InvalidReconciliationTransition(
                durable.payload.status,
            ));
        }
        require_revision(canonical, durable.payload.report.source.source_revision)?;
        let capabilities = client.structured_capabilities().await?;
        let snapshot = client.snapshot().await?;
        let current_source = reconciliation_source(canonical.head()?, &capabilities, &snapshot)?;
        let current_expected = canonical.managed_state_for_linked_target(
            &snapshot.project_id,
            &snapshot.scene_id,
            &current_source.target_identity_digest,
        )?;
        if current_expected != durable.payload.expected_managed_state {
            durable.payload.status = ReconciliationTaskStatus::Stale;
            let _ = self.append_reconciliation(&durable.payload, Some(durable.generation));
            return Err(ProjectOperationError::DurableExpectedStateChanged);
        }
        let current_report = SemanticDriftReport::build(
            current_source.clone(),
            current_expected,
            crate::managed_projection::project_snapshot(&snapshot),
        )?;
        if durable.payload.report.is_stale_against(&current_source)
            || current_report.actual_managed_state_digest
                != durable.payload.report.actual_managed_state_digest
        {
            durable.payload.status = ReconciliationTaskStatus::Stale;
            let _ = self.append_reconciliation(&durable.payload, Some(durable.generation));
            return Err(ProjectOperationError::StaleTargetState);
        }
        durable.payload.materialized_children =
            self.materialize_reconciliation_children(&durable.payload.report, &preview)?;
        durable.payload.status = ReconciliationTaskStatus::ActionsMaterialized;
        self.append_reconciliation(&durable.payload, Some(durable.generation))
    }

    /// Reads the latest validated reconciliation generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the report is missing or its journal is corrupt.
    pub fn reconciliation_status(
        &self,
        report_digest: &str,
    ) -> Result<DurableTaskRecord<ReconciliationTaskRecord>, ProjectOperationError> {
        self.load_latest(TaskKind::Reconciliation, report_digest)?
            .ok_or_else(|| ProjectOperationError::ReconciliationNotFound(report_digest.into()))
    }

    /// Reads one validated reconciliation child generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the child is missing or its journal is corrupt.
    pub fn reconciliation_child_status(
        &self,
        child_task_id: &str,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        self.load_latest(TaskKind::ReconciliationChild, child_task_id)?
            .ok_or_else(|| ProjectOperationError::ReconciliationChildNotFound(child_task_id.into()))
    }

    /// Records a separate exact-digest approval for a metadata-detach child.
    /// Reconciliation approval alone is intentionally insufficient.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-detach child, stale canonical revision,
    /// mismatched digest, corrupt parent binding, or invalid lifecycle.
    pub fn approve_reconciliation_detach(
        &self,
        canonical: &DurableProjectStore,
        child_task_id: &str,
        approved_digest: &str,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let mut durable = self.validated_reconciliation_child(child_task_id)?;
        let ReconciliationChildTask::MetadataDetach(draft) = &mut durable.payload else {
            return Err(ProjectOperationError::WrongReconciliationChildKind);
        };
        require_revision(canonical, draft.source.source_revision)?;
        if approved_digest != draft.patch.digest {
            return Err(ProjectOperationError::ReconciliationChildApprovalMismatch);
        }
        if draft.status == ReconciliationDetachStatus::Approved {
            draft.patch.authorize_commit(draft.source.source_revision)?;
            return Ok(durable);
        }
        if draft.status != ReconciliationDetachStatus::PreviewReady {
            return Err(ProjectOperationError::InvalidDetachTransition(draft.status));
        }
        draft.patch.approve()?;
        draft.status = ReconciliationDetachStatus::Approved;
        self.append(
            TaskKind::ReconciliationChild,
            child_task_id,
            &durable.payload,
            Some(durable.generation),
        )
    }

    /// Executes an approved Remark-only detach through the bridge's WAL route.
    /// The service stores the exact request before I/O, authenticates the
    /// receipt, requires a fresh semantic read-back with the identity gone,
    /// then CAS-removes canonical ownership and advances exactly one revision.
    ///
    /// # Errors
    ///
    /// Returns an error for missing independent approval, source/target drift,
    /// unsupported identity, bridge recovery state, unbound receipt, failed
    /// preservation proof, or corrupt durable state.
    // This deliberately remains one linear, auditable state machine: request
    // persistence must precede bridge I/O and terminal persistence must follow
    // receipt/read-back validation without hidden transition paths.
    #[allow(clippy::too_many_lines)]
    pub async fn execute_reconciliation_detach(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        child_task_id: &str,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let mut durable = self.validated_reconciliation_child(child_task_id)?;
        if reissuable_detach_terminal(&durable.payload) {
            let terminal_error = metadata_detach_terminal_error(&durable.payload);
            let reissued = self.complete_persisted_detach_reissue(canonical, durable)?;
            return Err(ProjectOperationError::MetadataDetachNotVerified(
                ReconciliationDetachStatus::PreviewReady,
                terminal_error.or_else(|| {
                    Some(format!(
                        "Metadata detach replacement attempt {} requires new approval",
                        metadata_detach_operation_id(&reissued.payload)
                    ))
                }),
            ));
        }
        let ReconciliationChildTask::MetadataDetach(draft) = &mut durable.payload else {
            return Err(ProjectOperationError::WrongReconciliationChildKind);
        };
        let expected_committed_revision = draft
            .source
            .source_revision
            .checked_next()
            .ok_or(PatchError::RevisionOverflow)?;
        let canonical_state = canonical.snapshot()?;
        let existing_commit = canonical_state.external_commits.get(&draft.operation_id);
        if draft.status == ReconciliationDetachStatus::Verified {
            if draft.committed_revision != Some(expected_committed_revision)
                || !existing_commit.is_some_and(|record| {
                    record.base_revision == draft.source.source_revision
                        && record.committed_revision == expected_committed_revision
                })
            {
                return Err(ProjectOperationError::InvalidReconciliationChildren);
            }
        } else if canonical_state.head != draft.source.source_revision
            && !existing_commit.is_some_and(|record| {
                record.base_revision == draft.source.source_revision
                    && record.committed_revision == expected_committed_revision
            })
        {
            require_revision(canonical, draft.source.source_revision)?;
        }
        draft.patch.authorize_commit(draft.source.source_revision)?;
        if draft.contract.bridge_wire_status != ReconciliationBridgeWireStatus::Available {
            return Err(ProjectOperationError::ReconciliationDetachUnavailable);
        }
        if draft.status == ReconciliationDetachStatus::Approved {
            let capabilities = client.structured_capabilities().await?;
            require_detach_capability(&capabilities)?;
            let snapshot = client.snapshot().await?;
            require_reconciliation_target(&draft.source, &capabilities, &snapshot)?;
            let realization_id = draft
                .identity
                .realization_id
                .ok_or(ProjectOperationError::DetachRequiresRemarkIdentity)?;
            require_observed_identity_unchanged(draft, &snapshot)?;
            let request = Ymm4MetadataDetachRequest::try_new(Ymm4MetadataDetachRequestInput {
                operation_id: draft.operation_id,
                project_id: draft.source.project_id.clone(),
                scene_id: draft.source.scene_id.clone(),
                source_revision: draft.source.source_revision.0,
                expected_fingerprint: snapshot.fingerprint,
                entity_id: draft.identity.entity_id.clone(),
                realization_id,
            })?;
            draft.request = Some(request);
            draft.committed_revision = None;
            draft.status = ReconciliationDetachStatus::Applying;
            draft.error = None;
            durable = self.append(
                TaskKind::ReconciliationChild,
                child_task_id,
                &durable.payload,
                Some(durable.generation),
            )?;
        } else if !matches!(
            draft.status,
            ReconciliationDetachStatus::Applying
                | ReconciliationDetachStatus::RecoveryRequired
                | ReconciliationDetachStatus::Verified
        ) {
            return Err(ProjectOperationError::InvalidDetachTransition(draft.status));
        }

        let ReconciliationChildTask::MetadataDetach(draft) = &mut durable.payload else {
            unreachable!("child kind was checked before request persistence");
        };
        let request = draft
            .request
            .clone()
            .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;

        // Request durability precedes both the process-scoped exclusion and
        // the canonical reservation. The guard then spans every bridge call,
        // fresh read-back, and canonical finalize/rollback decision.
        let _mutation_fence = canonical.acquire_external_mutation_fence(draft.operation_id)?;
        let fenced_state = canonical.snapshot()?;
        let existing_commit = fenced_state
            .external_commits
            .get(&draft.operation_id)
            .cloned();
        let has_existing_reservation = fenced_state
            .pending_external_commit
            .as_ref()
            .is_some_and(|reservation| reservation.operation_id == draft.operation_id);
        if existing_commit.is_some() {
            // A canonical commit can outlive the task-journal response that
            // followed it. Recover from the immutable historical record and
            // authenticated exact bridge replay without requiring today's
            // capabilities, fingerprint, or target head to equal the old
            // post-detach state; later verified operations may have advanced
            // all three.
            let replay = client.detach_managed_metadata(&request).await?;
            if !replay.success || !replay.replayed {
                return Err(ProjectOperationError::MetadataDetachReplayMismatch);
            }
            verify_metadata_detach(&request, &replay.receipt)?;
            if draft
                .receipt
                .as_ref()
                .is_some_and(|receipt| receipt != &replay.receipt)
            {
                return Err(ProjectOperationError::MetadataDetachReplayMismatch);
            }
            let committed_revision = canonical.verify_committed_metadata_detach(
                draft.operation_id,
                draft.source.source_revision,
                &draft.patch.digest,
                &request.request_digest,
                &draft.source.project_id,
                &draft.source.scene_id,
                &draft.source.target_identity_digest,
                &draft.identity,
                &replay.receipt,
            )?;
            if committed_revision != expected_committed_revision {
                return Err(ProjectOperationError::InvalidReconciliationChildren);
            }
            if draft.status == ReconciliationDetachStatus::Verified
                && draft.committed_revision == Some(committed_revision)
                && draft.receipt.as_ref() == Some(&replay.receipt)
                && draft.error.is_none()
            {
                return Ok(durable);
            }
            draft.receipt = Some(replay.receipt);
            draft.committed_revision = Some(committed_revision);
            draft.status = ReconciliationDetachStatus::Verified;
            draft.error = None;
            return self.append(
                TaskKind::ReconciliationChild,
                child_task_id,
                &durable.payload,
                Some(durable.generation),
            );
        }

        let mut recover_pending_apply = false;
        let mut sealed_pending_response = None;
        if has_existing_reservation {
            canonical.verify_metadata_detach_reservation(
                draft.operation_id,
                draft.source.source_revision,
                &draft.patch.digest,
                &request.request_digest,
                &draft.source.project_id,
                &draft.source.scene_id,
                &draft.source.target_identity_digest,
                &draft.identity,
            )?;
            match client.metadata_detach_operation(draft.operation_id).await {
                Ok(receipt) if receipt.status == Ymm4MetadataDetachStatus::Verified => {
                    verify_metadata_detach(&request, &receipt)?;
                    let committed_revision = canonical.finalize_reserved_metadata_detach(
                        draft.operation_id,
                        draft.source.source_revision,
                        &draft.patch.digest,
                        &request.request_digest,
                        &draft.source.project_id,
                        &draft.source.scene_id,
                        &draft.source.target_identity_digest,
                        &draft.identity,
                        &receipt,
                    )?;
                    draft.receipt = Some(receipt);
                    draft.committed_revision = Some(committed_revision);
                    draft.status = ReconciliationDetachStatus::Verified;
                    draft.error = None;
                    return self.append(
                        TaskKind::ReconciliationChild,
                        child_task_id,
                        &durable.payload,
                        Some(durable.generation),
                    );
                }
                Ok(receipt) if receipt.status == Ymm4MetadataDetachStatus::RolledBack => {
                    takegraph_node::verify_metadata_detach_rollback(&request, &receipt)?;
                    let terminal = self.persist_detach_terminal(durable, receipt)?;
                    let error = metadata_detach_terminal_error(&terminal.payload);
                    let reissued =
                        self.complete_persisted_detach_reissue_under_fence(canonical, terminal)?;
                    return Err(ProjectOperationError::MetadataDetachNotVerified(
                        ReconciliationDetachStatus::PreviewReady,
                        error.or_else(|| {
                            Some(format!(
                                "Metadata detach rolled back; replacement attempt {} requires new approval",
                                metadata_detach_operation_id(&reissued.payload)
                            ))
                        }),
                    ));
                }
                Ok(receipt) if receipt.status == Ymm4MetadataDetachStatus::NotStarted => {
                    verify_metadata_detach_not_started(&request, &receipt)?;
                    let terminal = self.persist_detach_terminal(durable, receipt)?;
                    let error = metadata_detach_terminal_error(&terminal.payload);
                    let reissued =
                        self.complete_persisted_detach_reissue_under_fence(canonical, terminal)?;
                    return Err(ProjectOperationError::MetadataDetachNotVerified(
                        ReconciliationDetachStatus::PreviewReady,
                        error.or_else(|| {
                            Some(format!(
                                "Metadata detach did not start; replacement attempt {} requires new approval",
                                metadata_detach_operation_id(&reissued.payload)
                            ))
                        }),
                    ));
                }
                Ok(receipt)
                    if matches!(
                        receipt.status,
                        Ymm4MetadataDetachStatus::Failed
                            | Ymm4MetadataDetachStatus::RecoveryRequired
                    ) =>
                {
                    draft.receipt = Some(receipt.clone());
                    draft.status = map_metadata_detach_status(receipt.status);
                    draft.error = receipt.error;
                    let status = draft.status;
                    let error = draft.error.clone();
                    let _ = self.append(
                        TaskKind::ReconciliationChild,
                        child_task_id,
                        &durable.payload,
                        Some(durable.generation),
                    )?;
                    return Err(ProjectOperationError::MetadataDetachNotVerified(
                        status, error,
                    ));
                }
                Ok(receipt) => {
                    // Applying must recover through the exact idempotent POST.
                    if receipt.status != Ymm4MetadataDetachStatus::Applying {
                        return Err(ProjectOperationError::MetadataDetachResponseInconsistent);
                    }
                    recover_pending_apply = true;
                }
                Err(Ymm4Error::Bridge { status, .. }) if status.as_u16() == 404 => {
                    // No bridge WAL exists. Do not execute an approval under a
                    // potentially changed capability contract. Instead seal a
                    // no-mutation tombstone under the bridge apply gate. A
                    // delayed old POST then replays the tombstone, while a WAL
                    // that won the race is returned here for exact recovery.
                    let sealed = client.seal_metadata_detach_not_started(&request).await?;
                    recover_pending_apply = true;
                    if sealed.receipt.status == Ymm4MetadataDetachStatus::Applying {
                        // A delayed apply won the gate and published its WAL;
                        // recover it through the exact idempotent POST below.
                    } else {
                        sealed_pending_response = Some(sealed);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }

        let live_context = if recover_pending_apply {
            None
        } else {
            let capabilities = client.structured_capabilities().await?;
            require_detach_capability(&capabilities)?;
            let snapshot = client.snapshot().await?;
            require_reconciliation_target(&draft.source, &capabilities, &snapshot)?;
            Some((capabilities, snapshot))
        };

        if !recover_pending_apply {
            let (capabilities, snapshot) = live_context
                .as_ref()
                .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;
            // With no pre-existing durable reservation, no target I/O could
            // have happened under this protocol. Recheck the exact approved
            // preimage before publishing the reservation.
            if !has_existing_reservation {
                if snapshot.fingerprint != request.expected_fingerprint {
                    return Err(ProjectOperationError::StaleTargetState);
                }
                require_observed_identity_unchanged(draft, snapshot)?;
            }
            let target = crate::ymm4_target_plan::ymm4_target_identity(capabilities, snapshot);
            canonical.reserve_metadata_detach(
                draft.operation_id,
                draft.source.source_revision,
                &draft.patch.digest,
                &request.request_digest,
                &crate::VerifiedTargetBinding {
                    adapter_id: target.adapter_id,
                    target_project_id: draft.source.project_id.clone(),
                    scene_id: draft.source.scene_id.clone(),
                    target_identity_digest: draft.source.target_identity_digest.clone(),
                    verified_fingerprint: snapshot.fingerprint.clone(),
                },
                &draft.identity,
            )?;
        }

        let apply_result = match sealed_pending_response {
            Some(response) => Ok(response),
            None => client.detach_managed_metadata(&request).await,
        };
        let response = match apply_result {
            Ok(response) => response,
            Err(apply_error) => {
                // The recovery seal is serialized with apply and durably
                // tombstones an operation only when no WAL exists. The sealed
                // immutable request carries the approved fingerprint, but the
                // no-start proof deliberately does not require today's target
                // to still match it. If apply actually
                // reached a receipt, the same endpoint replays that receipt.
                match client.seal_metadata_detach_not_started(&request).await {
                    Ok(response) => response,
                    Err(recovery_error) => {
                        draft.error = Some(format!(
                            "{apply_error}; no-mutation recovery proof failed: {recovery_error}"
                        ));
                        let _ = self.append(
                            TaskKind::ReconciliationChild,
                            child_task_id,
                            &durable.payload,
                            Some(durable.generation),
                        );
                        return Err(apply_error.into());
                    }
                }
            }
        };
        let response_is_consistent = if response.success {
            response.receipt.status == Ymm4MetadataDetachStatus::Verified
                && response.receipt.verified
        } else {
            response.receipt.status != Ymm4MetadataDetachStatus::Verified
                && !response.receipt.verified
                && response.receipt.error.is_some()
        };
        if !response_is_consistent {
            // Never let a contradictory wire envelope promote the durable
            // child to Verified. Keep the exact request in Applying so an
            // authenticated retry can recover the bridge's actual outcome.
            draft.error = Some(
                "YMM4 metadata detach response success disagreed with its verified receipt".into(),
            );
            let _ = self.append(
                TaskKind::ReconciliationChild,
                child_task_id,
                &durable.payload,
                Some(durable.generation),
            );
            return Err(ProjectOperationError::MetadataDetachResponseInconsistent);
        }
        if response.receipt.status == Ymm4MetadataDetachStatus::Applying {
            draft.receipt = None;
            draft.status = ReconciliationDetachStatus::Applying;
            draft.error = Some(
                response
                    .receipt
                    .error
                    .clone()
                    .unwrap_or_else(|| "YMM4 metadata detach is still applying".into()),
            );
            self.append(
                TaskKind::ReconciliationChild,
                child_task_id,
                &durable.payload,
                Some(durable.generation),
            )?;
            return Err(ProjectOperationError::MetadataDetachNotVerified(
                ReconciliationDetachStatus::Applying,
                Some("YMM4 metadata detach is still applying".into()),
            ));
        }
        if response.receipt.status == Ymm4MetadataDetachStatus::RolledBack {
            takegraph_node::verify_metadata_detach_rollback(&request, &response.receipt)?;
            let terminal = self.persist_detach_terminal(durable, response.receipt)?;
            let error = metadata_detach_terminal_error(&terminal.payload);
            let _ = self.complete_persisted_detach_reissue_under_fence(canonical, terminal)?;
            return Err(ProjectOperationError::MetadataDetachNotVerified(
                ReconciliationDetachStatus::PreviewReady,
                error.or_else(|| {
                    Some(
                        "Metadata detach rolled back; the replacement attempt requires new approval"
                            .into(),
                    )
                }),
            ));
        }
        if response.receipt.status == Ymm4MetadataDetachStatus::NotStarted {
            verify_metadata_detach_not_started(&request, &response.receipt)?;
            let terminal = self.persist_detach_terminal(durable, response.receipt)?;
            let error = metadata_detach_terminal_error(&terminal.payload);
            let _ = self.complete_persisted_detach_reissue_under_fence(canonical, terminal)?;
            return Err(ProjectOperationError::MetadataDetachNotVerified(
                ReconciliationDetachStatus::PreviewReady,
                error.or_else(|| {
                    Some(
                        "Metadata detach did not start; the replacement attempt requires new approval"
                            .into(),
                    )
                }),
            ));
        }
        draft.receipt = Some(response.receipt.clone());
        draft.error.clone_from(&response.receipt.error);
        draft.status = map_metadata_detach_status(response.receipt.status);

        if response.success {
            verify_metadata_detach(&request, &response.receipt)?;
            if recover_pending_apply {
                match canonical.finalize_reserved_metadata_detach(
                    draft.operation_id,
                    draft.source.source_revision,
                    &draft.patch.digest,
                    &request.request_digest,
                    &draft.source.project_id,
                    &draft.source.scene_id,
                    &draft.source.target_identity_digest,
                    &draft.identity,
                    &response.receipt,
                ) {
                    Ok(committed_revision) => {
                        draft.committed_revision = Some(committed_revision);
                        draft.status = ReconciliationDetachStatus::Verified;
                        draft.error = None;
                    }
                    Err(error) => {
                        draft.status = ReconciliationDetachStatus::Applying;
                        draft.error = Some(error.to_string());
                    }
                }
            } else {
                let (capabilities, _) = live_context
                    .as_ref()
                    .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;
                let fresh = client.snapshot().await?;
                require_reconciliation_target(&draft.source, capabilities, &fresh)?;
                if fresh.fingerprint != response.receipt.after_fingerprint {
                    draft.status = ReconciliationDetachStatus::RecoveryRequired;
                    draft.error = Some(
                        "YMM4 changed between metadata detach receipt and fresh read-back".into(),
                    );
                } else if let Err(error) = require_detached_identity_absent(&draft.identity, &fresh)
                {
                    draft.status = ReconciliationDetachStatus::RecoveryRequired;
                    draft.error = Some(error.to_string());
                } else {
                    match finalize_reconciliation_detach(
                        canonical,
                        draft,
                        capabilities,
                        &fresh,
                        &response.receipt,
                    ) {
                        Ok(committed_revision) => {
                            draft.committed_revision = Some(committed_revision);
                            draft.status = ReconciliationDetachStatus::Verified;
                            draft.error = None;
                        }
                        Err(error) => {
                            // The target is already detached but canonical ownership
                            // finalization did not commit. Retain the authenticated
                            // receipt and Applying lifecycle so exact retry can CAS
                            // the same operation without another YMM4 mutation.
                            draft.status = ReconciliationDetachStatus::Applying;
                            draft.error = Some(error.to_string());
                        }
                    }
                }
            }
        }
        let terminal = self.append(
            TaskKind::ReconciliationChild,
            child_task_id,
            &durable.payload,
            Some(durable.generation),
        )?;
        let ReconciliationChildTask::MetadataDetach(draft) = &terminal.payload else {
            unreachable!("child kind was checked before terminal persistence");
        };
        if draft.status != ReconciliationDetachStatus::Verified {
            return Err(ProjectOperationError::MetadataDetachNotVerified(
                draft.status,
                draft.error.clone(),
            ));
        }
        Ok(terminal)
    }

    fn validated_reconciliation_child(
        &self,
        child_task_id: &str,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let durable = self.reconciliation_child_status(child_task_id)?;
        let (report_digest, entry_id) = match &durable.payload {
            ReconciliationChildTask::ImportPatch(draft) => (&draft.report_digest, &draft.entry_id),
            ReconciliationChildTask::MetadataDetach(draft) => {
                (&draft.report_digest, &draft.entry_id)
            }
            ReconciliationChildTask::CanonicalReExport(task) => {
                (&task.report_digest, &task.entry_id)
            }
        };
        let parent = self.reconciliation_status(report_digest)?;
        if parent.payload.status != ReconciliationTaskStatus::ActionsMaterialized {
            return Err(ProjectOperationError::InvalidReconciliationChildren);
        }
        let preview = parent
            .payload
            .preview
            .as_ref()
            .ok_or(ProjectOperationError::MissingReconciliationPreview)?;
        let action = preview
            .actions
            .iter()
            .find(|action| reconciliation_action_entry_id(action) == entry_id)
            .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;
        let action_digest = canonical_sha256(
            "takegraph-reconciliation-child-action-v1",
            &(
                &parent.payload.report.report_digest,
                &parent.payload.report.source,
                action,
            ),
        )?;
        let expected_child_id = canonical_sha256(
            "takegraph-reconciliation-child-task-id-v1",
            &(
                &parent.payload.report.report_digest,
                entry_id,
                &action_digest,
            ),
        )?;
        if expected_child_id != child_task_id
            || !parent
                .payload
                .materialized_children
                .iter()
                .any(|reference| {
                    reference.child_task_id == child_task_id
                        && reference.entry_id == *entry_id
                        && reference.action_digest == action_digest
                })
        {
            return Err(ProjectOperationError::InvalidReconciliationChildren);
        }
        validate_reconciliation_child(
            &durable.payload,
            child_task_id,
            &action_digest,
            &parent.payload.report,
            action,
        )?;
        Ok(durable)
    }

    fn reissue_metadata_detach_attempt(
        &self,
        mut durable: DurableTaskRecord<ReconciliationChildTask>,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let (report_digest, entry_id, child_task_id) = match &durable.payload {
            ReconciliationChildTask::MetadataDetach(draft) => (
                draft.report_digest.clone(),
                draft.entry_id.clone(),
                draft.child_task_id.clone(),
            ),
            _ => return Err(ProjectOperationError::WrongReconciliationChildKind),
        };
        let parent = self.reconciliation_status(&report_digest)?;
        let preview = parent
            .payload
            .preview
            .as_ref()
            .ok_or(ProjectOperationError::MissingReconciliationPreview)?;
        let action = preview
            .actions
            .iter()
            .find(|action| reconciliation_action_entry_id(action) == entry_id)
            .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;
        let action_digest = canonical_sha256(
            "takegraph-reconciliation-child-action-v1",
            &(
                &parent.payload.report.report_digest,
                &parent.payload.report.source,
                action,
            ),
        )?;
        let ReconciliationChildTask::MetadataDetach(draft) = &mut durable.payload else {
            unreachable!("child kind was checked above");
        };
        reissue_detach_attempt(draft, &action_digest, &parent.payload.report)?;
        self.append(
            TaskKind::ReconciliationChild,
            &child_task_id,
            &durable.payload,
            Some(durable.generation),
        )
    }

    fn persist_detach_terminal(
        &self,
        mut durable: DurableTaskRecord<ReconciliationChildTask>,
        receipt: Ymm4MetadataDetachReceipt,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let ReconciliationChildTask::MetadataDetach(draft) = &mut durable.payload else {
            return Err(ProjectOperationError::WrongReconciliationChildKind);
        };
        match receipt.status {
            Ymm4MetadataDetachStatus::RolledBack => {
                draft.status = ReconciliationDetachStatus::RolledBack;
            }
            Ymm4MetadataDetachStatus::NotStarted => {
                draft.status = ReconciliationDetachStatus::Failed;
            }
            _ => return Err(ProjectOperationError::MetadataDetachResponseInconsistent),
        }
        draft.error.clone_from(&receipt.error);
        draft.receipt = Some(receipt);
        draft.committed_revision = None;
        self.append(
            TaskKind::ReconciliationChild,
            &draft.child_task_id.clone(),
            &durable.payload,
            Some(durable.generation),
        )
    }

    fn complete_persisted_detach_reissue(
        &self,
        canonical: &DurableProjectStore,
        durable: DurableTaskRecord<ReconciliationChildTask>,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let operation_id = metadata_detach_operation_id(&durable.payload);
        let _mutation_fence = canonical.acquire_external_mutation_fence(operation_id)?;
        self.complete_persisted_detach_reissue_under_fence(canonical, durable)
    }

    fn complete_persisted_detach_reissue_under_fence(
        &self,
        canonical: &DurableProjectStore,
        durable: DurableTaskRecord<ReconciliationChildTask>,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let ReconciliationChildTask::MetadataDetach(draft) = &durable.payload else {
            return Err(ProjectOperationError::WrongReconciliationChildKind);
        };
        let request = draft
            .request
            .as_ref()
            .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;
        let receipt = draft
            .receipt
            .as_ref()
            .ok_or(ProjectOperationError::InvalidReconciliationChildren)?;
        match receipt.status {
            Ymm4MetadataDetachStatus::RolledBack => {
                takegraph_node::verify_metadata_detach_rollback(request, receipt)?;
            }
            Ymm4MetadataDetachStatus::NotStarted => {
                verify_metadata_detach_not_started(request, receipt)?;
            }
            _ => return Err(ProjectOperationError::MetadataDetachResponseInconsistent),
        }
        canonical.abort_metadata_detach_reservation(draft.operation_id, &request.request_digest)?;
        self.reissue_metadata_detach_attempt(durable)
    }

    /// Dispatches a canonical re-export child into its existing exporter and
    /// persists only that exporter's ordinary unapproved preview. This method
    /// never approves or applies the downstream task.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest route/semantics differ from the
    /// canonical child, source or target drifted, a dependency cannot be
    /// proven, exporter staging fails, or the child journal is corrupt.
    #[allow(clippy::too_many_lines)] // Keeps the three route projections and shared approval gate together.
    pub async fn dispatch_reconciliation_re_export(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        child_task_id: &str,
        manifest: ReconciliationReExportManifest,
    ) -> Result<DurableTaskRecord<ReconciliationChildTask>, ProjectOperationError> {
        let mut durable = self.validated_reconciliation_child(child_task_id)?;
        let ReconciliationChildTask::CanonicalReExport(handoff) = &mut durable.payload else {
            return Err(ProjectOperationError::WrongReconciliationChildKind);
        };
        require_revision(canonical, handoff.source.source_revision)?;
        let manifest_digest = canonical_sha256(
            "takegraph-reconciliation-re-export-dispatch-v1",
            &(&handoff.handoff_digest, &manifest),
        )?;
        if handoff.status == ReconciliationReExportStatus::PreviewReady {
            if handoff.dispatch_manifest_digest.as_deref() != Some(manifest_digest.as_str()) {
                return Err(ProjectOperationError::ReExportManifestReplayMismatch);
            }
            return Ok(durable);
        }
        if handoff.status != ReconciliationReExportStatus::AwaitingManifest {
            return Err(ProjectOperationError::InvalidReExportTransition(
                handoff.status,
            ));
        }
        if manifest_route(&manifest) != handoff.exporter_route {
            return Err(ProjectOperationError::ReExportRouteMismatch);
        }
        let capabilities = client.structured_capabilities().await?;
        let snapshot = client.snapshot().await?;
        require_reconciliation_target(&handoff.source, &capabilities, &snapshot)?;

        let preview = match manifest {
            ReconciliationReExportManifest::PortablePair { utterances } => {
                require_re_export_projection(
                    &handoff.source,
                    &handoff.canonical,
                    project_portable_manifest(&utterances),
                )?;
                ReconciliationDownstreamPreview::PortablePair(
                    Ymm4ExportPatch::stage_from_snapshot_with_operation_id(
                        client,
                        handoff.source.source_revision,
                        snapshot,
                        utterances,
                        handoff.downstream_task_id,
                    )
                    .await?,
                )
            }
            ReconciliationReExportManifest::NativeVoiceMutation { mutations } => {
                require_native_voice_re_export_scope(
                    &handoff.identity,
                    &handoff.canonical,
                    &mutations,
                )?;
                require_re_export_projection(
                    &handoff.source,
                    &handoff.canonical,
                    project_native_voice_manifest(&mutations),
                )?;
                ReconciliationDownstreamPreview::NativeVoiceMutation(
                    Ymm4NativeVoiceMutationPatch::stage_from_snapshot_with_operation_id(
                        client,
                        handoff.source.source_revision,
                        snapshot,
                        mutations,
                        handoff.downstream_task_id,
                    )
                    .await?,
                )
            }
            ReconciliationReExportManifest::NativeExtension {
                manifest,
                artifact_root,
            } => {
                let identity_overrides =
                    native_extension_identity_overrides(&handoff.identity, &manifest)?;
                let task = Ymm4NativeExtensionTask::stage_with_operation_id_and_identity_overrides(
                    client,
                    handoff.source.source_revision,
                    manifest,
                    artifact_root,
                    handoff.downstream_task_id,
                    identity_overrides,
                )
                .await?;
                require_re_export_projection(
                    &handoff.source,
                    &handoff.canonical,
                    project_native_extension_preview(&task)?,
                )?;
                ReconciliationDownstreamPreview::NativeExtension(Box::new(task))
            }
        };
        if downstream_preview_patch(&preview).status != PatchStatus::Previewable
            || downstream_preview_patch(&preview).approved_digest.is_some()
            || downstream_preview_patch(&preview).base != handoff.source.source_revision
            || downstream_preview_operation_id(&preview) != handoff.downstream_task_id
        {
            return Err(ProjectOperationError::DownstreamPreviewWasApproved);
        }
        handoff.dispatch_manifest_digest = Some(manifest_digest);
        handoff.downstream_preview = Some(preview);
        handoff.status = ReconciliationReExportStatus::PreviewReady;
        handoff.error = None;
        self.append(
            TaskKind::ReconciliationChild,
            child_task_id,
            &durable.payload,
            Some(durable.generation),
        )
    }

    fn materialize_reconciliation_children(
        &self,
        report: &SemanticDriftReport,
        preview: &ReconciliationPreview,
    ) -> Result<Vec<ReconciliationChildReference>, ProjectOperationError> {
        let mut children = Vec::with_capacity(preview.actions.len());
        for action in &preview.actions {
            let entry_id = reconciliation_action_entry_id(action);
            let action_digest = canonical_sha256(
                "takegraph-reconciliation-child-action-v1",
                &(&report.report_digest, &report.source, action),
            )?;
            let child_task_id = canonical_sha256(
                "takegraph-reconciliation-child-task-id-v1",
                &(&report.report_digest, entry_id, &action_digest),
            )?;
            let durable = if let Some(existing) = self.load_latest::<ReconciliationChildTask>(
                TaskKind::ReconciliationChild,
                &child_task_id,
            )? {
                existing
            } else {
                let task =
                    build_reconciliation_child(&child_task_id, &action_digest, report, action)?;
                match self.append(TaskKind::ReconciliationChild, &child_task_id, &task, None) {
                    Ok(record) => record,
                    Err(ProjectOperationError::ConcurrentJournalUpdate { .. }) => self
                        .load_latest(TaskKind::ReconciliationChild, &child_task_id)?
                        .ok_or_else(|| {
                            ProjectOperationError::ReconciliationChildNotFound(
                                child_task_id.clone(),
                            )
                        })?,
                    Err(error) => return Err(error),
                }
            };
            children.push(validate_reconciliation_child(
                &durable.payload,
                &child_task_id,
                &action_digest,
                report,
                action,
            )?);
        }
        Ok(children)
    }

    fn verify_materialized_children(
        &self,
        reconciliation: &ReconciliationTaskRecord,
        preview: &ReconciliationPreview,
    ) -> Result<(), ProjectOperationError> {
        if reconciliation.materialized_children.len() != preview.actions.len() {
            return Err(ProjectOperationError::InvalidReconciliationChildren);
        }
        let expected = self.materialize_reconciliation_children(&reconciliation.report, preview)?;
        if expected.len() != reconciliation.materialized_children.len()
            || expected
                .iter()
                .zip(&reconciliation.materialized_children)
                .any(|(current, stored)| !same_stable_child_reference(current, stored))
        {
            return Err(ProjectOperationError::InvalidReconciliationChildren);
        }
        Ok(())
    }

    async fn record_render_task(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        mut durable: DurableTaskRecord<RenderTaskRecord>,
        task: Ymm4RenderTask,
    ) -> Result<DurableTaskRecord<RenderTaskRecord>, ProjectOperationError> {
        validate_render_task(&durable.payload.request, &task)?;
        let bridge_status = task.status;
        durable.payload.status = map_bridge_render_status(bridge_status);
        durable.payload.error = task.error.clone();
        durable.payload.bridge_task = Some(task);
        durable = self.append_render(&durable.payload, Some(durable.generation))?;
        if bridge_status != Ymm4RenderStatus::Succeeded {
            return Ok(durable);
        }
        let media = match (|| -> Result<VerifiedMediaProbe, ProjectOperationError> {
            let media = verify_rendered_media(
                &durable.payload.request,
                durable
                    .payload
                    .bridge_task
                    .as_ref()
                    .ok_or(ProjectOperationError::MissingBridgeTask)?,
            )?;
            verify_media_profile(&durable.payload.profile, &media)?;
            Ok(media)
        })() {
            Ok(media) => media,
            Err(error) => {
                durable.payload.status = RenderTaskStatus::RecoveryRequired;
                durable.payload.error = Some(error.to_string());
                self.append_render(&durable.payload, Some(durable.generation))?;
                return Err(error);
            }
        };
        if let Err(error) = self
            .require_render_fresh(canonical, client, &durable.payload)
            .await
        {
            self.record_render_stale(&mut durable, &error);
            return Err(error);
        }
        durable.payload.status = RenderTaskStatus::Verified;
        durable.payload.verified_media = Some(media);
        durable.payload.error = None;
        self.append_render(&durable.payload, Some(durable.generation))
    }

    fn record_checkpoint_stale(
        &self,
        durable: &mut DurableTaskRecord<CheckpointTaskRecord>,
        error: &ProjectOperationError,
    ) {
        if !error.is_stale() {
            return;
        }
        durable.payload.status = CheckpointTaskStatus::Stale;
        durable.payload.error = Some(error.to_string());
        let _ = self.append_checkpoint(&durable.payload, Some(durable.generation));
    }

    fn record_render_stale(
        &self,
        durable: &mut DurableTaskRecord<RenderTaskRecord>,
        error: &ProjectOperationError,
    ) {
        if !error.is_stale() {
            return;
        }
        durable.payload.status = RenderTaskStatus::Stale;
        durable.payload.error = Some(error.to_string());
        let _ = self.append_render(&durable.payload, Some(durable.generation));
    }

    async fn require_checkpoint_fresh(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        payload: &CheckpointTaskRecord,
    ) -> Result<(), ProjectOperationError> {
        require_bridge_feature(client, "project.checkpoint").await?;
        require_revision(canonical, RevisionId(payload.request.source_revision))?;
        let health = client.health().await?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != payload.request.project_id
            || snapshot.scene_id != payload.request.scene_id
            || snapshot.fingerprint != payload.request.expected_state_digest
            || snapshot.project_path != payload.expected_project_path
            || target_identity_digest(&health.ymm4_version, &snapshot)?
                != payload.request.target_identity_digest
        {
            return Err(ProjectOperationError::StaleTargetState);
        }
        Ok(())
    }

    async fn require_render_fresh(
        &self,
        canonical: &DurableProjectStore,
        client: &Ymm4BridgeClient,
        payload: &RenderTaskRecord,
    ) -> Result<(), ProjectOperationError> {
        require_bridge_feature(client, "project.render").await?;
        require_revision(canonical, RevisionId(payload.request.source_revision))?;
        verify_render_checkpoint_file(&payload.request)?;
        let health = client.health().await?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != payload.request.project_id
            || snapshot.scene_id != payload.request.scene_id
            || snapshot.fingerprint != payload.request.expected_state_digest
            || target_identity_digest(&health.ymm4_version, &snapshot)?
                != payload.request.target_identity_digest
        {
            return Err(ProjectOperationError::StaleTargetState);
        }
        Ok(())
    }

    fn append_checkpoint(
        &self,
        payload: &CheckpointTaskRecord,
        generation: Option<u64>,
    ) -> Result<DurableTaskRecord<CheckpointTaskRecord>, ProjectOperationError> {
        self.append(
            TaskKind::Checkpoint,
            &payload.request.operation_id.to_string(),
            payload,
            generation,
        )
    }

    fn append_render(
        &self,
        payload: &RenderTaskRecord,
        generation: Option<u64>,
    ) -> Result<DurableTaskRecord<RenderTaskRecord>, ProjectOperationError> {
        self.append(
            TaskKind::Render,
            &payload.request.task_id.to_string(),
            payload,
            generation,
        )
    }

    fn append_reconciliation(
        &self,
        payload: &ReconciliationTaskRecord,
        generation: Option<u64>,
    ) -> Result<DurableTaskRecord<ReconciliationTaskRecord>, ProjectOperationError> {
        self.append(
            TaskKind::Reconciliation,
            &payload.report.report_digest,
            payload,
            generation,
        )
    }

    fn append<T>(
        &self,
        kind: TaskKind,
        key: &str,
        payload: &T,
        expected_generation: Option<u64>,
    ) -> Result<DurableTaskRecord<T>, ProjectOperationError>
    where
        T: Clone + Serialize + DeserializeOwned,
    {
        let directory = self.record_directory(kind, key)?;
        fs::create_dir_all(directory.join("generations"))?;
        let lock = open_lock(&directory)?;
        lock.lock()?;
        let current = Self::load_unlocked::<T>(&directory)?;
        let actual_generation = current.as_ref().map(|record| record.generation);
        if actual_generation != expected_generation {
            return Err(ProjectOperationError::ConcurrentJournalUpdate {
                expected: expected_generation,
                actual: actual_generation,
            });
        }
        let generation = current.as_ref().map_or(Ok(0), |record| {
            record
                .generation
                .checked_add(1)
                .ok_or(ProjectOperationError::JournalGenerationOverflow)
        })?;
        let previous_record_digest = current.map(|record| record.record_digest);
        let record_digest = journal_digest(generation, previous_record_digest.as_deref(), payload)?;
        let record = DurableTaskRecord {
            schema_version: JOURNAL_SCHEMA_VERSION,
            generation,
            previous_record_digest,
            payload: payload.clone(),
            record_digest,
        };
        let final_path = directory.join("generations").join(format!(
            "record-{generation:020}-{}.json",
            record.record_digest.trim_start_matches("sha256:")
        ));
        let temporary_path = directory
            .join("generations")
            .join(format!(".record-{}.tmp", Uuid::new_v4()));
        let write_result = (|| -> Result<(), ProjectOperationError> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)?;
            file.write_all(&serde_json::to_vec_pretty(&record)?)?;
            file.sync_all()?;
            fs::rename(&temporary_path, &final_path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        write_result?;
        lock.unlock()?;
        Ok(record)
    }

    fn load_latest<T>(
        &self,
        kind: TaskKind,
        key: &str,
    ) -> Result<Option<DurableTaskRecord<T>>, ProjectOperationError>
    where
        T: Clone + Serialize + DeserializeOwned,
    {
        let directory = self.record_directory(kind, key)?;
        if !directory.exists() {
            return Ok(None);
        }
        let lock = open_lock(&directory)?;
        lock.lock()?;
        let result = Self::load_unlocked(&directory);
        lock.unlock()?;
        result
    }

    fn load_unlocked<T>(
        directory: &Path,
    ) -> Result<Option<DurableTaskRecord<T>>, ProjectOperationError>
    where
        T: Clone + Serialize + DeserializeOwned,
    {
        let generations = directory.join("generations");
        if !generations.exists() {
            return Ok(None);
        }
        let mut paths = fs::read_dir(generations)?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with("record-") && name.ends_with(".json")
                })
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut previous = None;
        let mut latest = None;
        for (expected_generation, path) in paths.into_iter().enumerate() {
            let mut bytes = Vec::new();
            File::open(&path)?.read_to_end(&mut bytes)?;
            let record: DurableTaskRecord<T> = serde_json::from_slice(&bytes).map_err(|error| {
                ProjectOperationError::CorruptJournal(format!("{}: {error}", path.display()))
            })?;
            let expected_generation = u64::try_from(expected_generation)
                .map_err(|_| ProjectOperationError::JournalGenerationOverflow)?;
            if record.schema_version != JOURNAL_SCHEMA_VERSION
                || record.generation != expected_generation
                || record.previous_record_digest != previous
            {
                return Err(ProjectOperationError::CorruptJournal(format!(
                    "broken generation chain at {}",
                    path.display()
                )));
            }
            let digest = journal_digest(
                record.generation,
                record.previous_record_digest.as_deref(),
                &record.payload,
            )?;
            if digest != record.record_digest
                || !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains(&digest[7..]))
            {
                return Err(ProjectOperationError::CorruptJournal(format!(
                    "record digest mismatch at {}",
                    path.display()
                )));
            }
            previous = Some(record.record_digest.clone());
            latest = Some(record);
        }
        Ok(latest)
    }

    fn record_directory(
        &self,
        kind: TaskKind,
        key: &str,
    ) -> Result<PathBuf, ProjectOperationError> {
        if key.trim().is_empty() {
            return Err(ProjectOperationError::EmptyTaskKey);
        }
        let digest = canonical_sha256("takegraph-operation-journal-key-v1", &key)?;
        Ok(self
            .root
            .join(kind.directory())
            .join(digest.trim_start_matches("sha256:")))
    }
}

fn open_lock(directory: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("journal.lock"))
}

#[derive(Debug, Clone, Copy)]
enum TaskKind {
    Checkpoint,
    Render,
    Reconciliation,
    ReconciliationChild,
}

impl TaskKind {
    const fn directory(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoints",
            Self::Render => "renders",
            Self::Reconciliation => "reconciliations",
            Self::ReconciliationChild => "reconciliation-children",
        }
    }
}

fn journal_digest<T: Serialize + ?Sized>(
    generation: u64,
    previous: Option<&str>,
    payload: &T,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-operation-journal-record-v1",
        &(JOURNAL_SCHEMA_VERSION, generation, previous, payload),
    )
}

fn require_revision(
    canonical: &DurableProjectStore,
    expected: RevisionId,
) -> Result<(), ProjectOperationError> {
    let actual = canonical.head()?;
    if actual != expected {
        return Err(ProjectOperationError::StaleRevision { expected, actual });
    }
    Ok(())
}

async fn require_bridge_feature(
    client: &Ymm4BridgeClient,
    feature: &str,
) -> Result<(), ProjectOperationError> {
    let capabilities = client.structured_capabilities().await?;
    capabilities
        .require(&[CapabilityRequirement {
            feature: feature.into(),
            minimum_version: 1,
            schema_digest: capabilities
                .feature(feature)
                .map(|descriptor| descriptor.schema_digest.clone()),
        }])
        .map_err(|error| ProjectOperationError::MissingCapability(error.to_string()))
}

fn target_identity_digest(
    ymm4_version: &str,
    snapshot: &Ymm4ProjectSnapshot,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-ymm4-target-identity-v1",
        &(
            "takegraph-ymm4-bridge",
            snapshot.project_id.as_str(),
            snapshot.scene_id.as_str(),
            snapshot.fps,
            ymm4_version,
        ),
    )
}

fn reconciliation_target_identity_digest(
    capabilities: &takegraph_node::StructuredYmm4Capabilities,
    snapshot: &Ymm4ProjectSnapshot,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-ymm4-target-identity",
        &crate::ymm4_target_plan::ymm4_target_identity(capabilities, snapshot),
    )
}

fn reconciliation_source(
    source_revision: RevisionId,
    capabilities: &takegraph_node::StructuredYmm4Capabilities,
    snapshot: &Ymm4ProjectSnapshot,
) -> Result<ReconciliationSource, ProjectOperationError> {
    let feature = capabilities
        .feature("managedIdentity.detach")
        .ok_or_else(|| ProjectOperationError::MissingCapability("managedIdentity.detach".into()))?;
    Ok(ReconciliationSource {
        source_revision,
        project_id: snapshot.project_id.clone(),
        scene_id: snapshot.scene_id.clone(),
        target_identity_digest: reconciliation_target_identity_digest(capabilities, snapshot)?,
        state_profile_digest: canonical_sha256(
            "takegraph-reconciliation-state-profile-v1",
            &(
                RECONCILIATION_STATE_PROFILE,
                crate::managed_projection::NATIVE_EXTENSION_PROJECTION_PROFILE,
                capabilities.driver.plugin_version.as_str(),
            ),
        )?,
        capability_digest: capabilities.capability_digest.clone(),
        mutation_feature_version: feature.version,
        mutation_feature_schema_digest: feature.schema_digest.clone(),
        mutation_feature_properties_digest: canonical_sha256(
            "takegraph-reconciliation-mutation-feature-properties-v1",
            &feature.properties,
        )?,
    })
}

fn require_detach_capability(
    capabilities: &takegraph_node::StructuredYmm4Capabilities,
) -> Result<(), ProjectOperationError> {
    if !capabilities
        .feature("managedIdentity.detach")
        .is_some_and(|feature| feature.available)
    {
        return Err(ProjectOperationError::MissingCapability(
            "managedIdentity.detach".into(),
        ));
    }
    Ok(())
}

fn require_reconciliation_target(
    source: &ReconciliationSource,
    capabilities: &takegraph_node::StructuredYmm4Capabilities,
    snapshot: &Ymm4ProjectSnapshot,
) -> Result<(), ProjectOperationError> {
    let current = reconciliation_source(source.source_revision, capabilities, snapshot)?;
    if current != *source {
        return Err(ProjectOperationError::StaleTargetState);
    }
    Ok(())
}

fn require_observed_identity_unchanged(
    draft: &ReconciliationDetachDraft,
    snapshot: &Ymm4ProjectSnapshot,
) -> Result<(), ProjectOperationError> {
    let current = crate::managed_projection::project_snapshot(snapshot)
        .into_iter()
        .filter(|item| item.identity == draft.identity)
        .collect::<Vec<_>>();
    let comparison =
        SemanticDriftReport::build(draft.source.clone(), draft.observed.clone(), current)?;
    if !comparison.entries.is_empty() {
        return Err(ProjectOperationError::StaleTargetState);
    }
    Ok(())
}

fn require_detached_identity_absent(
    identity: &ManagedSemanticIdentity,
    snapshot: &Ymm4ProjectSnapshot,
) -> Result<(), ProjectOperationError> {
    if crate::managed_projection::project_snapshot(snapshot)
        .iter()
        .any(|item| &item.identity == identity)
    {
        return Err(ProjectOperationError::DetachedIdentityStillPresent);
    }
    Ok(())
}

fn finalize_reconciliation_detach(
    canonical: &DurableProjectStore,
    draft: &ReconciliationDetachDraft,
    capabilities: &takegraph_node::StructuredYmm4Capabilities,
    snapshot: &Ymm4ProjectSnapshot,
    receipt: &Ymm4MetadataDetachReceipt,
) -> Result<RevisionId, ProjectOperationError> {
    let managed_update =
        crate::project_store::VerifiedManagedStateUpdate::removing_identity_from_metadata_detach_receipt(
            draft.identity.clone(),
            receipt,
        )?;
    let target = crate::ymm4_target_plan::ymm4_target_identity(capabilities, snapshot);
    let proof = crate::project_store::VerifiedExternalCommit::from_receipt(
        draft.operation_id,
        draft.source.source_revision,
        draft.patch.digest.clone(),
        receipt.request_digest.clone(),
        receipt,
        crate::VerifiedTargetBinding {
            adapter_id: target.adapter_id,
            target_project_id: draft.source.project_id.clone(),
            scene_id: draft.source.scene_id.clone(),
            target_identity_digest: draft.source.target_identity_digest.clone(),
            verified_fingerprint: receipt.after_fingerprint.clone(),
        },
    )?
    .with_managed_state_update(managed_update)?;
    Ok(canonical.commit_verified_external(&proof)?)
}

const fn map_metadata_detach_status(
    status: Ymm4MetadataDetachStatus,
) -> ReconciliationDetachStatus {
    match status {
        Ymm4MetadataDetachStatus::Applying => ReconciliationDetachStatus::Applying,
        Ymm4MetadataDetachStatus::Verified => ReconciliationDetachStatus::Verified,
        Ymm4MetadataDetachStatus::RolledBack => ReconciliationDetachStatus::RolledBack,
        Ymm4MetadataDetachStatus::NotStarted | Ymm4MetadataDetachStatus::Failed => {
            ReconciliationDetachStatus::Failed
        }
        Ymm4MetadataDetachStatus::RecoveryRequired => ReconciliationDetachStatus::RecoveryRequired,
    }
}

const fn manifest_route(manifest: &ReconciliationReExportManifest) -> ReconciliationExporterRoute {
    match manifest {
        ReconciliationReExportManifest::PortablePair { .. } => {
            ReconciliationExporterRoute::PortablePair
        }
        ReconciliationReExportManifest::NativeVoiceMutation { .. } => {
            ReconciliationExporterRoute::NativeVoiceMutation
        }
        ReconciliationReExportManifest::NativeExtension { .. } => {
            ReconciliationExporterRoute::NativeExtension
        }
    }
}

fn require_re_export_projection(
    source: &ReconciliationSource,
    canonical: &[ManagedSemanticItem],
    projected: Vec<ManagedSemanticItem>,
) -> Result<(), ProjectOperationError> {
    let comparison = SemanticDriftReport::build(source.clone(), canonical.to_vec(), projected)?;
    if !comparison.entries.is_empty() {
        return Err(ProjectOperationError::ReExportManifestMismatch);
    }
    Ok(())
}

fn project_portable_manifest(utterances: &[ManagedUtterance]) -> Vec<ManagedSemanticItem> {
    utterances
        .iter()
        .flat_map(|utterance| {
            let identity = ManagedSemanticIdentity {
                entity_id: utterance.entity_id.clone(),
                realization_id: None,
            };
            let common = BTreeMap::from([
                (
                    "frame".into(),
                    ManagedSemanticValue::Integer(i64::from(utterance.frame)),
                ),
                (
                    "length".into(),
                    ManagedSemanticValue::Integer(i64::from(utterance.length)),
                ),
                (
                    "artifactHash".into(),
                    ManagedSemanticValue::Text(utterance.artifact_hash.clone()),
                ),
                (
                    "speaker".into(),
                    ManagedSemanticValue::Text(utterance.speaker.clone()),
                ),
            ]);
            let mut audio = common.clone();
            audio.insert(
                "layer".into(),
                ManagedSemanticValue::Integer(i64::from(utterance.audio_layer)),
            );
            audio.insert(
                "audioPath".into(),
                ManagedSemanticValue::Text(utterance.audio_path.clone()),
            );
            let mut caption = common;
            caption.insert(
                "layer".into(),
                ManagedSemanticValue::Integer(i64::from(utterance.caption_layer)),
            );
            caption.insert(
                "text".into(),
                ManagedSemanticValue::Text(utterance.caption.clone()),
            );
            [
                ManagedSemanticItem {
                    identity: identity.clone(),
                    entity_revision: utterance.revision,
                    realization_kind: "portable_audio".into(),
                    owned_fields: audio,
                },
                ManagedSemanticItem {
                    identity,
                    entity_revision: utterance.revision,
                    realization_kind: "portable_caption".into(),
                    owned_fields: caption,
                },
            ]
        })
        .collect()
}

fn project_native_voice_manifest(
    mutations: &[Ymm4NativeVoiceMutation],
) -> Vec<ManagedSemanticItem> {
    mutations
        .iter()
        .filter(|mutation| mutation.action != takegraph_node::Ymm4NativeVoiceMutationAction::Delete)
        .map(|mutation| ManagedSemanticItem {
            identity: ManagedSemanticIdentity {
                entity_id: mutation.entity_id.clone(),
                realization_id: Some(mutation.realization_id),
            },
            entity_revision: mutation.revision,
            realization_kind: "ymm4_native_voice".into(),
            owned_fields: BTreeMap::from([
                (
                    "frame".into(),
                    ManagedSemanticValue::Integer(i64::from(mutation.frame)),
                ),
                (
                    "layer".into(),
                    ManagedSemanticValue::Integer(i64::from(mutation.layer)),
                ),
                (
                    "length".into(),
                    ManagedSemanticValue::Integer(i64::from(mutation.max_length)),
                ),
                (
                    "text".into(),
                    ManagedSemanticValue::Text(mutation.display_text.clone()),
                ),
                (
                    "speaker".into(),
                    ManagedSemanticValue::Text(mutation.character_name.clone()),
                ),
            ]),
        })
        .collect()
}

fn require_native_voice_re_export_scope(
    identity: &ManagedSemanticIdentity,
    canonical: &[ManagedSemanticItem],
    mutations: &[Ymm4NativeVoiceMutation],
) -> Result<(), ProjectOperationError> {
    let [mutation] = mutations else {
        return Err(ProjectOperationError::ReExportManifestMismatch);
    };
    if mutation.entity_id != identity.entity_id
        || Some(mutation.realization_id) != identity.realization_id
    {
        return Err(ProjectOperationError::ReExportManifestMismatch);
    }
    let expected_revision = canonical
        .iter()
        .find(|item| item.identity == *identity)
        .map(|item| item.entity_revision);
    if canonical.is_empty() {
        if mutation.action != takegraph_node::Ymm4NativeVoiceMutationAction::Delete {
            return Err(ProjectOperationError::ReExportManifestMismatch);
        }
    } else if mutation.action == takegraph_node::Ymm4NativeVoiceMutationAction::Delete
        || expected_revision != Some(mutation.revision)
        || canonical.iter().any(|item| item.identity != *identity)
    {
        return Err(ProjectOperationError::ReExportManifestMismatch);
    }
    Ok(())
}

fn native_extension_identity_overrides(
    identity: &ManagedSemanticIdentity,
    manifest: &NativeExtensionStageManifest,
) -> Result<BTreeMap<String, Uuid>, ProjectOperationError> {
    if manifest.intents.len() != 1 {
        return Err(ProjectOperationError::ReExportManifestMismatch);
    }
    let realization_id = identity
        .realization_id
        .ok_or(ProjectOperationError::ReExportManifestMismatch)?;
    Ok(BTreeMap::from([(
        manifest.intents[0].logical_key(),
        realization_id,
    )]))
}

// Keeping the exhaustive intent-to-semantic projection together makes it
// easier to compare this fail-closed adapter with managed_projection.rs.
#[allow(clippy::too_many_lines)]
fn project_native_extension_preview(
    task: &Ymm4NativeExtensionTask,
) -> Result<Vec<ManagedSemanticItem>, ProjectOperationError> {
    let mut projected = Vec::new();
    for operation in &task.plan.operations {
        if operation.action == takegraph_core::NativeExtensionAction::Delete {
            continue;
        }
        let (entity_id, entity_revision, kind, mut fields) = match &operation.intent {
            NativeExtensionIntent::UpsertPortrait(intent) => {
                let kind = match intent.presentation {
                    PortraitPresentation::Portrait => "portrait",
                    PortraitPresentation::Face => "face",
                };
                (
                    intent.entity_id.clone(),
                    intent.entity_revision,
                    kind,
                    BTreeMap::from([
                        ("frame".into(), intent.placement.frame.to_string()),
                        ("layer".into(), intent.placement.primary_layer.to_string()),
                        ("length".into(), intent.duration_frames.to_string()),
                        (
                            "descriptorId".into(),
                            intent.character_binding.descriptor_id.clone(),
                        ),
                    ]),
                )
            }
            NativeExtensionIntent::UpsertAsset(intent) => {
                let kind = match intent.asset.kind {
                    AssetKind::Image => "image",
                    AssetKind::Video => "video",
                    AssetKind::Audio => "audio",
                    AssetKind::Bgm => "bgm",
                };
                (
                    intent.entity_id.clone(),
                    intent.entity_revision,
                    kind,
                    BTreeMap::from([
                        ("frame".into(), intent.placement.frame.to_string()),
                        ("layer".into(), intent.placement.primary_layer.to_string()),
                        ("length".into(), intent.duration_frames.to_string()),
                        (
                            "artifactDigest".into(),
                            intent.asset.artifact_digest.clone(),
                        ),
                        ("byteLength".into(), intent.asset.byte_length.to_string()),
                        ("loopPlayback".into(), intent.loop_playback.to_string()),
                    ]),
                )
            }
            NativeExtensionIntent::MutateEffect(intent) => {
                let EffectOperation::Upsert { parameters } = &intent.operation else {
                    continue;
                };
                let descriptor = task
                    .descriptor_catalog
                    .descriptors
                    .iter()
                    .find(|descriptor| descriptor.descriptor_id == intent.descriptor.descriptor_id)
                    .ok_or(ProjectOperationError::ReExportManifestMismatch)?;
                let stable_type_id = descriptor
                    .metadata
                    .get("type")
                    .cloned()
                    .ok_or(ProjectOperationError::ReExportManifestMismatch)?;
                let collection = match descriptor.kind.as_str() {
                    "audio-effect" => "AudioEffects",
                    "video-effect" => "VideoEffects",
                    _ => return Err(ProjectOperationError::ReExportManifestMismatch),
                };
                (
                    intent.target_entity_id.clone(),
                    intent.target_entity_revision,
                    "managed_effect",
                    BTreeMap::from([
                        (
                            "descriptorId".into(),
                            intent.descriptor.descriptor_id.clone(),
                        ),
                        ("stableTypeId".into(), stable_type_id),
                        ("collection".into(), collection.into()),
                        (
                            "parametersDigest".into(),
                            canonical_sha256(
                                "takegraph-ymm4-native-extension-effect-parameters-v1",
                                parameters,
                            )?,
                        ),
                    ]),
                )
            }
            NativeExtensionIntent::InstantiateTemplate(intent) => {
                let descriptor = task
                    .descriptor_catalog
                    .descriptors
                    .iter()
                    .find(|descriptor| descriptor.descriptor_id == intent.template.descriptor_id)
                    .ok_or(ProjectOperationError::ReExportManifestMismatch)?;
                let part_count = descriptor
                    .metadata
                    .get("itemTypes")
                    .map(|value| value.lines().filter(|line| !line.trim().is_empty()).count())
                    .filter(|count| *count > 0)
                    .ok_or(ProjectOperationError::ReExportManifestMismatch)?;
                (
                    intent.entity_id.clone(),
                    intent.entity_revision,
                    "template",
                    BTreeMap::from([
                        ("frame".into(), intent.placement.frame.to_string()),
                        ("layer".into(), intent.placement.primary_layer.to_string()),
                        ("descriptorId".into(), intent.template.descriptor_id.clone()),
                        ("partCount".into(), part_count.to_string()),
                    ]),
                )
            }
        };
        fields.extend([
            ("logicalKey".into(), operation.intent.logical_key()),
            ("projectId".into(), task.target.project_id.clone()),
            ("entityId".into(), entity_id.clone()),
            ("entityRevision".into(), entity_revision.to_string()),
            ("kind".into(), kind.into()),
        ]);
        projected.push(ManagedSemanticItem {
            identity: ManagedSemanticIdentity {
                entity_id,
                realization_id: Some(operation.realization_id),
            },
            entity_revision,
            realization_kind: format!("ymm4_native_{kind}"),
            owned_fields: fields
                .into_iter()
                .map(|(name, value)| (name, ManagedSemanticValue::Text(value)))
                .collect(),
        });
    }
    Ok(projected)
}

const fn map_bridge_render_status(status: Ymm4RenderStatus) -> RenderTaskStatus {
    match status {
        Ymm4RenderStatus::Queued | Ymm4RenderStatus::Running => RenderTaskStatus::Running,
        Ymm4RenderStatus::Succeeded => RenderTaskStatus::Finalizing,
        Ymm4RenderStatus::Cancelling => RenderTaskStatus::Cancelling,
        Ymm4RenderStatus::Cancelled => RenderTaskStatus::Cancelled,
        Ymm4RenderStatus::Failed => RenderTaskStatus::Failed,
        Ymm4RenderStatus::Stale => RenderTaskStatus::Stale,
        Ymm4RenderStatus::RecoveryRequired => RenderTaskStatus::RecoveryRequired,
    }
}

fn verify_media_profile(
    profile: &Ymm4RenderProfileDescriptor,
    media: &VerifiedMediaProbe,
) -> Result<(), ProjectOperationError> {
    if media.container != profile.container
        || media.width != profile.width
        || media.height != profile.height
        || media.fps_numerator != profile.fps_numerator
        || media.fps_denominator != profile.fps_denominator
        || media.video_streams != 1
        || media.video_codec != profile.video_codec
        || media.pixel_format != profile.pixel_format
        || (!profile.has_audio && media.audio_streams != 0)
        || (profile.has_audio
            && (media.audio_streams != 1
                || media.audio_codec.as_deref() != Some(profile.audio_codec.as_str())
                || media.audio_sample_rate != Some(profile.audio_sample_rate)))
    {
        return Err(ProjectOperationError::RenderProfileOutputMismatch);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProjectOperationError {
    #[error("project has no existing path; Save As is not authorized")]
    UnsavedProject,
    #[error("bridge checkpoint profile is not existing-path-only or has no digest")]
    InvalidCheckpointProfile,
    #[error("render profile was not found: {0}")]
    RenderProfileNotFound(String),
    #[error("render profile is not exactly bindable: {0}")]
    RenderProfileNotBindable(String),
    #[error("render requires a verified checkpoint operation: {0}")]
    RenderCheckpointNotVerified(Uuid),
    #[error("required structured bridge capability is unavailable: {0}")]
    MissingCapability(String),
    #[error("external operation task was not found: {0}")]
    TaskNotFound(Uuid),
    #[error("reconciliation report was not found: {0}")]
    ReconciliationNotFound(String),
    #[error("reconciliation child task was not found: {0}")]
    ReconciliationChildNotFound(String),
    #[error("durable operation journal key is empty")]
    EmptyTaskKey,
    #[error("checkpoint transition is invalid from {0:?}")]
    InvalidCheckpointTransition(CheckpointTaskStatus),
    #[error("render transition is invalid from {0:?}")]
    InvalidRenderTransition(RenderTaskStatus),
    #[error("reconciliation transition is invalid from {0:?}")]
    InvalidReconciliationTransition(ReconciliationTaskStatus),
    #[error("reconciliation has no explicit-decision preview")]
    MissingReconciliationPreview,
    #[error("durable receipt-derived reconciliation expectation changed after preview")]
    DurableExpectedStateChanged,
    #[error("durable reconciliation child does not match the approved action")]
    InvalidReconciliationChildren,
    #[error("no existing exporter route can safely materialize reconciliation kinds: {0:?}")]
    UnsupportedReconciliationExporter(Vec<String>),
    #[error("reconciliation child is not the requested downstream kind")]
    WrongReconciliationChildKind,
    #[error("reconciliation child approval does not match its current digest")]
    ReconciliationChildApprovalMismatch,
    #[error("metadata detach transition is invalid from {0:?}")]
    InvalidDetachTransition(ReconciliationDetachStatus),
    #[error("metadata detach requires a realization ID carried in Remark")]
    DetachRequiresRemarkIdentity,
    #[error("metadata detach bridge route is unavailable")]
    ReconciliationDetachUnavailable,
    #[error("fresh YMM4 read-back still contains the detached identity")]
    DetachedIdentityStillPresent,
    #[error("metadata detach was not verified ({0:?}): {1:?}")]
    MetadataDetachNotVerified(ReconciliationDetachStatus, Option<String>),
    #[error("authenticated metadata detach replay did not match the durable verified receipt")]
    MetadataDetachReplayMismatch,
    #[error("metadata detach bridge response success disagreed with its verified receipt")]
    MetadataDetachResponseInconsistent,
    #[error("canonical re-export transition is invalid from {0:?}")]
    InvalidReExportTransition(ReconciliationReExportStatus),
    #[error("canonical re-export manifest selected the wrong exporter route")]
    ReExportRouteMismatch,
    #[error("canonical re-export manifest does not exactly reproduce the durable projection")]
    ReExportManifestMismatch,
    #[error("canonical re-export replay manifest does not match the persisted downstream preview")]
    ReExportManifestReplayMismatch,
    #[error("existing exporter unexpectedly returned an approved downstream patch")]
    DownstreamPreviewWasApproved,
    #[error("render journal has no bridge task")]
    MissingBridgeTask,
    #[error("verified checkpoint journal has no receipt")]
    MissingCheckpointReceipt,
    #[error("canonical revision is stale: expected {expected:?}, got {actual:?}")]
    StaleRevision {
        expected: RevisionId,
        actual: RevisionId,
    },
    #[error("operation canonical project mismatch: expected {expected}, got {actual}")]
    CanonicalProjectMismatch { expected: String, actual: String },
    #[error("YMM4 target state changed after staging")]
    StaleTargetState,
    #[error("render progress regressed from {previous} to {current}")]
    ProgressRegressed { previous: u16, current: u16 },
    #[error("final media does not satisfy the pinned render profile")]
    RenderProfileOutputMismatch,
    #[error("durable operation journal is corrupt: {0}")]
    CorruptJournal(String),
    #[error("operation journal changed concurrently (expected {expected:?}, got {actual:?})")]
    ConcurrentJournalUpdate {
        expected: Option<u64>,
        actual: Option<u64>,
    },
    #[error("operation journal generation overflowed")]
    JournalGenerationOverflow,
    #[error(transparent)]
    Bridge(#[from] Ymm4Error),
    #[error(transparent)]
    Node(#[from] ProjectOperationNodeError),
    #[error(transparent)]
    MetadataDetachNode(#[from] MetadataDetachNodeError),
    #[error(transparent)]
    PortableExport(#[from] crate::Ymm4ExportError),
    #[error(transparent)]
    NativeVoiceMutation(#[from] crate::Ymm4NativeVoiceMutationError),
    #[error(transparent)]
    NativeExtension(#[from] crate::Ymm4NativeExtensionError),
    #[error(transparent)]
    ProjectStore(#[from] ProjectStoreError),
    #[error(transparent)]
    Reconciliation(#[from] ReconciliationError),
    #[error(transparent)]
    Patch(#[from] PatchError),
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl ProjectOperationError {
    const fn is_stale(&self) -> bool {
        matches!(
            self,
            Self::StaleRevision { .. }
                | Self::StaleTargetState
                | Self::CanonicalProjectMismatch { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use takegraph_core::{ManagedSemanticValue, ReconciliationChoice};
    use takegraph_node::{
        CapabilityValue, StructuredYmm4Capabilities, Ymm4Capabilities, Ymm4Capability, Ymm4Health,
    };

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!("takegraph-project-operations-{}", Uuid::new_v4()))
    }

    fn checkpoint() -> CheckpointTaskRecord {
        CheckpointTaskRecord {
            request: Ymm4CheckpointRequest::try_new(Ymm4CheckpointRequestInput {
                operation_id: Uuid::new_v4(),
                project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                source_revision: 3,
                target_identity_digest: "target".into(),
                expected_state_digest: "state".into(),
                checkpoint_profile_digest: "profile".into(),
            })
            .unwrap(),
            profile: Ymm4CheckpointProfile {
                profile_digest: "profile".into(),
                driver_profile_digest: "driver".into(),
                existing_path_only: true,
            },
            expected_project_path: "project.ymmp".into(),
            status: CheckpointTaskStatus::Staged,
            receipt: None,
            error: None,
        }
    }

    fn semantic_item(entity_id: &str, text: &str) -> ManagedSemanticItem {
        ManagedSemanticItem {
            identity: ManagedSemanticIdentity {
                entity_id: entity_id.into(),
                realization_id: Some(Uuid::new_v4()),
            },
            entity_revision: 3,
            realization_kind: "ymm4_native_voice".into(),
            owned_fields: BTreeMap::from([(
                "text".into(),
                ManagedSemanticValue::Text(text.into()),
            )]),
        }
    }

    fn seed_managed_identity(store: &DurableProjectStore, identity: &ManagedSemanticIdentity) {
        let operation_id = Uuid::new_v4();
        let receipt = takegraph_node::Ymm4OperationReceipt {
            operation_id,
            request_digest: "seed-request".into(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            expected_fingerprint: "seed-before".into(),
            status: takegraph_node::Ymm4OperationStatus::Verified,
            before_fingerprint: "seed-before".into(),
            after_fingerprint: "seed-after".into(),
            applied_items: vec![takegraph_node::Ymm4ManagedItem {
                entity_id: identity.entity_id.clone(),
                revision: 3,
                kind: takegraph_node::ManagedItemKind::Voice,
                frame: 0,
                layer: 0,
                length: 1,
                text: Some("canonical".into()),
                audio_path: None,
                artifact_hash: None,
                speaker: None,
                realization_id: identity.realization_id,
            }],
            verified: true,
            error: None,
        };
        let update =
            crate::project_store::VerifiedManagedStateUpdate::replacing_identities_from_receipt(
                [identity.clone()],
                &receipt,
            )
            .unwrap();
        let proof = crate::project_store::VerifiedExternalCommit::from_receipt(
            operation_id,
            RevisionId(0),
            "seed-patch",
            "seed-request",
            &receipt,
            crate::VerifiedTargetBinding {
                adapter_id: "ymm4-4.55".into(),
                target_project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                verified_fingerprint: "seed-after".into(),
            },
        )
        .unwrap()
        .with_managed_state_update(update)
        .unwrap();
        store.commit_verified_external(&proof).unwrap();
    }

    fn test_terminal_detach_receipt(
        request: &Ymm4MetadataDetachRequest,
        status: Ymm4MetadataDetachStatus,
    ) -> Ymm4MetadataDetachReceipt {
        let digest = "a".repeat(64);
        Ymm4MetadataDetachReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest.clone(),
            project_id: request.project_id.clone(),
            scene_id: request.scene_id.clone(),
            source_revision: request.source_revision,
            expected_fingerprint: request.expected_fingerprint.clone(),
            entity_id: request.entity_id.clone(),
            realization_id: request.realization_id,
            identity_carrier: request.identity_carrier.clone(),
            status,
            before_fingerprint: request.expected_fingerprint.clone(),
            after_fingerprint: request.expected_fingerprint.clone(),
            detached_item_count: u32::from(status == Ymm4MetadataDetachStatus::RolledBack),
            before_remark_digest: digest.clone(),
            expected_after_remark_digest: if status == Ymm4MetadataDetachStatus::NotStarted {
                digest.clone()
            } else {
                "b".repeat(64)
            },
            remark_digest_after: digest.clone(),
            non_remark_content_digest_before: digest.clone(),
            non_remark_content_digest_after: digest,
            remark_absent: false,
            verified: false,
            error: Some(match status {
                Ymm4MetadataDetachStatus::NotStarted => "durable no-mutation tombstone".into(),
                Ymm4MetadataDetachStatus::RolledBack => "exact rollback".into(),
                _ => unreachable!(),
            }),
        }
    }

    fn reconciliation_capabilities() -> StructuredYmm4Capabilities {
        StructuredYmm4Capabilities::from_bridge(
            &Ymm4Health {
                status: "running".into(),
                protocol_version: 2,
                plugin_version: "test-plugin".into(),
                ymm4_version: "4.55.1.1".into(),
            },
            &Ymm4Capabilities {
                protocol_version: 2,
                capabilities: vec![
                    Ymm4Capability::ReadbackVerification,
                    Ymm4Capability::IdempotentApply,
                    Ymm4Capability::RequestBoundReceipts,
                    Ymm4Capability::WriteAheadApply,
                    Ymm4Capability::RecoveryReadback,
                    Ymm4Capability::MetadataRemarkDetach,
                    Ymm4Capability::MutationProfileYmm4_4_55_1_1,
                ],
            },
        )
        .unwrap()
    }

    fn reconciliation_snapshot() -> Ymm4ProjectSnapshot {
        Ymm4ProjectSnapshot {
            project_id: "project-a".into(),
            project_name: "test".into(),
            project_path: "test.ymmp".into(),
            scene_id: "scene-a".into(),
            fps: 60,
            fingerprint: "fingerprint-a".into(),
            managed_items: Vec::new(),
            native_extensions: Vec::new(),
            unmanaged_context_count: 0,
        }
    }

    #[test]
    fn reconciliation_source_rejects_same_version_feature_contract_drift() {
        let capabilities = reconciliation_capabilities();
        let snapshot = reconciliation_snapshot();
        let source = reconciliation_source(RevisionId(3), &capabilities, &snapshot).unwrap();

        let mut property_drift = capabilities.clone();
        property_drift
            .features
            .get_mut("managedIdentity.detach")
            .unwrap()
            .properties
            .insert("freshReadback".into(), CapabilityValue::Boolean(false));
        assert!(matches!(
            require_reconciliation_target(&source, &property_drift, &snapshot),
            Err(ProjectOperationError::StaleTargetState)
        ));

        let mut schema_drift = capabilities;
        schema_drift
            .features
            .get_mut("managedIdentity.detach")
            .unwrap()
            .schema_digest = "sha256:changed-schema".into();
        assert!(matches!(
            require_reconciliation_target(&source, &schema_drift, &snapshot),
            Err(ProjectOperationError::StaleTargetState)
        ));

        let mut full_capability_drift = reconciliation_capabilities();
        full_capability_drift.capability_digest = "sha256:changed-capabilities".into();
        assert!(matches!(
            require_reconciliation_target(&source, &full_capability_drift, &snapshot),
            Err(ProjectOperationError::StaleTargetState)
        ));
    }

    #[test]
    fn append_only_generations_survive_reopen() {
        let root = test_root();
        let store = ProjectOperationStore::new(&root);
        let mut payload = checkpoint();
        let first = store.append_checkpoint(&payload, None).unwrap();
        payload.status = CheckpointTaskStatus::Executing;
        let second = store
            .append_checkpoint(&payload, Some(first.generation))
            .unwrap();
        assert_eq!(second.generation, 1);
        drop(store);
        let reopened = ProjectOperationStore::new(&root)
            .checkpoint_status(payload.request.operation_id)
            .unwrap();
        assert_eq!(reopened.payload.status, CheckpointTaskStatus::Executing);
        assert_eq!(reopened.previous_record_digest, Some(first.record_digest));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_concurrent_generation_is_rejected() {
        let root = test_root();
        let store = ProjectOperationStore::new(&root);
        let payload = checkpoint();
        store.append_checkpoint(&payload, None).unwrap();
        assert!(matches!(
            store.append_checkpoint(&payload, None),
            Err(ProjectOperationError::ConcurrentJournalUpdate { .. })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_generation_fails_closed() {
        let root = test_root();
        let store = ProjectOperationStore::new(&root);
        let payload = checkpoint();
        store.append_checkpoint(&payload, None).unwrap();
        let directory = store
            .record_directory(
                TaskKind::Checkpoint,
                &payload.request.operation_id.to_string(),
            )
            .unwrap()
            .join("generations");
        let path = fs::read_dir(directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(path, b"{corrupt").unwrap();
        assert!(matches!(
            store.checkpoint_status(payload.request.operation_id),
            Err(ProjectOperationError::CorruptJournal(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_journal_never_advances_the_canonical_revision() {
        let root = test_root();
        let canonical_root = root.join("canonical");
        let operation_root = root.join("operations");
        let canonical =
            DurableProjectStore::open_or_bootstrap(&canonical_root, "project-a", RevisionId(9))
                .unwrap();
        let operations = ProjectOperationStore::new(operation_root);
        let payload = checkpoint();

        operations.append_checkpoint(&payload, None).unwrap();

        assert_eq!(canonical.head().unwrap(), RevisionId(9));
        assert_eq!(canonical.snapshot().unwrap().generation, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_required_is_a_terminal_service_render_state() {
        assert!(RenderTaskStatus::RecoveryRequired.is_terminal());
    }

    #[test]
    // The end-to-end fixture keeps the complete parent/child journal evidence visible.
    #[allow(clippy::too_many_lines)]
    fn reconciliation_actions_materialize_inert_durable_children_idempotently() {
        let root = test_root();
        let store = ProjectOperationStore::new(&root);
        let import_expected = semantic_item("utt-import", "canonical");
        let mut import_actual = import_expected.clone();
        import_actual
            .owned_fields
            .insert("text".into(), ManagedSemanticValue::Text("edited".into()));
        let detach_actual = semantic_item("utt-detach", "foreign-managed");
        let reexport_expected = semantic_item("utt-reexport", "canonical");
        let report = SemanticDriftReport::build(
            ReconciliationSource {
                source_revision: RevisionId(11),
                project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                state_profile_digest: "sha256:profile".into(),
                capability_digest: "sha256:capabilities".into(),
                mutation_feature_version: 1,
                mutation_feature_schema_digest: "sha256:feature-schema".into(),
                mutation_feature_properties_digest: "sha256:feature-properties".into(),
            },
            vec![import_expected, reexport_expected],
            vec![import_actual, detach_actual],
        )
        .unwrap();
        let decisions = report
            .entries
            .iter()
            .map(|entry| ReconciliationDecision {
                entry_id: entry.entry_id.clone(),
                choice: match entry.identity.entity_id.as_str() {
                    "utt-import" => ReconciliationChoice::ImportIntoTakeGraph,
                    "utt-detach" => ReconciliationChoice::DetachFromTakeGraph,
                    "utt-reexport" => ReconciliationChoice::ReExportCanonical,
                    other => panic!("unexpected entry {other}"),
                },
            })
            .collect();
        let preview = ReconciliationPreview::build(&report, decisions).unwrap();

        let first = store
            .materialize_reconciliation_children(&report, &preview)
            .unwrap();
        let replay = store
            .materialize_reconciliation_children(&report, &preview)
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(first.len(), 3);
        assert!(
            first
                .iter()
                .all(|child| child.child_task_id.starts_with("sha256:"))
        );

        for reference in &first {
            let child = store
                .reconciliation_child_status(&reference.child_task_id)
                .unwrap();
            match child.payload {
                ReconciliationChildTask::ImportPatch(draft) => {
                    assert_eq!(draft.patch.status, PatchStatus::Previewable);
                    assert!(draft.patch.approved_digest.is_none());
                    assert_eq!(reference.patch_id, Some(draft.patch.id));
                }
                ReconciliationChildTask::MetadataDetach(draft) => {
                    assert_eq!(draft.patch.status, PatchStatus::Previewable);
                    assert!(draft.patch.approved_digest.is_none());
                    assert_eq!(draft.status, ReconciliationDetachStatus::PreviewReady);
                    assert!(draft.request.is_none());
                    assert!(draft.receipt.is_none());
                    assert!(draft.error.is_none());
                    assert!(
                        draft
                            .contract
                            .requirements
                            .contains(&ReconciliationDetachRequirement::WriteAheadLog)
                    );
                    assert!(
                        draft
                            .contract
                            .requirements
                            .contains(&ReconciliationDetachRequirement::FreshReadback)
                    );
                    assert!(
                        draft
                            .contract
                            .requirements
                            .contains(&ReconciliationDetachRequirement::ContentDigestUnchanged)
                    );
                    assert_eq!(
                        draft.contract.bridge_wire_status,
                        ReconciliationBridgeWireStatus::Available
                    );
                }
                ReconciliationChildTask::CanonicalReExport(handoff) => {
                    assert!(handoff.downstream_preview_required);
                    assert!(handoff.downstream_approval_required);
                    assert_eq!(
                        handoff.status,
                        ReconciliationReExportStatus::AwaitingManifest
                    );
                    assert!(handoff.downstream_preview.is_none());
                    assert!(handoff.error.is_none());
                    assert_eq!(
                        handoff.exporter_route,
                        ReconciliationExporterRoute::NativeVoiceMutation
                    );
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detach_child_lifecycle_requires_its_own_approval_and_exact_request() {
        let expected = semantic_item("utt-detach", "canonical");
        let mut actual = expected.clone();
        actual
            .owned_fields
            .insert("text".into(), ManagedSemanticValue::Text("edited".into()));
        let report = SemanticDriftReport::build(
            ReconciliationSource {
                source_revision: RevisionId(5),
                project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                state_profile_digest: "sha256:profile".into(),
                capability_digest: "sha256:capabilities".into(),
                mutation_feature_version: 1,
                mutation_feature_schema_digest: "sha256:feature-schema".into(),
                mutation_feature_properties_digest: "sha256:feature-properties".into(),
            },
            vec![expected],
            vec![actual],
        )
        .unwrap();
        let entry = &report.entries[0];
        let action_digest = "sha256:action";
        let child = build_detach_child(
            "sha256:child",
            action_digest,
            &report,
            &entry.entry_id,
            &entry.identity,
            &entry.actual,
        )
        .unwrap();
        let ReconciliationChildTask::MetadataDetach(mut draft) = child else {
            panic!("expected detach child");
        };
        let digest = draft.patch.digest.clone();
        assert!(valid_detach_child_lifecycle(
            &draft,
            &report.source,
            &digest
        ));

        draft.patch.approve().unwrap();
        assert!(!valid_detach_child_lifecycle(
            &draft,
            &report.source,
            &digest
        ));
        draft.status = ReconciliationDetachStatus::Approved;
        assert!(valid_detach_child_lifecycle(
            &draft,
            &report.source,
            &digest
        ));

        let realization_id = draft.identity.realization_id.unwrap();
        draft.request = Some(
            Ymm4MetadataDetachRequest::try_new(Ymm4MetadataDetachRequestInput {
                operation_id: draft.operation_id,
                project_id: draft.source.project_id.clone(),
                scene_id: draft.source.scene_id.clone(),
                source_revision: draft.source.source_revision.0,
                expected_fingerprint: "fingerprint".into(),
                entity_id: draft.identity.entity_id.clone(),
                realization_id,
            })
            .unwrap(),
        );
        assert!(!valid_detach_child_lifecycle(
            &draft,
            &report.source,
            &digest
        ));
        draft.status = ReconciliationDetachStatus::Applying;
        assert!(valid_detach_child_lifecycle(
            &draft,
            &report.source,
            &digest
        ));
    }

    #[test]
    fn terminal_no_mutation_detach_reissues_with_new_approval_boundary() {
        let expected = semantic_item("utt-detach", "canonical");
        let mut actual = expected.clone();
        actual
            .owned_fields
            .insert("text".into(), ManagedSemanticValue::Text("edited".into()));
        let report = SemanticDriftReport::build(
            ReconciliationSource {
                source_revision: RevisionId(5),
                project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                state_profile_digest: "sha256:profile".into(),
                capability_digest: "sha256:capabilities".into(),
                mutation_feature_version: 1,
                mutation_feature_schema_digest: "sha256:feature-schema".into(),
                mutation_feature_properties_digest: "sha256:feature-properties".into(),
            },
            vec![expected],
            vec![actual],
        )
        .unwrap();
        let entry = &report.entries[0];
        let action_digest = "sha256:action";
        let child = build_detach_child(
            "sha256:child",
            action_digest,
            &report,
            &entry.entry_id,
            &entry.identity,
            &entry.actual,
        )
        .unwrap();
        let ReconciliationChildTask::MetadataDetach(mut draft) = child else {
            panic!("expected detach child");
        };
        let old_operation_id = draft.operation_id;
        let old_patch_id = draft.patch.id;
        let old_digest = draft.patch.digest.clone();
        draft.patch.approve().unwrap();
        draft.status = ReconciliationDetachStatus::RolledBack;
        draft.error = Some("exact rollback".into());
        draft.request = Some(
            Ymm4MetadataDetachRequest::try_new(Ymm4MetadataDetachRequestInput {
                operation_id: draft.operation_id,
                project_id: draft.source.project_id.clone(),
                scene_id: draft.source.scene_id.clone(),
                source_revision: draft.source.source_revision.0,
                expected_fingerprint: "fingerprint".into(),
                entity_id: draft.identity.entity_id.clone(),
                realization_id: draft.identity.realization_id.unwrap(),
            })
            .unwrap(),
        );

        reissue_detach_attempt(&mut draft, action_digest, &report).unwrap();

        assert_eq!(draft.status, ReconciliationDetachStatus::PreviewReady);
        assert_ne!(draft.operation_id, old_operation_id);
        assert_ne!(draft.patch.id, old_patch_id);
        assert_ne!(draft.patch.digest, old_digest);
        assert_eq!(draft.patch.status, PatchStatus::Previewable);
        assert!(draft.patch.approved_digest.is_none());
        assert!(draft.request.is_none());
        assert!(draft.receipt.is_none());
        assert!(draft.committed_revision.is_none());
        assert!(draft.error.is_none());
        assert!(valid_detach_child_lifecycle(
            &draft,
            &report.source,
            &draft.patch.digest
        ));

        let old_reference = ReconciliationChildReference {
            entry_id: draft.entry_id.clone(),
            child_task_id: draft.child_task_id.clone(),
            kind: ReconciliationChildKind::MetadataDetach,
            patch_id: Some(old_patch_id),
            downstream_task_id: Some(old_operation_id),
            action_digest: action_digest.into(),
        };
        let new_reference = validate_detach_child(
            &draft,
            &draft.child_task_id,
            action_digest,
            &report,
            &entry.entry_id,
            &entry.identity,
            &entry.actual,
        )
        .unwrap();
        assert!(same_stable_child_reference(&new_reference, &old_reference));
    }

    #[test]
    // Exercises both crash boundaries for both authenticated terminal kinds:
    // after terminal child publication and after canonical reservation abort.
    #[allow(clippy::too_many_lines)]
    fn terminal_detach_reissue_resumes_across_each_persisted_phase() {
        for terminal_status in [
            Ymm4MetadataDetachStatus::NotStarted,
            Ymm4MetadataDetachStatus::RolledBack,
        ] {
            for abort_before_reopen in [false, true] {
                let root = test_root();
                let canonical_root = root.join("canonical");
                let operation_root = root.join("operations");
                let canonical = DurableProjectStore::open(&canonical_root, "project-a").unwrap();
                let identity = ManagedSemanticIdentity {
                    entity_id: "utt-detach".into(),
                    realization_id: Some(Uuid::new_v4()),
                };
                seed_managed_identity(&canonical, &identity);
                let source = ReconciliationSource {
                    source_revision: RevisionId(1),
                    project_id: "project-a".into(),
                    scene_id: "scene-a".into(),
                    target_identity_digest: "sha256:target".into(),
                    state_profile_digest: "sha256:profile".into(),
                    capability_digest: "sha256:capabilities".into(),
                    mutation_feature_version: 1,
                    mutation_feature_schema_digest: "sha256:feature-schema".into(),
                    mutation_feature_properties_digest: "sha256:feature-properties".into(),
                };
                let mut observed = semantic_item("utt-detach", "observed");
                observed.identity = identity.clone();
                let report = SemanticDriftReport::build(source, vec![], vec![observed]).unwrap();
                let preview = ReconciliationPreview::build(
                    &report,
                    vec![ReconciliationDecision {
                        entry_id: report.entries[0].entry_id.clone(),
                        choice: ReconciliationChoice::DetachFromTakeGraph,
                    }],
                )
                .unwrap();
                let store = ProjectOperationStore::new(&operation_root);
                let child_reference = store
                    .materialize_reconciliation_children(&report, &preview)
                    .unwrap()
                    .remove(0);
                store
                    .append_reconciliation(
                        &ReconciliationTaskRecord {
                            report: report.clone(),
                            expected_managed_state: vec![],
                            preview: Some(preview),
                            materialized_children: vec![child_reference.clone()],
                            status: ReconciliationTaskStatus::ActionsMaterialized,
                        },
                        None,
                    )
                    .unwrap();
                let child = store
                    .reconciliation_child_status(&child_reference.child_task_id)
                    .unwrap();
                let ReconciliationChildTask::MetadataDetach(mut draft) = child.payload else {
                    panic!("expected detach child");
                };
                draft.patch.approve().unwrap();
                let target = crate::VerifiedTargetBinding {
                    adapter_id: "ymm4-4.55".into(),
                    target_project_id: draft.source.project_id.clone(),
                    scene_id: draft.source.scene_id.clone(),
                    target_identity_digest: draft.source.target_identity_digest.clone(),
                    verified_fingerprint: "fingerprint-before".into(),
                };
                let request = Ymm4MetadataDetachRequest::try_new(Ymm4MetadataDetachRequestInput {
                    operation_id: draft.operation_id,
                    project_id: draft.source.project_id.clone(),
                    scene_id: draft.source.scene_id.clone(),
                    source_revision: draft.source.source_revision.0,
                    expected_fingerprint: target.verified_fingerprint.clone(),
                    entity_id: draft.identity.entity_id.clone(),
                    realization_id: draft.identity.realization_id.unwrap(),
                })
                .unwrap();
                canonical
                    .reserve_metadata_detach(
                        draft.operation_id,
                        draft.source.source_revision,
                        &draft.patch.digest,
                        &request.request_digest,
                        &target,
                        &draft.identity,
                    )
                    .unwrap();
                let reserved = canonical
                    .snapshot()
                    .unwrap()
                    .pending_external_commit
                    .expect("detach reservation must be durable before bridge execution");
                assert_eq!(
                    reserved.kind,
                    crate::PendingExternalCommitKind::MetadataDetach
                );
                assert_eq!(reserved.operation_id, draft.operation_id);
                assert_eq!(reserved.request_digest, request.request_digest);
                draft.request = Some(request.clone());
                draft.status = match terminal_status {
                    Ymm4MetadataDetachStatus::NotStarted => ReconciliationDetachStatus::Failed,
                    Ymm4MetadataDetachStatus::RolledBack => ReconciliationDetachStatus::RolledBack,
                    _ => unreachable!(),
                };
                let receipt = test_terminal_detach_receipt(&request, terminal_status);
                draft.error.clone_from(&receipt.error);
                draft.receipt = Some(receipt.clone());
                let terminal = store
                    .append(
                        TaskKind::ReconciliationChild,
                        &draft.child_task_id,
                        &ReconciliationChildTask::MetadataDetach(draft.clone()),
                        Some(child.generation),
                    )
                    .unwrap();
                assert_eq!(
                    canonical
                        .snapshot()
                        .unwrap()
                        .pending_external_commit
                        .as_ref()
                        .map(|pending| pending.operation_id),
                    Some(draft.operation_id),
                    "publishing the terminal child must not silently clear the canonical reservation"
                );
                if abort_before_reopen {
                    canonical
                        .abort_metadata_detach_reservation(
                            draft.operation_id,
                            &request.request_digest,
                        )
                        .unwrap();
                }
                drop(canonical);
                drop(store);

                let reopened_canonical =
                    DurableProjectStore::open(&canonical_root, "project-a").unwrap();
                let reopened_store = ProjectOperationStore::new(&operation_root);
                let latest = reopened_store
                    .validated_reconciliation_child(&child_reference.child_task_id)
                    .unwrap();
                assert_eq!(latest.generation, terminal.generation);
                let ReconciliationChildTask::MetadataDetach(latest_draft) = &latest.payload else {
                    panic!("expected persisted terminal detach child");
                };
                assert_eq!(
                    latest_draft
                        .receipt
                        .as_ref()
                        .map(|terminal| terminal.status),
                    Some(terminal_status)
                );
                assert_eq!(
                    reopened_canonical
                        .snapshot()
                        .unwrap()
                        .pending_external_commit
                        .is_none(),
                    abort_before_reopen,
                    "the reopen fixture must stop at the selected persisted crash boundary"
                );
                let reissued = reopened_store
                    .complete_persisted_detach_reissue(&reopened_canonical, latest)
                    .unwrap();
                let reissued_generation = reissued.generation;
                let ReconciliationChildTask::MetadataDetach(reissued_draft) = reissued.payload
                else {
                    panic!("expected reissued detach child");
                };
                assert_eq!(
                    reissued_draft.status,
                    ReconciliationDetachStatus::PreviewReady
                );
                assert_ne!(reissued_draft.operation_id, draft.operation_id);
                assert_ne!(reissued_draft.patch.digest, draft.patch.digest);
                assert!(reissued_draft.patch.approved_digest.is_none());
                assert!(reissued_draft.request.is_none() && reissued_draft.receipt.is_none());
                assert!(
                    reopened_canonical
                        .snapshot()
                        .unwrap()
                        .pending_external_commit
                        .is_none()
                );

                drop(reopened_canonical);
                drop(reopened_store);
                let final_store = ProjectOperationStore::new(&operation_root);
                let persisted_reissue = final_store
                    .validated_reconciliation_child(&child_reference.child_task_id)
                    .unwrap();
                assert_eq!(persisted_reissue.generation, reissued_generation);
                let ReconciliationChildTask::MetadataDetach(persisted_draft) =
                    persisted_reissue.payload
                else {
                    panic!("expected persisted reissued detach child");
                };
                assert_eq!(persisted_draft.operation_id, reissued_draft.operation_id);
                assert_eq!(persisted_draft.patch.digest, reissued_draft.patch.digest);
                assert_eq!(
                    persisted_draft.status,
                    ReconciliationDetachStatus::PreviewReady
                );
                fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn re_export_manifest_projection_must_exactly_match_canonical() {
        let source = ReconciliationSource {
            source_revision: RevisionId(5),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            target_identity_digest: "sha256:target".into(),
            state_profile_digest: "sha256:profile".into(),
            capability_digest: "sha256:capabilities".into(),
            mutation_feature_version: 1,
            mutation_feature_schema_digest: "sha256:feature-schema".into(),
            mutation_feature_properties_digest: "sha256:feature-properties".into(),
        };
        let utterance = ManagedUtterance {
            entity_id: "utt-01".into(),
            revision: 3,
            speaker: "marisa".into(),
            caption: "第二形態だぜ".into(),
            spoken_text: "だいにけいたいだぜ".into(),
            audio_path: "voice.wav".into(),
            artifact_hash: "a".repeat(64),
            frame: 120,
            length: 90,
            audio_layer: 10,
            caption_layer: 20,
        };
        let canonical = project_portable_manifest(std::slice::from_ref(&utterance));
        require_re_export_projection(
            &source,
            &canonical,
            project_portable_manifest(std::slice::from_ref(&utterance)),
        )
        .unwrap();

        let mut changed = utterance;
        changed.caption = "勝手に変えた字幕".into();
        assert!(matches!(
            require_re_export_projection(
                &source,
                &canonical,
                project_portable_manifest(&[changed]),
            ),
            Err(ProjectOperationError::ReExportManifestMismatch)
        ));
    }

    #[test]
    fn native_voice_re_export_rejects_hidden_unrelated_delete() {
        let realization_id = Uuid::new_v4();
        let identity = ManagedSemanticIdentity {
            entity_id: "utt-01".into(),
            realization_id: Some(realization_id),
        };
        let mutation = Ymm4NativeVoiceMutation {
            realization_id,
            entity_id: identity.entity_id.clone(),
            revision: 3,
            character_name: "魔理沙".into(),
            display_text: "第二形態だぜ".into(),
            spoken_text: "だいにけいたいだぜ".into(),
            frame: 120,
            layer: 20,
            max_length: 90,
            action: takegraph_node::Ymm4NativeVoiceMutationAction::Update,
        };
        let canonical = project_native_voice_manifest(std::slice::from_ref(&mutation));
        require_native_voice_re_export_scope(
            &identity,
            &canonical,
            std::slice::from_ref(&mutation),
        )
        .unwrap();

        let unrelated_delete = Ymm4NativeVoiceMutation {
            realization_id: Uuid::new_v4(),
            entity_id: "unrelated".into(),
            action: takegraph_node::Ymm4NativeVoiceMutationAction::Delete,
            ..mutation.clone()
        };
        assert!(matches!(
            require_native_voice_re_export_scope(
                &identity,
                &canonical,
                &[mutation.clone(), unrelated_delete]
            ),
            Err(ProjectOperationError::ReExportManifestMismatch)
        ));
        assert!(matches!(
            require_native_voice_re_export_scope(&identity, &[], std::slice::from_ref(&mutation)),
            Err(ProjectOperationError::ReExportManifestMismatch)
        ));
        let exact_delete = Ymm4NativeVoiceMutation {
            action: takegraph_node::Ymm4NativeVoiceMutationAction::Delete,
            ..mutation
        };
        require_native_voice_re_export_scope(&identity, &[], &[exact_delete]).unwrap();
    }

    #[test]
    fn materialized_parent_rejects_missing_or_rebound_child_references() {
        let root = test_root();
        let store = ProjectOperationStore::new(&root);
        let expected = semantic_item("utt-01", "canonical");
        let report = SemanticDriftReport::build(
            ReconciliationSource {
                source_revision: RevisionId(4),
                project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                state_profile_digest: "sha256:profile".into(),
                capability_digest: "sha256:capabilities".into(),
                mutation_feature_version: 1,
                mutation_feature_schema_digest: "sha256:feature-schema".into(),
                mutation_feature_properties_digest: "sha256:feature-properties".into(),
            },
            vec![expected],
            Vec::new(),
        )
        .unwrap();
        let preview = ReconciliationPreview::build(
            &report,
            vec![ReconciliationDecision {
                entry_id: report.entries[0].entry_id.clone(),
                choice: ReconciliationChoice::ReExportCanonical,
            }],
        )
        .unwrap();
        let children = store
            .materialize_reconciliation_children(&report, &preview)
            .unwrap();
        let mut parent = ReconciliationTaskRecord {
            report,
            expected_managed_state: Vec::new(),
            preview: Some(preview.clone()),
            materialized_children: children,
            status: ReconciliationTaskStatus::ActionsMaterialized,
        };
        store
            .verify_materialized_children(&parent, &preview)
            .unwrap();
        parent.materialized_children[0].action_digest = "sha256:rebound".into();
        assert!(matches!(
            store.verify_materialized_children(&parent, &preview),
            Err(ProjectOperationError::InvalidReconciliationChildren)
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
