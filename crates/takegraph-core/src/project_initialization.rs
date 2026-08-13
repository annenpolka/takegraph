//! Portable approval contract for initializing a `TakeGraph` project binding.
//!
//! Host paths deliberately stay outside this type. The service stores them in
//! its local task journal, while this plan seals only their canonical digest
//! and a review-safe file name.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{CanonicalError, RevisionId, canonical_sha256};

/// Current portable project-initialization plan schema.
pub const PROJECT_INITIALIZATION_PLAN_SCHEMA_VERSION: u32 = 1;

/// How the active YMM4 project becomes bound to a canonical `TakeGraph` store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectInitializationMode {
    /// Bind an already-named active YMM4 project without changing YMM4.
    AdoptActive,
    /// Save the active untitled project to a new path, then initialize its store.
    SaveUntitled,
}

/// Exact transient YMM4 source observed while staging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectInitializationSource {
    pub project_id: String,
    pub scene_id: String,
    pub fingerprint: String,
    /// Process-local identity of the active project object. A bridge restart or
    /// switching between indistinguishable untitled projects invalidates it.
    pub project_instance_id: String,
    /// Digest of the existing absolute project path; absent only for untitled projects.
    pub project_path_digest: Option<String>,
}

/// Save-As destination without leaking its host path into the portable plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectInitializationDestination {
    pub project_id: String,
    pub path_digest: String,
    pub file_name: String,
}

/// Exact, approval-bound project initialization proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectInitializationPlan {
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub mode: ProjectInitializationMode,
    pub source: ProjectInitializationSource,
    pub destination: Option<ProjectInitializationDestination>,
    pub canonical_revision: RevisionId,
    /// Whether the matching canonical project store already existed at stage time.
    pub canonical_preexisting: bool,
    /// Digest of the exact bridge feature/profile used for staging.
    pub capability_digest: String,
    pub plan_digest: String,
}

impl ProjectInitializationPlan {
    /// Builds and hashes a validated plan.
    ///
    /// # Errors
    ///
    /// Returns an error for incomplete source/destination bindings or canonicalization.
    pub fn build(
        operation_id: Uuid,
        mode: ProjectInitializationMode,
        source: ProjectInitializationSource,
        destination: Option<ProjectInitializationDestination>,
        canonical_revision: RevisionId,
        canonical_preexisting: bool,
        capability_digest: impl Into<String>,
    ) -> Result<Self, ProjectInitializationPlanError> {
        let mut plan = Self {
            schema_version: PROJECT_INITIALIZATION_PLAN_SCHEMA_VERSION,
            operation_id,
            mode,
            source,
            destination,
            canonical_revision,
            canonical_preexisting,
            capability_digest: capability_digest.into(),
            plan_digest: String::new(),
        };
        plan.validate_without_digest()?;
        plan.plan_digest = plan.compute_digest()?;
        Ok(plan)
    }

    /// Verifies both structural invariants and the canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when serialized plan data was changed or is unsupported.
    pub fn verify(&self) -> Result<(), ProjectInitializationPlanError> {
        self.validate_without_digest()?;
        if self.plan_digest != self.compute_digest()? {
            return Err(ProjectInitializationPlanError::DigestMismatch);
        }
        Ok(())
    }

    /// Returns the canonical project ID that will own the initialized store.
    #[must_use]
    pub fn target_project_id(&self) -> &str {
        self.destination
            .as_ref()
            .map_or(self.source.project_id.as_str(), |value| {
                value.project_id.as_str()
            })
    }

