//! Common canonical-to-target mutation exclusion.
//!
//! A bridge operation and the durable publication of its verified receipt are
//! one service-level critical section. Route implementations keep the returned
//! guard alive from immediately before bridge I/O through durable finalization.

use uuid::Uuid;

use takegraph_core::{Patch, PatchError, PatchStatus, RevisionId};
use takegraph_node::{
    Ymm4NativeExtensionApplyRequest, Ymm4NativeExtensionApplyResponse, Ymm4NativeExtensionStatus,
    Ymm4OperationReceipt, Ymm4OperationStatus,
};

use crate::{DurableProjectStore, ExternalCommitRecord, ExternalMutationFence, ProjectStoreError};

/// Result of one canonical-to-YMM durable mutation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableExternalMutationOutcome {
    pub revision: RevisionId,
    /// True only when an already-published exact operation was recovered.
    pub canonical_replay: bool,
}

pub(crate) struct ExternalMutationScope {
    _fence: ExternalMutationFence,
    pub committed: Option<ExternalCommitRecord>,
    pub pending_replay: bool,
}

/// Acquires the project-wide external-mutation fence for an ordinary exporter.
///
/// The durable head is checked only after the OS lock has been acquired. This
/// closes the race where another exporter commits while this caller is waiting
/// for the lock. A metadata-detach reservation blocks every ordinary exporter,
/// including a task that reuses the reservation's operation ID; only the
/// detach recovery/finalization workflow is authorized to consume its durable
/// reservation. An exact ordinary-operation retry may reacquire its own fence;
/// [`DurableProjectStore::reserve_external_commit`] subsequently verifies all
/// durable evidence before any bridge I/O.
pub(crate) fn acquire_external_mutation_scope(
    store: &DurableProjectStore,
    operation_id: Uuid,
    expected_head: RevisionId,
) -> Result<ExternalMutationScope, ProjectStoreError> {
    let fence = store.acquire_external_mutation_fence(operation_id)?;
    let state = store.snapshot()?;
    if let Some(pending) = &state.pending_external_commit
        && pending.operation_id != operation_id
    {
        return Err(ProjectStoreError::ExternalCommitReserved(
            pending.operation_id,
        ));
    }
    if state.head != expected_head {
        return Err(ProjectStoreError::StaleRevision {
            expected: expected_head,
            actual: state.head,
        });
    }
    Ok(ExternalMutationScope {
        _fence: fence,
        committed: state.external_commits.get(&operation_id).cloned(),
        pending_replay: state
            .pending_external_commit
            .as_ref()
            .is_some_and(|pending| pending.operation_id == operation_id),
    })
}

/// Authorizes the original approved base for both an in-flight operation and
/// an exact task-file recovery after its durable commit was already published.
pub(crate) fn authorize_external_patch(patch: &Patch) -> Result<(), PatchError> {
    if patch.status == PatchStatus::Approved {
        return patch.authorize_commit(patch.base);
    }
    if patch.status != PatchStatus::Committed {
        return Err(PatchError::UnexpectedStatus {
            expected: PatchStatus::Approved,
            actual: patch.status,
        });
    }
    if patch.approved_digest.as_deref() != Some(patch.digest.as_str()) {
        return Err(PatchError::StaleApproval);
    }
    if patch.touches_hard_lock {
        return Err(PatchError::HardLock);
    }
    Ok(())
}

/// The bridge's request-bound rolled-back receipt is a safe reservation abort
/// proof only when the target fingerprint is exactly restored.
pub(crate) fn operation_receipt_proves_exact_rollback(
    receipt: &Ymm4OperationReceipt,
    operation_id: Uuid,
    request_digest: &str,
    project_id: &str,
    scene_id: &str,
    expected_fingerprint: &str,
) -> bool {
    operation_receipt_is_request_bound(
        receipt,
        operation_id,
        request_digest,
        project_id,
        scene_id,
        expected_fingerprint,
    ) && receipt.status == Ymm4OperationStatus::RolledBack
        && !receipt.verified
        && receipt.before_fingerprint == expected_fingerprint
        && receipt.after_fingerprint == receipt.before_fingerprint
        && receipt
            .error
            .as_ref()
            .is_some_and(|error| !error.trim().is_empty())
}

