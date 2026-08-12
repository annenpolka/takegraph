use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use takegraph_core::{
    CanonicalError, ManagedSemanticIdentity, ManagedSemanticItem, RevisionId, canonical_sha256,
};
use takegraph_node::{
    Ymm4MetadataDetachReceipt, Ymm4NativeExtensionApplyResponse, Ymm4OperationReceipt,
};
use thiserror::Error;
use uuid::Uuid;

const PROJECT_STATE_SCHEMA_VERSION: u32 = 3;
const MINIMUM_PROJECT_STATE_SCHEMA_VERSION: u32 = 1;
const STATE_PREFIX: &str = "state-";
const STATE_SUFFIX: &str = ".json";

/// Durable canonical project state. Each successful commit writes a new,
/// immutable generation so a crash cannot destroy the preceding revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableProjectState {
    pub schema_version: u32,
    pub project_id: String,
    pub generation: u64,
    pub previous_state_digest: Option<String>,
    pub bootstrap_revision: RevisionId,
    pub head: RevisionId,
    pub target_links: BTreeMap<String, TargetLink>,
    pub external_commits: BTreeMap<Uuid, ExternalCommitRecord>,
    /// Receipt-bound canonical semantic projections for target links. Version-1
    /// stores deserialize this as empty and must be re-seeded by a verified
    /// exporter commit before reconciliation can run.
    #[serde(default)]
    pub managed_target_states: BTreeMap<String, ManagedTargetState>,
    /// At most one canonical external mutation may be in flight. The
    /// reservation is itself a published generation and fences every other
    /// operation before target I/O begins.
    #[serde(default)]
    pub pending_external_commit: Option<PendingExternalCommit>,
    pub state_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingExternalCommit {
    pub kind: PendingExternalCommitKind,
    pub operation_id: Uuid,
    pub base_revision: RevisionId,
    pub patch_digest: String,
    pub request_digest: String,
    pub target_link_key: String,
    pub target_identity_digest: String,
    pub managed_identity: Option<ManagedSemanticIdentity>,
    pub reservation_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingExternalCommitKind {
    ExternalMutation,
    MetadataDetach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetLink {
    pub key: String,
    pub adapter_id: String,
    pub target_project_id: String,
    pub scene_id: String,
    pub target_identity_digest: String,
    pub last_verified_fingerprint: String,
    pub canonical_revision: RevisionId,
    pub last_operation_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCommitRecord {
    pub operation_id: Uuid,
    pub base_revision: RevisionId,
    pub committed_revision: RevisionId,
    pub patch_digest: String,
    pub request_digest: String,
    pub receipt_digest: String,
    pub target_link_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_identity_digest: String,
    #[serde(default)]
    pub managed_state_update_digest: Option<String>,
}

/// Latest canonical managed-only projection associated with one target link.
/// Its contents are derived only from authenticated bridge read-back receipts,
/// never from reconciliation caller input or an unverified live snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedTargetState {
    pub target_link_key: String,
    pub adapter_id: String,
    pub target_project_id: String,
    pub scene_id: String,
    pub canonical_revision: RevisionId,
    pub source_operation_id: Uuid,
    pub source_receipt_digest: String,
    pub items: Vec<ManagedSemanticItem>,
    pub projection_digest: String,
}

/// Target identity captured by verified external read-back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedTargetBinding {
    pub adapter_id: String,
    pub target_project_id: String,
    pub scene_id: String,
    pub target_identity_digest: String,
    pub verified_fingerprint: String,
}

/// Proof passed internally by a workflow only after authenticated semantic
/// read-back has been verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedExternalCommit {
    pub operation_id: Uuid,
    pub base_revision: RevisionId,
    pub patch_digest: String,
    pub request_digest: String,
    pub receipt_digest: String,
    pub target: VerifiedTargetBinding,
    pub managed_state_update: Option<VerifiedManagedStateUpdate>,
}

/// A receipt-authenticated replacement delta for the canonical managed subset.
/// Entity replacement is used by portable pair exports; exact identity
/// replacement is used by native voice and native-extension mutations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VerifiedManagedStateUpdate {
    source_receipt_digest: String,
    replace_entity_ids: BTreeSet<String>,
    replace_identities: BTreeSet<ManagedSemanticIdentity>,
    require_existing_identities: bool,
    items: Vec<ManagedSemanticItem>,
}

impl VerifiedManagedStateUpdate {
    pub(crate) fn removing_identity_from_metadata_detach_receipt(
        identity: ManagedSemanticIdentity,
        receipt: &Ymm4MetadataDetachReceipt,
    ) -> Result<Self, ProjectStoreError> {
        let update = Self {
            source_receipt_digest: canonical_sha256("takegraph-external-receipt", receipt)?,
            replace_entity_ids: BTreeSet::new(),
            replace_identities: BTreeSet::from([identity]),
            require_existing_identities: true,
            items: Vec::new(),
        };
        validate_managed_update(&update)?;
        Ok(update)
    }

    pub(crate) fn replacing_entities_from_receipt(
        entity_ids: impl IntoIterator<Item = String>,
        receipt: &Ymm4OperationReceipt,
    ) -> Result<Self, ProjectStoreError> {
        let update = Self {
            source_receipt_digest: canonical_sha256("takegraph-external-receipt", receipt)?,
            replace_entity_ids: entity_ids.into_iter().collect(),
            replace_identities: BTreeSet::new(),
            require_existing_identities: false,
            items: receipt
                .applied_items
                .iter()
                .map(crate::managed_projection::project_managed_item)
                .collect(),
        };
        validate_managed_update(&update)?;
        Ok(update)
    }

    pub(crate) fn replacing_identities_from_receipt(
        identities: impl IntoIterator<Item = ManagedSemanticIdentity>,
        receipt: &Ymm4OperationReceipt,
    ) -> Result<Self, ProjectStoreError> {
        let update = Self {
            source_receipt_digest: canonical_sha256("takegraph-external-receipt", receipt)?,
            replace_entity_ids: BTreeSet::new(),
            replace_identities: identities.into_iter().collect(),
            require_existing_identities: false,
            items: receipt
                .applied_items
                .iter()
                .map(crate::managed_projection::project_managed_item)
                .collect(),
        };
        validate_managed_update(&update)?;
        Ok(update)
    }

    pub(crate) fn replacing_native_extensions_from_receipt(
        identities: impl IntoIterator<Item = ManagedSemanticIdentity>,
        receipt: &Ymm4NativeExtensionApplyResponse,
    ) -> Result<Self, ProjectStoreError> {
        let update = Self {
            source_receipt_digest: canonical_sha256("takegraph-external-receipt", receipt)?,
            replace_entity_ids: BTreeSet::new(),
            replace_identities: identities.into_iter().collect(),
            require_existing_identities: false,
            items: receipt
                .realizations
                .iter()
                .map(crate::managed_projection::project_native_extension_realization)
                .collect(),
        };
        validate_managed_update(&update)?;
        Ok(update)
    }

    fn digest(&self) -> Result<String, CanonicalError> {
        canonical_sha256("takegraph-managed-state-update-v1", self)
    }
}

impl VerifiedExternalCommit {
    pub(crate) fn from_receipt<R: Serialize + ?Sized>(
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: impl Into<String>,
        request_digest: impl Into<String>,
        receipt: &R,
        target: VerifiedTargetBinding,
    ) -> Result<Self, ProjectStoreError> {
        Ok(Self {
            operation_id,
            base_revision,
            patch_digest: patch_digest.into(),
            request_digest: request_digest.into(),
            receipt_digest: canonical_sha256("takegraph-external-receipt", receipt)?,
            target,
            managed_state_update: None,
        })
    }

    pub(crate) fn with_managed_state_update(
        mut self,
        update: VerifiedManagedStateUpdate,
    ) -> Result<Self, ProjectStoreError> {
        validate_managed_update(&update)?;
        if update.source_receipt_digest != self.receipt_digest {
            return Err(ProjectStoreError::ManagedUpdateReceiptMismatch);
        }
        self.managed_state_update = Some(update);
        Ok(self)
    }
}

/// Append-only canonical project revision store.
///
/// Readers select the greatest complete generation. Writers take an OS file
/// lock and publish a new immutable state file with an atomic rename. Temporary
/// files are ignored after a crash; a corrupt published generation fails closed.
#[derive(Debug, Clone)]
pub struct DurableProjectStore {
    root: PathBuf,
    project_id: String,
}

/// Process-lifetime exclusion around one external target mutation and its
/// canonical commit. Dropping the guard releases the OS lock; a durable
/// [`PendingExternalCommit`] remains authoritative across process crashes.
#[derive(Debug)]
pub struct ExternalMutationFence {
    lock_file: File,
}

impl Drop for ExternalMutationFence {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

impl DurableProjectStore {
    /// Returns the canonical project identity bound to this store.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Opens a project beneath a shared store root using a content-addressed
    /// directory name, avoiding any path interpretation of the project ID.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid identity, canonicalization, I/O, or corrupt state.
    pub fn open_scoped(
        shared_root: impl AsRef<Path>,
        project_id: impl Into<String>,
    ) -> Result<Self, ProjectStoreError> {
        Self::open_scoped_or_bootstrap(shared_root, project_id, RevisionId(0))
    }