    fn validate_without_digest(&self) -> Result<(), ProjectInitializationPlanError> {
        if self.schema_version != PROJECT_INITIALIZATION_PLAN_SCHEMA_VERSION {
            return Err(ProjectInitializationPlanError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        if self.operation_id.is_nil() {
            return Err(ProjectInitializationPlanError::InvalidOperationId);
        }
        for (name, value) in [
            ("projectId", self.source.project_id.as_str()),
            ("sceneId", self.source.scene_id.as_str()),
            ("fingerprint", self.source.fingerprint.as_str()),
            (
                "projectInstanceId",
                self.source.project_instance_id.as_str(),
            ),
            ("capabilityDigest", self.capability_digest.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ProjectInitializationPlanError::EmptyField(name));
            }
        }
        require_digest("fingerprint", &self.source.fingerprint)?;
        require_digest("capabilityDigest", &self.capability_digest)?;
        if let Some(path_digest) = &self.source.project_path_digest {
            require_digest("projectPathDigest", path_digest)?;
        }

        match (self.mode, &self.destination) {
            (ProjectInitializationMode::AdoptActive, None) => {
                if self.source.project_path_digest.is_none()
                    || (!self.canonical_preexisting && self.canonical_revision != RevisionId(0))
                {
                    return Err(ProjectInitializationPlanError::ModeBindingMismatch);
                }
            }
            (ProjectInitializationMode::SaveUntitled, Some(destination)) => {
                if self.source.project_path_digest.is_some()
                    || self.canonical_preexisting
                    || self.canonical_revision != RevisionId(0)
                    || destination.project_id.trim().is_empty()
                    || destination.file_name.trim().is_empty()
                    || destination.project_id == self.source.project_id
                {
                    return Err(ProjectInitializationPlanError::ModeBindingMismatch);
                }
                require_digest("destinationPathDigest", &destination.path_digest)?;
            }
            _ => return Err(ProjectInitializationPlanError::ModeBindingMismatch),
        }
        Ok(())
    }

    fn compute_digest(&self) -> Result<String, CanonicalError> {
        canonical_sha256(
            "takegraph-project-initialization-plan-v1",
            &(
                self.schema_version,
                self.operation_id,
                self.mode,
                &self.source,
                &self.destination,
                self.canonical_revision,
                self.canonical_preexisting,
                &self.capability_digest,
            ),
        )
    }
}

fn require_digest(field: &'static str, value: &str) -> Result<(), ProjectInitializationPlanError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(ProjectInitializationPlanError::InvalidDigest(field));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProjectInitializationPlanError::InvalidDigest(field));
    }
    Ok(())
}

/// Validation failure for a portable project initialization plan.
#[derive(Debug, Error)]
pub enum ProjectInitializationPlanError {
    #[error("unsupported project initialization schema version {0}")]
    UnsupportedSchema(u32),
    #[error("project initialization operation ID must not be nil")]
    InvalidOperationId,
    #[error("project initialization field is empty: {0}")]
    EmptyField(&'static str),
    #[error("project initialization digest is invalid: {0}")]
    InvalidDigest(&'static str),
    #[error("project initialization mode does not match its source/destination binding")]
    ModeBindingMismatch,
    #[error("project initialization plan digest does not match its contents")]
    DigestMismatch,
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    fn source(path: Option<String>) -> ProjectInitializationSource {
        ProjectInitializationSource {
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            fingerprint: digest('a'),
            project_instance_id: "instance-a".into(),
            project_path_digest: path,
        }
    }

    #[test]
    fn adopt_plan_is_digest_bound_and_has_no_host_path() {
        let plan = ProjectInitializationPlan::build(
            Uuid::new_v4(),
            ProjectInitializationMode::AdoptActive,
            source(Some(digest('b'))),
            None,
            RevisionId(4),
            true,
            digest('c'),
        )
        .unwrap();

        plan.verify().unwrap();
        assert_eq!(plan.target_project_id(), "project-a");
        let json = serde_json::to_string(&plan).unwrap();
        assert!(!json.contains("C:\\"));
        assert!(!json.contains("destinationPath"));
    }

    #[test]
    fn save_untitled_requires_new_destination_and_absent_store() {
        let destination = ProjectInitializationDestination {
            project_id: "project-b".into(),
            path_digest: digest('d'),
            file_name: "movie.ymmp".into(),
        };
        let plan = ProjectInitializationPlan::build(
            Uuid::new_v4(),
            ProjectInitializationMode::SaveUntitled,
            source(None),
            Some(destination),
            RevisionId(0),
            false,
            digest('c'),
        )
        .unwrap();
        plan.verify().unwrap();
        assert_eq!(plan.target_project_id(), "project-b");

        let mut changed = plan;
        changed.destination.as_mut().unwrap().file_name = "other.ymmp".into();
        assert!(matches!(
            changed.verify(),
            Err(ProjectInitializationPlanError::DigestMismatch)
        ));
    }

    #[test]
    fn modes_fail_closed_on_path_state_mismatch() {
        let error = ProjectInitializationPlan::build(
            Uuid::new_v4(),
            ProjectInitializationMode::SaveUntitled,
            source(Some(digest('b'))),
            Some(ProjectInitializationDestination {
                project_id: "project-b".into(),
                path_digest: digest('d'),
                file_name: "movie.ymmp".into(),
            }),
            RevisionId(0),
            false,
            digest('c'),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProjectInitializationPlanError::ModeBindingMismatch
        ));

        let error = ProjectInitializationPlan::build(
            Uuid::new_v4(),
            ProjectInitializationMode::SaveUntitled,
            source(None),
            Some(ProjectInitializationDestination {
                project_id: "project-b".into(),
                path_digest: digest('d'),
                file_name: "movie.ymmp".into(),
            }),
            RevisionId(4),
            false,
            digest('c'),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProjectInitializationPlanError::ModeBindingMismatch
        ));
    }
}
