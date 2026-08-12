//! Staged, digest-bound scene capture and human review lifecycle.
//!
//! A scene inspection is an advisory post-apply quality gate. It never advances
//! the canonical project revision and never substitutes for semantic YMM4
//! read-back or a final render receipt.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use takegraph_core::RevisionId;
use takegraph_node::{
    ImportedSceneCapture, SCENE_PIXEL_DETECTOR_VERSION, SceneInspectionFinding,
    SceneInspectionNodeError, SceneVisualCheckProfile, Ymm4BridgeClient, Ymm4Error,
    Ymm4SceneCaptureFrameReceipt, Ymm4SceneCaptureReceipt, Ymm4SceneCaptureRequest,
    Ymm4SceneCaptureRequestInput, Ymm4SceneCaptureStatus, import_png_capture,
    verify_imported_capture,
};
use thiserror::Error;
use uuid::Uuid;

const MAX_CAPTURE_FRAMES: usize = 64;
const MAX_YMM4_FRAME: u32 = 2_147_483_647;

/// Scene state to which an inspection is bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneInspectionSource {
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: RevisionId,
    /// Aggregate bridge fingerprint used as the capture precondition.
    pub expected_fingerprint: String,
    pub managed_fingerprint: String,
    pub conflict_fingerprint: String,
}

/// Capture/inspection policy whose digest is sent to the bridge and receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneCaptureProfile {
    pub profile_id: String,
    pub driver_profile_digest: String,
    pub alpha: bool,
    pub max_actual_frame_delta: u32,
    pub visual_checks: SceneVisualCheckProfile,
}

impl SceneCaptureProfile {
    /// Computes the stable digest of the complete driver and detector policy.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid visual profile or serialization failure.
    pub fn digest(&self) -> Result<String, SceneInspectionError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestPayload<'a> {
            detector_version: &'static str,
            profile: &'a SceneCaptureProfile,
        }
        self.validate()?;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&DigestPayload {
                detector_version: SCENE_PIXEL_DETECTOR_VERSION,
                profile: self,
            })?)
        ))
    }

    fn validate(&self) -> Result<(), SceneInspectionError> {
        if self.profile_id.trim().is_empty() || self.driver_profile_digest.trim().is_empty() {
            return Err(SceneInspectionError::InvalidPlan(
                "capture profile and driver profile IDs must be non-empty".into(),
            ));
        }
        self.visual_checks.validate()?;
        Ok(())
    }
}

/// One exact frame requested from the native capture driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneCaptureSamplePlan {
    pub sample_id: Uuid,
    pub requested_frame: u32,
}

/// A changed cue range used to derive first/middle/last inspection samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangedCueFrameRange {
    pub first_frame: u32,
    pub length: u32,
}

/// Digest-bound scene capture plan. Capturing is a separate Tier 2 operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneInspectionPlan {
    pub inspection_id: Uuid,
    pub operation_id: Uuid,
    pub source: SceneInspectionSource,
    pub capture_profile: SceneCaptureProfile,
    pub capture_profile_digest: String,
    pub samples: Vec<SceneCaptureSamplePlan>,
    pub digest: String,
    pub approved_digest: Option<String>,
}

impl SceneInspectionPlan {
    /// Stages an exact, de-duplicated set of frames without contacting YMM4.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty/invalid source, profile, or frame list.
    pub fn stage(
        source: SceneInspectionSource,
        capture_profile: SceneCaptureProfile,
        frames: impl IntoIterator<Item = u32>,
    ) -> Result<Self, SceneInspectionError> {
        validate_source(&source)?;
        let capture_profile_digest = capture_profile.digest()?;
        let unique_frames: BTreeSet<_> = frames.into_iter().collect();
        if unique_frames.is_empty() {
            return Err(SceneInspectionError::InvalidPlan(
                "at least one capture frame is required".into(),
            ));
        }
        if unique_frames.len() > MAX_CAPTURE_FRAMES {
            return Err(SceneInspectionError::InvalidPlan(format!(
                "at most {MAX_CAPTURE_FRAMES} capture frames are supported"
            )));
        }
        if unique_frames.iter().any(|frame| *frame > MAX_YMM4_FRAME) {
            return Err(SceneInspectionError::InvalidPlan(
                "capture frame exceeds YMM4's signed frame range".into(),
            ));
        }
        let samples = unique_frames
            .into_iter()
            .map(|requested_frame| SceneCaptureSamplePlan {
                sample_id: Uuid::new_v4(),
                requested_frame,
            })
            .collect();
        let mut plan = Self {
            inspection_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4(),
            source,
            capture_profile,
            capture_profile_digest,
            samples,
            digest: String::new(),
            approved_digest: None,
        };
        plan.digest = plan.canonical_digest()?;
        Ok(plan)
    }

    /// Stages first-stable, midpoint, and last-stable frames for changed cues.
    ///
    /// Short or overlapping cues are de-duplicated globally.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid cue range or ordinary plan validation.
    pub fn stage_changed_cues(
        source: SceneInspectionSource,
        capture_profile: SceneCaptureProfile,
        changed_cues: &[ChangedCueFrameRange],
        stability_inset_frames: u32,
    ) -> Result<Self, SceneInspectionError> {
        let frames = sample_changed_cue_frames(changed_cues, stability_inset_frames)?;
        Self::stage(source, capture_profile, frames)
    }

