use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Deserializer, Serialize};
use takegraph_core::{
    CanonicalError, Patch, PatchError, PatchStatus, RevisionId, canonical_sha256,
};
use takegraph_node::{
    ArtifactError, CapabilityRequirement, ImportedYmm4NativeVoiceArtifact, ManagedItemKind,
    StructuredCapabilityError, StructuredYmm4Capabilities, Ymm4ApplyResponse, Ymm4BridgeClient,
    Ymm4Error, Ymm4ManagedItem, Ymm4NativeVoiceArtifactRequest, Ymm4NativeVoiceMutation,
    Ymm4NativeVoiceMutationAction, Ymm4NativeVoiceMutationApplyRequest,
    Ymm4NativeVoiceMutationPlanRequest, Ymm4NativeVoiceMutationPlanResponse, Ymm4OperationReceipt,
    Ymm4OperationStatus, Ymm4ProjectSnapshot, import_ymm4_native_voice_artifact,
};
use thiserror::Error;
use uuid::Uuid;

/// Digest-approved lifecycle for native voice create/update/delete operations.
///
/// Bridge receipts and artifact paths are deliberately not part of the approval
/// payload because they are authenticated outputs. A deserialized receipt is
/// never trusted until the exact request is replayed through the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4NativeVoiceMutationPatch {
    pub patch: Patch,
    pub operation_id: Uuid,
    pub target: Ymm4ProjectSnapshot,
    pub mutations: Vec<Ymm4NativeVoiceMutation>,
    pub plan: Ymm4NativeVoiceMutationPlanResponse,
    pub capability_digest: String,
    pub capability_requirements: Vec<CapabilityRequirement>,
    pub adapter_id: String,
    pub target_identity_digest: String,
    #[serde(default)]
    artifacts: Vec<ImportedYmm4NativeVoiceArtifact>,
    receipt: Option<Ymm4OperationReceipt>,
    #[serde(skip)]
    receipt_trusted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UncheckedMutationPatch {
    patch: Patch,
    operation_id: Uuid,
    target: Ymm4ProjectSnapshot,
    mutations: Vec<Ymm4NativeVoiceMutation>,
    plan: Ymm4NativeVoiceMutationPlanResponse,
    capability_digest: String,
    capability_requirements: Vec<CapabilityRequirement>,
    adapter_id: String,
    target_identity_digest: String,
    #[serde(default)]
    artifacts: Vec<ImportedYmm4NativeVoiceArtifact>,
    receipt: Option<Ymm4OperationReceipt>,
}

impl UncheckedMutationPatch {
    fn into_patch(self) -> Ymm4NativeVoiceMutationPatch {
        Ymm4NativeVoiceMutationPatch {
            patch: self.patch,
            operation_id: self.operation_id,
            target: self.target,
            mutations: self.mutations,
            plan: self.plan,
            capability_digest: self.capability_digest,
            capability_requirements: self.capability_requirements,
            adapter_id: self.adapter_id,
            target_identity_digest: self.target_identity_digest,
            artifacts: self.artifacts,
            receipt: self.receipt,
            receipt_trusted: false,
        }
    }
}

impl<'de> Deserialize<'de> for Ymm4NativeVoiceMutationPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let patch = UncheckedMutationPatch::deserialize(deserializer)?.into_patch();
        patch
            .validate_payload_digest()
            .map_err(<D::Error as serde::de::Error>::custom)?;
        Ok(patch)
    }
}

impl Ymm4NativeVoiceMutationPatch {
    /// Stages a mutation preview against a caller-loaded canonical revision.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid mutations, missing structured features,
    /// stale YMM4 state, bridge failure, or canonicalization failure.
    pub async fn stage(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        mutations: Vec<Ymm4NativeVoiceMutation>,
    ) -> Result<Self, Ymm4NativeVoiceMutationError> {
        let target = client.snapshot().await?;
        Self::stage_from_snapshot(client, head, target, mutations).await
    }

    /// Stages after the caller has resolved the durable project store from the
    /// same snapshot identity.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid mutations, missing features, stale state,
    /// bridge failure, or canonicalization failure.
    pub async fn stage_from_snapshot(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        mutations: Vec<Ymm4NativeVoiceMutation>,
    ) -> Result<Self, Ymm4NativeVoiceMutationError> {
        Self::stage_from_snapshot_with_operation_id(client, head, target, mutations, Uuid::new_v4())
            .await
    }

