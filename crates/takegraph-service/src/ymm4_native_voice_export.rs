use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use takegraph_core::{Patch, PatchError, PatchStatus, RevisionId, TargetPlan, canonical_sha256};
use takegraph_node::{
    CapabilityRequirement, Ymm4BridgeClient, Ymm4Error, Ymm4ManagedItem, Ymm4NativeVoiceCue,
    Ymm4NativeVoicePlanResponse, Ymm4OperationReceipt, Ymm4ProjectSnapshot,
    Ymm4TargetPlanApplyRequest, Ymm4TargetPlanRequest,
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ymm4NativeVoiceExportPatch {
    pub patch: Patch,
    pub operation_id: Uuid,
    pub target: Ymm4ProjectSnapshot,
    pub cues: Vec<Ymm4NativeVoiceCue>,
    pub plan: Ymm4NativeVoicePlanResponse,
    pub target_plan: TargetPlan,
    receipt: Option<Ymm4OperationReceipt>,
    #[serde(skip)]
    receipt_trusted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UncheckedYmm4NativeVoiceExportPatch {
    patch: Patch,
    operation_id: Uuid,
    target: Ymm4ProjectSnapshot,
    cues: Vec<Ymm4NativeVoiceCue>,
    plan: Ymm4NativeVoicePlanResponse,
    target_plan: TargetPlan,
    receipt: Option<Ymm4OperationReceipt>,
}

impl UncheckedYmm4NativeVoiceExportPatch {
    fn into_export(self) -> Ymm4NativeVoiceExportPatch {
        Ymm4NativeVoiceExportPatch {
            patch: self.patch,
            operation_id: self.operation_id,
            target: self.target,
            cues: self.cues,
            plan: self.plan,
            target_plan: self.target_plan,
            receipt: self.receipt,
            receipt_trusted: false,
        }
    }
}

impl<'de> Deserialize<'de> for Ymm4NativeVoiceExportPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let export = UncheckedYmm4NativeVoiceExportPatch::deserialize(deserializer)?.into_export();
        export
            .validate_payload_digest()
            .map_err(<D::Error as serde::de::Error>::custom)?;
        Ok(export)
    }
}