    /// Validates persisted plan contents against their stored profile/plan hashes.
    ///
    /// # Errors
    ///
    /// Returns an error if any digest-bound plan field changed.
    pub fn validate_digest(&self) -> Result<(), SceneInspectionError> {
        let actual_profile_digest = self.capture_profile.digest()?;
        if actual_profile_digest != self.capture_profile_digest {
            return Err(SceneInspectionError::CaptureProfileDigestMismatch {
                stored: self.capture_profile_digest.clone(),
                actual: actual_profile_digest,
            });
        }
        let actual = self.canonical_digest()?;
        if actual != self.digest {
            return Err(SceneInspectionError::PlanDigestMismatch {
                stored: self.digest.clone(),
                actual,
            });
        }
        Ok(())
    }

    /// Records human approval for the exact capture plan.
    ///
    /// # Errors
    ///
    /// Returns an error for tampering, a stale source, or a different digest.
    pub fn approve(
        &mut self,
        approved_digest: &str,
        current_source: &SceneInspectionSource,
    ) -> Result<(), SceneInspectionError> {
        self.validate_digest()?;
        if &self.source != current_source {
            return Err(SceneInspectionError::StaleSource);
        }
        if approved_digest != self.digest {
            return Err(SceneInspectionError::ApprovalDigestMismatch);
        }
        self.approved_digest = Some(self.digest.clone());
        Ok(())
    }

    /// Builds the request whose canonical digest is independently checked by
    /// the YMM4 bridge.
    ///
    /// # Errors
    ///
    /// Returns an error unless the exact current plan has been approved.
    pub fn capture_request(&self) -> Result<Ymm4SceneCaptureRequest, SceneInspectionError> {
        self.require_approved()?;
        Ok(Ymm4SceneCaptureRequest::new(Ymm4SceneCaptureRequestInput {
            operation_id: self.operation_id,
            project_id: self.source.project_id.clone(),
            scene_id: self.source.scene_id.clone(),
            expected_fingerprint: self.source.expected_fingerprint.clone(),
            source_revision: self.source.source_revision.0,
            capture_profile_digest: self.capture_profile_digest.clone(),
            frames: self
                .samples
                .iter()
                .map(|sample| sample.requested_frame)
                .collect(),
            alpha: self.capture_profile.alpha,
        }))
    }

    fn require_approved(&self) -> Result<(), SceneInspectionError> {
        self.validate_digest()?;
        if self.approved_digest.as_deref() != Some(self.digest.as_str()) {
            return Err(SceneInspectionError::PlanNotApproved);
        }
        Ok(())
    }

    fn canonical_digest(&self) -> Result<String, SceneInspectionError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestPayload<'a> {
            canonical_version: &'static str,
            inspection_id: Uuid,
            operation_id: Uuid,
            source: &'a SceneInspectionSource,
            capture_profile_digest: &'a str,
            samples: &'a [SceneCaptureSamplePlan],
        }

        let payload = DigestPayload {
            canonical_version: "takegraph-scene-inspection-plan/v1",
            inspection_id: self.inspection_id,
            operation_id: self.operation_id,
            source: &self.source,
            capture_profile_digest: &self.capture_profile_digest,
            samples: &self.samples,
        };
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&payload)?)
        ))
    }
}

/// Lifecycle of a persisted scene-inspection receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneInspectionStatus {
    Staged,
    Approved,
    Captured,
    Reviewed,
    Accepted,
    Rejected,
    Stale,
}

/// Explicit human disposition; pixel findings never decide this automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneReviewDecision {
    Accept,
    Reject,
}

/// Audit note for a human scene review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneHumanReview {
    pub reviewer: String,
    pub note: String,
    pub decision: Option<SceneReviewDecision>,
}

/// Persistable non-authoritative inspection receipt returned to CLI/MCP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneInspectionReceipt {
    pub inspection_id: Uuid,
    pub operation_id: Uuid,
    pub plan_digest: String,
    pub source: SceneInspectionSource,
    pub capture_profile_digest: String,
    pub driver_profile_digest: String,
    pub status: SceneInspectionStatus,
    pub captures: Vec<ImportedSceneCapture>,
    pub findings: Vec<SceneInspectionFinding>,
    pub capture_evidence: Option<SceneCaptureEvidence>,
    pub review: Option<SceneHumanReview>,
    pub stale_reasons: Vec<String>,
}

/// Read-back evidence that native capture preserved YMM4's scene and UI state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneCaptureEvidence {
    pub driver: String,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub transient_state_restored: bool,
    pub project_dirty_before: bool,
    pub project_dirty_after: bool,
}

/// Stateful service-owned inspection task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneInspectionTask {
    pub plan: SceneInspectionPlan,
    pub receipt: SceneInspectionReceipt,
    #[serde(skip)]
    bridge_receipt_trusted: bool,
}

