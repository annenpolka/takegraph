use sha2::{Digest, Sha256};
use takegraph_core::{
    BindingDependency, CapabilityDependency, ChangeBudget, DurationResolution, FallbackPolicy,
    ManagedCueIntent, OrderingPolicy, OwnershipMask, PlacementIntent, PlanWarning, PlannedAction,
    PlannedCue, RealizationPreference, RealizationStrategy, ResolvedPlacement, ResolvedRealization,
    RevisionId, ScopeFingerprints, TARGET_PLAN_CANONICAL_VERSION, TargetIdentity, TargetPlan,
    TargetPlanError, TimingAnchor, canonical_sha256,
};
use takegraph_node::{
    ManagedUtterance, StructuredYmm4Capabilities, Ymm4NativeVoiceCue, Ymm4ProjectSnapshot,
};
use thiserror::Error;
use uuid::Uuid;

/// Adapts the version-1 physical pair DTO to the same semantic target-plan
/// contract used by native voice. The legacy wire request remains unchanged.
///
/// # Errors
///
/// Returns an error if capabilities cannot satisfy the requested realization
/// or canonical dependency digests cannot be produced.
pub fn portable_pair_target_plan(
    base_revision: RevisionId,
    operation_id: Uuid,
    target: &Ymm4ProjectSnapshot,
    utterances: &[ManagedUtterance],
    capabilities: &StructuredYmm4Capabilities,
) -> Result<TargetPlan, TargetPlanBuildError> {
    let target_plan_feature = required_feature(capabilities, "targetPlan.apply")?;
    let feature = required_feature(capabilities, "managedPair.apply")?;
    let transaction = required_feature(capabilities, "timeline.transaction")?;
    let readback = required_feature(capabilities, "readback.semantic")?;
    let mut cues = Vec::with_capacity(utterances.len());
    for utterance in utterances {
        let mut intent = ManagedCueIntent::new(
            &utterance.entity_id,
            utterance.revision,
            &utterance.caption,
            &utterance.spoken_text,
            &utterance.speaker,
            PlacementIntent {
                anchor: TimingAnchor::AbsoluteFrame {
                    frame: utterance.frame,
                },
                ordering: OrderingPolicy::Fixed,
                track_role: "dialogue".into(),
            },
        );
        intent.realization_preference = RealizationPreference::RequirePortable;
        intent.fallback_policy = FallbackPolicy::Reject;
        intent.voice_profile = Some(utterance.speaker.clone());
        let action = if target
            .managed_items
            .iter()
            .any(|item| item.entity_id == utterance.entity_id)
        {
            PlannedAction::Update
        } else {
            PlannedAction::Create
        };
        cues.push(PlannedCue {
            intent,
            realization_id: deterministic_realization_id(operation_id, &utterance.entity_id),
            action,
            strategy: RealizationStrategy::PortableAudioCaption,
            fallback: None,
            placement: ResolvedPlacement {
                frame: utterance.frame,
                primary_layer: utterance.audio_layer,
                secondary_layer: Some(utterance.caption_layer),
            },
            duration: DurationResolution::Exact {
                frames: positive_frames(utterance.length)?,
            },
            ownership: OwnershipMask::portable_pair_create(),
            capability_dependencies: vec![
                capability_dependency("targetPlan.apply", target_plan_feature),
                capability_dependency("managedPair.apply", feature),
                capability_dependency("timeline.transaction", transaction),
                capability_dependency("readback.semantic", readback),
            ],
            binding_dependencies: vec![BindingDependency {
                kind: "audio_artifact".into(),
                id: utterance.artifact_hash.clone(),
                digest: format!("sha256:{}", utterance.artifact_hash),
            }],
            resolved_realization: ResolvedRealization::PortablePair {
                audio_path: utterance.audio_path.clone(),
                artifact_digest: format!("sha256:{}", utterance.artifact_hash),
            },
        });
    }

    let mut warnings = Vec::new();
    let replace_count = utterances
        .iter()
        .filter(|utterance| {
            target
                .managed_items
                .iter()
                .any(|item| item.entity_id == utterance.entity_id)
        })
        .count();
    if replace_count > 0 {
        warnings.push(PlanWarning {
            code: "legacy_pair_replacement".into(),
            message: format!(
                "{replace_count} existing portable pair realization(s) will be replaced"
            ),
        });
    }
    finish_plan(
        base_revision,
        operation_id,
        target,
        capabilities,
        cues,
        warnings,
    )
}

