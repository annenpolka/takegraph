//! Portable field-edit admission from `ymm4EditSurfaceProtocol`.
//!
//! This module executes the modeled normalize → admit → approve → apply →
//! exact-readback → commit path. It does not talk to YMM4.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Quint `initialCanonicalRevision`.
pub const INITIAL_CANONICAL_REVISION: i64 = 13;
const INITIAL_SOURCE_ID: i64 = 71;
const INITIAL_SOURCE_REVISION: i64 = 5;
const INITIAL_SOURCE_FINGERPRINT: i64 = 500;
const INITIAL_TARGET_FINGERPRINT: i64 = 700;
const INITIAL_SURFACE_REVISION: i64 = 4;
const INITIAL_CAPABILITY_DIGEST: i64 = 410;
const INITIAL_SCHEMA_DIGEST: i64 = 420;
const INITIAL_DESCRIPTOR_DIGEST: i64 = 430;
const INITIAL_FIELD_VALUE: i64 = 20;
const DEFAULT_FIELD_VALUE: i64 = 0;
const INITIAL_UNTOUCHED_KNOWN_FIELD: i64 = 31;
const INITIAL_UNKNOWN_FIELD_DIGEST: i64 = 991;

/// Field-editable item families. Structural containers live elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemFamily {
    ManagedVoiceItem,
    ManagedAudioItem,
    ManagedTextItem,
    ShapeItem,
    EffectItem,
    FrameBufferItem,
    GenericYmmItem,
}

/// Semantic field catalog. `UnknownField` is preservable, never writable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldClass {
    TimingField,
    TransformPositionField,
    TransformScaleField,
    TransformRotationField,
    OpacityVisibilityField,
    VisualStyleField,
    TextContentField,
    TextStyleField,
    AudioSourceField,
    AudioParametersField,
    MaskField,
    LockField,
    TypedEffectParameterField,
    KnownExtensionField,
    UnknownField,
}

/// Caller-supplied patch form before normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchInput {
    PatchAbsent,
    PatchReset,
    PatchValue,
    PatchExplicitNoOp,
}

/// Canonical patch after `normalize_patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedPatch {
    NotNormalized,
    FieldUntouched,
    ResetToDefault,
    SetCanonicalValue,
    NormalizedNoOp,
}

/// Create / update / delete at the field-edit surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrudKind {
    Create,
    Update,
    Delete,
}

/// Whether a capability, schema, descriptor, or observation is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
}

/// Ownership of the target entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Managed,
    Unmanaged,
    OwnershipNotApplicable,
}

/// Lock state of the target entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLock {
    Unlocked,
    Locked,
    LockNotApplicable,
}

/// Modeled admission / approval reject reasons, in protocol priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum EditSurfaceRejectReason {
    #[error("edit surface has no rejection")]
    NoRejection,
    #[error("capability is unavailable or does not cover this family/crud/field")]
    UnsupportedCapability,
    #[error("schema advertisement is missing")]
    MissingSchema,
    #[error("schema does not cover the requested field")]
    SchemaDoesNotCoverField,
    #[error("descriptor advertisement is missing")]
    MissingDescriptor,
    #[error("descriptor family does not match the request")]
    DescriptorFamilyMismatch,
    #[error("descriptor does not cover the requested field")]
    DescriptorDoesNotCoverField,
    #[error("authenticated observation is unavailable")]
    ObservationUnavailable,
    #[error("source/target snapshot is stale")]
    SourceBindingStale,
    #[error("target existence does not match the requested crud")]
    TargetExistenceMismatch,
    #[error("target is unmanaged")]
    UnmanagedTarget,
    #[error("target is locked")]
    LockedTarget,
    #[error("approval digest does not match the sealed plan")]
    ApprovalDigestMismatch,
}

/// Lifecycle of one field-edit request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditSurfaceStatus {
    Draft,
    Normalized,
    Admitted,
    Previewable,
    Approved,
    Verifying,
    Verified,
    Committed,
    Rejected,
    VerificationFailed,
    Conflicted,
}

/// Approval-bound plan for one field edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanBinding {
    pub request_id: i64,
    pub family: ItemFamily,
    pub crud: CrudKind,
    pub field_class: FieldClass,
    pub normalized_patch: NormalizedPatch,
    pub request_digest: i64,
    pub plan_digest: i64,
    pub canonical_base_revision: i64,
    pub source_id: i64,
    pub source_revision: i64,
    pub source_fingerprint: i64,
    pub target_fingerprint: i64,
    pub surface_revision: i64,
    pub capability_digest: i64,
    pub schema_digest: i64,
    pub descriptor_digest: i64,
}

/// Evidence recorded at apply time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct ApplicationEvidence {
    pub capability_accepted: bool,
    pub schema_accepted: bool,
    pub descriptor_accepted: bool,
    pub observation_accepted: bool,
    pub source_binding_fresh: bool,
    pub existence_accepted: bool,
    pub ownership_accepted: bool,
    pub lock_accepted: bool,
    pub approval_exact: bool,
    pub plan_binding_fresh: bool,
}

