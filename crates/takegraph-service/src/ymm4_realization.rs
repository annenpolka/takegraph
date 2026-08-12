use std::collections::BTreeMap;

use takegraph_core::{
    DurationResolution, NativeRealization, PlannedAction, PlannedCue, RealizationStrategy,
    TargetPlan, canonical_sha256,
};
use takegraph_node::{ManagedItemKind, Ymm4ManagedItem};
use thiserror::Error;

/// Normalizes either a native `VoiceItem` or a portable audio/caption pair into
/// one semantic realization and verifies it against the sealed target plan.
///
/// # Errors
///
/// Returns an error for missing, duplicated, differently shaped, or
/// out-of-budget read-back state.
pub fn normalize_realizations(
    plan: &TargetPlan,
    items: &[Ymm4ManagedItem],
) -> Result<Vec<NativeRealization>, RealizationReadbackError> {
    plan.validate()
        .map_err(|error| RealizationReadbackError::InvalidPlan(error.to_string()))?;
    plan.cues
        .iter()
        .filter(|cue| cue.action != PlannedAction::Delete)
        .map(|cue| match cue.strategy {
            RealizationStrategy::Ymm4NativeVoice => normalize_native_voice(cue, items),
            RealizationStrategy::PortableAudioCaption => normalize_portable_pair(cue, items),
        })
        .collect()
}

fn normalize_native_voice(
    cue: &PlannedCue,
    items: &[Ymm4ManagedItem],
) -> Result<NativeRealization, RealizationReadbackError> {
    let matching = items
        .iter()
        .filter(|item| item.realization_id == Some(cue.realization_id))
        .collect::<Vec<_>>();
    let [item] = matching.as_slice() else {
        return Err(RealizationReadbackError::ItemCount {
            entity_id: cue.intent.entity_id.clone(),
            expected: 1,
            actual: matching.len(),
        });
    };
    let expected_speaker = cue
        .intent
        .voice_profile
        .as_deref()
        .unwrap_or(&cue.intent.speaker_role);
    let actual_length = validate_common_item(cue, item)?;
    if item.kind != ManagedItemKind::Voice
        || item.text.as_deref() != Some(cue.intent.display_text.as_str())
        || item.speaker.as_deref() != Some(expected_speaker)
    {
        return Err(RealizationReadbackError::SemanticMismatch(
            cue.intent.entity_id.clone(),
        ));
    }
    let owned_field_digest = canonical_sha256(
        "takegraph-ymm4-owned-fields",
        &serde_json::json!({
            "entityId": item.entity_id,
            "entityRevision": item.revision,
            "realizationId": item.realization_id,
            "kind": item.kind,
            "frame": item.frame,
            "layer": item.layer,
            "length": item.length,
            "text": item.text,
            "speaker": item.speaker,
        }),
    )?;
    Ok(NativeRealization {
        realization_id: cue.realization_id,
        entity_id: cue.intent.entity_id.clone(),
        entity_revision: cue.intent.entity_revision,
        strategy: cue.strategy,
        native_item_ids: Vec::new(),
        resolved_frame: item.frame,
        resolved_layers: vec![item.layer],
        actual_length,
        owned_field_digest,
        preserved_field_digest: None,
        host_bound: true,
        artifact_digests: BTreeMap::new(),
    })
}

