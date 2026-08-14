//! Persistable Phase-4 YMM4 native-extension lifecycle.
//!
//! This module connects the portable planner to the authenticated bridge while
//! retaining a path-free core plan. Target descriptor observations, opaque
//! native effects, exact loss approvals, and node-owned artifact mappings are
//! all approval-bound.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use takegraph_core::{
    AssetKind, ChangeBudget, ManagedSemanticIdentity, NativeExtensionAction, NativeExtensionIntent,
    NativeExtensionPlan, OpaqueNativeEffect, Patch, PatchError, PatchStatus, PreservedNativeField,
    RevisionId, ScopeFingerprints, TargetIdentity, approval_digests_match, canonical_sha256,
};
use takegraph_node::{
    CapabilityRequirement, NativeExtensionNodeError, StructuredYmm4Capabilities, Ymm4BridgeClient,
    Ymm4DescriptorCatalog, Ymm4Error, Ymm4ExistingNativeExtensionKind, Ymm4ExistingUpdateMode,
    Ymm4NativeExtensionApplyRequest, Ymm4NativeExtensionApplyResponse, Ymm4NativeExtensionArtifact,
    Ymm4NativeExtensionPlanRequest, Ymm4NativeExtensionPlanResponse,
    Ymm4NativeExtensionRealization, Ymm4NativeExtensionStatus, Ymm4ProjectSnapshot,
    materialize_native_extension_artifact,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    DurableProjectStore, ExistingNativeExtension, ExistingNativeExtensionKind, ExistingUpdateMode,
    NativeExtensionCapabilities, NativeExtensionFeature, NativeExtensionObservation,
    NativeExtensionPlanContext, NativeExtensionPlanError, VerifiedTargetBinding,
    plan_native_extensions_with_identity_overrides,
};

/// A local source path paired only with an immutable content identity. The
/// source path is never copied into the portable plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeExtensionArtifactSource {
    pub artifact_digest: String,
    pub source_path: PathBuf,
}

/// Input manifest for staging a Phase-4 plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeExtensionStageManifest {
    pub intents: Vec<NativeExtensionIntent>,
    #[serde(default)]
    pub artifact_sources: Vec<NativeExtensionArtifactSource>,
    pub change_budget: ChangeBudget,
}

/// Persisted stage/apply/verification task used by CLI and MCP workflows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionTask {
    pub patch: Patch,
    pub operation_id: Uuid,
    pub target: Ymm4ProjectSnapshot,
    pub descriptor_catalog: Ymm4DescriptorCatalog,
    pub artifact_root: PathBuf,
    pub artifacts: Vec<Ymm4NativeExtensionArtifact>,
    pub bridge_preview: Ymm4NativeExtensionPlanResponse,
    pub plan: NativeExtensionPlan,
    pub receipt: Option<Ymm4NativeExtensionApplyResponse>,
    #[serde(skip)]
    receipt_trusted: bool,
}

impl Ymm4NativeExtensionTask {
    /// Stages a target-observed, portable plan without changing YMM4.
    ///
    /// Asset bytes are copied to an immutable node-owned root, then rehashed.
    /// Exact replacement losses must already be named in each intent's
    /// `approvedLossyFields`; the pure planner fails closed otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale target, descriptor/capability mismatch,
    /// unsafe artifact, unsupported typed operation, or insufficient loss
    /// allowlist.
    pub async fn stage(
        client: &Ymm4BridgeClient,
        base_revision: RevisionId,
        manifest: NativeExtensionStageManifest,
        artifact_root: PathBuf,
    ) -> Result<Self, Ymm4NativeExtensionError> {
        Self::stage_with_identity_overrides(
            client,
            base_revision,
            manifest,
            artifact_root,
            BTreeMap::new(),
        )
        .await
    }

    /// Stages a reconciliation re-export while preserving canonical
    /// realization IDs. This still returns an ordinary previewable,
    /// independently approvable native-extension task and performs no apply.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid identity overrides, target/descriptor or
    /// capability drift, unsafe artifacts, or an invalid native plan.
    pub async fn stage_with_identity_overrides(
        client: &Ymm4BridgeClient,
        base_revision: RevisionId,
        manifest: NativeExtensionStageManifest,
        artifact_root: PathBuf,
        identity_overrides: BTreeMap<String, Uuid>,
    ) -> Result<Self, Ymm4NativeExtensionError> {
        Self::stage_with_operation_id_and_identity_overrides(
            client,
            base_revision,
            manifest,
            artifact_root,
            Uuid::new_v4(),
            identity_overrides,
        )
        .await
    }

    /// Stages a reconciliation child using its already-durable operation ID
    /// while preserving any canonical realization identities.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty operation ID or the ordinary extension
    /// staging validation, bridge, artifact, and planning failures.
    pub async fn stage_with_operation_id_and_identity_overrides(
        client: &Ymm4BridgeClient,
        base_revision: RevisionId,
        manifest: NativeExtensionStageManifest,
        artifact_root: PathBuf,
        operation_id: Uuid,
        identity_overrides: BTreeMap<String, Uuid>,
    ) -> Result<Self, Ymm4NativeExtensionError> {
        if operation_id.is_nil() {
            return Err(Ymm4NativeExtensionError::InvalidPayload(
                "operation ID must not be nil".into(),
            ));
        }
        validate_manifest(&manifest)?;
        let health = client.health().await?;
        let raw_capabilities = client.capabilities().await?;
        let structured = StructuredYmm4Capabilities::from_bridge(&health, &raw_capabilities)
            .map_err(|error| Ymm4NativeExtensionError::Capability(error.to_string()))?;
        let target = client.snapshot().await?;
        crate::require_existing_project_path(&target.project_path)?;
        let descriptor_catalog = client.native_descriptors().await?;
        descriptor_catalog.validate()?;
        validate_catalog_target(&descriptor_catalog, &target)?;

        let artifact_root = std::path::absolute(artifact_root)?;
        std::fs::create_dir_all(&artifact_root)?;
        let artifact_root = std::fs::canonicalize(artifact_root)?;
        let artifacts = materialize_manifest_artifacts(&manifest, &artifact_root)?;
        let preflight_request = Ymm4NativeExtensionPlanRequest::new(
            operation_id,
            target.project_id.clone(),
            target.scene_id.clone(),
            target.fingerprint.clone(),
            descriptor_catalog.catalog_digest.clone(),
            manifest.intents.clone(),
            artifacts.clone(),
        );
        let bridge_preview = client.plan_native_extensions(&preflight_request).await?;
        validate_preflight(&bridge_preview, &target, &descriptor_catalog)?;

        let planning_catalog = descriptor_catalog.planning_catalog()?;
        let capabilities = planner_capabilities(&structured);
        let observation = planner_observation(&bridge_preview)?;
        let target_identity = target_identity(&structured, &target);
        let expected_scope = scoped_fingerprints(&target_identity, &target)?;
        let plan = plan_native_extensions_with_identity_overrides(
            NativeExtensionPlanContext {
                operation_id,
                base_revision,
                target: target_identity,
                capability_digest: structured.capability_digest,
                expected_scope,
                change_budget: manifest.change_budget,
            },
            manifest.intents,
            &planning_catalog,
            &capabilities,
            &observation,
            &identity_overrides,
        )?;
        validate_plan_artifacts(&plan, &artifacts)?;

        let mut task = Self {
            patch: Patch::draft(base_revision, "pending"),
            operation_id,
            target,
            descriptor_catalog,
            artifact_root,
            artifacts,
            bridge_preview,
            plan,
            receipt: None,
            receipt_trusted: false,
        };
        task.patch.digest = task.payload_digest()?;
        task.patch.validate()?;
        task.patch.materialize_preview()?;
        Ok(task)
    }

