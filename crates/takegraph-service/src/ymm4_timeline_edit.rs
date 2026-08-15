//! One approval and one canonical commit for an ordered mixed voice edit.

use serde::{Deserialize, Serialize};
use takegraph_core::{
    CapabilityDependency, ChangeBudget, Patch, PatchError, PatchStatus, PlanWarning, RevisionId,
    SourceEvidenceRef, TARGET_PLAN_CANONICAL_VERSION, TIMELINE_EDIT_MAX_OPERATIONS,
    TIMELINE_EDIT_PLAN_CANONICAL_VERSION, TargetPlan, TimelineEditOperation, TimelineEditPlan,
    approval_digests_match, canonical_sha256,
};
use takegraph_node::{
    CapabilityRequirement, ManagedUtterance, StructuredYmm4Capabilities, Ymm4BridgeClient,
    Ymm4Error, Ymm4ManagedItem, Ymm4NativeVoiceCue, Ymm4OperationStatus, Ymm4ProjectSnapshot,
    Ymm4TimelineEditApplyRequest, Ymm4TimelineEditReceipt, Ymm4TimelineEditValidation,
    Ymm4TimelineEditValidationRequest,
};
use thiserror::Error;
use uuid::Uuid;

/// Caller-ordered, target-independent materialized inputs for a timeline edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineEditStageManifest {
    pub operations: Vec<TimelineEditStageOperation>,
    #[serde(default)]
    pub max_changed_entities: Option<usize>,
}

/// MVP timeline operations. Native extensions remain absent until the bridge
/// can include their preservation proof in the same WAL and aggregate receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineEditStageOperation {
    PortableVoiceCreate {
        utterance: ManagedUtterance,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_evidence: Option<SourceEvidenceRef>,
    },
    NativeVoiceCreate {
        cue: Ymm4NativeVoiceCue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_evidence: Option<SourceEvidenceRef>,
    },
}

impl TimelineEditStageManifest {
    fn change_budget(&self) -> Result<ChangeBudget, Ymm4TimelineEditError> {
        if self.operations.is_empty() {
            return Err(Ymm4TimelineEditError::InvalidManifest(
                "at least one operation is required".into(),
            ));
        }
        if self.operations.len() > TIMELINE_EDIT_MAX_OPERATIONS {
            return Err(Ymm4TimelineEditError::InvalidManifest(format!(
                "at most {TIMELINE_EDIT_MAX_OPERATIONS} operations are allowed"
            )));
        }
        let maximum = self.max_changed_entities.unwrap_or(self.operations.len());
        if maximum < self.operations.len() || maximum > TIMELINE_EDIT_MAX_OPERATIONS {
            return Err(Ymm4TimelineEditError::InvalidManifest(format!(
                "maxChangedEntities must be between {} and {TIMELINE_EDIT_MAX_OPERATIONS}",
                self.operations.len()
            )));
        }
        Ok(ChangeBudget::create_only(maximum))
    }
}

fn build_timeline_edit_plan(
    base_revision: RevisionId,
    operation_id: Uuid,
    target: &Ymm4ProjectSnapshot,
    manifest: &TimelineEditStageManifest,
    capabilities: &StructuredYmm4Capabilities,
) -> Result<TimelineEditPlan, Ymm4TimelineEditError> {
    let change_budget = manifest.change_budget()?;
    let timeline_feature = capabilities
        .feature("timelineEdit.apply")
        .filter(|feature| feature.available)
        .ok_or_else(|| {
            Ymm4TimelineEditError::Unsupported(
                "the active bridge does not advertise timelineEdit.apply".into(),
            )
        })?;
    let timeline_dependency = CapabilityDependency {
        feature: "timelineEdit.apply".into(),
        minimum_version: timeline_feature.version,
        schema_digest: Some(timeline_feature.schema_digest.clone()),
    };

    let mut operations = Vec::with_capacity(manifest.operations.len());
    let mut source_evidence = Vec::new();
    let mut target_identity = None;
    let mut expected_scope = None;
    let mut warnings = Vec::new();
    for input in &manifest.operations {
        let mut child = match input {
            TimelineEditStageOperation::PortableVoiceCreate {
                utterance,
                source_evidence: evidence,
            } => {
                if let Some(evidence) = evidence {
                    source_evidence.push(evidence.clone());
                }
                crate::portable_pair_target_plan(
                    base_revision,
                    operation_id,
                    target,
                    std::slice::from_ref(utterance),
                    capabilities,
                )?
            }
            TimelineEditStageOperation::NativeVoiceCreate {
                cue,
                source_evidence: evidence,
            } => {
                if let Some(evidence) = evidence {
                    source_evidence.push(evidence.clone());
                }
                crate::native_voice_target_plan(
                    base_revision,
                    operation_id,
                    target,
                    std::slice::from_ref(cue),
                    capabilities,
                )?
            }
        };
        if target_identity.is_none() {
            target_identity = Some(child.target.clone());
            expected_scope = Some(child.expected_scope.clone());
        }
        let mut cue = child
            .cues
            .pop()
            .ok_or_else(|| Ymm4TimelineEditError::InvalidPlan("child plan has no cue".into()))?;
        cue.capability_dependencies
            .push(timeline_dependency.clone());
        warnings.extend(child.warnings);
        operations.push(TimelineEditOperation::ManagedCue { cue: Box::new(cue) });
    }
    warnings.push(PlanWarning {
        code: "atomic_timeline_edit".into(),
        message: format!(
            "{} ordered operation(s) will commit or roll back as one YMM4 transaction",
            operations.len()
        ),
    });
    let plan = TimelineEditPlan {
        canonical_version: TIMELINE_EDIT_PLAN_CANONICAL_VERSION,
        operation_id,
        base_revision,
        target: target_identity
            .ok_or_else(|| Ymm4TimelineEditError::InvalidPlan("target is missing".into()))?,
        capability_digest: capabilities.capability_digest.clone(),
        expected_scope: expected_scope
            .ok_or_else(|| Ymm4TimelineEditError::InvalidPlan("scope is missing".into()))?,
        change_budget,
        operations,
        warnings,
        source_evidence,
    };
    plan.validate()
        .map_err(|error| Ymm4TimelineEditError::InvalidPlan(error.to_string()))?;
    Ok(plan)
}