/// Adapts create-only native voice requests to the unified target-plan model.
///
/// # Errors
///
/// Returns an error if native voice/transaction capabilities are unavailable,
/// cue fields are invalid, or canonical dependency digests cannot be produced.
pub fn native_voice_target_plan(
    base_revision: RevisionId,
    operation_id: Uuid,
    target: &Ymm4ProjectSnapshot,
    native_cues: &[Ymm4NativeVoiceCue],
    capabilities: &StructuredYmm4Capabilities,
) -> Result<TargetPlan, TargetPlanBuildError> {
    let target_plan_feature = required_feature(capabilities, "targetPlan.apply")?;
    let voice_feature = required_feature(capabilities, "voiceItem.create")?;
    let transaction = required_feature(capabilities, "timeline.transaction")?;
    let readback = required_feature(capabilities, "readback.semantic")?;
    let mut cues = Vec::with_capacity(native_cues.len());
    for cue in native_cues {
        let mut intent = ManagedCueIntent::new(
            &cue.entity_id,
            cue.revision,
            &cue.display_text,
            &cue.spoken_text,
            &cue.character_name,
            PlacementIntent {
                anchor: TimingAnchor::AbsoluteFrame { frame: cue.frame },
                ordering: OrderingPolicy::Fixed,
                track_role: "dialogue".into(),
            },
        );
        intent.realization_preference = RealizationPreference::RequireNative;
        intent.fallback_policy = FallbackPolicy::Reject;
        intent.voice_profile = Some(cue.character_name.clone());
        let binding_digest =
            canonical_sha256("takegraph-ymm4-character-name-binding", &cue.character_name)?;
        cues.push(PlannedCue {
            intent,
            realization_id: cue.realization_id,
            action: PlannedAction::Create,
            strategy: RealizationStrategy::Ymm4NativeVoice,
            fallback: None,
            placement: ResolvedPlacement {
                frame: cue.frame,
                primary_layer: cue.layer,
                secondary_layer: None,
            },
            duration: DurationResolution::Bounded {
                max_frames: positive_frames(cue.max_length)?,
            },
            ownership: OwnershipMask::native_voice_create(),
            capability_dependencies: vec![
                capability_dependency("targetPlan.apply", target_plan_feature),
                capability_dependency("voiceItem.create", voice_feature),
                capability_dependency("timeline.transaction", transaction),
                capability_dependency("readback.semantic", readback),
            ],
            binding_dependencies: vec![BindingDependency {
                kind: "character_name_legacy".into(),
                id: cue.character_name.clone(),
                digest: binding_digest.clone(),
            }],
            resolved_realization: ResolvedRealization::NativeVoice {
                character_name: cue.character_name.clone(),
                character_binding_digest: binding_digest,
            },
        });
    }

    let warnings = vec![PlanWarning {
        code: "legacy_character_name_binding".into(),
        message: format!(
            "{} native cue(s) use an exact character name until descriptor bindings are exposed",
            native_cues.len()
        ),
    }];
    finish_plan(
        base_revision,
        operation_id,
        target,
        capabilities,
        cues,
        warnings,
    )
}

fn required_feature<'a>(
    capabilities: &'a StructuredYmm4Capabilities,
    name: &str,
) -> Result<&'a takegraph_node::FeatureDescriptor, TargetPlanBuildError> {
    capabilities
        .feature(name)
        .filter(|feature| feature.available)
        .ok_or_else(|| TargetPlanBuildError::MissingCapability(name.into()))
}

fn capability_dependency(
    name: &str,
    feature: &takegraph_node::FeatureDescriptor,
) -> CapabilityDependency {
    CapabilityDependency {
        feature: name.into(),
        minimum_version: feature.version,
        schema_digest: Some(feature.schema_digest.clone()),
    }
}

fn finish_plan(
    base_revision: RevisionId,
    operation_id: Uuid,
    target: &Ymm4ProjectSnapshot,
    capabilities: &StructuredYmm4Capabilities,
    cues: Vec<PlannedCue>,
    warnings: Vec<PlanWarning>,
) -> Result<TargetPlan, TargetPlanBuildError> {
    let target_identity = ymm4_target_identity(capabilities, target);
    let expected_scope = ScopeFingerprints {
        target_identity_digest: canonical_sha256(
            "takegraph-ymm4-target-identity",
            &target_identity,
        )?,
        managed_state_digest: canonical_sha256(
            "takegraph-ymm4-managed-state",
            &target.managed_items,
        )?,
        conflict_scope_digest: canonical_sha256(
            "takegraph-ymm4-conflict-scope-v1-whole-scene",
            &serde_json::json!({
                "fingerprint": target.fingerprint,
                "unmanagedContextCount": target.unmanaged_context_count,
            }),
        )?,
    };
    let plan = TargetPlan {
        canonical_version: TARGET_PLAN_CANONICAL_VERSION,
        operation_id,
        base_revision,
        target: target_identity,
        capability_digest: capabilities.capability_digest.clone(),
        expected_scope,
        change_budget: ChangeBudget::create_only(cues.len()),
        cues,
        warnings,
    };
    plan.validate()?;
    Ok(plan)
}

/// Canonical target identity shared by all YMM4 export strategies and by
/// reconciliation. Keeping it here prevents route-specific string formatting
/// from producing mutually unreadable durable target links.
pub(crate) fn ymm4_target_identity(
    capabilities: &StructuredYmm4Capabilities,
    target: &Ymm4ProjectSnapshot,
) -> TargetIdentity {
    TargetIdentity {
        adapter_id: capabilities.driver.id.clone(),
        project_id: target.project_id.clone(),
        scene_id: target.scene_id.clone(),
        fps: target.fps,
        driver_version: format!(
            "{}/{}",
            capabilities.driver.ymm4_version, capabilities.driver.plugin_version
        ),
    }
}

