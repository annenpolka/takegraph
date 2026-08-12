use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use takegraph_core::{
    AssetKind, CapabilityDependency, ChangeBudget, CharacterDescriptor, DescriptorDependency,
    DescriptorKind, DescriptorReference, EffectOperation, ManagedEffectIntent,
    NATIVE_EXTENSION_PLAN_CANONICAL_VERSION, NativeDescriptorCatalog, NativeExtensionAction,
    NativeExtensionError, NativeExtensionIntent, NativeExtensionPlan, NativeMutationMode,
    OpaqueNativeEffect, PlannedNativeExtension, PortraitPresentation, PreservationPlan,
    PreservedNativeField, ReplacementGuard, RevisionId, ScopeFingerprints, TargetIdentity,
};
use thiserror::Error;
use uuid::Uuid;

/// Target/scoping data supplied by the canonical service when staging Phase 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeExtensionPlanContext {
    pub operation_id: Uuid,
    pub base_revision: RevisionId,
    pub target: TargetIdentity,
    pub capability_digest: String,
    pub expected_scope: ScopeFingerprints,
    pub change_budget: ChangeBudget,
}

/// Pure planning view of one probed target feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeExtensionFeature {
    pub version: u32,
    pub available: bool,
    pub schema_digest: String,
}

/// Feature snapshot passed into the pure planner. The node adapter may derive
/// this from the structured YMM4 capabilities, but the planner has no I/O or
/// bridge dependency.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeExtensionCapabilities {
    pub features: BTreeMap<String, NativeExtensionFeature>,
}

/// Observed kind for an existing managed realization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExistingNativeExtensionKind {
    Portrait,
    Face,
    Image,
    Video,
    Audio,
    Bgm,
    ManagedEffect,
}

/// Driver assessment of the supported update path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExistingUpdateMode {
    InPlace,
    Replace { lossy_fields: Vec<String> },
}

/// Read-only preservation evidence for an existing target item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingNativeExtension {
    pub logical_key: String,
    pub realization_id: Uuid,
    pub kind: ExistingNativeExtensionKind,
    pub update_mode: ExistingUpdateMode,
    pub preserved_fields: Vec<PreservedNativeField>,
    pub unknown_effects: Vec<OpaqueNativeEffect>,
}

/// Snapshot keyed by the stable logical key produced by
/// [`NativeExtensionIntent::logical_key`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeExtensionObservation {
    pub existing: BTreeMap<String, ExistingNativeExtension>,
}

/// Resolves descriptor/capability dependencies, actions, target-local
/// preservation, and exact loss approval without performing any I/O.
///
/// # Errors
///
/// Returns an error when a feature or descriptor is missing/drifted, an effect
/// parameter violates its typed schema, observed identity is ambiguous, or a
/// replacement would lose a field that was not approved by exact name.
pub fn plan_native_extensions(
    context: NativeExtensionPlanContext,
    intents: Vec<NativeExtensionIntent>,
    descriptors: &NativeDescriptorCatalog,
    capabilities: &NativeExtensionCapabilities,
    observation: &NativeExtensionObservation,
) -> Result<NativeExtensionPlan, NativeExtensionPlanError> {
    plan_native_extensions_with_identity_overrides(
        context,
        intents,
        descriptors,
        capabilities,
        observation,
        &BTreeMap::new(),
    )
}