    /// Opens a content-addressed project directory or initializes its explicit
    /// pre-store migration baseline.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid identity, canonicalization, I/O, or corrupt state.
    pub fn open_scoped_or_bootstrap(
        shared_root: impl AsRef<Path>,
        project_id: impl Into<String>,
        bootstrap_revision: RevisionId,
    ) -> Result<Self, ProjectStoreError> {
        let project_id = project_id.into();
        if project_id.trim().is_empty() {
            return Err(ProjectStoreError::EmptyField("projectId"));
        }
        let digest = canonical_sha256("takegraph-project-store-key", &project_id)?;
        let directory = digest
            .strip_prefix("sha256:")
            .ok_or_else(|| ProjectStoreError::Corrupt("invalid project key digest".into()))?;
        Self::open_or_bootstrap(
            shared_root.as_ref().join(directory),
            project_id,
            bootstrap_revision,
        )
    }

    /// Opens or initializes a project-specific canonical store.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty project identity, filesystem failure, or
    /// any corrupt published state generation.
    pub fn open(
        root: impl AsRef<Path>,
        project_id: impl Into<String>,
    ) -> Result<Self, ProjectStoreError> {
        Self::open_or_bootstrap(root, project_id, RevisionId(0))
    }

    /// Opens a store or, only when no published generation exists, records an
    /// explicit migration baseline for a project created before durable state.
    /// Existing state always wins over the supplied baseline.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid identity, I/O, or corrupt existing state.
    pub fn open_or_bootstrap(
        root: impl AsRef<Path>,
        project_id: impl Into<String>,
        bootstrap_revision: RevisionId,
    ) -> Result<Self, ProjectStoreError> {
        let project_id = project_id.into();
        if project_id.trim().is_empty() {
            return Err(ProjectStoreError::EmptyField("projectId"));
        }
        let store = Self {
            root: root.as_ref().to_path_buf(),
            project_id,
        };
        fs::create_dir_all(store.revisions_path())?;
        store.with_lock(|store| {
            if store.load_latest_unlocked()?.is_none() {
                store.persist_unlocked(DurableProjectState {
                    schema_version: PROJECT_STATE_SCHEMA_VERSION,
                    project_id: store.project_id.clone(),
                    generation: 0,
                    previous_state_digest: None,
                    bootstrap_revision,
                    head: bootstrap_revision,
                    target_links: BTreeMap::new(),
                    external_commits: BTreeMap::new(),
                    managed_target_states: BTreeMap::new(),
                    pending_external_commit: None,
                    state_digest: String::new(),
                })?;
            }
            Ok(())
        })?;
        Ok(store)
    }

    /// Returns the durable canonical head after validating the latest generation.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O or corrupt state.
    pub fn head(&self) -> Result<RevisionId, ProjectStoreError> {
        Ok(self.snapshot()?.head)
    }

    /// Loads and validates the latest complete project generation.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O, missing initialized state, or corruption.
    pub fn snapshot(&self) -> Result<DurableProjectState, ProjectStoreError> {
        self.with_lock(|store| {
            store
                .load_latest_unlocked()?
                .ok_or_else(|| ProjectStoreError::Corrupt("initialized state is missing".into()))
        })
    }

    /// Serializes target I/O with canonical publication for this project.
    /// A durable reservation owned by another operation rejects the caller
    /// after it acquires the OS lock, so a crashed detach remains fail-closed.
    ///
    /// # Errors
    ///
    /// Returns an error for lock I/O, corrupt state, or a reservation owned by
    /// another operation.
    pub fn acquire_external_mutation_fence(
        &self,
        operation_id: Uuid,
    ) -> Result<ExternalMutationFence, ProjectStoreError> {
        if operation_id.is_nil() {
            return Err(ProjectStoreError::InvalidExternalCommitReservation);
        }
        fs::create_dir_all(&self.root)?;
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("external-mutation.lock"))?;
        lock_file.lock()?;
        let state = match self.snapshot() {
            Ok(state) => state,
            Err(error) => {
                let _ = lock_file.unlock();
                return Err(error);
            }
        };
        if let Some(pending) = &state.pending_external_commit
            && pending.operation_id != operation_id
        {
            let owner = pending.operation_id;
            let _ = lock_file.unlock();
            return Err(ProjectStoreError::ExternalCommitReserved(owner));
        }
        Ok(ExternalMutationFence { lock_file })
    }

    /// Loads the receipt-derived canonical managed projection for an exact,
    /// currently linked target. Caller-provided expected state is deliberately
    /// not accepted at this boundary.
    ///
    /// # Errors
    ///
    /// Returns an error for corrupt state, an unknown/stale target link, or a
    /// version-1/pre-seeding project without receipt-derived managed evidence.
    pub fn managed_state_for_target(
        &self,
        adapter_id: &str,
        target_project_id: &str,
        scene_id: &str,
        target_identity_digest: &str,
    ) -> Result<Vec<ManagedSemanticItem>, ProjectStoreError> {
        let state = self.snapshot()?;
        let key = target_link_key_parts(adapter_id, target_project_id, scene_id)?;
        let link = state
            .target_links
            .get(&key)
            .ok_or_else(|| ProjectStoreError::TargetLinkNotFound(key.clone()))?;
        if link.target_identity_digest != target_identity_digest {
            return Err(ProjectStoreError::TargetIdentityMismatch {
                expected: link.target_identity_digest.clone(),
                actual: target_identity_digest.into(),
            });
        }
        let projection = state
            .managed_target_states
            .get(&key)
            .ok_or_else(|| ProjectStoreError::ManagedStateUnavailable(key.clone()))?;
        validate_managed_target_state(projection, &key, state.head)?;
        Ok(projection.items.clone())
    }

    /// Resolves the sole durable adapter link for a project/scene and loads its
    /// receipt-derived projection. Multiple adapter links are rejected rather
    /// than guessed.
    ///
    /// # Errors
    ///
    /// Returns an error for corrupt state, a missing/ambiguous/stale target
    /// link, or a pre-seeding store without receipt-derived managed evidence.
    pub fn managed_state_for_linked_target(
        &self,
        target_project_id: &str,
        scene_id: &str,
        target_identity_digest: &str,
    ) -> Result<Vec<ManagedSemanticItem>, ProjectStoreError> {
        let state = self.snapshot()?;
        let matches = state
            .target_links
            .values()
            .filter(|link| link.target_project_id == target_project_id && link.scene_id == scene_id)
            .collect::<Vec<_>>();
        let [link] = matches.as_slice() else {
            return if matches.is_empty() {
                Err(ProjectStoreError::TargetLinkNotFound(format!(
                    "{target_project_id}/{scene_id}"
                )))
            } else {
                Err(ProjectStoreError::AmbiguousTargetLink(format!(
                    "{target_project_id}/{scene_id}"
                )))
            };
        };
        if link.target_identity_digest != target_identity_digest {
            return Err(ProjectStoreError::TargetIdentityMismatch {
                expected: link.target_identity_digest.clone(),
                actual: target_identity_digest.into(),
            });
        }
        let projection = state
            .managed_target_states
            .get(&link.key)
            .ok_or_else(|| ProjectStoreError::ManagedStateUnavailable(link.key.clone()))?;
        validate_managed_target_state(projection, &link.key, state.head)?;
        Ok(projection.items.clone())
    }

    /// Durably fences the canonical base revision before an external detach
    /// can mutate YMM4. Exact replay is idempotent; every other canonical
    /// operation is rejected until this reservation is finalized or aborted.
    ///
    /// # Errors
    ///
    /// Returns an error for stale revision, changed reservation evidence,
    /// missing canonical ownership, another in-flight operation, or corrupt I/O.
    pub(crate) fn reserve_metadata_detach(
        &self,
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: &str,
        request_digest: &str,
        target: &VerifiedTargetBinding,
        identity: &ManagedSemanticIdentity,
    ) -> Result<(), ProjectStoreError> {
        self.reserve_external_commit_inner(
            PendingExternalCommitKind::MetadataDetach,
            operation_id,
            base_revision,
            patch_digest,
            request_digest,
            target,
            Some(identity.clone()),
        )
    }