/// Executable field-edit session. Starts at the protocol `init` state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct EditSurfaceSession {
    pub status: EditSurfaceStatus,
    pub rejection: EditSurfaceRejectReason,
    pub canonical_head_revision: i64,
    pub external_commit_count: i64,
    pub source_id: i64,
    pub source_revision: i64,
    pub source_fingerprint: i64,
    pub target_fingerprint: i64,
    pub target_exists: bool,
    pub surface_revision: i64,
    pub capability: Availability,
    pub capability_digest: i64,
    pub capability_family: ItemFamily,
    pub capability_crud: CrudKind,
    pub capability_supports_field: bool,
    pub schema: Availability,
    pub schema_digest: i64,
    pub schema_family: ItemFamily,
    pub schema_covers_field: bool,
    pub descriptor: Availability,
    pub descriptor_digest: i64,
    pub descriptor_family: ItemFamily,
    pub descriptor_covers_field: bool,
    pub observation: Availability,
    pub observed_canonical_head_revision: i64,
    pub observed_source_id: i64,
    pub observed_source_revision: i64,
    pub observed_source_fingerprint: i64,
    pub observed_target_fingerprint: i64,
    pub observed_target_exists: bool,
    pub observed_surface_revision: i64,
    pub observation_count: i64,
    pub ownership: Ownership,
    pub target_lock: TargetLock,
    pub patch_input: PatchInput,
    pub supplied_value: i64,
    pub current_field_value: i64,
    pub default_field_value: i64,
    pub normalized_patch: NormalizedPatch,
    pub expected_field_value: i64,
    pub untouched_known_field_value: i64,
    pub unknown_field_digest: i64,
    pub plan_clock: i64,
    pub task: PlanBinding,
    pub approved_binding: Option<PlanBinding>,
    pub invalidated_binding: Option<PlanBinding>,
    pub applied_binding: Option<PlanBinding>,
    pub commit_binding: Option<PlanBinding>,
    pub application_evidence: ApplicationEvidence,
    pub application_count: i64,
    pub target_mutation_count: i64,
    pub read_back_count: i64,
    pub read_back_field_value: Option<i64>,
    pub read_back_target_exists: Option<bool>,
    pub touched_field_read_back_exact: bool,
    pub target_existence_read_back_exact: bool,
    pub canonical_commit_count: i64,
    pub replay_count: i64,
    pub approval_invalidation_count: i64,
}

/// Errors for illegal lifecycle transitions (not modeled reject reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EditSurfaceError {
    #[error("edit surface is in {actual:?}, expected {expected:?}")]
    UnexpectedStatus {
        expected: EditSurfaceStatus,
        actual: EditSurfaceStatus,
    },
    #[error(transparent)]
    Rejected(#[from] EditSurfaceRejectReason),
}

impl EditSurfaceSession {
    /// Protocol `init`: Draft update of audio parameters, observation unavailable.
    #[must_use]
    pub fn initial() -> Self {
        Self {
            status: EditSurfaceStatus::Draft,
            rejection: EditSurfaceRejectReason::NoRejection,
            canonical_head_revision: INITIAL_CANONICAL_REVISION,
            external_commit_count: 0,
            source_id: INITIAL_SOURCE_ID,
            source_revision: INITIAL_SOURCE_REVISION,
            source_fingerprint: INITIAL_SOURCE_FINGERPRINT,
            target_fingerprint: INITIAL_TARGET_FINGERPRINT,
            target_exists: true,
            surface_revision: INITIAL_SURFACE_REVISION,
            capability: Availability::Available,
            capability_digest: INITIAL_CAPABILITY_DIGEST,
            capability_family: ItemFamily::ManagedVoiceItem,
            capability_crud: CrudKind::Update,
            capability_supports_field: true,
            schema: Availability::Available,
            schema_digest: INITIAL_SCHEMA_DIGEST,
            schema_family: ItemFamily::ManagedVoiceItem,
            schema_covers_field: true,
            descriptor: Availability::Available,
            descriptor_digest: INITIAL_DESCRIPTOR_DIGEST,
            descriptor_family: ItemFamily::ManagedVoiceItem,
            descriptor_covers_field: true,
            observation: Availability::Unavailable,
            observed_canonical_head_revision: -1,
            observed_source_id: -1,
            observed_source_revision: -1,
            observed_source_fingerprint: -1,
            observed_target_fingerprint: -1,
            observed_target_exists: false,
            observed_surface_revision: -1,
            observation_count: 0,
            ownership: Ownership::Managed,
            target_lock: TargetLock::Unlocked,
            patch_input: PatchInput::PatchValue,
            supplied_value: 25,
            current_field_value: INITIAL_FIELD_VALUE,
            default_field_value: DEFAULT_FIELD_VALUE,
            normalized_patch: NormalizedPatch::NotNormalized,
            expected_field_value: -1,
            untouched_known_field_value: INITIAL_UNTOUCHED_KNOWN_FIELD,
            unknown_field_digest: INITIAL_UNKNOWN_FIELD_DIGEST,
            plan_clock: 0,
            task: PlanBinding {
                request_id: 1,
                family: ItemFamily::ManagedVoiceItem,
                crud: CrudKind::Update,
                field_class: FieldClass::AudioParametersField,
                normalized_patch: NormalizedPatch::NotNormalized,
                request_digest: 100,
                plan_digest: -1,
                canonical_base_revision: -1,
                source_id: -1,
                source_revision: -1,
                source_fingerprint: -1,
                target_fingerprint: -1,
                surface_revision: -1,
                capability_digest: -1,
                schema_digest: -1,
                descriptor_digest: -1,
            },
            approved_binding: None,
            invalidated_binding: None,
            applied_binding: None,
            commit_binding: None,
            application_evidence: ApplicationEvidence::default(),
            application_count: 0,
            target_mutation_count: 0,
            read_back_count: 0,
            read_back_field_value: None,
            read_back_target_exists: None,
            touched_field_read_back_exact: false,
            target_existence_read_back_exact: false,
            canonical_commit_count: 0,
            replay_count: 0,
            approval_invalidation_count: 0,
        }
    }