/// Plans extensions while preserving explicitly approved realization IDs for
/// reconciliation re-export. Ordinary staging calls the wrapper above with no
/// overrides. An override is accepted only for the exact logical key and may
/// never rebind an already observed realization.
///
/// # Errors
///
/// Returns an error for an invalid override, unavailable capability or
/// descriptor, unsafe preservation/loss contract, or invalid resulting plan.
pub fn plan_native_extensions_with_identity_overrides(
    context: NativeExtensionPlanContext,
    intents: Vec<NativeExtensionIntent>,
    descriptors: &NativeDescriptorCatalog,
    capabilities: &NativeExtensionCapabilities,
    observation: &NativeExtensionObservation,
    identity_overrides: &BTreeMap<String, Uuid>,
) -> Result<NativeExtensionPlan, NativeExtensionPlanError> {
    descriptors.validate()?;
    if context.change_budget.allow_unmanaged_changes {
        return Err(NativeExtensionPlanError::UnmanagedChangesForbidden);
    }

    validate_observation(observation)?;
    let transaction = require_feature(capabilities, "timeline.transaction")?;
    let mut operations = Vec::with_capacity(intents.len());
    let mut warnings = Vec::new();

    let intent_keys = intents
        .iter()
        .map(NativeExtensionIntent::logical_key)
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(key) = identity_overrides
        .keys()
        .find(|key| !intent_keys.contains(*key))
    {
        return Err(NativeExtensionPlanError::UnknownIdentityOverride(
            key.clone(),
        ));
    }
    if identity_overrides.values().any(Uuid::is_nil)
        || identity_overrides
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != identity_overrides.len()
    {
        return Err(NativeExtensionPlanError::InvalidIdentityOverrides);
    }

    for intent in intents {
        let logical_key = intent.logical_key();
        intent.validate()?;
        let operation = plan_one_extension(
            context.operation_id,
            intent,
            descriptors,
            capabilities,
            observation,
            transaction,
            identity_overrides.get(&logical_key).copied(),
        )?;
        if !operation.preservation.lossy_fields.is_empty() {
            warnings.push(format!(
                "{} replaces a native item and loses approved fields: {}",
                operation.intent.logical_key(),
                operation.preservation.lossy_fields.join(", ")
            ));
        }
        operations.push(operation);
    }

    let plan = NativeExtensionPlan {
        canonical_version: NATIVE_EXTENSION_PLAN_CANONICAL_VERSION,
        operation_id: context.operation_id,
        base_revision: context.base_revision,
        target: context.target,
        capability_digest: context.capability_digest,
        descriptor_catalog_digest: descriptors.canonical_digest()?,
        expected_scope: context.expected_scope,
        change_budget: context.change_budget,
        operations,
        warnings,
    };
    plan.validate()?;
    Ok(plan)
}

