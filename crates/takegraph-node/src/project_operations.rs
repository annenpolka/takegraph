//! Typed YMM4 checkpoint/render protocol and local final-media verification.
//!
//! The bridge performs native save/render work. This module deliberately
//! re-hashes and probes successful render output on the Rust side before the
//! service accepts an authoritative receipt.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{YMM4_BRIDGE_PROTOCOL_VERSION, Ymm4BridgeClient, Ymm4Error};

/// Domain separator for the .NET/Rust checkpoint request golden vectors.
pub const CHECKPOINT_REQUEST_DOMAIN: &str = "takegraph-ymm4-checkpoint-v2";
/// Domain separator for the .NET/Rust render request golden vectors.
pub const RENDER_REQUEST_DOMAIN: &str = "takegraph-ymm4-render-v2";
/// Versioned node-side media probe included in final receipt verification.
pub const FINAL_MEDIA_PROBE_PROFILE: &str = "takegraph-final-media-probe/mp4-v3";

/// Input fields captured before staging a verified save checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ymm4CheckpointRequestInput {
    pub operation_id: Uuid,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub target_identity_digest: String,
    pub expected_state_digest: String,
    pub checkpoint_profile_digest: String,
}

/// Idempotent, source-bound request to save the existing YMM4 project path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4CheckpointRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub target_identity_digest: String,
    pub expected_state_digest: String,
    pub checkpoint_profile_digest: String,
}

/// Tier 0 descriptor for verified existing-path checkpoint support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4CheckpointProfile {
    pub profile_digest: String,
    pub driver_profile_digest: String,
    pub existing_path_only: bool,
}

impl Ymm4CheckpointRequest {
    /// Validates required source bindings and computes the cross-runtime digest.
    ///
    /// # Errors
    ///
    /// Returns an error when a binding is empty.
    pub fn try_new(input: Ymm4CheckpointRequestInput) -> Result<Self, ProjectOperationNodeError> {
        validate_bindings(&[
            ("projectId", &input.project_id),
            ("sceneId", &input.scene_id),
            ("targetIdentityDigest", &input.target_identity_digest),
            ("expectedStateDigest", &input.expected_state_digest),
            ("checkpointProfileDigest", &input.checkpoint_profile_digest),
        ])?;
        let mut request = Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id: input.operation_id,
            request_digest: String::new(),
            project_id: input.project_id,
            scene_id: input.scene_id,
            source_revision: input.source_revision,
            target_identity_digest: input.target_identity_digest,
            expected_state_digest: input.expected_state_digest,
            checkpoint_profile_digest: input.checkpoint_profile_digest,
        };
        request.request_digest = checkpoint_request_digest(&request);
        Ok(request)
    }

    /// Recomputes the request digest after loading an editable staged file.
    #[must_use]
    pub fn recompute_digest(&self) -> String {
        checkpoint_request_digest(self)
    }
}

/// Bridge-side checkpoint lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4CheckpointStatus {
    Applying,
    Verified,
    Stale,
    Failed,
    RecoveryRequired,
}

/// Authenticated save evidence. Saving must not alter target semantic state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4CheckpointReceipt {
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub target_identity_digest: String,
    pub expected_state_digest: String,
    pub checkpoint_profile_digest: String,
    pub status: Ymm4CheckpointStatus,
    pub project_path: String,
    pub pre_file_sha256: Option<String>,
    pub post_file_sha256: Option<String>,
    pub post_file_bytes: Option<u64>,
    pub before_state_digest: String,
    pub after_state_digest: String,
    pub driver_profile_digest: String,
    pub error: Option<String>,
}

/// Whether a pre-existing render output can be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderOverwritePolicy {
    Deny,
    ReplaceExisting,
}

/// Read-only descriptor for one bridge-supported YMM4 render profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4RenderProfileDescriptor {
    pub descriptor_id: String,
    pub display_name: String,
    pub profile_digest: String,
    pub container: MediaContainer,
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub has_audio: bool,
    pub video_codec: String,
    pub audio_codec: String,
    pub audio_sample_rate: u32,
    pub pixel_format: String,
    pub writer_plugin: String,
    pub binding_manifest_digest: String,
    pub driver_profile_digest: String,
    pub bindable: bool,
    pub binding_error: Option<String>,
}

/// Render profiles returned by the Tier 0 descriptor endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4RenderProfiles {
    pub profiles: Vec<Ymm4RenderProfileDescriptor>,
    pub descriptor_set_digest: String,
}

/// Input fields captured before staging a render task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ymm4RenderRequestInput {
    pub task_id: Uuid,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub target_identity_digest: String,
    pub expected_state_digest: String,
    pub checkpoint_operation_id: Uuid,
    pub checkpoint_request_digest: String,
    pub checkpoint_project_path: String,
    pub checkpoint_file_sha256: String,
    pub checkpoint_file_bytes: u64,
    pub render_profile_digest: String,
    pub output_path: PathBuf,
    pub overwrite_policy: RenderOverwritePolicy,
}

/// Idempotent request for one authoritative YMM4 render task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4RenderRequest {
    pub protocol_version: u32,
    pub task_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub target_identity_digest: String,
    pub expected_state_digest: String,
    pub checkpoint_operation_id: Uuid,
    pub checkpoint_request_digest: String,
    pub checkpoint_project_path: String,
    pub checkpoint_file_sha256: String,
    pub checkpoint_file_bytes: u64,
    pub render_profile_digest: String,
    pub output_path: String,
    pub overwrite_policy: RenderOverwritePolicy,
}

impl Ymm4RenderRequest {
    /// Requires an explicit absolute output and computes its cross-runtime digest.
    /// Existing output is rejected unless replacement was explicitly approved.
    ///
    /// # Errors
    ///
    /// Returns an error for missing bindings, a relative path, directory target,
    /// or a denied existing output.
    pub fn try_new(input: Ymm4RenderRequestInput) -> Result<Self, ProjectOperationNodeError> {
        validate_bindings(&[
            ("projectId", &input.project_id),
            ("sceneId", &input.scene_id),
            ("targetIdentityDigest", &input.target_identity_digest),
            ("expectedStateDigest", &input.expected_state_digest),
            ("checkpointRequestDigest", &input.checkpoint_request_digest),
            ("checkpointProjectPath", &input.checkpoint_project_path),
            ("checkpointFileSha256", &input.checkpoint_file_sha256),
            ("renderProfileDigest", &input.render_profile_digest),
        ])?;
        if input.checkpoint_operation_id.is_nil()
            || input.checkpoint_file_sha256.len() != 64
            || !input
                .checkpoint_file_sha256
                .bytes()
                .all(|value| value.is_ascii_hexdigit())
            || !Path::new(&input.checkpoint_project_path).is_absolute()
        {
            return Err(ProjectOperationNodeError::InvalidCheckpointBinding);
        }
        validate_output_path(&input.output_path, input.overwrite_policy)?;
        let output_path = input
            .output_path
            .to_str()
            .ok_or_else(|| {
                ProjectOperationNodeError::OutputPathNotUnicode(input.output_path.clone())
            })?
            .to_owned();
        let mut request = Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            task_id: input.task_id,
            request_digest: String::new(),
            project_id: input.project_id,
            scene_id: input.scene_id,
            source_revision: input.source_revision,
            target_identity_digest: input.target_identity_digest,
            expected_state_digest: input.expected_state_digest,
            checkpoint_operation_id: input.checkpoint_operation_id,
            checkpoint_request_digest: input.checkpoint_request_digest,
            checkpoint_project_path: input.checkpoint_project_path,
            checkpoint_file_sha256: input.checkpoint_file_sha256,
            checkpoint_file_bytes: input.checkpoint_file_bytes,
            render_profile_digest: input.render_profile_digest,
            output_path,
            overwrite_policy: input.overwrite_policy,
        };
        request.request_digest = render_request_digest(&request);
        Ok(request)
    }

    /// Recomputes the request digest after loading an editable staged file.
    #[must_use]
    pub fn recompute_digest(&self) -> String {
        render_request_digest(self)
    }
}

/// Persistable native render lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4RenderStatus {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Succeeded,
    Failed,
    Stale,
    RecoveryRequired,
}

impl Ymm4RenderStatus {
    /// Whether no further bridge progress is legal.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Succeeded | Self::Failed | Self::Stale | Self::RecoveryRequired
        )
    }
}

/// Supported authoritative media containers. The initial node probe only
/// accepts ISO BMFF/MP4 so it can independently verify streams and dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaContainer {
    Mp4,
}

/// Final bridge media claim, checked against a local file probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4RenderedMediaReceipt {
    pub output_path: String,
    pub sha256: String,
    pub byte_length: u64,
    pub container: MediaContainer,
    pub duration_millis: u64,
    pub width: u32,
    pub height: u32,
    pub video_streams: u32,
    pub audio_streams: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    pub audio_sample_rate: Option<u32>,
    pub pixel_format: String,
    pub probe_profile: String,
    pub probe_digest: String,
}

/// Durable replace-existing write-ahead state published by the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4RenderOverwriteJournal {
    pub state: String,
    pub original_existed: bool,
    pub original_sha256: Option<String>,
    pub original_byte_length: Option<u64>,
    pub backup_path: String,
    pub quarantine_path: String,
    pub candidate_directory: String,
    pub candidate_path: String,
    pub candidate_sha256: Option<String>,
    pub candidate_byte_length: Option<u64>,
}