    /// Loads persisted state while rejecting field/digest tampering. Persisted
    /// receipts remain untrusted until replayed through the authenticated bridge.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, task tampering, invalid descriptor
    /// state, or missing/changed immutable artifacts.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, Ymm4NativeExtensionError> {
        let mut task: Self = serde_json::from_slice(bytes)?;
        task.receipt_trusted = false;
        task.validate_payload()?;
        Ok(task)
    }

    /// Approves the exact target-observed payload at the exact canonical head.
    ///
    /// # Errors
    ///
    /// Returns an error for task tampering, stale canonical revision, a digest
    /// mismatch, or an invalid patch lifecycle state.
    pub fn approve(
        &mut self,
        approved_digest: &str,
        current_head: RevisionId,
    ) -> Result<(), Ymm4NativeExtensionError> {
        self.validate_payload()?;
        if self.patch.base != current_head {
            return Err(PatchError::StaleBase {
                expected: self.patch.base,
                actual: current_head,
            }
            .into());
        }
        if !approval_digests_match(&self.patch.digest, approved_digest) {
            return Err(Ymm4NativeExtensionError::ApprovalDigestMismatch);
        }
        if self.patch.status == PatchStatus::Previewable {
            self.patch.approve()?;
        }
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        Ok(())
    }

    async fn apply_reserved_request(
        &mut self,
        client: &Ymm4BridgeClient,
        request: &Ymm4NativeExtensionApplyRequest,
    ) -> Result<&Ymm4NativeExtensionApplyResponse, Ymm4NativeExtensionError> {
        let receipt = client.apply_native_extensions(request).await?;
        self.receipt = Some(receipt);
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(Ymm4NativeExtensionError::MissingVerifiedReceipt)?;
        validate_receipt(request, receipt)?;
        verify_realizations(&self.plan, &receipt.realizations)?;
        self.receipt_trusted = true;
        self.receipt
            .as_ref()
            .ok_or(Ymm4NativeExtensionError::MissingVerifiedReceipt)
    }

    /// Applies the approved native-extension plan and publishes its verified
    /// projection under one project-wide external-mutation fence.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale head, an active detach reservation, bridge
    /// apply/read-back failure, or durable publication failure.
    #[allow(clippy::too_many_lines)]
    pub async fn apply_and_finalize_durable(
        &mut self,
        client: &Ymm4BridgeClient,
        store: &DurableProjectStore,
        current_head: RevisionId,
    ) -> Result<crate::DurableExternalMutationOutcome, Ymm4NativeExtensionError> {
        self.validate_payload()?;
        crate::require_existing_project_path(&self.target.project_path)?;
        crate::require_existing_project_path(&client.snapshot().await?.project_path)?;
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        let request = self.apply_request()?;
        let target = VerifiedTargetBinding {
            adapter_id: self.plan.target.adapter_id.clone(),
            target_project_id: self.target.project_id.clone(),
            scene_id: self.target.scene_id.clone(),
            target_identity_digest: self.plan.expected_scope.target_identity_digest.clone(),
            verified_fingerprint: self.target.fingerprint.clone(),
        };
        let scope = crate::external_mutation::acquire_external_mutation_scope(
            store,
            self.operation_id,
            current_head,
        )?;
        if scope.committed.is_none() && !scope.pending_replay {
            self.verify_live_dependencies(client, true).await?;
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
        if scope.pending_replay {
            match client.native_extension_operation(self.operation_id).await {
                Ok(receipt) => {
                    let request_bound =
                        crate::external_mutation::native_extension_receipt_is_request_bound(
                            &receipt, &request,
                        );
                    let terminal = matches!(
                        receipt.status,
                        Ymm4NativeExtensionStatus::NotStarted
                            | Ymm4NativeExtensionStatus::Stale
                            | Ymm4NativeExtensionStatus::Failed
                            | Ymm4NativeExtensionStatus::RolledBack
                            | Ymm4NativeExtensionStatus::RecoveryRequired
                    );
                    if !request_bound || terminal {
                        let safe_abort = crate::external_mutation::
                            native_extension_receipt_proves_exact_rollback(&receipt, &request)
                            || crate::external_mutation::
                                native_extension_receipt_proves_no_mutation(&receipt, &request)
                            || crate::external_mutation::
                                native_extension_receipt_proves_not_started(&receipt, &request);
                        let message = receipt.error.clone().unwrap_or_else(|| {
                            "reserved native-extension operation has untrusted terminal evidence"
                                .into()
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
                        return Err(Ymm4NativeExtensionError::PendingRecovery(message));
                    }
                }
                Err(Ymm4Error::Bridge { status, .. }) if status.as_u16() == 404 => {
                    let sealed = client.seal_native_extension_not_started(&request).await?;
                    let not_started =
                        crate::external_mutation::native_extension_receipt_proves_not_started(
                            &sealed, &request,
                        );
                    let message = sealed.error.clone().unwrap_or_else(|| {
                        "reserved native-extension operation has no trusted recovery evidence"
                            .into()
                    });
                    self.receipt = Some(sealed);
                    if not_started {
                        store.abort_external_commit_reservation(
                            self.operation_id,
                            self.patch.base,
                            &self.patch.digest,
                            &request.request_digest,
                            &target,
                        )?;
                        return Err(Ymm4NativeExtensionError::PendingRecovery(message));
                    }
                    if !self.receipt.as_ref().is_some_and(|receipt| {
                        crate::external_mutation::native_extension_receipt_is_request_bound(
                            receipt, &request,
                        )
                    }) {
                        return Err(Ymm4NativeExtensionError::PendingRecovery(message));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }

        if let Err(error) = self.apply_reserved_request(client, &request).await {
            let safe_abort = self.receipt.as_ref().is_some_and(|receipt| {
                crate::external_mutation::native_extension_receipt_proves_exact_rollback(
                    receipt, &request,
                ) || crate::external_mutation::native_extension_receipt_proves_no_mutation(
                    receipt, &request,
                ) || crate::external_mutation::native_extension_receipt_proves_not_started(
                    receipt, &request,
                )
            });
            let sealed_not_started =
                if safe_abort {
                    false
                } else {
                    match client.seal_native_extension_not_started(&request).await {
                        Ok(sealed) => {
                            let proof = crate::external_mutation::
                            native_extension_receipt_proves_not_started(&sealed, &request);
                            self.receipt = Some(sealed);
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
        request: &Ymm4NativeExtensionApplyRequest,
        committed: &crate::ExternalCommitRecord,
    ) -> Result<(), Ymm4NativeExtensionError> {
        self.receipt_trusted = false;
        let receipt = client.apply_native_extensions(request).await?;
        self.receipt = Some(receipt);
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(Ymm4NativeExtensionError::MissingVerifiedReceipt)?;
        validate_receipt(request, receipt)?;
        verify_realizations(&self.plan, &receipt.realizations)?;
        if canonical_sha256("takegraph-external-receipt", receipt)? != committed.receipt_digest {
            return Err(Ymm4NativeExtensionError::DurableReceiptMismatch);
        }
        self.receipt_trusted = true;
        Ok(())
    }

    /// Rechecks the staged target, descriptor catalog, capability schemas, and
    /// immutable artifacts before recording human approval.
    ///
    /// # Errors
    ///
    /// Returns an error if any approval dependency changed after preview.
    pub async fn revalidate_preview(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4NativeExtensionError> {
        self.validate_payload()?;
        self.verify_live_dependencies(client, true).await
    }

    /// Replays the same request digest to recover/status-check an operation.
    /// The bridge must return the same bound receipt rather than execute a
    /// different request under the same operation ID.
    ///
    /// # Errors
    ///
    /// Returns an error for task/dependency drift, transport failure, or an
    /// unbound/unverified replay response.
    pub async fn replay_status(
        &mut self,
        client: &Ymm4BridgeClient,
    ) -> Result<&Ymm4NativeExtensionApplyResponse, Ymm4NativeExtensionError> {
        self.validate_payload()?;
        self.verify_live_dependencies(client, false).await?;
        let request = self.apply_request()?;
        let receipt = client.apply_native_extensions(&request).await?;
        validate_receipt(&request, &receipt)?;
        verify_realizations(&self.plan, &receipt.realizations)?;
        self.receipt = Some(receipt);
        self.receipt_trusted = true;
        self.receipt
            .as_ref()
            .ok_or(Ymm4NativeExtensionError::MissingVerifiedReceipt)
    }

    /// Independently re-observes the current target. This catches post-apply
    /// user edits that a replayed historical receipt cannot detect.
    ///
    /// # Errors
    ///
    /// Returns an error for task, descriptor, artifact, target, identity, kind,
    /// or unknown-effect state drift.
    pub async fn verify_current(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4NativeExtensionError> {
        self.validate_payload()?;
        for artifact in &self.artifacts {
            artifact.verify(&self.artifact_root)?;
        }
        let current_catalog = client.native_descriptors().await?;
        self.descriptor_catalog
            .require_unchanged(&current_catalog)?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != self.target.project_id
            || snapshot.scene_id != self.target.scene_id
        {
            return Err(Ymm4NativeExtensionError::TargetDrift(
                "active project or scene changed".into(),
            ));
        }
        let request = Ymm4NativeExtensionPlanRequest::new(
            self.operation_id,
            snapshot.project_id.clone(),
            snapshot.scene_id.clone(),
            snapshot.fingerprint.clone(),
            current_catalog.catalog_digest.clone(),
            self.plan
                .operations
                .iter()
                .map(|operation| operation.intent.clone())
                .collect(),
            self.artifacts.clone(),
        );
        let observed = client.plan_native_extensions(&request).await?;
        validate_preflight(&observed, &snapshot, &current_catalog)?;
        verify_observation(&self.plan, &planner_observation(&observed)?)
    }

    /// Advances the durable canonical revision only after a live authenticated
    /// receipt has been checked during this process.
    ///
    /// # Errors
    ///
    /// Returns an error for an untrusted/missing receipt, invalid read-back,
    /// stale durable head, operation conflict, or persistence failure.
    pub(crate) fn finalize_durable(
        &mut self,
        store: &DurableProjectStore,
    ) -> Result<RevisionId, Ymm4NativeExtensionError> {
        self.validate_payload()?;
        if !self.receipt_trusted {
            return Err(Ymm4NativeExtensionError::UntrustedReceipt);
        }
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(Ymm4NativeExtensionError::MissingVerifiedReceipt)?;
        let request = self.apply_request()?;
        validate_receipt(&request, receipt)?;
        verify_realizations(&self.plan, &receipt.realizations)?;
        if self.patch.status == PatchStatus::Approved {
            self.patch.authorize_commit(self.patch.base)?;
        } else if self.patch.status != PatchStatus::Committed {
            return Err(PatchError::UnexpectedStatus {
                expected: PatchStatus::Approved,
                actual: self.patch.status,
            }
            .into());
        }
        let managed_identities = self
            .plan
            .operations
            .iter()
            .map(|operation| ManagedSemanticIdentity {
                entity_id: native_extension_entity_id(&operation.intent).to_owned(),
                realization_id: Some(operation.realization_id),
            })
            .collect::<Vec<_>>();
        let managed_update =
            crate::project_store::VerifiedManagedStateUpdate::replacing_native_extensions_from_receipt(
                managed_identities,
                receipt,
            )?;
        let proof = crate::project_store::VerifiedExternalCommit::from_receipt(
            self.operation_id,
            self.patch.base,
            self.patch.digest.clone(),
            receipt.request_digest.clone(),
            receipt,
            VerifiedTargetBinding {
                adapter_id: self.plan.target.adapter_id.clone(),
                target_project_id: self.target.project_id.clone(),
                scene_id: self.target.scene_id.clone(),
                target_identity_digest: self.plan.expected_scope.target_identity_digest.clone(),
                verified_fingerprint: receipt.after_fingerprint.clone(),
            },
        )?
        .with_managed_state_update(managed_update)?;
        let revision = store.commit_verified_external(&proof)?;
        if self.patch.status == PatchStatus::Approved {
            let core_revision = self.patch.commit(self.patch.base)?;
            if core_revision != revision {
                return Err(Ymm4NativeExtensionError::TargetDrift(
                    "durable and core revisions disagreed".into(),
                ));
            }
        }
        Ok(revision)
    }

    fn apply_request(&self) -> Result<Ymm4NativeExtensionApplyRequest, Ymm4NativeExtensionError> {
        Ok(Ymm4NativeExtensionApplyRequest::new(
            self.target.project_id.clone(),
            self.target.scene_id.clone(),
            self.target.fingerprint.clone(),
            self.descriptor_catalog.catalog_digest.clone(),
            self.descriptor_catalog.driver_profile_digest.clone(),
            self.plan.clone(),
            self.artifacts.clone(),
        )?)
    }

    async fn verify_live_dependencies(
        &self,
        client: &Ymm4BridgeClient,
        require_original_fingerprint: bool,
    ) -> Result<(), Ymm4NativeExtensionError> {
        for artifact in &self.artifacts {
            artifact.verify(&self.artifact_root)?;
        }
        let current_catalog = client.native_descriptors().await?;
        self.descriptor_catalog
            .require_unchanged(&current_catalog)?;
        let health = client.health().await?;
        let raw = client.capabilities().await?;
        let structured = StructuredYmm4Capabilities::from_bridge(&health, &raw)
            .map_err(|error| Ymm4NativeExtensionError::Capability(error.to_string()))?;
        if structured.capability_digest != self.plan.capability_digest {
            return Err(Ymm4NativeExtensionError::CapabilityDrift {
                expected: self.plan.capability_digest.clone(),
                actual: structured.capability_digest,
            });
        }
        structured
            .require(&plan_capability_requirements(&self.plan))
            .map_err(|error| Ymm4NativeExtensionError::Capability(error.to_string()))?;
        if require_original_fingerprint {
            let snapshot = client.snapshot().await?;
            if snapshot.project_id != self.target.project_id
                || snapshot.scene_id != self.target.scene_id
                || snapshot.fingerprint != self.target.fingerprint
            {
                return Err(Ymm4NativeExtensionError::TargetDrift(
                    "active YMM4 target changed after preview".into(),
                ));
            }
        }
        Ok(())
    }

    fn validate_payload(&self) -> Result<(), Ymm4NativeExtensionError> {
        if self.operation_id != self.plan.operation_id
            || self.patch.base != self.plan.base_revision
            || self.target.project_id != self.plan.target.project_id
            || self.target.scene_id != self.plan.target.scene_id
        {
            return Err(Ymm4NativeExtensionError::InvalidPayload(
                "task, plan, base revision, operation, and target are not bound".into(),
            ));
        }
        self.descriptor_catalog.validate()?;
        validate_catalog_target(&self.descriptor_catalog, &self.target)?;
        validate_preflight(&self.bridge_preview, &self.target, &self.descriptor_catalog)?;
        self.plan
            .validate()
            .map_err(|error| Ymm4NativeExtensionError::InvalidPayload(error.to_string()))?;
        validate_plan_artifacts(&self.plan, &self.artifacts)?;
        for artifact in &self.artifacts {
            artifact.verify(&self.artifact_root)?;
        }
        let actual = self.payload_digest()?;
        if self.patch.digest != actual {
            return Err(Ymm4NativeExtensionError::PayloadDigestMismatch {
                stored: self.patch.digest.clone(),
                actual,
            });
        }
        Ok(())
    }

    fn payload_digest(&self) -> Result<String, Ymm4NativeExtensionError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload<'a> {
            operation_id: Uuid,
            base_revision: RevisionId,
            target: &'a Ymm4ProjectSnapshot,
            descriptor_catalog: &'a Ymm4DescriptorCatalog,
            artifact_root: &'a PathBuf,
            artifacts: &'a [Ymm4NativeExtensionArtifact],
            bridge_preview: &'a Ymm4NativeExtensionPlanResponse,
            plan: &'a NativeExtensionPlan,
        }
        Ok(canonical_sha256(
            "takegraph-ymm4-native-extension-task-v1",
            &Payload {
                operation_id: self.operation_id,
                base_revision: self.patch.base,
                target: &self.target,
                descriptor_catalog: &self.descriptor_catalog,
                artifact_root: &self.artifact_root,
                artifacts: &self.artifacts,
                bridge_preview: &self.bridge_preview,
                plan: &self.plan,
            },
        )?)
    }
}

fn native_extension_entity_id(intent: &NativeExtensionIntent) -> &str {
    match intent {
        NativeExtensionIntent::UpsertPortrait(value) => &value.entity_id,
        NativeExtensionIntent::UpsertAsset(value) => &value.entity_id,
        NativeExtensionIntent::MutateEffect(value) => &value.target_entity_id,
        NativeExtensionIntent::InstantiateTemplate(value) => &value.entity_id,
    }
}

fn validate_manifest(
    manifest: &NativeExtensionStageManifest,
) -> Result<(), Ymm4NativeExtensionError> {
    if manifest.intents.is_empty() {
        return Err(Ymm4NativeExtensionError::InvalidPayload(
            "at least one native-extension intent is required".into(),
        ));
    }
    if manifest.change_budget.allow_unmanaged_changes {
        return Err(Ymm4NativeExtensionError::InvalidPayload(
            "Phase-4 plans cannot authorize unmanaged target changes".into(),
        ));
    }
    Ok(())
}

fn materialize_manifest_artifacts(
    manifest: &NativeExtensionStageManifest,
    artifact_root: &std::path::Path,
) -> Result<Vec<Ymm4NativeExtensionArtifact>, Ymm4NativeExtensionError> {
    let sources = manifest
        .artifact_sources
        .iter()
        .map(|source| {
            (
                source.artifact_digest.as_str(),
                source.source_path.as_path(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if sources.len() != manifest.artifact_sources.len() {
        return Err(Ymm4NativeExtensionError::InvalidPayload(
            "artifact source digests must be unique".into(),
        ));
    }
    let mut required = BTreeMap::new();
    for intent in &manifest.intents {
        if let NativeExtensionIntent::UpsertAsset(asset) = intent
            && let Some(existing) =
                required.insert(asset.asset.artifact_digest.clone(), &asset.asset)
            && existing != &asset.asset
        {
            return Err(Ymm4NativeExtensionError::InvalidPayload(format!(
                "artifact {} has conflicting declarations",
                asset.asset.artifact_digest
            )));
        }
    }
    if sources.len() != required.len()
        || sources.keys().any(|digest| !required.contains_key(*digest))
    {
        return Err(Ymm4NativeExtensionError::InvalidPayload(
            "artifactSources must exactly match all distinct asset intents".into(),
        ));
    }
    let mut artifacts = Vec::with_capacity(required.len());
    for (digest, reference) in required {
        let source = sources
            .get(digest.as_str())
            .ok_or_else(|| Ymm4NativeExtensionError::MissingArtifact(digest.clone()))?;
        artifacts.push(materialize_native_extension_artifact(
            source,
            reference,
            artifact_root,
        )?);
    }
    artifacts.sort_by(|left, right| left.artifact_digest.cmp(&right.artifact_digest));
    Ok(artifacts)
}

fn validate_plan_artifacts(
    plan: &NativeExtensionPlan,
    artifacts: &[Ymm4NativeExtensionArtifact],
) -> Result<(), Ymm4NativeExtensionError> {
    let mapped = artifacts
        .iter()
        .map(|artifact| (artifact.artifact_digest.as_str(), artifact))
        .collect::<BTreeMap<_, _>>();
    let required = plan
        .operations
        .iter()
        .filter_map(|operation| match &operation.intent {
            NativeExtensionIntent::UpsertAsset(asset) => Some(&asset.asset),
            _ => None,
        })
        .collect::<Vec<_>>();
    for reference in required {
        let artifact = mapped
            .get(reference.artifact_digest.as_str())
            .ok_or_else(|| {
                Ymm4NativeExtensionError::MissingArtifact(reference.artifact_digest.clone())
            })?;
        if artifact.byte_length != reference.byte_length
            || artifact.media_type != reference.media_type
            || artifact.kind != reference.kind
            || artifact.sha256 != reference.artifact_digest
        {
            return Err(Ymm4NativeExtensionError::InvalidPayload(format!(
                "artifact mapping differs from plan for {}",
                reference.artifact_digest
            )));
        }
    }
    Ok(())
}

fn validate_catalog_target(
    catalog: &Ymm4DescriptorCatalog,
    target: &Ymm4ProjectSnapshot,
) -> Result<(), Ymm4NativeExtensionError> {
    if catalog.project_id != target.project_id || catalog.scene_id != target.scene_id {
        return Err(Ymm4NativeExtensionError::TargetDrift(
            "descriptor catalog belongs to another project or scene".into(),
        ));
    }
    Ok(())
}

fn validate_preflight(
    response: &Ymm4NativeExtensionPlanResponse,
    target: &Ymm4ProjectSnapshot,
    catalog: &Ymm4DescriptorCatalog,
) -> Result<(), Ymm4NativeExtensionError> {
    if response.fingerprint != target.fingerprint
        || response.descriptor_catalog_digest != catalog.catalog_digest
        || response.driver_profile_digest != catalog.driver_profile_digest
    {
        return Err(Ymm4NativeExtensionError::TargetDrift(
            "native-extension preview is not bound to the staged target/descriptors".into(),
        ));
    }
    Ok(())
}

fn planner_capabilities(structured: &StructuredYmm4Capabilities) -> NativeExtensionCapabilities {
    NativeExtensionCapabilities {
        features: structured
            .features
            .iter()
            .map(|(name, feature)| {
                (
                    name.clone(),
                    NativeExtensionFeature {
                        version: feature.version,
                        available: feature.available,
                        schema_digest: feature.schema_digest.clone(),
                    },
                )
            })
            .collect(),
    }
}

fn planner_observation(
    response: &Ymm4NativeExtensionPlanResponse,
) -> Result<NativeExtensionObservation, Ymm4NativeExtensionError> {
    let mut existing = BTreeMap::new();
    for (key, value) in &response.observation.existing {
        if key != &value.logical_key {
            return Err(Ymm4NativeExtensionError::InvalidPayload(format!(
                "observation key {key} differs from {}",
                value.logical_key
            )));
        }
        existing.insert(
            key.clone(),
            ExistingNativeExtension {
                logical_key: value.logical_key.clone(),
                realization_id: value.realization_id,
                kind: match value.kind {
                    Ymm4ExistingNativeExtensionKind::Portrait => {
                        ExistingNativeExtensionKind::Portrait
                    }
                    Ymm4ExistingNativeExtensionKind::Face => ExistingNativeExtensionKind::Face,
                    Ymm4ExistingNativeExtensionKind::Image
                    | Ymm4ExistingNativeExtensionKind::Template => {
                        ExistingNativeExtensionKind::Image
                    }
                    Ymm4ExistingNativeExtensionKind::Video => ExistingNativeExtensionKind::Video,
                    Ymm4ExistingNativeExtensionKind::Audio => ExistingNativeExtensionKind::Audio,
                    Ymm4ExistingNativeExtensionKind::Bgm => ExistingNativeExtensionKind::Bgm,
                    Ymm4ExistingNativeExtensionKind::ManagedEffect => {
                        ExistingNativeExtensionKind::ManagedEffect
                    }
                },
                update_mode: match &value.update_mode {
                    Ymm4ExistingUpdateMode::InPlace => ExistingUpdateMode::InPlace,
                    Ymm4ExistingUpdateMode::Replace { lossy_fields } => {
                        ExistingUpdateMode::Replace {
                            lossy_fields: lossy_fields.clone(),
                        }
                    }
                },
                preserved_fields: value
                    .preserved_fields
                    .iter()
                    .map(|field| PreservedNativeField {
                        field: field.field.clone(),
                        state_digest: field.state_digest.clone(),
                    })
                    .collect(),
                unknown_effects: value
                    .unknown_effects
                    .iter()
                    .map(|effect| OpaqueNativeEffect {
                        stable_type_id: effect.stable_type_id.clone(),
                        instance_key: effect.instance_key.clone(),
                        state_digest: effect.state_digest.clone(),
                    })
                    .collect(),
            },
        );
    }
    Ok(NativeExtensionObservation { existing })
}

fn target_identity(
    structured: &StructuredYmm4Capabilities,
    target: &Ymm4ProjectSnapshot,
) -> TargetIdentity {
    crate::ymm4_target_plan::ymm4_target_identity(structured, target)
}

fn scoped_fingerprints(
    identity: &TargetIdentity,
    target: &Ymm4ProjectSnapshot,
) -> Result<ScopeFingerprints, Ymm4NativeExtensionError> {
    Ok(ScopeFingerprints {
        target_identity_digest: canonical_sha256("takegraph-ymm4-target-identity", identity)?,
        managed_state_digest: canonical_sha256(
            "takegraph-ymm4-managed-state",
            &target.managed_items,
        )?,
        conflict_scope_digest: canonical_sha256(
            "takegraph-ymm4-conflict-scope",
            &serde_json::json!({
                "fingerprint": target.fingerprint,
                "unmanagedContextCount": target.unmanaged_context_count,
            }),
        )?,
    })
}

fn plan_capability_requirements(plan: &NativeExtensionPlan) -> Vec<CapabilityRequirement> {
    plan.operations
        .iter()
        .flat_map(|operation| operation.capability_dependencies.iter())
        .map(|dependency| CapabilityRequirement {
            feature: dependency.feature.clone(),
            minimum_version: dependency.minimum_version,
            schema_digest: dependency.schema_digest.clone(),
        })
        .collect()
}

fn validate_receipt(
    request: &Ymm4NativeExtensionApplyRequest,
    response: &Ymm4NativeExtensionApplyResponse,
) -> Result<(), Ymm4NativeExtensionError> {
    if response.operation_id != request.operation_id
        || response.request_digest != request.request_digest
        || response.project_id != request.project_id
        || response.scene_id != request.scene_id
        || response.before_fingerprint != request.expected_fingerprint
        || response.descriptor_catalog_digest != request.descriptor_catalog_digest
        || response.driver_profile_digest != request.driver_profile_digest
        || response.status != Ymm4NativeExtensionStatus::Verified
        || !response.verified
        || response.after_fingerprint.trim().is_empty()
        || response.error.is_some()
    {
        return Err(Ymm4NativeExtensionError::UnverifiedReceipt);
    }
    Ok(())
}

fn verify_realizations(
    plan: &NativeExtensionPlan,
    realizations: &[Ymm4NativeExtensionRealization],
) -> Result<(), Ymm4NativeExtensionError> {
    let mut by_key = BTreeMap::new();
    for realization in realizations {
        if by_key
            .insert(realization.logical_key.as_str(), realization)
            .is_some()
        {
            return Err(Ymm4NativeExtensionError::Readback(format!(
                "duplicate realization {}",
                realization.logical_key
            )));
        }
    }
    for operation in &plan.operations {
        let key = operation.intent.logical_key();
        if operation.action == NativeExtensionAction::Delete {
            if by_key.contains_key(key.as_str()) {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "deleted realization {key} is still present"
                )));
            }
            continue;
        }
        let realization = by_key.get(key.as_str()).ok_or_else(|| {
            Ymm4NativeExtensionError::Readback(format!("missing realization {key}"))
        })?;
        if realization.realization_id != operation.realization_id
            || realization.kind != expected_kind(&operation.intent)
            || realization.project_id != plan.target.project_id
        {
            return Err(Ymm4NativeExtensionError::Readback(format!(
                "realization identity/kind mismatch for {key}"
            )));
        }
        require_state_digest(&realization.state_digest)?;
        require_state_digest(&realization.owned_state_digest)?;
        let owned_digest = canonical_sha256(
            "takegraph-ymm4-native-extension-owned-state-v1",
            &realization.owned_fields,
        )?;
        if owned_digest != realization.owned_state_digest {
            return Err(Ymm4NativeExtensionError::Readback(format!(
                "owned-state digest mismatch for {key}"
            )));
        }
        verify_owned_fields(plan, operation, realization)?;
        let mut expected_effects = operation.preservation.unknown_effects.clone();
        expected_effects.sort_by(|left, right| {
            (&left.stable_type_id, &left.instance_key)
                .cmp(&(&right.stable_type_id, &right.instance_key))
        });
        let mut actual_effects = realization
            .unknown_effects
            .iter()
            .map(|effect| OpaqueNativeEffect {
                stable_type_id: effect.stable_type_id.clone(),
                instance_key: effect.instance_key.clone(),
                state_digest: effect.state_digest.clone(),
            })
            .collect::<Vec<_>>();
        actual_effects.sort_by(|left, right| {
            (&left.stable_type_id, &left.instance_key)
                .cmp(&(&right.stable_type_id, &right.instance_key))
        });
        if actual_effects != expected_effects {
            return Err(Ymm4NativeExtensionError::Readback(format!(
                "unknown native effects changed for {key}"
            )));
        }
        if operation.action == NativeExtensionAction::Update {
            let mut expected_fields = operation.preservation.preserved_fields.clone();
            expected_fields.sort_by(|left, right| left.field.cmp(&right.field));
            let mut actual_fields = realization
                .preserved_fields
                .iter()
                .map(|field| PreservedNativeField {
                    field: field.field.clone(),
                    state_digest: field.state_digest.clone(),
                })
                .collect::<Vec<_>>();
            actual_fields.sort_by(|left, right| left.field.cmp(&right.field));
            if actual_fields != expected_fields {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "preserved native fields changed for {key}"
                )));
            }
        }
        verify_placement(&operation.intent, realization)?;
    }
    if by_key.len()
        != plan
            .operations
            .iter()
            .filter(|operation| operation.action != NativeExtensionAction::Delete)
            .count()
    {
        return Err(Ymm4NativeExtensionError::Readback(
            "bridge returned an unplanned realization".into(),
        ));
    }
    Ok(())
}

fn verify_owned_fields(
    plan: &NativeExtensionPlan,
    operation: &takegraph_core::PlannedNativeExtension,
    realization: &Ymm4NativeExtensionRealization,
) -> Result<(), Ymm4NativeExtensionError> {
    let fields = &realization.owned_fields;
    let required = |name: &str| {
        fields.get(name).ok_or_else(|| {
            Ymm4NativeExtensionError::Readback(format!(
                "owned field {name} is missing for {}",
                realization.logical_key
            ))
        })
    };
    let (entity_id, revision) = match &operation.intent {
        NativeExtensionIntent::UpsertPortrait(value) => (&value.entity_id, value.entity_revision),
        NativeExtensionIntent::UpsertAsset(value) => (&value.entity_id, value.entity_revision),
        NativeExtensionIntent::MutateEffect(value) => {
            (&value.target_entity_id, value.target_entity_revision)
        }
        NativeExtensionIntent::InstantiateTemplate(value) => {
            (&value.entity_id, value.entity_revision)
        }
    };
    if realization.entity_id != *entity_id
        || realization.entity_revision != revision
        || required("logicalKey")? != &operation.intent.logical_key()
        || required("projectId")? != &plan.target.project_id
        || required("entityId")? != entity_id
        || required("entityRevision")? != &revision.to_string()
    {
        return Err(Ymm4NativeExtensionError::Readback(format!(
            "owned identity/revision mismatch for {}",
            realization.logical_key
        )));
    }
    match &operation.intent {
        NativeExtensionIntent::UpsertPortrait(value) => {
            if required("descriptorId")? != &value.character_binding.descriptor_id {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "character binding mismatch for {}",
                    realization.logical_key
                )));
            }
        }
        NativeExtensionIntent::UpsertAsset(value) => {
            if required("artifactDigest")? != &value.asset.artifact_digest
                || required("mediaType")? != &value.asset.media_type
                || required("byteLength")? != &value.asset.byte_length.to_string()
                || required("loopPlayback")? != if value.loop_playback { "true" } else { "false" }
            {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "immutable asset binding mismatch for {}",
                    realization.logical_key
                )));
            }
        }
        NativeExtensionIntent::MutateEffect(value) => {
            if required("descriptorId")? != &value.descriptor.descriptor_id
                || required("stableTypeId")?.trim().is_empty()
                || !matches!(
                    required("collection")?.as_str(),
                    "VideoEffects" | "AudioEffects"
                )
            {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "typed effect binding mismatch for {}",
                    realization.logical_key
                )));
            }
            let expected = match &value.operation {
                takegraph_core::EffectOperation::Upsert { parameters } => canonical_sha256(
                    "takegraph-ymm4-native-extension-effect-parameters-v1",
                    parameters,
                )?,
                takegraph_core::EffectOperation::Remove => canonical_sha256(
                    "takegraph-ymm4-native-extension-effect-parameters-v1",
                    &BTreeMap::<String, takegraph_core::EffectParameterValue>::new(),
                )?,
            };
            if required("parametersDigest")? != &expected {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "typed effect parameters mismatch for {}",
                    realization.logical_key
                )));
            }
        }
        NativeExtensionIntent::InstantiateTemplate(value) => {
            if required("descriptorId")? != &value.template.descriptor_id
                || required("partCount")?
                    .parse::<usize>()
                    .ok()
                    .is_none_or(|value| value == 0)
            {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "template binding/footprint mismatch for {}",
                    realization.logical_key
                )));
            }
            require_state_digest(required("footprintDigest")?)?;
        }
    }
    Ok(())
}