#[allow(clippy::too_many_lines)] // Each typed intent is resolved in one auditable dispatch.
fn plan_one_extension(
    operation_id: Uuid,
    intent: NativeExtensionIntent,
    descriptors: &NativeDescriptorCatalog,
    capabilities: &NativeExtensionCapabilities,
    observation: &NativeExtensionObservation,
    transaction: &NativeExtensionFeature,
    identity_override: Option<Uuid>,
) -> Result<PlannedNativeExtension, NativeExtensionPlanError> {
    let key = intent.logical_key();
    let existing = observation.existing.get(&key);
    let mut capability_dependencies = Vec::with_capacity(2);
    let mut descriptor_dependencies = Vec::with_capacity(1);

    let (action, preservation) = match &intent {
        NativeExtensionIntent::UpsertPortrait(portrait) => {
            let feature_name = match portrait.presentation {
                PortraitPresentation::Portrait => "portraitItem.upsert",
                PortraitPresentation::Face => "faceItem.upsert",
            };
            capability_dependencies.push(capability_dependency(
                feature_name,
                require_feature(capabilities, feature_name)?,
            ));
            let descriptor = resolve_character(
                descriptors,
                &portrait.character_binding,
                portrait.presentation,
            )?;
            descriptor_dependencies.push(descriptor_dependency(
                DescriptorKind::Character,
                &portrait.character_binding,
                descriptor.canonical_digest()?,
            ));
            if let Some(existing) = existing {
                ensure_existing_kind(
                    &key,
                    existing.kind,
                    match portrait.presentation {
                        PortraitPresentation::Portrait => ExistingNativeExtensionKind::Portrait,
                        PortraitPresentation::Face => ExistingNativeExtensionKind::Face,
                    },
                )?;
                (
                    NativeExtensionAction::Update,
                    preservation_for(existing, &portrait.replacement_guard)?,
                )
            } else {
                (NativeExtensionAction::Create, PreservationPlan::create())
            }
        }
        NativeExtensionIntent::UpsertAsset(asset) => {
            let (feature_name, expected_kind) = match asset.asset.kind {
                AssetKind::Image => ("imageItem.upsert", ExistingNativeExtensionKind::Image),
                AssetKind::Video => ("videoItem.upsert", ExistingNativeExtensionKind::Video),
                AssetKind::Audio => ("audioItem.upsert", ExistingNativeExtensionKind::Audio),
                AssetKind::Bgm => ("audioItem.upsert", ExistingNativeExtensionKind::Bgm),
            };
            capability_dependencies.push(capability_dependency(
                feature_name,
                require_feature(capabilities, feature_name)?,
            ));
            if let Some(existing) = existing {
                ensure_existing_kind(&key, existing.kind, expected_kind)?;
                (
                    NativeExtensionAction::Update,
                    preservation_for(existing, &asset.replacement_guard)?,
                )
            } else {
                (NativeExtensionAction::Create, PreservationPlan::create())
            }
        }
        NativeExtensionIntent::MutateEffect(effect) => {
            let feature_name = "effect.typedMutation";
            capability_dependencies.push(capability_dependency(
                feature_name,
                require_feature(capabilities, feature_name)?,
            ));
            let descriptor = resolve_effect(descriptors, effect)?;
            descriptor_dependencies.push(descriptor_dependency(
                DescriptorKind::Effect,
                &effect.descriptor,
                descriptor.canonical_digest()?,
            ));
            match &effect.operation {
                EffectOperation::Upsert { parameters } => {
                    descriptor.validate_parameters(parameters)?;
                    if let Some(existing) = existing {
                        ensure_existing_kind(
                            &key,
                            existing.kind,
                            ExistingNativeExtensionKind::ManagedEffect,
                        )?;
                        (
                            NativeExtensionAction::Update,
                            in_place_effect_preservation(existing)?,
                        )
                    } else {
                        (NativeExtensionAction::Create, PreservationPlan::create())
                    }
                }
                EffectOperation::Remove => {
                    let existing = existing.ok_or_else(|| {
                        NativeExtensionPlanError::MissingManagedEffect(key.clone())
                    })?;
                    ensure_existing_kind(
                        &key,
                        existing.kind,
                        ExistingNativeExtensionKind::ManagedEffect,
                    )?;
                    (
                        NativeExtensionAction::Delete,
                        in_place_effect_preservation(existing)?,
                    )
                }
            }
        }
        NativeExtensionIntent::InstantiateTemplate(template) => {
            let feature_name = "template.instantiate";
            capability_dependencies.push(capability_dependency(
                feature_name,
                require_feature(capabilities, feature_name)?,
            ));
            let descriptor = resolve_template(descriptors, &template.template)?;
            descriptor_dependencies.push(descriptor_dependency(
                DescriptorKind::Template,
                &template.template,
                descriptor.canonical_digest()?,
            ));
            if existing.is_some() {
                return Err(NativeExtensionPlanError::TemplateAlreadyInstantiated(key));
            }
            (
                NativeExtensionAction::Instantiate,
                PreservationPlan::create(),
            )
        }
    };

    capability_dependencies.push(capability_dependency("timeline.transaction", transaction));
    if let (Some(existing), Some(identity_override)) = (existing, identity_override)
        && existing.realization_id != identity_override
    {
        return Err(NativeExtensionPlanError::ExistingIdentityOverrideMismatch {
            logical_key: key,
            existing: existing.realization_id,
            requested: identity_override,
        });
    }
    let realization_id = existing.map_or_else(
        || identity_override.unwrap_or_else(|| deterministic_realization_id(operation_id, &key)),
        |existing| existing.realization_id,
    );
    Ok(PlannedNativeExtension {
        realization_id,
        action,
        intent,
        capability_dependencies,
        descriptor_dependencies,
        preservation,
    })
}

fn validate_observation(
    observation: &NativeExtensionObservation,
) -> Result<(), NativeExtensionPlanError> {
    for (key, existing) in &observation.existing {
        if key != &existing.logical_key {
            return Err(NativeExtensionPlanError::ObservationKeyMismatch {
                key: key.clone(),
                logical_key: existing.logical_key.clone(),
            });
        }
        let probe = PreservationPlan {
            mode: match existing.update_mode {
                ExistingUpdateMode::InPlace => NativeMutationMode::InPlace,
                ExistingUpdateMode::Replace { .. } => NativeMutationMode::Replace,
            },
            preserved_fields: normalized_fields(existing.preserved_fields.clone()),
            unknown_effects: normalized_effects(existing.unknown_effects.clone()),
            lossy_fields: match &existing.update_mode {
                ExistingUpdateMode::InPlace => Vec::new(),
                ExistingUpdateMode::Replace { lossy_fields } => lossy_fields.clone(),
            },
            approved_lossy_fields: match &existing.update_mode {
                ExistingUpdateMode::InPlace => Vec::new(),
                ExistingUpdateMode::Replace { lossy_fields } => lossy_fields.clone(),
            },
        };
        probe.validate()?;
    }
    Ok(())
}