    /// Selects family/crud/field and resets ownership/lock to the modeled defaults.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless the session is Draft
    /// and no observation has been taken.
    pub fn configure_request(
        &mut self,
        family: ItemFamily,
        crud: CrudKind,
        field_class: FieldClass,
    ) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Draft)?;
        if self.observation_count != 0 {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Draft,
                actual: self.status,
            });
        }
        self.task.family = family;
        self.task.crud = crud;
        self.task.field_class = field_class;
        self.task.request_digest += 1;
        self.task.normalized_patch = NormalizedPatch::NotNormalized;
        self.capability_family = family;
        self.capability_crud = crud;
        self.schema_family = family;
        self.descriptor_family = family;
        self.ownership = if crud == CrudKind::Create {
            Ownership::OwnershipNotApplicable
        } else {
            Ownership::Managed
        };
        self.target_lock = if crud == CrudKind::Create {
            TargetLock::LockNotApplicable
        } else {
            TargetLock::Unlocked
        };
        self.target_exists = crud != CrudKind::Create;
        self.normalized_patch = NormalizedPatch::NotNormalized;
        Ok(())
    }

    /// Sets the raw patch form. Draft only, before observation.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless the session is Draft.
    pub fn set_patch_input(&mut self, input: PatchInput) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Draft)?;
        self.patch_input = input;
        Ok(())
    }

    /// Sets the supplied value used by `PatchValue`.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless the session is Draft.
    pub fn set_supplied_value(&mut self, value: i64) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Draft)?;
        self.supplied_value = value;
        Ok(())
    }

    /// Normalizes the raw patch into a canonical form and expected value.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless the session is Draft.
    pub fn normalize(&mut self) -> Result<NormalizedPatch, EditSurfaceError> {
        self.require(EditSurfaceStatus::Draft)?;
        let normalized = normalize_patch(
            self.patch_input,
            self.current_field_value,
            self.default_field_value,
            self.supplied_value,
        );
        self.status = EditSurfaceStatus::Normalized;
        self.normalized_patch = normalized;
        self.expected_field_value = expected_value_for(
            normalized,
            self.current_field_value,
            self.default_field_value,
            self.supplied_value,
        );
        self.task.normalized_patch = normalized;
        Ok(normalized)
    }

    /// Records a fresh read-only observation of the current source/target.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Draft or Normalized.
    pub fn observe_read_only(&mut self) -> Result<(), EditSurfaceError> {
        if !matches!(
            self.status,
            EditSurfaceStatus::Draft | EditSurfaceStatus::Normalized
        ) {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Normalized,
                actual: self.status,
            });
        }
        self.observation = Availability::Available;
        self.observed_canonical_head_revision = self.canonical_head_revision;
        self.observed_source_id = self.source_id;
        self.observed_source_revision = self.source_revision;
        self.observed_source_fingerprint = self.source_fingerprint;
        self.observed_target_fingerprint = self.target_fingerprint;
        self.observed_target_exists = self.target_exists;
        self.observed_surface_revision = self.surface_revision;
        self.observation_count += 1;
        Ok(())
    }

    /// Drops the current observation without mutating the target.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Normalized and observed.
    pub fn lose_observation(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Normalized)?;
        if self.observation != Availability::Available {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Normalized,
                actual: self.status,
            });
        }
        self.observation = Availability::Unavailable;
        Ok(())
    }

    /// Admits a normalized request or fail-closes with the modeled reason.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Normalized, or
    /// [`EditSurfaceRejectReason`] when admission fails.
    pub fn admit(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Normalized)?;
        if admission_accepts(self) {
            self.status = EditSurfaceStatus::Admitted;
            self.rejection = EditSurfaceRejectReason::NoRejection;
            Ok(())
        } else {
            let reason = admission_failure(self);
            self.status = EditSurfaceStatus::Rejected;
            self.rejection = reason;
            Err(EditSurfaceError::Rejected(reason))
        }
    }

    /// Seals the admitted request to a previewable, digest-bound plan.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Admitted, or a
    /// reject reason if admission evidence has drifted.
    pub fn stage_preview(&mut self) -> Result<i64, EditSurfaceError> {
        self.require(EditSurfaceStatus::Admitted)?;
        if !admission_accepts(self) {
            let reason = admission_failure(self);
            self.status = EditSurfaceStatus::Rejected;
            self.rejection = reason;
            return Err(EditSurfaceError::Rejected(reason));
        }
        self.plan_clock += 1;
        self.status = EditSurfaceStatus::Previewable;
        self.task.normalized_patch = self.normalized_patch;
        self.task.plan_digest = self.plan_clock;
        self.task.canonical_base_revision = self.canonical_head_revision;
        self.task.source_id = self.source_id;
        self.task.source_revision = self.source_revision;
        self.task.source_fingerprint = self.source_fingerprint;
        self.task.target_fingerprint = self.target_fingerprint;
        self.task.surface_revision = self.surface_revision;
        self.task.capability_digest = self.capability_digest;
        self.task.schema_digest = self.schema_digest;
        self.task.descriptor_digest = self.descriptor_digest;
        Ok(self.task.plan_digest)
    }

    /// Approves the exact sealed plan digest.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceRejectReason::ApprovalDigestMismatch`] when the
    /// digest or binding is not fresh.
    pub fn approve_exact(&mut self, plan_digest: i64) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Previewable)?;
        if plan_digest != self.task.plan_digest || !plan_binding_is_fresh(self) {
            self.status = EditSurfaceStatus::Rejected;
            self.rejection = EditSurfaceRejectReason::ApprovalDigestMismatch;
            return Err(EditSurfaceError::Rejected(
                EditSurfaceRejectReason::ApprovalDigestMismatch,
            ));
        }
        self.status = EditSurfaceStatus::Approved;
        self.approved_binding = Some(self.task.clone());
        Ok(())
    }

    /// Bumps surface / capability / schema / descriptor revisions and invalidates approval.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Approved.
    pub fn expand_surface_after_approval(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Approved)?;
        self.status = EditSurfaceStatus::Draft;
        self.surface_revision += 1;
        self.capability_digest += 1;
        self.schema_digest += 1;
        self.descriptor_digest += 1;
        self.observation = Availability::Unavailable;
        self.normalized_patch = NormalizedPatch::NotNormalized;
        self.task.normalized_patch = NormalizedPatch::NotNormalized;
        self.invalidated_binding = self.approved_binding.clone();
        self.approval_invalidation_count += 1;
        Ok(())
    }

    /// Records external source/target drift after approval.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Approved.
    pub fn external_source_drift_after_approval(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Approved)?;
        self.canonical_head_revision += 1;
        self.external_commit_count += 1;
        self.source_revision += 1;
        self.source_fingerprint += 1;
        self.target_fingerprint += 1;
        Ok(())
    }

    /// Marks an approved plan conflicted when its binding is no longer fresh.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Approved and stale.
    pub fn mark_conflict(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Approved)?;
        if plan_binding_is_fresh(self) {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Approved,
                actual: self.status,
            });
        }
        self.status = EditSurfaceStatus::Conflicted;
        Ok(())
    }

    /// Applies an approved plan. Mutation count advances only for a real write.
    ///
    /// # Errors
    ///
    /// Returns a reject or unexpected-status error when the approval is not
    /// exact and fresh.
    pub fn apply_approved(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Approved)?;
        let approved = self
            .approved_binding
            .clone()
            .ok_or(EditSurfaceError::Rejected(
                EditSurfaceRejectReason::ApprovalDigestMismatch,
            ))?;
        if self.task != approved || !plan_binding_is_fresh(self) || !admission_accepts(self) {
            self.status = EditSurfaceStatus::Rejected;
            self.rejection = EditSurfaceRejectReason::ApprovalDigestMismatch;
            return Err(EditSurfaceError::Rejected(
                EditSurfaceRejectReason::ApprovalDigestMismatch,
            ));
        }
        if self.application_count != 0 {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Approved,
                actual: self.status,
            });
        }
        let mutates = request_mutates_target(self);
        self.status = EditSurfaceStatus::Verifying;
        self.applied_binding = Some(self.task.clone());
        self.application_evidence = ApplicationEvidence {
            capability_accepted: capability_accepts(self),
            schema_accepted: schema_accepts(self),
            descriptor_accepted: descriptor_accepts(self),
            observation_accepted: observation_is_fresh(self),
            source_binding_fresh: self.task.source_id == self.source_id
                && self.task.source_revision == self.source_revision
                && self.task.source_fingerprint == self.source_fingerprint,
            existence_accepted: target_existence_accepts(self),
            ownership_accepted: ownership_accepts(self),
            lock_accepted: lock_accepts(self),
            approval_exact: true,
            plan_binding_fresh: true,
        };
        self.application_count += 1;
        if mutates {
            self.target_mutation_count += 1;
            self.target_fingerprint += 1;
        }
        self.target_exists = expected_existence_after_apply(self.task.crud);
        if self.task.crud != CrudKind::Delete {
            self.current_field_value = self.expected_field_value;
        }
        Ok(())
    }

    /// Records exact read-back of the expected field and existence.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Verifying.
    pub fn record_exact_read_back(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Verifying)?;
        if self.read_back_count != 0 {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Verifying,
                actual: self.status,
            });
        }
        self.read_back_count = 1;
        self.read_back_field_value = Some(self.expected_field_value);
        self.read_back_target_exists = Some(expected_existence_after_apply(self.task.crud));
        self.touched_field_read_back_exact = true;
        self.target_existence_read_back_exact = true;
        Ok(())
    }

    /// Records a mismatched touched-field read-back and fails verification.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Verifying a touch.
    pub fn record_mismatched_touched_field_read_back(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Verifying)?;
        if self.read_back_count != 0 || !patch_touches_field(self.task.normalized_patch) {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Verifying,
                actual: self.status,
            });
        }
        self.status = EditSurfaceStatus::VerificationFailed;
        self.read_back_count = 1;
        self.read_back_field_value = Some(self.expected_field_value + 1);
        self.read_back_target_exists = Some(expected_existence_after_apply(self.task.crud));
        self.touched_field_read_back_exact = false;
        self.target_existence_read_back_exact = true;
        Ok(())
    }

    /// Accepts exact field and existence read-back.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless both witnesses match.
    pub fn accept_exact_read_back(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Verifying)?;
        if self.read_back_count != 1
            || !self.touched_field_read_back_exact
            || !self.target_existence_read_back_exact
            || self.read_back_field_value != Some(self.expected_field_value)
            || self.read_back_target_exists != Some(expected_existence_after_apply(self.task.crud))
        {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Verifying,
                actual: self.status,
            });
        }
        self.status = EditSurfaceStatus::Verified;
        Ok(())
    }

    /// Publishes one canonical revision only when the request mutated the target.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Verified with exact
    /// read-back.
    pub fn commit(&mut self) -> Result<i64, EditSurfaceError> {
        self.require(EditSurfaceStatus::Verified)?;
        let approved = self.approved_binding.as_ref();
        let applied = self.applied_binding.as_ref();
        if approved != Some(&self.task)
            || applied != Some(&self.task)
            || !self.touched_field_read_back_exact
            || !self.target_existence_read_back_exact
            || self.canonical_commit_count != 0
        {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Verified,
                actual: self.status,
            });
        }
        self.status = EditSurfaceStatus::Committed;
        if request_mutates_target(self) {
            self.canonical_head_revision += 1;
            self.canonical_commit_count += 1;
        }
        self.commit_binding = Some(self.task.clone());
        Ok(self.canonical_head_revision)
    }

    /// Exact committed replay is mutation-free.
    ///
    /// # Errors
    ///
    /// Returns [`EditSurfaceError::UnexpectedStatus`] unless Committed.
    pub fn replay_committed(&mut self) -> Result<(), EditSurfaceError> {
        self.require(EditSurfaceStatus::Committed)?;
        if self.commit_binding.as_ref() != Some(&self.task) {
            return Err(EditSurfaceError::UnexpectedStatus {
                expected: EditSurfaceStatus::Committed,
                actual: self.status,
            });
        }
        self.replay_count += 1;
        Ok(())
    }

    fn require(&self, expected: EditSurfaceStatus) -> Result<(), EditSurfaceError> {
        if self.status == expected {
            Ok(())
        } else {
            Err(EditSurfaceError::UnexpectedStatus {
                expected,
                actual: self.status,
            })
        }
    }
}

