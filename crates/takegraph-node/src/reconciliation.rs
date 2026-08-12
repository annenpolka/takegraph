//! Typed, digest-bound YMM4 reconciliation adapter operations.
//!
//! Detach deliberately removes only a `TakeGraph` identity carried in YMM4's
//! `Remark` field.  The bridge owns the write-ahead log and the independent
//! before/after content witness; callers still verify the authenticated
//! receipt and then perform a fresh semantic snapshot read-back.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{YMM4_BRIDGE_PROTOCOL_VERSION, Ymm4BridgeClient, Ymm4Error};

/// Cross-runtime request-digest domain for metadata-only detach.
pub const METADATA_DETACH_REQUEST_DOMAIN: &str = "takegraph-ymm4-metadata-detach-v2";

/// Immutable input used to construct a detach request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ymm4MetadataDetachRequestInput {
    pub operation_id: Uuid,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub expected_fingerprint: String,
    pub entity_id: String,
    pub realization_id: Uuid,
}

/// Project-scoped, identity-bound request to remove one native Remark marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4MetadataDetachRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub expected_fingerprint: String,
    pub entity_id: String,
    pub realization_id: Uuid,
    pub identity_carrier: String,
}

impl Ymm4MetadataDetachRequest {
    /// Validates the complete authorization binding and computes its digest.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty binding or nil operation/realization ID.
    pub fn try_new(input: Ymm4MetadataDetachRequestInput) -> Result<Self, MetadataDetachNodeError> {
        if input.operation_id.is_nil() {
            return Err(MetadataDetachNodeError::NilOperationId);
        }
        if input.realization_id.is_nil() {
            return Err(MetadataDetachNodeError::NilRealizationId);
        }
        for (name, value) in [
            ("projectId", input.project_id.as_str()),
            ("sceneId", input.scene_id.as_str()),
            ("expectedFingerprint", input.expected_fingerprint.as_str()),
            ("entityId", input.entity_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(MetadataDetachNodeError::EmptyField(name));
            }
        }
        let mut request = Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id: input.operation_id,
            request_digest: String::new(),
            project_id: input.project_id,
            scene_id: input.scene_id,
            source_revision: input.source_revision,
            expected_fingerprint: input.expected_fingerprint,
            entity_id: input.entity_id,
            realization_id: input.realization_id,
            identity_carrier: "takegraph_remark_v2".into(),
        };
        request.request_digest = metadata_detach_request_digest(&request);
        Ok(request)
    }

    /// Recomputes the digest after loading a durable/editable request.
    #[must_use]
    pub fn recompute_digest(&self) -> String {
        metadata_detach_request_digest(self)
    }
}

/// Durable bridge state for a metadata-only detach operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4MetadataDetachStatus {
    NotStarted,
    Applying,
    Verified,
    RolledBack,
    Failed,
    RecoveryRequired,
}

/// Authenticated evidence that only the Remark identity carrier changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4MetadataDetachReceipt {
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub expected_fingerprint: String,
    pub entity_id: String,
    pub realization_id: Uuid,
    pub identity_carrier: String,
    pub status: Ymm4MetadataDetachStatus,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub detached_item_count: u32,
    pub before_remark_digest: String,
    pub expected_after_remark_digest: String,
    pub remark_digest_after: String,
    pub non_remark_content_digest_before: String,
    pub non_remark_content_digest_after: String,
    pub remark_absent: bool,
    pub verified: bool,
    pub error: Option<String>,
}

/// Response indicates whether a durable result was replayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4MetadataDetachResponse {
    pub success: bool,
    pub replayed: bool,
    pub receipt: Ymm4MetadataDetachReceipt,
}

impl Ymm4BridgeClient {
    /// Executes (or idempotently replays) an authenticated metadata detach.
    ///
    /// # Errors
    ///
    /// Returns a transport, authorization, stale-state, recovery, or bridge
    /// contract error.
    pub async fn detach_managed_metadata(
        &self,
        request: &Ymm4MetadataDetachRequest,
    ) -> Result<Ymm4MetadataDetachResponse, Ymm4Error> {
        self.post_json("v2/reconciliation/detach", request).await
    }