fn resolve_character<'a>(
    descriptors: &'a NativeDescriptorCatalog,
    reference: &DescriptorReference,
    presentation: PortraitPresentation,
) -> Result<&'a CharacterDescriptor, NativeExtensionPlanError> {
    let descriptor = descriptors
        .characters
        .get(&reference.descriptor_id)
        .ok_or_else(|| NativeExtensionPlanError::MissingDescriptor {
            kind: DescriptorKind::Character,
            descriptor_id: reference.descriptor_id.clone(),
        })?;
    require_descriptor_digest(reference, descriptor.canonical_digest()?)?;
    if !descriptor.supported_presentations.contains(&presentation) {
        return Err(NativeExtensionPlanError::UnsupportedPortraitPresentation {
            descriptor_id: reference.descriptor_id.clone(),
            presentation,
        });
    }
    Ok(descriptor)
}

fn resolve_effect<'a>(
    descriptors: &'a NativeDescriptorCatalog,
    intent: &ManagedEffectIntent,
) -> Result<&'a takegraph_core::EffectDescriptor, NativeExtensionPlanError> {
    let descriptor = descriptors
        .effects
        .get(&intent.descriptor.descriptor_id)
        .ok_or_else(|| NativeExtensionPlanError::MissingDescriptor {
            kind: DescriptorKind::Effect,
            descriptor_id: intent.descriptor.descriptor_id.clone(),
        })?;
    require_descriptor_digest(&intent.descriptor, descriptor.canonical_digest()?)?;
    Ok(descriptor)
}

fn resolve_template<'a>(
    descriptors: &'a NativeDescriptorCatalog,
    reference: &DescriptorReference,
) -> Result<&'a takegraph_core::TemplateDescriptor, NativeExtensionPlanError> {
    let descriptor = descriptors
        .templates
        .get(&reference.descriptor_id)
        .ok_or_else(|| NativeExtensionPlanError::MissingDescriptor {
            kind: DescriptorKind::Template,
            descriptor_id: reference.descriptor_id.clone(),
        })?;
    require_descriptor_digest(reference, descriptor.canonical_digest()?)?;
    Ok(descriptor)
}

fn require_descriptor_digest(
    reference: &DescriptorReference,
    actual: String,
) -> Result<(), NativeExtensionPlanError> {
    if reference.expected_digest != actual {
        return Err(NativeExtensionPlanError::DescriptorDrift {
            descriptor_id: reference.descriptor_id.clone(),
            expected: reference.expected_digest.clone(),
            actual,
        });
    }
    Ok(())
}

fn descriptor_dependency(
    kind: DescriptorKind,
    reference: &DescriptorReference,
    actual_digest: String,
) -> DescriptorDependency {
    debug_assert_eq!(reference.expected_digest, actual_digest);
    DescriptorDependency {
        kind,
        descriptor_id: reference.descriptor_id.clone(),
        digest: actual_digest,
    }
}

fn require_feature<'a>(
    capabilities: &'a NativeExtensionCapabilities,
    feature: &str,
) -> Result<&'a NativeExtensionFeature, NativeExtensionPlanError> {
    capabilities
        .features
        .get(feature)
        .filter(|descriptor| descriptor.available && descriptor.version > 0)
        .ok_or_else(|| NativeExtensionPlanError::MissingCapability(feature.into()))
}

fn capability_dependency(
    feature: &str,
    descriptor: &NativeExtensionFeature,
) -> CapabilityDependency {
    CapabilityDependency {
        feature: feature.into(),
        minimum_version: descriptor.version,
        schema_digest: Some(descriptor.schema_digest.clone()),
    }
}

fn preservation_for(
    existing: &ExistingNativeExtension,
    guard: &ReplacementGuard,
) -> Result<PreservationPlan, NativeExtensionPlanError> {
    let (mode, lossy_fields) = match &existing.update_mode {
        ExistingUpdateMode::InPlace => (NativeMutationMode::InPlace, Vec::new()),
        ExistingUpdateMode::Replace { lossy_fields } => (
            NativeMutationMode::Replace,
            normalized_strings(lossy_fields),
        ),
    };
    let preservation = PreservationPlan {
        mode,
        preserved_fields: normalized_fields(existing.preserved_fields.clone()),
        unknown_effects: normalized_effects(existing.unknown_effects.clone()),
        lossy_fields,
        approved_lossy_fields: normalized_strings(&guard.approved_lossy_fields),
    };
    preservation.validate()?;
    Ok(preservation)
}

