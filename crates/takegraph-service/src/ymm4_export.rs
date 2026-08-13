use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use takegraph_core::{Patch, PatchError, PatchStatus, RevisionId, TargetPlan, canonical_sha256};
use takegraph_node::{
    CapabilityRequirement, ManagedItemKind, ManagedUtterance, Ymm4BridgeClient, Ymm4Error,
    Ymm4ManagedItem, Ymm4OperationReceipt, Ymm4PlanResponse, Ymm4ProjectSnapshot,
    Ymm4TargetPlanApplyRequest, Ymm4TargetPlanRequest, Ymm4TargetPlanValidation,
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4ExportPatch {
    pub patch: Patch,
    pub operation_id: Uuid,
    pub target: Ymm4ProjectSnapshot,
    pub utterances: Vec<ManagedUtterance>,
    pub plan: Ymm4PlanResponse,
    pub target_plan: TargetPlan,
    receipt: Option<Ymm4OperationReceipt>,
    #[serde(skip)]
    receipt_trusted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UncheckedYmm4ExportPatch {
    patch: Patch,
    operation_id: Uuid,
    target: Ymm4ProjectSnapshot,
    utterances: Vec<ManagedUtterance>,
    plan: Ymm4PlanResponse,
    target_plan: TargetPlan,
    receipt: Option<Ymm4OperationReceipt>,
}

impl UncheckedYmm4ExportPatch {
    fn into_export(self) -> Ymm4ExportPatch {
        Ymm4ExportPatch {
            patch: self.patch,
            operation_id: self.operation_id,
            target: self.target,
            utterances: self.utterances,
            plan: self.plan,
            target_plan: self.target_plan,
            receipt: self.receipt,
            receipt_trusted: false,
        }
    }
}

impl<'de> Deserialize<'de> for Ymm4ExportPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let export = UncheckedYmm4ExportPatch::deserialize(deserializer)?.into_export();
        export
            .validate_payload_digest()
            .map_err(<D::Error as serde::de::Error>::custom)?;
        Ok(export)
    }
}