/// Persisted task envelope. Apply-specific bridge fields are added only after
/// the read-only validate contract accepts the complete plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TimelineEditTask {
    pub patch: Patch,
    pub operation_id: Uuid,
    pub target: Ymm4ProjectSnapshot,
    pub timeline_edit_plan: TimelineEditPlan,
    pub bridge_validation: Ymm4TimelineEditValidation,
    pub receipt: Option<Ymm4TimelineEditReceipt>,
    #[serde(skip)]
    receipt_trusted: bool,
}

impl Ymm4TimelineEditTask {
    /// Builds and read-only validates the complete mixed plan before preview.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsaved/stale target, unsupported capability,
    /// invalid manifest/plan, bridge validation failure, or patch failure.
    pub async fn stage_from_snapshot(
        client: &Ymm4BridgeClient,
        base_revision: RevisionId,
        target: Ymm4ProjectSnapshot,
        manifest: TimelineEditStageManifest,
    ) -> Result<Self, Ymm4TimelineEditError> {
        crate::require_existing_project_path(&target.project_path)?;
        let capabilities = client.structured_capabilities().await?;
        let operation_id = Uuid::new_v4();
        let timeline_edit_plan = build_timeline_edit_plan(
            base_revision,
            operation_id,
            &target,
            &manifest,
            &capabilities,
        )?;
        let request = Ymm4TimelineEditValidationRequest::new(
            timeline_edit_plan.clone(),
            target.fingerprint.clone(),
        )?;
        let bridge_validation = client.validate_timeline_edit(&request).await?;
        validate_bridge_validation(&timeline_edit_plan, &target, &bridge_validation)?;
        let mut task = Self {
            patch: Patch::draft(base_revision, "pending"),
            operation_id,
            target,
            timeline_edit_plan,
            bridge_validation,
            receipt: None,
            receipt_trusted: false,
        };
        task.patch.digest = task.payload_digest()?;
        task.patch.validate()?;
        task.patch.materialize_preview()?;
        Ok(task)
    }

