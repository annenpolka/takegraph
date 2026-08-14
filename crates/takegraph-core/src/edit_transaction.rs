//! Heterogeneous edit-transaction envelope from `ymm4EditTransactionProtocol`.
//!
//! One digest seals an ordered 1..=128 descriptor list. Preparation may finish
//! in any order; apply/rollback order is only the sealed descriptor order.
//! This is the portable envelope, not a live mixed YMM apply.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{CanonicalError, canonical_sha256};

/// Inclusive production bound from the protocol.
pub const EDIT_TRANSACTION_MAX_OPERATIONS: usize = 128;
const TRANSACTION_ENDPOINT_REVISION: u32 = 2;

/// Semantic families the envelope can carry. The lifecycle never branches on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticFamily {
    ItemLifecycleFamily,
    TransformVisibilityFamily,
    TextStyleFamily,
    AudioPlaybackFamily,
    EffectChainFamily,
    CharacterVoiceFamily,
    TimelineStructureFamily,
    FutureExtensionFamily,
}

/// Ordinary edits are project-local. Global settings need a project operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetScope {
    ProjectLocalTarget,
    GlobalProjectSettingTarget,
}

/// Unified transaction vs a caller chaining legacy kind-specific endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointMode {
    UnifiedTransactionEndpoint,
    LegacyKindEndpoints,
}

/// Staging reject reasons. All fail before preparation or mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum TransactionRejectReason {
    #[error("target is unmanaged")]
    UnmanagedTarget,
    #[error("target is locked")]
    LockedTarget,
    #[error("global project-setting targets are outside this envelope")]
    GlobalSettingTarget,
    #[error("operation count must be between 1 and 128")]
    OperationCountOutOfBounds,
    #[error("write footprints overlap")]
    OverlappingWriteFootprint,
    #[error("dependency order is not the sealed descriptor order")]
    IllegalDependencyOrder,
    #[error("legacy kind-specific endpoints cannot form one atomic transaction")]
    PseudoBatchAcrossEndpoints,
}

/// One opaque operation descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionOperation {
    pub index: u32,
    pub family: SemanticFamily,
    pub kind_digest: String,
    pub write_footprint: String,
    pub depends_on: Vec<u32>,
    pub target_managed: bool,
    pub target_unlocked: bool,
    pub target_scope: TargetScope,
    pub required_capability: u32,
}

/// Inputs required to seal one heterogeneous transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditTransactionRequest {
    pub task_id: String,
    pub project_id: String,
    pub scene_id: String,
    pub base_revision: u64,
    pub source_fingerprint: String,
    pub capability_revision: u64,
    pub endpoint_mode: EndpointMode,
    pub operations: Vec<TransactionOperation>,
}

/// Sealed, digest-bound heterogeneous transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditTransactionPlan {
    pub task_id: String,
    pub project_id: String,
    pub scene_id: String,
    pub base_revision: u64,
    pub source_fingerprint: String,
    pub capability_revision: u64,
    pub endpoint_mode: EndpointMode,
    pub endpoint_revision: u32,
    pub operations: Vec<TransactionOperation>,
    pub plan_digest: String,
}

/// Later staged task freshness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaterTaskStatus {
    LaterAbsent,
    LaterPreviewable,
    LaterApproved,
    LaterQueued,
    LaterStale,
}

/// Session status for one sealed envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStatus {
    Draft,
    Preparing,
    Previewable,
    Approved,
    Applying,
    Verifying,
    Verified,
    Committed,
    Rejected,
}

/// Read-back witness for one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadbackEvidence {
    ReadbackPending,
    ReadbackExact,
    ReadbackMismatch,
}