    /// Reads one persisted detach result by operation ID.
    ///
    /// # Errors
    ///
    /// Returns a transport, authorization, or not-found error.
    pub async fn metadata_detach_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Ymm4MetadataDetachReceipt, Ymm4Error> {
        self.get_json(&format!("v2/reconciliation/detach/{operation_id}"))
            .await
    }

    /// Seals (or replays) a request-bound proof that detach never started.
    ///
    /// The bridge serializes this with detach apply and durably tombstones the
    /// operation, so a delayed exact apply cannot mutate after the proof.
    ///
    /// # Errors
    ///
    /// Returns a transport, authorization, stale-state, recovery, or bridge
    /// contract error.
    pub async fn seal_metadata_detach_not_started(
        &self,
        request: &Ymm4MetadataDetachRequest,
    ) -> Result<Ymm4MetadataDetachResponse, Ymm4Error> {
        self.post_json("v2/reconciliation/detach/not-started", request)
            .await
    }
}

/// Verifies every approval binding and the bridge's content-preservation proof.
///
/// # Errors
///
/// Returns an error unless the exact request was verified, the marker is
/// absent, and the non-Remark content witness is byte-for-byte unchanged.
pub fn verify_metadata_detach(
    request: &Ymm4MetadataDetachRequest,
    receipt: &Ymm4MetadataDetachReceipt,
) -> Result<(), MetadataDetachNodeError> {
    verify_metadata_detach_binding(request, receipt)?;
    if receipt.status != Ymm4MetadataDetachStatus::Verified || !receipt.verified {
        return Err(MetadataDetachNodeError::NotVerified(receipt.status));
    }
    if receipt.before_fingerprint != request.expected_fingerprint
        || receipt.detached_item_count != 1
        || !receipt.remark_absent
        || !is_sha256_hex(&receipt.before_remark_digest)
        || !is_sha256_hex(&receipt.expected_after_remark_digest)
        || receipt.remark_digest_after != receipt.expected_after_remark_digest
        || !is_sha256_hex(&receipt.remark_digest_after)
        || !is_sha256_hex(&receipt.non_remark_content_digest_before)
        || receipt.non_remark_content_digest_before != receipt.non_remark_content_digest_after
        || receipt.error.is_some()
    {
        return Err(MetadataDetachNodeError::PreservationProofMismatch);
    }
    Ok(())
}

/// Authenticates a terminal rollback before a canonical detach reservation is
/// released. A status string alone is never sufficient: the receipt must bind
/// the exact request and prove restoration of both the full Remark witness and
/// all non-Remark content.
///
/// # Errors
///
/// Returns an error for changed request bindings or incomplete restoration.
pub fn verify_metadata_detach_rollback(
    request: &Ymm4MetadataDetachRequest,
    receipt: &Ymm4MetadataDetachReceipt,
) -> Result<(), MetadataDetachNodeError> {
    verify_metadata_detach_binding(request, receipt)?;
    if receipt.status != Ymm4MetadataDetachStatus::RolledBack
        || receipt.verified
        || receipt.before_fingerprint != request.expected_fingerprint
        || receipt.after_fingerprint != receipt.before_fingerprint
        || receipt.detached_item_count != 1
        || receipt.remark_absent
        || !is_sha256_hex(&receipt.before_remark_digest)
        || !is_sha256_hex(&receipt.expected_after_remark_digest)
        || receipt.remark_digest_after != receipt.before_remark_digest
        || !is_sha256_hex(&receipt.remark_digest_after)
        || !is_sha256_hex(&receipt.non_remark_content_digest_before)
        || receipt.non_remark_content_digest_after != receipt.non_remark_content_digest_before
        || receipt.error.as_deref().is_none_or(str::is_empty)
    {
        return Err(MetadataDetachNodeError::RollbackProofMismatch);
    }
    Ok(())
}