/// Status response for a render start, poll, or cancellation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4RenderTask {
    pub task_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub target_identity_digest: String,
    pub expected_state_digest: String,
    pub checkpoint_operation_id: Uuid,
    pub checkpoint_request_digest: String,
    pub checkpoint_project_path: String,
    pub checkpoint_file_sha256: String,
    pub checkpoint_file_bytes: u64,
    pub render_profile_digest: String,
    pub output_path: String,
    pub overwrite_policy: RenderOverwritePolicy,
    pub encode_source_path: String,
    pub overwrite_journal: Ymm4RenderOverwriteJournal,
    pub status: Ymm4RenderStatus,
    /// Integer progress avoids non-canonical floating point values.
    pub progress_basis_points: u16,
    pub phase: String,
    pub cancellable: bool,
    pub before_state_digest: String,
    pub after_state_digest: Option<String>,
    pub media: Option<Ymm4RenderedMediaReceipt>,
    pub error: Option<String>,
}

/// Digest-bound cancellation; a task ID alone is not authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4RenderCancelRequest {
    pub protocol_version: u32,
    pub task_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub expected_state_digest: String,
    pub checkpoint_operation_id: Uuid,
    pub checkpoint_request_digest: String,
    pub checkpoint_file_sha256: String,
    pub checkpoint_file_bytes: u64,
    pub render_profile_digest: String,
}

impl From<&Ymm4RenderRequest> for Ymm4RenderCancelRequest {
    fn from(request: &Ymm4RenderRequest) -> Self {
        Self {
            protocol_version: request.protocol_version,
            task_id: request.task_id,
            request_digest: request.request_digest.clone(),
            project_id: request.project_id.clone(),
            scene_id: request.scene_id.clone(),
            source_revision: request.source_revision,
            expected_state_digest: request.expected_state_digest.clone(),
            checkpoint_operation_id: request.checkpoint_operation_id,
            checkpoint_request_digest: request.checkpoint_request_digest.clone(),
            checkpoint_file_sha256: request.checkpoint_file_sha256.clone(),
            checkpoint_file_bytes: request.checkpoint_file_bytes,
            render_profile_digest: request.render_profile_digest.clone(),
        }
    }
}

/// Independently measured final output evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedMediaProbe {
    pub output_path: String,
    pub sha256: String,
    pub byte_length: u64,
    pub container: MediaContainer,
    pub duration_millis: u64,
    pub width: u32,
    pub height: u32,
    pub video_streams: u32,
    pub audio_streams: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    pub audio_sample_rate: Option<u32>,
    pub pixel_format: String,
    pub probe_profile: String,
    pub probe_digest: String,
}

impl Ymm4BridgeClient {
    /// Reads the active verified checkpoint profile without changing YMM4.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge error.
    pub async fn checkpoint_profile(&self) -> Result<Ymm4CheckpointProfile, Ymm4Error> {
        self.get_json("v2/project/checkpoint-profile").await
    }

    /// Executes a verified existing-path save checkpoint.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge error.
    pub async fn create_checkpoint(
        &self,
        request: &Ymm4CheckpointRequest,
    ) -> Result<Ymm4CheckpointReceipt, Ymm4Error> {
        self.post_json("v2/project/checkpoints", request).await
    }

    /// Replays or reads an existing checkpoint receipt.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge error.
    pub async fn checkpoint(&self, operation_id: Uuid) -> Result<Ymm4CheckpointReceipt, Ymm4Error> {
        self.get_json(&format!("v2/project/checkpoints/{operation_id}"))
            .await
    }

    /// Lists stable render descriptors without modifying YMM4.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge error.
    pub async fn render_profiles(&self) -> Result<Ymm4RenderProfiles, Ymm4Error> {
        self.get_json("v2/render/profiles").await
    }

    /// Stages/starts an idempotent authoritative render task.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge error.
    pub async fn start_render(
        &self,
        request: &Ymm4RenderRequest,
    ) -> Result<Ymm4RenderTask, Ymm4Error> {
        self.post_json("v2/render/tasks", request).await
    }

    /// Polls a persisted render task.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge error.
    pub async fn render_task(&self, task_id: Uuid) -> Result<Ymm4RenderTask, Ymm4Error> {
        self.get_json(&format!("v2/render/tasks/{task_id}")).await
    }

    /// Requests cooperative cancellation with the original request binding.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, task-state, or bridge error.
    pub async fn cancel_render(
        &self,
        request: &Ymm4RenderCancelRequest,
    ) -> Result<Ymm4RenderTask, Ymm4Error> {
        self.post_json(
            &format!("v2/render/tasks/{}/cancel", request.task_id),
            request,
        )
        .await
    }
}

/// Re-hashes and probes a completed render, then compares every bridge claim.
///
/// # Errors
///
/// Returns an error unless the task succeeded, source/target state stayed
/// stable, and local MP4 evidence exactly matches the final receipt.
pub fn verify_rendered_media(
    request: &Ymm4RenderRequest,
    task: &Ymm4RenderTask,
) -> Result<VerifiedMediaProbe, ProjectOperationNodeError> {
    validate_render_task(request, task)?;
    verify_render_checkpoint_file(request)?;
    if task.status != Ymm4RenderStatus::Succeeded {
        return Err(ProjectOperationNodeError::RenderNotSucceeded(task.status));
    }
    let (encode_sha256, encode_bytes) = sha256_file(Path::new(&task.encode_source_path))?;
    if encode_sha256 != request.checkpoint_file_sha256
        || encode_bytes != request.checkpoint_file_bytes
    {
        return Err(ProjectOperationNodeError::CheckpointFileMismatch);
    }
    if task.after_state_digest.as_deref() != Some(request.expected_state_digest.as_str()) {
        return Err(ProjectOperationNodeError::TargetStateChanged);
    }
    let claimed = task
        .media
        .as_ref()
        .ok_or(ProjectOperationNodeError::MissingMediaReceipt)?;
    if claimed.output_path != request.output_path {
        return Err(ProjectOperationNodeError::OutputPathMismatch);
    }
    let measured = probe_mp4_output(Path::new(&request.output_path))?;
    if &measured != claimed {
        return Err(ProjectOperationNodeError::MediaReceiptMismatch {
            claimed: Box::new(claimed.clone()),
            measured: Box::new(measured),
        });
    }
    Ok(VerifiedMediaProbe {
        output_path: claimed.output_path.clone(),
        sha256: claimed.sha256.clone(),
        byte_length: claimed.byte_length,
        container: claimed.container,
        duration_millis: claimed.duration_millis,
        width: claimed.width,
        height: claimed.height,
        video_streams: claimed.video_streams,
        audio_streams: claimed.audio_streams,
        fps_numerator: claimed.fps_numerator,
        fps_denominator: claimed.fps_denominator,
        video_codec: claimed.video_codec.clone(),
        audio_codec: claimed.audio_codec.clone(),
        audio_sample_rate: claimed.audio_sample_rate,
        pixel_format: claimed.pixel_format.clone(),
        probe_profile: claimed.probe_profile.clone(),
        probe_digest: claimed.probe_digest.clone(),
    })
}

/// Re-hashes the exact checkpoint file bound into a render request.
///
/// # Errors
///
/// Returns an error if the request was edited or the checkpoint bytes drifted.
pub fn verify_render_checkpoint_file(
    request: &Ymm4RenderRequest,
) -> Result<(), ProjectOperationNodeError> {
    if request.recompute_digest() != request.request_digest {
        return Err(ProjectOperationNodeError::RequestDigestMismatch);
    }
    let (sha256, byte_length) = sha256_file(Path::new(&request.checkpoint_project_path))?;
    if sha256 != request.checkpoint_file_sha256 || byte_length != request.checkpoint_file_bytes {
        return Err(ProjectOperationNodeError::CheckpointFileMismatch);
    }
    Ok(())
}

/// Verifies all immutable checkpoint bindings and the saved file hash.
///
/// # Errors
///
/// Returns an error for stale state, edited request data, non-verified status,
/// or a file whose current bytes do not match the authenticated receipt.
pub fn verify_checkpoint(
    request: &Ymm4CheckpointRequest,
    profile: &Ymm4CheckpointProfile,
    receipt: &Ymm4CheckpointReceipt,
) -> Result<(), ProjectOperationNodeError> {
    if request.recompute_digest() != request.request_digest {
        return Err(ProjectOperationNodeError::RequestDigestMismatch);
    }
    if receipt.operation_id != request.operation_id
        || receipt.request_digest != request.request_digest
        || receipt.project_id != request.project_id
        || receipt.scene_id != request.scene_id
        || receipt.source_revision != request.source_revision
        || receipt.target_identity_digest != request.target_identity_digest
        || receipt.expected_state_digest != request.expected_state_digest
        || receipt.checkpoint_profile_digest != request.checkpoint_profile_digest
    {
        return Err(ProjectOperationNodeError::ReceiptBindingMismatch);
    }
    if !profile.existing_path_only
        || profile.profile_digest != request.checkpoint_profile_digest
        || profile.driver_profile_digest != receipt.driver_profile_digest
    {
        return Err(ProjectOperationNodeError::CheckpointProfileMismatch);
    }
    if receipt.status != Ymm4CheckpointStatus::Verified {
        return Err(ProjectOperationNodeError::CheckpointNotVerified(
            receipt.status,
        ));
    }
    if receipt.before_state_digest != request.expected_state_digest
        || receipt.after_state_digest != request.expected_state_digest
    {
        return Err(ProjectOperationNodeError::TargetStateChanged);
    }
    if receipt.pre_file_sha256.as_deref().is_none_or(str::is_empty) {
        return Err(ProjectOperationNodeError::MissingCheckpointHash);
    }
    let expected_hash = receipt
        .post_file_sha256
        .as_ref()
        .ok_or(ProjectOperationNodeError::MissingCheckpointHash)?;
    if expected_hash.is_empty() {
        return Err(ProjectOperationNodeError::MissingCheckpointHash);
    }
    let expected_bytes = receipt
        .post_file_bytes
        .ok_or(ProjectOperationNodeError::MissingCheckpointHash)?;
    let (actual_hash, actual_bytes) = sha256_file(Path::new(&receipt.project_path))?;
    if &actual_hash != expected_hash || actual_bytes != expected_bytes {
        return Err(ProjectOperationNodeError::CheckpointFileMismatch);
    }
    Ok(())
}