impl SceneInspectionTask {
    /// Creates a task around a staged plan without changing any project state.
    #[must_use]
    pub fn new(plan: SceneInspectionPlan) -> Self {
        let receipt = SceneInspectionReceipt {
            inspection_id: plan.inspection_id,
            operation_id: plan.operation_id,
            plan_digest: plan.digest.clone(),
            source: plan.source.clone(),
            capture_profile_digest: plan.capture_profile_digest.clone(),
            driver_profile_digest: plan.capture_profile.driver_profile_digest.clone(),
            status: if plan.approved_digest.as_deref() == Some(plan.digest.as_str()) {
                SceneInspectionStatus::Approved
            } else {
                SceneInspectionStatus::Staged
            },
            captures: Vec::new(),
            findings: Vec::new(),
            capture_evidence: None,
            review: None,
            stale_reasons: Vec::new(),
        };
        Self {
            plan,
            receipt,
            bridge_receipt_trusted: false,
        }
    }

    /// Parses persisted state while treating its bridge receipt as untrusted.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, digest tampering, or receipt/plan
    /// identity mismatch.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, SceneInspectionError> {
        let mut task: Self = serde_json::from_slice(bytes)?;
        task.plan.validate_digest()?;
        task.validate_receipt_identity()?;
        task.bridge_receipt_trusted = false;
        Ok(task)
    }

    /// Approves the exact plan and changes no YMM or `TakeGraph` project state.
    ///
    /// # Errors
    ///
    /// Returns an error for stale/tampered state or a mismatched digest.
    pub fn approve(
        &mut self,
        approved_digest: &str,
        current_source: &SceneInspectionSource,
    ) -> Result<(), SceneInspectionError> {
        self.require_status(SceneInspectionStatus::Staged)?;
        self.plan.approve(approved_digest, current_source)?;
        self.receipt.status = SceneInspectionStatus::Approved;
        Ok(())
    }

    /// Validates an authenticated bridge response, imports all PNGs atomically
    /// at task level, and records deterministic findings.
    ///
    /// Claimed hashes, dimensions, and paths in the bridge receipt are never
    /// trusted without filesystem read-back.
    ///
    /// # Errors
    ///
    /// Returns an error for request-binding mismatch, unsafe staging paths,
    /// capture side effects, missing/duplicate frames, pixel import failure, or
    /// a claim that differs from the imported artifact.
    pub fn ingest_authenticated_response(
        &mut self,
        bridge_receipt: &Ymm4SceneCaptureReceipt,
        authorized_staging_root: &Path,
        artifact_root: &Path,
    ) -> Result<(), SceneInspectionError> {
        if !matches!(
            self.receipt.status,
            SceneInspectionStatus::Approved
                | SceneInspectionStatus::Captured
                | SceneInspectionStatus::Reviewed
                | SceneInspectionStatus::Accepted
                | SceneInspectionStatus::Rejected
        ) {
            return Err(SceneInspectionError::UnexpectedStatus {
                expected: SceneInspectionStatus::Approved,
                actual: self.receipt.status,
            });
        }
        self.plan.require_approved()?;
        let prior_status = self.receipt.status;
        let prior_review = self.receipt.review.clone();
        let request = self.plan.capture_request()?;
        validate_bridge_receipt(
            &request,
            bridge_receipt,
            &self.plan.capture_profile.driver_profile_digest,
        )?;

        let staging_root = fs::canonicalize(authorized_staging_root)?;
        let expected: BTreeMap<u32, Uuid> = self
            .plan
            .samples
            .iter()
            .map(|sample| (sample.requested_frame, sample.sample_id))
            .collect();
        if bridge_receipt.frames.len() != expected.len() {
            return Err(SceneInspectionError::CaptureSetMismatch);
        }

        let mut seen = BTreeSet::new();
        let mut imported = Vec::with_capacity(expected.len());
        for frame in &bridge_receipt.frames {
            let Some(sample_id) = expected.get(&frame.requested_frame).copied() else {
                return Err(SceneInspectionError::UnexpectedCaptureFrame(
                    frame.requested_frame,
                ));
            };
            if !seen.insert(frame.requested_frame) {
                return Err(SceneInspectionError::DuplicateCaptureFrame(
                    frame.requested_frame,
                ));
            }
            let delta = frame.actual_frame.abs_diff(frame.requested_frame);
            if delta > self.plan.capture_profile.max_actual_frame_delta {
                return Err(SceneInspectionError::ActualFrameOutsideBudget {
                    requested: frame.requested_frame,
                    actual: frame.actual_frame,
                    maximum_delta: self.plan.capture_profile.max_actual_frame_delta,
                });
            }
            let source = authorized_capture_path(&staging_root, frame)?;
            let capture = import_png_capture(
                &source,
                artifact_root,
                sample_id,
                frame.requested_frame,
                frame.actual_frame,
                &self.plan.capture_profile.visual_checks,
            )?;
            validate_frame_claim(frame, &capture)?;
            imported.push(capture);
        }
        if seen.len() != expected.len() {
            return Err(SceneInspectionError::CaptureSetMismatch);
        }
        imported.sort_by_key(|capture| capture.requested_frame);

        // Mutate task state only after every capture has been validated/imported.
        self.receipt.findings = imported
            .iter()
            .flat_map(|capture| capture.inspection.findings.iter().cloned())
            .collect();
        self.receipt.captures = imported;
        self.receipt.capture_evidence = Some(SceneCaptureEvidence {
            driver: bridge_receipt.driver.clone(),
            before_fingerprint: bridge_receipt.before_fingerprint.clone(),
            after_fingerprint: bridge_receipt.after_fingerprint.clone(),
            transient_state_restored: bridge_receipt.transient_state_restored,
            project_dirty_before: bridge_receipt.project_dirty_before,
            project_dirty_after: bridge_receipt.project_dirty_after,
        });
        self.receipt.review = prior_review;
        self.receipt.stale_reasons.clear();
        self.receipt.status = match prior_status {
            SceneInspectionStatus::Reviewed
            | SceneInspectionStatus::Accepted
            | SceneInspectionStatus::Rejected => prior_status,
            _ => SceneInspectionStatus::Captured,
        };
        self.bridge_receipt_trusted = true;
        Ok(())
    }

    /// Executes the approved request over the authenticated loopback client and
    /// imports the returned captures before exposing them for review.
    ///
    /// Repeating this call uses the same operation ID and permits the bridge to
    /// replay its bound receipt. This is also how persisted, untrusted receipt
    /// state is re-established after a service restart.
    ///
    /// # Errors
    ///
    /// Returns a bridge, request-binding, staging-path, artifact, or inspection
    /// validation error.
    pub async fn capture_and_ingest(
        &mut self,
        client: &Ymm4BridgeClient,
        authorized_staging_root: &Path,
        artifact_root: &Path,
    ) -> Result<(), SceneInspectionError> {
        let request = self.plan.capture_request()?;
        let receipt = client.capture_scene(&request).await?;
        self.ingest_authenticated_response(&receipt, authorized_staging_root, artifact_root)
    }

    /// Re-reads all immutable artifacts before opening human review.
    ///
    /// # Errors
    ///
    /// Returns an error for untrusted persisted state, changed/missing bytes, or
    /// an invalid lifecycle transition.
    pub fn begin_review(
        &mut self,
        reviewer: impl Into<String>,
    ) -> Result<(), SceneInspectionError> {
        self.require_status(SceneInspectionStatus::Captured)?;
        if !self.bridge_receipt_trusted {
            return Err(SceneInspectionError::UntrustedBridgeReceipt);
        }
        let reviewer = reviewer.into();
        if reviewer.trim().is_empty() {
            return Err(SceneInspectionError::InvalidReviewer);
        }
        for capture in &self.receipt.captures {
            verify_imported_capture(capture)?;
        }
        self.receipt.review = Some(SceneHumanReview {
            reviewer,
            note: String::new(),
            decision: None,
        });
        self.receipt.status = SceneInspectionStatus::Reviewed;
        Ok(())
    }

    /// Records an explicit human accept/reject decision.
    ///
    /// # Errors
    ///
    /// Returns an error unless read-back verification opened the review first.
    pub fn decide(
        &mut self,
        decision: SceneReviewDecision,
        note: impl Into<String>,
    ) -> Result<(), SceneInspectionError> {
        self.require_status(SceneInspectionStatus::Reviewed)?;
        if !self.bridge_receipt_trusted {
            return Err(SceneInspectionError::UntrustedBridgeReceipt);
        }
        for capture in &self.receipt.captures {
            verify_imported_capture(capture)?;
        }
        let review = self
            .receipt
            .review
            .as_mut()
            .ok_or(SceneInspectionError::MissingReview)?;
        review.note = note.into();
        review.decision = Some(decision);
        self.receipt.status = match decision {
            SceneReviewDecision::Accept => SceneInspectionStatus::Accepted,
            SceneReviewDecision::Reject => SceneInspectionStatus::Rejected,
        };
        Ok(())
    }

    /// Returns whether this task is a trusted, accepted quality gate for the
    /// exact current scene and capture profile.
    ///
    /// Persisted JSON alone can never satisfy this check: the bridge receipt
    /// must first be replayed over the authenticated client, and all artifacts
    /// are read back again here.
    ///
    /// # Errors
    ///
    /// Returns an artifact read-back error if a previously accepted image was
    /// removed or modified.
    pub fn is_trusted_acceptance_for(
        &self,
        current_source: &SceneInspectionSource,
        current_capture_profile_digest: &str,
    ) -> Result<bool, SceneInspectionError> {
        if !self.bridge_receipt_trusted
            || self.receipt.status != SceneInspectionStatus::Accepted
            || &self.plan.source != current_source
            || self.plan.capture_profile_digest != current_capture_profile_digest
        {
            return Ok(false);
        }
        for capture in &self.receipt.captures {
            verify_imported_capture(capture)?;
        }
        Ok(true)
    }

    /// Invalidates captured/reviewed results after any relevant source/profile
    /// change. This method never changes the project revision.
    pub fn invalidate_if_changed(
        &mut self,
        current_source: &SceneInspectionSource,
        current_capture_profile_digest: &str,
    ) -> bool {
        let mut reasons = Vec::new();
        if &self.plan.source != current_source {
            reasons.push("project, scene, revision, or scene fingerprint changed".into());
        }
        if self.plan.capture_profile_digest != current_capture_profile_digest {
            reasons.push("capture profile changed".into());
        }
        if reasons.is_empty() {
            return false;
        }
        self.receipt.status = SceneInspectionStatus::Stale;
        self.receipt.stale_reasons = reasons;
        self.bridge_receipt_trusted = false;
        true
    }

    fn validate_receipt_identity(&self) -> Result<(), SceneInspectionError> {
        if self.receipt.inspection_id != self.plan.inspection_id
            || self.receipt.operation_id != self.plan.operation_id
            || self.receipt.plan_digest != self.plan.digest
            || self.receipt.source != self.plan.source
            || self.receipt.capture_profile_digest != self.plan.capture_profile_digest
            || self.receipt.driver_profile_digest != self.plan.capture_profile.driver_profile_digest
        {
            return Err(SceneInspectionError::ReceiptPlanMismatch);
        }
        Ok(())
    }

    fn require_status(&self, expected: SceneInspectionStatus) -> Result<(), SceneInspectionError> {
        if self.receipt.status == expected {
            Ok(())
        } else {
            Err(SceneInspectionError::UnexpectedStatus {
                expected,
                actual: self.receipt.status,
            })
        }
    }
}

