//! Portable, deterministic plan for one heterogeneous YMM4 timeline transaction.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    CanonicalError, ChangeBudget, NativeExtensionIntent, PlanWarning, PlannedCue,
    PlannedNativeExtension, RevisionId, ScopeFingerprints, SourceEvidenceRef, TargetIdentity,
    TargetPlanError, canonical_sha256, managed_cue::validate_planned_cue,
    native_extension::validate_planned_native_extension,
};

pub const TIMELINE_EDIT_PLAN_CANONICAL_VERSION: u32 = 1;
pub const TIMELINE_EDIT_MAX_OPERATIONS: usize = 128;

/// One operation in caller-selected canonical application order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineEditOperation {
    ManagedCue {
        cue: Box<PlannedCue>,
    },
    NativeExtension {
        descriptor_catalog_digest: String,
        operation: Box<PlannedNativeExtension>,
    },
}

impl TimelineEditOperation {
    #[must_use]
    pub fn realization_id(&self) -> Uuid {
        match self {
            Self::ManagedCue { cue } => cue.realization_id,
            Self::NativeExtension { operation, .. } => operation.realization_id,
        }
    }

    #[must_use]
    pub fn write_identity(&self) -> String {
        match self {
            Self::ManagedCue { cue } => format!("entity:{}", cue.intent.entity_id),
            Self::NativeExtension { operation, .. } => match &operation.intent {
                NativeExtensionIntent::UpsertPortrait(intent) => {
                    format!("entity:{}", intent.entity_id)
                }
                NativeExtensionIntent::UpsertAsset(intent) => {
                    format!("entity:{}", intent.entity_id)
                }
                NativeExtensionIntent::MutateEffect(intent) => format!(
                    "effect:{}:{}",
                    intent.target_entity_id, intent.effect_instance_id
                ),
                NativeExtensionIntent::InstantiateTemplate(intent) => {
                    format!("entity:{}", intent.entity_id)
                }
            },
        }
    }

    fn validate(&self) -> Result<(), TimelineEditError> {
        match self {
            Self::ManagedCue { cue } => validate_planned_cue(cue).map_err(Into::into),
            Self::NativeExtension {
                descriptor_catalog_digest,
                operation,
            } => {
                require_sha256(descriptor_catalog_digest, "descriptorCatalogDigest")?;
                validate_planned_native_extension(operation).map_err(Into::into)
            }
        }
    }
}

/// Exact, approval-bound envelope for one atomic timeline edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineEditPlan {
    pub canonical_version: u32,
    pub operation_id: Uuid,
    pub base_revision: RevisionId,
    pub target: TargetIdentity,
    pub capability_digest: String,
    pub expected_scope: ScopeFingerprints,
    pub change_budget: ChangeBudget,
    pub operations: Vec<TimelineEditOperation>,
    pub warnings: Vec<PlanWarning>,
    /// Optional annotation provenance sealed into the plan digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_evidence: Vec<SourceEvidenceRef>,
}

impl TimelineEditPlan {
    /// Validates the complete transaction and hashes its canonical ordered payload.
    ///
    /// # Errors
    ///
    /// Returns an error if the plan violates an invariant or canonical JSON
    /// serialization fails.
    pub fn canonical_digest(&self) -> Result<String, TimelineEditError> {
        self.validate()?;
        canonical_sha256("takegraph-timeline-edit-plan-v1", self).map_err(Into::into)
    }