/// Authenticates a durable, request-bound proof that no detach WAL or target
/// mutation was started.
///
/// # Errors
///
/// Returns an error for a rebound request or incomplete no-mutation proof.
pub fn verify_metadata_detach_not_started(
    request: &Ymm4MetadataDetachRequest,
    receipt: &Ymm4MetadataDetachReceipt,
) -> Result<(), MetadataDetachNodeError> {
    verify_metadata_detach_binding(request, receipt)?;
    if receipt.status != Ymm4MetadataDetachStatus::NotStarted
        || receipt.verified
        || receipt.before_fingerprint != request.expected_fingerprint
        || receipt.after_fingerprint != receipt.before_fingerprint
        || receipt.detached_item_count != 0
        || receipt.remark_absent
        || !is_sha256_hex(&receipt.before_remark_digest)
        || receipt.expected_after_remark_digest != receipt.before_remark_digest
        || receipt.remark_digest_after != receipt.before_remark_digest
        || receipt.non_remark_content_digest_before != receipt.before_remark_digest
        || receipt.non_remark_content_digest_after != receipt.before_remark_digest
        || receipt.error.as_deref().is_none_or(str::is_empty)
    {
        return Err(MetadataDetachNodeError::NotStartedProofMismatch);
    }
    Ok(())
}