    /// Stages a reconciliation child using its already-durable operation ID.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty operation ID or the ordinary mutation
    /// staging validation, bridge, and canonicalization failures.
    pub async fn stage_from_snapshot_with_operation_id(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        mutations: Vec<Ymm4NativeVoiceMutation>,
        operation_id: Uuid,
    ) -> Result<Self, Ymm4NativeVoiceMutationError> {
        if operation_id.is_nil() {
            return Err(Ymm4NativeVoiceMutationError::InvalidMutation(
                "operation ID must not be nil".into(),
            ));
        }
        validate_mutations(&mutations)?;
        let capabilities = client.structured_capabilities().await?;
        let capability_requirements = mutation_capability_requirements(&capabilities, &mutations)?;
        capabilities.require(&capability_requirements)?;
        let plan = client
            .plan_native_voice_mutations(&Ymm4NativeVoiceMutationPlanRequest::new(
                target.fingerprint.clone(),
                mutations.clone(),
            ))
            .await?;
        validate_plan(&target, &mutations, &plan)?;
        Self::from_validated_plan(
            head,
            target,
            mutations,
            plan,
            capabilities,
            capability_requirements,
            operation_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_validated_plan(
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        mutations: Vec<Ymm4NativeVoiceMutation>,
        plan: Ymm4NativeVoiceMutationPlanResponse,
        capabilities: StructuredYmm4Capabilities,
        capability_requirements: Vec<CapabilityRequirement>,
        operation_id: Uuid,
    ) -> Result<Self, Ymm4NativeVoiceMutationError> {
        let target_identity = crate::ymm4_target_plan::ymm4_target_identity(&capabilities, &target);
        let target_identity_digest =
            canonical_sha256("takegraph-ymm4-target-identity", &target_identity)?;
        let digest = mutation_patch_digest(
            head,
            operation_id,
            &target,
            &mutations,
            &plan,
            &capabilities.capability_digest,
            &capability_requirements,
            &target_identity.adapter_id,
            &target_identity_digest,
        )?;
        let mut patch = Patch::draft(head, digest);
        patch.validate()?;
        patch.materialize_preview()?;
        Ok(Self {
            patch,
            operation_id,
            target,
            mutations,
            plan,
            capability_digest: capabilities.capability_digest,
            capability_requirements,
            adapter_id: target_identity.adapter_id,
            target_identity_digest,
            artifacts: Vec::new(),
            receipt: None,
            receipt_trusted: false,
        })
    }

    /// Parses only when every approval-bound field still hashes to `patch.digest`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON or a payload-digest mismatch.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, Ymm4NativeVoiceMutationError> {
        let patch = serde_json::from_slice::<UncheckedMutationPatch>(bytes)?.into_patch();
        patch.validate_payload_digest()?;
        Ok(patch)
    }

    /// Recomputes the exact approval payload digest.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid mutation/plan data, canonicalization
    /// failure, or any approval-bound payload change.
    pub fn validate_payload_digest(&self) -> Result<(), Ymm4NativeVoiceMutationError> {
        validate_mutations(&self.mutations)?;
        validate_plan(&self.target, &self.mutations, &self.plan)?;
        let canonical = mutation_patch_digest(
            self.patch.base,
            self.operation_id,
            &self.target,
            &self.mutations,
            &self.plan,
            &self.capability_digest,
            &self.capability_requirements,
            &self.adapter_id,
            &self.target_identity_digest,
        )?;
        if canonical != self.patch.digest {
            return Err(Ymm4NativeVoiceMutationError::PayloadDigestMismatch {
                stored: self.patch.digest.clone(),
                canonical,
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn receipt(&self) -> Option<&Ymm4OperationReceipt> {
        self.receipt.as_ref()
    }

    #[must_use]
    pub fn artifacts(&self) -> &[ImportedYmm4NativeVoiceArtifact] {
        &self.artifacts
    }

    /// Records approval for exactly the current digest and canonical base.
    ///
    /// # Errors
    ///
    /// Returns an error for payload tampering, digest mismatch, invalid patch
    /// lifecycle, or a stale canonical revision.
    pub fn approve(
        &mut self,
        approved_digest: &str,
        current_head: RevisionId,
    ) -> Result<(), Ymm4NativeVoiceMutationError> {
        self.validate_payload_digest()?;
        if approved_digest != self.patch.digest {
            return Err(Ymm4NativeVoiceMutationError::DigestMismatch);
        }
        if self.patch.status == PatchStatus::Previewable {
            self.patch.approve()?;
        }
        if current_head != self.patch.base {
            return Err(PatchError::StaleBase {
                expected: self.patch.base,
                actual: current_head,
            }
            .into());
        }
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        Ok(())
    }

    async fn apply_reserved_request(
        &mut self,
        client: &Ymm4BridgeClient,
        request: &Ymm4NativeVoiceMutationApplyRequest,
    ) -> Result<&Ymm4OperationReceipt, Ymm4NativeVoiceMutationError> {
        self.receipt_trusted = false;
        let first = client.apply_native_voice_mutations(request).await?;
        self.receipt = Some(first.receipt.clone());
        self.validate_apply_response(&first, request, false)?;
        let replay = client.apply_native_voice_mutations(request).await?;
        self.receipt = Some(replay.receipt.clone());
        self.validate_apply_response(&replay, request, true)?;
        if first.receipt != replay.receipt {
            return Err(Ymm4NativeVoiceMutationError::ReplayMismatch(
                "initial and replay receipts differ".into(),
            ));
        }
        let live = client.snapshot().await?;
        self.validate_live_snapshot(&live, &replay.receipt)?;
        self.receipt = Some(replay.receipt);
        self.receipt_trusted = true;
        self.receipt
            .as_ref()
            .ok_or(Ymm4NativeVoiceMutationError::MissingVerifiedReceipt)
    }

    async fn replay_reserved_request(
        &mut self,
        client: &Ymm4BridgeClient,
        request: &Ymm4NativeVoiceMutationApplyRequest,
    ) -> Result<&Ymm4OperationReceipt, Ymm4NativeVoiceMutationError> {
        self.receipt_trusted = false;
        let replay = client.apply_native_voice_mutations(request).await?;
        self.receipt = Some(replay.receipt.clone());
        self.validate_apply_response(&replay, request, true)?;
        self.receipt_trusted = true;
        self.receipt
            .as_ref()
            .ok_or(Ymm4NativeVoiceMutationError::MissingVerifiedReceipt)
    }

    /// Applies/replays the approved native voice mutation, verifies fresh
    /// semantic state, and durably publishes it while holding the common
    /// project-wide external-mutation fence.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale head, an active detach reservation, bridge
    /// apply/replay/read-back failure, or durable publication failure.
    #[allow(clippy::too_many_lines)]
    pub async fn apply_and_finalize_durable(
        &mut self,
        client: &Ymm4BridgeClient,
        store: &crate::DurableProjectStore,
        current_head: RevisionId,
    ) -> Result<crate::DurableExternalMutationOutcome, Ymm4NativeVoiceMutationError> {
        self.validate_payload_digest()?;
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        let request = self.apply_request();
        let target = crate::VerifiedTargetBinding {
            adapter_id: self.adapter_id.clone(),
            target_project_id: self.target.project_id.clone(),
            scene_id: self.target.scene_id.clone(),
            target_identity_digest: self.target_identity_digest.clone(),
            verified_fingerprint: self.target.fingerprint.clone(),
        };
        let scope = crate::external_mutation::acquire_external_mutation_scope(
            store,
            self.operation_id,
            current_head,
        )?;
        if scope.committed.is_none() && !scope.pending_replay {
            self.validate_current_capabilities(client).await?;
        }
        store.reserve_external_commit(
            self.operation_id,
            self.patch.base,
            &self.patch.digest,
            &request.request_digest,
            &target,
        )?;
        if let Some(committed) = scope.committed.as_ref() {
            self.replay_committed(client, &request, committed).await?;
            let revision = self.finalize_durable(store)?;
            return Ok(crate::DurableExternalMutationOutcome {
                revision,
                canonical_replay: true,
            });
        }
        let mut replay_existing_bridge_operation = false;
        if scope.pending_replay {
            match client.operation(self.operation_id).await {
                Ok(receipt) => {
                    let request_bound =
                        crate::external_mutation::operation_receipt_is_request_bound(
                            &receipt,
                            self.operation_id,
                            &request.request_digest,
                            &request.project_id,
                            &request.scene_id,
                            &request.expected_fingerprint,
                        );
                    let terminal = matches!(
                        receipt.status,
                        Ymm4OperationStatus::NotStarted
                            | Ymm4OperationStatus::Failed
                            | Ymm4OperationStatus::RolledBack
                            | Ymm4OperationStatus::RecoveryRequired
                    );
                    if !request_bound || terminal {
                        let safe_abort =
                            crate::external_mutation::operation_receipt_proves_exact_rollback(
                                &receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.project_id,
                                &request.scene_id,
                                &request.expected_fingerprint,
                            ) || crate::external_mutation::operation_receipt_proves_no_mutation(
                                &receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.project_id,
                                &request.scene_id,
                                &request.expected_fingerprint,
                            ) || crate::external_mutation::operation_receipt_proves_not_started(
                                &receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.project_id,
                                &request.scene_id,
                                &request.expected_fingerprint,
                            );
                        let message = receipt.error.clone().unwrap_or_else(|| {
                            "reserved native-voice mutation has untrusted terminal evidence".into()
                        });
                        self.receipt = Some(receipt);
                        if safe_abort {
                            store.abort_external_commit_reservation(
                                self.operation_id,
                                self.patch.base,
                                &self.patch.digest,
                                &request.request_digest,
                                &target,
                            )?;
                        }
                        return Err(Ymm4NativeVoiceMutationError::ApplyFailed(message));
                    }
                    replay_existing_bridge_operation = true;
                }
                Err(Ymm4Error::Bridge { status, .. }) if status.as_u16() == 404 => {
                    let sealed = client
                        .seal_native_voice_mutation_not_started(&request)
                        .await?;
                    let not_started = !sealed.success
                        && crate::external_mutation::operation_receipt_proves_not_started(
                            &sealed.receipt,
                            self.operation_id,
                            &request.request_digest,
                            &request.project_id,
                            &request.scene_id,
                            &request.expected_fingerprint,
                        );
                    let message = sealed.receipt.error.clone().unwrap_or_else(|| {
                        "reserved native-voice mutation has no trusted recovery evidence".into()
                    });
                    self.receipt = Some(sealed.receipt);
                    if not_started {
                        store.abort_external_commit_reservation(
                            self.operation_id,
                            self.patch.base,
                            &self.patch.digest,
                            &request.request_digest,
                            &target,
                        )?;
                        return Err(Ymm4NativeVoiceMutationError::ApplyFailed(message));
                    }
                    if !self.receipt.as_ref().is_some_and(|receipt| {
                        crate::external_mutation::operation_receipt_is_request_bound(
                            receipt,
                            self.operation_id,
                            &request.request_digest,
                            &request.project_id,
                            &request.scene_id,
                            &request.expected_fingerprint,
                        )
                    }) {
                        return Err(Ymm4NativeVoiceMutationError::ApplyFailed(message));
                    }
                    replay_existing_bridge_operation = true;
                }
                Err(error) => return Err(error.into()),
            }
        }

        let apply_result = if replay_existing_bridge_operation {
            self.replay_reserved_request(client, &request).await
        } else {
            self.apply_reserved_request(client, &request).await
        };
        if let Err(error) = apply_result {
            let safe_abort = self.receipt.as_ref().is_some_and(|receipt| {
                crate::external_mutation::operation_receipt_proves_exact_rollback(
                    receipt,
                    self.operation_id,
                    &request.request_digest,
                    &request.project_id,
                    &request.scene_id,
                    &request.expected_fingerprint,
                ) || crate::external_mutation::operation_receipt_proves_no_mutation(
                    receipt,
                    self.operation_id,
                    &request.request_digest,
                    &request.project_id,
                    &request.scene_id,
                    &request.expected_fingerprint,
                ) || crate::external_mutation::operation_receipt_proves_not_started(
                    receipt,
                    self.operation_id,
                    &request.request_digest,
                    &request.project_id,
                    &request.scene_id,
                    &request.expected_fingerprint,
                )
            });
            let sealed_not_started = if safe_abort {
                false
            } else {
                match client
                    .seal_native_voice_mutation_not_started(&request)
                    .await
                {
                    Ok(sealed) => {
                        let proof = !sealed.success
                            && crate::external_mutation::operation_receipt_proves_not_started(
                                &sealed.receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.project_id,
                                &request.scene_id,
                                &request.expected_fingerprint,
                            );
                        self.receipt = Some(sealed.receipt);
                        proof
                    }
                    Err(_) => false,
                }
            };
            if safe_abort || sealed_not_started {
                store.abort_external_commit_reservation(
                    self.operation_id,
                    self.patch.base,
                    &self.patch.digest,
                    &request.request_digest,
                    &target,
                )?;
            }
            return Err(error);
        }
        Ok(crate::DurableExternalMutationOutcome {
            revision: self.finalize_durable(store)?,
            canonical_replay: false,
        })
    }

    async fn replay_committed(
        &mut self,
        client: &Ymm4BridgeClient,
        request: &Ymm4NativeVoiceMutationApplyRequest,
        committed: &crate::ExternalCommitRecord,
    ) -> Result<(), Ymm4NativeVoiceMutationError> {
        self.receipt_trusted = false;
        let replay = client.apply_native_voice_mutations(request).await?;
        self.receipt = Some(replay.receipt.clone());
        self.validate_apply_response(&replay, request, true)?;
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(Ymm4NativeVoiceMutationError::MissingVerifiedReceipt)?;
        if canonical_sha256("takegraph-external-receipt", receipt)? != committed.receipt_digest {
            return Err(Ymm4NativeVoiceMutationError::DurableReceiptMismatch);
        }
        self.receipt_trusted = true;
        Ok(())
    }

    /// Advances the canonical revision only from an authenticated replay and
    /// verified live readback. The durable store supplies the revision CAS.
    ///
    /// # Errors
    ///
    /// Returns an error for untrusted evidence, invalid lifecycle state, stale
    /// durable head, conflicting replay, or corrupt storage.
    pub(crate) fn finalize_durable(
        &mut self,
        store: &crate::DurableProjectStore,
    ) -> Result<RevisionId, Ymm4NativeVoiceMutationError> {
        self.validate_payload_digest()?;
        if !self.receipt_trusted {
            return Err(Ymm4NativeVoiceMutationError::UntrustedReceipt);
        }
        if self.patch.status == PatchStatus::Approved {
            self.patch.authorize_commit(self.patch.base)?;
        } else if self.patch.status != PatchStatus::Committed {
            return Err(PatchError::UnexpectedStatus {
                expected: PatchStatus::Approved,
                actual: self.patch.status,
            }
            .into());
        }
        let receipt = self.validate_verified_receipt(
            self.receipt
                .as_ref()
                .ok_or(Ymm4NativeVoiceMutationError::MissingVerifiedReceipt)?,
            &self.apply_request(),
        )?;
        let managed_state_update =
            crate::project_store::VerifiedManagedStateUpdate::replacing_identities_from_receipt(
                self.mutations
                    .iter()
                    .map(|mutation| takegraph_core::ManagedSemanticIdentity {
                        entity_id: mutation.entity_id.clone(),
                        realization_id: Some(mutation.realization_id),
                    }),
                receipt,
            )?;
        let proof = crate::project_store::VerifiedExternalCommit::from_receipt(
            self.operation_id,
            self.patch.base,
            self.patch.digest.clone(),
            receipt.request_digest.clone(),
            receipt,
            crate::VerifiedTargetBinding {
                adapter_id: self.adapter_id.clone(),
                target_project_id: self.target.project_id.clone(),
                scene_id: self.target.scene_id.clone(),
                target_identity_digest: self.target_identity_digest.clone(),
                verified_fingerprint: receipt.after_fingerprint.clone(),
            },
        )?
        .with_managed_state_update(managed_state_update)?;
        let revision = store.commit_verified_external(&proof)?;
        let expected = self
            .patch
            .base
            .checked_next()
            .ok_or(PatchError::RevisionOverflow)?;
        if revision != expected {
            return Err(Ymm4NativeVoiceMutationError::DurableRevisionMismatch {
                expected,
                actual: revision,
            });
        }
        if self.patch.status == PatchStatus::Approved {
            let core_revision = self.patch.commit(self.patch.base)?;
            debug_assert_eq!(core_revision, revision);
        }
        Ok(revision)
    }

    /// Verifies the current project/scene and create/update/delete semantics.
    ///
    /// # Errors
    ///
    /// Returns an error for payload tampering, bridge failure, target drift, or
    /// semantic mismatch.
    pub async fn verify(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4NativeVoiceMutationError> {
        self.validate_payload_digest()?;
        let snapshot = client.snapshot().await?;
        validate_target(&self.target, &snapshot)?;
        verify_mutation_items(&self.mutations, &snapshot.managed_items)
    }

    /// Re-authenticates the committed request, exports each surviving voice,
    /// and imports exact WAV + host-bound provenance into TakeGraph-owned CAS.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing durable commit, capability/target drift,
    /// replay mismatch, unsafe bridge paths, false content claims, malformed
    /// WAV/provenance, or immutable artifact collision.
    pub async fn capture_artifacts(
        &mut self,
        client: &Ymm4BridgeClient,
        store: &crate::DurableProjectStore,
        authorized_bridge_root: &Path,
        artifact_root: &Path,
    ) -> Result<&[ImportedYmm4NativeVoiceArtifact], Ymm4NativeVoiceMutationError> {
        self.validate_payload_digest()?;
        self.require_durable_commit(store)?;
        let capabilities = self.validate_current_capabilities(client).await?;
        require_feature(&capabilities, "voiceItem.artifactExport")?;
        let request = self.apply_request();
        let replay = client.apply_native_voice_mutations(&request).await?;
        self.validate_apply_response(&replay, &request, true)?;
        let live = client.snapshot().await?;
        self.validate_live_snapshot(&live, &replay.receipt)?;

        let mut imported = Vec::new();
        for mutation in self
            .mutations
            .iter()
            .filter(|value| value.action != Ymm4NativeVoiceMutationAction::Delete)
        {
            let staged = client
                .export_native_voice_artifact(&Ymm4NativeVoiceArtifactRequest::new(
                    live.project_id.clone(),
                    live.scene_id.clone(),
                    live.fingerprint.clone(),
                    mutation.realization_id,
                ))
                .await?;
            if staged.realization_id != mutation.realization_id {
                return Err(Ymm4NativeVoiceMutationError::ArtifactIdentityMismatch {
                    expected: mutation.realization_id,
                    actual: staged.realization_id,
                });
            }
            imported.push(import_ymm4_native_voice_artifact(
                &staged,
                authorized_bridge_root,
                artifact_root,
                &mutation.character_name,
                &mutation.display_text,
                &mutation.spoken_text,
            )?);
        }
        self.receipt = Some(replay.receipt);
        self.receipt_trusted = true;
        self.artifacts = imported;
        Ok(&self.artifacts)
    }

    /// Rehashes and reparses all persisted CAS receipts.
    ///
    /// # Errors
    ///
    /// Returns an error if a persisted artifact escaped the CAS root, changed,
    /// or no longer has the recorded WAV/provenance semantics.
    pub fn verify_artifacts(
        &self,
        artifact_root: &Path,
    ) -> Result<(), Ymm4NativeVoiceMutationError> {
        let expected = self
            .mutations
            .iter()
            .filter(|mutation| mutation.action != Ymm4NativeVoiceMutationAction::Delete)
            .map(|mutation| (mutation.realization_id, mutation))
            .collect::<std::collections::HashMap<_, _>>();
        let actual_ids = self
            .artifacts
            .iter()
            .map(|artifact| artifact.realization_id)
            .collect::<HashSet<_>>();
        if actual_ids.len() != self.artifacts.len()
            || actual_ids.len() != expected.len()
            || !actual_ids
                .iter()
                .all(|identity| expected.contains_key(identity))
        {
            return Err(Ymm4NativeVoiceMutationError::VerifyMismatch(
                "native voice artifact evidence is missing, duplicated, or unexpected".into(),
            ));
        }
        for artifact in &self.artifacts {
            let mutation = expected.get(&artifact.realization_id).ok_or_else(|| {
                Ymm4NativeVoiceMutationError::VerifyMismatch(format!(
                    "native voice artifact {} is not bound to a surviving mutation",
                    artifact.realization_id
                ))
            })?;
            artifact.verify_for(
                artifact_root,
                &mutation.character_name,
                &mutation.display_text,
                &mutation.spoken_text,
            )?;
        }
        Ok(())
    }

    fn apply_request(&self) -> Ymm4NativeVoiceMutationApplyRequest {
        Ymm4NativeVoiceMutationApplyRequest::new(
            self.operation_id,
            self.target.project_id.clone(),
            self.target.scene_id.clone(),
            self.target.fingerprint.clone(),
            self.mutations.clone(),
        )
    }

    async fn validate_current_capabilities(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<StructuredYmm4Capabilities, Ymm4NativeVoiceMutationError> {
        let actual = client.structured_capabilities().await?;
        if actual.capability_digest != self.capability_digest {
            return Err(Ymm4NativeVoiceMutationError::CapabilityDrift {
                expected: self.capability_digest.clone(),
                actual: actual.capability_digest,
            });
        }
        actual.require(&self.capability_requirements)?;
        Ok(actual)
    }

    fn validate_apply_response(
        &self,
        response: &Ymm4ApplyResponse,
        request: &Ymm4NativeVoiceMutationApplyRequest,
        require_replay: bool,
    ) -> Result<(), Ymm4NativeVoiceMutationError> {
        if !response.success {
            return Err(Ymm4NativeVoiceMutationError::ApplyFailed(
                response
                    .receipt
                    .error
                    .clone()
                    .unwrap_or_else(|| "bridge rejected native voice mutation".into()),
            ));
        }
        if require_replay && !response.replayed {
            return Err(Ymm4NativeVoiceMutationError::ReplayMismatch(
                "bridge did not mark the second identical request as replayed".into(),
            ));
        }
        self.validate_verified_receipt(&response.receipt, request)?;
        Ok(())
    }

    fn validate_verified_receipt<'a>(
        &self,
        receipt: &'a Ymm4OperationReceipt,
        request: &Ymm4NativeVoiceMutationApplyRequest,
    ) -> Result<&'a Ymm4OperationReceipt, Ymm4NativeVoiceMutationError> {
        if !receipt.verified
            || receipt.status != Ymm4OperationStatus::Verified
            || receipt.operation_id != request.operation_id
            || receipt.request_digest != request.request_digest
            || receipt.project_id != request.project_id
            || receipt.scene_id != request.scene_id
            || receipt.expected_fingerprint != request.expected_fingerprint
            || receipt.before_fingerprint != request.expected_fingerprint
            || receipt.after_fingerprint.trim().is_empty()
        {
            return Err(Ymm4NativeVoiceMutationError::ReceiptBindingMismatch);
        }
        verify_mutation_items(&self.mutations, &receipt.applied_items)?;
        Ok(receipt)
    }

    fn validate_live_snapshot(
        &self,
        live: &Ymm4ProjectSnapshot,
        receipt: &Ymm4OperationReceipt,
    ) -> Result<(), Ymm4NativeVoiceMutationError> {
        validate_target(&self.target, live)?;
        if live.fingerprint != receipt.after_fingerprint {
            return Err(Ymm4NativeVoiceMutationError::VerifyMismatch(format!(
                "live fingerprint {} differs from receipt {}",
                live.fingerprint, receipt.after_fingerprint
            )));
        }
        verify_mutation_items(&self.mutations, &live.managed_items)
    }

    fn require_durable_commit(
        &self,
        store: &crate::DurableProjectStore,
    ) -> Result<(), Ymm4NativeVoiceMutationError> {
        if self.patch.status != PatchStatus::Committed {
            return Err(Ymm4NativeVoiceMutationError::ArtifactRequiresCommittedPatch);
        }
        let request = self.apply_request();
        let state = store.snapshot()?;
        let record = state.external_commits.get(&self.operation_id).ok_or(
            Ymm4NativeVoiceMutationError::MissingDurableCommit(self.operation_id),
        )?;
        if record.base_revision != self.patch.base
            || record.patch_digest != self.patch.digest
            || record.request_digest != request.request_digest
        {
            return Err(Ymm4NativeVoiceMutationError::DurableCommitMismatch);
        }
        Ok(())
    }
}

fn mutation_capability_requirements(
    capabilities: &StructuredYmm4Capabilities,
    mutations: &[Ymm4NativeVoiceMutation],
) -> Result<Vec<CapabilityRequirement>, Ymm4NativeVoiceMutationError> {
    let mut names = vec!["timeline.transaction", "readback.semantic"];
    if mutations
        .iter()
        .any(|value| value.action == Ymm4NativeVoiceMutationAction::Create)
    {
        names.push("voiceItem.create");
    }
    if mutations
        .iter()
        .any(|value| value.action == Ymm4NativeVoiceMutationAction::Update)
    {
        names.push("voiceItem.update");
    }
    if mutations
        .iter()
        .any(|value| value.action == Ymm4NativeVoiceMutationAction::Delete)
    {
        names.push("voiceItem.delete");
    }
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let feature = capabilities.feature(name).ok_or_else(|| {
                Ymm4NativeVoiceMutationError::InvalidCapabilityContract(format!(
                    "structured feature is missing: {name}"
                ))
            })?;
            Ok(CapabilityRequirement {
                feature: name.into(),
                minimum_version: 1,
                schema_digest: Some(feature.schema_digest.clone()),
            })
        })
        .collect()
}

fn require_feature(
    capabilities: &StructuredYmm4Capabilities,
    name: &str,
) -> Result<(), Ymm4NativeVoiceMutationError> {
    let feature = capabilities.feature(name).ok_or_else(|| {
        Ymm4NativeVoiceMutationError::InvalidCapabilityContract(format!(
            "structured feature is missing: {name}"
        ))
    })?;
    capabilities.require(&[CapabilityRequirement {
        feature: name.into(),
        minimum_version: 1,
        schema_digest: Some(feature.schema_digest.clone()),
    }])?;
    Ok(())
}

fn validate_mutations(
    mutations: &[Ymm4NativeVoiceMutation],
) -> Result<(), Ymm4NativeVoiceMutationError> {
    if mutations.is_empty() || mutations.len() > 128 {
        return Err(Ymm4NativeVoiceMutationError::InvalidMutation(
            "1-128 mutations are required".into(),
        ));
    }
    let mut realization_ids = HashSet::new();
    let mut entity_ids = HashSet::new();
    for mutation in mutations {
        if mutation.realization_id.is_nil()
            || mutation.entity_id.trim().is_empty()
            || !realization_ids.insert(mutation.realization_id)
            || !entity_ids.insert(mutation.entity_id.as_str())
            || mutation.frame < 0
            || mutation.layer < 0
            || mutation.max_length <= 0
        {
            return Err(Ymm4NativeVoiceMutationError::InvalidMutation(
                "mutations require unique non-empty identities and non-negative bounded placement"
                    .into(),
            ));
        }
        if mutation.action != Ymm4NativeVoiceMutationAction::Delete
            && (mutation.character_name.trim().is_empty()
                || mutation.display_text.trim().is_empty()
                || mutation.spoken_text.trim().is_empty()
                || mutation.display_text != mutation.spoken_text)
        {
            return Err(Ymm4NativeVoiceMutationError::InvalidMutation(
                "create/update requires exact character, text, and equal display/spoken text"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn validate_plan(
    target: &Ymm4ProjectSnapshot,
    mutations: &[Ymm4NativeVoiceMutation],
    plan: &Ymm4NativeVoiceMutationPlanResponse,
) -> Result<(), Ymm4NativeVoiceMutationError> {
    let count = |action| {
        mutations
            .iter()
            .filter(|value| value.action == action)
            .count()
    };
    if plan.fingerprint != target.fingerprint
        || plan.create_count != count(Ymm4NativeVoiceMutationAction::Create)
        || plan.update_count != count(Ymm4NativeVoiceMutationAction::Update)
        || plan.delete_count != count(Ymm4NativeVoiceMutationAction::Delete)
        || plan.duration_resolution != "bounded"
    {
        return Err(Ymm4NativeVoiceMutationError::InvalidPlan(
            "bridge plan is not bound to the requested fingerprint/actions/duration contract"
                .into(),
        ));
    }
    if plan.update_count > 0 && plan.preserved_fields.is_empty() {
        return Err(Ymm4NativeVoiceMutationError::InvalidPlan(
            "update plan did not declare its preserved target-local field groups".into(),
        ));
    }
    Ok(())
}

fn validate_target(
    expected: &Ymm4ProjectSnapshot,
    actual: &Ymm4ProjectSnapshot,
) -> Result<(), Ymm4NativeVoiceMutationError> {
    if expected.project_id != actual.project_id || expected.scene_id != actual.scene_id {
        return Err(Ymm4NativeVoiceMutationError::VerifyMismatch(
            "active YMM4 project or scene differs from the approved target".into(),
        ));
    }
    Ok(())
}

fn verify_mutation_items(
    mutations: &[Ymm4NativeVoiceMutation],
    items: &[Ymm4ManagedItem],
) -> Result<(), Ymm4NativeVoiceMutationError> {
    for mutation in mutations {
        let matching = items
            .iter()
            .filter(|item| item.realization_id == Some(mutation.realization_id))
            .collect::<Vec<_>>();
        if mutation.action == Ymm4NativeVoiceMutationAction::Delete {
            if !matching.is_empty() {
                return Err(Ymm4NativeVoiceMutationError::VerifyMismatch(format!(
                    "deleted realization {} remains present",
                    mutation.realization_id
                )));
            }
            continue;
        }
        let [item] = matching.as_slice() else {
            return Err(Ymm4NativeVoiceMutationError::VerifyMismatch(format!(
                "realization {} expected exactly once, found {}",
                mutation.realization_id,
                matching.len()
            )));
        };
        if item.kind != ManagedItemKind::Voice
            || item.entity_id != mutation.entity_id
            || item.revision != mutation.revision
            || item.speaker.as_deref() != Some(mutation.character_name.as_str())
            || item.text.as_deref() != Some(mutation.display_text.as_str())
            || item.frame != mutation.frame
            || item.layer != mutation.layer
            || item.length <= 0
            || item.length > mutation.max_length
        {
            return Err(Ymm4NativeVoiceMutationError::VerifyMismatch(format!(
                "realization {} does not match approved identity/text/placement/duration",
                mutation.realization_id
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn mutation_patch_digest(
    head: RevisionId,
    operation_id: Uuid,
    target: &Ymm4ProjectSnapshot,
    mutations: &[Ymm4NativeVoiceMutation],
    plan: &Ymm4NativeVoiceMutationPlanResponse,
    capability_digest: &str,
    capability_requirements: &[CapabilityRequirement],
    adapter_id: &str,
    target_identity_digest: &str,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-ymm4-native-voice-mutation-patch-v1",
        &serde_json::json!({
            "baseRevision": head,
            "operationId": operation_id,
            "target": target,
            "mutations": mutations,
            "plan": plan,
            "capabilityDigest": capability_digest,
            "capabilityRequirements": capability_requirements,
            "adapterId": adapter_id,
            "targetIdentityDigest": target_identity_digest,
        }),
    )
}

#[derive(Debug, Error)]
pub enum Ymm4NativeVoiceMutationError {
    #[error(transparent)]
    Bridge(#[from] Ymm4Error),
    #[error(transparent)]
    Core(#[from] PatchError),
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    #[error(transparent)]
    Capability(#[from] StructuredCapabilityError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    ProjectStore(#[from] crate::ProjectStoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid native voice mutation: {0}")]
    InvalidMutation(String),
    #[error("invalid native voice mutation plan: {0}")]
    InvalidPlan(String),
    #[error("invalid structured capability contract: {0}")]
    InvalidCapabilityContract(String),
    #[error("approval does not match the current native voice mutation digest")]
    DigestMismatch,
    #[error(
        "stored native voice mutation digest does not match its payload (stored {stored}, canonical {canonical})"
    )]
    PayloadDigestMismatch { stored: String, canonical: String },
    #[error("YMM4 capabilities changed after preview (expected {expected}, got {actual})")]
    CapabilityDrift { expected: String, actual: String },
    #[error("native voice mutation apply failed: {0}")]
    ApplyFailed(String),
    #[error("native voice mutation receipt is not bound to the exact request")]
    ReceiptBindingMismatch,
    #[error("native voice mutation replay failed: {0}")]
    ReplayMismatch(String),
    #[error("native voice mutation has no verified receipt")]
    MissingVerifiedReceipt,
    #[error("deserialized native voice mutation receipt must be replay-authenticated")]
    UntrustedReceipt,
    #[error("native voice mutation readback mismatch: {0}")]
    VerifyMismatch(String),
    #[error("durable revision mismatch: expected {expected:?}, got {actual:?}")]
    DurableRevisionMismatch {
        expected: RevisionId,
        actual: RevisionId,
    },
    #[error("artifact capture requires a committed mutation patch")]
    ArtifactRequiresCommittedPatch,
    #[error("durable commit is missing for operation {0}")]
    MissingDurableCommit(Uuid),
    #[error("durable commit does not bind this patch and bridge request")]
    DurableCommitMismatch,
    #[error("authenticated native-voice replay does not match the durable receipt digest")]
    DurableReceiptMismatch,
    #[error("artifact realization mismatch: expected {expected}, got {actual}")]
    ArtifactIdentityMismatch { expected: Uuid, actual: Uuid },
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
                plugin_version: "0.3.0".into(),
                ymm4_version: "4.55.1.1".into(),
            },
            &Ymm4Capabilities {
                protocol_version: 2,
                capabilities: vec![
                    Ymm4Capability::NativeVoiceCreate,
                    Ymm4Capability::NativeVoiceUpdateReplacePreservingUserState,
                    Ymm4Capability::NativeVoiceDelete,
                    Ymm4Capability::NativeVoiceExactWavExport,
                    Ymm4Capability::NativeVoiceHostBoundProvenance,
                    Ymm4Capability::NativeVoiceRemarkIdentity,
                    Ymm4Capability::NativeVoiceBoundedDuration,
                    Ymm4Capability::ReadbackVerification,
                    Ymm4Capability::IdempotentApply,
                    Ymm4Capability::RequestBoundReceipts,
                    Ymm4Capability::WriteAheadApply,
                    Ymm4Capability::MutationProfileYmm4_4_55_1_1,
                ],
            },
        )
        .unwrap()
    }

    fn mutation(action: Ymm4NativeVoiceMutationAction) -> Ymm4NativeVoiceMutation {
        Ymm4NativeVoiceMutation {
            realization_id: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
            entity_id: "utt-01".into(),
            revision: 4,
            character_name: "魔理沙".into(),
            display_text: "ここから第二形態だぜ".into(),
            spoken_text: "ここから第二形態だぜ".into(),
            frame: 120,
            layer: 20,
            max_length: 180,
            action,
        }
    }

    fn item() -> Ymm4ManagedItem {
        let mutation = mutation(Ymm4NativeVoiceMutationAction::Update);
        Ymm4ManagedItem {
            entity_id: mutation.entity_id,
            revision: mutation.revision,
            kind: ManagedItemKind::Voice,
            frame: mutation.frame,
            layer: mutation.layer,
            length: 90,
            text: Some(mutation.display_text),
            audio_path: None,
            artifact_hash: None,
            speaker: Some(mutation.character_name),
            realization_id: Some(mutation.realization_id),
        }
    }

    fn staged_patch() -> Ymm4NativeVoiceMutationPatch {
        let capabilities = capabilities();
        let mutations = vec![mutation(Ymm4NativeVoiceMutationAction::Update)];
        let target = Ymm4ProjectSnapshot {
            project_id: "project-a".into(),
            project_name: "test".into(),
            project_path: "test.ymmp".into(),
            scene_id: "scene-a".into(),
            fps: 60,
            fingerprint: "external-a".into(),
            managed_items: vec![item()],
            native_extensions: vec![],
            unmanaged_context_count: 0,
        };
        let plan = Ymm4NativeVoiceMutationPlanResponse {
            fingerprint: target.fingerprint.clone(),
            create_count: 0,
            update_count: 1,
            delete_count: 0,
            duration_resolution: "bounded".into(),
            preserved_fields: vec!["audioEffects".into()],
        };
        let requirements = mutation_capability_requirements(&capabilities, &mutations).unwrap();
        let adapter_id = capabilities.driver.id.clone();
        let target_identity_digest = String::from("sha256:target-a");
        let operation_id = Uuid::nil();
        let base = RevisionId(7);
        let digest = mutation_patch_digest(
            base,
            operation_id,
            &target,
            &mutations,
            &plan,
            &capabilities.capability_digest,
            &requirements,
            &adapter_id,
            &target_identity_digest,
        )
        .unwrap();
        let mut patch = Patch::draft(base, digest);
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();
        Ymm4NativeVoiceMutationPatch {
            patch,
            operation_id,
            target,
            mutations,
            plan,
            capability_digest: capabilities.capability_digest,
            capability_requirements: requirements,
            adapter_id,
            target_identity_digest,
            artifacts: Vec::new(),
            receipt: None,
            receipt_trusted: false,
        }
    }

    fn verified_receipt(patch: &Ymm4NativeVoiceMutationPatch) -> Ymm4OperationReceipt {
        let request = patch.apply_request();
        Ymm4OperationReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest,
            project_id: request.project_id,
            scene_id: request.scene_id,
            expected_fingerprint: request.expected_fingerprint.clone(),
            status: Ymm4OperationStatus::Verified,
            before_fingerprint: request.expected_fingerprint,
            after_fingerprint: "external-b".into(),
            applied_items: vec![item()],
            verified: true,
            error: None,
        }
    }

    #[test]
    fn semantic_readback_handles_update_and_delete() {
        verify_mutation_items(
            &[mutation(Ymm4NativeVoiceMutationAction::Update)],
            &[item()],
        )
        .unwrap();
        verify_mutation_items(&[mutation(Ymm4NativeVoiceMutationAction::Delete)], &[]).unwrap();
        assert!(
            verify_mutation_items(
                &[mutation(Ymm4NativeVoiceMutationAction::Delete)],
                &[item()]
            )
            .is_err()
        );
    }

    #[test]
    fn action_specific_requirements_bind_all_mutation_features() {
        let capabilities = capabilities();
        let requirements = mutation_capability_requirements(
            &capabilities,
            &[
                mutation(Ymm4NativeVoiceMutationAction::Create),
                Ymm4NativeVoiceMutation {
                    realization_id: Uuid::new_v4(),
                    entity_id: "utt-02".into(),
                    action: Ymm4NativeVoiceMutationAction::Update,
                    ..mutation(Ymm4NativeVoiceMutationAction::Update)
                },
                Ymm4NativeVoiceMutation {
                    realization_id: Uuid::new_v4(),
                    entity_id: "utt-03".into(),
                    action: Ymm4NativeVoiceMutationAction::Delete,
                    ..mutation(Ymm4NativeVoiceMutationAction::Delete)
                },
            ],
        )
        .unwrap();
        let names = requirements
            .iter()
            .map(|value| value.feature.as_str())
            .collect::<HashSet<_>>();
        assert!(names.contains("voiceItem.create"));
        assert!(names.contains("voiceItem.update"));
        assert!(names.contains("voiceItem.delete"));
        assert!(names.contains("timeline.transaction"));
        assert!(names.contains("readback.semantic"));
    }

    #[test]
    fn reconciliation_stage_preserves_explicit_operation_id() {
        let template = staged_patch();
        let capabilities = capabilities();
        let operation_id = Uuid::new_v4();
        let requirements =
            mutation_capability_requirements(&capabilities, &template.mutations).unwrap();
        let staged = Ymm4NativeVoiceMutationPatch::from_validated_plan(
            template.patch.base,
            template.target,
            template.mutations,
            template.plan,
            capabilities,
            requirements,
            operation_id,
        )
        .unwrap();
        assert_eq!(staged.operation_id, operation_id);
        assert_eq!(staged.apply_request().operation_id, operation_id);
        staged.validate_payload_digest().unwrap();
    }

    #[test]
    fn editable_payload_tamper_invalidates_approval_digest() {
        let mut patch = staged_patch();
        patch.mutations[0].frame += 1;
        assert!(matches!(
            patch.validate_payload_digest(),
            Err(Ymm4NativeVoiceMutationError::PayloadDigestMismatch { .. })
        ));
    }

    #[test]
    fn replay_marker_and_receipt_binding_are_both_required() {
        let patch = staged_patch();
        let request = patch.apply_request();
        let receipt = verified_receipt(&patch);
        let initial = Ymm4ApplyResponse {
            success: true,
            replayed: false,
            receipt: receipt.clone(),
        };
        patch
            .validate_apply_response(&initial, &request, false)
            .unwrap();
        assert!(matches!(
            patch.validate_apply_response(&initial, &request, true),
            Err(Ymm4NativeVoiceMutationError::ReplayMismatch(_))
        ));
        let mut wrong = receipt;
        wrong.request_digest = "changed".into();
        assert!(matches!(
            patch.validate_verified_receipt(&wrong, &request),
            Err(Ymm4NativeVoiceMutationError::ReceiptBindingMismatch)
        ));
    }

    #[test]
    fn deserialized_receipt_is_untrusted_and_durable_finalize_is_cas_idempotent() {
        let mut patch = staged_patch();
        let base = patch.patch.base;
        let digest = patch.patch.digest.clone();
        patch.approve(&digest, base).unwrap();
        patch.receipt = Some(verified_receipt(&patch));
        let serialized = serde_json::to_vec(&patch).unwrap();
        let loaded = Ymm4NativeVoiceMutationPatch::from_json_slice(&serialized).unwrap();
        assert!(!loaded.receipt_trusted);

        patch.receipt_trusted = true;
        let root =
            std::env::temp_dir().join(format!("takegraph-mutation-store-{}", Uuid::new_v4()));
        let store = crate::DurableProjectStore::open_or_bootstrap(
            &root,
            patch.target.project_id.clone(),
            base,
        )
        .unwrap();
        assert_eq!(patch.finalize_durable(&store).unwrap(), RevisionId(8));
        assert_eq!(patch.finalize_durable(&store).unwrap(), RevisionId(8));
        assert_eq!(store.head().unwrap(), RevisionId(8));
        let _ = std::fs::remove_dir_all(root);
    }
}