/// Normalizes a raw patch against the current and default field values.
#[must_use]
pub fn normalize_patch(
    input: PatchInput,
    current_value: i64,
    default_value: i64,
    supplied: i64,
) -> NormalizedPatch {
    match input {
        PatchInput::PatchAbsent => NormalizedPatch::FieldUntouched,
        PatchInput::PatchExplicitNoOp => NormalizedPatch::NormalizedNoOp,
        PatchInput::PatchReset if current_value == default_value => NormalizedPatch::NormalizedNoOp,
        PatchInput::PatchReset => NormalizedPatch::ResetToDefault,
        PatchInput::PatchValue if supplied == current_value => NormalizedPatch::NormalizedNoOp,
        PatchInput::PatchValue => NormalizedPatch::SetCanonicalValue,
    }
}

/// Expected stored value after applying a normalized patch.
#[must_use]
pub fn expected_value_for(
    normalized: NormalizedPatch,
    current_value: i64,
    default_value: i64,
    supplied: i64,
) -> i64 {
    match normalized {
        NormalizedPatch::ResetToDefault => default_value,
        NormalizedPatch::SetCanonicalValue => supplied,
        _ => current_value,
    }
}

#[must_use]
fn field_is_known(field_class: FieldClass) -> bool {
    field_class != FieldClass::UnknownField
}

