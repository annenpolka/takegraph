use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::RevisionId;

/// Version of the canonical JSON encoding used by target-plan digests.
pub const TARGET_PLAN_CANONICAL_VERSION: u32 = 1;

/// Portable editing meaning for one managed spoken cue.
///
/// This type deliberately contains no YMM4 item type, path, frame, or layer.
/// Those are selected in a [`TargetPlan`] and can therefore change without
/// changing the canonical cue intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCueIntent {
    pub entity_id: String,
    pub entity_revision: u64,
    pub display_text: String,
    /// Approved synthesis input. `None` leaves Hatsuon to the YMM4 driver.
    #[serde(default)]
    pub spoken_text: Option<String>,
    pub speaker_role: String,
    pub voice_profile: Option<String>,
    pub caption_style: Option<String>,
    pub segmentation_locked: bool,
    pub placement: PlacementIntent,
    pub realization_preference: RealizationPreference,
    pub fallback_policy: FallbackPolicy,
    pub accepted_portable_take: Option<Uuid>,
    pub template: Option<TargetReference>,
    pub effects: Vec<TargetReference>,
    pub hard_lock_preconditions: Vec<String>,
}

impl ManagedCueIntent {
    /// Constructs the required portable cue fields with conservative defaults.
    #[must_use]
    pub fn new(
        entity_id: impl Into<String>,
        entity_revision: u64,
        display_text: impl Into<String>,
        spoken_text: impl Into<String>,
        speaker_role: impl Into<String>,
        placement: PlacementIntent,
    ) -> Self {
        Self {
            entity_id: entity_id.into(),
            entity_revision,
            display_text: display_text.into(),
            spoken_text: Some(spoken_text.into()),
            speaker_role: speaker_role.into(),
            voice_profile: None,
            caption_style: None,
            segmentation_locked: false,
            placement,
            realization_preference: RealizationPreference::PreferNative,
            fallback_policy: FallbackPolicy::AllowPortable,
            accepted_portable_take: None,
            template: None,
            effects: Vec::new(),
            hard_lock_preconditions: Vec::new(),
        }
    }

    /// Selects an explicit realization strategy for preview.
    ///
    /// A text mismatch can use native voice only when the target explicitly
    /// reports support for separate display and spoken text. Any fallback is
    /// returned as data so it becomes part of the approved target plan.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested strategy is unavailable or fallback
    /// was forbidden.
    pub fn select_strategy(
        &self,
        availability: RealizationAvailability,
    ) -> Result<StrategySelection, TargetPlanError> {
        self.validate()?;
        let native_text_supported = match self.spoken_text.as_deref() {
            None => true,
            Some(spoken) => {
                spoken == self.display_text || availability.native_separate_display_and_spoken_text
            }
        };
        let native_available = availability.native_voice && native_text_supported;

        match self.realization_preference {
            RealizationPreference::RequirePortable => {
                if availability.portable_audio_caption {
                    Ok(StrategySelection {
                        strategy: RealizationStrategy::PortableAudioCaption,
                        fallback: None,
                    })
                } else {
                    Err(TargetPlanError::RequiredStrategyUnavailable(
                        RealizationStrategy::PortableAudioCaption,
                    ))
                }
            }
            RealizationPreference::RequireNative => {
                if native_available {
                    Ok(StrategySelection {
                        strategy: RealizationStrategy::Ymm4NativeVoice,
                        fallback: None,
                    })
                } else if availability.native_voice {
                    Err(TargetPlanError::SeparateTextUnsupported)
                } else {
                    Err(TargetPlanError::RequiredStrategyUnavailable(
                        RealizationStrategy::Ymm4NativeVoice,
                    ))
                }
            }
            RealizationPreference::PreferNative if native_available => Ok(StrategySelection {
                strategy: RealizationStrategy::Ymm4NativeVoice,
                fallback: None,
            }),
            RealizationPreference::PreferNative => {
                if self.fallback_policy == FallbackPolicy::Reject {
                    return Err(if availability.native_voice {
                        TargetPlanError::SeparateTextUnsupported
                    } else {
                        TargetPlanError::FallbackForbidden
                    });
                }
                if !availability.portable_audio_caption {
                    return Err(TargetPlanError::RequiredStrategyUnavailable(
                        RealizationStrategy::PortableAudioCaption,
                    ));
                }
                Ok(StrategySelection {
                    strategy: RealizationStrategy::PortableAudioCaption,
                    fallback: Some(if availability.native_voice {
                        FallbackReason::SeparateDisplayAndSpokenText
                    } else {
                        FallbackReason::MissingNativeCapability
                    }),
                })
            }
        }
    }