fn in_place_effect_preservation(
    existing: &ExistingNativeExtension,
) -> Result<PreservationPlan, NativeExtensionPlanError> {
    if !matches!(existing.update_mode, ExistingUpdateMode::InPlace) {
        return Err(NativeExtensionPlanError::EffectWouldReplaceParent(
            existing.logical_key.clone(),
        ));
    }
    let preservation = PreservationPlan {
        mode: NativeMutationMode::InPlace,
        preserved_fields: normalized_fields(existing.preserved_fields.clone()),
        unknown_effects: normalized_effects(existing.unknown_effects.clone()),
        lossy_fields: Vec::new(),
        approved_lossy_fields: Vec::new(),
    };
    preservation.validate()?;
    Ok(preservation)
}

fn ensure_existing_kind(
    key: &str,
    actual: ExistingNativeExtensionKind,
    expected: ExistingNativeExtensionKind,
) -> Result<(), NativeExtensionPlanError> {
    if actual != expected {
        return Err(NativeExtensionPlanError::ExistingKindMismatch {
            logical_key: key.into(),
            expected,
            actual,
        });
    }
    Ok(())
}

fn normalized_fields(mut fields: Vec<PreservedNativeField>) -> Vec<PreservedNativeField> {
    fields.sort_by(|left, right| left.field.cmp(&right.field));
    fields
}

fn normalized_effects(mut effects: Vec<OpaqueNativeEffect>) -> Vec<OpaqueNativeEffect> {
    effects.sort_by(|left, right| {
        (&left.stable_type_id, &left.instance_key)
            .cmp(&(&right.stable_type_id, &right.instance_key))
    });
    effects
}

fn normalized_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values
}