fn deterministic_realization_id(operation_id: Uuid, entity_id: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"takegraph-portable-realization\0");
    hasher.update(operation_id.as_bytes());
    hasher.update(entity_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn positive_frames(value: i32) -> Result<u32, TargetPlanBuildError> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or(TargetPlanBuildError::InvalidDuration(value))
}

#[derive(Debug, Error)]
pub enum TargetPlanBuildError {
    #[error("required YMM4 capability is unavailable: {0}")]
    MissingCapability(String),
    #[error("duration must be a positive frame count, got {0}")]
    InvalidDuration(i32),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
    #[error(transparent)]
    Plan(#[from] TargetPlanError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_node::{Ymm4Capabilities, Ymm4Capability, Ymm4Health};

    fn capabilities() -> StructuredYmm4Capabilities {
        StructuredYmm4Capabilities::from_bridge(
            &Ymm4Health {
                status: "running".into(),
                protocol_version: 2,
                plugin_version: "0.2.0".into(),
                ymm4_version: "4.55.1.1".into(),
            },
            &Ymm4Capabilities {
                protocol_version: 2,
                capabilities: vec![
                    Ymm4Capability::ManagedAudio,
                    Ymm4Capability::ManagedCaption,
                    Ymm4Capability::UnifiedTargetPlan,
                    Ymm4Capability::ReadbackVerification,
                    Ymm4Capability::IdempotentApply,
                    Ymm4Capability::RequestBoundReceipts,
                    Ymm4Capability::WriteAheadApply,
                    Ymm4Capability::NativeVoiceCreate,
                    Ymm4Capability::NativeVoiceRemarkIdentity,
                    Ymm4Capability::NativeVoiceBoundedDuration,
                    Ymm4Capability::MutationProfileYmm4_4_55_1_1,
                ],
            },
        )
        .unwrap()
    }

    fn snapshot() -> Ymm4ProjectSnapshot {
        Ymm4ProjectSnapshot {
            project_id: "project-a".into(),
            project_name: "test".into(),
            project_path: "test.ymmp".into(),
            scene_id: "scene-a".into(),
            fps: 60,
            fingerprint: "external-a".into(),
            managed_items: vec![],
            native_extensions: vec![],
            unmanaged_context_count: 3,
        }
    }

    #[test]
    fn native_and_pair_share_one_plan_contract() {
        let native = native_voice_target_plan(
            RevisionId(3),
            Uuid::from_u128(1),
            &snapshot(),
            &[Ymm4NativeVoiceCue {
                realization_id: Uuid::from_u128(2),
                entity_id: "utt-01".into(),
                revision: 1,
                character_name: "魔理沙".into(),
                display_text: "第二形態だぜ".into(),
                spoken_text: "第二形態だぜ".into(),
                frame: 10,
                layer: 20,
                max_length: 120,
            }],
            &capabilities(),
        )
        .unwrap();
        let portable = portable_pair_target_plan(
            RevisionId(3),
            Uuid::from_u128(1),
            &snapshot(),
            &[ManagedUtterance {
                entity_id: "utt-01".into(),
                revision: 1,
                speaker: "魔理沙".into(),
                caption: "第二形態だぜ".into(),
                spoken_text: "だいにけいたいだぜ".into(),
                audio_path: "audio.wav".into(),
                artifact_hash: "a".repeat(64),
                frame: 10,
                length: 120,
                audio_layer: 20,
                caption_layer: 21,
            }],
            &capabilities(),
        )
        .unwrap();

        assert_eq!(
            native.cues[0].strategy,
            RealizationStrategy::Ymm4NativeVoice
        );
        assert_eq!(
            portable.cues[0].strategy,
            RealizationStrategy::PortableAudioCaption
        );
        assert_eq!(portable.cues[0].intent.display_text, "第二形態だぜ");
        assert_eq!(portable.cues[0].intent.spoken_text, "だいにけいたいだぜ");
        assert_eq!(native.capability_digest, portable.capability_digest);
        let request =
            takegraph_node::Ymm4TargetPlanApplyRequest::new(native.clone(), snapshot().fingerprint)
                .unwrap();
        let request_json = serde_json::to_value(request).unwrap();
        assert_eq!(request_json["expectedFingerprint"], snapshot().fingerprint);
        assert_eq!(
            request_json["targetPlan"]["cues"][0]["strategy"],
            "native_voice"
        );
        assert_eq!(
            request_json["targetPlan"]["cues"][0]["resolvedRealization"]["kind"],
            "native_voice"
        );
        assert_ne!(
            native.canonical_digest().unwrap(),
            portable.canonical_digest().unwrap()
        );
    }
}
