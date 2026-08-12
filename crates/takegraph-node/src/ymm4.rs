use std::collections::BTreeMap;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use takegraph_core::{TargetPlan, TargetPlanError, canonical_sha256};
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
    NativePortraitUpsert,
    NativeFaceUpsert,
    NativeImageUpsert,
    NativeVideoUpsert,
    NativeAudioUpsert,
    NativeEffectTypedMutation,
    NativeTemplateInstantiate,
    ProjectCheckpointVerified,
    ProjectRender,
    ProjectRenderCancel,
    ProjectRenderMediaReceipt,
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
    pub spoken_text: String,
    pub frame: i32,
    pub layer: i32,
    pub max_length: i32,
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
    pub spoken_text: String,
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
        write_string(&mut canonical, "spokenText", &cue.spoken_text);
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
        write_string(&mut canonical, "spokenText", &mutation.spoken_text);
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
        if let Some(expected) = &self.expected_project_id
            && snapshot.project_id != *expected
        {
            return Err(Ymm4Error::UnexpectedProject {
                expected: expected.clone(),
                actual: snapshot.project_id,
            });
        }
        Ok(snapshot)
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
            spoken_text: "ここから第二形態です".into(),
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
            spoken_text: "ここから第二形態です".into(),
            frame: 120,
            layer: 20,
            max_length: 180,
            action,
        }
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
                "mutation_profile_ymm4_4_55_1_1"
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
