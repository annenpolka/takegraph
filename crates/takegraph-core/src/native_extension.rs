use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    CanonicalError, CapabilityDependency, ChangeBudget, ResolvedPlacement, RevisionId,
    ScopeFingerprints, TargetIdentity, canonical_sha256,
};

/// Canonical JSON contract version for native target-extension plans.
pub const NATIVE_EXTENSION_PLAN_CANONICAL_VERSION: u32 = 1;

/// A digest-pinned target descriptor. Display names are deliberately absent:
/// callers must resolve and explicitly bind a descriptor before planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescriptorReference {
    pub descriptor_id: String,
    pub expected_digest: String,
}

impl DescriptorReference {
    fn validate(&self) -> Result<(), NativeExtensionError> {
        require_non_empty(&self.descriptor_id, "descriptorId")?;
        require_sha256(&self.expected_digest, "expectedDigest")
    }
}

/// Immutable artifact reference used by media clips.
///
/// It contains content identity and media metadata, never a host path. The
/// target adapter is responsible for materializing the artifact beneath an
/// authorized staging root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmutableAssetReference {
    pub artifact_digest: String,
    pub media_type: String,
    pub byte_length: u64,
    pub kind: AssetKind,
}

impl ImmutableAssetReference {
    /// Validates content identity and the kind/media-type relationship.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-SHA-256 identity, empty artifact, or media
    /// type that does not match the declared semantic kind.
    pub fn validate(&self) -> Result<(), NativeExtensionError> {
        require_sha256(&self.artifact_digest, "artifactDigest")?;
        require_non_empty(&self.media_type, "mediaType")?;
        if self.byte_length == 0 {
            return Err(NativeExtensionError::EmptyArtifact);
        }
        let expected_prefix = match self.kind {
            AssetKind::Image => "image/",
            AssetKind::Video => "video/",
            AssetKind::Audio | AssetKind::Bgm => "audio/",
        };
        if !self.media_type.starts_with(expected_prefix) {
            return Err(NativeExtensionError::MediaTypeMismatch {
                kind: self.kind,
                media_type: self.media_type.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Video,
    Audio,
    Bgm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortraitPresentation {
    Portrait,
    Face,
}

/// Portable request for a character-bound portrait or face realization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortraitIntent {
    pub entity_id: String,
    pub entity_revision: u64,
    pub presentation: PortraitPresentation,
    pub character_binding: DescriptorReference,
    pub placement: ResolvedPlacement,
    pub duration_frames: u32,
    pub replacement_guard: ReplacementGuard,
}

/// Portable request for an immutable image, video, audio, or BGM clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetClipIntent {
    pub entity_id: String,
    pub entity_revision: u64,
    pub asset: ImmutableAssetReference,
    pub placement: ResolvedPlacement,
    pub duration_frames: u32,
    pub loop_playback: bool,
    pub replacement_guard: ReplacementGuard,
}

/// A typed, TakeGraph-owned effect instance on a managed target entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEffectIntent {
    pub target_entity_id: String,
    pub target_entity_revision: u64,
    pub effect_instance_id: String,
    pub descriptor: DescriptorReference,
    pub operation: EffectOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectOperation {
    Upsert {
        parameters: BTreeMap<String, EffectParameterValue>,
    },
    Remove,
}

/// A digest-pinned native template instantiation. Target templates are opaque
/// target assets; portable `TakeGraph` templates expand before this layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeTemplateIntent {
    pub entity_id: String,
    pub entity_revision: u64,
    pub template: DescriptorReference,
    pub placement: ResolvedPlacement,
}

/// Approval input for one Phase-4 native extension operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "intent", rename_all = "snake_case")]
pub enum NativeExtensionIntent {
    UpsertPortrait(PortraitIntent),
    UpsertAsset(AssetClipIntent),
    MutateEffect(ManagedEffectIntent),
    InstantiateTemplate(NativeTemplateIntent),
}

impl NativeExtensionIntent {
    #[must_use]
    pub fn logical_key(&self) -> String {
        match self {
            Self::UpsertPortrait(intent) => format!("portrait:{}", intent.entity_id),
            Self::UpsertAsset(intent) => format!("asset:{}", intent.entity_id),
            Self::MutateEffect(intent) => format!(
                "effect:{}:{}",
                intent.target_entity_id, intent.effect_instance_id
            ),
            Self::InstantiateTemplate(intent) => format!("template:{}", intent.entity_id),
        }
    }

    /// Validates the target-independent portion of an intent.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid identity, duration, asset identity, or
    /// descriptor references. Effect parameters are checked against the
    /// resolved descriptor by the service planner.
    pub fn validate(&self) -> Result<(), NativeExtensionError> {
        match self {
            Self::UpsertPortrait(intent) => {
                require_non_empty(&intent.entity_id, "entityId")?;
                intent.character_binding.validate()?;
                validate_placement(&intent.placement)?;
                require_duration(intent.duration_frames)?;
                intent.replacement_guard.validate()
            }
            Self::UpsertAsset(intent) => {
                require_non_empty(&intent.entity_id, "entityId")?;
                intent.asset.validate()?;
                validate_placement(&intent.placement)?;
                require_duration(intent.duration_frames)?;
                intent.replacement_guard.validate()
            }
            Self::MutateEffect(intent) => {
                require_non_empty(&intent.target_entity_id, "targetEntityId")?;
                require_non_empty(&intent.effect_instance_id, "effectInstanceId")?;
                intent.descriptor.validate()
            }
            Self::InstantiateTemplate(intent) => {
                require_non_empty(&intent.entity_id, "entityId")?;
                validate_placement(&intent.placement)?;
                intent.template.validate()
            }
        }
    }
}

/// Exact lossy fields a caller is willing to approve for a replacement.
///
/// The planner compares this set with the driver assessment. A blanket
/// `allowLossy` boolean would hide newly discovered loss, so it is not used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplacementGuard {
    pub approved_lossy_fields: Vec<String>,
}

impl ReplacementGuard {
    fn validate(&self) -> Result<(), NativeExtensionError> {
        require_unique_non_empty(&self.approved_lossy_fields, "approvedLossyFields")
    }
}

/// Stable character descriptor returned by a target observer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterDescriptor {
    pub descriptor_id: String,
    pub display_name: String,
    pub supported_presentations: BTreeSet<PortraitPresentation>,
    pub configuration: BTreeMap<String, String>,
}