/// A request-bound terminal failure is also a safe abort proof when the
/// bridge attests that no target state changed and reports no applied items.
pub(crate) fn operation_receipt_proves_no_mutation(
    receipt: &Ymm4OperationReceipt,
    operation_id: Uuid,
    request_digest: &str,
    project_id: &str,
    scene_id: &str,
    expected_fingerprint: &str,
) -> bool {
    operation_receipt_is_request_bound(
        receipt,
        operation_id,
        request_digest,
        project_id,
        scene_id,
        expected_fingerprint,
    ) && receipt.status == Ymm4OperationStatus::Failed
        && !receipt.verified
        && receipt.before_fingerprint == expected_fingerprint
        && receipt.after_fingerprint == receipt.before_fingerprint
        && receipt.applied_items.is_empty()
        && receipt
            .error
            .as_ref()
            .is_some_and(|error| !error.trim().is_empty())
}

/// An apply-gate-serialized durable tombstone is the only safe absence proof
/// for a pending request whose earlier POST may still be delayed in transport.
pub(crate) fn operation_receipt_proves_not_started(
    receipt: &Ymm4OperationReceipt,
    operation_id: Uuid,
    request_digest: &str,
    project_id: &str,
    scene_id: &str,
    expected_fingerprint: &str,
) -> bool {
    operation_receipt_is_request_bound(
        receipt,
        operation_id,
        request_digest,
        project_id,
        scene_id,
        expected_fingerprint,
    ) && receipt.status == Ymm4OperationStatus::NotStarted
        && !receipt.verified
        && receipt.before_fingerprint == expected_fingerprint
        && receipt.after_fingerprint == receipt.before_fingerprint
        && receipt.applied_items.is_empty()
        && receipt
            .error
            .as_ref()
            .is_some_and(|error| !error.trim().is_empty())
}

pub(crate) fn operation_receipt_is_request_bound(
    receipt: &Ymm4OperationReceipt,
    operation_id: Uuid,
    request_digest: &str,
    project_id: &str,
    scene_id: &str,
    expected_fingerprint: &str,
) -> bool {
    receipt.operation_id == operation_id
        && receipt.request_digest == request_digest
        && receipt.project_id == project_id
        && receipt.scene_id == scene_id
        && receipt.expected_fingerprint == expected_fingerprint
        && receipt.before_fingerprint == expected_fingerprint
}

pub(crate) fn native_extension_receipt_proves_exact_rollback(
    receipt: &Ymm4NativeExtensionApplyResponse,
    request: &Ymm4NativeExtensionApplyRequest,
) -> bool {
    native_extension_receipt_is_request_bound(receipt, request)
        && receipt.status == Ymm4NativeExtensionStatus::RolledBack
        && !receipt.verified
        && receipt.before_fingerprint == request.expected_fingerprint
        && receipt.after_fingerprint == receipt.before_fingerprint
        && receipt.realizations.is_empty()
        && receipt
            .error
            .as_ref()
            .is_some_and(|error| !error.trim().is_empty())
}

pub(crate) fn native_extension_receipt_proves_no_mutation(
    receipt: &Ymm4NativeExtensionApplyResponse,
    request: &Ymm4NativeExtensionApplyRequest,
) -> bool {
    native_extension_receipt_is_request_bound(receipt, request)
        && receipt.status == Ymm4NativeExtensionStatus::Failed
        && !receipt.verified
        && receipt.before_fingerprint == request.expected_fingerprint
        && receipt.after_fingerprint == receipt.before_fingerprint
        && receipt.realizations.is_empty()
        && receipt
            .error
            .as_ref()
            .is_some_and(|error| !error.trim().is_empty())
}

pub(crate) fn native_extension_receipt_proves_not_started(
    receipt: &Ymm4NativeExtensionApplyResponse,
    request: &Ymm4NativeExtensionApplyRequest,
) -> bool {
    native_extension_receipt_is_request_bound(receipt, request)
        && receipt.status == Ymm4NativeExtensionStatus::NotStarted
        && !receipt.verified
        && receipt.before_fingerprint == request.expected_fingerprint
        && receipt.after_fingerprint == receipt.before_fingerprint
        && receipt.realizations.is_empty()
        && receipt
            .error
            .as_ref()
            .is_some_and(|error| !error.trim().is_empty())
}