/// Returns de-duplicated first-stable, midpoint, and last-stable samples.
///
/// # Errors
///
/// Returns an error for a zero-length range or frame arithmetic overflow.
pub fn sample_changed_cue_frames(
    changed_cues: &[ChangedCueFrameRange],
    stability_inset_frames: u32,
) -> Result<Vec<u32>, SceneInspectionError> {
    if changed_cues.is_empty() {
        return Err(SceneInspectionError::InvalidPlan(
            "at least one changed cue is required".into(),
        ));
    }
    let mut frames = BTreeSet::new();
    for cue in changed_cues {
        if cue.length == 0 {
            return Err(SceneInspectionError::InvalidPlan(
                "changed cue length must be non-zero".into(),
            ));
        }
        let last = cue
            .first_frame
            .checked_add(cue.length - 1)
            .ok_or_else(|| SceneInspectionError::InvalidPlan("cue frame range overflow".into()))?;
        let inset = stability_inset_frames.min((cue.length - 1) / 2);
        let first_stable = cue
            .first_frame
            .checked_add(inset)
            .ok_or_else(|| SceneInspectionError::InvalidPlan("cue sample frame overflow".into()))?;
        let last_stable = last - inset;
        let midpoint = cue
            .first_frame
            .checked_add((cue.length - 1) / 2)
            .ok_or_else(|| SceneInspectionError::InvalidPlan("cue midpoint overflow".into()))?;
        frames.extend([first_stable, midpoint, last_stable]);
    }
    Ok(frames.into_iter().collect())
}