    /// Durably reserves a canonical revision for an ordinary external
    /// mutation before bridge I/O. Exact replay is idempotent and consumes the
    /// same reservation during verified finalization.
    ///
    /// # Errors
    ///
    /// Returns an error for stale/rebound evidence, another pending operation,
    /// or corrupt durable state.
    pub(crate) fn reserve_external_commit(
        &self,
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: &str,
        request_digest: &str,
        target: &VerifiedTargetBinding,
    ) -> Result<(), ProjectStoreError> {
        self.reserve_external_commit_inner(
            PendingExternalCommitKind::ExternalMutation,
            operation_id,
            base_revision,
            patch_digest,
            request_digest,
            target,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn reserve_external_commit_inner(
        &self,
        kind: PendingExternalCommitKind,
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: &str,
        request_digest: &str,
        target: &VerifiedTargetBinding,
        managed_identity: Option<ManagedSemanticIdentity>,
    ) -> Result<(), ProjectStoreError> {
        let target_link_key = target_link_key(target)?;
        let mut reservation = PendingExternalCommit {
            kind,
            operation_id,
            base_revision,
            patch_digest: patch_digest.into(),
            request_digest: request_digest.into(),
            target_link_key,
            target_identity_digest: target.target_identity_digest.clone(),
            managed_identity,
            reservation_digest: String::new(),
        };
        reservation.reservation_digest = pending_external_commit_digest(&reservation)?;
        validate_pending_external_commit(&reservation)?;
        self.with_lock(|store| {
            let mut state = store
                .load_latest_unlocked()?
                .ok_or_else(|| ProjectStoreError::Corrupt("initialized state is missing".into()))?;
            if let Some(committed) = state.external_commits.get(&operation_id) {
                return if committed.base_revision == reservation.base_revision
                    && committed.patch_digest == reservation.patch_digest
                    && committed.request_digest == reservation.request_digest
                    && committed.target_link_key == reservation.target_link_key
                    && committed.target_identity_digest == reservation.target_identity_digest
                {
                    Ok(())
                } else {
                    Err(ProjectStoreError::OperationConflict(operation_id))
                };
            }
            if let Some(existing) = &state.pending_external_commit {
                return if existing == &reservation {
                    Ok(())
                } else if existing.operation_id == operation_id {
                    Err(ProjectStoreError::OperationConflict(operation_id))
                } else {
                    Err(ProjectStoreError::ExternalCommitReserved(
                        existing.operation_id,
                    ))
                };
            }
            if state.head != base_revision {
                return Err(ProjectStoreError::StaleRevision {
                    expected: base_revision,
                    actual: state.head,
                });
            }
            if reservation.kind == PendingExternalCommitKind::MetadataDetach {
                let identity = reservation
                    .managed_identity
                    .as_ref()
                    .ok_or(ProjectStoreError::InvalidExternalCommitReservation)?;
                let projection = state
                    .managed_target_states
                    .get(&reservation.target_link_key)
                    .ok_or_else(|| {
                        ProjectStoreError::ManagedStateUnavailable(
                            reservation.target_link_key.clone(),
                        )
                    })?;
                if !projection
                    .items
                    .iter()
                    .any(|item| item.identity == *identity)
                {
                    return Err(ProjectStoreError::ManagedIdentityNotFoundForRemoval);
                }
            }
            publish_reserved_state(store, &mut state, Some(reservation))
        })
    }

    /// Releases an exact reservation only after the bridge proves that the
    /// target mutation rolled back and canonical ownership must remain.
    pub(crate) fn abort_metadata_detach_reservation(
        &self,
        operation_id: Uuid,
        request_digest: &str,
    ) -> Result<(), ProjectStoreError> {
        self.abort_external_commit_reservation_inner(
            PendingExternalCommitKind::MetadataDetach,
            operation_id,
            request_digest,
        )
    }

    /// Verifies that the durable in-flight reservation is the exact metadata
    /// detach request being recovered. This check is independent of today's
    /// bridge capabilities and target fingerprint.
    #[allow(clippy::too_many_arguments)] // Every sealed reservation binding is supplied explicitly.
    pub(crate) fn verify_metadata_detach_reservation(
        &self,
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: &str,
        request_digest: &str,
        target_project_id: &str,
        scene_id: &str,
        target_identity_digest: &str,
        identity: &ManagedSemanticIdentity,
    ) -> Result<(), ProjectStoreError> {
        let state = self.snapshot()?;
        let pending = state
            .pending_external_commit
            .as_ref()
            .ok_or(ProjectStoreError::OperationConflict(operation_id))?;
        let link = state
            .target_links
            .get(&pending.target_link_key)
            .ok_or(ProjectStoreError::OperationConflict(operation_id))?;
        if pending.kind != PendingExternalCommitKind::MetadataDetach
            || pending.operation_id != operation_id
            || pending.base_revision != base_revision
            || pending.patch_digest != patch_digest
            || pending.request_digest != request_digest
            || pending.target_identity_digest != target_identity_digest
            || pending.managed_identity.as_ref() != Some(identity)
            || link.target_project_id != target_project_id
            || link.scene_id != scene_id
            || link.target_identity_digest != target_identity_digest
        {
            return Err(ProjectStoreError::OperationConflict(operation_id));
        }
        Ok(())
    }

    /// Finalizes a verified metadata detach from its exact durable reservation.
    /// The adapter identity is recovered from the sealed target link, so a
    /// plugin/capability change after the bridge receipt cannot wedge recovery.
    #[allow(clippy::too_many_arguments)] // Finalization re-authenticates the complete sealed tuple.
    pub(crate) fn finalize_reserved_metadata_detach(
        &self,
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: &str,
        request_digest: &str,
        target_project_id: &str,
        scene_id: &str,
        target_identity_digest: &str,
        identity: &ManagedSemanticIdentity,
        receipt: &Ymm4MetadataDetachReceipt,
    ) -> Result<RevisionId, ProjectStoreError> {
        self.verify_metadata_detach_reservation(
            operation_id,
            base_revision,
            patch_digest,
            request_digest,
            target_project_id,
            scene_id,
            target_identity_digest,
            identity,
        )?;
        let state = self.snapshot()?;
        let pending = state
            .pending_external_commit
            .as_ref()
            .ok_or(ProjectStoreError::OperationConflict(operation_id))?;
        let link = state
            .target_links
            .get(&pending.target_link_key)
            .ok_or(ProjectStoreError::OperationConflict(operation_id))?;
        let managed_update =
            VerifiedManagedStateUpdate::removing_identity_from_metadata_detach_receipt(
                identity.clone(),
                receipt,
            )?;
        let proof = VerifiedExternalCommit::from_receipt(
            operation_id,
            base_revision,
            patch_digest,
            request_digest,
            receipt,
            VerifiedTargetBinding {
                adapter_id: link.adapter_id.clone(),
                target_project_id: target_project_id.into(),
                scene_id: scene_id.into(),
                target_identity_digest: target_identity_digest.into(),
                verified_fingerprint: receipt.after_fingerprint.clone(),
            },
        )?
        .with_managed_state_update(managed_update)?;
        self.commit_verified_external(&proof)
    }

    /// Authenticates a metadata-detach commit that was durably finalized
    /// before its operation-task terminal generation was published.
    ///
    /// This historical check deliberately does not depend on the current
    /// target snapshot or capability descriptor: later verified operations may
    /// already have advanced both. The immutable commit record must instead
    /// match the exact request, receipt, target identity, and remove-only
    /// managed-state delta.
    #[allow(clippy::too_many_arguments)] // Historical recovery compares every immutable proof binding.
    pub(crate) fn verify_committed_metadata_detach(
        &self,
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: &str,
        request_digest: &str,
        target_project_id: &str,
        scene_id: &str,
        target_identity_digest: &str,
        identity: &ManagedSemanticIdentity,
        receipt: &Ymm4MetadataDetachReceipt,
    ) -> Result<RevisionId, ProjectStoreError> {
        let state = self.snapshot()?;
        let record = state
            .external_commits
            .get(&operation_id)
            .ok_or(ProjectStoreError::OperationConflict(operation_id))?;
        let link = state
            .target_links
            .get(&record.target_link_key)
            .ok_or(ProjectStoreError::OperationConflict(operation_id))?;
        let expected_target_key =
            target_link_key_parts(&link.adapter_id, target_project_id, scene_id)?;
        let receipt_digest = canonical_sha256("takegraph-external-receipt", receipt)?;
        let update = VerifiedManagedStateUpdate::removing_identity_from_metadata_detach_receipt(
            identity.clone(),
            receipt,
        )?;
        let expected_revision = base_revision
            .checked_next()
            .ok_or(ProjectStoreError::RevisionOverflow)?;
        if record.operation_id != operation_id
            || record.base_revision != base_revision
            || record.committed_revision != expected_revision
            || record.patch_digest != patch_digest
            || record.request_digest != request_digest
            || record.receipt_digest != receipt_digest
            || record.target_link_key != expected_target_key
            || record.target_identity_digest != target_identity_digest
            || record.managed_state_update_digest.as_deref() != Some(update.digest()?.as_str())
            || link.target_project_id != target_project_id
            || link.scene_id != scene_id
        {
            return Err(ProjectStoreError::OperationConflict(operation_id));
        }
        Ok(record.committed_revision)
    }

    /// Releases an ordinary external-mutation reservation only after its
    /// route has authenticated an exact request-bound rollback receipt.
    ///
    /// # Errors
    ///
    /// Returns an error for a changed operation/request binding, a detach
    /// reservation, or corrupt durable state.
    pub(crate) fn abort_external_commit_reservation(
        &self,
        operation_id: Uuid,
        base_revision: RevisionId,
        patch_digest: &str,
        request_digest: &str,
        target: &VerifiedTargetBinding,
    ) -> Result<(), ProjectStoreError> {
        let target_link_key = target_link_key(target)?;
        self.with_lock(|store| {
            let mut state = store
                .load_latest_unlocked()?
                .ok_or_else(|| ProjectStoreError::Corrupt("initialized state is missing".into()))?;
            let Some(pending) = &state.pending_external_commit else {
                return Ok(());
            };
            if pending.kind != PendingExternalCommitKind::ExternalMutation
                || pending.operation_id != operation_id
                || pending.base_revision != base_revision
                || pending.patch_digest != patch_digest
                || pending.request_digest != request_digest
                || pending.target_link_key != target_link_key
                || pending.target_identity_digest != target.target_identity_digest
            {
                return Err(ProjectStoreError::OperationConflict(operation_id));
            }
            publish_reserved_state(store, &mut state, None)
        })
    }

    fn abort_external_commit_reservation_inner(
        &self,
        kind: PendingExternalCommitKind,
        operation_id: Uuid,
        request_digest: &str,
    ) -> Result<(), ProjectStoreError> {
        self.with_lock(|store| {
            let mut state = store
                .load_latest_unlocked()?
                .ok_or_else(|| ProjectStoreError::Corrupt("initialized state is missing".into()))?;
            let Some(pending) = &state.pending_external_commit else {
                return Ok(());
            };
            if pending.kind != kind
                || pending.operation_id != operation_id
                || pending.request_digest != request_digest
            {
                return Err(ProjectStoreError::OperationConflict(operation_id));
            }
            publish_reserved_state(store, &mut state, None)
        })
    }

    #[allow(clippy::too_many_lines)] // One lock scope validates and atomically publishes all proof-derived state.
    pub(crate) fn commit_verified_external(
        &self,
        proof: &VerifiedExternalCommit,
    ) -> Result<RevisionId, ProjectStoreError> {
        validate_proof(proof)?;
        self.with_lock(|store| {
            let mut state = store
                .load_latest_unlocked()?
                .ok_or_else(|| ProjectStoreError::Corrupt("initialized state is missing".into()))?;
            if let Some(existing) = state.external_commits.get(&proof.operation_id) {
                let target_key = target_link_key(&proof.target)?;
                let managed_state_update_digest = proof
                    .managed_state_update
                    .as_ref()
                    .map(VerifiedManagedStateUpdate::digest)
                    .transpose()?;
                if existing.base_revision == proof.base_revision
                    && existing.patch_digest == proof.patch_digest
                    && existing.request_digest == proof.request_digest
                    && existing.receipt_digest == proof.receipt_digest
                    && existing.target_link_key == target_key
                    && existing.target_identity_digest == proof.target.target_identity_digest
                    && existing.managed_state_update_digest == managed_state_update_digest
                {
                    return Ok(existing.committed_revision);
                }
                return Err(ProjectStoreError::OperationConflict(proof.operation_id));
            }
            if let Some(pending) = &state.pending_external_commit
                && pending.operation_id != proof.operation_id
            {
                return Err(ProjectStoreError::ExternalCommitReserved(
                    pending.operation_id,
                ));
            }
            if let Some(pending) = &state.pending_external_commit {
                let target_key = target_link_key(&proof.target)?;
                let detach_update_mismatch = pending.kind
                    == PendingExternalCommitKind::MetadataDetach
                    && !proof.managed_state_update.as_ref().is_some_and(|update| {
                        pending.managed_identity.as_ref().is_some_and(|identity| {
                            update.require_existing_identities
                                && update.replace_identities == BTreeSet::from([identity.clone()])
                                && update.items.is_empty()
                        })
                    });
                if pending.base_revision != proof.base_revision
                    || pending.patch_digest != proof.patch_digest
                    || pending.request_digest != proof.request_digest
                    || pending.target_link_key != target_key
                    || pending.target_identity_digest != proof.target.target_identity_digest
                    || detach_update_mismatch
                {
                    return Err(ProjectStoreError::OperationConflict(proof.operation_id));
                }
            }
            if state.head != proof.base_revision {
                return Err(ProjectStoreError::StaleRevision {
                    expected: proof.base_revision,
                    actual: state.head,
                });
            }
            let committed_revision = state
                .head
                .checked_next()
                .ok_or(ProjectStoreError::RevisionOverflow)?;
            let target_key = target_link_key(&proof.target)?;
            let managed_state_update_digest = proof
                .managed_state_update
                .as_ref()
                .map(VerifiedManagedStateUpdate::digest)
                .transpose()?;
            if let Some(update) = &proof.managed_state_update {
                apply_managed_state_update(
                    &mut state,
                    &target_key,
                    committed_revision,
                    proof,
                    update,
                )?;
            }
            state.target_links.insert(
                target_key.clone(),
                TargetLink {
                    key: target_key.clone(),
                    adapter_id: proof.target.adapter_id.clone(),
                    target_project_id: proof.target.target_project_id.clone(),
                    scene_id: proof.target.scene_id.clone(),
                    target_identity_digest: proof.target.target_identity_digest.clone(),
                    last_verified_fingerprint: proof.target.verified_fingerprint.clone(),
                    canonical_revision: committed_revision,
                    last_operation_id: proof.operation_id,
                },
            );
            state.external_commits.insert(
                proof.operation_id,
                ExternalCommitRecord {
                    operation_id: proof.operation_id,
                    base_revision: proof.base_revision,
                    committed_revision,
                    patch_digest: proof.patch_digest.clone(),
                    request_digest: proof.request_digest.clone(),
                    receipt_digest: proof.receipt_digest.clone(),
                    target_link_key: target_key,
                    target_identity_digest: proof.target.target_identity_digest.clone(),
                    managed_state_update_digest,
                },
            );
            if state
                .pending_external_commit
                .as_ref()
                .is_some_and(|pending| pending.operation_id == proof.operation_id)
            {
                state.pending_external_commit = None;
            }
            let previous_state_digest = state.state_digest.clone();
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(ProjectStoreError::GenerationOverflow)?;
            state.previous_state_digest = Some(previous_state_digest);
            state.head = committed_revision;
            state.schema_version = PROJECT_STATE_SCHEMA_VERSION;
            store.persist_unlocked(state)?;
            Ok(committed_revision)
        })
    }

    fn with_lock<T>(
        &self,
        action: impl FnOnce(&Self) -> Result<T, ProjectStoreError>,
    ) -> Result<T, ProjectStoreError> {
        fs::create_dir_all(&self.root)?;
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("project.lock"))?;
        lock_file.lock()?;
        let result = action(self);
        lock_file.unlock()?;
        result
    }

    fn revisions_path(&self) -> PathBuf {
        self.root.join("revisions")
    }

    fn load_latest_unlocked(&self) -> Result<Option<DurableProjectState>, ProjectStoreError> {
        let revisions = self.revisions_path();
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&revisions)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(STATE_PREFIX) || !name.ends_with(STATE_SUFFIX) {
                continue;
            }
            let (generation, expected_digest) = parse_state_filename(&name)?;
            candidates.push((generation, expected_digest, entry.path()));
        }
        if candidates.is_empty() {
            return Ok(None);
        }
        candidates.sort_by_key(|candidate| candidate.0);
        let mut previous_digest = None;
        let mut latest = None;
        for (expected_generation, (generation, expected_digest, path)) in
            candidates.into_iter().enumerate()
        {
            let expected_generation = u64::try_from(expected_generation)
                .map_err(|_| ProjectStoreError::GenerationOverflow)?;
            if generation != expected_generation {
                return Err(ProjectStoreError::Corrupt(format!(
                    "project state generation chain skips or duplicates {expected_generation}"
                )));
            }
            let mut bytes = Vec::new();
            File::open(&path)?.read_to_end(&mut bytes)?;
            let state: DurableProjectState = serde_json::from_slice(&bytes).map_err(|error| {
                ProjectStoreError::Corrupt(format!("{}: {error}", path.display()))
            })?;
            validate_state(
                &state,
                &self.project_id,
                generation,
                &expected_digest,
                previous_digest.as_deref(),
            )?;
            previous_digest = Some(state.state_digest.clone());
            latest = Some(state);
        }
        Ok(latest)
    }

    fn persist_unlocked(&self, mut state: DurableProjectState) -> Result<(), ProjectStoreError> {
        state.state_digest = state_digest(&state)?;
        let hash = state
            .state_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| ProjectStoreError::Corrupt("invalid state digest prefix".into()))?;
        let final_path = self.revisions_path().join(format!(
            "{STATE_PREFIX}{:020}-{hash}{STATE_SUFFIX}",
            state.generation
        ));
        if final_path.exists() {
            return Err(ProjectStoreError::Corrupt(format!(
                "state generation already exists: {}",
                final_path.display()
            )));
        }
        let temporary_path = self
            .revisions_path()
            .join(format!(".state-{}.tmp", Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(&state)?;
        let write_result = (|| -> Result<(), ProjectStoreError> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary_path, &final_path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        write_result
    }
}

fn state_digest(state: &DurableProjectState) -> Result<String, CanonicalError> {
    if state.schema_version == 1 {
        let external_commits = state
            .external_commits
            .iter()
            .map(|(id, record)| {
                (
                    *id,
                    ExternalCommitRecordV1 {
                        operation_id: record.operation_id,
                        base_revision: record.base_revision,
                        committed_revision: record.committed_revision,
                        patch_digest: record.patch_digest.clone(),
                        request_digest: record.request_digest.clone(),
                        receipt_digest: record.receipt_digest.clone(),
                        target_link_key: record.target_link_key.clone(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        canonical_sha256(
            "takegraph-project-state",
            &StateDigestPayloadV1 {
                schema_version: state.schema_version,
                project_id: &state.project_id,
                generation: state.generation,
                previous_state_digest: state.previous_state_digest.as_deref(),
                bootstrap_revision: state.bootstrap_revision,
                head: state.head,
                target_links: &state.target_links,
                external_commits: &external_commits,
            },
        )
    } else if state.schema_version == 2 {
        canonical_sha256(
            "takegraph-project-state",
            &StateDigestPayloadV2 {
                schema_version: state.schema_version,
                project_id: &state.project_id,
                generation: state.generation,
                previous_state_digest: state.previous_state_digest.as_deref(),
                bootstrap_revision: state.bootstrap_revision,
                head: state.head,
                target_links: &state.target_links,
                external_commits: &state.external_commits,
                managed_target_states: &state.managed_target_states,
            },
        )
    } else {
        canonical_sha256(
            "takegraph-project-state",
            &StateDigestPayloadV3 {
                schema_version: state.schema_version,
                project_id: &state.project_id,
                generation: state.generation,
                previous_state_digest: state.previous_state_digest.as_deref(),
                bootstrap_revision: state.bootstrap_revision,
                head: state.head,
                target_links: &state.target_links,
                external_commits: &state.external_commits,
                managed_target_states: &state.managed_target_states,
                pending_external_commit: state.pending_external_commit.as_ref(),
            },
        )
    }
}

fn validate_state(
    state: &DurableProjectState,
    project_id: &str,
    expected_generation: u64,
    expected_digest: &str,
    expected_previous_digest: Option<&str>,
) -> Result<(), ProjectStoreError> {
    if !(MINIMUM_PROJECT_STATE_SCHEMA_VERSION..=PROJECT_STATE_SCHEMA_VERSION)
        .contains(&state.schema_version)
    {
        return Err(ProjectStoreError::Corrupt(format!(
            "unsupported project state schema {}",
            state.schema_version
        )));
    }
    if state.project_id != project_id {
        return Err(ProjectStoreError::Corrupt(format!(
            "project state belongs to {}, expected {project_id}",
            state.project_id
        )));
    }
    if state.generation != expected_generation {
        return Err(ProjectStoreError::Corrupt(format!(
            "state content generation {} does not match filename {expected_generation}",
            state.generation
        )));
    }
    if state.previous_state_digest.as_deref() != expected_previous_digest {
        return Err(ProjectStoreError::Corrupt(format!(
            "state generation {expected_generation} does not link to its predecessor"
        )));
    }
    let actual = state_digest(state)?;
    let expected = format!("sha256:{expected_digest}");
    if state.state_digest != actual || actual != expected {
        return Err(ProjectStoreError::Corrupt(format!(
            "project state digest mismatch (stored {}, computed {actual}, filename {expected})",
            state.state_digest
        )));
    }
    if state.bootstrap_revision > state.head {
        return Err(ProjectStoreError::Corrupt(
            "bootstrap revision is ahead of project head".into(),
        ));
    }
    if state
        .target_links
        .values()
        .any(|link| link.canonical_revision > state.head)
        || state
            .external_commits
            .values()
            .any(|record| record.committed_revision > state.head)
    {
        return Err(ProjectStoreError::Corrupt(
            "project state contains a future revision".into(),
        ));
    }
    if state.schema_version == 1 && !state.managed_target_states.is_empty() {
        return Err(ProjectStoreError::Corrupt(
            "version-1 project state contains version-2 managed projections".into(),
        ));
    }
    if state.schema_version < 3 && state.pending_external_commit.is_some() {
        return Err(ProjectStoreError::Corrupt(
            "legacy project state contains an external-commit reservation".into(),
        ));
    }
    if let Some(pending) = &state.pending_external_commit {
        validate_pending_external_commit(pending)?;
        let detach_ownership_missing = pending.kind == PendingExternalCommitKind::MetadataDetach
            && (state
                .target_links
                .get(&pending.target_link_key)
                .is_none_or(|link| link.target_identity_digest != pending.target_identity_digest)
                || !state
                    .managed_target_states
                    .get(&pending.target_link_key)
                    .is_some_and(|projection| {
                        pending.managed_identity.as_ref().is_some_and(|identity| {
                            projection
                                .items
                                .iter()
                                .any(|item| &item.identity == identity)
                        })
                    }));
        if pending.base_revision != state.head
            || state.external_commits.contains_key(&pending.operation_id)
            || detach_ownership_missing
        {
            return Err(ProjectStoreError::InvalidExternalCommitReservation);
        }
    }
    for (key, projection) in &state.managed_target_states {
        validate_managed_target_state(projection, key, state.head)?;
        if !state.target_links.contains_key(key) {
            return Err(ProjectStoreError::Corrupt(format!(
                "managed projection {key} has no target link"
            )));
        }
    }
    Ok(())
}

fn parse_state_filename(name: &str) -> Result<(u64, String), ProjectStoreError> {
    let body = name
        .strip_prefix(STATE_PREFIX)
        .and_then(|value| value.strip_suffix(STATE_SUFFIX))
        .ok_or_else(|| ProjectStoreError::Corrupt(format!("invalid state filename: {name}")))?;
    let (generation, digest) = body
        .split_once('-')
        .ok_or_else(|| ProjectStoreError::Corrupt(format!("invalid state filename: {name}")))?;
    let generation = generation
        .parse::<u64>()
        .map_err(|_| ProjectStoreError::Corrupt(format!("invalid state filename: {name}")))?;
    if digest.len() != 64 || !digest.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(ProjectStoreError::Corrupt(format!(
            "invalid state digest filename: {name}"
        )));
    }
    Ok((generation, digest.into()))
}

fn target_link_key(target: &VerifiedTargetBinding) -> Result<String, CanonicalError> {
    target_link_key_parts(
        &target.adapter_id,
        &target.target_project_id,
        &target.scene_id,
    )
}

fn target_link_key_parts(
    adapter_id: &str,
    target_project_id: &str,
    scene_id: &str,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-target-link",
        &(adapter_id, target_project_id, scene_id),
    )
}

fn pending_external_commit_digest(
    reservation: &PendingExternalCommit,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-pending-external-commit-v2",
        &(
            reservation.operation_id,
            reservation.kind,
            reservation.base_revision,
            reservation.patch_digest.as_str(),
            reservation.request_digest.as_str(),
            reservation.target_link_key.as_str(),
            reservation.target_identity_digest.as_str(),
            &reservation.managed_identity,
        ),
    )
}

fn validate_pending_external_commit(
    reservation: &PendingExternalCommit,
) -> Result<(), ProjectStoreError> {
    if reservation.operation_id.is_nil()
        || reservation.patch_digest.trim().is_empty()
        || reservation.request_digest.trim().is_empty()
        || reservation.target_link_key.trim().is_empty()
        || reservation.target_identity_digest.trim().is_empty()
        || (reservation.kind == PendingExternalCommitKind::MetadataDetach
            && !reservation
                .managed_identity
                .as_ref()
                .is_some_and(|identity| {
                    !identity.entity_id.trim().is_empty() && identity.realization_id.is_some()
                }))
        || (reservation.kind == PendingExternalCommitKind::ExternalMutation
            && reservation.managed_identity.is_some())
        || pending_external_commit_digest(reservation)? != reservation.reservation_digest
    {
        return Err(ProjectStoreError::InvalidExternalCommitReservation);
    }
    Ok(())
}

fn publish_reserved_state(
    store: &DurableProjectStore,
    state: &mut DurableProjectState,
    reservation: Option<PendingExternalCommit>,
) -> Result<(), ProjectStoreError> {
    let previous_state_digest = state.state_digest.clone();
    state.generation = state
        .generation
        .checked_add(1)
        .ok_or(ProjectStoreError::GenerationOverflow)?;
    state.previous_state_digest = Some(previous_state_digest);
    state.schema_version = PROJECT_STATE_SCHEMA_VERSION;
    state.pending_external_commit = reservation;
    store.persist_unlocked(state.clone())
}

fn validate_managed_update(update: &VerifiedManagedStateUpdate) -> Result<(), ProjectStoreError> {
    if update.replace_entity_ids.is_empty()
        && update.replace_identities.is_empty()
        && update.items.is_empty()
    {
        return Err(ProjectStoreError::EmptyManagedStateUpdate);
    }
    if update.source_receipt_digest.trim().is_empty() {
        return Err(ProjectStoreError::ManagedUpdateReceiptMismatch);
    }
    if update
        .replace_entity_ids
        .iter()
        .any(|value| value.trim().is_empty())
        || update
            .replace_identities
            .iter()
            .any(|identity| identity.entity_id.trim().is_empty())
    {
        return Err(ProjectStoreError::EmptyField("managedIdentity"));
    }
    validate_managed_items(&update.items)?;
    if update.items.iter().any(|item| {
        !update.replace_entity_ids.contains(&item.identity.entity_id)
            && !update.replace_identities.contains(&item.identity)
    }) {
        return Err(ProjectStoreError::ManagedUpdateOutsideReplacementScope);
    }
    Ok(())
}

fn validate_managed_items(items: &[ManagedSemanticItem]) -> Result<(), ProjectStoreError> {
    if items.iter().any(|item| {
        item.identity.entity_id.trim().is_empty()
            || item.realization_kind.trim().is_empty()
            || item
                .owned_fields
                .keys()
                .any(|field| field.trim().is_empty())
    }) {
        return Err(ProjectStoreError::InvalidManagedProjection);
    }
    Ok(())
}

fn sort_managed_items(items: &mut [ManagedSemanticItem]) {
    items.sort_by_key(|item| {
        (
            item.identity.clone(),
            item.realization_kind.clone(),
            item.entity_revision,
            canonical_sha256("takegraph-managed-project-state-sort-v1", item).unwrap_or_default(),
        )
    });
}

fn managed_projection_digest(
    target_link_key: &str,
    canonical_revision: RevisionId,
    source_operation_id: Uuid,
    source_receipt_digest: &str,
    items: &[ManagedSemanticItem],
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-managed-target-projection-v1",
        &(
            target_link_key,
            canonical_revision,
            source_operation_id,
            source_receipt_digest,
            items,
        ),
    )
}

fn validate_managed_target_state(
    projection: &ManagedTargetState,
    expected_key: &str,
    head: RevisionId,
) -> Result<(), ProjectStoreError> {
    validate_managed_items(&projection.items)?;
    if projection.target_link_key != expected_key
        || projection.adapter_id.trim().is_empty()
        || projection.target_project_id.trim().is_empty()
        || projection.scene_id.trim().is_empty()
        || projection.source_receipt_digest.trim().is_empty()
        || projection.canonical_revision > head
    {
        return Err(ProjectStoreError::InvalidManagedProjection);
    }
    let mut sorted = projection.items.clone();
    sort_managed_items(&mut sorted);
    if sorted != projection.items {
        return Err(ProjectStoreError::InvalidManagedProjection);
    }
    let digest = managed_projection_digest(
        &projection.target_link_key,
        projection.canonical_revision,
        projection.source_operation_id,
        &projection.source_receipt_digest,
        &projection.items,
    )?;
    if digest != projection.projection_digest {
        return Err(ProjectStoreError::InvalidManagedProjection);
    }
    Ok(())
}

fn apply_managed_state_update(
    state: &mut DurableProjectState,
    target_key: &str,
    committed_revision: RevisionId,
    proof: &VerifiedExternalCommit,
    update: &VerifiedManagedStateUpdate,
) -> Result<(), ProjectStoreError> {
    validate_managed_update(update)?;
    let mut items = state
        .managed_target_states
        .get(target_key)
        .map_or_else(Vec::new, |projection| projection.items.clone());
    if update.require_existing_identities
        && !update
            .replace_identities
            .iter()
            .all(|identity| items.iter().any(|item| &item.identity == identity))
    {
        return Err(ProjectStoreError::ManagedIdentityNotFoundForRemoval);
    }
    items.retain(|item| {
        !update.replace_entity_ids.contains(&item.identity.entity_id)
            && !update.replace_identities.contains(&item.identity)
    });
    items.extend(update.items.clone());
    sort_managed_items(&mut items);
    let projection_digest = managed_projection_digest(
        target_key,
        committed_revision,
        proof.operation_id,
        &proof.receipt_digest,
        &items,
    )?;
    state.managed_target_states.insert(
        target_key.into(),
        ManagedTargetState {
            target_link_key: target_key.into(),
            adapter_id: proof.target.adapter_id.clone(),
            target_project_id: proof.target.target_project_id.clone(),
            scene_id: proof.target.scene_id.clone(),
            canonical_revision: committed_revision,
            source_operation_id: proof.operation_id,
            source_receipt_digest: proof.receipt_digest.clone(),
            items,
            projection_digest,
        },
    );
    Ok(())
}

fn validate_proof(proof: &VerifiedExternalCommit) -> Result<(), ProjectStoreError> {
    for (name, value) in [
        ("patchDigest", proof.patch_digest.as_str()),
        ("requestDigest", proof.request_digest.as_str()),
        ("receiptDigest", proof.receipt_digest.as_str()),
        ("adapterId", proof.target.adapter_id.as_str()),
        ("targetProjectId", proof.target.target_project_id.as_str()),
        ("sceneId", proof.target.scene_id.as_str()),
        (
            "targetIdentityDigest",
            proof.target.target_identity_digest.as_str(),
        ),
        (
            "verifiedFingerprint",
            proof.target.verified_fingerprint.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(ProjectStoreError::EmptyField(name));
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateDigestPayloadV1<'a> {
    schema_version: u32,
    project_id: &'a str,
    generation: u64,
    previous_state_digest: Option<&'a str>,
    bootstrap_revision: RevisionId,
    head: RevisionId,
    target_links: &'a BTreeMap<String, TargetLink>,
    external_commits: &'a BTreeMap<Uuid, ExternalCommitRecordV1>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalCommitRecordV1 {
    operation_id: Uuid,
    base_revision: RevisionId,
    committed_revision: RevisionId,
    patch_digest: String,
    request_digest: String,
    receipt_digest: String,
    target_link_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateDigestPayloadV2<'a> {
    schema_version: u32,
    project_id: &'a str,
    generation: u64,
    previous_state_digest: Option<&'a str>,
    bootstrap_revision: RevisionId,
    head: RevisionId,
    target_links: &'a BTreeMap<String, TargetLink>,
    external_commits: &'a BTreeMap<Uuid, ExternalCommitRecord>,
    managed_target_states: &'a BTreeMap<String, ManagedTargetState>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateDigestPayloadV3<'a> {
    schema_version: u32,
    project_id: &'a str,
    generation: u64,
    previous_state_digest: Option<&'a str>,
    bootstrap_revision: RevisionId,
    head: RevisionId,
    target_links: &'a BTreeMap<String, TargetLink>,
    external_commits: &'a BTreeMap<Uuid, ExternalCommitRecord>,
    managed_target_states: &'a BTreeMap<String, ManagedTargetState>,
    pending_external_commit: Option<&'a PendingExternalCommit>,
}

#[derive(Debug, Error)]
pub enum ProjectStoreError {
    #[error("required project-store field is empty: {0}")]
    EmptyField(&'static str),
    #[error("canonical project state is corrupt: {0}")]
    Corrupt(String),
    #[error("project revision is stale: expected {expected:?}, got {actual:?}")]
    StaleRevision {
        expected: RevisionId,
        actual: RevisionId,
    },
    #[error("operation ID was already committed with different evidence: {0}")]
    OperationConflict(Uuid),
    #[error("canonical project has a pending external commit reservation: {0}")]
    ExternalCommitReserved(Uuid),
    #[error("canonical external commit reservation is invalid")]
    InvalidExternalCommitReservation,
    #[error("durable target link was not found: {0}")]
    TargetLinkNotFound(String),
    #[error("more than one durable adapter link matches target: {0}")]
    AmbiguousTargetLink(String),
    #[error("target identity changed: expected {expected}, got {actual}")]
    TargetIdentityMismatch { expected: String, actual: String },
    #[error("receipt-derived managed projection is unavailable for target link: {0}")]
    ManagedStateUnavailable(String),
    #[error("verified managed-state update is empty")]
    EmptyManagedStateUpdate,
    #[error("managed-state receipt item is outside the declared replacement scope")]
    ManagedUpdateOutsideReplacementScope,
    #[error("receipt-authorized managed identity removal target is absent from canonical state")]
    ManagedIdentityNotFoundForRemoval,
    #[error("managed-state update is not bound to the external commit receipt")]
    ManagedUpdateReceiptMismatch,
    #[error("receipt-derived managed projection is invalid")]
    InvalidManagedProjection,
    #[error("project revision counter overflowed")]
    RevisionOverflow,
    #[error("project state generation counter overflowed")]
    GenerationOverflow,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::ManagedSemanticValue;
    use takegraph_node::{
        Ymm4ExistingNativeExtensionKind, Ymm4NativeExtensionApplyResponse,
        Ymm4NativeExtensionRealization, Ymm4NativeExtensionStatus, Ymm4OpaqueNativeEffect,
        Ymm4PreservedNativeField,
    };

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!("takegraph-project-store-{}", Uuid::new_v4()))
    }

    fn proof(operation_id: Uuid, request_digest: &str) -> VerifiedExternalCommit {
        VerifiedExternalCommit::from_receipt(
            operation_id,
            RevisionId(0),
            "patch-a",
            request_digest,
            &serde_json::json!({"verified": true, "operationId": operation_id}),
            VerifiedTargetBinding {
                adapter_id: "ymm4-4.55".into(),
                target_project_id: "target-project".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                verified_fingerprint: "sha256:after".into(),
            },
        )
        .unwrap()
    }

    fn managed_item(entity_id: &str, realization_id: Uuid, text: &str) -> ManagedSemanticItem {
        ManagedSemanticItem {
            identity: ManagedSemanticIdentity {
                entity_id: entity_id.into(),
                realization_id: Some(realization_id),
            },
            entity_revision: 3,
            realization_kind: "ymm4_native_voice".into(),
            owned_fields: BTreeMap::from([(
                "text".into(),
                ManagedSemanticValue::Text(text.into()),
            )]),
        }
    }

    fn proof_with_projection(
        operation_id: Uuid,
        base: RevisionId,
        identity: ManagedSemanticIdentity,
        items: &[ManagedSemanticItem],
    ) -> VerifiedExternalCommit {
        let receipt = Ymm4OperationReceipt {
            operation_id,
            request_digest: "request-projection".into(),
            project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            expected_fingerprint: "sha256:before".into(),
            status: takegraph_node::Ymm4OperationStatus::Verified,
            before_fingerprint: "sha256:before".into(),
            after_fingerprint: "sha256:after".into(),
            applied_items: items
                .iter()
                .map(|item| takegraph_node::Ymm4ManagedItem {
                    entity_id: item.identity.entity_id.clone(),
                    revision: item.entity_revision,
                    kind: takegraph_node::ManagedItemKind::Voice,
                    frame: 0,
                    layer: 0,
                    length: 1,
                    text: item.owned_fields.get("text").and_then(|value| match value {
                        ManagedSemanticValue::Text(text) => Some(text.clone()),
                        _ => None,
                    }),
                    audio_path: None,
                    artifact_hash: None,
                    speaker: None,
                    realization_id: item.identity.realization_id,
                })
                .collect(),
            verified: true,
            error: None,
        };
        let proof = VerifiedExternalCommit::from_receipt(
            operation_id,
            base,
            "patch-projection",
            "request-projection",
            &receipt,
            VerifiedTargetBinding {
                adapter_id: "ymm4-4.55".into(),
                target_project_id: "target-project".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                verified_fingerprint: "sha256:after".into(),
            },
        )
        .unwrap();
        proof
            .with_managed_state_update(
                VerifiedManagedStateUpdate::replacing_identities_from_receipt([identity], &receipt)
                    .unwrap(),
            )
            .unwrap()
    }

    fn native_extension_receipt(
        operation_id: Uuid,
        realizations: Vec<Ymm4NativeExtensionRealization>,
    ) -> Ymm4NativeExtensionApplyResponse {
        Ymm4NativeExtensionApplyResponse {
            operation_id,
            request_digest: "native-request".into(),
            project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            status: Ymm4NativeExtensionStatus::Verified,
            before_fingerprint: "sha256:before".into(),
            after_fingerprint: "sha256:after".into(),
            descriptor_catalog_digest: "sha256:catalog".into(),
            driver_profile_digest: "sha256:driver".into(),
            realizations,
            verified: true,
            error: None,
        }
    }

    fn metadata_detach_receipt(
        operation_id: Uuid,
        identity: &ManagedSemanticIdentity,
    ) -> Ymm4MetadataDetachReceipt {
        Ymm4MetadataDetachReceipt {
            operation_id,
            request_digest: "detach-request".into(),
            project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            source_revision: 1,
            expected_fingerprint: "sha256:before".into(),
            entity_id: identity.entity_id.clone(),
            realization_id: identity.realization_id.unwrap(),
            identity_carrier: "takegraph_remark_v2".into(),
            status: takegraph_node::Ymm4MetadataDetachStatus::Verified,
            before_fingerprint: "sha256:before".into(),
            after_fingerprint: "sha256:detached".into(),
            detached_item_count: 1,
            before_remark_digest: "a".repeat(64),
            expected_after_remark_digest: "b".repeat(64),
            remark_digest_after: "b".repeat(64),
            non_remark_content_digest_before: "c".repeat(64),
            non_remark_content_digest_after: "c".repeat(64),
            remark_absent: true,
            verified: true,
            error: None,
        }
    }

    #[test]
    fn verified_commit_and_target_link_survive_reopen() {
        let root = test_root();
        let operation_id = Uuid::new_v4();
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        assert_eq!(
            store
                .commit_verified_external(&proof(operation_id, "request-a"))
                .unwrap(),
            RevisionId(1)
        );
        drop(store);

        let reopened = DurableProjectStore::open(&root, "project-a").unwrap();
        let state = reopened.snapshot().unwrap();
        assert_eq!(state.head, RevisionId(1));
        assert_eq!(state.target_links.len(), 1);
        assert_eq!(
            state.external_commits[&operation_id].request_digest,
            "request-a"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Full receipt/store/reopen boundary is one atomic invariant.
    fn native_extension_receipt_projection_commits_atomically_and_respects_ownership() {
        let root = test_root();
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        let operation_id = Uuid::new_v4();
        let realization_id = Uuid::new_v4();
        let owned_fields = BTreeMap::from([
            ("logicalKey".into(), "portrait:portrait-01".into()),
            ("projectId".into(), "target-project".into()),
            ("entityId".into(), "portrait-01".into()),
            ("entityRevision".into(), "2".into()),
            ("kind".into(), "portrait".into()),
            ("frame".into(), "12".into()),
            ("layer".into(), "8".into()),
            ("length".into(), "60".into()),
            ("descriptorId".into(), "character.marisa".into()),
            // These are witnesses/preserved state, not semantic ownership.
            ("hostStateDigest".into(), "sha256:preserved".into()),
            ("footprintDigest".into(), "sha256:opaque-template".into()),
        ]);
        let realization = Ymm4NativeExtensionRealization {
            logical_key: "portrait:portrait-01".into(),
            realization_id,
            kind: Ymm4ExistingNativeExtensionKind::Portrait,
            project_id: "target-project".into(),
            entity_id: "portrait-01".into(),
            entity_revision: 2,
            frame: 12,
            layer: 8,
            length: 60,
            owned_state_digest: canonical_sha256(
                "takegraph-ymm4-native-extension-owned-state-v1",
                &owned_fields,
            )
            .unwrap(),
            owned_fields,
            preserved_fields: vec![Ymm4PreservedNativeField {
                field: "remark".into(),
                state_digest: "sha256:preserved".into(),
            }],
            state_digest: "sha256:native-state".into(),
            unknown_effects: vec![Ymm4OpaqueNativeEffect {
                stable_type_id: "third.party.Glow".into(),
                instance_key: "fx-1".into(),
                state_digest: "sha256:unknown".into(),
            }],
        };
        let receipt = native_extension_receipt(operation_id, vec![realization]);
        let identity = ManagedSemanticIdentity {
            entity_id: "portrait-01".into(),
            realization_id: Some(realization_id),
        };
        let update = VerifiedManagedStateUpdate::replacing_native_extensions_from_receipt(
            [identity.clone()],
            &receipt,
        )
        .unwrap();
        let proof = VerifiedExternalCommit::from_receipt(
            operation_id,
            RevisionId(0),
            "native-patch",
            "native-request",
            &receipt,
            VerifiedTargetBinding {
                adapter_id: "ymm4-4.55".into(),
                target_project_id: "target-project".into(),
                scene_id: "scene-a".into(),
                target_identity_digest: "sha256:target".into(),
                verified_fingerprint: "sha256:after".into(),
            },
        )
        .unwrap()
        .with_managed_state_update(update)
        .unwrap();
        assert_eq!(
            store.commit_verified_external(&proof).unwrap(),
            RevisionId(1)
        );

        let state = store.snapshot().unwrap();
        assert_eq!(state.generation, 1);
        let projection = state.managed_target_states.values().next().unwrap();
        assert_eq!(projection.canonical_revision, RevisionId(1));
        assert_eq!(projection.source_operation_id, operation_id);
        assert_eq!(projection.items.len(), 1);
        assert_eq!(projection.items[0].identity, identity);
        assert_eq!(projection.items[0].realization_kind, "ymm4_native_portrait");
        assert_eq!(
            projection.items[0].owned_fields.get("descriptorId"),
            Some(&ManagedSemanticValue::Text("character.marisa".into()))
        );
        assert!(
            !projection.items[0]
                .owned_fields
                .contains_key("hostStateDigest")
        );
        assert!(
            !projection.items[0]
                .owned_fields
                .contains_key("footprintDigest")
        );
        drop(store);

        let reopened = DurableProjectStore::open(&root, "project-a").unwrap();
        assert_eq!(
            reopened.snapshot().unwrap().managed_target_states,
            state.managed_target_states
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_operation_replay_is_idempotent_but_rebinding_conflicts() {
        let root = test_root();
        let operation_id = Uuid::new_v4();
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        let original = proof(operation_id, "request-a");
        assert_eq!(
            store.commit_verified_external(&original).unwrap(),
            RevisionId(1)
        );
        assert_eq!(
            store.commit_verified_external(&original).unwrap(),
            RevisionId(1)
        );
        assert_eq!(store.snapshot().unwrap().generation, 1);
        assert!(matches!(
            store.commit_verified_external(&proof(operation_id, "request-b")),
            Err(ProjectStoreError::OperationConflict(id)) if id == operation_id
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_external_reservation_survives_restart_and_is_consumed_by_proof() {
        let root = test_root();
        let operation_id = Uuid::new_v4();
        let proof = proof(operation_id, "request-a");
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        store
            .reserve_external_commit(
                operation_id,
                proof.base_revision,
                &proof.patch_digest,
                &proof.request_digest,
                &proof.target,
            )
            .unwrap();
        let mut rebound_target = proof.target.clone();
        rebound_target.target_identity_digest = "sha256:rebound-target".into();
        assert!(matches!(
            store.reserve_external_commit(
                operation_id,
                proof.base_revision,
                &proof.patch_digest,
                &proof.request_digest,
                &rebound_target,
            ),
            Err(ProjectStoreError::OperationConflict(id)) if id == operation_id
        ));
        assert!(matches!(
            store.abort_external_commit_reservation(
                operation_id,
                proof.base_revision,
                &proof.patch_digest,
                &proof.request_digest,
                &rebound_target,
            ),
            Err(ProjectStoreError::OperationConflict(id)) if id == operation_id
        ));
        assert_eq!(
            store
                .snapshot()
                .unwrap()
                .pending_external_commit
                .as_ref()
                .unwrap()
                .kind,
            PendingExternalCommitKind::ExternalMutation
        );
        drop(store);

        let reopened = DurableProjectStore::open(&root, "project-a").unwrap();
        reopened
            .reserve_external_commit(
                operation_id,
                proof.base_revision,
                &proof.patch_digest,
                &proof.request_digest,
                &proof.target,
            )
            .unwrap();
        assert!(matches!(
            reopened.reserve_external_commit(
                operation_id,
                proof.base_revision,
                &proof.patch_digest,
                &proof.request_digest,
                &rebound_target,
            ),
            Err(ProjectStoreError::OperationConflict(id)) if id == operation_id
        ));
        assert_eq!(
            reopened.commit_verified_external(&proof).unwrap(),
            RevisionId(1)
        );
        assert!(
            reopened
                .snapshot()
                .unwrap()
                .pending_external_commit
                .is_none()
        );
        // A task-file replay after the canonical publication accepts the same
        // reservation evidence without publishing another generation.
        reopened
            .reserve_external_commit(
                operation_id,
                proof.base_revision,
                &proof.patch_digest,
                &proof.request_digest,
                &proof.target,
            )
            .unwrap();
        assert_eq!(
            reopened.commit_verified_external(&proof).unwrap(),
            RevisionId(1)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // End-to-end reservation/restart/finalize evidence is intentionally contiguous.
    fn durable_detach_reservation_blocks_revision_race_and_finalizes_exactly_once() {
        let root = test_root();
        let realization_id = Uuid::new_v4();
        let identity = ManagedSemanticIdentity {
            entity_id: "utt-01".into(),
            realization_id: Some(realization_id),
        };
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        store
            .commit_verified_external(&proof_with_projection(
                Uuid::new_v4(),
                RevisionId(0),
                identity.clone(),
                &[managed_item("utt-01", realization_id, "before")],
            ))
            .unwrap();
        let operation_id = Uuid::new_v4();
        let target = VerifiedTargetBinding {
            adapter_id: "ymm4-4.55".into(),
            target_project_id: "target-project".into(),
            scene_id: "scene-a".into(),
            target_identity_digest: "sha256:target".into(),
            verified_fingerprint: "sha256:before".into(),
        };
        store
            .reserve_metadata_detach(
                operation_id,
                RevisionId(1),
                "detach-patch",
                "detach-request",
                &target,
                &identity,
            )
            .unwrap();
        let reserved = store.snapshot().unwrap();
        assert_eq!(reserved.head, RevisionId(1));
        assert_eq!(
            reserved
                .pending_external_commit
                .as_ref()
                .unwrap()
                .operation_id,
            operation_id
        );
        let reserved_generation = reserved.generation;

        // Exact replay after reopen is inert, while an unrelated canonical
        // commit cannot steal the reserved base revision.
        drop(store);
        let reopened = DurableProjectStore::open(&root, "project-a").unwrap();
        reopened
            .reserve_metadata_detach(
                operation_id,
                RevisionId(1),
                "detach-patch",
                "detach-request",
                &target,
                &identity,
            )
            .unwrap();
        assert_eq!(reopened.snapshot().unwrap().generation, reserved_generation);
        assert!(matches!(
            reopened.abort_metadata_detach_reservation(operation_id, "rebound-request"),
            Err(ProjectStoreError::OperationConflict(id)) if id == operation_id
        ));
        let mut competing = proof(Uuid::new_v4(), "competing-request");
        competing.base_revision = RevisionId(1);
        assert!(matches!(
            reopened.commit_verified_external(&competing),
            Err(ProjectStoreError::ExternalCommitReserved(id)) if id == operation_id
        ));

        let receipt = metadata_detach_receipt(operation_id, &identity);
        let update = VerifiedManagedStateUpdate::removing_identity_from_metadata_detach_receipt(
            identity, &receipt,
        )
        .unwrap();
        let proof = VerifiedExternalCommit::from_receipt(
            operation_id,
            RevisionId(1),
            "detach-patch",
            "detach-request",
            &receipt,
            VerifiedTargetBinding {
                verified_fingerprint: receipt.after_fingerprint.clone(),
                ..target
            },
        )
        .unwrap()
        .with_managed_state_update(update)
        .unwrap();
        assert_eq!(
            reopened.commit_verified_external(&proof).unwrap(),
            RevisionId(2)
        );
        assert_eq!(
            reopened.commit_verified_external(&proof).unwrap(),
            RevisionId(2)
        );
        let finalized = reopened.snapshot().unwrap();
        assert!(finalized.pending_external_commit.is_none());
        assert_eq!(finalized.head, RevisionId(2));
        assert!(
            finalized
                .managed_target_states
                .values()
                .next()
                .unwrap()
                .items
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_latest_generation_fails_closed() {
        let root = test_root();
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        store
            .commit_verified_external(&proof(Uuid::new_v4(), "request-a"))
            .unwrap();
        let latest = fs::read_dir(root.join("revisions"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .max()
            .unwrap();
        fs::write(latest, b"{corrupt").unwrap();
        assert!(matches!(
            store.snapshot(),
            Err(ProjectStoreError::Corrupt(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_historical_generation_breaks_the_hash_chain() {
        let root = test_root();
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        store
            .commit_verified_external(&proof(Uuid::new_v4(), "request-a"))
            .unwrap();
        let mut second = proof(Uuid::new_v4(), "request-b");
        second.base_revision = RevisionId(1);
        store.commit_verified_external(&second).unwrap();
        let historical = fs::read_dir(root.join("revisions"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("state-00000000000000000001-")
            })
            .unwrap();
        fs::write(historical, b"{corrupt").unwrap();

        assert!(matches!(
            store.snapshot(),
            Err(ProjectStoreError::Corrupt(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_base_never_advances_durable_head() {
        let root = test_root();
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        let mut stale = proof(Uuid::new_v4(), "request-a");
        stale.base_revision = RevisionId(2);
        assert!(matches!(
            store.commit_verified_external(&stale),
            Err(ProjectStoreError::StaleRevision { .. })
        ));
        assert_eq!(store.head().unwrap(), RevisionId(0));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn receipt_projection_survives_reopen_and_identity_delete() {
        let root = test_root();
        let realization_id = Uuid::new_v4();
        let identity = ManagedSemanticIdentity {
            entity_id: "utt-01".into(),
            realization_id: Some(realization_id),
        };
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        store
            .commit_verified_external(&proof_with_projection(
                Uuid::new_v4(),
                RevisionId(0),
                identity.clone(),
                &[managed_item("utt-01", realization_id, "before")],
            ))
            .unwrap();
        drop(store);

        let reopened = DurableProjectStore::open(&root, "project-a").unwrap();
        let expected = reopened
            .managed_state_for_linked_target("target-project", "scene-a", "sha256:target")
            .unwrap();
        assert_eq!(expected.len(), 1);
        assert_eq!(
            expected[0].owned_fields["text"],
            ManagedSemanticValue::Text("before".into())
        );

        reopened
            .commit_verified_external(&proof_with_projection(
                Uuid::new_v4(),
                RevisionId(1),
                identity,
                &[],
            ))
            .unwrap();
        assert!(
            reopened
                .managed_state_for_linked_target("target-project", "scene-a", "sha256:target")
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_projection_tamper_is_detected_by_state_hash() {
        let root = test_root();
        let realization_id = Uuid::new_v4();
        let identity = ManagedSemanticIdentity {
            entity_id: "utt-01".into(),
            realization_id: Some(realization_id),
        };
        let store = DurableProjectStore::open(&root, "project-a").unwrap();
        store
            .commit_verified_external(&proof_with_projection(
                Uuid::new_v4(),
                RevisionId(0),
                identity,
                &[managed_item("utt-01", realization_id, "original")],
            ))
            .unwrap();
        let latest = fs::read_dir(root.join("revisions"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .max()
            .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&latest).unwrap()).unwrap();
        let states = value["managedTargetStates"].as_object_mut().unwrap();
        let projection = states.values_mut().next().unwrap();
        projection["items"][0]["ownedFields"]["text"]["value"] =
            serde_json::Value::String("tampered".into());
        fs::write(&latest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        assert!(matches!(
            store.snapshot(),
            Err(ProjectStoreError::Corrupt(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_v1_state_without_projection_fields_remains_readable() {
        let root = test_root();
        fs::create_dir_all(root.join("revisions")).unwrap();
        let store = DurableProjectStore {
            root: root.clone(),
            project_id: "project-a".into(),
        };
        store
            .persist_unlocked(DurableProjectState {
                schema_version: 1,
                project_id: "project-a".into(),
                generation: 0,
                previous_state_digest: None,
                bootstrap_revision: RevisionId(0),
                head: RevisionId(0),
                target_links: BTreeMap::new(),
                external_commits: BTreeMap::new(),
                managed_target_states: BTreeMap::new(),
                pending_external_commit: None,
                state_digest: String::new(),
            })
            .unwrap();
        let path = fs::read_dir(root.join("revisions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("managedTargetStates");
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let reopened = DurableProjectStore::open(&root, "project-a").unwrap();
        assert_eq!(reopened.head().unwrap(), RevisionId(0));
        assert!(matches!(
            reopened.managed_state_for_linked_target("target-project", "scene-a", "target"),
            Err(ProjectStoreError::TargetLinkNotFound(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_v2_state_without_reservation_field_remains_readable() {
        let root = test_root();
        fs::create_dir_all(root.join("revisions")).unwrap();
        let store = DurableProjectStore {
            root: root.clone(),
            project_id: "project-a".into(),
        };
        store
            .persist_unlocked(DurableProjectState {
                schema_version: 2,
                project_id: "project-a".into(),
                generation: 0,
                previous_state_digest: None,
                bootstrap_revision: RevisionId(0),
                head: RevisionId(0),
                target_links: BTreeMap::new(),
                external_commits: BTreeMap::new(),
                managed_target_states: BTreeMap::new(),
                pending_external_commit: None,
                state_digest: String::new(),
            })
            .unwrap();
        let path = fs::read_dir(root.join("revisions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("pendingExternalCommit");
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let reopened = DurableProjectStore::open(&root, "project-a").unwrap();
        let state = reopened.snapshot().unwrap();
        assert_eq!(state.schema_version, 2);
        assert!(state.pending_external_commit.is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