#[must_use]
fn patch_touches_field(normalized: NormalizedPatch) -> bool {
    matches!(
        normalized,
        NormalizedPatch::ResetToDefault | NormalizedPatch::SetCanonicalValue
    )
}

fn request_mutates_target(session: &EditSurfaceSession) -> bool {
    matches!(session.task.crud, CrudKind::Create | CrudKind::Delete)
        || (session.task.crud == CrudKind::Update
            && patch_touches_field(session.task.normalized_patch))
}

fn expected_existence_after_apply(crud: CrudKind) -> bool {
    crud != CrudKind::Delete
}

fn ownership_accepts(session: &EditSurfaceSession) -> bool {
    if session.task.crud == CrudKind::Create {
        session.ownership == Ownership::OwnershipNotApplicable
    } else {
        session.ownership == Ownership::Managed
    }
}

fn lock_accepts(session: &EditSurfaceSession) -> bool {
    if session.task.crud == CrudKind::Create {
        session.target_lock == TargetLock::LockNotApplicable
    } else {
        session.target_lock == TargetLock::Unlocked
    }
}

fn target_existence_accepts(session: &EditSurfaceSession) -> bool {
    if session.task.crud == CrudKind::Create {
        !session.observed_target_exists
    } else {
        session.observed_target_exists
    }
}