    /// Validates fields that are required independent of any target.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty stable identity, text, or speaker role.
    pub fn validate(&self) -> Result<(), TargetPlanError> {
        if self.entity_id.trim().is_empty() {
            return Err(TargetPlanError::EmptyField("entityId"));
        }
        if self.display_text.trim().is_empty() {
            return Err(TargetPlanError::EmptyField("displayText"));
        }
        if self
            .spoken_text
            .as_ref()
            .is_some_and(|spoken| spoken.trim().is_empty())
        {
            return Err(TargetPlanError::EmptyField("spokenText"));
        }
        if self.speaker_role.trim().is_empty() {
            return Err(TargetPlanError::EmptyField("speakerRole"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementIntent {
    pub anchor: TimingAnchor,
    pub ordering: OrderingPolicy,
    pub track_role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimingAnchor {
    AbsoluteFrame { frame: i32 },
    AfterEntity { entity_id: String, gap_frames: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingPolicy {
    Fixed,
    RippleFollowing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealizationPreference {
    PreferNative,
    RequireNative,
    RequirePortable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    AllowPortable,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetReference {
    pub id: String,
    pub expected_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealizationAvailability {
    pub native_voice: bool,
    pub native_separate_display_and_spoken_text: bool,
    pub portable_audio_caption: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategySelection {
    pub strategy: RealizationStrategy,
    pub fallback: Option<FallbackReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealizationStrategy {
    #[serde(rename = "native_voice")]
    Ymm4NativeVoice,
    #[serde(rename = "portable_pair")]
    PortableAudioCaption,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    MissingNativeCapability,
    SeparateDisplayAndSpokenText,
    LossyNativeReplacement,
}

/// The exact target-specific plan sealed by approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetPlan {
    pub canonical_version: u32,
    pub operation_id: Uuid,
    pub base_revision: RevisionId,
    pub target: TargetIdentity,
    pub capability_digest: String,
    pub expected_scope: ScopeFingerprints,
    pub change_budget: ChangeBudget,
    pub cues: Vec<PlannedCue>,
    pub warnings: Vec<PlanWarning>,
}

impl TargetPlan {
    /// Computes a deterministic SHA-256 digest over all approval-relevant data.
    ///
    /// # Errors
    ///
    /// Returns an error when validation or canonical serialization fails.
    pub fn canonical_digest(&self) -> Result<String, TargetPlanError> {
        self.validate()?;
        canonical_sha256("takegraph-target-plan", self).map_err(TargetPlanError::Canonical)
    }

    /// Checks target-plan invariants before digesting or applying it.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported canonical version, empty binding,
    /// duplicate cue identity, or cue exceeding the approved plan budget.
    pub fn validate(&self) -> Result<(), TargetPlanError> {
        if self.canonical_version != TARGET_PLAN_CANONICAL_VERSION {
            return Err(TargetPlanError::UnsupportedCanonicalVersion(
                self.canonical_version,
            ));
        }
        if self.target.adapter_id.trim().is_empty()
            || self.target.project_id.trim().is_empty()
            || self.target.scene_id.trim().is_empty()
        {
            return Err(TargetPlanError::EmptyField("target binding"));
        }
        if self.operation_id.is_nil() {
            return Err(TargetPlanError::EmptyField("operationId"));
        }
        if self.target.fps == 0 {
            return Err(TargetPlanError::InvalidCue(
                "target FPS must be positive".into(),
            ));
        }
        require_sha256(&self.capability_digest, "capabilityDigest")?;
        require_sha256(
            &self.expected_scope.target_identity_digest,
            "expectedScope.targetIdentityDigest",
        )?;
        require_sha256(
            &self.expected_scope.managed_state_digest,
            "expectedScope.managedStateDigest",
        )?;
        require_sha256(
            &self.expected_scope.conflict_scope_digest,
            "expectedScope.conflictScopeDigest",
        )?;
        if self.cues.is_empty() {
            return Err(TargetPlanError::EmptyField("cues"));
        }
        let mut identities = std::collections::BTreeSet::new();
        for cue in &self.cues {
            validate_planned_cue(cue)?;
            if !identities.insert((&cue.intent.entity_id, cue.realization_id)) {
                return Err(TargetPlanError::DuplicateCueIdentity(
                    cue.intent.entity_id.clone(),
                ));
            }
        }
        if self.cues.len() > self.change_budget.max_changed_entities {
            return Err(TargetPlanError::ChangeBudgetExceeded);
        }
        Ok(())
    }
}

pub(crate) fn validate_planned_cue(cue: &PlannedCue) -> Result<(), TargetPlanError> {
    cue.intent.validate()?;
    if cue.realization_id.is_nil() {
        return invalid_cue(cue, "has an empty realizationId");
    }
    if cue.placement.frame < 0
        || cue.placement.primary_layer < 0
        || cue.placement.secondary_layer.is_some_and(|value| value < 0)
        || cue.placement.secondary_layer == Some(cue.placement.primary_layer)
    {
        return invalid_cue(cue, "has an invalid resolved placement");
    }
    if let TimingAnchor::AbsoluteFrame { frame } = cue.intent.placement.anchor
        && frame != cue.placement.frame
    {
        return invalid_cue(cue, "resolved frame differs from its absolute intent");
    }
    validate_capability_dependencies(cue)?;
    validate_binding_dependencies(cue)?;
    validate_resolved_realization(cue)
}

fn validate_capability_dependencies(cue: &PlannedCue) -> Result<(), TargetPlanError> {
    if cue.capability_dependencies.is_empty() {
        return invalid_cue(cue, "has no capability dependency");
    }
    let mut names = std::collections::BTreeSet::new();
    for dependency in &cue.capability_dependencies {
        if dependency.feature.trim().is_empty()
            || dependency.minimum_version == 0
            || !names.insert(dependency.feature.as_str())
        {
            return invalid_cue(cue, "has an invalid capability dependency");
        }
        let Some(schema_digest) = dependency.schema_digest.as_deref() else {
            return invalid_cue(cue, "has an unsealed capability schema");
        };
        require_sha256(schema_digest, "capability dependency schemaDigest")?;
    }
    Ok(())
}

fn validate_binding_dependencies(cue: &PlannedCue) -> Result<(), TargetPlanError> {
    let mut keys = std::collections::BTreeSet::new();
    for dependency in &cue.binding_dependencies {
        if dependency.kind.trim().is_empty()
            || dependency.id.trim().is_empty()
            || !keys.insert((dependency.kind.as_str(), dependency.id.as_str()))
        {
            return invalid_cue(cue, "has an invalid binding dependency");
        }
        require_sha256(&dependency.digest, "binding dependency digest")?;
    }
    Ok(())
}

fn validate_resolved_realization(cue: &PlannedCue) -> Result<(), TargetPlanError> {
    match (&cue.strategy, &cue.resolved_realization) {
        (
            RealizationStrategy::PortableAudioCaption,
            ResolvedRealization::PortablePair {
                audio_path,
                artifact_digest,
            },
        ) => {
            if audio_path.trim().is_empty() || cue.placement.secondary_layer.is_none() {
                return invalid_cue(cue, "has an incomplete portable-pair realization");
            }
            require_sha256(artifact_digest, "portable artifactDigest")?;
            if !cue.binding_dependencies.iter().any(|binding| {
                binding.kind == "audio_artifact" && binding.digest == *artifact_digest
            }) {
                return invalid_cue(cue, "portable artifact is not dependency-bound");
            }
        }
        (
            RealizationStrategy::Ymm4NativeVoice,
            ResolvedRealization::NativeVoice {
                character_name,
                character_binding_digest,
            },
        ) => {
            if character_name.trim().is_empty() || cue.placement.secondary_layer.is_some() {
                return invalid_cue(cue, "has an invalid native-voice realization");
            }
            require_sha256(character_binding_digest, "character binding digest")?;
            if !cue.binding_dependencies.iter().any(|binding| {
                binding.id == *character_name && binding.digest == *character_binding_digest
            }) {
                return invalid_cue(cue, "native character is not dependency-bound");
            }
        }
        _ => return invalid_cue(cue, "strategy and resolved realization differ"),
    }
    Ok(())
}

fn invalid_cue<T>(cue: &PlannedCue, message: &str) -> Result<T, TargetPlanError> {
    Err(TargetPlanError::InvalidCue(format!(
        "{} {message}",
        cue.intent.entity_id
    )))
}

fn require_sha256(value: &str, field: &'static str) -> Result<(), TargetPlanError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(TargetPlanError::InvalidDigest(field));
    };
    if hex.len() != 64 || !hex.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(TargetPlanError::InvalidDigest(field));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetIdentity {
    pub adapter_id: String,
    pub project_id: String,
    pub scene_id: String,
    pub fps: u32,
    pub driver_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeFingerprints {
    pub target_identity_digest: String,
    pub managed_state_digest: String,
    pub conflict_scope_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeBudget {
    pub max_changed_entities: usize,
    pub max_shifted_entities: usize,
    pub max_shift_frames: u32,
    pub allow_locked_changes: bool,
    pub allow_unmanaged_changes: bool,
}

impl ChangeBudget {
    #[must_use]
    pub const fn create_only(max_changed_entities: usize) -> Self {
        Self {
            max_changed_entities,
            max_shifted_entities: 0,
            max_shift_frames: 0,
            allow_locked_changes: false,
            allow_unmanaged_changes: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedCue {
    pub intent: ManagedCueIntent,
    pub realization_id: Uuid,
    pub action: PlannedAction,
    pub strategy: RealizationStrategy,
    pub fallback: Option<FallbackReason>,
    pub placement: ResolvedPlacement,
    pub duration: DurationResolution,
    pub ownership: OwnershipMask,
    pub capability_dependencies: Vec<CapabilityDependency>,
    pub binding_dependencies: Vec<BindingDependency>,
    /// Driver-ready values resolved before approval. The bridge consumes this
    /// sealed payload as-is and never re-resolves placement or target bindings
    /// from portable intent fields.
    pub resolved_realization: ResolvedRealization,
}

/// Exact driver payload selected by a target planner.
///
/// This is deliberately narrower than the legacy physical request DTOs: frame,
/// layer, duration, text, ownership and dependencies remain first-class fields
/// on [`PlannedCue`]. Only values that cannot be reconstructed without crossing
/// an external boundary live here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedRealization {
    PortablePair {
        audio_path: String,
        artifact_digest: String,
    },
    NativeVoice {
        character_name: String,
        character_binding_digest: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedAction {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPlacement {
    pub frame: i32,
    pub primary_layer: i32,
    pub secondary_layer: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DurationResolution {
    Exact { frames: u32 },
    Bounded { max_frames: u32 },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipMask {
    pub strict: Vec<String>,
    pub derived: Vec<String>,
    pub preserve: Vec<String>,
    pub global: Vec<String>,
}

impl OwnershipMask {
    #[must_use]
    pub fn native_voice_create() -> Self {
        Self::native_voice_create_with_spoken(true)
    }

    /// Native create ownership. Bound spoken text is strict; omitted spoken
    /// text leaves pronunciation as a derived YMM field.
    #[must_use]
    pub fn native_voice_create_with_spoken(spoken_bound: bool) -> Self {
        if spoken_bound {
            Self {
                strict: vec![
                    "identity".into(),
                    "displayText".into(),
                    "spokenText".into(),
                    "characterBinding".into(),
                    "timingIntent".into(),
                ],
                derived: vec!["length".into(), "voiceCache".into()],
                preserve: vec!["unknownNativeFields".into()],
                global: vec!["characterDefinitions".into(), "projectSettings".into()],
            }
        } else {
            Self {
                strict: vec![
                    "identity".into(),
                    "displayText".into(),
                    "characterBinding".into(),
                    "timingIntent".into(),
                ],
                derived: vec!["length".into(), "pronunciation".into(), "voiceCache".into()],
                preserve: vec!["unknownNativeFields".into()],
                global: vec!["characterDefinitions".into(), "projectSettings".into()],
            }
        }
    }

    #[must_use]
    pub fn portable_pair_create() -> Self {
        Self {
            strict: vec![
                "identity".into(),
                "captionText".into(),
                "audioArtifact".into(),
                "timing".into(),
            ],
            derived: Vec::new(),
            preserve: vec!["unknownNativeFields".into()],
            global: vec!["projectSettings".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDependency {
    pub feature: String,
    pub minimum_version: u32,
    pub schema_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingDependency {
    pub kind: String,
    pub id: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanWarning {
    pub code: String,
    pub message: String,
}

/// Normalized semantic result returned by target read-back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRealization {
    pub realization_id: Uuid,
    pub entity_id: String,
    pub entity_revision: u64,
    pub strategy: RealizationStrategy,
    pub native_item_ids: Vec<String>,
    pub resolved_frame: i32,
    pub resolved_layers: Vec<i32>,
    pub actual_length: u32,
    pub owned_field_digest: String,
    /// Present only when the driver can observe every field declared `preserve`.
    pub preserved_field_digest: Option<String>,
    pub host_bound: bool,
    pub artifact_digests: BTreeMap<String, String>,
}

/// Produces canonical JSON independent of object insertion order and hashes it
/// with a domain separator.
///
/// # Errors
///
/// Returns an error if the value cannot be converted to JSON.
pub fn canonical_sha256<T: Serialize + ?Sized>(
    domain: &str,
    value: &T,
) -> Result<String, CanonicalError> {
    let value = serde_json::to_value(value)?;
    let mut bytes = Vec::new();
    write_canonical_json(&value, &mut bytes)?;
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Compares approval-bound SHA-256 digests.
///
/// Callers may present either the canonical `sha256:<hex>` form or the bare
/// 64-character hex. Other opaque tokens still require an exact match.
#[must_use]
pub fn approval_digests_match(stored: &str, presented: &str) -> bool {
    if stored == presented {
        return true;
    }
    match (sha256_hex(stored), sha256_hex(presented)) {
        (Some(left), Some(right)) => left == right,
        _ => stored.eq_ignore_ascii_case(presented),
    }
}

fn sha256_hex(value: &str) -> Option<String> {
    let hex = value
        .strip_prefix("sha256:")
        .or_else(|| value.strip_prefix("SHA256:"))
        .unwrap_or(value);
    if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(hex.to_ascii_lowercase())
    } else {
        None
    }
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), serde_json::Error> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        Value::String(value) => serde_json::to_writer(output, value)?,
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            output.push(b'{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_canonical_json(&values[key], output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum CanonicalError {
    #[error("canonical JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum TargetPlanError {
    #[error("required field is empty: {0}")]
    EmptyField(&'static str),
    #[error("unsupported target-plan canonical version: {0}")]
    UnsupportedCanonicalVersion(u32),
    #[error("duplicate cue identity in target plan: {0}")]
    DuplicateCueIdentity(String),
    #[error("target plan exceeds its change budget")]
    ChangeBudgetExceeded,
    #[error("invalid SHA-256 digest: {0}")]
    InvalidDigest(&'static str),
    #[error("invalid planned cue: {0}")]
    InvalidCue(String),
    #[error("required realization strategy is unavailable: {0:?}")]
    RequiredStrategyUnavailable(RealizationStrategy),
    #[error("native voice cannot represent separate display and spoken text")]
    SeparateTextUnsupported,
    #[error("native fallback is required but forbidden")]
    FallbackForbidden,
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(display: &str, spoken: &str) -> ManagedCueIntent {
        ManagedCueIntent::new(
            "utt-01",
            4,
            display,
            spoken,
            "marisa",
            PlacementIntent {
                anchor: TimingAnchor::AbsoluteFrame { frame: 120 },
                ordering: OrderingPolicy::Fixed,
                track_role: "dialogue".into(),
            },
        )
    }

    #[test]
    fn separate_text_makes_portable_fallback_explicit() {
        let selection = cue("第二形態", "だいにけいたい")
            .select_strategy(RealizationAvailability {
                native_voice: true,
                native_separate_display_and_spoken_text: false,
                portable_audio_caption: true,
            })
            .unwrap();

        assert_eq!(
            selection,
            StrategySelection {
                strategy: RealizationStrategy::PortableAudioCaption,
                fallback: Some(FallbackReason::SeparateDisplayAndSpokenText),
            }
        );
    }

    #[test]
    fn omitted_spoken_text_admits_native() {
        let mut intent = cue("第二形態", "unused");
        intent.spoken_text = None;
        let selection = intent
            .select_strategy(RealizationAvailability {
                native_voice: true,
                native_separate_display_and_spoken_text: false,
                portable_audio_caption: true,
            })
            .unwrap();
        assert_eq!(
            selection,
            StrategySelection {
                strategy: RealizationStrategy::Ymm4NativeVoice,
                fallback: None,
            }
        );
    }

    #[test]
    fn advertised_separate_text_admits_native() {
        let selection = cue("第二形態", "だいにけいたい")
            .select_strategy(RealizationAvailability {
                native_voice: true,
                native_separate_display_and_spoken_text: true,
                portable_audio_caption: true,
            })
            .unwrap();

        assert_eq!(
            selection,
            StrategySelection {
                strategy: RealizationStrategy::Ymm4NativeVoice,
                fallback: None,
            }
        );
    }

    #[test]
    fn required_native_rejects_separate_text_without_capability() {
        let mut required = cue("第二形態", "だいにけいたい");
        required.realization_preference = RealizationPreference::RequireNative;
        assert!(matches!(
            required
                .select_strategy(RealizationAvailability {
                    native_voice: true,
                    native_separate_display_and_spoken_text: false,
                    portable_audio_caption: true,
                })
                .unwrap_err(),
            TargetPlanError::SeparateTextUnsupported
        ));
    }

    #[test]
    fn target_plan_digest_binds_capability_scope_and_strategy() {
        let intent = cue("第二形態", "第二形態");
        let digest = |value: char| format!("sha256:{}", value.to_string().repeat(64));
        let mut plan = TargetPlan {
            canonical_version: TARGET_PLAN_CANONICAL_VERSION,
            operation_id: Uuid::from_u128(1),
            base_revision: RevisionId(7),
            target: TargetIdentity {
                adapter_id: "ymm4".into(),
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
            cues: vec![PlannedCue {
                intent,
                realization_id: Uuid::from_u128(2),
                action: PlannedAction::Create,
                strategy: RealizationStrategy::Ymm4NativeVoice,
                fallback: None,
                placement: ResolvedPlacement {
                    frame: 120,
                    primary_layer: 20,
                    secondary_layer: None,
                },
                duration: DurationResolution::Bounded { max_frames: 180 },
                ownership: OwnershipMask::native_voice_create(),
                capability_dependencies: vec![CapabilityDependency {
                    feature: "voiceItem.create".into(),
                    minimum_version: 1,
                    schema_digest: Some(digest('e')),
                }],
                binding_dependencies: vec![BindingDependency {
                    kind: "character_name_legacy".into(),
                    id: "marisa".into(),
                    digest: digest('f'),
                }],
                resolved_realization: ResolvedRealization::NativeVoice {
                    character_name: "marisa".into(),
                    character_binding_digest: digest('f'),
                },
            }],
            warnings: Vec::new(),
        };
        let baseline = plan.canonical_digest().unwrap();
        plan.capability_digest.push_str("-changed");
        assert!(plan.canonical_digest().is_err());
        plan.capability_digest = digest('a');
        plan.cues[0].strategy = RealizationStrategy::PortableAudioCaption;
        assert!(plan.canonical_digest().is_err());
        plan.cues[0].strategy = RealizationStrategy::Ymm4NativeVoice;
        plan.cues[0].resolved_realization = ResolvedRealization::NativeVoice {
            character_name: "marisa".into(),
            character_binding_digest: digest('1'),
        };
        plan.cues[0].binding_dependencies[0].digest = digest('1');
        assert_ne!(baseline, plan.canonical_digest().unwrap());
    }

    #[test]
    fn canonical_json_sorts_object_keys() {
        let left = serde_json::json!({"z": 1, "a": {"d": 2, "b": 3}});
        let right: Value = serde_json::from_str(r#"{"a":{"b":3,"d":2},"z":1}"#).unwrap();
        assert_eq!(
            canonical_sha256("test", &left).unwrap(),
            canonical_sha256("test", &right).unwrap()
        );
    }

    #[test]
    fn approval_digests_match_optional_sha256_prefix() {
        let hex = "a".repeat(64);
        let prefixed = format!("sha256:{hex}");
        assert!(approval_digests_match(&prefixed, &hex));
        assert!(approval_digests_match(&hex, &prefixed));
        assert!(approval_digests_match(&prefixed, &prefixed));
        assert!(approval_digests_match(&hex, &hex.to_ascii_uppercase()));
        assert!(!approval_digests_match(&prefixed, &"b".repeat(64)));
        assert!(approval_digests_match("opaque-token", "opaque-token"));
        assert!(!approval_digests_match("opaque-token", "other-token"));
    }
}