fn verify_observation(
    plan: &NativeExtensionPlan,
    observation: &NativeExtensionObservation,
) -> Result<(), Ymm4NativeExtensionError> {
    for operation in &plan.operations {
        let key = operation.intent.logical_key();
        let observed = observation.existing.get(&key);
        if operation.action == NativeExtensionAction::Delete {
            if observed.is_some() {
                return Err(Ymm4NativeExtensionError::Readback(format!(
                    "deleted realization {key} is present"
                )));
            }
            continue;
        }
        let observed = observed.ok_or_else(|| {
            Ymm4NativeExtensionError::Readback(format!("missing current realization {key}"))
        })?;
        if observed.realization_id != operation.realization_id
            || observed.kind != planner_expected_kind(&operation.intent)
        {
            return Err(Ymm4NativeExtensionError::Readback(format!(
                "current realization identity/kind mismatch for {key}"
            )));
        }
        let mut expected = operation.preservation.unknown_effects.clone();
        let mut actual = observed.unknown_effects.clone();
        expected.sort_by(|left, right| left.instance_key.cmp(&right.instance_key));
        actual.sort_by(|left, right| left.instance_key.cmp(&right.instance_key));
        if expected != actual {
            return Err(Ymm4NativeExtensionError::Readback(format!(
                "current unknown-effect state drifted for {key}"
            )));
        }
    }
    Ok(())
}