/// Errors around the envelope lifecycle.
#[derive(Debug, Error)]
pub enum EditTransactionError {
    #[error(transparent)]
    Rejected(#[from] TransactionRejectReason),
    #[error("transaction is in an unexpected status")]
    UnexpectedStatus,
    #[error("operation {0} is not part of the sealed plan")]
    UnknownOperation(u32),
    #[error("apply/rollback order must follow the sealed descriptor list")]
    ApplyOrder,
    #[error("canonical commit requires exact read-back of every operation")]
    IncompleteReadback,
    #[error("later task must be re-previewed against the new source")]
    LaterTaskStale,
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

/// Executable envelope session. Starts as an unsealed draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditTransactionSession {
    pub status: TransactionStatus,
    pub head_revision: u64,
    pub canonical_commit_count: u64,
    pub plan: Option<EditTransactionPlan>,
    pub approved_digest: Option<String>,
    pub prepared: BTreeSet<u32>,
    pub preparation_order: Vec<u32>,
    pub applied_prefix: usize,
    pub apply_order: Vec<u32>,
    pub readbacks: Vec<ReadbackEvidence>,
    pub later_status: LaterTaskStatus,
    pub later_digest: Option<String>,
    pub later_base_revision: Option<u64>,
}

impl EditTransactionPlan {
    /// Seals one heterogeneous transaction or fail-closes before mutation.
    ///
    /// # Errors
    ///
    /// Returns a modeled reject reason for unmanaged/locked/global targets,
    /// count bounds, overlapping writes, illegal deps, or a legacy pseudo-batch.
    pub fn seal(request: EditTransactionRequest) -> Result<Self, EditTransactionError> {
        if request.endpoint_mode == EndpointMode::LegacyKindEndpoints {
            return Err(TransactionRejectReason::PseudoBatchAcrossEndpoints.into());
        }
        if request.operations.is_empty()
            || request.operations.len() > EDIT_TRANSACTION_MAX_OPERATIONS
        {
            return Err(TransactionRejectReason::OperationCountOutOfBounds.into());
        }
        let mut footprints = BTreeSet::new();
        let mut seen_index = BTreeSet::new();
        for (position, operation) in request.operations.iter().enumerate() {
            if usize::try_from(operation.index).ok() != Some(position + 1)
                || !seen_index.insert(operation.index)
            {
                return Err(TransactionRejectReason::IllegalDependencyOrder.into());
            }
            if !operation.target_managed {
                return Err(TransactionRejectReason::UnmanagedTarget.into());
            }
            if !operation.target_unlocked {
                return Err(TransactionRejectReason::LockedTarget.into());
            }
            if operation.target_scope == TargetScope::GlobalProjectSettingTarget {
                return Err(TransactionRejectReason::GlobalSettingTarget.into());
            }
            if operation.required_capability == 0 || operation.write_footprint.trim().is_empty() {
                return Err(TransactionRejectReason::OverlappingWriteFootprint.into());
            }
            if !footprints.insert(operation.write_footprint.as_str()) {
                return Err(TransactionRejectReason::OverlappingWriteFootprint.into());
            }
            if operation
                .depends_on
                .iter()
                .any(|dependency| *dependency >= operation.index)
            {
                return Err(TransactionRejectReason::IllegalDependencyOrder.into());
            }
        }
        let mut plan = Self {
            task_id: request.task_id,
            project_id: request.project_id,
            scene_id: request.scene_id,
            base_revision: request.base_revision,
            source_fingerprint: request.source_fingerprint,
            capability_revision: request.capability_revision,
            endpoint_mode: request.endpoint_mode,
            endpoint_revision: TRANSACTION_ENDPOINT_REVISION,
            operations: request.operations,
            plan_digest: String::new(),
        };
        plan.plan_digest = plan.compute_digest()?;
        Ok(plan)
    }