fn verify_metadata_detach_binding(
    request: &Ymm4MetadataDetachRequest,
    receipt: &Ymm4MetadataDetachReceipt,
) -> Result<(), MetadataDetachNodeError> {
    if request.recompute_digest() != request.request_digest {
        return Err(MetadataDetachNodeError::RequestDigestMismatch);
    }
    if receipt.operation_id != request.operation_id
        || receipt.request_digest != request.request_digest
        || receipt.project_id != request.project_id
        || receipt.scene_id != request.scene_id
        || receipt.source_revision != request.source_revision
        || receipt.expected_fingerprint != request.expected_fingerprint
        || receipt.entity_id != request.entity_id
        || receipt.realization_id != request.realization_id
        || receipt.identity_carrier != request.identity_carrier
    {
        return Err(MetadataDetachNodeError::ReceiptBindingMismatch);
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn metadata_detach_request_digest(request: &Ymm4MetadataDetachRequest) -> String {
    use std::fmt::Write as _;
    let mut canonical = format!("{METADATA_DETACH_REQUEST_DOMAIN}\n");
    let _ = writeln!(canonical, "protocolVersion:{}", request.protocol_version);
    write_string(
        &mut canonical,
        "operationId",
        &request.operation_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", &request.project_id);
    write_string(&mut canonical, "sceneId", &request.scene_id);
    let _ = writeln!(canonical, "sourceRevision:{}", request.source_revision);
    write_string(
        &mut canonical,
        "expectedFingerprint",
        &request.expected_fingerprint,
    );
    write_string(&mut canonical, "entityId", &request.entity_id);
    write_string(
        &mut canonical,
        "realizationId",
        &request.realization_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "identityCarrier", &request.identity_carrier);
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn write_string(canonical: &mut String, name: &str, value: &str) {
    use std::fmt::Write as _;
    let _ = writeln!(canonical, "{name}:{}:{value}", value.len());
}

#[derive(Debug, Error)]
pub enum MetadataDetachNodeError {
    #[error("metadata detach operation ID must not be nil")]
    NilOperationId,
    #[error("metadata detach realization ID must not be nil")]
    NilRealizationId,
    #[error("metadata detach field is empty: {0}")]
    EmptyField(&'static str),
    #[error("metadata detach request digest does not match its payload")]
    RequestDigestMismatch,
    #[error("metadata detach receipt is not bound to the approved request")]
    ReceiptBindingMismatch,
    #[error("metadata detach was not verified: {0:?}")]
    NotVerified(Ymm4MetadataDetachStatus),
    #[error("metadata detach did not prove Remark-only preservation")]
    PreservationProofMismatch,
    #[error("metadata detach rollback did not prove the exact before-state was restored")]
    RollbackProofMismatch,
    #[error("metadata detach did not prove that the exact request never started")]
    NotStartedProofMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Ymm4MetadataDetachRequest {
        Ymm4MetadataDetachRequest::try_new(Ymm4MetadataDetachRequestInput {
            operation_id: Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            source_revision: 7,
            expected_fingerprint: "fingerprint-a".into(),
            entity_id: "utt-01".into(),
            realization_id: Uuid::parse_str("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
        })
        .unwrap()
    }

    #[test]
    fn request_digest_is_stable() {
        assert_eq!(
            request().request_digest,
            "a18c313ad1a046f8d0b25867ab8d988dbece617e55f23c102f70848a5e3e9802"
        );
    }

    #[test]
    fn receipt_requires_unchanged_non_remark_content() {
        let request = request();
        let mut receipt = Ymm4MetadataDetachReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest.clone(),
            project_id: request.project_id.clone(),
            scene_id: request.scene_id.clone(),
            source_revision: request.source_revision,
            expected_fingerprint: request.expected_fingerprint.clone(),
            entity_id: request.entity_id.clone(),
            realization_id: request.realization_id,
            identity_carrier: request.identity_carrier.clone(),
            status: Ymm4MetadataDetachStatus::Verified,
            before_fingerprint: request.expected_fingerprint.clone(),
            after_fingerprint: "after".into(),
            detached_item_count: 1,
            before_remark_digest: "a".repeat(64),
            expected_after_remark_digest: "c".repeat(64),
            remark_digest_after: "c".repeat(64),
            non_remark_content_digest_before: "b".repeat(64),
            non_remark_content_digest_after: "b".repeat(64),
            remark_absent: true,
            verified: true,
            error: None,
        };
        verify_metadata_detach(&request, &receipt).unwrap();
        receipt.non_remark_content_digest_after = "c".repeat(64);
        assert!(matches!(
            verify_metadata_detach(&request, &receipt),
            Err(MetadataDetachNodeError::PreservationProofMismatch)
        ));
    }

    #[test]
    fn rollback_requires_exact_bound_before_state_restoration() {
        let request = request();
        let mut receipt = Ymm4MetadataDetachReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest.clone(),
            project_id: request.project_id.clone(),
            scene_id: request.scene_id.clone(),
            source_revision: request.source_revision,
            expected_fingerprint: request.expected_fingerprint.clone(),
            entity_id: request.entity_id.clone(),
            realization_id: request.realization_id,
            identity_carrier: request.identity_carrier.clone(),
            status: Ymm4MetadataDetachStatus::RolledBack,
            before_fingerprint: request.expected_fingerprint.clone(),
            after_fingerprint: request.expected_fingerprint.clone(),
            detached_item_count: 1,
            before_remark_digest: "a".repeat(64),
            expected_after_remark_digest: "c".repeat(64),
            remark_digest_after: "a".repeat(64),
            non_remark_content_digest_before: "b".repeat(64),
            non_remark_content_digest_after: "b".repeat(64),
            remark_absent: false,
            verified: false,
            error: Some("mutation rolled back".into()),
        };
        verify_metadata_detach_rollback(&request, &receipt).unwrap();
        receipt.remark_digest_after = "c".repeat(64);
        assert!(matches!(
            verify_metadata_detach_rollback(&request, &receipt),
            Err(MetadataDetachNodeError::RollbackProofMismatch)
        ));
    }

    #[test]
    fn not_started_proof_is_exact_and_request_bound() {
        let request = request();
        let digest = "a".repeat(64);
        let mut receipt = Ymm4MetadataDetachReceipt {
            operation_id: request.operation_id,
            request_digest: request.request_digest.clone(),
            project_id: request.project_id.clone(),
            scene_id: request.scene_id.clone(),
            source_revision: request.source_revision,
            expected_fingerprint: request.expected_fingerprint.clone(),
            entity_id: request.entity_id.clone(),
            realization_id: request.realization_id,
            identity_carrier: request.identity_carrier.clone(),
            status: Ymm4MetadataDetachStatus::NotStarted,
            before_fingerprint: request.expected_fingerprint.clone(),
            after_fingerprint: request.expected_fingerprint.clone(),
            detached_item_count: 0,
            before_remark_digest: digest.clone(),
            expected_after_remark_digest: digest.clone(),
            remark_digest_after: digest.clone(),
            non_remark_content_digest_before: digest.clone(),
            non_remark_content_digest_after: digest,
            remark_absent: false,
            verified: false,
            error: Some("durable no-mutation tombstone".into()),
        };
        verify_metadata_detach_not_started(&request, &receipt).unwrap();
        receipt.detached_item_count = 1;
        assert!(matches!(
            verify_metadata_detach_not_started(&request, &receipt),
            Err(MetadataDetachNodeError::NotStartedProofMismatch)
        ));
    }
}