fn observation_is_fresh(session: &EditSurfaceSession) -> bool {
    session.observation == Availability::Available
        && session.observed_canonical_head_revision == session.canonical_head_revision
        && session.observed_source_id == session.source_id
        && session.observed_source_revision == session.source_revision
        && session.observed_source_fingerprint == session.source_fingerprint
        && session.observed_target_fingerprint == session.target_fingerprint
        && session.observed_target_exists == session.target_exists
        && session.observed_surface_revision == session.surface_revision
}

fn capability_accepts(session: &EditSurfaceSession) -> bool {
    session.capability == Availability::Available
        && session.capability_family == session.task.family
        && session.capability_crud == session.task.crud
        && session.capability_supports_field
}

fn schema_accepts(session: &EditSurfaceSession) -> bool {
    session.schema == Availability::Available
        && session.schema_family == session.task.family
        && session.schema_covers_field
        && field_is_known(session.task.field_class)
}

fn descriptor_accepts(session: &EditSurfaceSession) -> bool {
    session.descriptor == Availability::Available
        && session.descriptor_family == session.task.family
        && session.descriptor_covers_field
        && field_is_known(session.task.field_class)
}

fn admission_accepts(session: &EditSurfaceSession) -> bool {
    session.normalized_patch != NormalizedPatch::NotNormalized
        && capability_accepts(session)
        && schema_accepts(session)
        && descriptor_accepts(session)
        && observation_is_fresh(session)
        && target_existence_accepts(session)
        && ownership_accepts(session)
        && lock_accepts(session)
}

fn admission_failure(session: &EditSurfaceSession) -> EditSurfaceRejectReason {
    if session.capability != Availability::Available
        || session.capability_family != session.task.family
        || session.capability_crud != session.task.crud
        || !session.capability_supports_field
    {
        EditSurfaceRejectReason::UnsupportedCapability
    } else if session.schema != Availability::Available {
        EditSurfaceRejectReason::MissingSchema
    } else if session.schema_family != session.task.family
        || !session.schema_covers_field
        || !field_is_known(session.task.field_class)
    {
        EditSurfaceRejectReason::SchemaDoesNotCoverField
    } else if session.descriptor != Availability::Available {
        EditSurfaceRejectReason::MissingDescriptor
    } else if session.descriptor_family != session.task.family {
        EditSurfaceRejectReason::DescriptorFamilyMismatch
    } else if !session.descriptor_covers_field || !field_is_known(session.task.field_class) {
        EditSurfaceRejectReason::DescriptorDoesNotCoverField
    } else if session.observation != Availability::Available {
        EditSurfaceRejectReason::ObservationUnavailable
    } else if !observation_is_fresh(session) {
        EditSurfaceRejectReason::SourceBindingStale
    } else if !target_existence_accepts(session) {
        EditSurfaceRejectReason::TargetExistenceMismatch
    } else if !ownership_accepts(session) {
        EditSurfaceRejectReason::UnmanagedTarget
    } else if !lock_accepts(session) {
        EditSurfaceRejectReason::LockedTarget
    } else {
        EditSurfaceRejectReason::NoRejection
    }
}

