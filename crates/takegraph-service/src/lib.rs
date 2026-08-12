//! Canonical project service façade.
//!
//! Persistence is deliberately deferred. The initial service proves that all
//! commits pass through the deterministic core state transition.

use takegraph_core::{Patch, PatchError, PatchId, RevisionId};

#[derive(Debug, Default)]
pub struct ProjectService {
    head: RevisionId,
    patches: Vec<Patch>,
}

impl ProjectService {
    /// Returns the current canonical project revision.
    #[must_use]
    pub fn head(&self) -> RevisionId {
        self.head
    }

    /// Stores a reviewable patch without applying it.
    pub fn stage(&mut self, patch: Patch) {
        self.patches.push(patch);
    }

    /// Commits a staged patch through the portable core guard.
    ///
    /// # Errors
    ///
    /// Returns an error if the patch does not exist or fails core validation.
    pub fn commit(&mut self, patch_id: PatchId) -> Result<RevisionId, ServiceError> {
        let patch = self
            .patches
            .iter_mut()
            .find(|patch| patch.id == patch_id)
            .ok_or(ServiceError::PatchNotFound(patch_id))?;
        let next = patch.commit(self.head)?;
        self.head = next;
        Ok(next)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("patch not found: {0:?}")]
    PatchNotFound(PatchId),
    #[error(transparent)]
    Patch(#[from] PatchError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_commits_through_core_guard() {
        let mut service = ProjectService::default();
        let mut patch = Patch::draft(service.head(), "digest-a");
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();
        patch.approve().unwrap();
        let id = patch.id;
        service.stage(patch);

        assert_eq!(service.commit(id).unwrap(), RevisionId(1));
    }
}
