use std::collections::BTreeMap;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use takegraph_core::{
    TargetPlan, TargetPlanError, TimelineEditError, TimelineEditPlan, canonical_sha256,
};
use thiserror::Error;
use url::{Host, Url};
use uuid::Uuid;

pub const YMM4_BRIDGE_PROTOCOL_VERSION: u32 = 2;
pub const YMM4_TOKEN_HEADER: &str = "x-takegraph-token";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4Health {
    pub status: String,
    pub protocol_version: u32,
    pub plugin_version: String,
    pub ymm4_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4Capabilities {
    pub protocol_version: u32,
    pub capabilities: Vec<Ymm4Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ProjectControl {
    pub scope: String,
    pub name: String,
    pub can_execute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ProjectControls {
    pub commands: Vec<Ymm4ProjectControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ProjectControlResult {
    pub action: String,
    pub scope: String,
    pub command: String,
    pub success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4Capability {
    ManagedAudio,
    ManagedCaption,
    UnifiedTargetPlan,
    TimelineEditManagedCueMixed,
    MetadataRemarkDetach,
    ReadbackVerification,
    IdempotentApply,
    UndoBatch,
    RequestBoundReceipts,
    WriteAheadApply,
    RecoveryReadback,
    NativeVoiceCreate,
    NativeVoiceUpdateReplacePreservingUserState,
    NativeVoiceDelete,
    NativeVoiceExactWavExport,
    NativeVoiceHostBoundProvenance,
    NativeVoiceRemarkIdentity,
    NativeVoiceBoundedDuration,
    MutationProfileYmm4_4_55_1_1,
    SceneCaptureNativePng,
    SceneCapturePlayheadRestore,
    SceneCaptureContentHash,
    SceneCompositionCurrent,
    NativePortraitUpsert,
    NativeFaceUpsert,
    NativeImageUpsert,
    NativeVideoUpsert,
    NativeAudioUpsert,
    NativeEffectTypedMutation,
    NativeTemplateInstantiate,
    ProjectCheckpointVerified,
    ProjectInitializeSaveAsVerified,
    ProjectRender,
    ProjectRenderCancel,
    ProjectRenderMediaReceipt,
    EditSurfaceAdmit,
    CompositionGraphApply,
    ProjectSettingsMutation,
    ProjectSceneMutation,
    ProjectTimelineMutation,
    ProjectCharacterMutation,
    ProjectTemplateDefinitionEdit,
    EditTransactionApply,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ManagedItem {
    pub entity_id: String,
    pub revision: u64,
    pub kind: ManagedItemKind,
    pub frame: i32,
    pub layer: i32,
    pub length: i32,
    pub text: Option<String>,
    #[serde(default)]
    pub spoken_text: Option<String>,
    pub audio_path: Option<String>,
    pub artifact_hash: Option<String>,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub realization_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedItemKind {
    Audio,
    Caption,
    Voice,
    Annotation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ProjectSnapshot {
    pub project_id: String,
    pub project_name: String,
    pub project_path: String,
    pub scene_id: String,
    pub fps: u32,
    pub fingerprint: String,
    pub managed_items: Vec<Ymm4ManagedItem>,
    /// Fresh bridge read-back of only the fields owned by native-extension
    /// operations. Preserved fields and unknown effects are deliberately not
    /// represented here.
    #[serde(default)]
    pub native_extensions: Vec<Ymm4ManagedNativeExtension>,
    pub unmanaged_context_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4ProjectInitializationPrepareRequest {
    pub protocol_version: u32,
    pub destination_path: String,
}

impl Ymm4ProjectInitializationPrepareRequest {
    #[must_use]
    pub fn new(destination_path: impl Into<String>) -> Self {
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            destination_path: destination_path.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4ProjectInitializationPreparation {
    pub protocol_version: u32,
    pub driver_profile_digest: String,
    pub source_project_instance_id: String,
    pub source: Ymm4ProjectSnapshot,
    pub destination_path: String,
    pub destination_path_digest: String,
    pub predicted_project_id: String,
    pub predicted_fingerprint: String,
    pub overwrite: bool,
}

impl Ymm4ProjectInitializationPreparation {
    /// Rejects a preparation that is not the bridge's new-path-only profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the protocol, source binding, destination, or
    /// digest fields do not satisfy the initialization contract.
    pub fn validate(&self) -> Result<(), Ymm4Error> {
        if self.protocol_version != YMM4_BRIDGE_PROTOCOL_VERSION {
            return Err(Ymm4Error::ProtocolMismatch {
                expected: YMM4_BRIDGE_PROTOCOL_VERSION,
                actual: self.protocol_version,
            });
        }
        if self.overwrite
            || !self.source.project_path.trim().is_empty()
            || !self.source.managed_items.is_empty()
            || !self.source.native_extensions.is_empty()
            || self.source_project_instance_id.trim().is_empty()
            || self.driver_profile_digest.trim().is_empty()
            || self.source.project_id.trim().is_empty()
            || self.source.scene_id.trim().is_empty()
            || self.source.fingerprint.trim().is_empty()
            || self.destination_path_digest.trim().is_empty()
            || self.predicted_project_id.trim().is_empty()
            || self.predicted_fingerprint.trim().is_empty()
            || !has_ymmp_extension(&self.destination_path)
            || !is_sha256_digest(&self.driver_profile_digest)
            || !is_sha256_digest(&self.source.fingerprint)
            || !is_sha256_digest(&self.destination_path_digest)
            || !is_sha256_digest(&self.predicted_fingerprint)
        {
            return Err(Ymm4Error::InvalidProjectInitialization(
                "bridge returned an invalid new-project preparation".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4ProjectInstanceBinding {
    pub protocol_version: u32,
    pub driver_profile_digest: String,
    pub source_project_instance_id: String,
    pub source: Ymm4ProjectSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4ProjectInitializationRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub driver_profile_digest: String,
    pub source_project_instance_id: String,
    pub source_project_id: String,
    pub source_scene_id: String,
    pub expected_source_fingerprint: String,
    pub destination_path: String,
    pub destination_path_digest: String,
    pub predicted_project_id: String,
    pub predicted_fingerprint: String,
    pub overwrite: bool,
}

impl Ymm4ProjectInitializationRequest {
    /// Binds an exact preparation to a durable operation ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the preparation is invalid or the operation ID
    /// is nil.
    pub fn new(
        operation_id: Uuid,
        preparation: &Ymm4ProjectInitializationPreparation,
    ) -> Result<Self, Ymm4Error> {
        preparation.validate()?;
        if operation_id.is_nil() {
            return Err(Ymm4Error::InvalidProjectInitialization(
                "project initialization operation ID must not be nil".into(),
            ));
        }
        let mut request = Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            request_digest: String::new(),
            driver_profile_digest: preparation.driver_profile_digest.clone(),
            source_project_instance_id: preparation.source_project_instance_id.clone(),
            source_project_id: preparation.source.project_id.clone(),
            source_scene_id: preparation.source.scene_id.clone(),
            expected_source_fingerprint: preparation.source.fingerprint.clone(),
            destination_path: preparation.destination_path.clone(),
            destination_path_digest: preparation.destination_path_digest.clone(),
            predicted_project_id: preparation.predicted_project_id.clone(),
            predicted_fingerprint: preparation.predicted_fingerprint.clone(),
            overwrite: false,
        };
        request.request_digest = project_initialization_request_digest(&request);
        Ok(request)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4ProjectInitializationStatus {
    Applying,
    Verified,
    Replayed,
    RecoveryRequired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4ProjectInitializationReceipt {
    pub operation_id: Uuid,
    pub request_digest: String,
    pub status: Ymm4ProjectInitializationStatus,
    pub driver_profile_digest: String,
    pub source_project_instance_id: String,
    pub source_project_id: String,
    pub source_scene_id: String,
    pub before_fingerprint: String,
    pub destination_path: String,
    pub destination_path_digest: String,
    pub predicted_project_id: String,
    pub predicted_fingerprint: String,
    #[serde(default)]
    pub prepared_temporary_path: Option<String>,
    #[serde(default)]
    pub prepared_file_sha256: Option<String>,
    #[serde(default)]
    pub prepared_file_bytes: Option<u64>,
    pub after_snapshot: Option<Ymm4ProjectSnapshot>,
    pub file_sha256: Option<String>,
    pub file_bytes: Option<u64>,
    pub error: Option<String>,
}

fn project_initialization_request_digest(request: &Ymm4ProjectInitializationRequest) -> String {
    fn write_string(canonical: &mut String, label: &str, value: &str) {
        use std::fmt::Write as _;
        let _ = writeln!(canonical, "{label}:{}:{value}", value.len());
    }
    use std::fmt::Write as _;
    let mut canonical = String::from("takegraph-ymm4-project-initialization-v2\n");
    let _ = writeln!(canonical, "protocolVersion:{}", request.protocol_version);
    write_string(
        &mut canonical,
        "operationId",
        &request.operation_id.hyphenated().to_string(),
    );
    write_string(
        &mut canonical,
        "driverProfileDigest",
        &request.driver_profile_digest,
    );
    write_string(
        &mut canonical,
        "sourceProjectInstanceId",
        &request.source_project_instance_id,
    );
    write_string(
        &mut canonical,
        "sourceProjectId",
        &request.source_project_id,
    );
    write_string(&mut canonical, "sourceSceneId", &request.source_scene_id);
    write_string(
        &mut canonical,
        "expectedSourceFingerprint",
        &request.expected_source_fingerprint,
    );
    write_string(&mut canonical, "destinationPath", &request.destination_path);
    write_string(
        &mut canonical,
        "destinationPathDigest",
        &request.destination_path_digest,
    );
    write_string(
        &mut canonical,
        "predictedProjectId",
        &request.predicted_project_id,
    );
    write_string(
        &mut canonical,
        "predictedFingerprint",
        &request.predicted_fingerprint,
    );
    let _ = writeln!(canonical, "overwrite:{}", u8::from(request.overwrite));
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn has_ymmp_extension(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ymmp"))
        && std::path::Path::new(path).is_absolute()
}

fn is_sha256_digest(value: &str) -> bool {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparedProjectInitializationEvidence {
    None,
    PathOnly,
    Complete,
}

fn is_owned_project_initialization_temporary_path(
    destination_path: &str,
    temporary_path: &str,
) -> bool {
    let destination = std::path::Path::new(destination_path);
    let temporary = std::path::Path::new(temporary_path);
    if temporary_path.trim() != temporary_path
        || !destination.is_absolute()
        || !temporary.is_absolute()
        || destination == temporary
        || destination.parent() != temporary.parent()
        || !has_ymmp_extension(temporary_path)
    {
        return false;
    }

    temporary
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|stem| stem.strip_prefix(".takegraph-project-"))
        .is_some_and(|nonce| {
            nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn validate_prepared_project_initialization_evidence(
    receipt: &Ymm4ProjectInitializationReceipt,
) -> Result<PreparedProjectInitializationEvidence, Ymm4Error> {
    let evidence = match (
        receipt.prepared_temporary_path.as_deref(),
        receipt.prepared_file_sha256.as_deref(),
        receipt.prepared_file_bytes,
    ) {
        (None, None, None) => PreparedProjectInitializationEvidence::None,
        (Some(path), None, None)
            if is_owned_project_initialization_temporary_path(&receipt.destination_path, path) =>
        {
            PreparedProjectInitializationEvidence::PathOnly
        }
        (Some(path), Some(sha256), Some(bytes))
            if is_owned_project_initialization_temporary_path(&receipt.destination_path, path)
                && is_sha256_digest(sha256)
                && bytes > 0 =>
        {
            PreparedProjectInitializationEvidence::Complete
        }
        _ => {
            return Err(Ymm4Error::InvalidProjectInitialization(
                "receipt has malformed prepared-file evidence".into(),
            ));
        }
    };
    Ok(evidence)
}

fn validate_project_initialization_receipt(
    request: &Ymm4ProjectInitializationRequest,
    receipt: &Ymm4ProjectInitializationReceipt,
) -> Result<(), Ymm4Error> {
    if receipt.operation_id != request.operation_id
        || receipt.request_digest != request.request_digest
        || receipt.driver_profile_digest != request.driver_profile_digest
        || receipt.source_project_instance_id != request.source_project_instance_id
        || receipt.source_project_id != request.source_project_id
        || receipt.source_scene_id != request.source_scene_id
        || receipt.before_fingerprint != request.expected_source_fingerprint
        || receipt.destination_path != request.destination_path
        || receipt.destination_path_digest != request.destination_path_digest
        || receipt.predicted_project_id != request.predicted_project_id
        || receipt.predicted_fingerprint != request.predicted_fingerprint
    {
        return Err(Ymm4Error::InvalidProjectInitialization(
            "receipt is not bound to the submitted request".into(),
        ));
    }
    let prepared_evidence = validate_prepared_project_initialization_evidence(receipt)?;
    if matches!(
        receipt.status,
        Ymm4ProjectInitializationStatus::Verified | Ymm4ProjectInitializationStatus::Replayed
    ) {
        let after = receipt.after_snapshot.as_ref().ok_or_else(|| {
            Ymm4Error::InvalidProjectInitialization("verified receipt has no after snapshot".into())
        })?;
        let final_sha256 = receipt.file_sha256.as_deref().ok_or_else(|| {
            Ymm4Error::InvalidProjectInitialization(
                "verified receipt has no final file hash".into(),
            )
        })?;
        let final_bytes = receipt.file_bytes.ok_or_else(|| {
            Ymm4Error::InvalidProjectInitialization(
                "verified receipt has no final file byte count".into(),
            )
        })?;
        if after.project_id != request.predicted_project_id
            || after.scene_id != request.source_scene_id
            || after.fingerprint != request.predicted_fingerprint
            || after.project_path != request.destination_path
            || prepared_evidence != PreparedProjectInitializationEvidence::Complete
            || !is_sha256_digest(final_sha256)
            || final_bytes == 0
            || receipt.prepared_file_sha256.as_deref() != Some(final_sha256)
            || receipt.prepared_file_bytes != Some(final_bytes)
            || receipt.error.is_some()
        {
            return Err(Ymm4Error::InvalidProjectInitialization(
                "verified receipt lacks exact post-save evidence".into(),
            ));
        }
    }
    if receipt.status == Ymm4ProjectInitializationStatus::Failed {
        let after = receipt.after_snapshot.as_ref().ok_or_else(|| {
            Ymm4Error::InvalidProjectInitialization(
                "failed receipt has no authenticated source snapshot".into(),
            )
        })?;
        if prepared_evidence != PreparedProjectInitializationEvidence::None
            || after.project_id != request.source_project_id
            || after.scene_id != request.source_scene_id
            || after.fingerprint != request.expected_source_fingerprint
            || !after.project_path.is_empty()
            || receipt.file_sha256.is_some()
            || receipt.file_bytes.is_some()
            || receipt
                .error
                .as_deref()
                .is_none_or(|error| error.trim().is_empty())
        {
            return Err(Ymm4Error::InvalidProjectInitialization(
                "failed receipt is not an authenticated no-target-write terminal".into(),
            ));
        }
    }
    Ok(())
}

/// Managed-only native-extension observation returned by the general project
/// snapshot. This is intentionally smaller than an apply realization: it has
/// no preservation witnesses, opaque effects, host paths, or state digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4ManagedNativeExtension {
    pub logical_key: String,
    pub realization_id: Uuid,
    pub kind: crate::native_extension::Ymm4ExistingNativeExtensionKind,
    pub project_id: String,
    pub entity_id: String,
    pub entity_revision: u64,
    pub owned_fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedUtterance {
    pub entity_id: String,
    pub revision: u64,
    pub speaker: String,
    pub caption: String,
    /// Text that produced the already materialized audio artifact. Legacy
    /// manifests omitted it and deserialize as `caption`.
    pub spoken_text: String,
    pub audio_path: String,
    pub artifact_hash: String,
    pub frame: i32,
    pub length: i32,
    pub audio_layer: i32,
    pub caption_layer: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedUtteranceWire {
    entity_id: String,
    revision: u64,
    speaker: String,
    caption: String,
    #[serde(default)]
    spoken_text: Option<String>,
    audio_path: String,
    artifact_hash: String,
    frame: i32,
    length: i32,
    audio_layer: i32,
    caption_layer: i32,
}

impl<'de> Deserialize<'de> for ManagedUtterance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ManagedUtteranceWire::deserialize(deserializer)?;
        Ok(Self {
            entity_id: wire.entity_id,
            revision: wire.revision,
            speaker: wire.speaker,
            spoken_text: wire.spoken_text.unwrap_or_else(|| wire.caption.clone()),
            caption: wire.caption,
            audio_path: wire.audio_path,
            artifact_hash: wire.artifact_hash,
            frame: wire.frame,
            length: wire.length,
            audio_layer: wire.audio_layer,
            caption_layer: wire.caption_layer,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4NativeVoiceCue {
    pub realization_id: Uuid,
    pub entity_id: String,
    pub revision: u64,
    pub character_name: String,
    pub display_text: String,
    #[serde(default)]
    pub spoken_text: Option<String>,
    pub frame: i32,
    pub layer: i32,
    pub max_length: i32,
}

/// Digest-bound annotation pin applied as a dedicated non-visual item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4AnnotationMarkerCue {
    pub realization_id: Uuid,
    pub entity_id: String,
    pub annotation_id: Uuid,
    pub project_id: String,
    pub frame: i32,
    pub layer: i32,
    pub length: i32,
    pub label: String,
}

/// One exact, identity-bound mutation of a native YMM4 `VoiceItem`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeVoiceMutation {
    pub realization_id: Uuid,
    pub entity_id: String,
    pub revision: u64,
    pub character_name: String,
    pub display_text: String,
    #[serde(default)]
    pub spoken_text: Option<String>,
    pub frame: i32,
    pub layer: i32,
    pub max_length: i32,
    pub action: Ymm4NativeVoiceMutationAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4NativeVoiceMutationAction {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeVoiceMutationPlanRequest {
    pub protocol_version: u32,
    pub expected_fingerprint: String,
    pub mutations: Vec<Ymm4NativeVoiceMutation>,
}

impl Ymm4NativeVoiceMutationPlanRequest {
    #[must_use]
    pub fn new(
        expected_fingerprint: impl Into<String>,
        mutations: Vec<Ymm4NativeVoiceMutation>,
    ) -> Self {
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            expected_fingerprint: expected_fingerprint.into(),
            mutations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeVoiceMutationPlanResponse {
    pub fingerprint: String,
    pub create_count: usize,
    pub update_count: usize,
    pub delete_count: usize,
    pub duration_resolution: String,
    pub preserved_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4PlanRequest {
    pub protocol_version: u32,
    pub expected_fingerprint: String,
    pub utterances: Vec<ManagedUtterance>,
}

impl Ymm4PlanRequest {
    #[must_use]
    pub fn new(expected_fingerprint: impl Into<String>, utterances: Vec<ManagedUtterance>) -> Self {
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            expected_fingerprint: expected_fingerprint.into(),
            utterances,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4PlanResponse {
    pub fingerprint: String,
    pub operation_count: usize,
    pub create_count: usize,
    pub replace_count: usize,
    pub unchanged_count: usize,
    pub managed_items_after: Vec<Ymm4ManagedItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4NativeVoicePlanRequest {
    pub protocol_version: u32,
    pub expected_fingerprint: String,
    pub cues: Vec<Ymm4NativeVoiceCue>,
}

impl Ymm4NativeVoicePlanRequest {
    #[must_use]
    pub fn new(expected_fingerprint: impl Into<String>, cues: Vec<Ymm4NativeVoiceCue>) -> Self {
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            expected_fingerprint: expected_fingerprint.into(),
            cues,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4NativeVoicePlanResponse {
    pub fingerprint: String,
    pub create_count: usize,
    pub duration_resolution: String,
}

/// Sealed semantic plan accepted directly by the v2 bridge boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TargetPlanRequest {
    pub protocol_version: u32,
    pub target_plan_digest: String,
    pub target_plan: TargetPlan,
}

impl Ymm4TargetPlanRequest {
    /// Builds a bridge request only from a valid canonical target plan.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is invalid or cannot be canonicalized.
    pub fn new(target_plan: TargetPlan) -> Result<Self, TargetPlanError> {
        let target_plan_digest = target_plan.canonical_digest()?;
        Ok(Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            target_plan_digest,
            target_plan,
        })
    }
}

/// Mutation request whose only mutation payload is the approved target plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TargetPlanApplyRequest {
    pub protocol_version: u32,
    pub request_digest: String,
    pub expected_fingerprint: String,
    pub target_plan_digest: String,
    pub target_plan: TargetPlan,
}

impl Ymm4TargetPlanApplyRequest {
    /// Seals a valid target plan into the request-bound apply envelope.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan or canonical request cannot be hashed.
    pub fn new(
        target_plan: TargetPlan,
        expected_fingerprint: impl Into<String>,
    ) -> Result<Self, TargetPlanError> {
        let expected_fingerprint = expected_fingerprint.into();
        if expected_fingerprint.trim().is_empty() {
            return Err(TargetPlanError::EmptyField("expectedFingerprint"));
        }
        let target_plan_digest = target_plan.canonical_digest()?;
        let request_digest = target_plan_request_digest(
            YMM4_BRIDGE_PROTOCOL_VERSION,
            &expected_fingerprint,
            &target_plan_digest,
            &target_plan,
        )?;
        Ok(Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            request_digest,
            expected_fingerprint,
            target_plan_digest,
            target_plan,
        })
    }
}

fn target_plan_request_digest(
    protocol_version: u32,
    expected_fingerprint: &str,
    target_plan_digest: &str,
    target_plan: &TargetPlan,
) -> Result<String, TargetPlanError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload<'a> {
        protocol_version: u32,
        expected_fingerprint: &'a str,
        target_plan_digest: &'a str,
        target_plan: &'a TargetPlan,
    }
    let digest = canonical_sha256(
        "takegraph-ymm4-target-plan-request-v2",
        &Payload {
            protocol_version,
            expected_fingerprint,
            target_plan_digest,
            target_plan,
        },
    )
    .map_err(TargetPlanError::Canonical)?;
    Ok(digest
        .strip_prefix("sha256:")
        .ok_or(TargetPlanError::InvalidDigest("target plan request digest"))?
        .to_owned())
}

/// Bridge proof that a sealed plan can be materialized without re-resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TargetPlanValidation {
    pub operation_id: Uuid,
    pub target_plan_digest: String,
    pub fingerprint: String,
    pub strategy_counts: BTreeMap<String, usize>,
    pub create_count: usize,
    pub update_count: usize,
    pub delete_count: usize,
    pub physical_item_count: usize,
}

/// Strict read-only bridge envelope for a resolved timeline-edit plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TimelineEditValidationRequest {
    pub protocol_version: u32,
    pub expected_fingerprint: String,
    pub plan_digest: String,
    pub timeline_edit_plan: TimelineEditPlan,
    pub artifacts: Vec<crate::Ymm4NativeExtensionArtifact>,
}

impl Ymm4TimelineEditValidationRequest {
    /// Binds a valid portable timeline-edit plan to the observed target fingerprint.
    ///
    /// # Errors
    ///
    /// Returns an error when the fingerprint is empty or the plan fails
    /// validation/canonical digest generation.
    pub fn new(
        timeline_edit_plan: TimelineEditPlan,
        expected_fingerprint: impl Into<String>,
    ) -> Result<Self, TimelineEditError> {
        let expected_fingerprint = expected_fingerprint.into();
        if expected_fingerprint.trim().is_empty() {
            return Err(TimelineEditError::EmptyField("expectedFingerprint"));
        }
        let plan_digest = timeline_edit_plan.canonical_digest()?;
        Ok(Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            expected_fingerprint,
            plan_digest,
            timeline_edit_plan,
            artifacts: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TimelineEditValidation {
    pub operation_id: Uuid,
    pub plan_digest: String,
    pub fingerprint: String,
    pub strategy_counts: BTreeMap<String, usize>,
    pub create_count: usize,
    pub update_count: usize,
    pub delete_count: usize,
    pub physical_item_count: usize,
}

/// Exact request-bound envelope for a single heterogeneous managed-cue write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TimelineEditApplyRequest {
    pub protocol_version: u32,
    pub request_digest: String,
    pub expected_fingerprint: String,
    pub plan_digest: String,
    pub timeline_edit_plan: TimelineEditPlan,
    pub artifacts: Vec<crate::Ymm4NativeExtensionArtifact>,
}

impl Ymm4TimelineEditApplyRequest {
    /// Seals a valid timeline-edit plan and fingerprint into one exact request digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the fingerprint is empty or the plan fails
    /// validation/canonical digest generation.
    pub fn new(
        timeline_edit_plan: TimelineEditPlan,
        expected_fingerprint: impl Into<String>,
    ) -> Result<Self, TimelineEditError> {
        let expected_fingerprint = expected_fingerprint.into();
        if expected_fingerprint.trim().is_empty() {
            return Err(TimelineEditError::EmptyField("expectedFingerprint"));
        }
        let plan_digest = timeline_edit_plan.canonical_digest()?;
        let artifacts = Vec::new();
        let request_digest = timeline_edit_request_digest(
            YMM4_BRIDGE_PROTOCOL_VERSION,
            &expected_fingerprint,
            &plan_digest,
            &timeline_edit_plan,
            &artifacts,
        )?;
        Ok(Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            request_digest,
            expected_fingerprint,
            plan_digest,
            timeline_edit_plan,
            artifacts,
        })
    }
}

fn timeline_edit_request_digest(
    protocol_version: u32,
    expected_fingerprint: &str,
    plan_digest: &str,
    timeline_edit_plan: &TimelineEditPlan,
    artifacts: &[crate::Ymm4NativeExtensionArtifact],
) -> Result<String, TimelineEditError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload<'a> {
        protocol_version: u32,
        expected_fingerprint: &'a str,
        plan_digest: &'a str,
        timeline_edit_plan: &'a TimelineEditPlan,
        artifacts: &'a [crate::Ymm4NativeExtensionArtifact],
    }
    let digest = canonical_sha256(
        "takegraph-ymm4-timeline-edit-request-v1",
        &Payload {
            protocol_version,
            expected_fingerprint,
            plan_digest,
            timeline_edit_plan,
            artifacts,
        },
    )?;
    Ok(digest
        .strip_prefix("sha256:")
        .expect("canonical_sha256 always prefixes sha256")
        .to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TimelineEditReceipt {
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub plan_digest: String,
    pub status: Ymm4OperationStatus,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub applied_items: Vec<Ymm4ManagedItem>,
    pub applied_native_extensions: Vec<crate::Ymm4NativeExtensionRealization>,
    pub applied_operation_count: usize,
    pub verified: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TimelineEditApplyResponse {
    pub success: bool,
    pub replayed: bool,
    pub receipt: Ymm4TimelineEditReceipt,
}

fn validate_timeline_edit_response(
    request: &Ymm4TimelineEditApplyRequest,
    response: &Ymm4TimelineEditApplyResponse,
) -> Result<(), Ymm4Error> {
    let receipt = &response.receipt;
    let operation_count = request.timeline_edit_plan.operations.len();
    if receipt.operation_id != request.timeline_edit_plan.operation_id
        || receipt.request_digest != request.request_digest
        || receipt.project_id != request.timeline_edit_plan.target.project_id
        || receipt.scene_id != request.timeline_edit_plan.target.scene_id
        || receipt.expected_fingerprint != request.expected_fingerprint
        || receipt.plan_digest != request.plan_digest
        || receipt.before_fingerprint != request.expected_fingerprint
        || !is_sha256_digest(&receipt.request_digest)
        || !is_sha256_digest(&receipt.expected_fingerprint)
        || !is_sha256_digest(&receipt.plan_digest)
        || !is_sha256_digest(&receipt.before_fingerprint)
        || !is_sha256_digest(&receipt.after_fingerprint)
        || response.success != receipt.verified
        || receipt.verified != (receipt.status == Ymm4OperationStatus::Verified)
        || (receipt.verified && receipt.error.is_some())
        || (receipt.verified && receipt.applied_operation_count != operation_count)
        || (!receipt.verified && receipt.applied_operation_count != 0)
        || !receipt.applied_native_extensions.is_empty()
    {
        return Err(Ymm4Error::InvalidTimelineEditReceipt(
            "timeline-edit receipt is not an exact proof for the submitted request".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ApplyRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub utterances: Vec<ManagedUtterance>,
}

impl Ymm4ApplyRequest {
    #[must_use]
    pub fn new(
        operation_id: Uuid,
        project_id: impl Into<String>,
        scene_id: impl Into<String>,
        expected_fingerprint: impl Into<String>,
        utterances: Vec<ManagedUtterance>,
    ) -> Self {
        let project_id = project_id.into();
        let scene_id = scene_id.into();
        let expected_fingerprint = expected_fingerprint.into();
        let request_digest = apply_request_digest(
            YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            &project_id,
            &scene_id,
            &expected_fingerprint,
            &utterances,
        );
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            request_digest,
            project_id,
            scene_id,
            expected_fingerprint,
            utterances,
        }
    }
}

fn apply_request_digest(
    protocol_version: u32,
    operation_id: Uuid,
    project_id: &str,
    scene_id: &str,
    expected_fingerprint: &str,
    utterances: &[ManagedUtterance],
) -> String {
    fn write_string(canonical: &mut String, label: &str, value: &str) {
        use std::fmt::Write as _;
        let _ = writeln!(canonical, "{label}:{}:{value}", value.len());
    }

    use std::fmt::Write as _;
    let mut canonical = String::from("takegraph-ymm4-apply-v2\n");
    let _ = writeln!(canonical, "protocolVersion:{protocol_version}");
    write_string(
        &mut canonical,
        "operationId",
        &operation_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", project_id);
    write_string(&mut canonical, "sceneId", scene_id);
    write_string(&mut canonical, "expectedFingerprint", expected_fingerprint);
    let _ = writeln!(canonical, "utterances:{}", utterances.len());
    for utterance in utterances {
        write_string(&mut canonical, "entityId", &utterance.entity_id);
        let _ = writeln!(canonical, "revision:{}", utterance.revision);
        write_string(&mut canonical, "speaker", &utterance.speaker);
        write_string(&mut canonical, "caption", &utterance.caption);
        if utterance.spoken_text != utterance.caption {
            write_string(&mut canonical, "spokenText", &utterance.spoken_text);
        }
        write_string(&mut canonical, "audioPath", &utterance.audio_path);
        write_string(&mut canonical, "artifactHash", &utterance.artifact_hash);
        let _ = writeln!(canonical, "frame:{}", utterance.frame);
        let _ = writeln!(canonical, "length:{}", utterance.length);
        let _ = writeln!(canonical, "audioLayer:{}", utterance.audio_layer);
        let _ = writeln!(canonical, "captionLayer:{}", utterance.caption_layer);
    }
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4NativeVoiceApplyRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub cues: Vec<Ymm4NativeVoiceCue>,
}

impl Ymm4NativeVoiceApplyRequest {
    #[must_use]
    pub fn new(
        operation_id: Uuid,
        project_id: impl Into<String>,
        scene_id: impl Into<String>,
        expected_fingerprint: impl Into<String>,
        cues: Vec<Ymm4NativeVoiceCue>,
    ) -> Self {
        let project_id = project_id.into();
        let scene_id = scene_id.into();
        let expected_fingerprint = expected_fingerprint.into();
        let request_digest = native_voice_apply_request_digest(
            YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            &project_id,
            &scene_id,
            &expected_fingerprint,
            &cues,
        );
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            request_digest,
            project_id,
            scene_id,
            expected_fingerprint,
            cues,
        }
    }
}

fn native_voice_apply_request_digest(
    protocol_version: u32,
    operation_id: Uuid,
    project_id: &str,
    scene_id: &str,
    expected_fingerprint: &str,
    cues: &[Ymm4NativeVoiceCue],
) -> String {
    fn write_string(canonical: &mut String, label: &str, value: &str) {
        use std::fmt::Write as _;
        let _ = writeln!(canonical, "{label}:{}:{value}", value.len());
    }

    use std::fmt::Write as _;
    let mut canonical = String::from("takegraph-ymm4-native-voice-v2\n");
    let _ = writeln!(canonical, "protocolVersion:{protocol_version}");
    write_string(
        &mut canonical,
        "operationId",
        &operation_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", project_id);
    write_string(&mut canonical, "sceneId", scene_id);
    write_string(&mut canonical, "expectedFingerprint", expected_fingerprint);
    let _ = writeln!(canonical, "cues:{}", cues.len());
    for cue in cues {
        write_string(
            &mut canonical,
            "realizationId",
            &cue.realization_id.hyphenated().to_string(),
        );
        write_string(&mut canonical, "entityId", &cue.entity_id);
        let _ = writeln!(canonical, "revision:{}", cue.revision);
        write_string(&mut canonical, "characterName", &cue.character_name);
        write_string(&mut canonical, "displayText", &cue.display_text);
        if let Some(spoken) = &cue.spoken_text {
            write_string(&mut canonical, "spokenText", spoken);
        }
        let _ = writeln!(canonical, "frame:{}", cue.frame);
        let _ = writeln!(canonical, "layer:{}", cue.layer);
        let _ = writeln!(canonical, "maxLength:{}", cue.max_length);
    }
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeVoiceMutationApplyRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub mutations: Vec<Ymm4NativeVoiceMutation>,
}

impl Ymm4NativeVoiceMutationApplyRequest {
    #[must_use]
    pub fn new(
        operation_id: Uuid,
        project_id: impl Into<String>,
        scene_id: impl Into<String>,
        expected_fingerprint: impl Into<String>,
        mutations: Vec<Ymm4NativeVoiceMutation>,
    ) -> Self {
        let project_id = project_id.into();
        let scene_id = scene_id.into();
        let expected_fingerprint = expected_fingerprint.into();
        let request_digest = native_voice_mutation_apply_request_digest(
            YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            &project_id,
            &scene_id,
            &expected_fingerprint,
            &mutations,
        );
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            request_digest,
            project_id,
            scene_id,
            expected_fingerprint,
            mutations,
        }
    }
}

fn native_voice_mutation_apply_request_digest(
    protocol_version: u32,
    operation_id: Uuid,
    project_id: &str,
    scene_id: &str,
    expected_fingerprint: &str,
    mutations: &[Ymm4NativeVoiceMutation],
) -> String {
    fn write_string(canonical: &mut String, label: &str, value: &str) {
        use std::fmt::Write as _;
        let _ = writeln!(canonical, "{label}:{}:{value}", value.len());
    }

    use std::fmt::Write as _;
    let mut canonical = String::from("takegraph-ymm4-native-voice-mutation-v2\n");
    let _ = writeln!(canonical, "protocolVersion:{protocol_version}");
    write_string(
        &mut canonical,
        "operationId",
        &operation_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", project_id);
    write_string(&mut canonical, "sceneId", scene_id);
    write_string(&mut canonical, "expectedFingerprint", expected_fingerprint);
    let _ = writeln!(canonical, "mutations:{}", mutations.len());
    for mutation in mutations {
        write_string(
            &mut canonical,
            "realizationId",
            &mutation.realization_id.hyphenated().to_string(),
        );
        write_string(&mut canonical, "entityId", &mutation.entity_id);
        let _ = writeln!(canonical, "revision:{}", mutation.revision);
        write_string(&mut canonical, "characterName", &mutation.character_name);
        write_string(&mut canonical, "displayText", &mutation.display_text);
        if let Some(spoken) = &mutation.spoken_text {
            write_string(&mut canonical, "spokenText", spoken);
        }
        let _ = writeln!(canonical, "frame:{}", mutation.frame);
        let _ = writeln!(canonical, "layer:{}", mutation.layer);
        let _ = writeln!(canonical, "maxLength:{}", mutation.max_length);
        write_string(
            &mut canonical,
            "action",
            match mutation.action {
                Ymm4NativeVoiceMutationAction::Create => "create",
                Ymm4NativeVoiceMutationAction::Update => "update",
                Ymm4NativeVoiceMutationAction::Delete => "delete",
            },
        );
    }
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeVoiceArtifactRequest {
    pub protocol_version: u32,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub realization_id: Uuid,
}

impl Ymm4NativeVoiceArtifactRequest {
    #[must_use]
    pub fn new(
        project_id: impl Into<String>,
        scene_id: impl Into<String>,
        expected_fingerprint: impl Into<String>,
        realization_id: Uuid,
    ) -> Self {
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            project_id: project_id.into(),
            scene_id: scene_id.into(),
            expected_fingerprint: expected_fingerprint.into(),
            realization_id,
        }
    }
}

/// Bridge-owned staging paths and hashes for one exact WAV plus normalized,
/// host-bound voice-state provenance document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeVoiceArtifact {
    pub realization_id: Uuid,
    pub audio_path: String,
    pub audio_sha256: String,
    pub audio_bytes: u64,
    pub query_path: String,
    pub query_sha256: String,
    pub query_bytes: u64,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4OperationStatus {
    NotStarted,
    Applying,
    Applied,
    Verified,
    Failed,
    RolledBack,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4OperationReceipt {
    pub operation_id: Uuid,
    #[serde(default)]
    pub request_digest: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub scene_id: String,
    #[serde(default)]
    pub expected_fingerprint: String,
    pub status: Ymm4OperationStatus,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub applied_items: Vec<Ymm4ManagedItem>,
    pub verified: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ApplyResponse {
    pub success: bool,
    pub replayed: bool,
    pub receipt: Ymm4OperationReceipt,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeErrorBody {
    error: Option<String>,
    actual_fingerprint: Option<String>,
}

/// Typed client for the TakeGraph-owned YMM4 bridge.
///
/// The bridge is deliberately not an MCP-to-MCP hop. It is a private,
/// versioned, loopback-only platform adapter used by `takegraph-node`.
#[derive(Debug, Clone)]
pub struct Ymm4BridgeClient {
    endpoint: Url,
    token: String,
    http: Client,
    expected_project_id: Option<String>,
}

impl Ymm4BridgeClient {
    /// Creates a bridge client for a loopback endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid/non-loopback URL or an empty token.
    pub fn new(endpoint: &str, token: impl Into<String>) -> Result<Self, Ymm4Error> {
        let mut endpoint = Url::parse(endpoint)?;
        if !is_loopback(&endpoint) {
            return Err(Ymm4Error::NonLoopbackEndpoint(endpoint));
        }
        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }
        let token = token.into();
        if token.trim().is_empty() {
            return Err(Ymm4Error::MissingToken);
        }

        Ok(Self {
            endpoint,
            token,
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            expected_project_id: None,
        })
    }

    /// Pins every project snapshot read by this client to the project that
    /// selected the canonical store. This closes the gap where the active YMM4
    /// project changes between an MCP `canonical-head` call and a later CLI
    /// command.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty project identity.
    pub fn with_expected_project_id(
        mut self,
        project_id: impl Into<String>,
    ) -> Result<Self, Ymm4Error> {
        let project_id = project_id.into();
        if project_id.trim().is_empty() {
            return Err(Ymm4Error::MissingExpectedProjectId);
        }
        self.expected_project_id = Some(project_id);
        Ok(self)
    }

    /// Acknowledges an explicitly reviewed recovery-required operation.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge response error.
    pub async fn acknowledge_recovery(
        &self,
        operation_id: uuid::Uuid,
    ) -> Result<serde_json::Value, Ymm4Error> {
        self.post_empty(&format!("v2/recovery/{operation_id}/acknowledge"))
            .await
    }

    /// Checks protocol and runtime compatibility.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, response, or protocol mismatch error.
    pub async fn health(&self) -> Result<Ymm4Health, Ymm4Error> {
        let health: Ymm4Health = self.get_json("v1/health").await?;
        if health.protocol_version != YMM4_BRIDGE_PROTOCOL_VERSION {
            return Err(Ymm4Error::ProtocolMismatch {
                expected: YMM4_BRIDGE_PROTOCOL_VERSION,
                actual: health.protocol_version,
            });
        }
        Ok(health)
    }

    /// Reads the bridge capability contract.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, response, or protocol mismatch error.
    pub async fn capabilities(&self) -> Result<Ymm4Capabilities, Ymm4Error> {
        let capabilities: Ymm4Capabilities = self.get_json("v1/capabilities").await?;
        if capabilities.protocol_version != YMM4_BRIDGE_PROTOCOL_VERSION {
            return Err(Ymm4Error::ProtocolMismatch {
                expected: YMM4_BRIDGE_PROTOCOL_VERSION,
                actual: capabilities.protocol_version,
            });
        }
        Ok(capabilities)
    }

    /// Reads health and capabilities and normalizes them into the digest-bound
    /// structured v2 contract used during target planning.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, or capability-normalization error.
    pub async fn structured_capabilities(
        &self,
    ) -> Result<crate::StructuredYmm4Capabilities, Ymm4Error> {
        let health = self.health().await?;
        let capabilities = self.capabilities().await?;
        crate::StructuredYmm4Capabilities::from_bridge(&health, &capabilities)
            .map_err(|error| Ymm4Error::CapabilityContract(error.to_string()))
    }

    /// Captures the active YMM4 project and its concurrency fingerprint.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge response error.
    pub async fn snapshot(&self) -> Result<Ymm4ProjectSnapshot, Ymm4Error> {
        let snapshot: Ymm4ProjectSnapshot = self.get_json("v1/project/snapshot").await?;
        self.require_expected_project_id(&snapshot.project_id)?;
        Ok(snapshot)
    }

    pub(crate) fn require_expected_project_id(&self, actual: &str) -> Result<(), Ymm4Error> {
        if let Some(expected) = &self.expected_project_id
            && actual != expected
        {
            return Err(Ymm4Error::UnexpectedProject {
                expected: expected.clone(),
                actual: actual.into(),
            });
        }
        Ok(())
    }

    /// Lists the fixed save/undo/redo controls exposed by the active YMM4 version.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge response error.
    pub async fn project_controls(&self) -> Result<Ymm4ProjectControls, Ymm4Error> {
        self.get_json("v1/project/controls").await
    }

    /// Saves the already-open YMM4 project using its existing path.
    ///
    /// # Errors
    ///
    /// Returns an error when no save command/path is available or the bridge request fails.
    pub async fn save_project(&self) -> Result<Ymm4ProjectControlResult, Ymm4Error> {
        self.post_empty("v1/project/save").await
    }

    /// Captures a process-local project-instance token for identity-safe
    /// adoption or initialization planning.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, binding-validation, or project mismatch
    /// error.
    pub async fn project_instance_binding(&self) -> Result<Ymm4ProjectInstanceBinding, Ymm4Error> {
        let binding: Ymm4ProjectInstanceBinding =
            self.get_json("v2/project/instance-binding").await?;
        if binding.protocol_version != YMM4_BRIDGE_PROTOCOL_VERSION {
            return Err(Ymm4Error::ProtocolMismatch {
                expected: YMM4_BRIDGE_PROTOCOL_VERSION,
                actual: binding.protocol_version,
            });
        }
        if binding.driver_profile_digest.trim().is_empty()
            || binding.source_project_instance_id.trim().is_empty()
            || binding.source.project_id.trim().is_empty()
            || binding.source.scene_id.trim().is_empty()
            || binding.source.fingerprint.trim().is_empty()
            || !is_sha256_digest(&binding.driver_profile_digest)
            || !is_sha256_digest(&binding.source.fingerprint)
        {
            return Err(Ymm4Error::InvalidProjectInitialization(
                "bridge returned an incomplete project-instance binding".into(),
            ));
        }
        self.require_expected_project_id(&binding.source.project_id)?;
        Ok(binding)
    }

    /// Validates and predicts an untitled project's non-overwriting Save As.
    ///
    /// # Errors
    ///
    /// Returns a transport, preparation-validation, or project mismatch error.
    pub async fn prepare_project_initialization(
        &self,
        destination_path: impl Into<String>,
    ) -> Result<Ymm4ProjectInitializationPreparation, Ymm4Error> {
        let request = Ymm4ProjectInitializationPrepareRequest::new(destination_path);
        let preparation: Ymm4ProjectInitializationPreparation = self
            .post_json("v2/project/initialization/prepare", &request)
            .await?;
        preparation.validate()?;
        self.require_expected_project_id(&preparation.source.project_id)?;
        Ok(preparation)
    }

    /// Performs the exact prepared Save As operation and returns its durable receipt.
    ///
    /// # Errors
    ///
    /// Returns a transport error or rejects a receipt that does not match the
    /// exact prepared request.
    pub async fn initialize_project(
        &self,
        request: &Ymm4ProjectInitializationRequest,
    ) -> Result<Ymm4ProjectInitializationReceipt, Ymm4Error> {
        let receipt: Ymm4ProjectInitializationReceipt = self
            .post_json("v2/project/initialization/apply", request)
            .await?;
        validate_project_initialization_receipt(request, &receipt)?;
        Ok(receipt)
    }

    /// Reads a durable project initialization receipt.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, not-found, or bridge response
    /// error.
    pub async fn project_initialization(
        &self,
        operation_id: Uuid,
    ) -> Result<Ymm4ProjectInitializationReceipt, Ymm4Error> {
        self.get_json(&format!("v2/project/initialization/{operation_id}"))
            .await
    }

    /// Undoes the most recent YMM4 edit batch.
    ///
    /// # Errors
    ///
    /// Returns an error when undo is unavailable or the bridge request fails.
    pub async fn undo(&self) -> Result<Ymm4ProjectControlResult, Ymm4Error> {
        self.post_empty("v1/project/undo").await
    }

    /// Redoes the most recently undone YMM4 edit batch.
    ///
    /// # Errors
    ///
    /// Returns an error when redo is unavailable or the bridge request fails.
    pub async fn redo(&self) -> Result<Ymm4ProjectControlResult, Ymm4Error> {
        self.post_empty("v1/project/redo").await
    }

    /// Requests a normal YMM4 main-window close after the bridge response is sent.
    ///
    /// # Errors
    ///
    /// Returns an error when the main window is unavailable or the bridge request fails.
    pub async fn close_application(&self) -> Result<Ymm4ProjectControlResult, Ymm4Error> {
        self.post_empty("v1/application/close").await
    }

    /// Computes managed-subset impact without mutating YMM4.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn plan(&self, request: &Ymm4PlanRequest) -> Result<Ymm4PlanResponse, Ymm4Error> {
        self.post_json("v1/managed/plan", request).await
    }

    /// Applies a digest-approved managed batch exactly once and verifies it by read-back.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn apply(&self, request: &Ymm4ApplyRequest) -> Result<Ymm4ApplyResponse, Ymm4Error> {
        self.post_json("v1/managed/apply", request).await
    }

    /// Validates a unified, resolved target plan without mutating YMM4.
    ///
    /// # Errors
    ///
    /// Returns a transport, capability, binding, scope, or validation error.
    pub async fn validate_target_plan(
        &self,
        request: &Ymm4TargetPlanRequest,
    ) -> Result<Ymm4TargetPlanValidation, Ymm4Error> {
        self.post_json("v2/target-plan/validate", request).await
    }

    /// Applies the unified target plan exactly once and verifies read-back.
    ///
    /// # Errors
    ///
    /// Returns a transport, authorization, stale-state, or bridge response error.
    pub async fn apply_target_plan(
        &self,
        request: &Ymm4TargetPlanApplyRequest,
    ) -> Result<Ymm4ApplyResponse, Ymm4Error> {
        self.post_json("v2/target-plan/apply", request).await
    }

    /// Seals or replays a durable, request-bound proof under the bridge apply
    /// gate. When no operation WAL exists, a delayed exact apply is thereby
    /// prevented from mutating after canonical releases its reservation.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn seal_target_plan_not_started(
        &self,
        request: &Ymm4TargetPlanApplyRequest,
    ) -> Result<Ymm4ApplyResponse, Ymm4Error> {
        self.post_json("v2/target-plan/not-started", request).await
    }

    /// Validates a timeline-edit plan against the active bridge without mutation.
    ///
    /// # Errors
    ///
    /// Returns a transport, stale-state, bridge-validation, or response-binding error.
    pub async fn validate_timeline_edit(
        &self,
        request: &Ymm4TimelineEditValidationRequest,
    ) -> Result<Ymm4TimelineEditValidation, Ymm4Error> {
        let response: Ymm4TimelineEditValidation =
            self.post_json("v2/timeline-edit/validate", request).await?;
        if response.operation_id != request.timeline_edit_plan.operation_id
            || response.plan_digest != request.plan_digest
            || response.fingerprint != request.expected_fingerprint
        {
            return Err(Ymm4Error::InvalidTimelineEditReceipt(
                "validation response is not bound to the submitted plan".into(),
            ));
        }
        Ok(response)
    }

    /// Applies all supported timeline-edit operations under one bridge writer gate.
    ///
    /// # Errors
    ///
    /// Returns a transport, stale-state, bridge-validation, recovery, or
    /// receipt-binding error.
    pub async fn apply_timeline_edit(
        &self,
        request: &Ymm4TimelineEditApplyRequest,
    ) -> Result<Ymm4TimelineEditApplyResponse, Ymm4Error> {
        let response: Ymm4TimelineEditApplyResponse =
            self.post_json("v2/timeline-edit/apply", request).await?;
        validate_timeline_edit_response(request, &response)?;
        Ok(response)
    }

    /// Seals a durable proof that the exact timeline edit never started.
    ///
    /// # Errors
    ///
    /// Returns a transport, bridge-validation, recovery, or receipt-binding error.
    pub async fn seal_timeline_edit_not_started(
        &self,
        request: &Ymm4TimelineEditApplyRequest,
    ) -> Result<Ymm4TimelineEditApplyResponse, Ymm4Error> {
        let response: Ymm4TimelineEditApplyResponse = self
            .post_json("v2/timeline-edit/not-started", request)
            .await?;
        validate_timeline_edit_response(request, &response)?;
        Ok(response)
    }

    /// Plans native YMM4 `VoiceItem` creation without mutating the project.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn plan_native_voice(
        &self,
        request: &Ymm4NativeVoicePlanRequest,
    ) -> Result<Ymm4NativeVoicePlanResponse, Ymm4Error> {
        self.post_json("v2/native-voice/plan", request).await
    }

    /// Applies a digest-bound native YMM4 `VoiceItem` batch exactly once.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn apply_native_voice(
        &self,
        request: &Ymm4NativeVoiceApplyRequest,
    ) -> Result<Ymm4ApplyResponse, Ymm4Error> {
        self.post_json("v2/native-voice/apply", request).await
    }

    /// Plans identity-bound native voice create/update/delete operations.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, validation, or bridge error.
    pub async fn plan_native_voice_mutations(
        &self,
        request: &Ymm4NativeVoiceMutationPlanRequest,
    ) -> Result<Ymm4NativeVoiceMutationPlanResponse, Ymm4Error> {
        self.post_json("v2/native-voice/mutation/plan", request)
            .await
    }

    /// Applies an approved native voice create/update/delete batch exactly once.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, validation, or bridge error.
    pub async fn apply_native_voice_mutations(
        &self,
        request: &Ymm4NativeVoiceMutationApplyRequest,
    ) -> Result<Ymm4ApplyResponse, Ymm4Error> {
        self.post_json("v2/native-voice/mutation/apply", request)
            .await
    }

    /// Seals or replays the exact native-voice mutation operation under the
    /// same bridge apply gate as the mutating route.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn seal_native_voice_mutation_not_started(
        &self,
        request: &Ymm4NativeVoiceMutationApplyRequest,
    ) -> Result<Ymm4ApplyResponse, Ymm4Error> {
        self.post_json("v2/native-voice/mutation/not-started", request)
            .await
    }

    /// Exports one bridge-staged exact WAV plus normalized host-bound provenance.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, validation, or bridge error.
    pub async fn export_native_voice_artifact(
        &self,
        request: &Ymm4NativeVoiceArtifactRequest,
    ) -> Result<Ymm4NativeVoiceArtifact, Ymm4Error> {
        self.post_json("v2/native-voice/artifacts", request).await
    }

    /// Captures staged scene frames through the typed YMM4 inspection route.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn capture_scene(
        &self,
        request: &crate::scene_inspection::Ymm4SceneCaptureRequest,
    ) -> Result<crate::scene_inspection::Ymm4SceneCaptureReceipt, Ymm4Error> {
        self.post_json("v2/scene/capture", request).await
    }

    /// Reads a persisted in-process operation receipt. A 404 is only an
    /// observation that no record was visible to this GET; safe reservation
    /// release requires an apply-gate-serialized `not_started` seal.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge response error.
    pub async fn operation(&self, operation_id: Uuid) -> Result<Ymm4OperationReceipt, Ymm4Error> {
        self.get_json(&format!("v1/operations/{operation_id}"))
            .await
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, Ymm4Error> {
        let url = self.endpoint.join(path)?;
        let response = self
            .http
            .get(url)
            .header(YMM4_TOKEN_HEADER, &self.token)
            .send()
            .await?;
        decode_response(response).await
    }

    pub(crate) async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, Ymm4Error> {
        let url = self.endpoint.join(path)?;
        let response = self
            .http
            .post(url)
            .header(YMM4_TOKEN_HEADER, &self.token)
            .json(body)
            .send()
            .await?;
        decode_response(response).await
    }

    async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, Ymm4Error> {
        let url = self.endpoint.join(path)?;
        let response = self
            .http
            .post(url)
            .header(YMM4_TOKEN_HEADER, &self.token)
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .body(Vec::new())
            .send()
            .await?;
        decode_response(response).await
    }
}

async fn decode_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, Ymm4Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response.json().await?);
    }

    let body = response
        .json::<BridgeErrorBody>()
        .await
        .unwrap_or(BridgeErrorBody {
            error: None,
            actual_fingerprint: None,
        });
    if status == StatusCode::CONFLICT {
        return Err(Ymm4Error::StaleExternalState {
            actual_fingerprint: body.actual_fingerprint.unwrap_or_default(),
        });
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err(Ymm4Error::Unauthorized);
    }
    Err(Ymm4Error::Bridge {
        status,
        message: body
            .error
            .unwrap_or_else(|| "YMM4 bridge request failed".into()),
    })
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[derive(Debug, Error)]
pub enum Ymm4Error {
    #[error("invalid YMM4 bridge endpoint: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("YMM4 bridge endpoint must be loopback-only: {0}")]
    NonLoopbackEndpoint(Url),
    #[error("YMM4 bridge token must not be empty")]
    MissingToken,
    #[error("expected YMM4 project ID must not be empty")]
    MissingExpectedProjectId,
    #[error("invalid YMM4 project initialization contract: {0}")]
    InvalidProjectInitialization(String),
    #[error("active YMM4 project changed: expected {expected}, got {actual}")]
    UnexpectedProject { expected: String, actual: String },
    #[error("YMM4 bridge request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("YMM4 bridge rejected the token")]
    Unauthorized,
    #[error("YMM4 bridge protocol mismatch: expected {expected}, got {actual}")]
    ProtocolMismatch { expected: u32, actual: u32 },
    #[error("invalid YMM4 capability contract: {0}")]
    CapabilityContract(String),
    #[error("invalid YMM4 timeline-edit receipt: {0}")]
    InvalidTimelineEditReceipt(String),
    #[error(transparent)]
    SceneComposition(#[from] crate::scene_composition::Ymm4SceneCompositionError),
    #[error("YMM4 changed after preview; actual fingerprint is {actual_fingerprint}")]
    StaleExternalState { actual_fingerprint: String },
    #[error("YMM4 bridge returned {status}: {message}")]
    Bridge { status: StatusCode, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::TargetPlan;

    fn native_voice_cue() -> Ymm4NativeVoiceCue {
        Ymm4NativeVoiceCue {
            realization_id: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
            entity_id: "utt-01".into(),
            revision: 4,
            character_name: "春日部つむぎ".into(),
            display_text: "ここから第二形態です".into(),
            spoken_text: Some("ここから第二形態です".into()),
            frame: 120,
            layer: 20,
            max_length: 180,
        }
    }

    fn native_voice_mutation(action: Ymm4NativeVoiceMutationAction) -> Ymm4NativeVoiceMutation {
        Ymm4NativeVoiceMutation {
            realization_id: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
            entity_id: "utt-01".into(),
            revision: 4,
            character_name: "春日部つむぎ".into(),
            display_text: "ここから第二形態です".into(),
            spoken_text: Some("ここから第二形態です".into()),
            frame: 120,
            layer: 20,
            max_length: 180,
            action,
        }
    }

    fn timeline_edit_plan_for_node_test(entity: &str, realization: u128) -> TimelineEditPlan {
        use takegraph_core::{
            BindingDependency, CapabilityDependency, ChangeBudget, DurationResolution,
            FallbackPolicy, ManagedCueIntent, OrderingPolicy, OwnershipMask, PlacementIntent,
            PlannedAction, PlannedCue, RealizationPreference, RealizationStrategy,
            ResolvedPlacement, ResolvedRealization, RevisionId, ScopeFingerprints, TargetIdentity,
            TimelineEditOperation, TimingAnchor,
        };
        let digest = |value: char| format!("sha256:{}", value.to_string().repeat(64));
        let placement = PlacementIntent {
            anchor: TimingAnchor::AbsoluteFrame { frame: 12 },
            ordering: OrderingPolicy::Fixed,
            track_role: "dialogue".into(),
        };
        let mut intent =
            ManagedCueIntent::new(entity, 1, "caption", "spoken", "speaker", placement);
        intent.voice_profile = Some("speaker".into());
        intent.realization_preference = RealizationPreference::RequirePortable;
        intent.fallback_policy = FallbackPolicy::Reject;
        TimelineEditPlan {
            canonical_version: 1,
            operation_id: Uuid::from_u128(77),
            base_revision: RevisionId(9),
            target: TargetIdentity {
                adapter_id: "ymm4-4.55".into(),
                project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                fps: 60,
                driver_version: "4.55.1.1/0.2.0".into(),
            },
            capability_digest: digest('a'),
            expected_scope: ScopeFingerprints {
                target_identity_digest: digest('b'),
                managed_state_digest: digest('c'),
                conflict_scope_digest: digest('d'),
            },
            change_budget: ChangeBudget::create_only(1),
            operations: vec![TimelineEditOperation::ManagedCue {
                cue: Box::new(PlannedCue {
                    intent,
                    realization_id: Uuid::from_u128(realization),
                    action: PlannedAction::Create,
                    strategy: RealizationStrategy::PortableAudioCaption,
                    fallback: None,
                    placement: ResolvedPlacement {
                        frame: 12,
                        primary_layer: 1,
                        secondary_layer: Some(2),
                    },
                    duration: DurationResolution::Exact { frames: 30 },
                    ownership: OwnershipMask::portable_pair_create(),
                    capability_dependencies: vec![CapabilityDependency {
                        feature: "timelineEdit.apply".into(),
                        minimum_version: 1,
                        schema_digest: Some(digest('e')),
                    }],
                    binding_dependencies: vec![BindingDependency {
                        kind: "audio_artifact".into(),
                        id: "artifact".into(),
                        digest: digest('f'),
                    }],
                    resolved_realization: ResolvedRealization::PortablePair {
                        audio_path: "audio.wav".into(),
                        artifact_digest: digest('f'),
                    },
                }),
            }],
            warnings: vec![],
            source_evidence: vec![],
        }
    }

    #[test]
    fn timeline_edit_request_digest_binds_plan_and_fingerprint() {
        let plan = timeline_edit_plan_for_node_test("one", 1);
        let first =
            Ymm4TimelineEditApplyRequest::new(plan.clone(), format!("sha256:{}", "1".repeat(64)))
                .unwrap();
        let changed_fingerprint =
            Ymm4TimelineEditApplyRequest::new(plan.clone(), format!("sha256:{}", "2".repeat(64)))
                .unwrap();
        let mut changed_plan = plan;
        changed_plan.operations = timeline_edit_plan_for_node_test("two", 2).operations;
        let changed_plan =
            Ymm4TimelineEditApplyRequest::new(changed_plan, format!("sha256:{}", "1".repeat(64)))
                .unwrap();
        assert_ne!(first.request_digest, changed_fingerprint.request_digest);
        assert_ne!(first.request_digest, changed_plan.request_digest);
        assert_ne!(first.plan_digest, changed_plan.plan_digest);
        assert_eq!(
            first.request_digest,
            "e56f1165f5ff15bf3c1c1377b0aae4c59d31650e7dae7633cb9bc06471582fa0"
        );
        assert_eq!(
            first.plan_digest,
            "sha256:2ac5a436e8d866383cc04a9d8ea02e7b185d3462be53fea9e7a01f25375b739b"
        );
    }

    #[test]
    fn timeline_edit_request_json_is_strict_and_receipt_shape_is_exact() {
        let request = Ymm4TimelineEditApplyRequest::new(
            timeline_edit_plan_for_node_test("one", 1),
            format!("sha256:{}", "1".repeat(64)),
        )
        .unwrap();
        let mut value = serde_json::to_value(&request).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), true.into());
        assert!(serde_json::from_value::<Ymm4TimelineEditApplyRequest>(value).is_err());

        let receipt = serde_json::json!({
            "success": true,
            "replayed": false,
            "receipt": {
                "operationId": request.timeline_edit_plan.operation_id,
                "requestDigest": request.request_digest,
                "projectId": "project-a",
                "sceneId": "scene-a",
                "expectedFingerprint": request.expected_fingerprint,
                "planDigest": request.plan_digest,
                "status": "verified",
                "beforeFingerprint": format!("sha256:{}", "1".repeat(64)),
                "afterFingerprint": format!("sha256:{}", "2".repeat(64)),
                "appliedItems": [],
                "appliedNativeExtensions": [],
                "appliedOperationCount": 1,
                "verified": true,
                "error": null
            }
        });
        let decoded: Ymm4TimelineEditApplyResponse = serde_json::from_value(receipt).unwrap();
        assert_eq!(decoded.receipt.applied_operation_count, 1);
        assert_eq!(decoded.receipt.plan_digest, request.plan_digest);
        validate_timeline_edit_response(&request, &decoded).unwrap();
        let mut wrong = decoded;
        wrong.receipt.applied_operation_count = 0;
        assert!(matches!(
            validate_timeline_edit_response(&request, &wrong),
            Err(Ymm4Error::InvalidTimelineEditReceipt(_))
        ));
    }

    fn project_initialization_receipt_fixture() -> (
        Ymm4ProjectInitializationRequest,
        Ymm4ProjectInitializationReceipt,
    ) {
        let preparation = Ymm4ProjectInitializationPreparation {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            driver_profile_digest: format!("sha256:{}", "1".repeat(64)),
            source_project_instance_id: "instance-a".into(),
            source: Ymm4ProjectSnapshot {
                project_id: "project-untitled".into(),
                project_name: String::new(),
                project_path: String::new(),
                scene_id: "scene-a".into(),
                fps: 60,
                fingerprint: format!("sha256:{}", "2".repeat(64)),
                managed_items: vec![],
                native_extensions: vec![],
                unmanaged_context_count: 0,
            },
            destination_path: r"C:\projects\initialized.ymmp".into(),
            destination_path_digest: format!("sha256:{}", "3".repeat(64)),
            predicted_project_id: "project-saved".into(),
            predicted_fingerprint: format!("sha256:{}", "4".repeat(64)),
            overwrite: false,
        };
        let request = Ymm4ProjectInitializationRequest::new(
            Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
            &preparation,
        )
        .unwrap();
        let receipt = Ymm4ProjectInitializationReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest.clone(),
            status: Ymm4ProjectInitializationStatus::Verified,
            driver_profile_digest: request.driver_profile_digest.clone(),
            source_project_instance_id: request.source_project_instance_id.clone(),
            source_project_id: request.source_project_id.clone(),
            source_scene_id: request.source_scene_id.clone(),
            before_fingerprint: request.expected_source_fingerprint.clone(),
            destination_path: request.destination_path.clone(),
            destination_path_digest: request.destination_path_digest.clone(),
            predicted_project_id: request.predicted_project_id.clone(),
            predicted_fingerprint: request.predicted_fingerprint.clone(),
            prepared_temporary_path: Some(
                r"C:\projects\.takegraph-project-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.ymmp".into(),
            ),
            prepared_file_sha256: Some("5".repeat(64)),
            prepared_file_bytes: Some(1024),
            after_snapshot: Some(Ymm4ProjectSnapshot {
                project_id: request.predicted_project_id.clone(),
                project_name: "initialized".into(),
                project_path: request.destination_path.clone(),
                scene_id: request.source_scene_id.clone(),
                fps: 60,
                fingerprint: request.predicted_fingerprint.clone(),
                managed_items: vec![],
                native_extensions: vec![],
                unmanaged_context_count: 0,
            }),
            file_sha256: Some("5".repeat(64)),
            file_bytes: Some(1024),
            error: None,
        };
        (request, receipt)
    }

    #[test]
    fn target_plan_request_digest_matches_cross_runtime_golden() {
        let target_plan: TargetPlan = serde_json::from_value(serde_json::json!({
            "canonicalVersion": 1,
            "operationId": "11111111-1111-4111-8111-111111111111",
            "baseRevision": 7,
            "target": {
                "adapterId": "ymm4-4.55",
                "projectId": "project-日本語",
                "sceneId": "scene-魔理沙",
                "fps": 60,
                "driverVersion": "4.55.1.1/0.2.0"
            },
            "capabilityDigest": format!("sha256:{}", "1".repeat(64)),
            "expectedScope": {
                "targetIdentityDigest": format!("sha256:{}", "2".repeat(64)),
                "managedStateDigest": format!("sha256:{}", "3".repeat(64)),
                "conflictScopeDigest": format!("sha256:{}", "4".repeat(64))
            },
            "changeBudget": {
                "maxChangedEntities": 1,
                "maxShiftedEntities": 0,
                "maxShiftFrames": 0,
                "allowLockedChanges": false,
                "allowUnmanagedChanges": false
            },
            "cues": [],
            "warnings": []
        }))
        .unwrap();
        let plan_digest = canonical_sha256("takegraph-target-plan", &target_plan).unwrap();
        let expected_fingerprint = format!("sha256:{}", "5".repeat(64));
        let request_digest =
            target_plan_request_digest(2, &expected_fingerprint, &plan_digest, &target_plan)
                .unwrap();
        assert_eq!(
            request_digest,
            "18a5f6cf916dd6250a51ee63dc9a03879f6759c107b2f419b871e84ca31f79fb"
        );
        assert_ne!(
            request_digest,
            target_plan_request_digest(
                2,
                &format!("sha256:{}", "6".repeat(64)),
                &plan_digest,
                &target_plan,
            )
            .unwrap()
        );
    }

    #[test]
    fn accepts_only_loopback_and_nonempty_token() {
        assert!(Ymm4BridgeClient::new("http://127.0.0.1:8766", "secret").is_ok());
        assert!(Ymm4BridgeClient::new("http://localhost:8766", "secret").is_ok());
        assert!(matches!(
            Ymm4BridgeClient::new("https://example.com", "secret"),
            Err(Ymm4Error::NonLoopbackEndpoint(_))
        ));
        assert!(matches!(
            Ymm4BridgeClient::new("http://127.0.0.1:8766", ""),
            Err(Ymm4Error::MissingToken)
        ));
        assert!(matches!(
            Ymm4BridgeClient::new("http://127.0.0.1:8766", "secret")
                .unwrap()
                .with_expected_project_id("  "),
            Err(Ymm4Error::MissingExpectedProjectId)
        ));
    }

    #[test]
    fn project_initialization_request_is_new_path_only_and_cross_runtime_bound() {
        let preparation = Ymm4ProjectInitializationPreparation {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            driver_profile_digest: format!("sha256:{}", "1".repeat(64)),
            source_project_instance_id: "instance-日本語".into(),
            source: Ymm4ProjectSnapshot {
                project_id: "project-untitled".into(),
                project_name: String::new(),
                project_path: String::new(),
                scene_id: "scene-a".into(),
                fps: 60,
                fingerprint: format!("sha256:{}", "2".repeat(64)),
                managed_items: vec![],
                native_extensions: vec![],
                unmanaged_context_count: 0,
            },
            destination_path: r"C:\projects\新規.ymmp".into(),
            destination_path_digest: format!("sha256:{}", "3".repeat(64)),
            predicted_project_id: "project-saved".into(),
            predicted_fingerprint: format!("sha256:{}", "4".repeat(64)),
            overwrite: false,
        };
        let request = Ymm4ProjectInitializationRequest::new(
            Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
            &preparation,
        )
        .unwrap();
        assert_eq!(
            request.request_digest,
            "32bbaf6a44ed11fdfa2c8de61cd6d6ed309047ebaf0d116bfb1add97d0d4b79a"
        );
        assert!(!request.overwrite);

        let mut saved_source = preparation.clone();
        saved_source.source.project_path = r"C:\projects\source.ymmp".into();
        assert!(matches!(
            Ymm4ProjectInitializationRequest::new(Uuid::new_v4(), &saved_source),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));
        let mut overwrite = preparation;
        overwrite.overwrite = true;
        assert!(matches!(
            Ymm4ProjectInitializationRequest::new(Uuid::new_v4(), &overwrite),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));
    }

    #[test]
    fn project_initialization_verified_receipt_requires_matching_prepared_evidence() {
        let (request, receipt) = project_initialization_receipt_fixture();
        assert!(validate_project_initialization_receipt(&request, &receipt).is_ok());

        let mut replayed = receipt.clone();
        replayed.status = Ymm4ProjectInitializationStatus::Replayed;
        assert!(validate_project_initialization_receipt(&request, &replayed).is_ok());

        let mut mismatched_hash = receipt.clone();
        mismatched_hash.prepared_file_sha256 = Some("6".repeat(64));
        assert!(matches!(
            validate_project_initialization_receipt(&request, &mismatched_hash),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));

        let mut mismatched_bytes = receipt.clone();
        mismatched_bytes.prepared_file_bytes = Some(2048);
        assert!(matches!(
            validate_project_initialization_receipt(&request, &mismatched_bytes),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));

        let mut foreign_temporary_path = receipt.clone();
        foreign_temporary_path.prepared_temporary_path =
            Some(r"C:\other\.takegraph-project-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.ymmp".into());
        assert!(matches!(
            validate_project_initialization_receipt(&request, &foreign_temporary_path),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));

        let mut unowned_temporary_path = receipt.clone();
        unowned_temporary_path.prepared_temporary_path =
            Some(r"C:\projects\manually-created.ymmp".into());
        assert!(matches!(
            validate_project_initialization_receipt(&request, &unowned_temporary_path),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));

        let mut malformed_hash = receipt;
        malformed_hash.prepared_file_sha256 = Some("not-a-sha256".into());
        malformed_hash.file_sha256 = Some("not-a-sha256".into());
        assert!(matches!(
            validate_project_initialization_receipt(&request, &malformed_hash),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));
    }

    #[test]
    fn project_initialization_in_progress_receipt_rejects_partial_prepared_evidence() {
        let (request, mut receipt) = project_initialization_receipt_fixture();
        receipt.status = Ymm4ProjectInitializationStatus::Applying;
        receipt.after_snapshot = None;
        receipt.file_sha256 = None;
        receipt.file_bytes = None;

        receipt.prepared_temporary_path = None;
        receipt.prepared_file_sha256 = None;
        receipt.prepared_file_bytes = None;
        assert!(validate_project_initialization_receipt(&request, &receipt).is_ok());

        receipt.prepared_temporary_path =
            Some(r"C:\projects\.takegraph-project-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.ymmp".into());
        assert!(validate_project_initialization_receipt(&request, &receipt).is_ok());

        receipt.status = Ymm4ProjectInitializationStatus::RecoveryRequired;
        receipt.prepared_file_sha256 = Some("5".repeat(64));
        receipt.prepared_file_bytes = Some(1024);
        assert!(validate_project_initialization_receipt(&request, &receipt).is_ok());

        receipt.status = Ymm4ProjectInitializationStatus::Failed;
        receipt.prepared_temporary_path = None;
        assert!(matches!(
            validate_project_initialization_receipt(&request, &receipt),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));

        receipt.prepared_temporary_path =
            Some(r"C:\projects\.takegraph-project-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.ymmp".into());
        receipt.prepared_file_bytes = Some(0);
        assert!(matches!(
            validate_project_initialization_receipt(&request, &receipt),
            Err(Ymm4Error::InvalidProjectInitialization(_))
        ));
    }

    #[test]
    fn project_initialization_failed_receipt_requires_authenticated_untouched_source() {
        let (request, mut receipt) = project_initialization_receipt_fixture();
        receipt.status = Ymm4ProjectInitializationStatus::Failed;
        receipt.prepared_temporary_path = None;
        receipt.prepared_file_sha256 = None;
        receipt.prepared_file_bytes = None;
        receipt.after_snapshot = Some(Ymm4ProjectSnapshot {
            project_id: request.source_project_id.clone(),
            project_name: String::new(),
            project_path: String::new(),
            scene_id: request.source_scene_id.clone(),
            fps: 60,
            fingerprint: request.expected_source_fingerprint.clone(),
            managed_items: vec![],
            native_extensions: vec![],
            unmanaged_context_count: 0,
        });
        receipt.file_sha256 = None;
        receipt.file_bytes = None;
        receipt.error = Some("destination validation failed".into());
        assert!(validate_project_initialization_receipt(&request, &receipt).is_ok());

        let mut prepared = receipt.clone();
        prepared.prepared_temporary_path =
            Some(r"C:\projects\.takegraph-project-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.ymmp".into());
        assert!(validate_project_initialization_receipt(&request, &prepared).is_err());

        let mut missing_snapshot = receipt.clone();
        missing_snapshot.after_snapshot = None;
        assert!(validate_project_initialization_receipt(&request, &missing_snapshot).is_err());

        for mutate in [
            |snapshot: &mut Ymm4ProjectSnapshot| snapshot.project_id.push_str("-changed"),
            |snapshot: &mut Ymm4ProjectSnapshot| snapshot.scene_id.push_str("-changed"),
            |snapshot: &mut Ymm4ProjectSnapshot| snapshot.fingerprint.push('0'),
            |snapshot: &mut Ymm4ProjectSnapshot| {
                snapshot.project_path = r"C:\projects\initialized.ymmp".into();
            },
        ] {
            let mut changed_source = receipt.clone();
            mutate(changed_source.after_snapshot.as_mut().unwrap());
            assert!(validate_project_initialization_receipt(&request, &changed_source).is_err());
        }

        let mut final_hash = receipt.clone();
        final_hash.file_sha256 = Some("5".repeat(64));
        assert!(validate_project_initialization_receipt(&request, &final_hash).is_err());

        let mut final_bytes = receipt.clone();
        final_bytes.file_bytes = Some(1024);
        assert!(validate_project_initialization_receipt(&request, &final_bytes).is_err());

        let mut no_error = receipt.clone();
        no_error.error = None;
        assert!(validate_project_initialization_receipt(&request, &no_error).is_err());

        let mut blank_error = receipt;
        blank_error.error = Some(" \t".into());
        assert!(validate_project_initialization_receipt(&request, &blank_error).is_err());
    }

    #[tokio::test]
    async fn snapshot_rejects_an_active_project_switch() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            let body = serde_json::json!({
                "projectId": "project-b",
                "projectName": "switched",
                "projectPath": "project-b.ymmp",
                "sceneId": "scene-b",
                "fps": 60,
                "fingerprint": "f".repeat(64),
                "managedItems": [],
                "nativeExtensions": [],
                "unmanagedContextCount": 0
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let client = Ymm4BridgeClient::new(&format!("http://{address}"), "secret")
            .unwrap()
            .with_expected_project_id("project-a")
            .unwrap();

        let result = client.snapshot().await;
        server.join().unwrap();
        assert!(matches!(
            result,
            Err(Ymm4Error::UnexpectedProject { expected, actual })
                if expected == "project-a" && actual == "project-b"
        ));
    }

    #[test]
    fn contract_uses_camel_case_and_stable_operation_id() {
        let operation_id = Uuid::nil();
        let request = Ymm4ApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![ManagedUtterance {
                entity_id: "utt-01".into(),
                revision: 4,
                speaker: "春日部つむぎ".into(),
                caption: "ここから第二形態です".into(),
                spoken_text: "ここから第二形態です".into(),
                audio_path: r"C:\artifacts\take.wav".into(),
                artifact_hash: "abc".into(),
                frame: 120,
                length: 60,
                audio_layer: 20,
                caption_layer: 21,
            }],
        );
        assert_eq!(
            request.request_digest,
            "0330383be4127ab508fc5ebff79d0586fffc8d542edf0d245a92fdb547318c15"
        );
        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["operationId"], operation_id.to_string());
        assert_eq!(json["projectId"], "project-a");
        assert_eq!(json["sceneId"], "scene-a");
        assert_eq!(json["requestDigest"].as_str().unwrap().len(), 64);
        assert_eq!(json["utterances"][0]["speaker"], "春日部つむぎ");
        assert_eq!(json["utterances"][0]["spokenText"], "ここから第二形態です");
        assert!(json.get("operation_id").is_none());

        let mut legacy = json["utterances"][0].clone();
        legacy.as_object_mut().unwrap().remove("spokenText");
        let legacy: ManagedUtterance = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.spoken_text, legacy.caption);

        let mut separate_text = json["utterances"][0].clone();
        separate_text["spokenText"] = "ここからだいにけいたいです".into();
        let separate_text: ManagedUtterance = serde_json::from_value(separate_text).unwrap();
        let separate_text_request = Ymm4ApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![separate_text],
        );
        assert_eq!(
            separate_text_request.request_digest,
            "4b6d857b78891e00d753e835eab9304dfdc50de5d82f4cff7dc721fa20ca3895"
        );
    }

    #[test]
    fn apply_request_digest_binds_target_and_payload() {
        let operation_id = Uuid::nil();
        let utterance = ManagedUtterance {
            entity_id: "utt-01".into(),
            revision: 4,
            speaker: "春日部つむぎ".into(),
            caption: "表示文\n二行目".into(),
            spoken_text: "よみあげぶん".into(),
            audio_path: r"C:\artifacts\take.wav".into(),
            artifact_hash: "abc".into(),
            frame: 120,
            length: 60,
            audio_layer: 20,
            caption_layer: 21,
        };
        let baseline = Ymm4ApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![utterance.clone()],
        );
        let changed_scene = Ymm4ApplyRequest::new(
            operation_id,
            "project-a",
            "scene-b",
            "fingerprint-a",
            vec![utterance.clone()],
        );
        let mut changed_utterance = utterance;
        changed_utterance.spoken_text.push('!');
        let changed_payload = Ymm4ApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![changed_utterance],
        );

        assert_ne!(baseline.request_digest, changed_scene.request_digest);
        assert_ne!(baseline.request_digest, changed_payload.request_digest);
    }

    #[test]
    fn native_voice_contract_uses_camel_case() {
        let operation_id = Uuid::nil();
        let request = Ymm4NativeVoiceApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![native_voice_cue()],
        );
        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["protocolVersion"], YMM4_BRIDGE_PROTOCOL_VERSION);
        assert_eq!(json["operationId"], operation_id.to_string());
        assert_eq!(json["projectId"], "project-a");
        assert_eq!(
            json["cues"][0]["realizationId"],
            "11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(json["cues"][0]["characterName"], "春日部つむぎ");
        assert_eq!(json["cues"][0]["maxLength"], 180);
        assert!(json.get("operation_id").is_none());
    }

    #[test]
    fn native_managed_item_deserializes_voice_kind_and_identity() {
        let item: Ymm4ManagedItem = serde_json::from_value(serde_json::json!({
            "entityId": "utt-01",
            "revision": 4,
            "kind": "voice",
            "frame": 120,
            "layer": 20,
            "length": 90,
            "text": "ここから第二形態です",
            "audioPath": null,
            "artifactHash": null,
            "speaker": "春日部つむぎ",
            "realizationId": "11111111-2222-3333-4444-555555555555"
        }))
        .unwrap();

        assert_eq!(item.kind, ManagedItemKind::Voice);
        assert_eq!(item.speaker.as_deref(), Some("春日部つむぎ"));
        assert_eq!(item.realization_id, Some(native_voice_cue().realization_id));
    }

    #[test]
    fn capabilities_deserialize_verified_native_voice_profile() {
        let capabilities: Ymm4Capabilities = serde_json::from_value(serde_json::json!({
            "protocolVersion": 2,
            "capabilities": [
                "request_bound_receipts",
                "native_voice_create",
                "native_voice_remark_identity",
                "native_voice_bounded_duration",
                "mutation_profile_ymm4_4_55_1_1",
                "scene_composition_current"
            ]
        }))
        .unwrap();
        assert!(
            capabilities
                .capabilities
                .contains(&Ymm4Capability::NativeVoiceCreate)
        );
        assert!(
            capabilities
                .capabilities
                .contains(&Ymm4Capability::MutationProfileYmm4_4_55_1_1)
        );
        assert!(
            capabilities
                .capabilities
                .contains(&Ymm4Capability::SceneCompositionCurrent)
        );
    }

    #[test]
    fn native_voice_apply_digest_matches_cross_runtime_golden_and_binds_order() {
        let operation_id = Uuid::nil();
        let baseline = Ymm4NativeVoiceApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![native_voice_cue()],
        );
        assert_eq!(
            baseline.request_digest,
            "f71d95d8871636271eeacf9fe4ddd858eaf706d1b38a8c63b0ec0dc11a6ad932"
        );

        let mut second = native_voice_cue();
        second.realization_id = Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();
        second.entity_id = "utt-02".into();
        let ordered = Ymm4NativeVoiceApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![native_voice_cue(), second.clone()],
        );
        let reversed = Ymm4NativeVoiceApplyRequest::new(
            operation_id,
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![second, native_voice_cue()],
        );
        assert_ne!(ordered.request_digest, reversed.request_digest);
    }

    #[test]
    fn native_voice_mutation_contract_and_digest_bind_action() {
        let baseline = Ymm4NativeVoiceMutationApplyRequest::new(
            Uuid::nil(),
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![native_voice_mutation(Ymm4NativeVoiceMutationAction::Update)],
        );
        assert_eq!(
            baseline.request_digest,
            "ac5a73a7bc78fa68d8d1835018d7d7da4752ee987d23c25749317657a595f7c4"
        );
        let json = serde_json::to_value(&baseline).unwrap();
        assert_eq!(json["mutations"][0]["action"], "update");
        assert_eq!(
            json["mutations"][0]["realizationId"],
            native_voice_cue().realization_id.to_string()
        );

        let changed = Ymm4NativeVoiceMutationApplyRequest::new(
            Uuid::nil(),
            "project-a",
            "scene-a",
            "fingerprint-a",
            vec![native_voice_mutation(Ymm4NativeVoiceMutationAction::Delete)],
        );
        assert_ne!(baseline.request_digest, changed.request_digest);
    }

    #[test]
    fn artifact_contract_keeps_exact_hash_and_host_bound_provenance_distinct() {
        let artifact: Ymm4NativeVoiceArtifact = serde_json::from_value(serde_json::json!({
            "realizationId": "11111111-2222-3333-4444-555555555555",
            "audioPath": "audio/a.wav",
            "audioSha256": "a".repeat(64),
            "audioBytes": 44,
            "queryPath": "queries/q.json",
            "querySha256": "b".repeat(64),
            "queryBytes": 123,
            "provenance": "ymm4-native-create-audio+normalized-host-bound-voice-state/4.55.1.1"
        }))
        .unwrap();
        assert_eq!(artifact.audio_sha256, "a".repeat(64));
        assert!(artifact.provenance.contains("host-bound"));
    }
}