fn plan_binding_is_fresh(session: &EditSurfaceSession) -> bool {
    session.task.canonical_base_revision == session.canonical_head_revision
        && session.task.source_id == session.source_id
        && session.task.source_revision == session.source_revision
        && session.task.source_fingerprint == session.source_fingerprint
        && session.task.target_fingerprint == session.target_fingerprint
        && session.task.surface_revision == session.surface_revision
        && session.task.capability_digest == session.capability_digest
        && session.task.schema_digest == session.schema_digest
        && session.task.descriptor_digest == session.descriptor_digest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reject_from(error: EditSurfaceError) -> EditSurfaceRejectReason {
        match error {
            EditSurfaceError::Rejected(reason) => reason,
            EditSurfaceError::UnexpectedStatus { expected, actual } => {
                panic!("expected modeled reject, got unexpected {actual:?} (wanted {expected:?})")
            }
        }
    }

    fn normalized_observed(session: &mut EditSurfaceSession) {
        session.normalize().expect("normalize");
        session.observe_read_only().expect("observe");
    }

    fn through_approve(session: &mut EditSurfaceSession) -> i64 {
        normalized_observed(session);
        session.admit().expect("admit");
        let digest = session.stage_preview().expect("preview");
        session.approve_exact(digest).expect("approve");
        digest
    }

    fn through_commit(session: &mut EditSurfaceSession) {
        through_approve(session);
        session.apply_approved().expect("apply");
        session.record_exact_read_back().expect("readback");
        session.accept_exact_read_back().expect("accept");
        session.commit().expect("commit");
    }

    #[test]
    fn value_patch_happy_path_advances_revision_after_exact_readback() {
        let mut session = EditSurfaceSession::initial();
        through_commit(&mut session);
        session.replay_committed().expect("replay");
        assert_eq!(session.status, EditSurfaceStatus::Committed);
        assert_eq!(session.normalized_patch, NormalizedPatch::SetCanonicalValue);
        assert_eq!(session.current_field_value, 25);
        assert_eq!(session.target_mutation_count, 1);
        assert_eq!(session.canonical_commit_count, 1);
        assert_eq!(
            session.canonical_head_revision,
            INITIAL_CANONICAL_REVISION + 1
        );
        assert_eq!(session.replay_count, 1);
        assert_eq!(
            session.untouched_known_field_value,
            INITIAL_UNTOUCHED_KNOWN_FIELD
        );
        assert_eq!(session.unknown_field_digest, INITIAL_UNKNOWN_FIELD_DIGEST);
    }

    #[test]
    fn absent_patch_is_untouched_and_does_not_advance_revision() {
        let mut session = EditSurfaceSession::initial();
        session
            .set_patch_input(PatchInput::PatchAbsent)
            .expect("patch");
        through_commit(&mut session);
        assert_eq!(session.normalized_patch, NormalizedPatch::FieldUntouched);
        assert_eq!(session.current_field_value, INITIAL_FIELD_VALUE);
        assert_eq!(session.target_mutation_count, 0);
        assert_eq!(session.canonical_commit_count, 0);
        assert_eq!(session.canonical_head_revision, INITIAL_CANONICAL_REVISION);
    }

    #[test]
    fn reset_patch_uses_descriptor_default() {
        let mut session = EditSurfaceSession::initial();
        session
            .set_patch_input(PatchInput::PatchReset)
            .expect("patch");
        through_commit(&mut session);
        assert_eq!(session.normalized_patch, NormalizedPatch::ResetToDefault);
        assert_eq!(session.current_field_value, DEFAULT_FIELD_VALUE);
        assert_eq!(session.target_mutation_count, 1);
        assert_eq!(session.canonical_commit_count, 1);
    }

    #[test]
    fn equal_value_and_explicit_noop_do_not_mutate() {
        let mut equal = EditSurfaceSession::initial();
        equal
            .set_supplied_value(INITIAL_FIELD_VALUE)
            .expect("value");
        through_commit(&mut equal);
        assert_eq!(equal.normalized_patch, NormalizedPatch::NormalizedNoOp);
        assert_eq!(equal.target_mutation_count, 0);
        assert_eq!(equal.canonical_commit_count, 0);

        let mut explicit = EditSurfaceSession::initial();
        explicit
            .set_patch_input(PatchInput::PatchExplicitNoOp)
            .expect("patch");
        through_commit(&mut explicit);
        assert_eq!(explicit.normalized_patch, NormalizedPatch::NormalizedNoOp);
        assert_eq!(explicit.target_mutation_count, 0);
    }

    #[test]
    fn each_admission_reject_reason_fail_closes_before_apply() {
        type Fixture = fn(&mut EditSurfaceSession);
        let cases: [(&str, Fixture, EditSurfaceRejectReason); 10] = [
            (
                "capability",
                |s| s.capability = Availability::Unavailable,
                EditSurfaceRejectReason::UnsupportedCapability,
            ),
            (
                "schema",
                |s| s.schema = Availability::Unavailable,
                EditSurfaceRejectReason::MissingSchema,
            ),
            (
                "descriptor",
                |s| s.descriptor = Availability::Unavailable,
                EditSurfaceRejectReason::MissingDescriptor,
            ),
            (
                "descriptor family",
                |s| s.descriptor_family = ItemFamily::ShapeItem,
                EditSurfaceRejectReason::DescriptorFamilyMismatch,
            ),
            (
                "unknown field",
                |s| {
                    s.task.field_class = FieldClass::UnknownField;
                    s.schema_covers_field = false;
                    s.descriptor_covers_field = false;
                },
                EditSurfaceRejectReason::SchemaDoesNotCoverField,
            ),
            (
                "unobserved",
                |_| {},
                EditSurfaceRejectReason::ObservationUnavailable,
            ),
            (
                "stale source",
                |s| {
                    s.observe_read_only().expect("observe");
                    s.source_fingerprint += 1;
                },
                EditSurfaceRejectReason::SourceBindingStale,
            ),
            (
                "existence",
                |s| s.target_exists = false,
                EditSurfaceRejectReason::TargetExistenceMismatch,
            ),
            (
                "unmanaged",
                |s| s.ownership = Ownership::Unmanaged,
                EditSurfaceRejectReason::UnmanagedTarget,
            ),
            (
                "locked",
                |s| s.target_lock = TargetLock::Locked,
                EditSurfaceRejectReason::LockedTarget,
            ),
        ];

        for (label, fixture, expected) in cases {
            let mut session = EditSurfaceSession::initial();
            fixture(&mut session);
            session.normalize().expect(label);
            if expected != EditSurfaceRejectReason::ObservationUnavailable
                && expected != EditSurfaceRejectReason::SourceBindingStale
            {
                session.observe_read_only().expect(label);
            }
            let reason = reject_from(session.admit().expect_err(label));
            assert_eq!(reason, expected, "{label}");
            assert_eq!(session.status, EditSurfaceStatus::Rejected, "{label}");
            assert_eq!(session.application_count, 0, "{label}");
            assert_eq!(session.target_mutation_count, 0, "{label}");
            assert_eq!(session.canonical_commit_count, 0, "{label}");
            assert!(session.applied_binding.is_none(), "{label}");
            assert_eq!(
                session.untouched_known_field_value, INITIAL_UNTOUCHED_KNOWN_FIELD,
                "{label}"
            );
            assert_eq!(
                session.unknown_field_digest, INITIAL_UNKNOWN_FIELD_DIGEST,
                "{label}"
            );
        }
    }

    #[test]
    fn mismatched_approval_digest_and_readback_never_commit() {
        let mut digest = EditSurfaceSession::initial();
        normalized_observed(&mut digest);
        digest.admit().expect("admit");
        let sealed = digest.stage_preview().expect("preview");
        let reason = reject_from(digest.approve_exact(sealed + 1).expect_err("mismatch"));
        assert_eq!(reason, EditSurfaceRejectReason::ApprovalDigestMismatch);
        assert_eq!(digest.application_count, 0);

        let mut readback = EditSurfaceSession::initial();
        through_approve(&mut readback);
        readback.apply_approved().expect("apply");
        readback
            .record_mismatched_touched_field_read_back()
            .expect("mismatch");
        assert_eq!(readback.status, EditSurfaceStatus::VerificationFailed);
        assert!(!readback.touched_field_read_back_exact);
        assert_eq!(readback.canonical_commit_count, 0);
        assert!(readback.commit_binding.is_none());
    }

    #[test]
    fn source_drift_conflicts_and_surface_expansion_invalidates_approval() {
        let mut drift = EditSurfaceSession::initial();
        through_approve(&mut drift);
        drift.external_source_drift_after_approval().expect("drift");
        drift.mark_conflict().expect("conflict");
        assert_eq!(drift.status, EditSurfaceStatus::Conflicted);
        assert_eq!(drift.application_count, 0);
        assert_eq!(drift.canonical_commit_count, 0);

        let mut expand = EditSurfaceSession::initial();
        through_approve(&mut expand);
        let old = expand.approved_binding.clone().expect("approved");
        expand.expand_surface_after_approval().expect("expand");
        assert_eq!(expand.status, EditSurfaceStatus::Draft);
        assert_eq!(expand.observation, Availability::Unavailable);
        assert_eq!(expand.invalidated_binding.as_ref(), Some(&old));
        assert!(old.surface_revision < expand.surface_revision);
        assert!(old.capability_digest < expand.capability_digest);
        assert!(old.schema_digest < expand.schema_digest);
        assert!(old.descriptor_digest < expand.descriptor_digest);
        assert_eq!(expand.application_count, 0);

        through_commit(&mut expand);
        assert_eq!(expand.status, EditSurfaceStatus::Committed);
        assert_ne!(
            expand
                .applied_binding
                .as_ref()
                .map(|binding| binding.surface_revision),
            Some(old.surface_revision)
        );
        assert_eq!(
            expand
                .applied_binding
                .as_ref()
                .map(|binding| binding.surface_revision),
            Some(expand.surface_revision)
        );
        assert_eq!(expand.canonical_commit_count, 1);
    }

    #[test]
    fn create_and_delete_use_modeled_existence_and_lock_admission() {
        let mut create = EditSurfaceSession::initial();
        create
            .configure_request(
                ItemFamily::ManagedVoiceItem,
                CrudKind::Create,
                FieldClass::AudioParametersField,
            )
            .expect("configure");
        through_approve(&mut create);
        assert_eq!(create.task.crud, CrudKind::Create);
        assert!(!create.observed_target_exists);
        assert_eq!(create.ownership, Ownership::OwnershipNotApplicable);
        assert_eq!(create.target_lock, TargetLock::LockNotApplicable);

        let mut delete = EditSurfaceSession::initial();
        delete
            .configure_request(
                ItemFamily::ManagedVoiceItem,
                CrudKind::Delete,
                FieldClass::AudioParametersField,
            )
            .expect("configure");
        through_approve(&mut delete);
        assert_eq!(delete.task.crud, CrudKind::Delete);
        assert!(delete.observed_target_exists);
        assert_eq!(delete.ownership, Ownership::Managed);
        assert_eq!(delete.target_lock, TargetLock::Unlocked);
    }

    #[test]
    fn read_only_observation_does_not_mutate() {
        let mut session = EditSurfaceSession::initial();
        session.observe_read_only().expect("one");
        session.observe_read_only().expect("two");
        assert_eq!(session.status, EditSurfaceStatus::Draft);
        assert_eq!(session.observation_count, 2);
        assert_eq!(session.source_fingerprint, INITIAL_SOURCE_FINGERPRINT);
        assert_eq!(session.target_fingerprint, INITIAL_TARGET_FINGERPRINT);
        assert_eq!(session.target_mutation_count, 0);
        assert_eq!(session.canonical_head_revision, INITIAL_CANONICAL_REVISION);
    }
}