/// Validates immutable task/request binding and progress range.
///
/// # Errors
///
/// Returns an error when an authenticated status is not the staged task.
pub fn validate_render_task(
    request: &Ymm4RenderRequest,
    task: &Ymm4RenderTask,
) -> Result<(), ProjectOperationNodeError> {
    if request.recompute_digest() != request.request_digest {
        return Err(ProjectOperationNodeError::RequestDigestMismatch);
    }
    if task.task_id != request.task_id
        || task.request_digest != request.request_digest
        || task.project_id != request.project_id
        || task.scene_id != request.scene_id
        || task.source_revision != request.source_revision
        || task.target_identity_digest != request.target_identity_digest
        || task.expected_state_digest != request.expected_state_digest
        || task.checkpoint_operation_id != request.checkpoint_operation_id
        || task.checkpoint_request_digest != request.checkpoint_request_digest
        || task.checkpoint_project_path != request.checkpoint_project_path
        || task.checkpoint_file_sha256 != request.checkpoint_file_sha256
        || task.checkpoint_file_bytes != request.checkpoint_file_bytes
        || task.render_profile_digest != request.render_profile_digest
        || task.output_path != request.output_path
        || task.overwrite_policy != request.overwrite_policy
    {
        return Err(ProjectOperationNodeError::ReceiptBindingMismatch);
    }
    if task.progress_basis_points > 10_000 {
        return Err(ProjectOperationNodeError::InvalidProgress(
            task.progress_basis_points,
        ));
    }
    let output = Path::new(&task.output_path);
    let output_parent = output
        .parent()
        .ok_or(ProjectOperationNodeError::MalformedOverwriteJournal)?;
    let expected_backup = format!(
        "{}.takegraph-backup-{}",
        task.output_path,
        task.task_id.simple()
    );
    let expected_quarantine = format!(
        "{}.takegraph-stale-{}",
        task.output_path,
        task.task_id.simple()
    );
    let expected_candidate_directory =
        output_parent.join(format!(".takegraph-render-{}", task.task_id.simple()));
    let expected_candidate = expected_candidate_directory.join("candidate.mp4");
    if task.encode_source_path.trim().is_empty()
        || task.overwrite_journal.state.trim().is_empty()
        || task.overwrite_journal.backup_path.trim().is_empty()
        || task.overwrite_journal.quarantine_path.trim().is_empty()
        || task.overwrite_journal.candidate_directory.trim().is_empty()
        || task.overwrite_journal.candidate_path.trim().is_empty()
        || task.overwrite_journal.backup_path != expected_backup
        || task.overwrite_journal.quarantine_path != expected_quarantine
        || Path::new(&task.overwrite_journal.candidate_directory) != expected_candidate_directory
        || Path::new(&task.overwrite_journal.candidate_path) != expected_candidate
        || task.overwrite_journal.candidate_sha256.is_some()
            != task.overwrite_journal.candidate_byte_length.is_some()
        || task.overwrite_journal.original_existed
            != (task.overwrite_journal.original_sha256.is_some()
                && task.overwrite_journal.original_byte_length.is_some())
        || task
            .overwrite_journal
            .original_sha256
            .as_deref()
            .is_some_and(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        || task
            .overwrite_journal
            .candidate_sha256
            .as_deref()
            .is_some_and(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        || !matches!(
            task.overwrite_journal.state.as_str(),
            "prepared"
                | "encoding_candidate"
                | "candidate_verified"
                | "candidate_published"
                | "committed"
                | "quarantined_and_restored"
                | "restored"
        )
    {
        return Err(ProjectOperationNodeError::MalformedOverwriteJournal);
    }
    Ok(())
}

fn checkpoint_request_digest(request: &Ymm4CheckpointRequest) -> String {
    let mut canonical = format!("{CHECKPOINT_REQUEST_DOMAIN}\n");
    write_number(&mut canonical, "protocolVersion", request.protocol_version);
    write_string(
        &mut canonical,
        "operationId",
        &request.operation_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", &request.project_id);
    write_string(&mut canonical, "sceneId", &request.scene_id);
    write_number(&mut canonical, "sourceRevision", request.source_revision);
    write_string(
        &mut canonical,
        "targetIdentityDigest",
        &request.target_identity_digest,
    );
    write_string(
        &mut canonical,
        "expectedStateDigest",
        &request.expected_state_digest,
    );
    write_string(
        &mut canonical,
        "checkpointProfileDigest",
        &request.checkpoint_profile_digest,
    );
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn render_request_digest(request: &Ymm4RenderRequest) -> String {
    let mut canonical = format!("{RENDER_REQUEST_DOMAIN}\n");
    write_number(&mut canonical, "protocolVersion", request.protocol_version);
    write_string(
        &mut canonical,
        "taskId",
        &request.task_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", &request.project_id);
    write_string(&mut canonical, "sceneId", &request.scene_id);
    write_number(&mut canonical, "sourceRevision", request.source_revision);
    write_string(
        &mut canonical,
        "targetIdentityDigest",
        &request.target_identity_digest,
    );
    write_string(
        &mut canonical,
        "expectedStateDigest",
        &request.expected_state_digest,
    );
    write_string(
        &mut canonical,
        "checkpointOperationId",
        &request.checkpoint_operation_id.hyphenated().to_string(),
    );
    write_string(
        &mut canonical,
        "checkpointRequestDigest",
        &request.checkpoint_request_digest,
    );
    write_string(
        &mut canonical,
        "checkpointProjectPath",
        &request.checkpoint_project_path,
    );
    write_string(
        &mut canonical,
        "checkpointFileSha256",
        &request.checkpoint_file_sha256,
    );
    write_number(
        &mut canonical,
        "checkpointFileBytes",
        request.checkpoint_file_bytes,
    );
    write_string(
        &mut canonical,
        "renderProfileDigest",
        &request.render_profile_digest,
    );
    write_string(&mut canonical, "outputPath", &request.output_path);
    write_string(
        &mut canonical,
        "overwritePolicy",
        match request.overwrite_policy {
            RenderOverwritePolicy::Deny => "deny",
            RenderOverwritePolicy::ReplaceExisting => "replace_existing",
        },
    );
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn write_string(canonical: &mut String, label: &str, value: &str) {
    use std::fmt::Write as _;
    let _ = writeln!(canonical, "{label}:{}:{value}", value.len());
}

fn write_number(canonical: &mut String, label: &str, value: impl std::fmt::Display) {
    use std::fmt::Write as _;
    let _ = writeln!(canonical, "{label}:{value}");
}

fn validate_bindings(
    bindings: &[(&'static str, &String)],
) -> Result<(), ProjectOperationNodeError> {
    if let Some((name, _)) = bindings.iter().find(|(_, value)| value.trim().is_empty()) {
        return Err(ProjectOperationNodeError::EmptyField(name));
    }
    Ok(())
}

fn validate_output_path(
    path: &Path,
    overwrite: RenderOverwritePolicy,
) -> Result<(), ProjectOperationNodeError> {
    if !path.is_absolute() {
        return Err(ProjectOperationNodeError::OutputMustBeAbsolute(
            path.to_path_buf(),
        ));
    }
    if path.is_dir() {
        return Err(ProjectOperationNodeError::OutputIsDirectory(
            path.to_path_buf(),
        ));
    }
    if path.exists() && overwrite == RenderOverwritePolicy::Deny {
        return Err(ProjectOperationNodeError::OutputExists(path.to_path_buf()));
    }
    Ok(())
}

fn probe_mp4_output(path: &Path) -> Result<Ymm4RenderedMediaReceipt, ProjectOperationNodeError> {
    reject_media_reparse_path(path)?;
    let mut file = open_media_for_probe(path)?;
    let opened_identity = media_file_identity(path, &file)?;
    let byte_length = file.metadata()?.len();
    let mut header = [0_u8; 12];
    file.read_exact(&mut header)?;
    if &header[4..8] != b"ftyp" {
        return Err(ProjectOperationNodeError::UnsupportedMediaContainer);
    }
    let mut accumulator = Mp4Probe::default();
    scan_mp4_boxes(&mut file, 0, byte_length, 0, &mut accumulator)?;
    let post_parse_length = file.metadata()?.len();
    if post_parse_length != byte_length {
        return Err(ProjectOperationNodeError::MediaChangedDuringProbe);
    }
    file.seek(SeekFrom::Start(0))?;
    let sha256 = sha256_reader(&mut file)?;
    if file.metadata()?.len() != byte_length {
        return Err(ProjectOperationNodeError::MediaChangedDuringProbe);
    }
    reject_media_reparse_path(path)?;
    let mut path_file = open_media_for_probe(path)?;
    if media_file_identity(path, &path_file)? != opened_identity
        || path_file.metadata()?.len() != byte_length
        || sha256_reader(&mut path_file)? != sha256
    {
        return Err(ProjectOperationNodeError::MediaChangedDuringProbe);
    }
    finish_mp4_probe(path, accumulator, sha256, byte_length)
}

fn finish_mp4_probe(
    path: &Path,
    accumulator: Mp4Probe,
    sha256: String,
    byte_length: u64,
) -> Result<Ymm4RenderedMediaReceipt, ProjectOperationNodeError> {
    let duration_millis = accumulator
        .duration_millis
        .ok_or(ProjectOperationNodeError::IncompleteMediaProbe("duration"))?;
    let width = accumulator
        .width
        .ok_or(ProjectOperationNodeError::IncompleteMediaProbe("width"))?;
    let height = accumulator
        .height
        .ok_or(ProjectOperationNodeError::IncompleteMediaProbe("height"))?;
    if accumulator.video_streams != 1 {
        return Err(ProjectOperationNodeError::IncompleteMediaProbe(
            "exactly one video stream",
        ));
    }
    if accumulator.audio_streams > 1 {
        return Err(ProjectOperationNodeError::IncompleteMediaProbe(
            "at most one audio stream",
        ));
    }
    let video_codec =
        accumulator
            .video_codec
            .ok_or(ProjectOperationNodeError::IncompleteMediaProbe(
                "video codec",
            ))?;
    let pixel_format =
        accumulator
            .pixel_format
            .ok_or(ProjectOperationNodeError::IncompleteMediaProbe(
                "pixel format",
            ))?;
    let fps_numerator =
        accumulator
            .fps_numerator
            .ok_or(ProjectOperationNodeError::IncompleteMediaProbe(
                "frame rate numerator",
            ))?;
    let fps_denominator =
        accumulator
            .fps_denominator
            .ok_or(ProjectOperationNodeError::IncompleteMediaProbe(
                "frame rate denominator",
            ))?;
    if accumulator.audio_streams > 0
        && (accumulator.audio_codec.is_none() || accumulator.audio_sample_rate.is_none())
    {
        return Err(ProjectOperationNodeError::IncompleteMediaProbe(
            "audio codec",
        ));
    }
    let probe_profile = FINAL_MEDIA_PROBE_PROFILE.to_owned();
    let probe_digest = media_probe_digest(
        &sha256,
        byte_length,
        MediaContainer::Mp4,
        duration_millis,
        width,
        height,
        accumulator.video_streams,
        accumulator.audio_streams,
        fps_numerator,
        fps_denominator,
        &video_codec,
        accumulator.audio_codec.as_deref(),
        accumulator.audio_sample_rate,
        &pixel_format,
        &probe_profile,
    );
    Ok(Ymm4RenderedMediaReceipt {
        output_path: path.to_string_lossy().into_owned(),
        sha256,
        byte_length,
        container: MediaContainer::Mp4,
        duration_millis,
        width,
        height,
        video_streams: accumulator.video_streams,
        audio_streams: accumulator.audio_streams,
        fps_numerator,
        fps_denominator,
        video_codec,
        audio_codec: accumulator.audio_codec,
        audio_sample_rate: accumulator.audio_sample_rate,
        pixel_format,
        probe_profile,
        probe_digest,
    })
}

#[cfg(windows)]
fn open_media_for_probe(path: &Path) -> Result<File, std::io::Error> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};
    const FILE_SHARE_READ: u32 = 1;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
fn open_media_for_probe(path: &Path) -> Result<File, std::io::Error> {
    File::open(path)
}

#[cfg(windows)]
fn media_file_identity(path: &Path, file: &File) -> Result<(u64, u64, u64), std::io::Error> {
    use std::{os::windows::fs::MetadataExt as _, time::UNIX_EPOCH};
    let metadata = file.metadata()?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    let canonical_path = std::fs::canonicalize(path)?;
    let mut path_hasher = Sha256::new();
    path_hasher.update(canonical_path.as_os_str().to_string_lossy().as_bytes());
    let digest = path_hasher.finalize();
    Ok((
        metadata.file_size(),
        modified.as_secs(),
        u64::from_be_bytes(digest[..8].try_into().expect("eight bytes")),
    ))
}

#[cfg(unix)]
fn media_file_identity(_path: &Path, file: &File) -> Result<(u64, u64), std::io::Error> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

fn reject_media_reparse_path(path: &Path) -> Result<(), std::io::Error> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if !current.exists() {
            continue;
        }
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "final media path contains a symlink/reparse point: {}",
                    current.display()
                ),
            ));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if std::fs::symlink_metadata(&current)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "final media path contains a reparse point: {}",
                        current.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Mp4Probe {
    duration_millis: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    video_streams: u32,
    audio_streams: u32,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    audio_sample_rate: Option<u32>,
    pixel_format: Option<String>,
    fps_numerator: Option<u32>,
    fps_denominator: Option<u32>,
}

#[derive(Default)]
struct Mp4TrackProbe {
    handler: Option<[u8; 4]>,
    width: Option<u32>,
    height: Option<u32>,
    timescale: Option<u32>,
    sample_delta: Option<u32>,
    time_to_sample_seen: bool,
    sample_entries: Vec<Mp4SampleEntryProbe>,
}

struct Mp4SampleEntryProbe {
    kind: [u8; 4],
    video_codec: Option<&'static str>,
    audio_codec: Option<&'static str>,
    audio_sample_rate: Option<u32>,
    pixel_format: Option<&'static str>,
}

fn scan_mp4_boxes(
    file: &mut File,
    start: u64,
    end: u64,
    depth: u8,
    probe: &mut Mp4Probe,
) -> Result<(), ProjectOperationNodeError> {
    if depth > 7 {
        return Ok(());
    }
    let mut offset = start;
    while offset
        .checked_add(8)
        .is_some_and(|header_end| header_end <= end)
    {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; 16];
        file.read_exact(&mut header[..8])?;
        let short_size = u32::from_be_bytes(header[..4].try_into().expect("four bytes"));
        let kind: [u8; 4] = header[4..8].try_into().expect("four bytes");
        let (header_size, box_size) = if short_size == 1 {
            file.read_exact(&mut header[8..16])?;
            (
                16_u64,
                u64::from_be_bytes(header[8..16].try_into().expect("eight bytes")),
            )
        } else if short_size == 0 {
            (8_u64, end - offset)
        } else {
            (8_u64, u64::from(short_size))
        };
        if box_size < header_size
            || offset
                .checked_add(box_size)
                .is_none_or(|box_end| box_end > end)
        {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        let data_start = offset + header_size;
        let box_end = offset + box_size;
        match &kind {
            b"moov" => {
                scan_mp4_boxes(file, data_start, box_end, depth + 1, probe)?;
            }
            b"trak" => {
                let mut track = Mp4TrackProbe::default();
                scan_mp4_track_boxes(file, data_start, box_end, depth + 1, &mut track)?;
                complete_mp4_track(&track, probe)?;
            }
            b"mvhd" => read_mvhd(file, data_start, box_end, probe)?,
            _ => {}
        }
        offset = box_end;
    }
    Ok(())
}

fn scan_mp4_track_boxes(
    file: &mut File,
    start: u64,
    end: u64,
    depth: u8,
    track: &mut Mp4TrackProbe,
) -> Result<(), ProjectOperationNodeError> {
    if depth > 7 {
        return Ok(());
    }
    let mut offset = start;
    while offset
        .checked_add(8)
        .is_some_and(|header_end| header_end <= end)
    {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; 16];
        file.read_exact(&mut header[..8])?;
        let short_size = u32::from_be_bytes(header[..4].try_into().expect("four bytes"));
        let kind: [u8; 4] = header[4..8].try_into().expect("four bytes");
        let (header_size, box_size) = if short_size == 1 {
            file.read_exact(&mut header[8..16])?;
            (
                16_u64,
                u64::from_be_bytes(header[8..16].try_into().expect("eight bytes")),
            )
        } else if short_size == 0 {
            (8_u64, end - offset)
        } else {
            (8_u64, u64::from(short_size))
        };
        if box_size < header_size
            || offset
                .checked_add(box_size)
                .is_none_or(|box_end| box_end > end)
        {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        let data_start = offset + header_size;
        let box_end = offset + box_size;
        match &kind {
            b"mdia" | b"minf" | b"stbl" => {
                scan_mp4_track_boxes(file, data_start, box_end, depth + 1, track)?;
            }
            b"tkhd" => read_tkhd(file, data_start, box_end, track)?,
            b"hdlr" => read_hdlr(file, data_start, box_end, track)?,
            b"mdhd" => read_mdhd(file, data_start, box_end, track)?,
            b"stts" => read_stts(file, data_start, box_end, track)?,
            b"stsd" => read_stsd(file, data_start, box_end, track)?,
            _ => {}
        }
        offset = box_end;
    }
    Ok(())
}

fn read_mvhd(
    file: &mut File,
    start: u64,
    end: u64,
    probe: &mut Mp4Probe,
) -> Result<(), ProjectOperationNodeError> {
    file.seek(SeekFrom::Start(start))?;
    let mut data = [0_u8; 32];
    let available = usize::try_from((end - start).min(data.len() as u64))
        .map_err(|_| ProjectOperationNodeError::MalformedMp4)?;
    file.read_exact(&mut data[..available])?;
    let version = data[0];
    let (timescale, duration) = if version == 0 && available >= 20 {
        (
            u32::from_be_bytes(data[12..16].try_into().expect("four bytes")),
            u64::from(u32::from_be_bytes(
                data[16..20].try_into().expect("four bytes"),
            )),
        )
    } else if version == 1 && available >= 32 {
        (
            u32::from_be_bytes(data[20..24].try_into().expect("four bytes")),
            u64::from_be_bytes(data[24..32].try_into().expect("eight bytes")),
        )
    } else {
        return Err(ProjectOperationNodeError::MalformedMp4);
    };
    if timescale == 0 {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    probe.duration_millis = Some(
        duration
            .saturating_mul(1_000)
            .checked_div(u64::from(timescale))
            .unwrap_or_default(),
    );
    Ok(())
}

fn read_tkhd(
    file: &mut File,
    start: u64,
    end: u64,
    track: &mut Mp4TrackProbe,
) -> Result<(), ProjectOperationNodeError> {
    file.seek(SeekFrom::Start(start))?;
    let mut data = [0_u8; 104];
    let available = usize::try_from((end - start).min(data.len() as u64))
        .map_err(|_| ProjectOperationNodeError::MalformedMp4)?;
    file.read_exact(&mut data[..available])?;
    let dimension_offset = match data[0] {
        0 if available >= 84 => 76,
        1 if available >= 96 => 88,
        _ => return Err(ProjectOperationNodeError::MalformedMp4),
    };
    let width_fixed = u32::from_be_bytes(
        data[dimension_offset..dimension_offset + 4]
            .try_into()
            .expect("four bytes"),
    );
    let height_fixed = u32::from_be_bytes(
        data[dimension_offset + 4..dimension_offset + 8]
            .try_into()
            .expect("four bytes"),
    );
    let width = width_fixed >> 16;
    let height = height_fixed >> 16;
    if width > 0
        && height > 0
        && (track.width.replace(width).is_some() || track.height.replace(height).is_some())
    {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    Ok(())
}

fn read_hdlr(
    file: &mut File,
    start: u64,
    end: u64,
    track: &mut Mp4TrackProbe,
) -> Result<(), ProjectOperationNodeError> {
    if end - start < 12 {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    file.seek(SeekFrom::Start(start + 8))?;
    let mut handler = [0_u8; 4];
    file.read_exact(&mut handler)?;
    if track.handler.replace(handler).is_some() {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    Ok(())
}

fn read_mdhd(
    file: &mut File,
    start: u64,
    end: u64,
    track: &mut Mp4TrackProbe,
) -> Result<(), ProjectOperationNodeError> {
    file.seek(SeekFrom::Start(start))?;
    let mut data = [0_u8; 24];
    let available = usize::try_from((end - start).min(data.len() as u64))
        .map_err(|_| ProjectOperationNodeError::MalformedMp4)?;
    file.read_exact(&mut data[..available])?;
    let timescale = match data[0] {
        0 if available >= 16 => u32::from_be_bytes(data[12..16].try_into().expect("four bytes")),
        1 if available >= 24 => u32::from_be_bytes(data[20..24].try_into().expect("four bytes")),
        _ => return Err(ProjectOperationNodeError::MalformedMp4),
    };
    if timescale == 0 || track.timescale.replace(timescale).is_some() {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    Ok(())
}

fn read_stts(
    file: &mut File,
    start: u64,
    end: u64,
    track: &mut Mp4TrackProbe,
) -> Result<(), ProjectOperationNodeError> {
    if end - start < 8 || track.time_to_sample_seen {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    track.time_to_sample_seen = true;
    file.seek(SeekFrom::Start(start))?;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header)?;
    let entry_count = u32::from_be_bytes(header[4..8].try_into().expect("four bytes"));
    if entry_count == 0 || entry_count > 1_000_000 || end - start != 8 + u64::from(entry_count) * 8
    {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    let mut entry = [0_u8; 8];
    for _ in 0..entry_count {
        file.read_exact(&mut entry)?;
        let sample_count = u32::from_be_bytes(entry[..4].try_into().expect("four bytes"));
        let sample_delta = u32::from_be_bytes(entry[4..8].try_into().expect("four bytes"));
        if sample_count == 0 || sample_delta == 0 {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        if entry_count == 1 {
            track.sample_delta = Some(sample_delta);
        }
    }
    Ok(())
}

fn read_stsd(
    file: &mut File,
    start: u64,
    end: u64,
    track: &mut Mp4TrackProbe,
) -> Result<(), ProjectOperationNodeError> {
    if end - start < 8 {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    file.seek(SeekFrom::Start(start + 4))?;
    let mut count_bytes = [0_u8; 4];
    file.read_exact(&mut count_bytes)?;
    let count = u32::from_be_bytes(count_bytes);
    if count == 0 || count > 64 {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    let mut offset = start + 8;
    for _ in 0..count {
        if offset
            .checked_add(8)
            .is_none_or(|header_end| header_end > end)
        {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let size = u64::from(u32::from_be_bytes(
            header[..4].try_into().expect("four bytes"),
        ));
        if size < 8
            || offset
                .checked_add(size)
                .is_none_or(|entry_end| entry_end > end)
        {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        let kind: [u8; 4] = header[4..8].try_into().expect("four bytes");
        let entry_end = offset + size;
        let entry = match &kind {
            b"avc1" | b"avc3" => read_avc_sample_entry(file, offset, entry_end, kind)?,
            b"mp4a" => read_mp4a_sample_entry(file, offset, entry_end)?,
            _ => Mp4SampleEntryProbe {
                kind,
                video_codec: None,
                audio_codec: None,
                audio_sample_rate: None,
                pixel_format: None,
            },
        };
        track.sample_entries.push(entry);
        offset += size;
    }
    Ok(())
}

fn read_avc_sample_entry(
    file: &mut File,
    start: u64,
    end: u64,
    kind: [u8; 4],
) -> Result<Mp4SampleEntryProbe, ProjectOperationNodeError> {
    const FIXED_LENGTH: u64 = 8 + 78;
    if end - start < FIXED_LENGTH {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    let config = read_unique_child_box(file, start + FIXED_LENGTH, end, *b"avcC")?;
    let pixel_format = parse_avc_pixel_format(&config)?;
    Ok(Mp4SampleEntryProbe {
        kind,
        video_codec: Some("h264"),
        audio_codec: None,
        audio_sample_rate: None,
        pixel_format: Some(pixel_format),
    })
}

fn read_mp4a_sample_entry(
    file: &mut File,
    start: u64,
    end: u64,
) -> Result<Mp4SampleEntryProbe, ProjectOperationNodeError> {
    const FIXED_LENGTH: u64 = 8 + 28;
    if end - start < FIXED_LENGTH {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    file.seek(SeekFrom::Start(start + 8))?;
    let mut fixed_data = [0_u8; 28];
    file.read_exact(&mut fixed_data)?;
    let fixed_sample_rate = u32::from_be_bytes(fixed_data[24..28].try_into().expect("four bytes"));
    if fixed_sample_rate & 0xffff != 0 || fixed_sample_rate >> 16 == 0 {
        return Err(ProjectOperationNodeError::UnsupportedAudioSampleRate);
    }
    let esds = read_unique_child_box(file, start + FIXED_LENGTH, end, *b"esds")?;
    let (audio_codec, config_sample_rate) = parse_audio_specific_config(&esds)?;
    let sample_rate = fixed_sample_rate >> 16;
    if sample_rate != config_sample_rate {
        return Err(ProjectOperationNodeError::UnsupportedAudioSampleRate);
    }
    Ok(Mp4SampleEntryProbe {
        kind: *b"mp4a",
        video_codec: None,
        audio_codec: Some(audio_codec),
        audio_sample_rate: Some(sample_rate),
        pixel_format: None,
    })
}

fn read_unique_child_box(
    file: &mut File,
    start: u64,
    end: u64,
    required_kind: [u8; 4],
) -> Result<Vec<u8>, ProjectOperationNodeError> {
    let mut found = None;
    let mut offset = start;
    while offset
        .checked_add(8)
        .is_some_and(|header_end| header_end <= end)
    {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; 16];
        file.read_exact(&mut header[..8])?;
        let short_size = u32::from_be_bytes(header[..4].try_into().expect("four bytes"));
        let kind: [u8; 4] = header[4..8].try_into().expect("four bytes");
        let (header_size, box_size) = if short_size == 1 {
            file.read_exact(&mut header[8..16])?;
            (
                16_u64,
                u64::from_be_bytes(header[8..16].try_into().expect("eight bytes")),
            )
        } else if short_size == 0 {
            (8_u64, end - offset)
        } else {
            (8_u64, u64::from(short_size))
        };
        if box_size < header_size
            || offset
                .checked_add(box_size)
                .is_none_or(|box_end| box_end > end)
        {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        if kind == required_kind {
            if found.is_some() || box_size - header_size > 1024 * 1024 {
                return Err(ProjectOperationNodeError::MalformedMp4);
            }
            let mut payload = vec![
                0;
                usize::try_from(box_size - header_size)
                    .map_err(|_| ProjectOperationNodeError::MalformedMp4)?
            ];
            file.read_exact(&mut payload)?;
            found = Some(payload);
        }
        offset += box_size;
    }
    found.ok_or(ProjectOperationNodeError::MalformedMp4)
}

fn parse_avc_pixel_format(config: &[u8]) -> Result<&'static str, ProjectOperationNodeError> {
    if config.len() < 8 || config[0] != 1 {
        return Err(ProjectOperationNodeError::UnsupportedPixelFormat);
    }
    let count = config[5] & 0x1f;
    if count == 0 {
        return Err(ProjectOperationNodeError::UnsupportedPixelFormat);
    }
    let mut offset = 6_usize;
    let mut format = None;
    for _ in 0..count {
        let length_bytes = config
            .get(offset..offset + 2)
            .ok_or(ProjectOperationNodeError::MalformedMp4)?;
        let length = usize::from(u16::from_be_bytes(
            length_bytes.try_into().expect("two bytes"),
        ));
        offset += 2;
        let sps = config
            .get(offset..offset + length)
            .ok_or(ProjectOperationNodeError::MalformedMp4)?;
        if length < 4 {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        let measured = parse_sps_pixel_format(sps)?;
        if format.is_some_and(|existing| existing != measured) {
            return Err(ProjectOperationNodeError::UnsupportedPixelFormat);
        }
        format = Some(measured);
        offset += length;
    }
    format.ok_or(ProjectOperationNodeError::UnsupportedPixelFormat)
}

fn parse_sps_pixel_format(sps: &[u8]) -> Result<&'static str, ProjectOperationNodeError> {
    if sps.first().is_none_or(|value| value & 0x1f != 7) {
        return Err(ProjectOperationNodeError::UnsupportedPixelFormat);
    }
    let mut rbsp = Vec::with_capacity(sps.len());
    let mut zeroes = 0;
    for &value in &sps[1..] {
        if zeroes >= 2 && value == 3 {
            zeroes = 0;
            continue;
        }
        rbsp.push(value);
        zeroes = if value == 0 { zeroes + 1 } else { 0 };
    }
    let mut bits = AvcBitReader::new(&rbsp);
    let profile = bits.read_bits(8)?;
    let _ = bits.read_bits(8)?;
    let _ = bits.read_bits(8)?;
    let _ = bits.read_ue()?;
    let mut chroma_format = 1;
    let mut separate_colour_plane = false;
    let mut bit_depth_luma_minus_8 = 0;
    let mut bit_depth_chroma_minus_8 = 0;
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format = bits.read_ue()?;
        if chroma_format == 3 {
            separate_colour_plane = bits.read_bits(1)? != 0;
        }
        bit_depth_luma_minus_8 = bits.read_ue()?;
        bit_depth_chroma_minus_8 = bits.read_ue()?;
    }
    if chroma_format != 1
        || separate_colour_plane
        || bit_depth_luma_minus_8 != 0
        || bit_depth_chroma_minus_8 != 0
    {
        return Err(ProjectOperationNodeError::UnsupportedPixelFormat);
    }
    Ok("yuv420p")
}

struct AvcBitReader<'a> {
    bytes: &'a [u8],
    bit_offset: usize,
}

impl<'a> AvcBitReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_offset: 0,
        }
    }

    fn read_bits(&mut self, count: usize) -> Result<u32, ProjectOperationNodeError> {
        if count > 32 || self.bit_offset + count > self.bytes.len() * 8 {
            return Err(ProjectOperationNodeError::MalformedMp4);
        }
        let mut value = 0_u32;
        for _ in 0..count {
            value = (value << 1)
                | u32::from((self.bytes[self.bit_offset / 8] >> (7 - self.bit_offset % 8)) & 1);
            self.bit_offset += 1;
        }
        Ok(value)
    }

    fn read_ue(&mut self) -> Result<u32, ProjectOperationNodeError> {
        let mut zeroes = 0_usize;
        while self.read_bits(1)? == 0 {
            zeroes += 1;
            if zeroes > 31 {
                return Err(ProjectOperationNodeError::MalformedMp4);
            }
        }
        if zeroes == 0 {
            return Ok(0);
        }
        Ok(((1_u32 << zeroes) - 1) + self.read_bits(zeroes)?)
    }
}

fn parse_audio_specific_config(
    esds: &[u8],
) -> Result<(&'static str, u32), ProjectOperationNodeError> {
    if esds.len() < 8 {
        return Err(ProjectOperationNodeError::UnsupportedAudioCodec);
    }
    let mut offset = 4;
    let es = read_mp4_descriptor(esds, &mut offset, 0x03)?;
    if es.len() < 3 {
        return Err(ProjectOperationNodeError::UnsupportedAudioCodec);
    }
    let flags = es[2];
    let mut es_offset = 3_usize;
    if flags & 0x80 != 0 {
        es_offset += 2;
    }
    if flags & 0x40 != 0 {
        let length = usize::from(
            *es.get(es_offset)
                .ok_or(ProjectOperationNodeError::UnsupportedAudioCodec)?,
        );
        es_offset += 1 + length;
    }
    if flags & 0x20 != 0 {
        es_offset += 2;
    }
    let decoder = read_mp4_descriptor(es, &mut es_offset, 0x04)?;
    if decoder.len() < 15 || decoder[0] != 0x40 {
        return Err(ProjectOperationNodeError::UnsupportedAudioCodec);
    }
    let mut decoder_offset = 13;
    let config = read_mp4_descriptor(decoder, &mut decoder_offset, 0x05)?;
    let mut bits = AvcBitReader::new(config);
    let object_type = bits.read_bits(5)?;
    if object_type != 2 {
        return Err(ProjectOperationNodeError::UnsupportedAudioCodec);
    }
    let frequency_index = bits.read_bits(4)?;
    let sample_rate = match frequency_index {
        0 => 96_000,
        1 => 88_200,
        2 => 64_000,
        3 => 48_000,
        4 => 44_100,
        5 => 32_000,
        6 => 24_000,
        7 => 22_050,
        8 => 16_000,
        9 => 12_000,
        10 => 11_025,
        11 => 8_000,
        12 => 7_350,
        15 => bits.read_bits(24)?,
        _ => return Err(ProjectOperationNodeError::UnsupportedAudioSampleRate),
    };
    if sample_rate == 0 {
        return Err(ProjectOperationNodeError::UnsupportedAudioSampleRate);
    }
    Ok(("aac_lc", sample_rate))
}

fn read_mp4_descriptor<'a>(
    source: &'a [u8],
    offset: &mut usize,
    expected_tag: u8,
) -> Result<&'a [u8], ProjectOperationNodeError> {
    if source.get(*offset) != Some(&expected_tag) {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    *offset += 1;
    let mut length = 0_usize;
    let mut terminated = false;
    for _ in 0..4 {
        let value = *source
            .get(*offset)
            .ok_or(ProjectOperationNodeError::MalformedMp4)?;
        *offset += 1;
        length = length
            .checked_shl(7)
            .and_then(|result| result.checked_add(usize::from(value & 0x7f)))
            .ok_or(ProjectOperationNodeError::MalformedMp4)?;
        if value & 0x80 == 0 {
            terminated = true;
            break;
        }
    }
    let end = offset
        .checked_add(length)
        .ok_or(ProjectOperationNodeError::MalformedMp4)?;
    if !terminated || end > source.len() {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    let payload = &source[*offset..end];
    *offset = end;
    Ok(payload)
}

fn complete_mp4_track(
    track: &Mp4TrackProbe,
    probe: &mut Mp4Probe,
) -> Result<(), ProjectOperationNodeError> {
    match track.handler.as_ref() {
        Some(b"vide") => {
            let width = track
                .width
                .ok_or(ProjectOperationNodeError::IncompleteMediaProbe(
                    "video width",
                ))?;
            let height = track
                .height
                .ok_or(ProjectOperationNodeError::IncompleteMediaProbe(
                    "video height",
                ))?;
            merge_number(&mut probe.width, width)?;
            merge_number(&mut probe.height, height)?;
            let timescale = track
                .timescale
                .ok_or(ProjectOperationNodeError::UnsupportedFrameRate)?;
            let sample_delta = track
                .sample_delta
                .ok_or(ProjectOperationNodeError::UnsupportedFrameRate)?;
            if track.sample_entries.is_empty()
                || !track.sample_entries.iter().all(|entry| {
                    matches!(&entry.kind, b"avc1" | b"avc3")
                        && entry.video_codec == Some("h264")
                        && entry.pixel_format == Some("yuv420p")
                })
            {
                return Err(ProjectOperationNodeError::UnsupportedVideoCodec);
            }
            probe.video_streams = probe.video_streams.saturating_add(1);
            merge_codec(&mut probe.video_codec, "h264")?;
            merge_codec(&mut probe.pixel_format, "yuv420p")?;
            let divisor = greatest_common_divisor(timescale, sample_delta);
            merge_number(&mut probe.fps_numerator, timescale / divisor)?;
            merge_number(&mut probe.fps_denominator, sample_delta / divisor)?;
        }
        Some(b"soun") => {
            if track.sample_entries.len() != 1
                || &track.sample_entries[0].kind != b"mp4a"
                || track.sample_entries[0].audio_codec != Some("aac_lc")
                || track.sample_entries[0].audio_sample_rate.is_none()
                || track.sample_entries[0].audio_sample_rate != track.timescale
            {
                return Err(ProjectOperationNodeError::UnsupportedAudioCodec);
            }
            probe.audio_streams = probe.audio_streams.saturating_add(1);
            merge_codec(&mut probe.audio_codec, "aac_lc")?;
            merge_number(
                &mut probe.audio_sample_rate,
                track.sample_entries[0]
                    .audio_sample_rate
                    .ok_or(ProjectOperationNodeError::UnsupportedAudioSampleRate)?,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn merge_number(slot: &mut Option<u32>, value: u32) -> Result<(), ProjectOperationNodeError> {
    if slot.is_some_and(|existing| existing != value) {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    *slot = Some(value);
    Ok(())
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn merge_codec(slot: &mut Option<String>, codec: &str) -> Result<(), ProjectOperationNodeError> {
    if slot.as_deref().is_some_and(|existing| existing != codec) {
        return Err(ProjectOperationNodeError::MalformedMp4);
    }
    *slot = Some(codec.to_owned());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn media_probe_digest(
    sha256: &str,
    byte_length: u64,
    container: MediaContainer,
    duration_millis: u64,
    width: u32,
    height: u32,
    video_streams: u32,
    audio_streams: u32,
    fps_numerator: u32,
    fps_denominator: u32,
    video_codec: &str,
    audio_codec: Option<&str>,
    audio_sample_rate: Option<u32>,
    pixel_format: &str,
    profile: &str,
) -> String {
    let mut canonical = String::from("takegraph-final-media-probe-v3\n");
    write_string(&mut canonical, "sha256", sha256);
    write_number(&mut canonical, "byteLength", byte_length);
    write_string(
        &mut canonical,
        "container",
        match container {
            MediaContainer::Mp4 => "mp4",
        },
    );
    write_number(&mut canonical, "durationMillis", duration_millis);
    write_number(&mut canonical, "width", width);
    write_number(&mut canonical, "height", height);
    write_number(&mut canonical, "videoStreams", video_streams);
    write_number(&mut canonical, "audioStreams", audio_streams);
    write_number(&mut canonical, "fpsNumerator", fps_numerator);
    write_number(&mut canonical, "fpsDenominator", fps_denominator);
    write_string(&mut canonical, "videoCodec", video_codec);
    write_string(
        &mut canonical,
        "audioCodec",
        audio_codec.unwrap_or_default(),
    );
    write_string(
        &mut canonical,
        "audioSampleRate",
        &audio_sample_rate.map_or_else(String::new, |value| value.to_string()),
    );
    write_string(&mut canonical, "pixelFormat", pixel_format);
    write_string(&mut canonical, "probeProfile", profile);
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn sha256_file(path: &Path) -> Result<(String, u64), std::io::Error> {
    let mut file = File::open(path)?;
    let byte_length = file.metadata()?.len();
    let digest = sha256_reader(&mut file)?;
    Ok((digest, byte_length))
}

fn sha256_reader(file: &mut File) -> Result<String, std::io::Error> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Error)]
pub enum ProjectOperationNodeError {
    #[error("required project-operation field is empty: {0}")]
    EmptyField(&'static str),
    #[error("render request has an invalid verified-checkpoint binding")]
    InvalidCheckpointBinding,
    #[error("bridge render overwrite journal is malformed")]
    MalformedOverwriteJournal,
    #[error("render output path must be absolute: {0}")]
    OutputMustBeAbsolute(PathBuf),
    #[error("render output path is a directory: {0}")]
    OutputIsDirectory(PathBuf),
    #[error("render output already exists and overwrite was denied: {0}")]
    OutputExists(PathBuf),
    #[error("render output path cannot be represented exactly as Unicode: {0}")]
    OutputPathNotUnicode(PathBuf),
    #[error("editable request digest does not match its payload")]
    RequestDigestMismatch,
    #[error("bridge receipt does not match the staged request binding")]
    ReceiptBindingMismatch,
    #[error("checkpoint was not verified: {0:?}")]
    CheckpointNotVerified(Ymm4CheckpointStatus),
    #[error("render task has not succeeded: {0:?}")]
    RenderNotSucceeded(Ymm4RenderStatus),
    #[error("bridge project state changed during the external operation")]
    TargetStateChanged,
    #[error("checkpoint receipt has no final file hash/length")]
    MissingCheckpointHash,
    #[error("checkpoint profile/driver evidence does not match the staged descriptor")]
    CheckpointProfileMismatch,
    #[error("saved project bytes do not match the checkpoint receipt")]
    CheckpointFileMismatch,
    #[error("successful render has no final media receipt")]
    MissingMediaReceipt,
    #[error("render output path differs from the approved path")]
    OutputPathMismatch,
    #[error("render progress is outside 0..=10000 basis points: {0}")]
    InvalidProgress(u16),
    #[error("final output is not an independently probeable MP4")]
    UnsupportedMediaContainer,
    #[error("final MP4 structure is malformed")]
    MalformedMp4,
    #[error("final MP4 video sample entry is not supported H.264 avc1/avc3")]
    UnsupportedVideoCodec,
    #[error("final MP4 audio sample entry is not supported AAC/mp4a")]
    UnsupportedAudioCodec,
    #[error("final MP4 audio sample rate is unsupported or internally inconsistent")]
    UnsupportedAudioSampleRate,
    #[error("final MP4 AVC SPS is not supported yuv420p")]
    UnsupportedPixelFormat,
    #[error("final MP4 does not have an exactly probeable constant frame rate")]
    UnsupportedFrameRate,
    #[error("final MP4 probe did not find {0}")]
    IncompleteMediaProbe(&'static str),
    #[error("final media file changed while it was being probed")]
    MediaChangedDuringProbe,
    #[error("bridge final media claim differs from local measurement")]
    MediaReceiptMismatch {
        claimed: Box<Ymm4RenderedMediaReceipt>,
        measured: Box<Ymm4RenderedMediaReceipt>,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = u32::try_from(payload.len() + 8).unwrap();
        let mut output = Vec::with_capacity(payload.len() + 8);
        output.extend_from_slice(&size.to_be_bytes());
        output.extend_from_slice(&kind);
        output.extend_from_slice(payload);
        output
    }

    fn mp4_descriptor(tag: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 128);
        let mut output = vec![tag, u8::try_from(payload.len()).unwrap()];
        output.extend_from_slice(payload);
        output
    }

    fn sample_entry(kind: [u8; 4]) -> Vec<u8> {
        let payload = match &kind {
            b"avc1" | b"avc3" => {
                let avcc = [
                    1, 66, 0, 30, 0xff, 0xe1, 0, 5, 0x67, 0x42, 0, 0x1e, 0x80, 1, 0, 1, 0x68,
                ];
                let mut value = vec![0; 78];
                value.extend_from_slice(&mp4_box(*b"avcC", &avcc));
                value
            }
            b"mp4a" | b"aach" => {
                let asc = mp4_descriptor(
                    0x05,
                    if &kind == b"mp4a" {
                        &[0x11, 0x90]
                    } else {
                        &[0x29, 0x90]
                    },
                );
                let mut decoder_payload = vec![0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
                decoder_payload.extend_from_slice(&asc);
                let decoder = mp4_descriptor(0x04, &decoder_payload);
                let mut es_payload = vec![0, 1, 0];
                es_payload.extend_from_slice(&decoder);
                es_payload.extend_from_slice(&mp4_descriptor(0x06, &[2]));
                let es = mp4_descriptor(0x03, &es_payload);
                let mut esds_payload = vec![0; 4];
                esds_payload.extend_from_slice(&es);
                let mut value = vec![0; 28];
                value[24..28].copy_from_slice(&(48_000_u32 << 16).to_be_bytes());
                value.extend_from_slice(&mp4_box(*b"esds", &esds_payload));
                value
            }
            _ => Vec::new(),
        };
        mp4_box(if &kind == b"aach" { *b"mp4a" } else { kind }, &payload)
    }

    fn sample_description(entries: &[[u8; 4]]) -> Vec<u8> {
        let mut value = vec![0; 4];
        value.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_be_bytes());
        for &entry in entries {
            value.extend_from_slice(&sample_entry(entry));
        }
        mp4_box(*b"stsd", &value)
    }

    fn test_track(
        handler: [u8; 4],
        width: u32,
        height: u32,
        version_one: bool,
        entries: &[[u8; 4]],
    ) -> Vec<u8> {
        let mut tkhd = vec![0_u8; if version_one { 96 } else { 84 }];
        tkhd[0] = u8::from(version_one);
        let dimension_offset = if version_one { 88 } else { 76 };
        tkhd[dimension_offset..dimension_offset + 4].copy_from_slice(&(width << 16).to_be_bytes());
        tkhd[dimension_offset + 4..dimension_offset + 8]
            .copy_from_slice(&(height << 16).to_be_bytes());
        let mut hdlr = vec![0_u8; 12];
        hdlr[8..12].copy_from_slice(&handler);
        let mut mdhd = vec![0; 24];
        mdhd[12..16].copy_from_slice(
            &(if &handler == b"vide" {
                60_000_u32
            } else {
                48_000_u32
            })
            .to_be_bytes(),
        );
        let mut stts = vec![0; 16];
        stts[4..8].copy_from_slice(&1_u32.to_be_bytes());
        stts[8..12].copy_from_slice(
            &(if &handler == b"vide" {
                150_u32
            } else {
                12_000_u32
            })
            .to_be_bytes(),
        );
        stts[12..16].copy_from_slice(
            &(if &handler == b"vide" {
                1_000_u32
            } else {
                1_u32
            })
            .to_be_bytes(),
        );
        let mut stbl = mp4_box(*b"stts", &stts);
        stbl.extend_from_slice(&sample_description(entries));
        let mut mdia_payload = mp4_box(*b"hdlr", &hdlr);
        mdia_payload.splice(0..0, mp4_box(*b"mdhd", &mdhd));
        mdia_payload.extend_from_slice(&mp4_box(*b"minf", &mp4_box(*b"stbl", &stbl)));
        let mdia = mp4_box(*b"mdia", &mdia_payload);
        let mut trak = mp4_box(*b"tkhd", &tkhd);
        trak.extend_from_slice(&mdia);
        mp4_box(*b"trak", &trak)
    }

    fn test_mp4() -> Vec<u8> {
        let mut ftyp = Vec::from(&b"isom\0\0\0\0isom"[..]);
        ftyp.extend_from_slice(b"mp42");
        let mut mvhd = vec![0_u8; 20];
        mvhd[12..16].copy_from_slice(&1_000_u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&2_500_u32.to_be_bytes());
        let mut moov = mp4_box(*b"mvhd", &mvhd);
        moov.extend_from_slice(&test_track(*b"vide", 1_920, 1_080, false, &[*b"avc1"]));
        moov.extend_from_slice(&test_track(*b"soun", 0, 0, false, &[*b"mp4a"]));
        let mut output = mp4_box(*b"ftyp", &ftyp);
        output.extend_from_slice(&mp4_box(*b"moov", &moov));
        output
    }

    fn cross_runtime_v1_mp4() -> Vec<u8> {
        let mut mvhd = vec![0_u8; 20];
        mvhd[12..16].copy_from_slice(&1_000_u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&2_500_u32.to_be_bytes());
        let mut moov = mp4_box(*b"mvhd", &mvhd);
        moov.extend_from_slice(&test_track(*b"vide", 1_920, 1_080, true, &[*b"avc3"]));
        moov.extend_from_slice(&test_track(*b"soun", 0, 0, true, &[*b"mp4a"]));
        let mut output = mp4_box(*b"ftyp", b"isom");
        output.extend_from_slice(&mp4_box(*b"moov", &moov));
        output
    }

    fn test_mp4_with_entries(video: &[[u8; 4]], audio: &[[u8; 4]]) -> Vec<u8> {
        let mut mvhd = vec![0_u8; 20];
        mvhd[12..16].copy_from_slice(&1_000_u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&2_500_u32.to_be_bytes());
        let mut moov = mp4_box(*b"mvhd", &mvhd);
        moov.extend_from_slice(&test_track(*b"vide", 1_920, 1_080, false, video));
        moov.extend_from_slice(&test_track(*b"soun", 0, 0, false, audio));
        let mut output = mp4_box(*b"ftyp", b"isom");
        output.extend_from_slice(&mp4_box(*b"moov", &moov));
        output
    }

    #[test]
    fn checkpoint_digest_has_a_cross_runtime_golden_vector() {
        let request = Ymm4CheckpointRequest::try_new(Ymm4CheckpointRequestInput {
            operation_id: Uuid::nil(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            source_revision: 7,
            target_identity_digest: "target-a".into(),
            expected_state_digest: "state-a".into(),
            checkpoint_profile_digest: "profile-a".into(),
        })
        .unwrap();
        assert_eq!(
            request.request_digest,
            "7c4bc2d3d568eeee9ed9e018bc26158f9b8ffc7606373144b6a382771b0c32a5"
        );
    }

    #[test]
    fn checkpoint_requires_both_pre_and_post_file_evidence() {
        let project_path =
            std::env::temp_dir().join(format!("takegraph-checkpoint-{}.ymmp", Uuid::new_v4()));
        std::fs::write(&project_path, b"saved project").unwrap();
        let (post_hash, post_bytes) = sha256_file(&project_path).unwrap();
        let request = Ymm4CheckpointRequest::try_new(Ymm4CheckpointRequestInput {
            operation_id: Uuid::new_v4(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            source_revision: 7,
            target_identity_digest: "target-a".into(),
            expected_state_digest: "state-a".into(),
            checkpoint_profile_digest: "profile-a".into(),
        })
        .unwrap();
        let profile = Ymm4CheckpointProfile {
            profile_digest: "profile-a".into(),
            driver_profile_digest: "driver-a".into(),
            existing_path_only: true,
        };
        let receipt = Ymm4CheckpointReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest.clone(),
            project_id: request.project_id.clone(),
            scene_id: request.scene_id.clone(),
            source_revision: request.source_revision,
            target_identity_digest: request.target_identity_digest.clone(),
            expected_state_digest: request.expected_state_digest.clone(),
            checkpoint_profile_digest: request.checkpoint_profile_digest.clone(),
            status: Ymm4CheckpointStatus::Verified,
            project_path: project_path.to_str().unwrap().into(),
            pre_file_sha256: None,
            post_file_sha256: Some(post_hash),
            post_file_bytes: Some(post_bytes),
            before_state_digest: request.expected_state_digest.clone(),
            after_state_digest: request.expected_state_digest.clone(),
            driver_profile_digest: profile.driver_profile_digest.clone(),
            error: None,
        };

        assert!(matches!(
            verify_checkpoint(&request, &profile, &receipt),
            Err(ProjectOperationNodeError::MissingCheckpointHash)
        ));
        std::fs::remove_file(project_path).unwrap();
    }

    #[test]
    fn render_digest_has_a_cross_runtime_golden_vector() {
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\render\final.mp4")
        } else {
            PathBuf::from("/render/final.mp4")
        };
        let request = Ymm4RenderRequest::try_new(Ymm4RenderRequestInput {
            task_id: Uuid::nil(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            source_revision: 7,
            target_identity_digest: "target-a".into(),
            expected_state_digest: "state-a".into(),
            checkpoint_operation_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111")
                .unwrap(),
            checkpoint_request_digest: "checkpoint-request-a".into(),
            checkpoint_project_path: if cfg!(windows) {
                r"C:\project\source.ymmp".into()
            } else {
                "/project/source.ymmp".into()
            },
            checkpoint_file_sha256: "a".repeat(64),
            checkpoint_file_bytes: 123,
            render_profile_digest: "profile-a".into(),
            output_path: path,
            overwrite_policy: RenderOverwritePolicy::Deny,
        })
        .unwrap();
        let expected = if cfg!(windows) {
            "1f1bc2a281f3f96bd0e6dc63ba53d1a837b6b2794c7181529fbb4ca5642c53c4"
        } else {
            "365b3ac76f42864a9adcbfbc59568490240a588c477c8189aa775d5d0d2e8f80"
        };
        assert_eq!(request.request_digest, expected);
    }

    #[test]
    fn render_requires_absolute_path_and_explicit_overwrite() {
        let result = Ymm4RenderRequest::try_new(Ymm4RenderRequestInput {
            task_id: Uuid::nil(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            source_revision: 7,
            target_identity_digest: "target-a".into(),
            expected_state_digest: "state-a".into(),
            checkpoint_operation_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111")
                .unwrap(),
            checkpoint_request_digest: "checkpoint-request-a".into(),
            checkpoint_project_path: if cfg!(windows) {
                r"C:\project\source.ymmp".into()
            } else {
                "/project/source.ymmp".into()
            },
            checkpoint_file_sha256: "a".repeat(64),
            checkpoint_file_bytes: 123,
            render_profile_digest: "profile-a".into(),
            output_path: PathBuf::from("relative.mp4"),
            overwrite_policy: RenderOverwritePolicy::Deny,
        });
        assert!(matches!(
            result,
            Err(ProjectOperationNodeError::OutputMustBeAbsolute(_))
        ));
    }

    #[test]
    fn recovery_required_is_a_terminal_bridge_render_state() {
        assert!(Ymm4RenderStatus::RecoveryRequired.is_terminal());
    }

    #[test]
    fn independent_mp4_probe_hashes_and_reads_authoritative_metadata() {
        let path = std::env::temp_dir().join(format!("takegraph-probe-{}.mp4", Uuid::new_v4()));
        std::fs::write(&path, test_mp4()).unwrap();

        let receipt = probe_mp4_output(&path).unwrap();

        assert_eq!(receipt.output_path, path.to_str().unwrap());
        assert_eq!(receipt.byte_length, std::fs::metadata(&path).unwrap().len());
        assert_eq!(receipt.container, MediaContainer::Mp4);
        assert_eq!(receipt.duration_millis, 2_500);
        assert_eq!((receipt.width, receipt.height), (1_920, 1_080));
        assert_eq!((receipt.video_streams, receipt.audio_streams), (1, 1));
        assert_eq!(receipt.probe_profile, FINAL_MEDIA_PROBE_PROFILE);
        assert_eq!(
            receipt.probe_digest,
            media_probe_digest(
                &receipt.sha256,
                receipt.byte_length,
                receipt.container,
                receipt.duration_millis,
                receipt.width,
                receipt.height,
                receipt.video_streams,
                receipt.audio_streams,
                receipt.fps_numerator,
                receipt.fps_denominator,
                &receipt.video_codec,
                receipt.audio_codec.as_deref(),
                receipt.audio_sample_rate,
                &receipt.pixel_format,
                &receipt.probe_profile,
            )
        );

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn version_one_tkhd_has_a_cross_runtime_probe_golden() {
        let path = std::env::temp_dir().join(format!("takegraph-probe-v1-{}.mp4", Uuid::new_v4()));
        std::fs::write(&path, cross_runtime_v1_mp4()).unwrap();

        let receipt = probe_mp4_output(&path).unwrap();

        assert_eq!((receipt.width, receipt.height), (1_920, 1_080));
        assert_eq!(receipt.duration_millis, 2_500);
        assert_eq!((receipt.video_streams, receipt.audio_streams), (1, 1));
        assert_eq!(
            receipt.sha256,
            "086499a4a586bd4c16007bef212f546bcbf1637156e42cad213db6bec4540db5"
        );
        assert_eq!(
            receipt.probe_digest,
            "daf321f48e76f20b97da73a8f176d657afc08380f81ca80b94c250c0cb6eb0d4"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn independent_mp4_probe_rejects_ambiguous_or_unsupported_codec_evidence() {
        type CodecCase<'a> = (&'a str, &'a [[u8; 4]], &'a [[u8; 4]]);
        let cases: &[CodecCase<'_>] = &[
            ("hevc", &[*b"hev1"], &[*b"mp4a"]),
            ("opus", &[*b"avc1"], &[*b"Opus"]),
            ("missing-stsd-entry", &[], &[*b"mp4a"]),
            ("mixed-video-codecs", &[*b"avc1", *b"hev1"], &[*b"mp4a"]),
            ("aac-he", &[*b"avc1"], &[*b"aach"]),
        ];
        for (name, video, audio) in cases {
            let path = std::env::temp_dir().join(format!(
                "takegraph-probe-invalid-{name}-{}.mp4",
                Uuid::new_v4()
            ));
            std::fs::write(&path, test_mp4_with_entries(video, audio)).unwrap();
            assert!(probe_mp4_output(&path).is_err(), "accepted {name}");
            std::fs::remove_file(path).unwrap();
        }

        let path = std::env::temp_dir().join(format!(
            "takegraph-probe-extra-audio-{}.mp4",
            Uuid::new_v4()
        ));
        let mut bytes = test_mp4();
        let extra_audio = test_track(*b"soun", 0, 0, false, &[*b"mp4a"]);
        let moov_offset = bytes
            .windows(4)
            .position(|window| window == b"moov")
            .expect("moov")
            - 4;
        bytes.extend_from_slice(&extra_audio);
        let moov_size = u32::from_be_bytes(bytes[moov_offset..moov_offset + 4].try_into().unwrap());
        bytes[moov_offset..moov_offset + 4].copy_from_slice(
            &(moov_size + u32::try_from(extra_audio.len()).unwrap()).to_be_bytes(),
        );
        std::fs::write(&path, bytes).unwrap();
        assert!(probe_mp4_output(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn external_mp4_evidence_is_probed_when_requested() {
        let Ok(path) = std::env::var("TAKEGRAPH_TEST_MP4_PATH") else {
            return;
        };
        let receipt = probe_mp4_output(Path::new(&path)).unwrap();
        assert_eq!(receipt.video_codec, "h264");
        assert_eq!(receipt.audio_codec.as_deref(), Some("aac_lc"));
        assert_eq!(receipt.pixel_format, "yuv420p");
        assert_eq!((receipt.fps_numerator, receipt.fps_denominator), (60, 1));
    }
}