impl Ymm4NativeVoiceExportPatch {
    /// Reads a persisted native-voice patch only when its digest still matches
    /// the canonical approval payload.
    ///
    /// # Errors
    ///
    /// Returns a JSON error for malformed input or a payload-digest mismatch
    /// for a well-formed patch whose digest-bound fields were modified.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, Ymm4NativeVoiceExportError> {
        let export =
            serde_json::from_slice::<UncheckedYmm4NativeVoiceExportPatch>(bytes)?.into_export();
        export.validate_payload_digest()?;
        Ok(export)
    }

    /// Recomputes the canonical approval payload digest.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical serialization fails or the stored digest
    /// differs from the recomputed value.
    pub fn validate_payload_digest(&self) -> Result<(), Ymm4NativeVoiceExportError> {
        let canonical = native_voice_export_digest(
            self.patch.base,
            self.operation_id,
            &self.target,
            &self.cues,
            &self.plan,
            &self.target_plan,
        )?;
        if canonical != self.patch.digest {
            return Err(Ymm4NativeVoiceExportError::PayloadDigestMismatch {
                stored: self.patch.digest.clone(),
                canonical,
            });
        }
        Ok(())
    }

    /// Captures YMM4 state and stages a previewable native `VoiceItem` batch.
    ///
    /// # Errors
    ///
    /// Returns a bridge, serialization, or core lifecycle error.
    pub async fn stage(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        cues: Vec<Ymm4NativeVoiceCue>,
    ) -> Result<Self, Ymm4NativeVoiceExportError> {
        let capabilities = client.structured_capabilities().await?;
        let target = client.snapshot().await?;
        Self::stage_with_capabilities(client, head, target, cues, capabilities).await
    }

    /// Stages native voice against a caller-captured snapshot after the
    /// canonical service head has been loaded for that target identity.
    ///
    /// # Errors
    ///
    /// Returns a bridge, serialization, target-plan, or core lifecycle error.
    pub async fn stage_from_snapshot(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        cues: Vec<Ymm4NativeVoiceCue>,
    ) -> Result<Self, Ymm4NativeVoiceExportError> {
        let capabilities = client.structured_capabilities().await?;
        Self::stage_with_capabilities(client, head, target, cues, capabilities).await
    }

    async fn stage_with_capabilities(
        client: &Ymm4BridgeClient,
        head: RevisionId,
        target: Ymm4ProjectSnapshot,
        cues: Vec<Ymm4NativeVoiceCue>,
        capabilities: takegraph_node::StructuredYmm4Capabilities,
    ) -> Result<Self, Ymm4NativeVoiceExportError> {
        let operation_id = Uuid::new_v4();
        let target_plan =
            crate::native_voice_target_plan(head, operation_id, &target, &cues, &capabilities)?;
        let validation_request = Ymm4TargetPlanRequest::new(target_plan.clone())
            .map_err(|error| Ymm4NativeVoiceExportError::InvalidTargetPlan(error.to_string()))?;
        let validation = client.validate_target_plan(&validation_request).await?;
        if validation.operation_id != operation_id
            || validation.target_plan_digest != validation_request.target_plan_digest
            || validation.fingerprint != target.fingerprint
            || validation
                .strategy_counts
                .get("native_voice")
                .copied()
                .unwrap_or_default()
                != cues.len()
            || validation.physical_item_count != cues.len()
            || validation.create_count != cues.len()
            || validation.update_count != 0
            || validation.delete_count != 0
        {
            return Err(Ymm4NativeVoiceExportError::InvalidTargetPlan(
                "bridge target-plan validation response is not bound to this native-voice plan"
                    .into(),
            ));
        }
        let plan = Ymm4NativeVoicePlanResponse {
            fingerprint: validation.fingerprint,
            create_count: validation.create_count,
            duration_resolution: "bounded".into(),
        };
        let digest =
            native_voice_export_digest(head, operation_id, &target, &cues, &plan, &target_plan)?;
        let mut patch = Patch::draft(head, digest);
        patch.validate()?;
        patch.materialize_preview()?;
        Ok(Self {
            patch,
            operation_id,
            target,
            cues,
            plan,
            target_plan,
            receipt: None,
            receipt_trusted: false,
        })
    }

    /// Returns the stored native-voice receipt. A deserialized receipt remains
    /// informational until `apply` replays it through the authenticated bridge.
    #[must_use]
    pub fn receipt(&self) -> Option<&Ymm4OperationReceipt> {
        self.receipt.as_ref()
    }

    /// Records approval for the exact canonical native-voice payload.
    ///
    /// # Errors
    ///
    /// Returns an error for payload tampering, a mismatched approval digest,
    /// invalid lifecycle state, or a stale local revision.
    pub fn approve(
        &mut self,
        approved_digest: &str,
        current_head: RevisionId,
    ) -> Result<(), Ymm4NativeVoiceExportError> {
        self.validate_payload_digest()?;
        if approved_digest != self.patch.digest {
            return Err(Ymm4NativeVoiceExportError::DigestMismatch);
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

    /// Applies the approved native `VoiceItem` operation exactly once.
    ///
    /// # Errors
    ///
    /// Returns an error for failed authorization, payload tampering, stale YMM4
    /// state, an unbound receipt, apply failure, or read-back mismatch.
    #[cfg(test)]
    async fn apply(
        &mut self,
        client: &Ymm4BridgeClient,
        current_head: RevisionId,
    ) -> Result<&Ymm4OperationReceipt, Ymm4NativeVoiceExportError> {
        self.validate_payload_digest()?;
        self.patch.authorize_commit(current_head)?;
        self.validate_current_capabilities(client).await?;
        let request = Ymm4TargetPlanApplyRequest::new(
            self.target_plan.clone(),
            self.target.fingerprint.clone(),
        )
        .map_err(|error| Ymm4NativeVoiceExportError::InvalidTargetPlan(error.to_string()))?;
        self.apply_reserved_request(client, &request, false).await
    }

    async fn apply_reserved_request(
        &mut self,
        client: &Ymm4BridgeClient,
        request: &Ymm4TargetPlanApplyRequest,
        require_replay: bool,
    ) -> Result<&Ymm4OperationReceipt, Ymm4NativeVoiceExportError> {
        self.receipt_trusted = false;
        let expected_request_digest = request.request_digest.clone();
        let result = client.apply_target_plan(request).await?;
        self.receipt = Some(result.receipt);
        if require_replay && !result.replayed {
            return Err(Ymm4NativeVoiceExportError::ApplyFailed(
                "bridge did not mark the reserved native-voice operation as an exact replay".into(),
            ));
        }
        if !result.success {
            return Err(Ymm4NativeVoiceExportError::ApplyFailed(
                self.receipt
                    .as_ref()
                    .and_then(|receipt| receipt.error.clone())
                    .unwrap_or_else(|| {
                        "YMM4 bridge did not return a verified native-voice receipt".into()
                    }),
            ));
        }
        self.validate_verified_receipt(Some(expected_request_digest.as_str()))?;
        self.receipt_trusted = true;
        self.receipt
            .as_ref()
            .ok_or(Ymm4NativeVoiceExportError::MissingVerifiedReceipt)
    }

    /// Applies the approved native-voice creation batch and publishes its
    /// verified receipt under one project-wide external-mutation fence.
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
    ) -> Result<crate::DurableExternalMutationOutcome, Ymm4NativeVoiceExportError> {
        self.validate_payload_digest()?;
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        let request = Ymm4TargetPlanApplyRequest::new(
            self.target_plan.clone(),
            self.target.fingerprint.clone(),
        )
        .map_err(|error| Ymm4NativeVoiceExportError::InvalidTargetPlan(error.to_string()))?;
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
                            "reserved native-voice operation has untrusted terminal evidence".into()
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
                        return Err(Ymm4NativeVoiceExportError::ApplyFailed(message));
                    }
                    replay_existing_bridge_operation = true;
                }
                Err(Ymm4Error::Bridge { status, .. }) if status.as_u16() == 404 => {
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
                        "reserved native-voice operation has no trusted recovery evidence".into()
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
                        return Err(Ymm4NativeVoiceExportError::ApplyFailed(message));
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
                        return Err(Ymm4NativeVoiceExportError::ApplyFailed(message));
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
    ) -> Result<(), Ymm4NativeVoiceExportError> {
        self.receipt_trusted = false;
        let result = client.apply_target_plan(request).await?;
        self.receipt = Some(result.receipt);
        if !result.success || !result.replayed {
            return Err(Ymm4NativeVoiceExportError::ApplyFailed(
                "durably committed native-voice operation did not return an exact replay".into(),
            ));
        }
        let receipt = self.validate_verified_receipt(Some(&request.request_digest))?;
        if canonical_sha256("takegraph-external-receipt", receipt)? != committed.receipt_digest {
            return Err(Ymm4NativeVoiceExportError::DurableReceiptMismatch);
        }
        self.receipt_trusted = true;
        Ok(())
    }

    /// Advances the `TakeGraph` revision after verified native YMM4 read-back.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload changed, no verified receipt exists,
    /// or the core commit guard fails.
    pub fn finalize(
        &mut self,
        current_head: RevisionId,
    ) -> Result<RevisionId, Ymm4NativeVoiceExportError> {
        self.validate_payload_digest()?;
        if self.receipt.is_none() {
            return Err(Ymm4NativeVoiceExportError::MissingVerifiedReceipt);
        }
        if !self.receipt_trusted {
            return Err(Ymm4NativeVoiceExportError::UntrustedReceipt);
        }
        self.validate_verified_receipt(None)?;
        Ok(self.patch.commit(current_head)?)
    }

    /// Atomically records the verified native realization, target link, and
    /// canonical project revision in durable service storage.
    ///
    /// # Errors
    ///
    /// Returns an error for an untrusted receipt, invalid patch lifecycle,
    /// stale durable head, conflicting replay, or corrupt storage.
    pub(crate) fn finalize_durable(
        &mut self,
        store: &crate::DurableProjectStore,
    ) -> Result<RevisionId, Ymm4NativeVoiceExportError> {
        self.validate_payload_digest()?;
        if !self.receipt_trusted {
            return Err(Ymm4NativeVoiceExportError::UntrustedReceipt);
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
                crate::project_store::VerifiedManagedStateUpdate::replacing_identities_from_receipt(
                    self.cues
                        .iter()
                        .map(|cue| takegraph_core::ManagedSemanticIdentity {
                            entity_id: cue.entity_id.clone(),
                            realization_id: Some(cue.realization_id),
                        }),
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
            return Err(Ymm4NativeVoiceExportError::InvalidTargetPlan(format!(
                "durable operation revision {durable_revision:?} does not match patch revision {expected_revision:?}"
            )));
        }
        if self.patch.status == PatchStatus::Approved {
            let core_revision = self.patch.commit(self.patch.base)?;
            debug_assert_eq!(core_revision, durable_revision);
        }
        Ok(durable_revision)
    }

    /// Verifies current YMM4 native managed items against the approved cues.
    ///
    /// # Errors
    ///
    /// Returns a bridge error or a semantic read-back mismatch.
    pub async fn verify(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4NativeVoiceExportError> {
        self.validate_payload_digest()?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != self.target.project_id
            || snapshot.scene_id != self.target.scene_id
        {
            return Err(Ymm4NativeVoiceExportError::VerifyMismatch(
                "active YMM4 project or scene does not match the staged native-voice target".into(),
            ));
        }
        self.verify_items(&snapshot.managed_items)
    }

    fn verify_items(&self, items: &[Ymm4ManagedItem]) -> Result<(), Ymm4NativeVoiceExportError> {
        crate::normalize_realizations(&self.target_plan, items)
            .map(|_| ())
            .map_err(|error| Ymm4NativeVoiceExportError::VerifyMismatch(error.to_string()))
    }

    fn validate_verified_receipt(
        &self,
        expected_request_digest: Option<&str>,
    ) -> Result<&Ymm4OperationReceipt, Ymm4NativeVoiceExportError> {
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(Ymm4NativeVoiceExportError::MissingVerifiedReceipt)?;
        let computed_request = Ymm4TargetPlanApplyRequest::new(
            self.target_plan.clone(),
            self.target.fingerprint.clone(),
        )
        .map_err(|error| Ymm4NativeVoiceExportError::InvalidTargetPlan(error.to_string()))?;
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
            return Err(Ymm4NativeVoiceExportError::ApplyFailed(
                "YMM4 bridge receipt is not verified and bound to this native-voice request".into(),
            ));
        }
        self.verify_items(&receipt.applied_items)?;
        Ok(receipt)
    }

    async fn validate_current_capabilities(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4NativeVoiceExportError> {
        let actual = client.structured_capabilities().await?;
        if actual.capability_digest != self.target_plan.capability_digest {
            return Err(Ymm4NativeVoiceExportError::CapabilityDrift {
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
            .map_err(|error| Ymm4NativeVoiceExportError::InvalidTargetPlan(error.to_string()))
    }
}

fn native_voice_export_digest(
    head: RevisionId,
    operation_id: Uuid,
    target: &Ymm4ProjectSnapshot,
    cues: &[Ymm4NativeVoiceCue],
    plan: &Ymm4NativeVoicePlanResponse,
    target_plan: &TargetPlan,
) -> Result<String, Ymm4NativeVoiceExportError> {
    target_plan
        .validate()
        .map_err(|error| Ymm4NativeVoiceExportError::InvalidTargetPlan(error.to_string()))?;
    if target_plan.base_revision != head
        || target_plan.operation_id != operation_id
        || target_plan.target.project_id != target.project_id
        || target_plan.target.scene_id != target.scene_id
    {
        return Err(Ymm4NativeVoiceExportError::InvalidTargetPlan(
            "target plan is not bound to the native export base, operation, project, and scene"
                .into(),
        ));
    }
    let target_plan_digest = target_plan
        .canonical_digest()
        .map_err(|error| Ymm4NativeVoiceExportError::InvalidTargetPlan(error.to_string()))?;
    let canonical = serde_json::to_vec(&serde_json::json!({
        "baseRevision": head,
        "operationId": operation_id,
        "projectId": target.project_id,
        "sceneId": target.scene_id,
        "expectedFingerprint": target.fingerprint,
        "cues": cues,
        "plan": plan,
        "targetPlanDigest": target_plan_digest,
        "targetPlan": target_plan,
    }))?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

#[derive(Debug, Error)]
pub enum Ymm4NativeVoiceExportError {
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
    #[error("invalid YMM4 native target plan: {0}")]
    InvalidTargetPlan(String),
    #[error("YMM4 capabilities changed after preview (expected {expected}, got {actual})")]
    CapabilityDrift { expected: String, actual: String },
    #[error("approval does not match the current YMM4 native-voice export digest")]
    DigestMismatch,
    #[error(
        "stored YMM4 native-voice digest does not match its canonical payload (stored {stored}, canonical {canonical})"
    )]
    PayloadDigestMismatch { stored: String, canonical: String },
    #[error("YMM4 native-voice apply failed: {0}")]
    ApplyFailed(String),
    #[error("YMM4 native-voice apply has no verified read-back receipt")]
    MissingVerifiedReceipt,
    #[error(
        "YMM4 native-voice receipt must be replayed through the authenticated bridge before finalization"
    )]
    UntrustedReceipt,
    #[error("authenticated native-voice replay does not match the durable receipt digest")]
    DurableReceiptMismatch,
    #[error("YMM4 native-voice verification failed: {0}")]
    VerifyMismatch(String),
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
                    Ymm4Capability::NativeVoiceCreate,
                    Ymm4Capability::UnifiedTargetPlan,
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

    fn cue() -> Ymm4NativeVoiceCue {
        Ymm4NativeVoiceCue {
            realization_id: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
            entity_id: "utt-01".into(),
            revision: 1,
            character_name: "春日部つむぎ".into(),
            display_text: "ここから第二形態です".into(),
            spoken_text: "ここから第二形態です".into(),
            frame: 120,
            layer: 20,
            max_length: 180,
        }
    }

    fn staged_patch() -> Ymm4NativeVoiceExportPatch {
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
        let cues = vec![cue()];
        let plan = Ymm4NativeVoicePlanResponse {
            fingerprint: "external-a".into(),
            create_count: 1,
            duration_resolution: "bounded".into(),
        };
        let target_plan =
            crate::native_voice_target_plan(base, operation_id, &target, &cues, &capabilities())
                .unwrap();
        let digest =
            native_voice_export_digest(base, operation_id, &target, &cues, &plan, &target_plan)
                .unwrap();
        let mut patch = Patch::draft(base, digest);
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();
        Ymm4NativeVoiceExportPatch {
            patch,
            operation_id,
            target,
            cues,
            plan,
            target_plan,
            receipt: None,
            receipt_trusted: false,
        }
    }

    fn managed_voice() -> Ymm4ManagedItem {
        let cue = cue();
        Ymm4ManagedItem {
            entity_id: cue.entity_id,
            revision: cue.revision,
            kind: takegraph_node::ManagedItemKind::Voice,
            frame: cue.frame,
            layer: cue.layer,
            length: 90,
            text: Some(cue.display_text),
            audio_path: None,
            artifact_hash: None,
            speaker: Some(cue.character_name),
            realization_id: Some(cue.realization_id),
        }
    }

    fn verified_receipt(patch: &Ymm4NativeVoiceExportPatch) -> Ymm4OperationReceipt {
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
            applied_items: vec![managed_voice()],
            verified: true,
            error: None,
        }
    }

    #[test]
    fn canonical_payload_round_trips_and_rejects_tampering() {
        let patch = staged_patch();
        let bytes = serde_json::to_vec(&patch).unwrap();
        let loaded = Ymm4NativeVoiceExportPatch::from_json_slice(&bytes).unwrap();
        assert_eq!(loaded.patch.digest, patch.patch.digest);

        let mut value = serde_json::to_value(&patch).unwrap();
        value["cues"][0]["displayText"] = serde_json::json!("tampered");
        let tampered = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            Ymm4NativeVoiceExportPatch::from_json_slice(&tampered),
            Err(Ymm4NativeVoiceExportError::PayloadDigestMismatch { .. })
        ));
        assert!(serde_json::from_slice::<Ymm4NativeVoiceExportPatch>(&tampered).is_err());
    }

    #[test]
    fn approval_is_digest_and_revision_bound() {
        let mut patch = staged_patch();
        assert!(matches!(
            patch.approve("wrong", RevisionId(4)),
            Err(Ymm4NativeVoiceExportError::DigestMismatch)
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
    fn approval_and_apply_reject_in_memory_payload_tampering() {
        let mut approval_patch = staged_patch();
        let digest = approval_patch.patch.digest.clone();
        approval_patch.plan.duration_resolution = "unknown".into();
        assert!(matches!(
            approval_patch.approve(&digest, RevisionId(4)),
            Err(Ymm4NativeVoiceExportError::PayloadDigestMismatch { .. })
        ));

        let mut apply_patch = staged_patch();
        let digest = apply_patch.patch.digest.clone();
        apply_patch.approve(&digest, RevisionId(4)).unwrap();
        apply_patch.cues[0].max_length += 1;
        let client = Ymm4BridgeClient::new("http://127.0.0.1:9", "test-token").unwrap();
        let mut future = std::pin::pin!(apply_patch.apply(&client, RevisionId(4)));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            std::future::Future::poll(future.as_mut(), &mut context),
            std::task::Poll::Ready(Err(
                Ymm4NativeVoiceExportError::PayloadDigestMismatch { .. }
            ))
        ));
    }

    #[test]
    fn verifies_one_bounded_native_item_per_realization() {
        let patch = staged_patch();
        let item = managed_voice();
        patch.verify_items(std::slice::from_ref(&item)).unwrap();

        let mut wrong_kind = item.clone();
        wrong_kind.kind = takegraph_node::ManagedItemKind::Audio;
        assert!(matches!(
            patch.verify_items(std::slice::from_ref(&wrong_kind)),
            Err(Ymm4NativeVoiceExportError::VerifyMismatch(_))
        ));

        let mut too_long = item;
        too_long.length = patch.cues[0].max_length + 1;
        assert!(matches!(
            patch.verify_items(std::slice::from_ref(&too_long)),
            Err(Ymm4NativeVoiceExportError::VerifyMismatch(_))
        ));
    }

    #[test]
    fn finalize_requires_verified_receipt() {
        let mut patch = staged_patch();
        let digest = patch.patch.digest.clone();
        patch.approve(&digest, RevisionId(4)).unwrap();
        assert!(matches!(
            patch.finalize(RevisionId(4)),
            Err(Ymm4NativeVoiceExportError::MissingVerifiedReceipt)
        ));
    }

    #[test]
    fn finalize_rejects_unbound_or_forged_receipt() {
        let mut patch = staged_patch();
        let digest = patch.patch.digest.clone();
        patch.approve(&digest, RevisionId(4)).unwrap();
        let mut forged = verified_receipt(&patch);
        forged.project_id = "other-project".into();
        patch.receipt = Some(forged);
        patch.receipt_trusted = true;
        assert!(matches!(
            patch.finalize(RevisionId(4)),
            Err(Ymm4NativeVoiceExportError::ApplyFailed(_))
        ));

        patch.receipt = Some(verified_receipt(&patch));
        patch.receipt_trusted = false;
        assert!(matches!(
            patch.finalize(RevisionId(4)),
            Err(Ymm4NativeVoiceExportError::UntrustedReceipt)
        ));
        patch.receipt_trusted = true;
        assert_eq!(patch.finalize(RevisionId(4)).unwrap(), RevisionId(5));
    }

    #[test]
    fn native_finalize_uses_the_same_durable_revision_authority() {
        let root = std::env::temp_dir().join(format!("takegraph-native-store-{}", Uuid::new_v4()));
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
        assert_eq!(store.head().unwrap(), RevisionId(5));
        assert_eq!(store.snapshot().unwrap().target_links.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