fn validate_source(source: &SceneInspectionSource) -> Result<(), SceneInspectionError> {
    if source.project_id.trim().is_empty()
        || source.scene_id.trim().is_empty()
        || source.expected_fingerprint.trim().is_empty()
        || source.managed_fingerprint.trim().is_empty()
        || source.conflict_fingerprint.trim().is_empty()
    {
        return Err(SceneInspectionError::InvalidPlan(
            "source identity and fingerprints must be non-empty".into(),
        ));
    }
    Ok(())
}

fn validate_bridge_receipt(
    request: &Ymm4SceneCaptureRequest,
    receipt: &Ymm4SceneCaptureReceipt,
    expected_driver_profile_digest: &str,
) -> Result<(), SceneInspectionError> {
    if receipt.status != Ymm4SceneCaptureStatus::Captured {
        return Err(SceneInspectionError::CaptureFailed(
            receipt
                .error
                .clone()
                .unwrap_or_else(|| "bridge did not verify scene capture".into()),
        ));
    }
    if receipt.operation_id != request.operation_id
        || receipt.request_digest != request.request_digest
        || receipt.project_id != request.project_id
        || receipt.scene_id != request.scene_id
        || receipt.expected_fingerprint != request.expected_fingerprint
        || receipt.source_revision != request.source_revision
        || receipt.capture_profile_digest != request.capture_profile_digest
        || receipt.driver_profile_digest != expected_driver_profile_digest
    {
        return Err(SceneInspectionError::BridgeReceiptBindingMismatch);
    }
    if receipt.before_fingerprint != request.expected_fingerprint
        || receipt.after_fingerprint != request.expected_fingerprint
    {
        return Err(SceneInspectionError::CaptureChangedProjectState);
    }
    if receipt.driver.trim().is_empty()
        || !receipt.transient_state_restored
        || receipt.project_dirty_before != receipt.project_dirty_after
    {
        return Err(SceneInspectionError::CaptureLeakedTransientState);
    }
    Ok(())
}

fn authorized_capture_path(
    staging_root: &Path,
    frame: &Ymm4SceneCaptureFrameReceipt,
) -> Result<PathBuf, SceneInspectionError> {
    let path = fs::canonicalize(&frame.path)?;
    if !path.starts_with(staging_root) || path == staging_root {
        return Err(SceneInspectionError::CaptureOutsideStaging(path));
    }
    Ok(path)
}