impl CharacterDescriptor {
    /// Computes the complete configuration digest pinned by plans.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_digest(&self) -> Result<String, CanonicalError> {
        canonical_sha256("takegraph-character-descriptor-v1", self)
    }
}

/// Stable native template descriptor. `contentDigest` binds the source target
/// asset in addition to normalized descriptor fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateDescriptor {
    pub descriptor_id: String,
    pub display_name: String,
    pub content_digest: String,
    pub produced_item_kinds: BTreeSet<String>,
}

impl TemplateDescriptor {
    /// Computes the descriptor and native-template content digest.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_digest(&self) -> Result<String, CanonicalError> {
        canonical_sha256("takegraph-template-descriptor-v1", self)
    }
}

/// One allowlisted parameter in a typed target effect schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectParameterSpec {
    pub required: bool,
    #[serde(flatten)]
    pub schema: EffectParameterSchema,
}

/// Integer/fixed-point types avoid non-portable JSON floating-point behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectParameterSchema {
    Boolean,
    Integer {
        min: i64,
        max: i64,
    },
    Fixed {
        scale: u32,
        min_scaled: i64,
        max_scaled: i64,
    },
    Text {
        max_length: usize,
    },
    Choice {
        values: Vec<String>,
    },
    ColorRgba,
}

/// Concrete JSON value for an allowlisted effect parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum EffectParameterValue {
    Boolean(bool),
    Integer(i64),
    Fixed { scale: u32, scaled: i64 },
    Text(String),
    Choice(String),
    ColorRgba([u8; 4]),
}

/// A stable target effect type plus its complete allowlisted parameter schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectDescriptor {
    pub descriptor_id: String,
    pub stable_type_id: String,
    pub display_name: String,
    pub schema_version: u32,
    pub parameters: BTreeMap<String, EffectParameterSpec>,
}