fn expected_kind(intent: &NativeExtensionIntent) -> Ymm4ExistingNativeExtensionKind {
    match intent {
        NativeExtensionIntent::UpsertPortrait(value) => match value.presentation {
            takegraph_core::PortraitPresentation::Portrait => {
                Ymm4ExistingNativeExtensionKind::Portrait
            }
            takegraph_core::PortraitPresentation::Face => Ymm4ExistingNativeExtensionKind::Face,
        },
        NativeExtensionIntent::UpsertAsset(value) => match value.asset.kind {
            AssetKind::Image => Ymm4ExistingNativeExtensionKind::Image,
            AssetKind::Video => Ymm4ExistingNativeExtensionKind::Video,
            AssetKind::Audio => Ymm4ExistingNativeExtensionKind::Audio,
            AssetKind::Bgm => Ymm4ExistingNativeExtensionKind::Bgm,
        },
        NativeExtensionIntent::MutateEffect(_) => Ymm4ExistingNativeExtensionKind::ManagedEffect,
        NativeExtensionIntent::InstantiateTemplate(_) => Ymm4ExistingNativeExtensionKind::Template,
    }
}

fn planner_expected_kind(intent: &NativeExtensionIntent) -> ExistingNativeExtensionKind {
    match expected_kind(intent) {
        Ymm4ExistingNativeExtensionKind::Portrait => ExistingNativeExtensionKind::Portrait,
        Ymm4ExistingNativeExtensionKind::Face => ExistingNativeExtensionKind::Face,
        Ymm4ExistingNativeExtensionKind::Image | Ymm4ExistingNativeExtensionKind::Template => {
            ExistingNativeExtensionKind::Image
        }
        Ymm4ExistingNativeExtensionKind::Video => ExistingNativeExtensionKind::Video,
        Ymm4ExistingNativeExtensionKind::Audio => ExistingNativeExtensionKind::Audio,
        Ymm4ExistingNativeExtensionKind::Bgm => ExistingNativeExtensionKind::Bgm,
        Ymm4ExistingNativeExtensionKind::ManagedEffect => {
            ExistingNativeExtensionKind::ManagedEffect
        }
    }
}

