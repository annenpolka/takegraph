use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Monotonic project revision identifier.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RevisionId(pub u64);

impl RevisionId {
    /// Returns the next revision, if the counter has not overflowed.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// Stable patch identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PatchId(pub Uuid);

impl PatchId {
    /// Creates a new patch identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PatchId {
    fn default() -> Self {
        Self::new()
    }
}

/// Lifecycle for a reviewable edit proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchStatus {
    Draft,
    Validated,
    Previewable,
    Approved,
    Committed,
    Conflicted,
}

/// Minimal patch state shared by the service and the formal specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub id: PatchId,
    pub base: RevisionId,
    pub status: PatchStatus,
    pub digest: String,
    pub approved_digest: Option<String>,
    pub touches_hard_lock: bool,
}

impl Patch {
    /// Starts a draft against a specific base revision.
    #[must_use]
    pub fn draft(base: RevisionId, digest: impl Into<String>) -> Self {
        Self {
            id: PatchId::new(),
            base,
            status: PatchStatus::Draft,
            digest: digest.into(),
            approved_digest: None,
            touches_hard_lock: false,
        }
    }

    /// Marks a draft as structurally valid.
    ///
    /// # Errors
    ///
    /// Returns [`PatchError::UnexpectedStatus`] unless the patch is a draft.
    pub fn validate(&mut self) -> Result<(), PatchError> {
        self.require_status(PatchStatus::Draft)?;
        self.status = PatchStatus::Validated;
        Ok(())
    }

    /// Marks a validated patch as ready for preview and review.
    ///
    /// # Errors
    ///
    /// Returns [`PatchError::UnexpectedStatus`] unless validation succeeded.
    pub fn materialize_preview(&mut self) -> Result<(), PatchError> {
        self.require_status(PatchStatus::Validated)?;
        self.status = PatchStatus::Previewable;
        Ok(())
    }

    /// Records approval for the exact current digest.
    ///
    /// # Errors
    ///
    /// Returns [`PatchError::UnexpectedStatus`] unless a preview is available.
    pub fn approve(&mut self) -> Result<(), PatchError> {
        self.require_status(PatchStatus::Previewable)?;
        self.approved_digest = Some(self.digest.clone());
        self.status = PatchStatus::Approved;
        Ok(())
    }

    /// Replaces patch content and invalidates any prior approval.
    pub fn replace_digest(&mut self, digest: impl Into<String>) {
        self.digest = digest.into();
        self.approved_digest = None;
        if self.status == PatchStatus::Approved {
            self.status = PatchStatus::Previewable;
        }
    }

    /// Commits only when approval, base revision, digest, and hard locks agree.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid lifecycle state, stale base, stale
    /// approval, hard-lock conflict, or revision overflow.
    pub fn commit(&mut self, current_head: RevisionId) -> Result<RevisionId, PatchError> {
        self.require_status(PatchStatus::Approved)?;

        if self.base != current_head {
            self.status = PatchStatus::Conflicted;
            return Err(PatchError::StaleBase {
                expected: self.base,
                actual: current_head,
            });
        }

        if self.approved_digest.as_deref() != Some(self.digest.as_str()) {
            return Err(PatchError::StaleApproval);
        }

        if self.touches_hard_lock {
            return Err(PatchError::HardLock);
        }

        let next = current_head
            .checked_next()
            .ok_or(PatchError::RevisionOverflow)?;
        self.status = PatchStatus::Committed;
        Ok(next)
    }

    fn require_status(&self, expected: PatchStatus) -> Result<(), PatchError> {
        if self.status == expected {
            Ok(())
        } else {
            Err(PatchError::UnexpectedStatus {
                expected,
                actual: self.status,
            })
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PatchError {
    #[error("expected patch status {expected:?}, got {actual:?}")]
    UnexpectedStatus {
        expected: PatchStatus,
        actual: PatchStatus,
    },
    #[error("patch base {expected:?} does not match project head {actual:?}")]
    StaleBase {
        expected: RevisionId,
        actual: RevisionId,
    },
    #[error("approval does not match the current patch digest")]
    StaleApproval,
    #[error("patch touches a hard-locked value")]
    HardLock,
    #[error("project revision counter overflowed")]
    RevisionOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_requires_fresh_approval() {
        let mut patch = Patch::draft(RevisionId(7), "digest-a");
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();
        patch.approve().unwrap();
        patch.replace_digest("digest-b");

        assert_eq!(
            patch.commit(RevisionId(7)),
            Err(PatchError::UnexpectedStatus {
                expected: PatchStatus::Approved,
                actual: PatchStatus::Previewable,
            })
        );
    }

    #[test]
    fn stale_base_becomes_conflict() {
        let mut patch = Patch::draft(RevisionId(7), "digest-a");
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();
        patch.approve().unwrap();

        assert!(matches!(
            patch.commit(RevisionId(8)),
            Err(PatchError::StaleBase { .. })
        ));
        assert_eq!(patch.status, PatchStatus::Conflicted);
    }
}