fn validate_frame_claim(
    claim: &Ymm4SceneCaptureFrameReceipt,
    capture: &ImportedSceneCapture,
) -> Result<(), SceneInspectionError> {
    if claim.sha256 != capture.sha256
        || claim.width != capture.width
        || claim.height != capture.height
        || claim.media_type != capture.media_type
    {
        return Err(SceneInspectionError::CaptureClaimMismatch {
            requested_frame: claim.requested_frame,
        });
    }
    Ok(())
}

/// Failure in planning, importing, or reviewing a scene inspection.
#[derive(Debug, Error)]
pub enum SceneInspectionError {
    #[error("invalid scene inspection plan: {0}")]
    InvalidPlan(String),
    #[error("capture profile digest mismatch: stored {stored}, actual {actual}")]
    CaptureProfileDigestMismatch { stored: String, actual: String },
    #[error("scene inspection plan digest mismatch: stored {stored}, actual {actual}")]
    PlanDigestMismatch { stored: String, actual: String },
    #[error("approved digest does not match the scene inspection plan")]
    ApprovalDigestMismatch,
    #[error("scene inspection source is stale")]
    StaleSource,
    #[error("scene inspection plan has not been approved")]
    PlanNotApproved,
    #[error("expected scene inspection status {expected:?}, got {actual:?}")]
    UnexpectedStatus {
        expected: SceneInspectionStatus,
        actual: SceneInspectionStatus,
    },
    #[error("bridge scene-capture receipt is not bound to the approved request")]
    BridgeReceiptBindingMismatch,
    #[error("scene capture failed: {0}")]
    CaptureFailed(String),
    #[error("scene capture changed the project fingerprint")]
    CaptureChangedProjectState,
    #[error("scene capture did not restore transient YMM state")]
    CaptureLeakedTransientState,
    #[error("bridge returned a different capture frame set")]
    CaptureSetMismatch,
    #[error("bridge returned unexpected requested frame {0}")]
    UnexpectedCaptureFrame(u32),
    #[error("bridge returned requested frame {0} more than once")]
    DuplicateCaptureFrame(u32),
    #[error(
        "actual frame {actual} is outside the ±{maximum_delta} budget for requested frame {requested}"
    )]
    ActualFrameOutsideBudget {
        requested: u32,
        actual: u32,
        maximum_delta: u32,
    },
    #[error("capture path is outside the authorized staging directory: {0}")]
    CaptureOutsideStaging(PathBuf),
    #[error("bridge PNG claims do not match imported frame {requested_frame}")]
    CaptureClaimMismatch { requested_frame: u32 },
    #[error("persisted scene inspection receipt does not match its plan")]
    ReceiptPlanMismatch,
    #[error("persisted bridge receipt must be replayed through the authenticated bridge")]
    UntrustedBridgeReceipt,
    #[error("reviewer must be non-empty")]
    InvalidReviewer,
    #[error("scene inspection review record is missing")]
    MissingReview,
    #[error(transparent)]
    Node(#[from] SceneInspectionNodeError),
    #[error(transparent)]
    Bridge(#[from] Ymm4Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_node::{PixelRect, Rgba8, VisualRegionExpectation, VisualRegionKind};

    fn temporary_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "takegraph-scene-service-{label}-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(rgba).unwrap();
        }
        bytes
    }

    fn source() -> SceneInspectionSource {
        SceneInspectionSource {
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            source_revision: RevisionId(7),
            expected_fingerprint: "fingerprint-a".into(),
            managed_fingerprint: "managed-a".into(),
            conflict_fingerprint: "conflict-a".into(),
        }
    }

    fn capture_profile() -> SceneCaptureProfile {
        SceneCaptureProfile {
            profile_id: "native-preview-rgba-v1".into(),
            driver_profile_digest: "driver-a".into(),
            alpha: true,
            max_actual_frame_delta: 0,
            visual_checks: SceneVisualCheckProfile {
                expected_width: 8,
                expected_height: 8,
                black_luma_threshold: 8,
                black_pixel_ratio_ppm: 995_000,
                blank_channel_span_threshold: 2,
                safe_area: Some(PixelRect {
                    x: 1,
                    y: 1,
                    width: 6,
                    height: 6,
                }),
                regions: vec![VisualRegionExpectation {
                    region_id: "caption".into(),
                    kind: VisualRegionKind::Caption,
                    bounds: PixelRect {
                        x: 1,
                        y: 4,
                        width: 6,
                        height: 3,
                    },
                    background: Rgba8 {
                        red: 20,
                        green: 20,
                        blue: 20,
                        alpha: 255,
                    },
                    color_tolerance: 5,
                    min_foreground_ppm: 10_000,
                    minimum_edge_clearance_px: 0,
                }],
            },
        }
    }

    fn approved_task() -> SceneInspectionTask {
        let mut plan = SceneInspectionPlan::stage(source(), capture_profile(), [10]).unwrap();
        let digest = plan.digest.clone();
        plan.approve(&digest, &source()).unwrap();
        SceneInspectionTask::new(plan)
    }

    fn response_for(task: &SceneInspectionTask, path: &Path) -> Ymm4SceneCaptureReceipt {
        let request = task.plan.capture_request().unwrap();
        let bytes = fs::read(path).unwrap();
        Ymm4SceneCaptureReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest,
            project_id: request.project_id,
            scene_id: request.scene_id,
            expected_fingerprint: request.expected_fingerprint.clone(),
            source_revision: request.source_revision,
            capture_profile_digest: request.capture_profile_digest,
            status: Ymm4SceneCaptureStatus::Captured,
            before_fingerprint: request.expected_fingerprint.clone(),
            after_fingerprint: request.expected_fingerprint,
            frames: vec![Ymm4SceneCaptureFrameReceipt {
                requested_frame: 10,
                actual_frame: 10,
                path: path.to_string_lossy().into_owned(),
                sha256: format!("{:x}", Sha256::digest(bytes)),
                width: 8,
                height: 8,
                media_type: "image/png".into(),
            }],
            driver: "ymm4-preview-save-image/4.55.1.1".into(),
            driver_profile_digest: "driver-a".into(),
            transient_state_restored: true,
            project_dirty_before: false,
            project_dirty_after: false,
            error: None,
        }
    }

    #[test]
    fn samples_short_and_overlapping_cues_without_duplicates() {
        assert_eq!(
            sample_changed_cue_frames(
                &[
                    ChangedCueFrameRange {
                        first_frame: 10,
                        length: 1,
                    },
                    ChangedCueFrameRange {
                        first_frame: 10,
                        length: 5,
                    },
                ],
                1,
            )
            .unwrap(),
            vec![10, 11, 12, 13]
        );
    }

    #[test]
    fn capture_import_review_and_accept_is_a_non_revision_lifecycle() {
        let root = temporary_directory("happy");
        let staging = root.join("staging");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&staging).unwrap();
        let path = staging.join("frame-10.png");
        let mut pixels = [20, 20, 20, 255].repeat(64);
        pixels[((5 * 8 + 3) * 4)..((5 * 8 + 3) * 4 + 4)].copy_from_slice(&[255, 255, 255, 255]);
        fs::write(&path, encode_rgba(8, 8, &pixels)).unwrap();
        let mut task = approved_task();
        let revision_before = task.plan.source.source_revision;

        task.ingest_authenticated_response(&response_for(&task, &path), &staging, &artifacts)
            .unwrap();
        assert_eq!(task.receipt.status, SceneInspectionStatus::Captured);
        assert_eq!(task.receipt.captures.len(), 1);
        task.begin_review("lance").unwrap();
        task.decide(SceneReviewDecision::Accept, "composition looks correct")
            .unwrap();

        assert_eq!(task.receipt.status, SceneInspectionStatus::Accepted);
        assert_eq!(task.plan.source.source_revision, revision_before);
        assert_eq!(
            task.receipt.review.as_ref().unwrap().decision,
            Some(SceneReviewDecision::Accept)
        );
        assert!(
            task.is_trusted_acceptance_for(&source(), &task.plan.capture_profile_digest)
                .unwrap()
        );

        let mut restored =
            SceneInspectionTask::from_json_slice(&serde_json::to_vec(&task).unwrap()).unwrap();
        assert!(
            !restored
                .is_trusted_acceptance_for(&source(), &restored.plan.capture_profile_digest)
                .unwrap()
        );
        restored
            .ingest_authenticated_response(&response_for(&restored, &path), &staging, &artifacts)
            .unwrap();
        assert_eq!(restored.receipt.status, SceneInspectionStatus::Accepted);
        assert!(
            restored
                .is_trusted_acceptance_for(&source(), &restored.plan.capture_profile_digest)
                .unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_capture_outside_authorized_staging_and_false_hash_claim() {
        let root = temporary_directory("security");
        let staging = root.join("staging");
        let outside = root.join("outside");
        fs::create_dir_all(&staging).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let path = outside.join("frame.png");
        let mut pixels = [20, 20, 20, 255].repeat(64);
        pixels[0] = 21;
        fs::write(&path, encode_rgba(8, 8, &pixels)).unwrap();
        let mut task = approved_task();

        assert!(matches!(
            task.ingest_authenticated_response(
                &response_for(&task, &path),
                &staging,
                &root.join("artifacts")
            ),
            Err(SceneInspectionError::CaptureOutsideStaging(_))
        ));
        assert_eq!(task.receipt.status, SceneInspectionStatus::Approved);

        let inside = staging.join("frame.png");
        fs::copy(&path, &inside).unwrap();
        let mut response = response_for(&task, &inside);
        response.frames[0].sha256 = "0".repeat(64);
        assert!(matches!(
            task.ingest_authenticated_response(&response, &staging, &root.join("artifacts")),
            Err(SceneInspectionError::CaptureClaimMismatch { .. })
        ));
        assert_eq!(task.receipt.status, SceneInspectionStatus::Approved);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_receipt_requires_authenticated_replay_before_review() {
        let root = temporary_directory("trust");
        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();
        let path = staging.join("frame.png");
        let mut pixels = [20, 20, 20, 255].repeat(64);
        pixels[0] = 21;
        fs::write(&path, encode_rgba(8, 8, &pixels)).unwrap();
        let mut task = approved_task();
        task.ingest_authenticated_response(
            &response_for(&task, &path),
            &staging,
            &root.join("artifacts"),
        )
        .unwrap();
        let bytes = serde_json::to_vec(&task).unwrap();

        let mut restored = SceneInspectionTask::from_json_slice(&bytes).unwrap();
        assert!(matches!(
            restored.begin_review("reviewer"),
            Err(SceneInspectionError::UntrustedBridgeReceipt)
        ));
        restored
            .ingest_authenticated_response(
                &response_for(&restored, &path),
                &staging,
                &root.join("artifacts"),
            )
            .unwrap();
        restored.begin_review("reviewer").unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_review_requires_authenticated_replay_before_decision() {
        let root = temporary_directory("review-trust");
        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();
        let path = staging.join("frame.png");
        let mut pixels = [20, 20, 20, 255].repeat(64);
        pixels[0] = 21;
        fs::write(&path, encode_rgba(8, 8, &pixels)).unwrap();
        let mut task = approved_task();
        task.ingest_authenticated_response(
            &response_for(&task, &path),
            &staging,
            &root.join("artifacts"),
        )
        .unwrap();
        task.begin_review("reviewer").unwrap();

        let mut restored =
            SceneInspectionTask::from_json_slice(&serde_json::to_vec(&task).unwrap()).unwrap();
        assert!(matches!(
            restored.decide(SceneReviewDecision::Accept, "looks good"),
            Err(SceneInspectionError::UntrustedBridgeReceipt)
        ));
        restored
            .ingest_authenticated_response(
                &response_for(&restored, &path),
                &staging,
                &root.join("artifacts"),
            )
            .unwrap();
        restored
            .decide(SceneReviewDecision::Accept, "looks good")
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn review_rechecks_content_addressed_artifact_bytes() {
        let root = temporary_directory("artifact-tamper");
        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();
        let path = staging.join("frame.png");
        let mut pixels = [20, 20, 20, 255].repeat(64);
        pixels[0] = 21;
        fs::write(&path, encode_rgba(8, 8, &pixels)).unwrap();
        let mut task = approved_task();
        task.ingest_authenticated_response(
            &response_for(&task, &path),
            &staging,
            &root.join("artifacts"),
        )
        .unwrap();
        fs::write(&task.receipt.captures[0].artifact_path, b"tampered").unwrap();

        assert!(matches!(
            task.begin_review("reviewer"),
            Err(SceneInspectionError::Node(
                SceneInspectionNodeError::ArtifactReadbackMismatch { .. }
            ))
        ));
        assert_eq!(task.receipt.status, SceneInspectionStatus::Captured);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_capture_that_leaks_dirty_or_transient_state() {
        let root = temporary_directory("state-leak");
        let staging = root.join("staging");
        fs::create_dir_all(&staging).unwrap();
        let path = staging.join("frame.png");
        let mut pixels = [20, 20, 20, 255].repeat(64);
        pixels[0] = 21;
        fs::write(&path, encode_rgba(8, 8, &pixels)).unwrap();
        let mut task = approved_task();
        let mut response = response_for(&task, &path);
        response.project_dirty_after = true;

        assert!(matches!(
            task.ingest_authenticated_response(&response, &staging, &root.join("artifacts")),
            Err(SceneInspectionError::CaptureLeakedTransientState)
        ));
        response.project_dirty_after = false;
        response.transient_state_restored = false;
        assert!(matches!(
            task.ingest_authenticated_response(&response, &staging, &root.join("artifacts")),
            Err(SceneInspectionError::CaptureLeakedTransientState)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_or_profile_change_marks_inspection_stale() {
        let mut task = approved_task();
        let mut changed_source = source();
        changed_source.conflict_fingerprint = "conflict-b".into();

        assert!(
            task.invalidate_if_changed(&changed_source, &task.plan.capture_profile_digest.clone())
        );
        assert_eq!(task.receipt.status, SceneInspectionStatus::Stale);
        assert!(!task.receipt.stale_reasons.is_empty());

        let mut profile_task = approved_task();
        assert!(profile_task.invalidate_if_changed(&source(), &"9".repeat(64)));
        assert_eq!(profile_task.receipt.status, SceneInspectionStatus::Stale);
        assert!(
            profile_task
                .receipt
                .stale_reasons
                .iter()
                .any(|reason| reason.contains("profile"))
        );
    }

    #[test]
    fn automated_findings_cannot_bypass_human_review() {
        let mut task = approved_task();
        task.receipt.status = SceneInspectionStatus::Captured;
        assert!(matches!(
            task.decide(
                SceneReviewDecision::Accept,
                "automation attempted acceptance"
            ),
            Err(SceneInspectionError::UnexpectedStatus {
                expected: SceneInspectionStatus::Reviewed,
                actual: SceneInspectionStatus::Captured,
            })
        ));
        assert_eq!(task.receipt.status, SceneInspectionStatus::Captured);
        assert!(task.receipt.review.is_none());
    }

    #[test]
    fn rejects_plan_tampering_after_approval() {
        let mut task = approved_task();
        task.plan.samples[0].requested_frame += 1;
        assert!(matches!(
            task.plan.capture_request(),
            Err(SceneInspectionError::PlanDigestMismatch { .. })
        ));
    }
}