fn verify_placement(
    intent: &NativeExtensionIntent,
    realization: &Ymm4NativeExtensionRealization,
) -> Result<(), Ymm4NativeExtensionError> {
    let expected = match intent {
        NativeExtensionIntent::UpsertPortrait(value) => Some((
            value.placement.frame,
            value.placement.primary_layer,
            i32::try_from(value.duration_frames).unwrap_or(i32::MAX),
        )),
        NativeExtensionIntent::UpsertAsset(value) => Some((
            value.placement.frame,
            value.placement.primary_layer,
            i32::try_from(value.duration_frames).unwrap_or(i32::MAX),
        )),
        NativeExtensionIntent::InstantiateTemplate(value) => {
            Some((value.placement.frame, value.placement.primary_layer, 0))
        }
        NativeExtensionIntent::MutateEffect(_) => None,
    };
    if let Some((frame, layer, length)) = expected
        && (realization.frame != frame
            || realization.layer != layer
            || (length > 0 && realization.length != length))
    {
        return Err(Ymm4NativeExtensionError::Readback(format!(
            "placement mismatch for {}",
            realization.logical_key
        )));
    }
    Ok(())
}

fn require_state_digest(value: &str) -> Result<(), Ymm4NativeExtensionError> {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Ymm4NativeExtensionError::Readback(
            "realization state digest is not SHA-256".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum Ymm4NativeExtensionError {
    #[error(transparent)]
    Bridge(#[from] Ymm4Error),
    #[error(transparent)]
    Node(#[from] NativeExtensionNodeError),
    #[error(transparent)]
    Plan(#[from] NativeExtensionPlanError),
    #[error(transparent)]
    Patch(#[from] PatchError),
    #[error(transparent)]
    ProjectStore(#[from] crate::ProjectStoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
    #[error(transparent)]
    UnsavedProject(#[from] crate::UnsavedProjectError),
    #[error("invalid native-extension payload: {0}")]
    InvalidPayload(String),
    #[error("native-extension approval does not match the staged digest")]
    ApprovalDigestMismatch,
    #[error("native-extension payload digest mismatch: stored {stored}, actual {actual}")]
    PayloadDigestMismatch { stored: String, actual: String },
    #[error("native-extension capability contract is unavailable: {0}")]
    Capability(String),
    #[error("native-extension capabilities drifted: expected {expected}, got {actual}")]
    CapabilityDrift { expected: String, actual: String },
    #[error("native-extension target drift: {0}")]
    TargetDrift(String),
    #[error("missing source/materialization for artifact {0}")]
    MissingArtifact(String),
    #[error("native-extension bridge receipt is not verified and request-bound")]
    UnverifiedReceipt,
    #[error("native-extension semantic read-back failed: {0}")]
    Readback(String),
    #[error("native-extension verified receipt is missing")]
    MissingVerifiedReceipt,
    #[error("persisted native-extension receipt must be replayed through the bridge")]
    UntrustedReceipt,
    #[error("authenticated native-extension replay does not match the durable receipt digest")]
    DurableReceiptMismatch,
    #[error("native-extension reservation requires recovery: {0}")]
    PendingRecovery(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        DescriptorReference, PortraitIntent, PortraitPresentation, ReplacementGuard,
        ResolvedPlacement,
    };
    use takegraph_node::{
        Ymm4NativeExtensionObservation, Ymm4OpaqueNativeEffect, Ymm4PreservedNativeField,
    };

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn raw_digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    #[test]
    fn observation_preserves_unknown_effect_identity_and_loss_fields() {
        let key = "portrait:portrait-01".to_owned();
        let response = Ymm4NativeExtensionPlanResponse {
            fingerprint: "fingerprint".into(),
            descriptor_catalog_digest: raw_digest('a'),
            driver_profile_digest: raw_digest('b'),
            observation: Ymm4NativeExtensionObservation {
                existing: BTreeMap::from([(
                    key.clone(),
                    takegraph_node::Ymm4ExistingNativeExtension {
                        logical_key: key.clone(),
                        realization_id: Uuid::nil(),
                        kind: Ymm4ExistingNativeExtensionKind::Portrait,
                        update_mode: Ymm4ExistingUpdateMode::Replace {
                            lossy_fields: vec!["nativeAnimation.keyframes".into()],
                        },
                        preserved_fields: vec![Ymm4PreservedNativeField {
                            field: "remark".into(),
                            state_digest: digest('1'),
                        }],
                        unknown_effects: vec![Ymm4OpaqueNativeEffect {
                            stable_type_id: "user.glow".into(),
                            instance_key: "fx-1".into(),
                            state_digest: digest('2'),
                        }],
                    },
                )]),
            },
            warnings: Vec::new(),
        };
        let observed = planner_observation(&response).unwrap();
        let item = &observed.existing[&key];
        assert!(matches!(
            item.update_mode,
            ExistingUpdateMode::Replace { .. }
        ));
        assert_eq!(item.unknown_effects[0].instance_key, "fx-1");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn realization_readback_requires_unknown_effect_state_exactly() {
        let intent = NativeExtensionIntent::UpsertPortrait(PortraitIntent {
            entity_id: "portrait-01".into(),
            entity_revision: 1,
            presentation: PortraitPresentation::Portrait,
            character_binding: DescriptorReference {
                descriptor_id: "character.marisa".into(),
                expected_digest: digest('3'),
            },
            placement: ResolvedPlacement {
                frame: 10,
                primary_layer: 20,
                secondary_layer: None,
            },
            duration_frames: 30,
            replacement_guard: ReplacementGuard::default(),
        });
        let effect = OpaqueNativeEffect {
            stable_type_id: "user.glow".into(),
            instance_key: "fx-1".into(),
            state_digest: digest('4'),
        };
        let operation = takegraph_core::PlannedNativeExtension {
            realization_id: Uuid::nil(),
            action: NativeExtensionAction::Update,
            intent,
            capability_dependencies: vec![takegraph_core::CapabilityDependency {
                feature: "portraitItem.upsert".into(),
                minimum_version: 1,
                schema_digest: Some(digest('5')),
            }],
            descriptor_dependencies: Vec::new(),
            preservation: takegraph_core::PreservationPlan {
                mode: takegraph_core::NativeMutationMode::InPlace,
                preserved_fields: Vec::new(),
                unknown_effects: vec![effect.clone()],
                lossy_fields: Vec::new(),
                approved_lossy_fields: Vec::new(),
            },
        };
        let plan = NativeExtensionPlan {
            canonical_version: 1,
            operation_id: Uuid::nil(),
            base_revision: RevisionId(0),
            target: TargetIdentity {
                adapter_id: "ymm4".into(),
                project_id: "project".into(),
                scene_id: "scene".into(),
                fps: 30,
                driver_version: "driver".into(),
            },
            capability_digest: digest('6'),
            descriptor_catalog_digest: digest('7'),
            expected_scope: ScopeFingerprints {
                target_identity_digest: digest('8'),
                managed_state_digest: digest('9'),
                conflict_scope_digest: digest('a'),
            },
            change_budget: ChangeBudget::create_only(1),
            operations: vec![operation],
            warnings: Vec::new(),
        };
        let owned_fields = BTreeMap::from([
            ("logicalKey".into(), "portrait:portrait-01".into()),
            ("projectId".into(), "project".into()),
            ("entityId".into(), "portrait-01".into()),
            ("entityRevision".into(), "1".into()),
            ("kind".into(), "portrait".into()),
            ("frame".into(), "10".into()),
            ("layer".into(), "20".into()),
            ("length".into(), "30".into()),
            ("descriptorId".into(), "character.marisa".into()),
        ]);
        let mut readback = Ymm4NativeExtensionRealization {
            logical_key: "portrait:portrait-01".into(),
            realization_id: Uuid::nil(),
            kind: Ymm4ExistingNativeExtensionKind::Portrait,
            project_id: "project".into(),
            entity_id: "portrait-01".into(),
            entity_revision: 1,
            frame: 10,
            layer: 20,
            length: 30,
            owned_state_digest: canonical_sha256(
                "takegraph-ymm4-native-extension-owned-state-v1",
                &owned_fields,
            )
            .unwrap(),
            owned_fields,
            preserved_fields: Vec::new(),
            state_digest: digest('b'),
            unknown_effects: vec![takegraph_node::Ymm4OpaqueNativeEffect {
                stable_type_id: effect.stable_type_id,
                instance_key: effect.instance_key,
                state_digest: effect.state_digest,
            }],
        };
        verify_realizations(&plan, std::slice::from_ref(&readback)).unwrap();
        readback
            .owned_fields
            .insert("descriptorId".into(), "character.tampered".into());
        readback.owned_state_digest = canonical_sha256(
            "takegraph-ymm4-native-extension-owned-state-v1",
            &readback.owned_fields,
        )
        .unwrap();
        assert!(matches!(
            verify_realizations(&plan, std::slice::from_ref(&readback)),
            Err(Ymm4NativeExtensionError::Readback(_))
        ));
        readback
            .owned_fields
            .insert("descriptorId".into(), "character.marisa".into());
        readback.owned_state_digest = canonical_sha256(
            "takegraph-ymm4-native-extension-owned-state-v1",
            &readback.owned_fields,
        )
        .unwrap();
        readback.unknown_effects[0].state_digest = digest('c');
        assert!(matches!(
            verify_realizations(&plan, &[readback]),
            Err(Ymm4NativeExtensionError::Readback(_))
        ));
    }
}