    fn compute_digest(&self) -> Result<String, CanonicalError> {
        canonical_sha256(
            "takegraph-edit-transaction-plan-v1",
            &(
                &self.task_id,
                &self.project_id,
                &self.scene_id,
                self.base_revision,
                &self.source_fingerprint,
                self.capability_revision,
                self.endpoint_mode,
                self.endpoint_revision,
                &self.operations,
            ),
        )
    }
}

impl EditTransactionSession {
    /// Empty draft at the supplied head revision.
    #[must_use]
    pub fn draft(head_revision: u64) -> Self {
        Self {
            status: TransactionStatus::Draft,
            head_revision,
            canonical_commit_count: 0,
            plan: None,
            approved_digest: None,
            prepared: BTreeSet::new(),
            preparation_order: Vec::new(),
            applied_prefix: 0,
            apply_order: Vec::new(),
            readbacks: Vec::new(),
            later_status: LaterTaskStatus::LaterAbsent,
            later_digest: None,
            later_base_revision: None,
        }
    }

    /// Accepts an already-sealed plan and opens preparation.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::UnexpectedStatus`] unless Draft.
    pub fn accept_sealed_plan(
        &mut self,
        plan: EditTransactionPlan,
    ) -> Result<(), EditTransactionError> {
        if self.status != TransactionStatus::Draft {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        let count = plan.operations.len();
        self.plan = Some(plan);
        self.status = TransactionStatus::Preparing;
        self.readbacks = vec![ReadbackEvidence::ReadbackPending; count];
        Ok(())
    }

    /// Records preparation of one operation. Order may be arbitrary.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::UnknownOperation`] for an index outside
    /// the sealed list.
    pub fn prepare(&mut self, index: u32) -> Result<(), EditTransactionError> {
        if self.status != TransactionStatus::Preparing {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        let plan = self
            .plan
            .as_ref()
            .ok_or(EditTransactionError::UnexpectedStatus)?;
        if !plan
            .operations
            .iter()
            .any(|operation| operation.index == index)
        {
            return Err(EditTransactionError::UnknownOperation(index));
        }
        if self.prepared.insert(index) {
            self.preparation_order.push(index);
        }
        Ok(())
    }

    /// Seals preview once every operation is prepared. Digest is order-independent.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::UnexpectedStatus`] until every operation
    /// is prepared.
    pub fn seal_preview(&mut self) -> Result<String, EditTransactionError> {
        if self.status != TransactionStatus::Preparing {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        let plan = self
            .plan
            .as_ref()
            .ok_or(EditTransactionError::UnexpectedStatus)?;
        if self.prepared.len() != plan.operations.len() {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        self.status = TransactionStatus::Previewable;
        Ok(plan.plan_digest.clone())
    }

    /// Approves the exact sealed digest against the current head.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::UnexpectedStatus`] on digest/head mismatch.
    pub fn approve(&mut self, plan_digest: &str) -> Result<(), EditTransactionError> {
        if self.status != TransactionStatus::Previewable {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        let plan = self
            .plan
            .as_ref()
            .ok_or(EditTransactionError::UnexpectedStatus)?;
        if plan.plan_digest != plan_digest || plan.base_revision != self.head_revision {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        self.status = TransactionStatus::Approved;
        self.approved_digest = Some(plan.plan_digest.clone());
        Ok(())
    }

    /// Stages a later task against the same source. It becomes stale after commit.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::UnexpectedStatus`] unless a main task is
    /// already approved.
    pub fn stage_later_task(
        &mut self,
        digest: impl Into<String>,
    ) -> Result<(), EditTransactionError> {
        if !matches!(
            self.status,
            TransactionStatus::Approved | TransactionStatus::Applying
        ) {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        self.later_status = LaterTaskStatus::LaterPreviewable;
        self.later_digest = Some(digest.into());
        self.later_base_revision = Some(self.head_revision);
        Ok(())
    }

    /// Approves the later task only while the source is still this head.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::LaterTaskStale`] after the main commit.
    pub fn approve_later_task(&mut self) -> Result<(), EditTransactionError> {
        if self.later_status == LaterTaskStatus::LaterStale {
            return Err(EditTransactionError::LaterTaskStale);
        }
        if self.later_status != LaterTaskStatus::LaterPreviewable
            || self.later_base_revision != Some(self.head_revision)
        {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        self.later_status = LaterTaskStatus::LaterApproved;
        Ok(())
    }

    /// Applies the next sealed operation. Preparation order is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::ApplyOrder`] if the caller skips ahead.
    pub fn apply_next(&mut self) -> Result<u32, EditTransactionError> {
        if !matches!(
            self.status,
            TransactionStatus::Approved | TransactionStatus::Applying
        ) {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        let plan = self
            .plan
            .as_ref()
            .ok_or(EditTransactionError::UnexpectedStatus)?;
        if self.approved_digest.as_deref() != Some(plan.plan_digest.as_str()) {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        if self.applied_prefix >= plan.operations.len() {
            return Err(EditTransactionError::ApplyOrder);
        }
        let index = plan.operations[self.applied_prefix].index;
        self.status = if self.applied_prefix + 1 == plan.operations.len() {
            TransactionStatus::Verifying
        } else {
            TransactionStatus::Applying
        };
        self.applied_prefix += 1;
        self.apply_order.push(index);
        Ok(index)
    }

    /// Records read-back for one applied operation.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::UnknownOperation`] for an unapplied index.
    pub fn record_readback(
        &mut self,
        index: u32,
        evidence: ReadbackEvidence,
    ) -> Result<(), EditTransactionError> {
        if self.status != TransactionStatus::Verifying {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        let slot = usize::try_from(index)
            .ok()
            .and_then(|value| value.checked_sub(1))
            .ok_or(EditTransactionError::UnknownOperation(index))?;
        if slot >= self.readbacks.len() || slot >= self.applied_prefix {
            return Err(EditTransactionError::UnknownOperation(index));
        }
        self.readbacks[slot] = evidence;
        Ok(())
    }

    /// Accepts the whole read-back set only when every operation is exact.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::IncompleteReadback`] on any pending or
    /// mismatched witness.
    pub fn accept_whole_readback(&mut self) -> Result<(), EditTransactionError> {
        if self.status != TransactionStatus::Verifying {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        if !self
            .readbacks
            .iter()
            .all(|evidence| *evidence == ReadbackEvidence::ReadbackExact)
        {
            return Err(EditTransactionError::IncompleteReadback);
        }
        self.status = TransactionStatus::Verified;
        Ok(())
    }

    /// Publishes one canonical revision and marks any later task stale.
    ///
    /// # Errors
    ///
    /// Returns [`EditTransactionError::IncompleteReadback`] unless Verified.
    pub fn commit(&mut self) -> Result<u64, EditTransactionError> {
        if self.status != TransactionStatus::Verified {
            return Err(EditTransactionError::IncompleteReadback);
        }
        if self.canonical_commit_count != 0 {
            return Err(EditTransactionError::UnexpectedStatus);
        }
        self.status = TransactionStatus::Committed;
        self.head_revision += 1;
        self.canonical_commit_count = 1;
        if self.later_status != LaterTaskStatus::LaterAbsent {
            self.later_status = LaterTaskStatus::LaterStale;
        }
        Ok(self.head_revision)
    }
}

/// Representative three-family batch used by the protocol tests.
#[must_use]
pub fn representative_operations() -> Vec<TransactionOperation> {
    vec![
        TransactionOperation {
            index: 1,
            family: SemanticFamily::ItemLifecycleFamily,
            kind_digest: "kind-lifecycle".into(),
            write_footprint: "entity:one".into(),
            depends_on: Vec::new(),
            target_managed: true,
            target_unlocked: true,
            target_scope: TargetScope::ProjectLocalTarget,
            required_capability: 11,
        },
        TransactionOperation {
            index: 2,
            family: SemanticFamily::TransformVisibilityFamily,
            kind_digest: "kind-transform".into(),
            write_footprint: "entity:two".into(),
            depends_on: vec![1],
            target_managed: true,
            target_unlocked: true,
            target_scope: TargetScope::ProjectLocalTarget,
            required_capability: 22,
        },
        TransactionOperation {
            index: 3,
            family: SemanticFamily::AudioPlaybackFamily,
            kind_digest: "kind-audio".into(),
            write_footprint: "entity:three".into(),
            depends_on: vec![1],
            target_managed: true,
            target_unlocked: true,
            target_scope: TargetScope::ProjectLocalTarget,
            required_capability: 33,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        operations: Vec<TransactionOperation>,
        mode: EndpointMode,
    ) -> EditTransactionRequest {
        EditTransactionRequest {
            task_id: "task-main".into(),
            project_id: "project".into(),
            scene_id: "scene".into(),
            base_revision: 10,
            source_fingerprint: "sha256:source".into(),
            capability_revision: 7,
            endpoint_mode: mode,
            operations,
        }
    }

    fn sealed() -> EditTransactionPlan {
        EditTransactionPlan::seal(request(
            representative_operations(),
            EndpointMode::UnifiedTransactionEndpoint,
        ))
        .expect("seal")
    }

    fn reject_of(error: &EditTransactionError) -> TransactionRejectReason {
        match error {
            EditTransactionError::Rejected(reason) => *reason,
            EditTransactionError::UnexpectedStatus
            | EditTransactionError::UnknownOperation(_)
            | EditTransactionError::ApplyOrder
            | EditTransactionError::IncompleteReadback
            | EditTransactionError::LaterTaskStale
            | EditTransactionError::Canonical(_) => {
                panic!("expected reject, got {error}")
            }
        }
    }

    fn reject_seal(
        operations: Vec<TransactionOperation>,
        mode: EndpointMode,
    ) -> TransactionRejectReason {
        reject_of(&EditTransactionPlan::seal(request(operations, mode)).expect_err("reject"))
    }

    #[test]
    fn reverse_preparation_still_applies_in_sealed_order_and_commits_once() {
        let plan = sealed();
        let digest = plan.plan_digest.clone();
        let mut session = EditTransactionSession::draft(10);
        session.accept_sealed_plan(plan).expect("accept");
        session.prepare(3).expect("p3");
        session.prepare(2).expect("p2");
        session.prepare(1).expect("p1");
        assert_eq!(session.seal_preview().expect("preview"), digest);
        session.approve(&digest).expect("approve");
        assert_eq!(session.apply_next().expect("a1"), 1);
        assert_eq!(session.apply_next().expect("a2"), 2);
        assert_eq!(session.apply_next().expect("a3"), 3);
        assert_eq!(session.apply_order, [1, 2, 3]);
        assert_eq!(session.preparation_order, [3, 2, 1]);
        session
            .record_readback(3, ReadbackEvidence::ReadbackExact)
            .expect("r3");
        session
            .record_readback(1, ReadbackEvidence::ReadbackExact)
            .expect("r1");
        session
            .record_readback(2, ReadbackEvidence::ReadbackExact)
            .expect("r2");
        session.accept_whole_readback().expect("readback");
        assert_eq!(session.commit().expect("commit"), 11);
        assert_eq!(session.status, TransactionStatus::Committed);
        assert_eq!(session.canonical_commit_count, 1);
        assert_eq!(session.head_revision, 11);
    }

    #[test]
    fn later_staged_task_is_stale_after_commit_and_must_repreview() {
        let plan = sealed();
        let digest = plan.plan_digest.clone();
        let mut session = EditTransactionSession::draft(10);
        session.accept_sealed_plan(plan).expect("accept");
        for index in 1..=3 {
            session.prepare(index).expect("prep");
        }
        session.seal_preview().expect("preview");
        session.approve(&digest).expect("approve");
        session.stage_later_task("later-preview").expect("later");
        session.approve_later_task().expect("later approve");
        assert_eq!(session.later_status, LaterTaskStatus::LaterApproved);
        for _ in 0..3 {
            session.apply_next().expect("apply");
        }
        for index in 1..=3 {
            session
                .record_readback(index, ReadbackEvidence::ReadbackExact)
                .expect("rb");
        }
        session.accept_whole_readback().expect("accept");
        session.commit().expect("commit");
        assert_eq!(session.later_status, LaterTaskStatus::LaterStale);
        assert!(matches!(
            session.approve_later_task(),
            Err(EditTransactionError::LaterTaskStale)
        ));
    }

    #[test]
    fn envelope_rejects_unmanaged_locked_global_and_count() {
        let unified = EndpointMode::UnifiedTransactionEndpoint;
        let mut unmanaged = representative_operations();
        unmanaged[0].target_managed = false;
        assert_eq!(
            reject_seal(unmanaged, unified),
            TransactionRejectReason::UnmanagedTarget
        );
        let mut locked = representative_operations();
        locked[1].target_unlocked = false;
        assert_eq!(
            reject_seal(locked, unified),
            TransactionRejectReason::LockedTarget
        );
        let mut global = representative_operations();
        global[2].target_scope = TargetScope::GlobalProjectSettingTarget;
        assert_eq!(
            reject_seal(global, unified),
            TransactionRejectReason::GlobalSettingTarget
        );
        assert_eq!(
            reject_seal(Vec::new(), unified),
            TransactionRejectReason::OperationCountOutOfBounds
        );
        let too_many = (1..=129)
            .map(|index| TransactionOperation {
                index,
                family: SemanticFamily::FutureExtensionFamily,
                kind_digest: format!("k{index}"),
                write_footprint: format!("e{index}"),
                depends_on: Vec::new(),
                target_managed: true,
                target_unlocked: true,
                target_scope: TargetScope::ProjectLocalTarget,
                required_capability: 1,
            })
            .collect();
        assert_eq!(
            reject_seal(too_many, unified),
            TransactionRejectReason::OperationCountOutOfBounds
        );
    }

    #[test]
    fn envelope_rejects_overlap_illegal_deps_and_legacy_pseudo_batch() {
        let unified = EndpointMode::UnifiedTransactionEndpoint;
        let mut overlap = representative_operations();
        overlap[2].write_footprint = overlap[0].write_footprint.clone();
        assert_eq!(
            reject_seal(overlap, unified),
            TransactionRejectReason::OverlappingWriteFootprint
        );
        let mut deps = representative_operations();
        deps[0].depends_on = vec![2];
        assert_eq!(
            reject_seal(deps, unified),
            TransactionRejectReason::IllegalDependencyOrder
        );
        assert_eq!(
            reject_seal(
                representative_operations(),
                EndpointMode::LegacyKindEndpoints
            ),
            TransactionRejectReason::PseudoBatchAcrossEndpoints
        );
    }

    #[test]
    fn missing_readback_cannot_publish_a_revision() {
        let plan = sealed();
        let digest = plan.plan_digest.clone();
        let mut session = EditTransactionSession::draft(10);
        session.accept_sealed_plan(plan).expect("accept");
        for index in 1..=3 {
            session.prepare(index).expect("prep");
        }
        session.seal_preview().expect("preview");
        session.approve(&digest).expect("approve");
        for _ in 0..3 {
            session.apply_next().expect("apply");
        }
        session
            .record_readback(1, ReadbackEvidence::ReadbackExact)
            .expect("r1");
        session
            .record_readback(3, ReadbackEvidence::ReadbackExact)
            .expect("r3");
        assert!(matches!(
            session.accept_whole_readback(),
            Err(EditTransactionError::IncompleteReadback)
        ));
        assert!(matches!(
            session.commit(),
            Err(EditTransactionError::IncompleteReadback)
        ));
        assert_eq!(session.head_revision, 10);
        assert_eq!(session.canonical_commit_count, 0);
    }
}