fn normalize_portable_pair(
    cue: &PlannedCue,
    items: &[Ymm4ManagedItem],
) -> Result<NativeRealization, RealizationReadbackError> {
    let matching = items
        .iter()
        .filter(|item| item.entity_id == cue.intent.entity_id)
        .collect::<Vec<_>>();
    if matching.len() != 2 {
        return Err(RealizationReadbackError::ItemCount {
            entity_id: cue.intent.entity_id.clone(),
            expected: 2,
            actual: matching.len(),
        });
    }
    let audio = matching
        .iter()
        .find(|item| item.kind == ManagedItemKind::Audio)
        .copied();
    let caption = matching
        .iter()
        .find(|item| item.kind == ManagedItemKind::Caption)
        .copied();
    let (Some(audio), Some(caption)) = (audio, caption) else {
        return Err(RealizationReadbackError::SemanticMismatch(
            cue.intent.entity_id.clone(),
        ));
    };
    let actual_length = validate_common_item(cue, audio)?;
    let caption_length = validate_duration(cue, caption.length)?;
    let expected_caption_layer = cue.placement.secondary_layer.ok_or_else(|| {
        RealizationReadbackError::InvalidPlan(format!(
            "portable cue {} has no caption layer",
            cue.intent.entity_id
        ))
    })?;
    let expected_artifact = cue
        .binding_dependencies
        .iter()
        .find(|binding| binding.kind == "audio_artifact")
        .map(|binding| binding.id.as_str())
        .ok_or_else(|| {
            RealizationReadbackError::InvalidPlan(format!(
                "portable cue {} has no audio artifact binding",
                cue.intent.entity_id
            ))
        })?;
    if caption_length != actual_length
        || caption.revision != cue.intent.entity_revision
        || caption.frame != cue.placement.frame
        || caption.layer != expected_caption_layer
        || caption.text.as_deref() != Some(cue.intent.display_text.as_str())
        || audio.artifact_hash.as_deref() != Some(expected_artifact)
        || caption.artifact_hash.as_deref() != Some(expected_artifact)
    {
        return Err(RealizationReadbackError::SemanticMismatch(
            cue.intent.entity_id.clone(),
        ));
    }
    let owned_field_digest = canonical_sha256(
        "takegraph-ymm4-owned-fields",
        &serde_json::json!({"audio": audio, "caption": caption}),
    )?;
    let mut artifact_digests = BTreeMap::new();
    artifact_digests.insert("audio".into(), format!("sha256:{expected_artifact}"));
    Ok(NativeRealization {
        realization_id: cue.realization_id,
        entity_id: cue.intent.entity_id.clone(),
        entity_revision: cue.intent.entity_revision,
        strategy: cue.strategy,
        native_item_ids: Vec::new(),
        resolved_frame: audio.frame,
        resolved_layers: vec![audio.layer, caption.layer],
        actual_length,
        owned_field_digest,
        preserved_field_digest: None,
        host_bound: false,
        artifact_digests,
    })
}

fn validate_common_item(
    cue: &PlannedCue,
    item: &Ymm4ManagedItem,
) -> Result<u32, RealizationReadbackError> {
    let actual_length = validate_duration(cue, item.length)?;
    if item.entity_id != cue.intent.entity_id
        || item.revision != cue.intent.entity_revision
        || item.frame != cue.placement.frame
        || item.layer != cue.placement.primary_layer
    {
        return Err(RealizationReadbackError::SemanticMismatch(
            cue.intent.entity_id.clone(),
        ));
    }
    Ok(actual_length)
}

fn validate_duration(cue: &PlannedCue, actual: i32) -> Result<u32, RealizationReadbackError> {
    let actual = u32::try_from(actual)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| RealizationReadbackError::Duration(cue.intent.entity_id.clone()))?;
    let valid = match cue.duration {
        DurationResolution::Exact { frames } => actual == frames,
        DurationResolution::Bounded { max_frames } => actual <= max_frames,
        DurationResolution::Unknown => true,
    };
    if !valid {
        return Err(RealizationReadbackError::Duration(
            cue.intent.entity_id.clone(),
        ));
    }
    Ok(actual)
}