    /// Rejects malformed, oversized, ambiguous, or conflicting edit transactions.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid binding, operation, budget, digest,
    /// realization identity, or conflicting write identity.
    pub fn validate(&self) -> Result<(), TimelineEditError> {
        if self.canonical_version != TIMELINE_EDIT_PLAN_CANONICAL_VERSION {
            return Err(TimelineEditError::UnsupportedCanonicalVersion(
                self.canonical_version,
            ));
        }
        if self.operation_id.is_nil() {
            return Err(TimelineEditError::EmptyField("operationId"));
        }
        if self.target.adapter_id.trim().is_empty()
            || self.target.project_id.trim().is_empty()
            || self.target.scene_id.trim().is_empty()
            || self.target.driver_version.trim().is_empty()
        {
            return Err(TimelineEditError::EmptyField("target binding"));
        }
        if self.target.fps == 0 {
            return Err(TimelineEditError::InvalidTargetFps);
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
        if self.change_budget.allow_unmanaged_changes {
            return Err(TimelineEditError::UnmanagedChangesForbidden);
        }
        if self.operations.is_empty() {
            return Err(TimelineEditError::EmptyOperations);
        }
        if self.operations.len() > TIMELINE_EDIT_MAX_OPERATIONS
            || self.change_budget.max_changed_entities > TIMELINE_EDIT_MAX_OPERATIONS
            || self.operations.len() > self.change_budget.max_changed_entities
        {
            return Err(TimelineEditError::ChangeBudgetExceeded);
        }

        let mut realizations = BTreeSet::new();
        let mut writes = BTreeSet::new();
        for operation in &self.operations {
            let realization_id = operation.realization_id();
            if realization_id.is_nil() {
                return Err(TimelineEditError::EmptyRealization);
            }
            operation.validate()?;
            if !realizations.insert(realization_id) {
                return Err(TimelineEditError::DuplicateRealization(realization_id));
            }
            let identity = operation.write_identity();
            if !writes.insert(identity.clone()) {
                return Err(TimelineEditError::ConflictingWriteIdentity(identity));
            }
        }
        for evidence in &self.source_evidence {
            evidence
                .validate()
                .map_err(|error| TimelineEditError::InvalidSourceEvidence(error.to_string()))?;
        }
        Ok(())
    }
}

fn require_sha256(value: &str, field: &'static str) -> Result<(), TimelineEditError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(TimelineEditError::InvalidDigest(field));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TimelineEditError::InvalidDigest(field));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum TimelineEditError {
    #[error("required field is empty: {0}")]
    EmptyField(&'static str),
    #[error("unsupported timeline-edit canonical version: {0}")]
    UnsupportedCanonicalVersion(u32),
    #[error("target FPS must be positive")]
    InvalidTargetFps,
    #[error("field is not a canonical SHA-256 digest: {0}")]
    InvalidDigest(&'static str),
    #[error("timeline edit requires at least one operation")]
    EmptyOperations,
    #[error("timeline edit exceeds its operation/change budget")]
    ChangeBudgetExceeded,
    #[error("timeline edits never authorize unmanaged changes")]
    UnmanagedChangesForbidden,
    #[error("duplicate realization ID: {0}")]
    DuplicateRealization(Uuid),
    #[error("timeline-edit realization ID must not be nil")]
    EmptyRealization,
    #[error("multiple operations write the same identity: {0}")]
    ConflictingWriteIdentity(String),
    #[error("invalid source evidence: {0}")]
    InvalidSourceEvidence(String),
    #[error(transparent)]
    TargetPlan(#[from] TargetPlanError),
    #[error(transparent)]
    NativeExtension(#[from] crate::NativeExtensionError),
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BindingDependency, CapabilityDependency, DurationResolution, FallbackPolicy,
        FallbackReason, ManagedCueIntent, OrderingPolicy, OwnershipMask, PlacementIntent,
        PlannedAction, RealizationPreference, RealizationStrategy, ResolvedPlacement,
        ResolvedRealization, TimingAnchor,
    };

    fn digest(value: char) -> String {
        format!("sha256:{}", value.to_string().repeat(64))
    }

    fn cue(entity_id: &str, realization_id: u128) -> PlannedCue {
        let placement = PlacementIntent {
            anchor: TimingAnchor::AbsoluteFrame { frame: 10 },
            ordering: OrderingPolicy::Fixed,
            track_role: "dialogue".into(),
        };
        let mut intent =
            ManagedCueIntent::new(entity_id, 1, "caption", "spoken", "speaker", placement);
        intent.voice_profile = Some("speaker".into());
        intent.realization_preference = RealizationPreference::RequirePortable;
        intent.fallback_policy = FallbackPolicy::Reject;
        PlannedCue {
            intent,
            realization_id: Uuid::from_u128(realization_id),
            action: PlannedAction::Create,
            strategy: RealizationStrategy::PortableAudioCaption,
            fallback: None::<FallbackReason>,
            placement: ResolvedPlacement {
                frame: 10,
                primary_layer: 1,
                secondary_layer: Some(2),
            },
            duration: DurationResolution::Exact { frames: 30 },
            ownership: OwnershipMask::portable_pair_create(),
            capability_dependencies: vec![CapabilityDependency {
                feature: "timelineEdit.apply".into(),
                minimum_version: 1,
                schema_digest: Some(digest('a')),
            }],
            binding_dependencies: vec![BindingDependency {
                kind: "audio_artifact".into(),
                id: "artifact".into(),
                digest: digest('b'),
            }],
            resolved_realization: ResolvedRealization::PortablePair {
                audio_path: "staged/audio.wav".into(),
                artifact_digest: digest('b'),
            },
        }
    }

    fn plan(operations: Vec<TimelineEditOperation>) -> TimelineEditPlan {
        TimelineEditPlan {
            canonical_version: TIMELINE_EDIT_PLAN_CANONICAL_VERSION,
            operation_id: Uuid::from_u128(42),
            base_revision: RevisionId(7),
            target: TargetIdentity {
                adapter_id: "ymm4".into(),
                project_id: "project".into(),
                scene_id: "scene".into(),
                fps: 60,
                driver_version: "4.55.1.1/0.3.0".into(),
            },
            capability_digest: digest('c'),
            expected_scope: ScopeFingerprints {
                target_identity_digest: digest('d'),
                managed_state_digest: digest('e'),
                conflict_scope_digest: digest('f'),
            },
            change_budget: ChangeBudget::create_only(operations.len()),
            operations,
            warnings: vec![],
            source_evidence: vec![],
        }
    }

    #[test]
    fn digest_binds_source_evidence() {
        use crate::{AnnotationId, SourceEvidenceRef};
        let operations = vec![TimelineEditOperation::ManagedCue {
            cue: Box::new(cue("a", 1)),
        }];
        let without = plan(operations.clone()).canonical_digest().unwrap();
        let mut with_evidence = plan(operations);
        with_evidence.source_evidence = vec![SourceEvidenceRef {
            annotation_id: AnnotationId::new(),
            capture_audio_sha256: digest('1'),
            transcript_digest: digest('2'),
            interpretation_digest: digest('3'),
        }];
        assert_ne!(without, with_evidence.canonical_digest().unwrap());
    }

    #[test]
    fn digest_binds_canonical_operation_order() {
        let first = TimelineEditOperation::ManagedCue {
            cue: Box::new(cue("a", 1)),
        };
        let second = TimelineEditOperation::ManagedCue {
            cue: Box::new(cue("b", 2)),
        };
        assert_ne!(
            plan(vec![first.clone(), second.clone()])
                .canonical_digest()
                .unwrap(),
            plan(vec![second, first]).canonical_digest().unwrap()
        );
    }

    #[test]
    fn rejects_duplicate_write_identity_across_batch() {
        let result = plan(vec![
            TimelineEditOperation::ManagedCue {
                cue: Box::new(cue("same", 1)),
            },
            TimelineEditOperation::ManagedCue {
                cue: Box::new(cue("same", 2)),
            },
        ])
        .validate();
        assert!(matches!(
            result,
            Err(TimelineEditError::ConflictingWriteIdentity(identity)) if identity == "entity:same"
        ));
    }

    #[test]
    fn rejects_empty_and_more_than_128_operations() {
        assert!(matches!(
            plan(vec![]).validate(),
            Err(TimelineEditError::EmptyOperations)
        ));
        let operations = (1..=129)
            .map(|index| TimelineEditOperation::ManagedCue {
                cue: Box::new(cue(&format!("entity-{index}"), index)),
            })
            .collect::<Vec<_>>();
        let mut oversized = plan(operations);
        oversized.change_budget.max_changed_entities = 129;
        assert!(matches!(
            oversized.validate(),
            Err(TimelineEditError::ChangeBudgetExceeded)
        ));

        let mut overbroad_budget = plan(vec![TimelineEditOperation::ManagedCue {
            cue: Box::new(cue("one", 1)),
        }]);
        overbroad_budget.change_budget.max_changed_entities = 129;
        assert!(matches!(
            overbroad_budget.validate(),
            Err(TimelineEditError::ChangeBudgetExceeded)
        ));
    }

    #[test]
    fn rejects_nil_realization_identity() {
        let mut invalid = cue("one", 1);
        invalid.realization_id = Uuid::nil();
        assert!(matches!(
            plan(vec![TimelineEditOperation::ManagedCue {
                cue: Box::new(invalid),
            }])
            .validate(),
            Err(TimelineEditError::EmptyRealization)
        ));
    }

    #[test]
    fn json_uses_ordered_tagged_operations() {
        let mut native = cue("a", 1);
        native.strategy = RealizationStrategy::Ymm4NativeVoice;
        native.duration = DurationResolution::Bounded { max_frames: 120 };
        native.resolved_realization = ResolvedRealization::NativeVoice {
            character_name: "character".into(),
            character_binding_digest: digest('9'),
        };
        let value = serde_json::to_value(plan(vec![TimelineEditOperation::ManagedCue {
            cue: Box::new(native),
        }]))
        .unwrap();
        assert_eq!(value["operations"][0]["kind"], "managed_cue");
        assert_eq!(value["operations"][0]["cue"]["intent"]["entityId"], "a");
        assert_eq!(value["operations"][0]["cue"]["duration"]["max_frames"], 120);
        assert!(
            value["operations"][0]["cue"]["duration"]
                .get("maxFrames")
                .is_none()
        );
        assert_eq!(
            value["operations"][0]["cue"]["resolvedRealization"]["character_name"],
            "character"
        );
    }
}