impl EffectDescriptor {
    /// Computes a digest over the stable type and complete typed schema.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_digest(&self) -> Result<String, CanonicalError> {
        canonical_sha256("takegraph-effect-descriptor-v1", self)
    }

    /// Checks that parameters have no unknown keys, contain every required
    /// key, and match the pinned schema exactly.
    ///
    /// # Errors
    ///
    /// Returns an error on missing, unknown, wrong-type, or out-of-range
    /// parameters.
    pub fn validate_parameters(
        &self,
        values: &BTreeMap<String, EffectParameterValue>,
    ) -> Result<(), NativeExtensionError> {
        for name in values.keys() {
            if !self.parameters.contains_key(name) {
                return Err(NativeExtensionError::UnknownEffectParameter {
                    descriptor_id: self.descriptor_id.clone(),
                    parameter: name.clone(),
                });
            }
        }
        for (name, spec) in &self.parameters {
            let Some(value) = values.get(name) else {
                if spec.required {
                    return Err(NativeExtensionError::MissingEffectParameter {
                        descriptor_id: self.descriptor_id.clone(),
                        parameter: name.clone(),
                    });
                }
                continue;
            };
            if !spec.schema.accepts(value) {
                return Err(NativeExtensionError::InvalidEffectParameter {
                    descriptor_id: self.descriptor_id.clone(),
                    parameter: name.clone(),
                });
            }
        }
        Ok(())
    }
}

impl EffectParameterSchema {
    fn accepts(&self, value: &EffectParameterValue) -> bool {
        match (self, value) {
            (Self::Boolean, EffectParameterValue::Boolean(_))
            | (Self::ColorRgba, EffectParameterValue::ColorRgba(_)) => true,
            (Self::Integer { min, max }, EffectParameterValue::Integer(value)) => {
                min <= value && value <= max
            }
            (
                Self::Fixed {
                    scale,
                    min_scaled,
                    max_scaled,
                },
                EffectParameterValue::Fixed {
                    scale: value_scale,
                    scaled,
                },
            ) => scale == value_scale && min_scaled <= scaled && scaled <= max_scaled,
            (Self::Text { max_length }, EffectParameterValue::Text(value)) => {
                value.chars().count() <= *max_length
            }
            (Self::Choice { values }, EffectParameterValue::Choice(value)) => {
                values.contains(value)
            }
            _ => false,
        }
    }
}

/// Complete read-only descriptor snapshot used while staging a plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeDescriptorCatalog {
    pub characters: BTreeMap<String, CharacterDescriptor>,
    pub templates: BTreeMap<String, TemplateDescriptor>,
    pub effects: BTreeMap<String, EffectDescriptor>,
}