#[derive(Debug, Error)]
pub enum RealizationReadbackError {
    #[error("invalid target plan for read-back: {0}")]
    InvalidPlan(String),
    #[error("expected {expected} item(s) for {entity_id}, found {actual}")]
    ItemCount {
        entity_id: String,
        expected: usize,
        actual: usize,
    },
    #[error("semantic realization mismatch for {0}")]
    SemanticMismatch(String),
    #[error("realization duration is outside the approved range for {0}")]
    Duration(String),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        ChangeBudget, ManagedCueIntent, OrderingPolicy, OwnershipMask, PlacementIntent,
        RealizationPreference, RevisionId, ScopeFingerprints, TARGET_PLAN_CANONICAL_VERSION,
        TargetIdentity, TimingAnchor,
    };
    use uuid::Uuid;

    // This fixture intentionally spells out the complete sealed-plan contract.
    #[allow(clippy::too_many_lines)]
    fn plan(strategy: RealizationStrategy) -> TargetPlan {
        let digest = |value: char| format!("sha256:{}", value.to_string().repeat(64));
        let mut intent = ManagedCueIntent::new(
            "utt-01",
            2,
            "第二形態だぜ",
            "第二形態だぜ",
            "marisa",
            PlacementIntent {
                anchor: TimingAnchor::AbsoluteFrame { frame: 10 },
                ordering: OrderingPolicy::Fixed,
                track_role: "dialogue".into(),
            },
        );
        intent.voice_profile = Some("marisa".into());
        intent.realization_preference = match strategy {
            RealizationStrategy::Ymm4NativeVoice => RealizationPreference::RequireNative,
            RealizationStrategy::PortableAudioCaption => RealizationPreference::RequirePortable,
        };
        TargetPlan {
            canonical_version: TARGET_PLAN_CANONICAL_VERSION,
            operation_id: Uuid::from_u128(1),
            base_revision: RevisionId(0),
            target: TargetIdentity {
                adapter_id: "ymm4".into(),
                project_id: "project".into(),
                scene_id: "scene".into(),
                fps: 60,
                driver_version: "test".into(),
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
                strategy,
                fallback: None,
                placement: takegraph_core::ResolvedPlacement {
                    frame: 10,
                    primary_layer: 20,
                    secondary_layer: (strategy == RealizationStrategy::PortableAudioCaption)
                        .then_some(21),
                },
                duration: match strategy {
                    RealizationStrategy::Ymm4NativeVoice => {
                        DurationResolution::Bounded { max_frames: 100 }
                    }
                    RealizationStrategy::PortableAudioCaption => {
                        DurationResolution::Exact { frames: 40 }
                    }
                },
                ownership: match strategy {
                    RealizationStrategy::Ymm4NativeVoice => OwnershipMask::native_voice_create(),
                    RealizationStrategy::PortableAudioCaption => {
                        OwnershipMask::portable_pair_create()
                    }
                },
                capability_dependencies: vec![takegraph_core::CapabilityDependency {
                    feature: match strategy {
                        RealizationStrategy::Ymm4NativeVoice => "voiceItem.create",
                        RealizationStrategy::PortableAudioCaption => "managedPair.apply",
                    }
                    .into(),
                    minimum_version: 1,
                    schema_digest: Some(digest('e')),
                }],
                binding_dependencies: Some(match strategy {
                    RealizationStrategy::PortableAudioCaption => {
                        takegraph_core::BindingDependency {
                            kind: "audio_artifact".into(),
                            id: "a".repeat(64),
                            digest: digest('a'),
                        }
                    }
                    RealizationStrategy::Ymm4NativeVoice => takegraph_core::BindingDependency {
                        kind: "character_name_legacy".into(),
                        id: "marisa".into(),
                        digest: digest('f'),
                    },
                })
                .into_iter()
                .collect(),
                resolved_realization: match strategy {
                    RealizationStrategy::PortableAudioCaption => {
                        takegraph_core::ResolvedRealization::PortablePair {
                            audio_path: "audio.wav".into(),
                            artifact_digest: digest('a'),
                        }
                    }
                    RealizationStrategy::Ymm4NativeVoice => {
                        takegraph_core::ResolvedRealization::NativeVoice {
                            character_name: "marisa".into(),
                            character_binding_digest: digest('f'),
                        }
                    }
                },
            }],
            warnings: vec![],
        }
    }

    #[test]
    fn one_semantic_contract_accepts_native_or_pair_shape() {
        let native = Ymm4ManagedItem {
            entity_id: "utt-01".into(),
            revision: 2,
            kind: ManagedItemKind::Voice,
            frame: 10,
            layer: 20,
            length: 80,
            text: Some("第二形態だぜ".into()),
            audio_path: None,
            artifact_hash: None,
            speaker: Some("marisa".into()),
            realization_id: Some(Uuid::from_u128(2)),
        };
        let native_result =
            normalize_realizations(&plan(RealizationStrategy::Ymm4NativeVoice), &[native]).unwrap();
        assert!(native_result[0].host_bound);

        let pair = vec![
            Ymm4ManagedItem {
                entity_id: "utt-01".into(),
                revision: 2,
                kind: ManagedItemKind::Audio,
                frame: 10,
                layer: 20,
                length: 40,
                text: None,
                audio_path: Some("audio.wav".into()),
                artifact_hash: Some("a".repeat(64)),
                speaker: None,
                realization_id: None,
            },
            Ymm4ManagedItem {
                entity_id: "utt-01".into(),
                revision: 2,
                kind: ManagedItemKind::Caption,
                frame: 10,
                layer: 21,
                length: 40,
                text: Some("第二形態だぜ".into()),
                audio_path: None,
                artifact_hash: Some("a".repeat(64)),
                speaker: None,
                realization_id: None,
            },
        ];
        let pair_result =
            normalize_realizations(&plan(RealizationStrategy::PortableAudioCaption), &pair)
                .unwrap();
        assert!(!pair_result[0].host_bound);
        assert_eq!(
            pair_result[0].artifact_digests["audio"],
            format!("sha256:{}", "a".repeat(64))
        );
    }
}