pub(crate) fn native_extension_receipt_is_request_bound(
    receipt: &Ymm4NativeExtensionApplyResponse,
    request: &Ymm4NativeExtensionApplyRequest,
) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_digest == request.request_digest
        && receipt.project_id == request.project_id
        && receipt.scene_id == request.scene_id
        && receipt.descriptor_catalog_digest == request.descriptor_catalog_digest
        && receipt.driver_profile_digest == request.driver_profile_digest
        && receipt.before_fingerprint == request.expected_fingerprint
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, sync::mpsc, thread, time::Duration};

    use takegraph_core::{
        ChangeBudget, ManagedSemanticIdentity, NativeExtensionPlan, RevisionId, ScopeFingerprints,
        TargetIdentity,
    };
    use takegraph_node::{
        ManagedItemKind, Ymm4ManagedItem, Ymm4NativeExtensionApplyResponse,
        Ymm4NativeExtensionStatus, Ymm4OperationReceipt, Ymm4OperationStatus,
    };

    use super::*;
    use crate::VerifiedTargetBinding;
    use crate::project_store::{VerifiedExternalCommit, VerifiedManagedStateUpdate};

    fn test_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("takegraph-external-fence-{}", Uuid::new_v4()))
    }

    #[test]
    fn fence_serializes_routes_through_the_whole_canonical_scope() {
        let root = test_root();
        let store = DurableProjectStore::open(&root, "canonical-project").unwrap();
        let first_store = store.clone();
        let second_store = store.clone();
        let first_operation = Uuid::new_v4();
        let second_operation = Uuid::new_v4();
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();

        let first = thread::spawn(move || {
            let scope =
                acquire_external_mutation_scope(&first_store, first_operation, RevisionId(0))
                    .unwrap();
            assert!(scope.committed.is_none());
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        held_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let second = thread::spawn(move || {
            let scope =
                acquire_external_mutation_scope(&second_store, second_operation, RevisionId(0))
                    .unwrap();
            assert!(scope.committed.is_none());
            second_entered_tx.send(()).unwrap();
        });

        assert!(
            second_entered_rx
                .recv_timeout(Duration::from_millis(150))
                .is_err(),
            "a second canonical-to-YMM route entered while the first held the fence"
        );
        release_tx.send(()).unwrap();
        second_entered_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        first.join().unwrap();
        second.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_attempt_releases_fence_for_exact_retry() {
        let root = test_root();
        let store = DurableProjectStore::open(&root, "canonical-project").unwrap();
        let operation_id = Uuid::new_v4();

        let first = acquire_external_mutation_scope(&store, operation_id, RevisionId(0)).unwrap();
        assert!(first.committed.is_none());
        drop(first); // Models apply/rollback returning an error from the wrapper.

        let retry = acquire_external_mutation_scope(&store, operation_id, RevisionId(0)).unwrap();
        assert!(retry.committed.is_none());
        drop(retry);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn durable_reservation_survives_restart_and_only_exact_owner_reenters() {
        let root = test_root();
        let operation_id = Uuid::new_v4();
        let target = VerifiedTargetBinding {
            adapter_id: "ymm4-4.55".into(),
            target_project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            target_identity_digest: "sha256:target".into(),
            verified_fingerprint: "sha256:before".into(),
        };
        {
            let store = DurableProjectStore::open(&root, "canonical-project").unwrap();
            let scope =
                acquire_external_mutation_scope(&store, operation_id, RevisionId(0)).unwrap();
            store
                .reserve_external_commit(
                    operation_id,
                    RevisionId(0),
                    "patch-a",
                    "request-a",
                    &target,
                )
                .unwrap();
            drop(scope);
        }

        let reopened = DurableProjectStore::open(&root, "canonical-project").unwrap();
        let retry =
            acquire_external_mutation_scope(&reopened, operation_id, RevisionId(0)).unwrap();
        assert!(retry.pending_replay);
        assert!(retry.committed.is_none());
        drop(retry);
        let other = Uuid::new_v4();
        assert!(matches!(
            acquire_external_mutation_scope(&reopened, other, RevisionId(0)),
            Err(ProjectStoreError::ExternalCommitReserved(owner)) if owner == operation_id
        ));
        let retry =
            acquire_external_mutation_scope(&reopened, operation_id, RevisionId(0)).unwrap();
        reopened
            .abort_external_commit_reservation(
                operation_id,
                RevisionId(0),
                "patch-a",
                "request-a",
                &target,
            )
            .unwrap();
        drop(retry);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn terminal_receipt_abort_requires_exact_restored_no_item_proof() {
        let operation_id = Uuid::new_v4();
        let mut receipt = Ymm4OperationReceipt {
            operation_id,
            request_digest: "request-a".into(),
            project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            expected_fingerprint: "before".into(),
            status: Ymm4OperationStatus::Failed,
            before_fingerprint: "before".into(),
            after_fingerprint: "before".into(),
            applied_items: Vec::new(),
            verified: false,
            error: Some("rejected before mutation".into()),
        };
        assert!(operation_receipt_proves_no_mutation(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));

        receipt.after_fingerprint = "drifted".into();
        assert!(!operation_receipt_proves_no_mutation(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
        receipt.after_fingerprint = "before".into();
        receipt.applied_items.push(Ymm4ManagedItem {
            entity_id: "utt-01".into(),
            revision: 1,
            kind: ManagedItemKind::Voice,
            frame: 0,
            layer: 0,
            length: 1,
            text: Some("voice".into()),
            audio_path: None,
            artifact_hash: None,
            speaker: Some("speaker".into()),
            realization_id: Some(Uuid::new_v4()),
        });
        assert!(!operation_receipt_proves_no_mutation(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
        receipt.applied_items.clear();
        receipt.status = Ymm4OperationStatus::NotStarted;
        assert!(operation_receipt_proves_not_started(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
        receipt.after_fingerprint = "drifted".into();
        assert!(!operation_receipt_proves_not_started(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
        receipt.after_fingerprint = "before".into();
        receipt.status = Ymm4OperationStatus::RecoveryRequired;
        receipt.error = Some("durable successful readback needs manual recovery".into());
        assert!(!operation_receipt_proves_exact_rollback(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
        assert!(!operation_receipt_proves_no_mutation(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
        assert!(!operation_receipt_proves_not_started(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
        receipt.status = Ymm4OperationStatus::RolledBack;
        assert!(operation_receipt_proves_exact_rollback(
            &receipt,
            operation_id,
            "request-a",
            "target-project",
            "scene-a",
            "before",
        ));
    }

    #[test]
    fn native_extension_not_started_proof_is_exact_and_restored() {
        let operation_id = Uuid::new_v4();
        let request = Ymm4NativeExtensionApplyRequest {
            protocol_version: takegraph_node::YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            request_digest: "request-a".into(),
            project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            expected_fingerprint: "before".into(),
            descriptor_catalog_digest: "catalog-a".into(),
            driver_profile_digest: "driver-a".into(),
            plan_digest: "plan-a".into(),
            plan: NativeExtensionPlan {
                canonical_version: 1,
                operation_id,
                base_revision: RevisionId(0),
                target: TargetIdentity {
                    adapter_id: "ymm4".into(),
                    project_id: "target-project".into(),
                    scene_id: "scene-a".into(),
                    fps: 30,
                    driver_version: "driver".into(),
                },
                capability_digest: "capability-a".into(),
                descriptor_catalog_digest: "catalog-a".into(),
                expected_scope: ScopeFingerprints {
                    target_identity_digest: "target-a".into(),
                    managed_state_digest: "managed-a".into(),
                    conflict_scope_digest: "conflict-a".into(),
                },
                change_budget: ChangeBudget::create_only(0),
                operations: Vec::new(),
                warnings: Vec::new(),
            },
            artifacts: Vec::new(),
        };
        let mut receipt = Ymm4NativeExtensionApplyResponse {
            operation_id,
            request_digest: request.request_digest.clone(),
            project_id: request.project_id.clone(),
            scene_id: request.scene_id.clone(),
            status: Ymm4NativeExtensionStatus::NotStarted,
            before_fingerprint: request.expected_fingerprint.clone(),
            after_fingerprint: request.expected_fingerprint.clone(),
            descriptor_catalog_digest: request.descriptor_catalog_digest.clone(),
            driver_profile_digest: request.driver_profile_digest.clone(),
            realizations: Vec::new(),
            verified: false,
            error: Some("durable no-mutation tombstone".into()),
        };
        assert!(native_extension_receipt_proves_not_started(
            &receipt, &request
        ));
        receipt.after_fingerprint = "drifted".into();
        assert!(!native_extension_receipt_proves_not_started(
            &receipt, &request
        ));
        receipt.after_fingerprint = request.expected_fingerprint.clone();
        receipt.status = Ymm4NativeExtensionStatus::RecoveryRequired;
        receipt.error = Some("durable successful readback needs manual recovery".into());
        assert!(!native_extension_receipt_proves_exact_rollback(
            &receipt, &request
        ));
        assert!(!native_extension_receipt_proves_no_mutation(
            &receipt, &request
        ));
        assert!(!native_extension_receipt_proves_not_started(
            &receipt, &request
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn durable_reservation_checks_stale_head_and_blocks_other_routes() {
        let root = test_root();
        let store = DurableProjectStore::open(&root, "canonical-project").unwrap();
        let seed_operation = Uuid::new_v4();
        let realization_id = Uuid::new_v4();
        let identity = ManagedSemanticIdentity {
            entity_id: "utt-01".into(),
            realization_id: Some(realization_id),
        };
        let receipt = Ymm4OperationReceipt {
            operation_id: seed_operation,
            request_digest: "seed-request".into(),
            project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            expected_fingerprint: "sha256:before".into(),
            status: Ymm4OperationStatus::Verified,
            before_fingerprint: "sha256:before".into(),
            after_fingerprint: "sha256:after".into(),
            applied_items: vec![Ymm4ManagedItem {
                entity_id: identity.entity_id.clone(),
                revision: 1,
                kind: ManagedItemKind::Voice,
                frame: 0,
                layer: 0,
                length: 1,
                text: Some("voice".into()),
                audio_path: None,
                artifact_hash: None,
                speaker: Some("speaker".into()),
                realization_id: identity.realization_id,
            }],
            verified: true,
            error: None,
        };
        let target = VerifiedTargetBinding {
            adapter_id: "ymm4-4.55".into(),
            target_project_id: receipt.project_id.clone(),
            scene_id: receipt.scene_id.clone(),
            target_identity_digest: "sha256:target".into(),
            verified_fingerprint: receipt.after_fingerprint.clone(),
        };
        let update = VerifiedManagedStateUpdate::replacing_identities_from_receipt(
            BTreeSet::from([identity.clone()]),
            &receipt,
        )
        .unwrap();
        let seed = VerifiedExternalCommit::from_receipt(
            seed_operation,
            RevisionId(0),
            "seed-patch",
            receipt.request_digest.clone(),
            &receipt,
            target.clone(),
        )
        .unwrap()
        .with_managed_state_update(update)
        .unwrap();
        assert_eq!(
            store.commit_verified_external(&seed).unwrap(),
            RevisionId(1)
        );

        let stale_operation = Uuid::new_v4();
        let stale_scope =
            acquire_external_mutation_scope(&store, stale_operation, RevisionId(1)).unwrap();
        assert!(stale_scope.committed.is_none());
        assert!(matches!(
            store.reserve_external_commit(
                stale_operation,
                RevisionId(0),
                "stale-patch",
                "stale-request",
                &target,
            ),
            Err(ProjectStoreError::StaleRevision {
                expected: RevisionId(0),
                actual: RevisionId(1)
            })
        ));
        drop(stale_scope);

        let detach_operation = Uuid::new_v4();
        store
            .reserve_metadata_detach(
                detach_operation,
                RevisionId(1),
                "detach-patch",
                "detach-request",
                &target,
                &identity,
            )
            .unwrap();

        assert!(matches!(
            acquire_external_mutation_scope(&store, Uuid::new_v4(), RevisionId(1)),
            Err(ProjectStoreError::ExternalCommitReserved(owner)) if owner == detach_operation
        ));
        assert!(matches!(
            store.reserve_external_commit(
                detach_operation,
                RevisionId(1),
                "ordinary-patch",
                "ordinary-request",
                &target,
            ),
            Err(ProjectStoreError::OperationConflict(owner)) if owner == detach_operation
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