impl NativeDescriptorCatalog {
    /// Computes a digest over the complete descriptor snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_digest(&self) -> Result<String, CanonicalError> {
        canonical_sha256("takegraph-native-descriptor-catalog-v1", self)
    }

    /// Validates map keys, stable IDs, descriptor content hashes, and schemas.
    ///
    /// # Errors
    ///
    /// Returns an error when a descriptor is malformed or stored under a key
    /// other than its stable ID.
    pub fn validate(&self) -> Result<(), NativeExtensionError> {
        for (key, descriptor) in &self.characters {
            if key != &descriptor.descriptor_id {
                return Err(NativeExtensionError::DescriptorKeyMismatch {
                    key: key.clone(),
                    descriptor_id: descriptor.descriptor_id.clone(),
                });
            }
            require_non_empty(key, "character descriptorId")?;
            require_non_empty(&descriptor.display_name, "character displayName")?;
            for (name, value) in &descriptor.configuration {
                require_non_empty(name, "character configuration key")?;
                require_non_empty(value, "character configuration value")?;
            }
            if descriptor.supported_presentations.is_empty() {
                return Err(NativeExtensionError::EmptyDescriptorCapability(key.clone()));
            }
        }
        for (key, descriptor) in &self.templates {
            if key != &descriptor.descriptor_id {
                return Err(NativeExtensionError::DescriptorKeyMismatch {
                    key: key.clone(),
                    descriptor_id: descriptor.descriptor_id.clone(),
                });
            }
            require_non_empty(key, "template descriptorId")?;
            require_non_empty(&descriptor.display_name, "template displayName")?;
            require_sha256(&descriptor.content_digest, "template contentDigest")?;
            if descriptor.produced_item_kinds.is_empty() {
                return Err(NativeExtensionError::EmptyDescriptorCapability(key.clone()));
            }
            for item_kind in &descriptor.produced_item_kinds {
                require_non_empty(item_kind, "template produced item kind")?;
            }
        }
        for (key, descriptor) in &self.effects {
            if key != &descriptor.descriptor_id {
                return Err(NativeExtensionError::DescriptorKeyMismatch {
                    key: key.clone(),
                    descriptor_id: descriptor.descriptor_id.clone(),
                });
            }
            require_non_empty(key, "effect descriptorId")?;
            require_non_empty(&descriptor.stable_type_id, "effect stableTypeId")?;
            require_non_empty(&descriptor.display_name, "effect displayName")?;
            if descriptor.schema_version == 0 {
                return Err(NativeExtensionError::InvalidEffectSchema(key.clone()));
            }
            for (name, spec) in &descriptor.parameters {
                require_non_empty(name, "effect parameter name")?;
                match &spec.schema {
                    EffectParameterSchema::Integer { min, max } if min > max => {
                        return Err(NativeExtensionError::InvalidEffectSchema(key.clone()));
                    }
                    EffectParameterSchema::Fixed {
                        scale,
                        min_scaled,
                        max_scaled,
                    } if *scale == 0 || min_scaled > max_scaled => {
                        return Err(NativeExtensionError::InvalidEffectSchema(key.clone()));
                    }
                    EffectParameterSchema::Choice { values }
                        if values.is_empty()
                            || values.iter().any(String::is_empty)
                            || values.iter().collect::<BTreeSet<_>>().len() != values.len() =>
                    {
                        return Err(NativeExtensionError::InvalidEffectSchema(key.clone()));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// Opaque evidence for an effect the target owns and `TakeGraph` must preserve.
/// Parameter contents are intentionally unavailable to mutation planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpaqueNativeEffect {
    pub stable_type_id: String,
    pub instance_key: String,
    pub state_digest: String,
}

/// A host-local field claimed by neither portable semantics nor `TakeGraph`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreservedNativeField {
    pub field: String,
    pub state_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeMutationMode {
    Create,
    InPlace,
    Replace,
}

/// Approval-bound preservation contract derived from target observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreservationPlan {
    pub mode: NativeMutationMode,
    pub preserved_fields: Vec<PreservedNativeField>,
    pub unknown_effects: Vec<OpaqueNativeEffect>,
    pub lossy_fields: Vec<String>,
    pub approved_lossy_fields: Vec<String>,
}

impl PreservationPlan {
    #[must_use]
    pub const fn create() -> Self {
        Self {
            mode: NativeMutationMode::Create,
            preserved_fields: Vec::new(),
            unknown_effects: Vec::new(),
            lossy_fields: Vec::new(),
            approved_lossy_fields: Vec::new(),
        }
    }

    /// Validates exact lossy approval and preservation evidence.
    ///
    /// # Errors
    ///
    /// Returns an error if replacement loss was not approved exactly or
    /// preservation evidence is ambiguous.
    pub fn validate(&self) -> Result<(), NativeExtensionError> {
        require_unique_non_empty(&self.lossy_fields, "lossyFields")?;
        require_unique_non_empty(&self.approved_lossy_fields, "approvedLossyFields")?;
        if normalized_strings(&self.lossy_fields) != normalized_strings(&self.approved_lossy_fields)
        {
            return Err(NativeExtensionError::LossyReplacementNotApproved {
                required: normalized_strings(&self.lossy_fields),
                approved: normalized_strings(&self.approved_lossy_fields),
            });
        }
        if self.mode != NativeMutationMode::Replace && !self.lossy_fields.is_empty() {
            return Err(NativeExtensionError::LossOutsideReplacement);
        }

        let mut fields = BTreeSet::new();
        for field in &self.preserved_fields {
            require_non_empty(&field.field, "preserved field")?;
            require_sha256(&field.state_digest, "preserved field digest")?;
            if !fields.insert(&field.field) {
                return Err(NativeExtensionError::DuplicatePreservationIdentity(
                    field.field.clone(),
                ));
            }
        }
        let mut effects = BTreeSet::new();
        for effect in &self.unknown_effects {
            require_non_empty(&effect.stable_type_id, "unknown effect stableTypeId")?;
            require_non_empty(&effect.instance_key, "unknown effect instanceKey")?;
            require_sha256(&effect.state_digest, "unknown effect stateDigest")?;
            if !effects.insert((&effect.stable_type_id, &effect.instance_key)) {
                return Err(NativeExtensionError::DuplicatePreservationIdentity(
                    effect.instance_key.clone(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeExtensionAction {
    Create,
    Update,
    Delete,
    Instantiate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescriptorDependency {
    pub kind: DescriptorKind,
    pub descriptor_id: String,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptorKind {
    Character,
    Effect,
    Template,
}

/// One fully resolved target operation. All target-local state that must
/// survive is represented in `preservation` and participates in approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedNativeExtension {
    pub realization_id: Uuid,
    pub action: NativeExtensionAction,
    pub intent: NativeExtensionIntent,
    pub capability_dependencies: Vec<CapabilityDependency>,
    pub descriptor_dependencies: Vec<DescriptorDependency>,
    pub preservation: PreservationPlan,
}

/// Digest-approved plan for portraits, assets, effects, and native templates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeExtensionPlan {
    pub canonical_version: u32,
    pub operation_id: Uuid,
    pub base_revision: RevisionId,
    pub target: TargetIdentity,
    pub capability_digest: String,
    pub descriptor_catalog_digest: String,
    pub expected_scope: ScopeFingerprints,
    pub change_budget: ChangeBudget,
    pub operations: Vec<PlannedNativeExtension>,
    pub warnings: Vec<String>,
}

impl NativeExtensionPlan {
    /// Computes the canonical, approval-bound digest.
    ///
    /// # Errors
    ///
    /// Returns an error if the plan is invalid or cannot be serialized.
    pub fn canonical_digest(&self) -> Result<String, NativeExtensionError> {
        self.validate()?;
        Ok(canonical_sha256(
            "takegraph-native-extension-plan-v1",
            self,
        )?)
    }

    /// Checks portable plan invariants before approval or apply.
    ///
    /// # Errors
    ///
    /// Returns an error when version, target, budget, identity,
    /// preservation, or dependency invariants are violated.
    pub fn validate(&self) -> Result<(), NativeExtensionError> {
        if self.canonical_version != NATIVE_EXTENSION_PLAN_CANONICAL_VERSION {
            return Err(NativeExtensionError::UnsupportedCanonicalVersion(
                self.canonical_version,
            ));
        }
        require_non_empty(&self.target.adapter_id, "target.adapterId")?;
        require_non_empty(&self.target.project_id, "target.projectId")?;
        require_non_empty(&self.target.scene_id, "target.sceneId")?;
        require_non_empty(&self.target.driver_version, "target.driverVersion")?;
        if self.target.fps == 0 {
            return Err(NativeExtensionError::InvalidTargetFps);
        }
        require_sha256(&self.capability_digest, "capabilityDigest")?;
        require_sha256(&self.descriptor_catalog_digest, "descriptorCatalogDigest")?;
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
        if self.change_budget.allow_unmanaged_changes {
            return Err(NativeExtensionError::UnmanagedChangesForbidden);
        }
        if self.operations.len() > self.change_budget.max_changed_entities {
            return Err(NativeExtensionError::ChangeBudgetExceeded);
        }
        let mut keys = BTreeSet::new();
        let mut realizations = BTreeSet::new();
        for operation in &self.operations {
            operation.intent.validate()?;
            operation.preservation.validate()?;
            let key = operation.intent.logical_key();
            if !keys.insert(key.clone()) {
                return Err(NativeExtensionError::DuplicateOperation(key));
            }
            if !realizations.insert(operation.realization_id) {
                return Err(NativeExtensionError::DuplicateRealization(
                    operation.realization_id,
                ));
            }
            if operation.capability_dependencies.is_empty() {
                return Err(NativeExtensionError::MissingCapabilityDependency(key));
            }
            for dependency in &operation.capability_dependencies {
                require_non_empty(&dependency.feature, "capability feature")?;
                if dependency.minimum_version == 0 {
                    return Err(NativeExtensionError::InvalidCapabilityDependency(
                        dependency.feature.clone(),
                    ));
                }
                let schema_digest = dependency.schema_digest.as_ref().ok_or_else(|| {
                    NativeExtensionError::InvalidCapabilityDependency(dependency.feature.clone())
                })?;
                require_sha256(schema_digest, "capability schema digest")?;
            }
            for dependency in &operation.descriptor_dependencies {
                require_non_empty(&dependency.descriptor_id, "descriptor dependency id")?;
                require_sha256(&dependency.digest, "descriptor dependency digest")?;
            }
        }
        Ok(())
    }
}

fn require_duration(value: u32) -> Result<(), NativeExtensionError> {
    if value == 0 {
        Err(NativeExtensionError::InvalidDuration)
    } else {
        Ok(())
    }
}

fn validate_placement(placement: &ResolvedPlacement) -> Result<(), NativeExtensionError> {
    if placement.frame < 0 || placement.primary_layer < 0 || placement.secondary_layer.is_some() {
        Err(NativeExtensionError::InvalidPlacement)
    } else {
        Ok(())
    }
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), NativeExtensionError> {
    if value.trim().is_empty() {
        Err(NativeExtensionError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn require_sha256(value: &str, field: &'static str) -> Result<(), NativeExtensionError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(NativeExtensionError::InvalidDigest(field));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(NativeExtensionError::InvalidDigest(field));
    }
    Ok(())
}

fn require_unique_non_empty(
    values: &[String],
    field: &'static str,
) -> Result<(), NativeExtensionError> {
    let mut unique = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            return Err(NativeExtensionError::EmptyField(field));
        }
        if !unique.insert(value) {
            return Err(NativeExtensionError::DuplicateListValue {
                field,
                value: value.clone(),
            });
        }
    }
    Ok(())
}

fn normalized_strings(values: &[String]) -> Vec<String> {
    let mut normalized = values.to_vec();
    normalized.sort();
    normalized
}

#[derive(Debug, Error)]
pub enum NativeExtensionError {
    #[error("required field is empty: {0}")]
    EmptyField(&'static str),
    #[error("field is not a canonical SHA-256 digest: {0}")]
    InvalidDigest(&'static str),
    #[error("immutable artifact cannot be empty")]
    EmptyArtifact,
    #[error("media type {media_type} does not match {kind:?}")]
    MediaTypeMismatch { kind: AssetKind, media_type: String },
    #[error("duration must be a positive frame count")]
    InvalidDuration,
    #[error(
        "native extension placement requires a non-negative frame/primary layer and no secondary layer"
    )]
    InvalidPlacement,
    #[error("target FPS must be positive")]
    InvalidTargetFps,
    #[error("descriptor map key {key} differs from descriptor ID {descriptor_id}")]
    DescriptorKeyMismatch { key: String, descriptor_id: String },
    #[error("descriptor exposes no usable capability: {0}")]
    EmptyDescriptorCapability(String),
    #[error("invalid typed effect schema: {0}")]
    InvalidEffectSchema(String),
    #[error("effect {descriptor_id} does not allow parameter {parameter}")]
    UnknownEffectParameter {
        descriptor_id: String,
        parameter: String,
    },
    #[error("effect {descriptor_id} requires parameter {parameter}")]
    MissingEffectParameter {
        descriptor_id: String,
        parameter: String,
    },
    #[error("effect {descriptor_id} parameter {parameter} has the wrong type or range")]
    InvalidEffectParameter {
        descriptor_id: String,
        parameter: String,
    },
    #[error(
        "lossy replacement fields were not approved exactly; required {required:?}, approved {approved:?}"
    )]
    LossyReplacementNotApproved {
        required: Vec<String>,
        approved: Vec<String>,
    },
    #[error("lossy fields are only valid for replacement mode")]
    LossOutsideReplacement,
    #[error("duplicate preservation identity: {0}")]
    DuplicatePreservationIdentity(String),
    #[error("duplicate value {value} in {field}")]
    DuplicateListValue { field: &'static str, value: String },
    #[error("unsupported native-extension canonical version: {0}")]
    UnsupportedCanonicalVersion(u32),
    #[error("native-extension plan exceeds its change budget")]
    ChangeBudgetExceeded,
    #[error("duplicate native-extension operation: {0}")]
    DuplicateOperation(String),
    #[error("duplicate native realization ID: {0}")]
    DuplicateRealization(Uuid),
    #[error("operation has no capability dependency: {0}")]
    MissingCapabilityDependency(String),
    #[error("capability dependency is missing a positive version or typed schema digest: {0}")]
    InvalidCapabilityDependency(String),
    #[error("native-extension plans never authorize unmanaged changes")]
    UnmanagedChangesForbidden,
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn effect_descriptor() -> EffectDescriptor {
        EffectDescriptor {
            descriptor_id: "effect.opacity".into(),
            stable_type_id: "YMM4.VideoEffects.Opacity".into(),
            display_name: "Opacity".into(),
            schema_version: 1,
            parameters: BTreeMap::from([
                (
                    "enabled".into(),
                    EffectParameterSpec {
                        required: true,
                        schema: EffectParameterSchema::Boolean,
                    },
                ),
                (
                    "opacity".into(),
                    EffectParameterSpec {
                        required: true,
                        schema: EffectParameterSchema::Fixed {
                            scale: 1000,
                            min_scaled: 0,
                            max_scaled: 1000,
                        },
                    },
                ),
            ]),
        }
    }

    #[test]
    fn immutable_asset_json_has_no_host_path() {
        let asset = ImmutableAssetReference {
            artifact_digest: digest('a'),
            media_type: "audio/wav".into(),
            byte_length: 42,
            kind: AssetKind::Bgm,
        };
        asset.validate().unwrap();
        assert_eq!(
            serde_json::to_value(asset).unwrap(),
            serde_json::json!({
                "artifactDigest": digest('a'),
                "mediaType": "audio/wav",
                "byteLength": 42,
                "kind": "bgm"
            })
        );
    }

    #[test]
    fn typed_effect_rejects_unknown_wrong_scale_and_out_of_range_values() {
        let descriptor = effect_descriptor();
        let valid = BTreeMap::from([
            ("enabled".into(), EffectParameterValue::Boolean(true)),
            (
                "opacity".into(),
                EffectParameterValue::Fixed {
                    scale: 1000,
                    scaled: 750,
                },
            ),
        ]);
        descriptor.validate_parameters(&valid).unwrap();

        let mut unknown = valid.clone();
        unknown.insert(
            "reflectionProperty".into(),
            EffectParameterValue::Integer(1),
        );
        assert!(matches!(
            descriptor.validate_parameters(&unknown),
            Err(NativeExtensionError::UnknownEffectParameter { .. })
        ));

        let mut wrong_scale = valid.clone();
        wrong_scale.insert(
            "opacity".into(),
            EffectParameterValue::Fixed {
                scale: 100,
                scaled: 75,
            },
        );
        assert!(matches!(
            descriptor.validate_parameters(&wrong_scale),
            Err(NativeExtensionError::InvalidEffectParameter { .. })
        ));

        let mut outside = valid;
        outside.insert(
            "opacity".into(),
            EffectParameterValue::Fixed {
                scale: 1000,
                scaled: 1001,
            },
        );
        assert!(matches!(
            descriptor.validate_parameters(&outside),
            Err(NativeExtensionError::InvalidEffectParameter { .. })
        ));
    }

    #[test]
    fn replacement_requires_exact_field_level_loss_approval() {
        let unapproved = PreservationPlan {
            mode: NativeMutationMode::Replace,
            preserved_fields: Vec::new(),
            unknown_effects: Vec::new(),
            lossy_fields: vec!["nativeAnimation.keyframes".into()],
            approved_lossy_fields: Vec::new(),
        };
        assert!(matches!(
            unapproved.validate(),
            Err(NativeExtensionError::LossyReplacementNotApproved { .. })
        ));

        let approved = PreservationPlan {
            approved_lossy_fields: vec!["nativeAnimation.keyframes".into()],
            ..unapproved
        };
        approved.validate().unwrap();
    }
}