    /// Loads persisted state and rejects approval-bound field tampering.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON or any invalid/tampered task field.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, Ymm4TimelineEditError> {
        let mut task: Self = serde_json::from_slice(bytes)?;
        task.receipt_trusted = false;
        task.validate_payload()?;
        Ok(task)
    }

    #[must_use]
    pub fn receipt(&self) -> Option<&Ymm4TimelineEditReceipt> {
        self.receipt.as_ref()
    }

    /// Approves one exact aggregate digest at one canonical base revision.
    ///
    /// # Errors
    ///
    /// Returns an error for task tampering, digest mismatch, stale revision,
    /// or an invalid patch lifecycle state.
    pub fn approve(
        &mut self,
        approved_digest: &str,
        current_head: RevisionId,
    ) -> Result<(), Ymm4TimelineEditError> {
        self.validate_payload()?;
        if self.patch.base != current_head {
            return Err(PatchError::StaleBase {
                expected: self.patch.base,
                actual: current_head,
            }
            .into());
        }
        if !approval_digests_match(&self.patch.digest, approved_digest) {
            return Err(Ymm4TimelineEditError::ApprovalDigestMismatch);
        }
        if self.patch.status == PatchStatus::Previewable {
            self.patch.approve()?;
        }
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        Ok(())
    }

    /// Executes one request-bound bridge transaction and publishes one revision.
    ///
    /// # Errors
    ///
    /// Returns an error for stale state, dependency drift, bridge failure,
    /// untrusted read-back, recovery-required state, or durable commit failure.
    #[allow(clippy::too_many_lines)]
    pub async fn apply_and_finalize_durable(
        &mut self,
        client: &Ymm4BridgeClient,
        store: &crate::DurableProjectStore,
        current_head: RevisionId,
    ) -> Result<crate::DurableExternalMutationOutcome, Ymm4TimelineEditError> {
        self.validate_payload()?;
        crate::require_existing_project_path(&self.target.project_path)?;
        crate::external_mutation::authorize_external_patch(&self.patch)?;
        let request = self.apply_request()?;
        let target = crate::VerifiedTargetBinding {
            adapter_id: self.timeline_edit_plan.target.adapter_id.clone(),
            target_project_id: self.target.project_id.clone(),
            scene_id: self.target.scene_id.clone(),
            target_identity_digest: self
                .timeline_edit_plan
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
            self.validate_live_dependencies(client).await?;
        }
        store.reserve_external_commit(
            self.operation_id,
            self.patch.base,
            &self.patch.digest,
            &request.request_digest,
            &target,
        )?;

        if let Some(committed) = scope.committed.as_ref() {
            let response = client.apply_timeline_edit(&request).await?;
            self.receipt = Some(response.receipt);
            if !response.success || !response.replayed {
                return Err(Ymm4TimelineEditError::ApplyFailed(
                    "a committed timeline edit did not return an exact bridge replay".into(),
                ));
            }
            self.validate_verified_receipt(&request)?;
            let receipt = self
                .receipt
                .as_ref()
                .ok_or(Ymm4TimelineEditError::MissingVerifiedReceipt)?;
            if canonical_sha256("takegraph-external-receipt", receipt)? != committed.receipt_digest
            {
                return Err(Ymm4TimelineEditError::DurableReceiptMismatch);
            }
            self.receipt_trusted = true;
            let revision = self.finalize_durable(store)?;
            return Ok(crate::DurableExternalMutationOutcome {
                revision,
                canonical_replay: true,
            });
        }

        match client.apply_timeline_edit(&request).await {
            Ok(response) => {
                self.receipt = Some(response.receipt);
                if !response.success {
                    self.abort_if_safe(store, &request, &target)?;
                    return Err(Ymm4TimelineEditError::ApplyFailed(
                        self.receipt
                            .as_ref()
                            .and_then(|receipt| receipt.error.clone())
                            .unwrap_or_else(|| "timeline edit was not verified".into()),
                    ));
                }
                self.validate_verified_receipt(&request)?;
            }
            Err(apply_error) => {
                // A timed-out apply may still be queued. The exact not-started
                // route shares the bridge apply gate and therefore gives the
                // only safe absence proof (or returns the completed receipt).
                match client.seal_timeline_edit_not_started(&request).await {
                    Ok(sealed) => {
                        self.receipt = Some(sealed.receipt);
                        if sealed.success {
                            self.validate_verified_receipt(&request)?;
                        } else {
                            self.abort_if_safe(store, &request, &target)?;
                            return Err(Ymm4TimelineEditError::ApplyFailed(
                                self.receipt
                                    .as_ref()
                                    .and_then(|receipt| receipt.error.clone())
                                    .unwrap_or_else(|| apply_error.to_string()),
                            ));
                        }
                    }
                    Err(_) => return Err(apply_error.into()),
                }
            }
        }
        self.receipt_trusted = true;
        Ok(crate::DurableExternalMutationOutcome {
            revision: self.finalize_durable(store)?,
            canonical_replay: false,
        })
    }

    /// Verifies the currently open target against every approved managed cue.
    ///
    /// # Errors
    ///
    /// Returns an error for task tampering, bridge failure, target mismatch,
    /// or semantic read-back mismatch.
    pub async fn verify_current(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4TimelineEditError> {
        self.validate_payload()?;
        let snapshot = client.snapshot().await?;
        if snapshot.project_id != self.target.project_id
            || snapshot.scene_id != self.target.scene_id
        {
            return Err(Ymm4TimelineEditError::VerifyMismatch(
                "the active project or scene differs from the staged target".into(),
            ));
        }
        self.verify_current_items(&snapshot.managed_items)
    }

    fn apply_request(&self) -> Result<Ymm4TimelineEditApplyRequest, Ymm4TimelineEditError> {
        Ok(Ymm4TimelineEditApplyRequest::new(
            self.timeline_edit_plan.clone(),
            self.target.fingerprint.clone(),
        )?)
    }

    async fn validate_live_dependencies(
        &self,
        client: &Ymm4BridgeClient,
    ) -> Result<(), Ymm4TimelineEditError> {
        let actual = client.structured_capabilities().await?;
        if actual.capability_digest != self.timeline_edit_plan.capability_digest {
            return Err(Ymm4TimelineEditError::CapabilityDrift {
                expected: self.timeline_edit_plan.capability_digest.clone(),
                actual: actual.capability_digest,
            });
        }
        let requirements = self
            .timeline_edit_plan
            .operations
            .iter()
            .flat_map(|operation| match operation {
                TimelineEditOperation::ManagedCue { cue } => cue.capability_dependencies.as_slice(),
                TimelineEditOperation::NativeExtension { operation, .. } => {
                    operation.capability_dependencies.as_slice()
                }
            })
            .map(|dependency| CapabilityRequirement {
                feature: dependency.feature.clone(),
                minimum_version: dependency.minimum_version,
                schema_digest: dependency.schema_digest.clone(),
            })
            .collect::<Vec<_>>();
        actual.require(&requirements).map_err(|error| {
            Ymm4TimelineEditError::InvalidPlan(format!("capability requirement failed: {error}"))
        })?;
        let validation = client
            .validate_timeline_edit(&Ymm4TimelineEditValidationRequest::new(
                self.timeline_edit_plan.clone(),
                self.target.fingerprint.clone(),
            )?)
            .await?;
        validate_bridge_validation(&self.timeline_edit_plan, &self.target, &validation)
    }

    fn validate_verified_receipt(
        &self,
        request: &Ymm4TimelineEditApplyRequest,
    ) -> Result<&Ymm4TimelineEditReceipt, Ymm4TimelineEditError> {
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(Ymm4TimelineEditError::MissingVerifiedReceipt)?;
        if receipt.operation_id != self.operation_id
            || receipt.request_digest != request.request_digest
            || receipt.project_id != self.target.project_id
            || receipt.scene_id != self.target.scene_id
            || receipt.expected_fingerprint != self.target.fingerprint
            || receipt.before_fingerprint != self.target.fingerprint
            || receipt.plan_digest != request.plan_digest
            || receipt.status != Ymm4OperationStatus::Verified
            || !receipt.verified
            || receipt.after_fingerprint.trim().is_empty()
            || receipt.applied_operation_count != self.timeline_edit_plan.operations.len()
            || !receipt.applied_native_extensions.is_empty()
            || receipt.error.is_some()
        {
            return Err(Ymm4TimelineEditError::ReceiptBindingMismatch);
        }
        self.verify_items(&receipt.applied_items)?;
        Ok(receipt)
    }

    fn verify_items(&self, items: &[Ymm4ManagedItem]) -> Result<(), Ymm4TimelineEditError> {
        let cues = self
            .timeline_edit_plan
            .operations
            .iter()
            .map(|operation| match operation {
                TimelineEditOperation::ManagedCue { cue } => Ok(cue.as_ref().clone()),
                TimelineEditOperation::NativeExtension { .. } => {
                    Err(Ymm4TimelineEditError::Unsupported(
                        "native extensions are not enabled in aggregate receipts".into(),
                    ))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected_physical = cues
            .iter()
            .map(|cue| match cue.strategy {
                takegraph_core::RealizationStrategy::PortableAudioCaption => 2,
                takegraph_core::RealizationStrategy::Ymm4NativeVoice => 1,
            })
            .sum::<usize>();
        if items.len() != expected_physical {
            return Err(Ymm4TimelineEditError::VerifyMismatch(format!(
                "read-back contains {} physical items, expected {expected_physical}",
                items.len()
            )));
        }
        let target_plan = TargetPlan {
            canonical_version: TARGET_PLAN_CANONICAL_VERSION,
            operation_id: self.operation_id,
            base_revision: self.patch.base,
            target: self.timeline_edit_plan.target.clone(),
            capability_digest: self.timeline_edit_plan.capability_digest.clone(),
            expected_scope: self.timeline_edit_plan.expected_scope.clone(),
            change_budget: self.timeline_edit_plan.change_budget.clone(),
            cues,
            warnings: self.timeline_edit_plan.warnings.clone(),
        };
        crate::normalize_realizations(&target_plan, items)
            .map(|_| ())
            .map_err(|error| Ymm4TimelineEditError::VerifyMismatch(error.to_string()))
    }

    fn verify_current_items(&self, items: &[Ymm4ManagedItem]) -> Result<(), Ymm4TimelineEditError> {
        let relevant = items
            .iter()
            .filter(|item| {
                self.timeline_edit_plan.operations.iter().any(|operation| {
                    let TimelineEditOperation::ManagedCue { cue } = operation else {
                        return false;
                    };
                    match cue.strategy {
                        takegraph_core::RealizationStrategy::PortableAudioCaption => {
                            item.entity_id == cue.intent.entity_id
                        }
                        takegraph_core::RealizationStrategy::Ymm4NativeVoice => {
                            item.realization_id == Some(cue.realization_id)
                        }
                    }
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        self.verify_items(&relevant)
    }

    fn abort_if_safe(
        &self,
        store: &crate::DurableProjectStore,
        request: &Ymm4TimelineEditApplyRequest,
        target: &crate::VerifiedTargetBinding,
    ) -> Result<(), Ymm4TimelineEditError> {
        if self.receipt.as_ref().is_some_and(|receipt| {
            timeline_receipt_proves_safe_abort(receipt, request, &self.target)
        }) {
            store.abort_external_commit_reservation(
                self.operation_id,
                self.patch.base,
                &self.patch.digest,
                &request.request_digest,
                target,
            )?;
        }
        Ok(())
    }

    fn finalize_durable(
        &mut self,
        store: &crate::DurableProjectStore,
    ) -> Result<RevisionId, Ymm4TimelineEditError> {
        self.validate_payload()?;
        if !self.receipt_trusted {
            return Err(Ymm4TimelineEditError::UntrustedReceipt);
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
        let request = self.apply_request()?;
        let proof = {
            let receipt = self.validate_verified_receipt(&request)?;
            let entity_ids = self
                .timeline_edit_plan
                .operations
                .iter()
                .filter_map(|operation| match operation {
                    TimelineEditOperation::ManagedCue { cue } => Some(cue.intent.entity_id.clone()),
                    TimelineEditOperation::NativeExtension { .. } => None,
                });
            let update = crate::project_store::VerifiedManagedStateUpdate::
                replacing_entities_from_timeline_receipt(entity_ids, receipt)?;
            crate::project_store::VerifiedExternalCommit::from_receipt(
                self.operation_id,
                self.patch.base,
                self.patch.digest.clone(),
                receipt.request_digest.clone(),
                receipt,
                crate::VerifiedTargetBinding {
                    adapter_id: self.timeline_edit_plan.target.adapter_id.clone(),
                    target_project_id: self.target.project_id.clone(),
                    scene_id: self.target.scene_id.clone(),
                    target_identity_digest: self
                        .timeline_edit_plan
                        .expected_scope
                        .target_identity_digest
                        .clone(),
                    verified_fingerprint: receipt.after_fingerprint.clone(),
                },
            )?
            .with_managed_state_update(update)?
        };
        let revision = store.commit_verified_external(&proof)?;
        let expected = self
            .patch
            .base
            .checked_next()
            .ok_or(PatchError::RevisionOverflow)?;
        if revision != expected {
            return Err(Ymm4TimelineEditError::InvalidPlan(format!(
                "durable revision {revision:?} differs from expected {expected:?}"
            )));
        }
        if self.patch.status == PatchStatus::Approved {
            let core_revision = self.patch.commit(self.patch.base)?;
            debug_assert_eq!(core_revision, revision);
        }
        Ok(revision)
    }

    fn validate_payload(&self) -> Result<(), Ymm4TimelineEditError> {
        self.timeline_edit_plan
            .validate()
            .map_err(|error| Ymm4TimelineEditError::InvalidPlan(error.to_string()))?;
        if self.operation_id != self.timeline_edit_plan.operation_id
            || self.patch.base != self.timeline_edit_plan.base_revision
            || self.target.project_id != self.timeline_edit_plan.target.project_id
            || self.target.scene_id != self.timeline_edit_plan.target.scene_id
        {
            return Err(Ymm4TimelineEditError::InvalidPlan(
                "task, plan, operation, revision, and target are not bound".into(),
            ));
        }
        validate_bridge_validation(
            &self.timeline_edit_plan,
            &self.target,
            &self.bridge_validation,
        )?;
        let actual = self.payload_digest()?;
        if actual != self.patch.digest {
            return Err(Ymm4TimelineEditError::PayloadDigestMismatch {
                stored: self.patch.digest.clone(),
                actual,
            });
        }
        Ok(())
    }

    fn payload_digest(&self) -> Result<String, Ymm4TimelineEditError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload<'a> {
            operation_id: Uuid,
            base_revision: RevisionId,
            target: &'a Ymm4ProjectSnapshot,
            timeline_edit_plan: &'a TimelineEditPlan,
            bridge_validation: &'a Ymm4TimelineEditValidation,
        }
        Ok(canonical_sha256(
            "takegraph-ymm4-timeline-edit-task-v1",
            &Payload {
                operation_id: self.operation_id,
                base_revision: self.patch.base,
                target: &self.target,
                timeline_edit_plan: &self.timeline_edit_plan,
                bridge_validation: &self.bridge_validation,
            },
        )?)
    }
}

fn validate_bridge_validation(
    plan: &TimelineEditPlan,
    target: &Ymm4ProjectSnapshot,
    validation: &Ymm4TimelineEditValidation,
) -> Result<(), Ymm4TimelineEditError> {
    let plan_digest = plan
        .canonical_digest()
        .map_err(|error| Ymm4TimelineEditError::InvalidPlan(error.to_string()))?;
    let mut portable = 0;
    let mut native = 0;
    let mut create = 0;
    let mut update = 0;
    let mut delete = 0;
    for operation in &plan.operations {
        let TimelineEditOperation::ManagedCue { cue } = operation else {
            return Err(Ymm4TimelineEditError::Unsupported(
                "native extensions are not enabled by this bridge profile".into(),
            ));
        };
        match cue.strategy {
            takegraph_core::RealizationStrategy::PortableAudioCaption => portable += 1,
            takegraph_core::RealizationStrategy::Ymm4NativeVoice => native += 1,
        }
        match cue.action {
            takegraph_core::PlannedAction::Create => create += 1,
            takegraph_core::PlannedAction::Update => update += 1,
            takegraph_core::PlannedAction::Delete => delete += 1,
        }
    }
    if validation.operation_id != plan.operation_id
        || validation.plan_digest != plan_digest
        || validation.fingerprint != target.fingerprint
        || validation.strategy_counts.len() != usize::from(portable > 0) + usize::from(native > 0)
        || validation
            .strategy_counts
            .get("portable_pair")
            .copied()
            .unwrap_or_default()
            != portable
        || validation
            .strategy_counts
            .get("native_voice")
            .copied()
            .unwrap_or_default()
            != native
        || validation.create_count != create
        || validation.update_count != update
        || validation.delete_count != delete
        || validation.physical_item_count != portable * 2 + native
    {
        return Err(Ymm4TimelineEditError::InvalidPlan(
            "bridge validation is not bound to the complete ordered plan".into(),
        ));
    }
    Ok(())
}

fn timeline_receipt_proves_safe_abort(
    receipt: &Ymm4TimelineEditReceipt,
    request: &Ymm4TimelineEditApplyRequest,
    target: &Ymm4ProjectSnapshot,
) -> bool {
    let bound = receipt.operation_id == request.timeline_edit_plan.operation_id
        && receipt.request_digest == request.request_digest
        && receipt.project_id == target.project_id
        && receipt.scene_id == target.scene_id
        && receipt.expected_fingerprint == request.expected_fingerprint
        && receipt.plan_digest == request.plan_digest
        && receipt.before_fingerprint == request.expected_fingerprint
        && receipt.after_fingerprint == receipt.before_fingerprint
        && !receipt.verified
        && receipt.applied_native_extensions.is_empty()
        && receipt.applied_operation_count == 0
        && receipt
            .error
            .as_ref()
            .is_some_and(|error| !error.trim().is_empty());
    if !bound {
        return false;
    }
    match receipt.status {
        Ymm4OperationStatus::RolledBack => true,
        Ymm4OperationStatus::NotStarted | Ymm4OperationStatus::Failed => {
            receipt.applied_items.is_empty()
        }
        _ => false,
    }
}

#[derive(Debug, Error)]
pub enum Ymm4TimelineEditError {
    #[error("invalid timeline-edit manifest: {0}")]
    InvalidManifest(String),
    #[error("timeline edit is unsupported by this bridge: {0}")]
    Unsupported(String),
    #[error("invalid timeline-edit plan: {0}")]
    InvalidPlan(String),
    #[error("timeline-edit approval digest does not match the staged payload")]
    ApprovalDigestMismatch,
    #[error("timeline-edit payload digest mismatch: stored {stored}, actual {actual}")]
    PayloadDigestMismatch { stored: String, actual: String },
    #[error("timeline-edit capability digest changed: expected {expected}, actual {actual}")]
    CapabilityDrift { expected: String, actual: String },
    #[error("timeline-edit apply failed: {0}")]
    ApplyFailed(String),
    #[error("timeline-edit receipt does not exactly bind the approved request")]
    ReceiptBindingMismatch,
    #[error("timeline-edit has no verified receipt")]
    MissingVerifiedReceipt,
    #[error("persisted receipt has not been authenticated by bridge replay")]
    UntrustedReceipt,
    #[error("durable receipt differs from authenticated bridge replay")]
    DurableReceiptMismatch,
    #[error("timeline-edit read-back mismatch: {0}")]
    VerifyMismatch(String),
    #[error(transparent)]
    TargetPlan(#[from] crate::TargetPlanBuildError),
    #[error(transparent)]
    Timeline(#[from] takegraph_core::TimelineEditError),
    #[error(transparent)]
    Bridge(#[from] Ymm4Error),
    #[error(transparent)]
    Patch(#[from] PatchError),
    #[error(transparent)]
    Store(#[from] crate::ProjectStoreError),
    #[error(transparent)]
    Unsaved(#[from] crate::UnsavedProjectError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };
    use takegraph_node::{Ymm4Capabilities, Ymm4Capability, Ymm4Health};

    const BASE_FINGERPRINT: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const AFTER_FINGERPRINT: &str =
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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
                    Ymm4Capability::ManagedAudio,
                    Ymm4Capability::ManagedCaption,
                    Ymm4Capability::UnifiedTargetPlan,
                    Ymm4Capability::TimelineEditManagedCueMixed,
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

    fn target() -> Ymm4ProjectSnapshot {
        Ymm4ProjectSnapshot {
            project_id: "project".into(),
            project_name: "project".into(),
            project_path: "project.ymmp".into(),
            scene_id: "scene".into(),
            fps: 60,
            fingerprint: BASE_FINGERPRINT.into(),
            managed_items: vec![],
            native_extensions: vec![],
            unmanaged_context_count: 0,
        }
    }

    fn portable(entity: &str) -> TimelineEditStageOperation {
        TimelineEditStageOperation::PortableVoiceCreate {
            source_evidence: None,
            utterance: ManagedUtterance {
                entity_id: entity.into(),
                revision: 1,
                speaker: "speaker".into(),
                caption: "caption".into(),
                spoken_text: "spoken".into(),
                audio_path: "audio.wav".into(),
                artifact_hash: "b".repeat(64),
                frame: 10,
                length: 30,
                audio_layer: 1,
                caption_layer: 2,
            },
        }
    }

    fn native(entity: &str) -> TimelineEditStageOperation {
        TimelineEditStageOperation::NativeVoiceCreate {
            source_evidence: None,
            cue: Ymm4NativeVoiceCue {
                realization_id: Uuid::from_u128(22),
                entity_id: entity.into(),
                revision: 1,
                character_name: "speaker".into(),
                display_text: "caption".into(),
                spoken_text: Some("caption".into()),
                frame: 50,
                layer: 3,
                max_length: 60,
            },
        }
    }

    #[test]
    fn builder_preserves_mixed_caller_order() {
        let manifest = TimelineEditStageManifest {
            operations: vec![native("native"), portable("portable")],
            max_changed_entities: None,
        };
        let plan = build_timeline_edit_plan(
            RevisionId(7),
            Uuid::from_u128(1),
            &target(),
            &manifest,
            &capabilities(),
        )
        .unwrap();
        assert_eq!(plan.operations.len(), 2);
        assert_eq!(plan.operations[0].write_identity(), "entity:native");
        assert_eq!(plan.operations[1].write_identity(), "entity:portable");
        assert!(plan.operations.iter().all(|operation| {
            match operation {
                TimelineEditOperation::ManagedCue { cue } => cue
                    .capability_dependencies
                    .iter()
                    .any(|dependency| dependency.feature == "timelineEdit.apply"),
                TimelineEditOperation::NativeExtension { .. } => false,
            }
        }));
    }

    #[test]
    fn manifest_budget_is_bounded_and_not_smaller_than_batch() {
        let manifest = TimelineEditStageManifest {
            operations: vec![portable("a"), portable("b")],
            max_changed_entities: Some(1),
        };
        assert!(matches!(
            manifest.change_budget(),
            Err(Ymm4TimelineEditError::InvalidManifest(_))
        ));
    }

    #[tokio::test]
    async fn approval_rejects_a_stale_canonical_head() {
        let bridge = MockBridge::start(ReceiptMode::Verified, false);
        let client = Ymm4BridgeClient::new(&bridge.endpoint, "test-token").unwrap();
        let mut task = Ymm4TimelineEditTask::stage_from_snapshot(
            &client,
            RevisionId(0),
            target(),
            mixed_manifest(),
        )
        .await
        .unwrap();
        let digest = task.patch.digest.clone();

        assert!(matches!(
            task.approve(&digest, RevisionId(1)),
            Err(Ymm4TimelineEditError::Patch(PatchError::StaleBase { .. }))
        ));
        assert_eq!(task.patch.status, PatchStatus::Previewable);
    }

    #[tokio::test]
    async fn current_verification_ignores_other_managed_entities() {
        let bridge = MockBridge::start(ReceiptMode::Verified, false);
        let (task, _store, root) = staged_task(&bridge).await;
        let mut items = correct_items()
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<Ymm4ManagedItem>, _>>()
            .unwrap();
        let mut unrelated = items[0].clone();
        unrelated.entity_id = "unrelated".into();
        unrelated.realization_id = Some(Uuid::from_u128(999));
        items.push(unrelated);

        assert!(task.verify_current_items(&items).is_ok());
        assert!(task.verify_items(&items).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[derive(Debug, Clone, Copy)]
    enum ReceiptMode {
        Verified,
        RolledBack,
        RecoveryRequired,
        WrongPlanDigest,
        WrongOperationCount,
        WrongItem,
    }

    struct MockState {
        mode: ReceiptMode,
        extra_strategy: bool,
        apply_calls: usize,
        receipt: Option<serde_json::Value>,
    }

    struct MockBridge {
        endpoint: String,
        state: Arc<Mutex<MockState>>,
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl MockBridge {
        fn start(mode: ReceiptMode, extra_strategy: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let state = Arc::new(Mutex::new(MockState {
                mode,
                extra_strategy,
                apply_calls: 0,
                receipt: None,
            }));
            let thread_state = Arc::clone(&state);
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let thread = thread::spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => handle_mock_request(&mut stream, &thread_state),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("mock bridge accept failed: {error}"),
                    }
                }
            });
            Self {
                endpoint,
                state,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for MockBridge {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    struct MockRequest {
        method: String,
        path: String,
        body: serde_json::Value,
    }

    fn read_mock_request(stream: &mut TcpStream) -> MockRequest {
        // Accepted sockets can inherit the Windows listener's nonblocking mode.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0, "request ended before headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(index) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let mut lines = headers.lines();
        let request_line = lines.next().unwrap();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap().to_owned();
        let path = request_parts.next().unwrap().to_owned();
        let content_length = lines
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or_default();
        while bytes.len() < header_end + content_length {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0, "request ended before body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        let body = if content_length == 0 {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap()
        };
        MockRequest { method, path, body }
    }

    fn handle_mock_request(stream: &mut TcpStream, state: &Arc<Mutex<MockState>>) {
        let request = read_mock_request(stream);
        let response = match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/v1/health") => serde_json::json!({
                "status": "running",
                "protocolVersion": 2,
                "pluginVersion": "0.3.0",
                "ymm4Version": "4.55.1.1"
            }),
            ("GET", "/v1/capabilities") => serde_json::json!({
                "protocolVersion": 2,
                "capabilities": [
                    "managed_audio",
                    "managed_caption",
                    "unified_target_plan",
                    "timeline_edit_managed_cue_mixed",
                    "readback_verification",
                    "idempotent_apply",
                    "request_bound_receipts",
                    "write_ahead_apply",
                    "native_voice_create",
                    "native_voice_remark_identity",
                    "native_voice_bounded_duration",
                    "mutation_profile_ymm4_4_55_1_1"
                ]
            }),
            ("POST", "/v2/timeline-edit/validate") => {
                validation_json(&request.body, state.lock().unwrap().extra_strategy)
            }
            ("POST", "/v2/timeline-edit/apply") => apply_json(&request.body, state),
            ("POST", "/v2/timeline-edit/not-started") => not_started_json(&request.body, state),
            _ => panic!(
                "unexpected mock bridge request: {} {}",
                request.method, request.path
            ),
        };
        let body = serde_json::to_vec(&response).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
        stream.flush().unwrap();
    }

    fn validation_json(request: &serde_json::Value, extra: bool) -> serde_json::Value {
        let plan = &request["timelineEditPlan"];
        let mut portable = 0_u64;
        let mut native = 0_u64;
        let mut create = 0_u64;
        let mut update = 0_u64;
        let mut delete = 0_u64;
        if let Some(operations) = plan["operations"].as_array() {
            for operation in operations {
                match operation["cue"]["strategy"].as_str() {
                    Some("portable_pair") => portable += 1,
                    Some("native_voice") => native += 1,
                    _ => {}
                }
                match operation["cue"]["action"].as_str() {
                    Some("create") => create += 1,
                    Some("update") => update += 1,
                    Some("delete") => delete += 1,
                    _ => {}
                }
            }
        }
        let mut strategies = serde_json::Map::new();
        if portable > 0 {
            strategies.insert("portable_pair".into(), serde_json::json!(portable));
        }
        if native > 0 {
            strategies.insert("native_voice".into(), serde_json::json!(native));
        }
        if extra {
            strategies.insert("unexpected".into(), serde_json::json!(1));
        }
        serde_json::json!({
            "operationId": plan["operationId"],
            "planDigest": request["planDigest"],
            "fingerprint": request["expectedFingerprint"],
            "strategyCounts": strategies,
            "createCount": create,
            "updateCount": update,
            "deleteCount": delete,
            "physicalItemCount": portable * 2 + native
        })
    }

    fn correct_items() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "entityId": "portable", "revision": 1, "kind": "audio",
                "frame": 10, "layer": 1, "length": 30, "text": null,
                "audioPath": "audio.wav", "artifactHash": "b".repeat(64),
                "speaker": null, "realizationId": null
            }),
            serde_json::json!({
                "entityId": "portable", "revision": 1, "kind": "caption",
                "frame": 10, "layer": 2, "length": 30, "text": "caption",
                "audioPath": null, "artifactHash": "b".repeat(64),
                "speaker": null, "realizationId": null
            }),
            serde_json::json!({
                "entityId": "native", "revision": 1, "kind": "voice",
                "frame": 50, "layer": 3, "length": 45, "text": "caption",
                "spokenText": "caption",
                "audioPath": null, "artifactHash": null, "speaker": "speaker",
                "realizationId": Uuid::from_u128(22)
            }),
        ]
    }

    fn apply_json(request: &serde_json::Value, state: &Arc<Mutex<MockState>>) -> serde_json::Value {
        let mut state = state.lock().unwrap();
        state.apply_calls += 1;
        if let Some(receipt) = state.receipt.clone() {
            return serde_json::json!({
                "success": receipt["verified"],
                "replayed": true,
                "receipt": receipt
            });
        }
        let plan = &request["timelineEditPlan"];
        let mut items = correct_items();
        let (status, verified, after, count, error) = match state.mode {
            ReceiptMode::Verified
            | ReceiptMode::WrongPlanDigest
            | ReceiptMode::WrongOperationCount
            | ReceiptMode::WrongItem => ("verified", true, AFTER_FINGERPRINT, 2, None),
            ReceiptMode::RolledBack => (
                "rolled_back",
                false,
                BASE_FINGERPRINT,
                0,
                Some("rolled back"),
            ),
            ReceiptMode::RecoveryRequired => (
                "recovery_required",
                false,
                AFTER_FINGERPRINT,
                1,
                Some("recovery required"),
            ),
        };
        if matches!(state.mode, ReceiptMode::WrongItem) {
            items[2]["speaker"] = serde_json::json!("wrong-speaker");
        }
        if !verified {
            items.clear();
        }
        let plan_digest = if matches!(state.mode, ReceiptMode::WrongPlanDigest) {
            format!("sha256:{}", "f".repeat(64))
        } else {
            request["planDigest"].as_str().unwrap().to_owned()
        };
        let count = if matches!(state.mode, ReceiptMode::WrongOperationCount) {
            1
        } else {
            count
        };
        let receipt = serde_json::json!({
            "operationId": plan["operationId"],
            "requestDigest": request["requestDigest"],
            "projectId": plan["target"]["projectId"],
            "sceneId": plan["target"]["sceneId"],
            "expectedFingerprint": request["expectedFingerprint"],
            "planDigest": plan_digest,
            "status": status,
            "beforeFingerprint": request["expectedFingerprint"],
            "afterFingerprint": after,
            "appliedItems": items,
            "appliedNativeExtensions": [],
            "appliedOperationCount": count,
            "verified": verified,
            "error": error
        });
        state.receipt = Some(receipt.clone());
        serde_json::json!({
            "success": verified,
            "replayed": false,
            "receipt": receipt
        })
    }

    fn not_started_json(
        request: &serde_json::Value,
        state: &Arc<Mutex<MockState>>,
    ) -> serde_json::Value {
        let mut state = state.lock().unwrap();
        if let Some(receipt) = state.receipt.clone() {
            return serde_json::json!({
                "success": receipt["verified"],
                "replayed": true,
                "receipt": receipt
            });
        }
        let plan = &request["timelineEditPlan"];
        let receipt = serde_json::json!({
            "operationId": plan["operationId"],
            "requestDigest": request["requestDigest"],
            "projectId": plan["target"]["projectId"],
            "sceneId": plan["target"]["sceneId"],
            "expectedFingerprint": request["expectedFingerprint"],
            "planDigest": request["planDigest"],
            "status": "not_started",
            "beforeFingerprint": request["expectedFingerprint"],
            "afterFingerprint": request["expectedFingerprint"],
            "appliedItems": [],
            "appliedNativeExtensions": [],
            "appliedOperationCount": 0,
            "verified": false,
            "error": "not started"
        });
        state.receipt = Some(receipt.clone());
        serde_json::json!({
            "success": false,
            "replayed": false,
            "receipt": receipt
        })
    }

    fn mixed_manifest() -> TimelineEditStageManifest {
        TimelineEditStageManifest {
            operations: vec![portable("portable"), native("native")],
            max_changed_entities: None,
        }
    }

    async fn staged_task(
        bridge: &MockBridge,
    ) -> (
        Ymm4TimelineEditTask,
        crate::DurableProjectStore,
        std::path::PathBuf,
    ) {
        let client = Ymm4BridgeClient::new(&bridge.endpoint, "test-token").unwrap();
        let mut task = Ymm4TimelineEditTask::stage_from_snapshot(
            &client,
            RevisionId(0),
            target(),
            mixed_manifest(),
        )
        .await
        .unwrap();
        let root = std::env::temp_dir().join(format!("takegraph-timeline-{}", Uuid::new_v4()));
        let store =
            crate::DurableProjectStore::open_scoped_or_bootstrap(&root, "project", RevisionId(0))
                .unwrap();
        let digest = task.patch.digest.clone();
        task.approve(&digest, RevisionId(0)).unwrap();
        (task, store, root)
    }

    #[tokio::test]
    async fn verified_mixed_batch_commits_one_revision_and_exact_retry_is_idempotent() {
        let bridge = MockBridge::start(ReceiptMode::Verified, false);
        let client = Ymm4BridgeClient::new(&bridge.endpoint, "test-token").unwrap();
        let (mut task, store, root) = staged_task(&bridge).await;
        let first = task
            .apply_and_finalize_durable(&client, &store, RevisionId(0))
            .await
            .unwrap();
        assert_eq!(first.revision, RevisionId(1));
        assert!(!first.canonical_replay);
        let state = store.snapshot().unwrap();
        assert_eq!(state.head, RevisionId(1));
        assert_eq!(state.external_commits.len(), 1);
        let projection = state.managed_target_states.values().next().unwrap();
        assert_eq!(projection.items.len(), 3);

        let replay = task
            .apply_and_finalize_durable(&client, &store, RevisionId(1))
            .await
            .unwrap();
        assert_eq!(replay.revision, RevisionId(1));
        assert!(replay.canonical_replay);
        assert_eq!(store.snapshot().unwrap().external_commits.len(), 1);
        assert_eq!(bridge.state.lock().unwrap().apply_calls, 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rolled_back_batch_aborts_reservation_without_revision() {
        let bridge = MockBridge::start(ReceiptMode::RolledBack, false);
        let client = Ymm4BridgeClient::new(&bridge.endpoint, "test-token").unwrap();
        let (mut task, store, root) = staged_task(&bridge).await;
        assert!(
            task.apply_and_finalize_durable(&client, &store, RevisionId(0))
                .await
                .is_err()
        );
        let state = store.snapshot().unwrap();
        assert_eq!(state.head, RevisionId(0));
        assert!(state.pending_external_commit.is_none());
        assert!(state.external_commits.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn recovery_required_retains_exact_reservation() {
        let bridge = MockBridge::start(ReceiptMode::RecoveryRequired, false);
        let client = Ymm4BridgeClient::new(&bridge.endpoint, "test-token").unwrap();
        let (mut task, store, root) = staged_task(&bridge).await;
        assert!(
            task.apply_and_finalize_durable(&client, &store, RevisionId(0))
                .await
                .is_err()
        );
        let state = store.snapshot().unwrap();
        assert_eq!(state.head, RevisionId(0));
        assert_eq!(
            state.pending_external_commit.as_ref().unwrap().operation_id,
            task.operation_id
        );
        assert!(state.external_commits.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn mismatched_receipts_and_extra_validation_strategy_fail_closed() {
        let extra = MockBridge::start(ReceiptMode::Verified, true);
        let client = Ymm4BridgeClient::new(&extra.endpoint, "test-token").unwrap();
        assert!(
            Ymm4TimelineEditTask::stage_from_snapshot(
                &client,
                RevisionId(0),
                target(),
                mixed_manifest(),
            )
            .await
            .is_err()
        );

        for mode in [
            ReceiptMode::WrongPlanDigest,
            ReceiptMode::WrongOperationCount,
            ReceiptMode::WrongItem,
        ] {
            let bridge = MockBridge::start(mode, false);
            let client = Ymm4BridgeClient::new(&bridge.endpoint, "test-token").unwrap();
            let (mut task, store, root) = staged_task(&bridge).await;
            assert!(
                task.apply_and_finalize_durable(&client, &store, RevisionId(0))
                    .await
                    .is_err()
            );
            let state = store.snapshot().unwrap();
            assert_eq!(state.head, RevisionId(0));
            assert!(state.pending_external_commit.is_some());
            assert!(state.external_commits.is_empty());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn narration_promotion_stages_timeline_edit_without_advancing_head() {
        let bridge = MockBridge::start(ReceiptMode::Verified, false);
        let client = Ymm4BridgeClient::new(&bridge.endpoint, "test-token").unwrap();
        let root = std::env::temp_dir().join(format!("takegraph-promote-stage-{}", Uuid::new_v4()));
        let annotation_store =
            crate::annotation_store::AnnotationStore::open_scoped(&root.join("ann"), "project")
                .unwrap();
        let project_store =
            crate::DurableProjectStore::open_scoped_or_bootstrap(&root.join("proj"), "project", RevisionId(0))
                .unwrap();
        let capture_id = takegraph_core::AnnotationId::new();
        annotation_store
            .import_capture(takegraph_core::AnnotationCapture {
                id: capture_id,
                session_id: takegraph_core::CaptureSessionId::new(),
                start_anchor: takegraph_core::SourceAnchor {
                    project_id: "project".into(),
                    scene_id: "scene".into(),
                    source_fingerprint: BASE_FINGERPRINT.into(),
                    fps: 60,
                    frame: 100,
                    observed_canonical_revision: Some(RevisionId(0)),
                },
                end_anchor: takegraph_core::SourceAnchor {
                    project_id: "project".into(),
                    scene_id: "scene".into(),
                    source_fingerprint: BASE_FINGERPRINT.into(),
                    fps: 60,
                    frame: 160,
                    observed_canonical_revision: Some(RevisionId(0)),
                },
                audio: takegraph_core::CapturedAudioEvidence {
                    audio_sha256: format!("sha256:{}", "a".repeat(64)),
                    byte_length: 32_000,
                    duration_samples: 16_000,
                    sample_rate: 16_000,
                    channels: 1,
                    bits_per_sample: 16,
                },
                captured_at_utc: "2026-08-14T13:34:57Z".into(),
            })
            .unwrap();
        annotation_store
            .attach_transcript(takegraph_core::AnnotationTranscript {
                id: Uuid::new_v4(),
                capture_id,
                audio_sha256: format!("sha256:{}", "a".repeat(64)),
                text: "Compressionの説明を入れる".into(),
                provider_id: "human".into(),
                provider_digest: format!("sha256:{}", "b".repeat(64)),
                transcript_digest: format!("sha256:{}", "c".repeat(64)),
            })
            .unwrap();
        annotation_store
            .attach_interpretation(takegraph_core::AnnotationInterpretation {
                id: Uuid::new_v4(),
                capture_id,
                transcript_digest: format!("sha256:{}", "c".repeat(64)),
                temporal: takegraph_core::TemporalReference {
                    reference_frame: 100,
                    start_offset_frames: 0,
                    end_offset_frames: Some(60),
                    relation: takegraph_core::TemporalRelation::Range,
                },
                intents: vec![takegraph_core::AnnotationIntent::Narration {
                    topic: "Compression".into(),
                    draft_hint: Some("ここで重要なのがPrimary Compressionです。".into()),
                }],
                model_id: "heuristic-v1".into(),
                model_digest: format!("sha256:{}", "d".repeat(64)),
                interpretation_digest: format!("sha256:{}", "e".repeat(64)),
            })
            .unwrap();

        let staged = crate::stage_narration_promotion(
            &annotation_store,
            &project_store,
            &client,
            target(),
            capture_id,
            "ゆっくり霊夢",
            2,
            300,
        )
        .await
        .unwrap();
        assert_eq!(staged.operations.len(), 1);
        assert!(staged.plan_digest.starts_with("sha256:"));
        assert_eq!(project_store.head().unwrap(), RevisionId(0));
        let promotion = annotation_store.capture(capture_id).unwrap().promotion.unwrap();
        assert_eq!(
            promotion.status,
            crate::annotation_store::PromotionStatus::Staged
        );
        assert_eq!(promotion.plan_digest, staged.plan_digest);
        assert_eq!(promotion.task_id, staged.task.operation_id.to_string());
        let evidence = staged.task.timeline_edit_plan.source_evidence.clone();
        assert!(!evidence.is_empty());
        let receipt = format!("sha256:{}", "f".repeat(64));
        let count = crate::commit_promotions_from_plan(
            &annotation_store,
            &evidence,
            &staged.task.operation_id.to_string(),
            RevisionId(1),
            &receipt,
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            annotation_store.capture(capture_id).unwrap().promotion.unwrap().status,
            crate::annotation_store::PromotionStatus::Committed
        );
        crate::commit_promotions_from_plan(
            &annotation_store,
            &evidence,
            &staged.task.operation_id.to_string(),
            RevisionId(1),
            &receipt,
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