impl Ymm4ExportPatch {
    /// Reads a persisted export patch and verifies that its stored digest still
    /// matches the canonical payload before returning any mutable state.
    ///
    /// # Errors
    ///
    /// Returns a JSON error for malformed input or a payload-digest mismatch
    /// for a well-formed patch whose digest-bound fields were modified.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, Ymm4ExportError> {
        let export = serde_json::from_slice::<UncheckedYmm4ExportPatch>(bytes)?.into_export();
        export.validate_payload_digest()?;
        Ok(export)
    }

    /// Recomputes the canonical payload digest and rejects stale or tampered
    /// persisted state.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical serialization fails or the stored digest
    /// differs from the recomputed digest.
    pub fn validate_payload_digest(&self) -> Result<(), Ymm4ExportError> {
        let canonical_digest = export_digest(
            self.patch.base,
            self.operation_id,
            &self.target,
            &self.utterances,
            &self.plan,
            &self.target_plan,
        )?;
        if canonical_digest != self.patch.digest {
            return Err(Ymm4ExportError::PayloadDigestMismatch {
                stored: self.patch.digest.clone(),
                canonical: canonical_digest,
            });
        }
        Ok(())
    }

    /// Captures YMM4 state and creates a previewable managed-subset patch.
    ///
    /// # Errors
    ///
    /// Returns a bridge, serialization, or core lifecycle error.
    pub async fn stage(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        utterances: Vec<ManagedUtterance>,
    ) -> Result<Self, Ymm4ExportError> {
        let capabilities = client.structured_capabilities().await?;
        let target = client.snapshot().await?;
        Self::stage_with_capabilities(
            client,
            head,
            target,
            utterances,
            capabilities,
            Uuid::new_v4(),
        )
        .await
    }

    /// Stages against a caller-captured snapshot after the canonical service
    /// head has been loaded for that exact external project identity.
    ///
    /// # Errors
    ///
    /// Returns a bridge, serialization, target-plan, or core lifecycle error.
    pub async fn stage_from_snapshot(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        utterances: Vec<ManagedUtterance>,
    ) -> Result<Self, Ymm4ExportError> {
        Self::stage_from_snapshot_with_operation_id(
            client,
            head,
            target,
            utterances,
            Uuid::new_v4(),
        )
        .await
    }

    /// Stages a reconciliation child using its already-durable operation ID.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty operation ID or the ordinary staging
    /// validation, bridge, and canonicalization failures.
    pub async fn stage_from_snapshot_with_operation_id(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        utterances: Vec<ManagedUtterance>,
        operation_id: Uuid,
    ) -> Result<Self, Ymm4ExportError> {
        if operation_id.is_nil() {
            return Err(Ymm4ExportError::InvalidTargetPlan(
                "operation ID must not be nil".into(),
            ));
        }
        crate::require_existing_project_path(&target.project_path)?;
        let capabilities = client.structured_capabilities().await?;
        Self::stage_with_capabilities(client, head, target, utterances, capabilities, operation_id)
            .await
    }

    async fn stage_with_capabilities(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        utterances: Vec<ManagedUtterance>,
        capabilities: takegraph_node::StructuredYmm4Capabilities,
        operation_id: Uuid,
    ) -> Result<Self, Ymm4ExportError> {
        let target_plan = crate::portable_pair_target_plan(
            head,
            operation_id,
            &target,
            &utterances,
            &capabilities,
        )?;
        let validation_request = Ymm4TargetPlanRequest::new(target_plan.clone())
            .map_err(|error| Ymm4ExportError::InvalidTargetPlan(error.to_string()))?;
        let validation = client.validate_target_plan(&validation_request).await?;
        Self::from_validated_target_plan(
            head,
            target,
            utterances,
            operation_id,
            target_plan,
            &validation_request,
            &validation,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_validated_target_plan(
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        utterances: Vec<ManagedUtterance>,
        operation_id: Uuid,
        target_plan: TargetPlan,
        validation_request: &Ymm4TargetPlanRequest,
        validation: &Ymm4TargetPlanValidation,
    ) -> Result<Self, Ymm4ExportError> {
        let expected_create_count = target_plan
            .cues
            .iter()
            .filter(|cue| cue.action == takegraph_core::PlannedAction::Create)
            .count();
        let expected_update_count = target_plan.cues.len() - expected_create_count;
        if validation.operation_id != operation_id
            || validation.target_plan_digest != validation_request.target_plan_digest
            || validation.fingerprint != target.fingerprint
            || validation
                .strategy_counts
                .get("portable_pair")
                .copied()
                .unwrap_or_default()
                != utterances.len()
            || validation.physical_item_count != utterances.len() * 2
            || validation.create_count != expected_create_count
            || validation.update_count != expected_update_count
            || validation.delete_count != 0
        {
            return Err(Ymm4ExportError::InvalidTargetPlan(
                "bridge target-plan validation response is not bound to this portable plan".into(),
            ));
        }
        let plan = portable_plan_impact(&target, &utterances, validation);
        let digest = export_digest(
            head,
            operation_id,
            &target,
            &utterances,
            &plan,
            &target_plan,
        )?;
        let mut patch = Patch::draft(head, digest);
        patch.validate()?;
        patch.materialize_preview()?;
        Ok(Self {
            patch,
            operation_id,
            target,
            utterances,
            plan,
            target_plan,
            receipt: None,
            receipt_trusted: false,
        })
    }

    /// Returns the most recently stored bridge receipt. A deserialized receipt
    /// is informational until `apply` replays it through the authenticated bridge.
    #[must_use]
    pub fn receipt(&self) -> Option<&Ymm4OperationReceipt> {
        self.receipt.as_ref()
    }

    /// Records approval for the exact current digest and performs local preflight.
    ///
    /// Callers should durably save the patch after this returns and before external I/O.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched digest, invalid lifecycle, or stale local revision.
    pub fn approve(
        &mut self,
        approved_digest: &str,
        current_head: RevisionId,
    ) -> Result<(), Ymm4ExportError> {
        self.validate_payload_digest()?;
        if approved_digest != self.patch.digest {
            return Err(Ymm4ExportError::DigestMismatch);
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

    /// Applies the approved operation exactly once and stores its verified receipt.
    ///
    /// Replaying this call uses the same operation ID; the bridge returns its prior receipt.
    ///
    /// # Errors
    ///
    /// Returns an error for failed authorization, stale YMM4 state, apply failure, or
    /// read-back mismatch.
    #[cfg(test)]
    async fn apply(
        &mut self,
        client: &Ymm4BridgeClient,
        current_head: RevisionId,
    ) -> Result<&Ymm4OperationReceipt, Ymm4ExportError> {
        self.validate_payload_digest()?;
        self.patch.authorize_commit(current_head)?;
        self.validate_current_capabilities(client).await?;
        let request = Ymm4TargetPlanApplyRequest::new(
            self.target_plan.clone(),
            self.target.fingerprint.clone(),
        )
        .map_err(|error| Ymm4ExportError::InvalidTargetPlan(error.to_string()))?;
        self.apply_reserved_request(client, &request, false).await
    }

    async fn apply_reserved_request(
        &mut self,
        client: &Ymm4BridgeClient,
        request: &Ymm4TargetPlanApplyRequest,
        require_replay: bool,
    ) -> Result<&Ymm4OperationReceipt, Ymm4ExportError> {
        self.receipt_trusted = false;
        let expected_request_digest = request.request_digest.clone();
        let result = client.apply_target_plan(request).await?;
        self.receipt = Some(result.receipt);
        if require_replay && !result.replayed {
            return Err(Ymm4ExportError::ApplyFailed(
                "bridge did not mark the reserved operation as an exact replay".into(),
            ));
        }
        if !result.success {
            return Err(Ymm4ExportError::ApplyFailed(
                self.receipt
                    .as_ref()
                    .and_then(|receipt| receipt.error.clone())
                    .unwrap_or_else(|| {
                        "YMM4 bridge did not return a verified read-back receipt".into()
                    }),
            ));
        }
        self.validate_verified_receipt(Some(expected_request_digest.as_str()))?;
        self.receipt_trusted = true;
        self.receipt
            .as_ref()
            .ok_or(Ymm4ExportError::MissingVerifiedReceipt)
    }

    /// Applies the approved portable-pair export and publishes its verified
    /// receipt under one project-wide external-mutation fence.
    ///
    /// The durable head is re-read after acquiring the fence. No other
    /// canonical-to-YMM mutation may enter until verified durable finalization
    /// succeeds or this call returns after the bridge has completed its
    /// rollback/recovery response.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale head, an active detach reservation, bridge
    /// apply/read-back failure, or durable publication failure.
    #[allow(clippy::too_many_lines)]
    pub async fn apply_and_finalize_durable(
        &mut self,
        client: &Ymm4BridgeClient,
        store: &crate::DurableProjectStore,
        current_head: RevisionId,
    ) -> Result<crate::DurableExternalMutationOutcome, Ymm4ExportError> {
        self.validate_payload_digest()?;
        crate::require_existing_project_path(&self.target.project_path)?;
        crate::require_existing_project_path(&client.snapshot().await?.project_path)?;
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        let request = Ymm4TargetPlanApplyRequest::new(
            self.target_plan.clone(),
            self.target.fingerprint.clone(),
        )
        .map_err(|error| Ymm4ExportError::InvalidTargetPlan(error.to_string()))?;
        let target = crate::VerifiedTargetBinding {
            adapter_id: self.target_plan.target.adapter_id.clone(),
            target_project_id: self.target.project_id.clone(),
            scene_id: self.target.scene_id.clone(),
            target_identity_digest: self
                .target_plan
                .expected_scope
                .target_identity_digest
                .clone(),
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
                            &request.target_plan.target.project_id,
                            &request.target_plan.target.scene_id,
                            &self.target.fingerprint,
                        );
                    let terminal = matches!(
                        receipt.status,
                        takegraph_node::Ymm4OperationStatus::NotStarted
                            | takegraph_node::Ymm4OperationStatus::Failed
                            | takegraph_node::Ymm4OperationStatus::RolledBack
                            | takegraph_node::Ymm4OperationStatus::RecoveryRequired
                    );
                    if !request_bound || terminal {
                        let safe_abort =
                            crate::external_mutation::operation_receipt_proves_exact_rollback(
                                &receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.target_plan.target.project_id,
                                &request.target_plan.target.scene_id,
                                &self.target.fingerprint,
                            ) || crate::external_mutation::operation_receipt_proves_no_mutation(
                                &receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.target_plan.target.project_id,
                                &request.target_plan.target.scene_id,
                                &self.target.fingerprint,
                            ) || crate::external_mutation::operation_receipt_proves_not_started(
                                &receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.target_plan.target.project_id,
                                &request.target_plan.target.scene_id,
                                &self.target.fingerprint,
                            );
                        let message = receipt.error.clone().unwrap_or_else(|| {
                            "reserved YMM4 operation has untrusted terminal evidence".into()
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
                        return Err(Ymm4ExportError::ApplyFailed(message));
                    }
                    replay_existing_bridge_operation = true;
                }
                Err(Ymm4Error::Bridge { status, .. }) if status.as_u16() == 404 => {
                    // Serialize with any delayed apply. Only the durable exact
                    // tombstone can prove that no mutation may start later.
                    let sealed = client.seal_target_plan_not_started(&request).await?;
                    let not_started = !sealed.success
                        && crate::external_mutation::operation_receipt_proves_not_started(
                            &sealed.receipt,
                            self.operation_id,
                            &request.request_digest,
                            &request.target_plan.target.project_id,
                            &request.target_plan.target.scene_id,
                            &self.target.fingerprint,
                        );
                    let message = sealed.receipt.error.clone().unwrap_or_else(|| {
                        "reserved YMM4 operation has no trusted recovery evidence".into()
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
                        return Err(Ymm4ExportError::ApplyFailed(message));
                    }
                    if !self.receipt.as_ref().is_some_and(|receipt| {
                        crate::external_mutation::operation_receipt_is_request_bound(
                            receipt,
                            self.operation_id,
                            &request.request_digest,
                            &request.target_plan.target.project_id,
                            &request.target_plan.target.scene_id,
                            &self.target.fingerprint,
                        )
                    }) {
                        return Err(Ymm4ExportError::ApplyFailed(message));
                    }
                    replay_existing_bridge_operation = true;
                }
                Err(error) => return Err(error.into()),
            }
        }

        if let Err(error) = self
            .apply_reserved_request(client, &request, replay_existing_bridge_operation)
            .await
        {
            let safe_abort = self.receipt.as_ref().is_some_and(|receipt| {
                crate::external_mutation::operation_receipt_proves_exact_rollback(
                    receipt,
                    self.operation_id,
                    &request.request_digest,
                    &request.target_plan.target.project_id,
                    &request.target_plan.target.scene_id,
                    &self.target.fingerprint,
                ) || crate::external_mutation::operation_receipt_proves_no_mutation(
                    receipt,
                    self.operation_id,
                    &request.request_digest,
                    &request.target_plan.target.project_id,
                    &request.target_plan.target.scene_id,
                    &self.target.fingerprint,
                ) || crate::external_mutation::operation_receipt_proves_not_started(
                    receipt,
                    self.operation_id,
                    &request.request_digest,
                    &request.target_plan.target.project_id,
                    &request.target_plan.target.scene_id,
                    &self.target.fingerprint,
                )
            });
            let sealed_not_started = if safe_abort {
                false
            } else {
                match client.seal_target_plan_not_started(&request).await {
                    Ok(sealed) => {
                        let proof = !sealed.success
                            && crate::external_mutation::operation_receipt_proves_not_started(
                                &sealed.receipt,
                                self.operation_id,
                                &request.request_digest,
                                &request.target_plan.target.project_id,
                                &request.target_plan.target.scene_id,
                                &self.target.fingerprint,
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
        request: &Ymm4TargetPlanApplyRequest,
        committed: &crate::ExternalCommitRecord,
    ) -> Result<(), Ymm4ExportError> {
        self.receipt_trusted = false;
        let result = client.apply_target_plan(request).await?;
        self.receipt = Some(result.receipt);
        if !result.success || !result.replayed {
            return Err(Ymm4ExportError::ApplyFailed(
                "durably committed YMM4 operation did not return an exact replay".into(),
            ));
        }
        let receipt = self.validate_verified_receipt(Some(&request.request_digest))?;
        if canonical_sha256("takegraph-external-receipt", receipt)? != committed.receipt_digest {
            return Err(Ymm4ExportError::DurableReceiptMismatch);
        }
        self.receipt_trusted = true;
        Ok(())
    }

    /// Advances the `TakeGraph` revision only after verified YMM4 read-back.
    ///
    /// # Errors
    ///
    /// Returns an error if no verified receipt exists or the core commit guard fails.
    pub fn finalize(&mut self, current_head: RevisionId) -> Result<RevisionId, Ymm4ExportError> {
        self.validate_payload_digest()?;
        if self.receipt.is_none() {
            return Err(Ymm4ExportError::MissingVerifiedReceipt);
        }
        if !self.receipt_trusted {
            return Err(Ymm4ExportError::UntrustedReceipt);
        }
        self.validate_verified_receipt(None)?;
        Ok(self.patch.commit(current_head)?)
    }

    /// Atomically records the verified external receipt, target link, and new
    /// canonical revision in the durable project store.
    ///
    /// Exact operation replay returns the previously committed revision. A
    /// crash after the store commit but before this patch file is saved is
    /// therefore safe to retry after re-authenticating the bridge receipt.
    ///
    /// # Errors
    ///
    /// Returns an error for an untrusted receipt, invalid patch lifecycle,
    /// stale durable head, conflicting operation replay, or corrupt storage.
    pub(crate) fn finalize_durable(
        &mut self,
        store: &crate::DurableProjectStore,
    ) -> Result<RevisionId, Ymm4ExportError> {
        self.validate_payload_digest()?;
        if !self.receipt_trusted {
            return Err(Ymm4ExportError::UntrustedReceipt);
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
        let proof = {
            let receipt = self.validate_verified_receipt(None)?;
            let managed_state_update =
                crate::project_store::VerifiedManagedStateUpdate::replacing_entities_from_receipt(
                    self.utterances
                        .iter()
                        .map(|utterance| utterance.entity_id.clone()),
                    receipt,
                )?;
            crate::project_store::VerifiedExternalCommit::from_receipt(
                self.operation_id,
                self.patch.base,
                self.patch.digest.clone(),
                receipt.request_digest.clone(),
                receipt,
                crate::VerifiedTargetBinding {
                    adapter_id: self.target_plan.target.adapter_id.clone(),
                    target_project_id: self.target.project_id.clone(),
                    scene_id: self.target.scene_id.clone(),
                    target_identity_digest: self
                        .target_plan
                        .expected_scope
                        .target_identity_digest
                        .clone(),
                    verified_fingerprint: receipt.after_fingerprint.clone(),
                },
            )?
            .with_managed_state_update(managed_state_update)?
        };
        let durable_revision = store.commit_verified_external(&proof)?;
        let expected_revision = self
            .patch
            .base
            .checked_next()
            .ok_or(PatchError::RevisionOverflow)?;
        if durable_revision != expected_revision {
            return Err(Ymm4ExportError::InvalidTargetPlan(format!(
                "durable operation revision {durable_revision:?} does not match patch revision {expected_revision:?}"
            )));
        }
        if self.patch.status == PatchStatus::Approved {
            let core_revision = self.patch.commit(self.patch.base)?;
            debug_assert_eq!(core_revision, durable_revision);
        }
        Ok(durable_revision)
    }

    /// Verifies that the current YMM4 managed items match this patch's desired state.
    ///
    /// # Errors
    ///
    /// Returns a bridge error or a read-back mismatch.
    pub async fn verify(&self, client: &Ymm4BridgeClient) -> Result<(), Ymm4ExportError> {
        self.validate_payload_digest()?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != self.target.project_id
            || snapshot.scene_id != self.target.scene_id
        {
            return Err(Ymm4ExportError::VerifyMismatch(
                "active YMM4 project or scene does not match the staged export target".into(),
            ));
        }
        self.verify_items(&snapshot.managed_items)
    }

    fn verify_items(&self, items: &[Ymm4ManagedItem]) -> Result<(), Ymm4ExportError> {
        crate::normalize_realizations(&self.target_plan, items)
            .map(|_| ())
            .map_err(|error| Ymm4ExportError::VerifyMismatch(error.to_string()))
    }

    fn validate_verified_receipt(
        &self,
        expected_request_digest: Option<&str>,
    ) -> Result<&Ymm4OperationReceipt, Ymm4ExportError> {
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(Ymm4ExportError::MissingVerifiedReceipt)?;
        let computed_request = Ymm4TargetPlanApplyRequest::new(
            self.target_plan.clone(),
            self.target.fingerprint.clone(),
        )
        .map_err(|error| Ymm4ExportError::InvalidTargetPlan(error.to_string()))?;
        let expected_request_digest =
            expected_request_digest.unwrap_or(computed_request.request_digest.as_str());
        if !receipt.verified
            || receipt.status != takegraph_node::Ymm4OperationStatus::Verified
            || receipt.operation_id != self.operation_id
            || receipt.request_digest != expected_request_digest
            || receipt.project_id != self.target.project_id
            || receipt.scene_id != self.target.scene_id
            || receipt.expected_fingerprint != self.target.fingerprint
        {
            return Err(Ymm4ExportError::ApplyFailed(
                "YMM4 bridge receipt is not verified and bound to this export request".into(),
            ));
        }
        self.verify_items(&receipt.applied_items)?;
        Ok(receipt)
    }

    async fn validate_current_capabilities(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4ExportError> {
        let actual = client.structured_capabilities().await?;
        if actual.capability_digest != self.target_plan.capability_digest {
            return Err(Ymm4ExportError::CapabilityDrift {
                expected: self.target_plan.capability_digest.clone(),
                actual: actual.capability_digest,
            });
        }
        let requirements = self
            .target_plan
            .cues
            .iter()
            .flat_map(|cue| cue.capability_dependencies.iter())
            .map(|dependency| CapabilityRequirement {
                feature: dependency.feature.clone(),
                minimum_version: dependency.minimum_version,
                schema_digest: dependency.schema_digest.clone(),
            })
            .collect::<Vec<_>>();
        actual
            .require(&requirements)
            .map_err(|error| Ymm4ExportError::InvalidTargetPlan(error.to_string()))
    }
}

fn portable_plan_impact(
    target: &Ymm4ProjectSnapshot,
    utterances: &[ManagedUtterance],
    validation: &Ymm4TargetPlanValidation,
) -> Ymm4PlanResponse {
    let requested = utterances
        .iter()
        .map(|utterance| utterance.entity_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let desired = utterances.iter().flat_map(|utterance| {
        [
            Ymm4ManagedItem {
                entity_id: utterance.entity_id.clone(),
                revision: utterance.revision,
                kind: ManagedItemKind::Audio,
                frame: utterance.frame,
                layer: utterance.audio_layer,
                length: utterance.length,
                text: None,
                audio_path: Some(utterance.audio_path.clone()),
                artifact_hash: Some(utterance.artifact_hash.clone()),
                speaker: None,
                realization_id: None,
            },
            Ymm4ManagedItem {
                entity_id: utterance.entity_id.clone(),
                revision: utterance.revision,
                kind: ManagedItemKind::Caption,
                frame: utterance.frame,
                layer: utterance.caption_layer,
                length: utterance.length,
                text: Some(utterance.caption.clone()),
                audio_path: None,
                artifact_hash: Some(utterance.artifact_hash.clone()),
                speaker: None,
                realization_id: None,
            },
        ]
    });
    let mut managed_items_after = target
        .managed_items
        .iter()
        .filter(|item| !requested.contains(item.entity_id.as_str()))
        .cloned()
        .chain(desired)
        .collect::<Vec<_>>();
    managed_items_after.sort_by(|left, right| {
        (left.frame, left.layer, left.entity_id.as_str()).cmp(&(
            right.frame,
            right.layer,
            right.entity_id.as_str(),
        ))
    });
    Ymm4PlanResponse {
        fingerprint: validation.fingerprint.clone(),
        operation_count: validation.create_count + validation.update_count,
        create_count: validation.create_count,
        replace_count: validation.update_count,
        unchanged_count: 0,
        managed_items_after,
    }
}

fn export_digest(
    head: RevisionId,
    operation_id: Uuid,
    target: &Ymm4ProjectSnapshot,
    utterances: &[ManagedUtterance],
    plan: &Ymm4PlanResponse,
    target_plan: &TargetPlan,
) -> Result<String, Ymm4ExportError> {
    target_plan
        .validate()
        .map_err(|error| Ymm4ExportError::InvalidTargetPlan(error.to_string()))?;
    if target_plan.base_revision != head
        || target_plan.operation_id != operation_id
        || target_plan.target.project_id != target.project_id
        || target_plan.target.scene_id != target.scene_id
    {
        return Err(Ymm4ExportError::InvalidTargetPlan(
            "target plan is not bound to the export base, operation, project, and scene".into(),
        ));
    }
    let target_plan_digest = target_plan
        .canonical_digest()
        .map_err(|error| Ymm4ExportError::InvalidTargetPlan(error.to_string()))?;
    let canonical = serde_json::to_vec(&serde_json::json!({
        "baseRevision": head,
        "operationId": operation_id,
        "projectId": target.project_id,
        "sceneId": target.scene_id,
        "expectedFingerprint": target.fingerprint,
        "utterances": utterances,
        "plan": plan,
        "targetPlanDigest": target_plan_digest,
        "targetPlan": target_plan,
    }))?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

#[derive(Debug, Error)]
pub enum Ymm4ExportError {
    #[error(transparent)]
    Bridge(#[from] Ymm4Error),
    #[error(transparent)]
    Core(#[from] PatchError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    TargetPlanBuild(#[from] crate::TargetPlanBuildError),
    #[error(transparent)]
    ProjectStore(#[from] crate::ProjectStoreError),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
    #[error("invalid YMM4 target plan: {0}")]
    InvalidTargetPlan(String),
    #[error("YMM4 capabilities changed after preview (expected {expected}, got {actual})")]
    CapabilityDrift { expected: String, actual: String },
    #[error("approval does not match the current YMM4 export digest")]
    DigestMismatch,
    #[error(
        "stored YMM4 export digest does not match its canonical payload (stored {stored}, canonical {canonical})"
    )]
    PayloadDigestMismatch { stored: String, canonical: String },
    #[error("YMM4 managed apply failed: {0}")]
    ApplyFailed(String),
    #[error("YMM4 apply has no verified read-back receipt")]
    MissingVerifiedReceipt,
    #[error("YMM4 receipt must be replayed through the authenticated bridge before finalization")]
    UntrustedReceipt,
    #[error("authenticated YMM4 replay does not match the durable receipt digest")]
    DurableReceiptMismatch,
    #[error("YMM4 managed verification failed: {0}")]
    VerifyMismatch(String),
    #[error(transparent)]
    UnsavedProject(#[from] crate::UnsavedProjectError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_node::{
        StructuredYmm4Capabilities, Ymm4Capabilities, Ymm4Capability, Ymm4Health,
    };

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
                    Ymm4Capability::MutationProfileYmm4_4_55_1_1,
                ],
            },
        )
        .unwrap()
    }

    fn staged_patch() -> Ymm4ExportPatch {
        let base = RevisionId(4);
        let operation_id = Uuid::from_u128(1);
        let target = Ymm4ProjectSnapshot {
            project_id: "project".into(),
            project_name: "test".into(),
            project_path: "test.ymmp".into(),
            scene_id: "scene".into(),
            fps: 30,
            fingerprint: "external-a".into(),
            managed_items: vec![],
            native_extensions: vec![],
            unmanaged_context_count: 0,
        };
        let utterances = vec![ManagedUtterance {
            entity_id: "utt-01".into(),
            revision: 1,
            speaker: "speaker".into(),
            caption: "caption".into(),
            spoken_text: "spoken caption".into(),
            audio_path: "audio.wav".into(),
            artifact_hash: "a".repeat(64),
            frame: 10,
            length: 20,
            audio_layer: 1,
            caption_layer: 2,
        }];
        let plan = Ymm4PlanResponse {
            fingerprint: "external-a".into(),
            operation_count: 1,
            create_count: 1,
            replace_count: 0,
            unchanged_count: 0,
            managed_items_after: vec![],
        };
        let target_plan = crate::portable_pair_target_plan(
            base,
            operation_id,
            &target,
            &utterances,
            &capabilities(),
        )
        .unwrap();
        let digest = export_digest(
            base,
            operation_id,
            &target,
            &utterances,
            &plan,
            &target_plan,
        )
        .unwrap();
        let mut patch = Patch::draft(base, digest);
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();
        Ymm4ExportPatch {
            patch,
            operation_id,
            target,
            utterances,
            plan,
            target_plan,
            receipt: None,
            receipt_trusted: false,
        }
    }

    fn managed_pair(patch: &Ymm4ExportPatch) -> Vec<Ymm4ManagedItem> {
        let utterance = &patch.utterances[0];
        vec![
            Ymm4ManagedItem {
                entity_id: utterance.entity_id.clone(),
                revision: utterance.revision,
                kind: takegraph_node::ManagedItemKind::Audio,
                frame: utterance.frame,
                layer: utterance.audio_layer,
                length: utterance.length,
                text: None,
                audio_path: Some(utterance.audio_path.clone()),
                artifact_hash: Some(utterance.artifact_hash.clone()),
                speaker: None,
                realization_id: None,
            },
            Ymm4ManagedItem {
                entity_id: utterance.entity_id.clone(),
                revision: utterance.revision,
                kind: takegraph_node::ManagedItemKind::Caption,
                frame: utterance.frame,
                layer: utterance.caption_layer,
                length: utterance.length,
                text: Some(utterance.caption.clone()),
                audio_path: None,
                artifact_hash: Some(utterance.artifact_hash.clone()),
                speaker: None,
                realization_id: None,
            },
        ]
    }

    fn verified_receipt(patch: &Ymm4ExportPatch) -> Ymm4OperationReceipt {
        let request = Ymm4TargetPlanApplyRequest::new(
            patch.target_plan.clone(),
            patch.target.fingerprint.clone(),
        )
        .unwrap();
        Ymm4OperationReceipt {
            operation_id: patch.operation_id,
            request_digest: request.request_digest,
            project_id: patch.target.project_id.clone(),
            scene_id: patch.target.scene_id.clone(),
            expected_fingerprint: patch.target.fingerprint.clone(),
            status: takegraph_node::Ymm4OperationStatus::Verified,
            before_fingerprint: patch.target.fingerprint.clone(),
            after_fingerprint: "external-b".into(),
            applied_items: managed_pair(patch),
            verified: true,
            error: None,
        }
    }

    #[test]
    fn approval_is_digest_and_revision_bound() {
        let mut patch = staged_patch();
        assert!(matches!(
            patch.approve("wrong", RevisionId(4)),
            Err(Ymm4ExportError::DigestMismatch)
        ));
        let digest = patch.patch.digest.clone();
        patch.approve(&digest, RevisionId(4)).unwrap();
        assert_eq!(patch.patch.status, PatchStatus::Approved);
        assert!(matches!(
            patch.patch.authorize_commit(RevisionId(5)),
            Err(PatchError::StaleBase { .. })
        ));
    }

    #[test]
    fn reconciliation_stage_preserves_explicit_operation_id() {
        let template = staged_patch();
        let operation_id = Uuid::new_v4();
        let target_plan = crate::portable_pair_target_plan(
            template.patch.base,
            operation_id,
            &template.target,
            &template.utterances,
            &capabilities(),
        )
        .unwrap();
        let request = Ymm4TargetPlanRequest::new(target_plan.clone()).unwrap();
        let validation = Ymm4TargetPlanValidation {
            operation_id,
            target_plan_digest: request.target_plan_digest.clone(),
            fingerprint: template.target.fingerprint.clone(),
            strategy_counts: std::collections::BTreeMap::from([(
                "portable_pair".into(),
                template.utterances.len(),
            )]),
            create_count: template.utterances.len(),
            update_count: 0,
            delete_count: 0,
            physical_item_count: template.utterances.len() * 2,
        };
        let staged = Ymm4ExportPatch::from_validated_target_plan(
            template.patch.base,
            template.target,
            template.utterances,
            operation_id,
            target_plan,
            &request,
            &validation,
        )
        .unwrap();
        assert_eq!(staged.operation_id, operation_id);
        assert_eq!(staged.target_plan.operation_id, operation_id);
        assert_eq!(
            Ymm4TargetPlanApplyRequest::new(
                staged.target_plan.clone(),
                staged.target.fingerprint.clone(),
            )
            .unwrap()
            .target_plan
            .operation_id,
            operation_id
        );
        staged.validate_payload_digest().unwrap();
    }

    #[test]
    fn finalize_requires_verified_receipt() {
        let mut patch = staged_patch();
        let digest = patch.patch.digest.clone();
        patch.approve(&digest, RevisionId(4)).unwrap();
        assert!(matches!(
            patch.finalize(RevisionId(4)),
            Err(Ymm4ExportError::MissingVerifiedReceipt)
        ));
    }

    #[test]
    fn finalize_rejects_unbound_or_forged_receipt() {
        let mut patch = staged_patch();
        let digest = patch.patch.digest.clone();
        patch.approve(&digest, RevisionId(4)).unwrap();
        let mut forged = verified_receipt(&patch);
        forged.request_digest = "forged".into();
        patch.receipt = Some(forged);
        patch.receipt_trusted = true;
        assert!(matches!(
            patch.finalize(RevisionId(4)),
            Err(Ymm4ExportError::ApplyFailed(_))
        ));

        patch.receipt = Some(verified_receipt(&patch));
        patch.receipt_trusted = false;
        assert!(matches!(
            patch.finalize(RevisionId(4)),
            Err(Ymm4ExportError::UntrustedReceipt)
        ));
        patch.receipt_trusted = true;
        assert_eq!(patch.finalize(RevisionId(4)).unwrap(), RevisionId(5));
    }

    #[test]
    fn durable_finalize_atomically_records_revision_target_and_replay() {
        let root = std::env::temp_dir().join(format!("takegraph-export-store-{}", Uuid::new_v4()));
        let store = crate::DurableProjectStore::open_or_bootstrap(
            &root,
            "canonical-project",
            RevisionId(4),
        )
        .unwrap();
        let mut export = staged_patch();
        let digest = export.patch.digest.clone();
        export.approve(&digest, RevisionId(4)).unwrap();
        export.receipt = Some(verified_receipt(&export));
        export.receipt_trusted = true;

        assert_eq!(export.finalize_durable(&store).unwrap(), RevisionId(5));
        assert_eq!(export.finalize_durable(&store).unwrap(), RevisionId(5));
        let state = store.snapshot().unwrap();
        assert_eq!(state.head, RevisionId(5));
        assert_eq!(state.external_commits.len(), 1);
        assert_eq!(state.target_links.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_patch_read_rejects_payload_tampering() {
        let patch = staged_patch();
        let mut value = serde_json::to_value(&patch).unwrap();
        value["utterances"][0]["caption"] = serde_json::json!("tampered");
        let bytes = serde_json::to_vec(&value).unwrap();

        assert!(matches!(
            Ymm4ExportPatch::from_json_slice(&bytes),
            Err(Ymm4ExportError::PayloadDigestMismatch { .. })
        ));
        assert!(serde_json::from_slice::<Ymm4ExportPatch>(&bytes).is_err());
    }

    #[test]
    fn persisted_patch_read_accepts_canonical_payload() {
        let patch = staged_patch();
        let bytes = serde_json::to_vec(&patch).unwrap();

        let loaded = Ymm4ExportPatch::from_json_slice(&bytes).unwrap();
        assert_eq!(loaded.patch.digest, patch.patch.digest);
        assert_eq!(loaded.operation_id, patch.operation_id);
        assert_eq!(loaded.utterances, patch.utterances);
    }

    #[test]
    fn approval_rejects_in_memory_payload_tampering() {
        let mut patch = staged_patch();
        let stored_digest = patch.patch.digest.clone();
        patch.plan.create_count += 1;

        assert!(matches!(
            patch.approve(&stored_digest, RevisionId(4)),
            Err(Ymm4ExportError::PayloadDigestMismatch { .. })
        ));
        assert_eq!(patch.patch.status, PatchStatus::Previewable);
        assert!(patch.patch.approved_digest.is_none());
    }

    #[test]
    fn apply_rejects_in_memory_payload_tampering_before_bridge_io() {
        let mut patch = staged_patch();
        let stored_digest = patch.patch.digest.clone();
        patch.approve(&stored_digest, RevisionId(4)).unwrap();
        patch.target.fingerprint = "tampered".into();
        let client = Ymm4BridgeClient::new("http://127.0.0.1:9", "test-token").unwrap();

        {
            let mut future = std::pin::pin!(patch.apply(&client, RevisionId(4)));
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(matches!(
                std::future::Future::poll(future.as_mut(), &mut context),
                std::task::Poll::Ready(Err(Ymm4ExportError::PayloadDigestMismatch { .. }))
            ));
        }
        assert!(patch.receipt.is_none());
    }
}