fn deterministic_realization_id(operation_id: Uuid, logical_key: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"takegraph-native-extension-realization-v1\0");
    hasher.update(operation_id.as_bytes());
    hasher.update(logical_key.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[derive(Debug, Error)]
pub enum NativeExtensionPlanError {
    #[error("Phase-4 planning never authorizes unmanaged target changes")]
    UnmanagedChangesForbidden,
    #[error("required native-extension capability is unavailable: {0}")]
    MissingCapability(String),
    #[error("missing {kind:?} descriptor: {descriptor_id}")]
    MissingDescriptor {
        kind: DescriptorKind,
        descriptor_id: String,
    },
    #[error("descriptor {descriptor_id} drifted; expected {expected}, got {actual}")]
    DescriptorDrift {
        descriptor_id: String,
        expected: String,
        actual: String,
    },
    #[error("character descriptor {descriptor_id} does not support {presentation:?}")]
    UnsupportedPortraitPresentation {
        descriptor_id: String,
        presentation: PortraitPresentation,
    },
    #[error("observation key {key} differs from embedded logical key {logical_key}")]
    ObservationKeyMismatch { key: String, logical_key: String },
    #[error("existing realization {logical_key} has kind {actual:?}, expected {expected:?}")]
    ExistingKindMismatch {
        logical_key: String,
        expected: ExistingNativeExtensionKind,
        actual: ExistingNativeExtensionKind,
    },
    #[error("managed effect does not exist: {0}")]
    MissingManagedEffect(String),
    #[error("typed effect mutation would replace its parent item: {0}")]
    EffectWouldReplaceParent(String),
    #[error("native target template was already instantiated for: {0}")]
    TemplateAlreadyInstantiated(String),
    #[error("identity override refers to an unknown logical key: {0}")]
    UnknownIdentityOverride(String),
    #[error("identity overrides must contain unique, non-nil realization IDs")]
    InvalidIdentityOverrides,
    #[error(
        "identity override for {logical_key} would rebind existing realization {existing} to {requested}"
    )]
    ExistingIdentityOverrideMismatch {
        logical_key: String,
        existing: Uuid,
        requested: Uuid,
    },
    #[error(transparent)]
    Contract(#[from] NativeExtensionError),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use takegraph_core::{
        AssetClipIntent, CharacterDescriptor, DescriptorReference, EffectDescriptor,
        EffectParameterSchema, EffectParameterSpec, EffectParameterValue, ImmutableAssetReference,
        ManagedEffectIntent, NativeTemplateIntent, PortraitIntent, ResolvedPlacement,
        TemplateDescriptor,
    };

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn placement(layer: i32) -> ResolvedPlacement {
        ResolvedPlacement {
            frame: 120,
            primary_layer: layer,
            secondary_layer: None,
        }
    }

    fn descriptors() -> NativeDescriptorCatalog {
        NativeDescriptorCatalog {
            characters: BTreeMap::from([(
                "character.marisa".into(),
                CharacterDescriptor {
                    descriptor_id: "character.marisa".into(),
                    display_name: "魔理沙".into(),
                    supported_presentations: BTreeSet::from([
                        PortraitPresentation::Portrait,
                        PortraitPresentation::Face,
                    ]),
                    configuration: BTreeMap::from([("voice".into(), "marisa-v1".into())]),
                },
            )]),
            templates: BTreeMap::from([(
                "template.emphasis".into(),
                TemplateDescriptor {
                    descriptor_id: "template.emphasis".into(),
                    display_name: "強調".into(),
                    content_digest: digest('4'),
                    produced_item_kinds: BTreeSet::from(["text".into(), "shape".into()]),
                },
            )]),
            effects: BTreeMap::from([(
                "effect.opacity".into(),
                EffectDescriptor {
                    descriptor_id: "effect.opacity".into(),
                    stable_type_id: "YMM4.VideoEffects.Opacity".into(),
                    display_name: "Opacity".into(),
                    schema_version: 1,
                    parameters: BTreeMap::from([(
                        "opacity".into(),
                        EffectParameterSpec {
                            required: true,
                            schema: EffectParameterSchema::Fixed {
                                scale: 1000,
                                min_scaled: 0,
                                max_scaled: 1000,
                            },
                        },
                    )]),
                },
            )]),
        }
    }

    fn reference<T>(id: &str, descriptor: &T, domain: &str) -> DescriptorReference
    where
        T: Serialize,
    {
        DescriptorReference {
            descriptor_id: id.into(),
            expected_digest: takegraph_core::canonical_sha256(domain, descriptor).unwrap(),
        }
    }

    fn capabilities() -> NativeExtensionCapabilities {
        let names = [
            "portraitItem.upsert",
            "faceItem.upsert",
            "imageItem.upsert",
            "videoItem.upsert",
            "audioItem.upsert",
            "effect.typedMutation",
            "template.instantiate",
            "timeline.transaction",
        ];
        NativeExtensionCapabilities {
            features: names
                .into_iter()
                .map(|name| {
                    (
                        name.into(),
                        NativeExtensionFeature {
                            version: 1,
                            available: true,
                            schema_digest: takegraph_core::canonical_sha256(
                                "test-feature-schema",
                                name,
                            )
                            .unwrap(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn context(max_changed_entities: usize) -> NativeExtensionPlanContext {
        NativeExtensionPlanContext {
            operation_id: Uuid::nil(),
            base_revision: RevisionId(8),
            target: TargetIdentity {
                adapter_id: "ymm4-4.55".into(),
                project_id: "project-a".into(),
                scene_id: "scene-a".into(),
                fps: 60,
                driver_version: "4.55.1.1/0.3.0".into(),
            },
            capability_digest: digest('1'),
            expected_scope: ScopeFingerprints {
                target_identity_digest: digest('2'),
                managed_state_digest: digest('3'),
                conflict_scope_digest: digest('4'),
            },
            change_budget: ChangeBudget::create_only(max_changed_entities),
        }
    }

    fn phase4_intents(catalog: &NativeDescriptorCatalog) -> Vec<NativeExtensionIntent> {
        let character = &catalog.characters["character.marisa"];
        let effect = &catalog.effects["effect.opacity"];
        let template = &catalog.templates["template.emphasis"];
        vec![
            NativeExtensionIntent::UpsertPortrait(PortraitIntent {
                entity_id: "portrait-01".into(),
                entity_revision: 2,
                presentation: PortraitPresentation::Portrait,
                character_binding: reference(
                    "character.marisa",
                    character,
                    "takegraph-character-descriptor-v1",
                ),
                placement: placement(10),
                duration_frames: 180,
                replacement_guard: ReplacementGuard::default(),
            }),
            NativeExtensionIntent::UpsertAsset(AssetClipIntent {
                entity_id: "bgm-01".into(),
                entity_revision: 1,
                asset: ImmutableAssetReference {
                    artifact_digest: digest('b'),
                    media_type: "audio/wav".into(),
                    byte_length: 4096,
                    kind: AssetKind::Bgm,
                },
                placement: placement(30),
                duration_frames: 600,
                loop_playback: true,
                replacement_guard: ReplacementGuard::default(),
            }),
            NativeExtensionIntent::MutateEffect(ManagedEffectIntent {
                target_entity_id: "portrait-01".into(),
                target_entity_revision: 2,
                effect_instance_id: "managed-opacity-01".into(),
                descriptor: reference("effect.opacity", effect, "takegraph-effect-descriptor-v1"),
                operation: EffectOperation::Upsert {
                    parameters: BTreeMap::from([(
                        "opacity".into(),
                        EffectParameterValue::Fixed {
                            scale: 1000,
                            scaled: 850,
                        },
                    )]),
                },
            }),
            NativeExtensionIntent::InstantiateTemplate(NativeTemplateIntent {
                entity_id: "template-use-01".into(),
                entity_revision: 1,
                template: reference(
                    "template.emphasis",
                    template,
                    "takegraph-template-descriptor-v1",
                ),
                placement: placement(40),
            }),
        ]
    }

    fn observation_with_unknown_effect() -> NativeExtensionObservation {
        let key = "effect:portrait-01:managed-opacity-01".to_owned();
        NativeExtensionObservation {
            existing: BTreeMap::from([(
                key.clone(),
                ExistingNativeExtension {
                    logical_key: key,
                    realization_id: Uuid::from_u128(9),
                    kind: ExistingNativeExtensionKind::ManagedEffect,
                    update_mode: ExistingUpdateMode::InPlace,
                    preserved_fields: vec![PreservedNativeField {
                        field: "keyframes".into(),
                        state_digest: digest('5'),
                    }],
                    unknown_effects: vec![OpaqueNativeEffect {
                        stable_type_id: "user.custom.Glow".into(),
                        instance_key: "native-effect-7".into(),
                        state_digest: digest('6'),
                    }],
                },
            )]),
        }
    }

    #[test]
    fn phase4_plan_is_concrete_path_free_and_preserves_unknown_effects() {
        let catalog = descriptors();
        let plan = plan_native_extensions(
            context(4),
            phase4_intents(&catalog),
            &catalog,
            &capabilities(),
            &observation_with_unknown_effect(),
        )
        .unwrap();

        assert_eq!(plan.operations.len(), 4);
        let effect = &plan.operations[2];
        assert_eq!(effect.action, NativeExtensionAction::Update);
        assert_eq!(effect.preservation.unknown_effects.len(), 1);
        assert_eq!(
            effect.preservation.unknown_effects[0].stable_type_id,
            "user.custom.Glow"
        );

        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains(r#""type":"upsert_asset""#));
        assert!(json.contains(r#""kind":"bgm""#));
        assert!(
            json.contains(r#""type":"fixed","value":{"scale":1000,"scaled":850}}"#),
            "{json}"
        );
        assert!(!json.to_ascii_lowercase().contains("path"));

        // Cross-language consumers can pin this canonical DTO vector.
        assert_eq!(
            plan.canonical_digest().unwrap(),
            "sha256:e11030e0cf9aa59b91f6815093e59b7b98a79196daaef6f144d26a17d87d7f6e"
        );
    }

    #[test]
    fn unknown_effect_state_is_approval_bound() {
        let catalog = descriptors();
        let baseline = plan_native_extensions(
            context(4),
            phase4_intents(&catalog),
            &catalog,
            &capabilities(),
            &observation_with_unknown_effect(),
        )
        .unwrap()
        .canonical_digest()
        .unwrap();
        let mut changed = observation_with_unknown_effect();
        changed
            .existing
            .values_mut()
            .next()
            .unwrap()
            .unknown_effects[0]
            .state_digest = digest('7');
        let changed = plan_native_extensions(
            context(4),
            phase4_intents(&catalog),
            &catalog,
            &capabilities(),
            &changed,
        )
        .unwrap()
        .canonical_digest()
        .unwrap();
        assert_ne!(baseline, changed);
    }

    #[test]
    fn descriptor_drift_and_untyped_effect_parameters_fail_closed() {
        let catalog = descriptors();
        let mut intents = phase4_intents(&catalog);
        let NativeExtensionIntent::UpsertPortrait(portrait) = &mut intents[0] else {
            panic!("expected portrait intent");
        };
        portrait.character_binding.expected_digest = digest('f');
        assert!(matches!(
            plan_native_extensions(
                context(4),
                intents,
                &catalog,
                &capabilities(),
                &observation_with_unknown_effect(),
            ),
            Err(NativeExtensionPlanError::DescriptorDrift { .. })
        ));

        let mut intents = phase4_intents(&catalog);
        let NativeExtensionIntent::MutateEffect(effect) = &mut intents[2] else {
            panic!("expected effect intent");
        };
        let EffectOperation::Upsert { parameters } = &mut effect.operation else {
            panic!("expected effect upsert");
        };
        parameters.insert("arbitraryProperty".into(), EffectParameterValue::Integer(1));
        assert!(matches!(
            plan_native_extensions(
                context(4),
                intents,
                &catalog,
                &capabilities(),
                &observation_with_unknown_effect(),
            ),
            Err(NativeExtensionPlanError::Contract(
                NativeExtensionError::UnknownEffectParameter { .. }
            ))
        ));
    }

    #[test]
    fn replacement_is_blocked_until_exact_loss_is_approved() {
        let catalog = descriptors();
        let intent = phase4_intents(&catalog).remove(0);
        let key = intent.logical_key();
        let observation = NativeExtensionObservation {
            existing: BTreeMap::from([(
                key.clone(),
                ExistingNativeExtension {
                    logical_key: key,
                    realization_id: Uuid::from_u128(10),
                    kind: ExistingNativeExtensionKind::Portrait,
                    update_mode: ExistingUpdateMode::Replace {
                        lossy_fields: vec!["nativeAnimation.keyframes".into()],
                    },
                    preserved_fields: Vec::new(),
                    unknown_effects: Vec::new(),
                },
            )]),
        };
        assert!(matches!(
            plan_native_extensions(
                context(1),
                vec![intent.clone()],
                &catalog,
                &capabilities(),
                &observation,
            ),
            Err(NativeExtensionPlanError::Contract(
                NativeExtensionError::LossyReplacementNotApproved { .. }
            ))
        ));

        let NativeExtensionIntent::UpsertPortrait(mut approved) = intent else {
            panic!("expected portrait intent");
        };
        approved.replacement_guard.approved_lossy_fields = vec!["nativeAnimation.keyframes".into()];
        let plan = plan_native_extensions(
            context(1),
            vec![NativeExtensionIntent::UpsertPortrait(approved)],
            &catalog,
            &capabilities(),
            &observation,
        )
        .unwrap();
        assert_eq!(
            plan.operations[0].preservation.lossy_fields,
            vec!["nativeAnimation.keyframes"]
        );
        assert_eq!(plan.warnings.len(), 1);
    }

    #[test]
    fn reconciliation_identity_override_preserves_but_never_rebinds_realization() {
        let catalog = descriptors();
        let intent = phase4_intents(&catalog).remove(0);
        let key = intent.logical_key();
        let canonical_realization = Uuid::from_u128(42);
        let overrides = BTreeMap::from([(key.clone(), canonical_realization)]);
        let plan = plan_native_extensions_with_identity_overrides(
            context(1),
            vec![intent.clone()],
            &catalog,
            &capabilities(),
            &NativeExtensionObservation::default(),
            &overrides,
        )
        .unwrap();
        assert_eq!(plan.operations[0].realization_id, canonical_realization);

        let observed_realization = Uuid::from_u128(7);
        let observation = NativeExtensionObservation {
            existing: BTreeMap::from([(
                key.clone(),
                ExistingNativeExtension {
                    logical_key: key,
                    realization_id: observed_realization,
                    kind: ExistingNativeExtensionKind::Portrait,
                    update_mode: ExistingUpdateMode::InPlace,
                    preserved_fields: Vec::new(),
                    unknown_effects: Vec::new(),
                },
            )]),
        };
        assert!(matches!(
            plan_native_extensions_with_identity_overrides(
                context(1),
                vec![intent],
                &catalog,
                &capabilities(),
                &observation,
                &overrides,
            ),
            Err(NativeExtensionPlanError::ExistingIdentityOverrideMismatch {
                existing,
                requested,
                ..
            }) if existing == observed_realization && requested == canonical_realization
        ));
    }
}
