//! Map a current-scene composition observation onto a [`SourceAnchor`].

use takegraph_core::{AnnotationError, RevisionId, SourceAnchor};
use takegraph_node::Ymm4SceneCompositionSnapshot;
use takegraph_service::project_store::DurableProjectStore;

use crate::error::CaptureError;

/// Composition plus the optional canonical head observed without opening
/// the project store for write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedComposition {
    pub snapshot: Ymm4SceneCompositionSnapshot,
    pub observed_canonical_revision: Option<RevisionId>,
}

/// Builds a validated source anchor from a current-frame composition.
///
/// # Errors
///
/// Returns [`AnnotationError::InvalidAnchor`] when identity, fps, or frame
/// fail the domain checks.
pub fn source_anchor_from_composition(
    snapshot: &Ymm4SceneCompositionSnapshot,
    observed_canonical_revision: Option<RevisionId>,
) -> Result<SourceAnchor, AnnotationError> {
    let anchor = SourceAnchor {
        project_id: snapshot.project_id.clone(),
        scene_id: snapshot.scene_id.clone(),
        source_fingerprint: snapshot.source_fingerprint.clone(),
        fps: snapshot.fps,
        frame: snapshot.frame,
        observed_canonical_revision,
    };
    anchor.validate()?;
    Ok(anchor)
}

/// Reads the canonical head without creating or locking the project store.
///
/// Absence, an uninitialized project, or a reserved initializer become
/// `None`. Corrupt journals surface as errors so the caller can decide.
///
/// # Errors
///
/// Returns a store error only for a corrupt published chain.
pub fn observe_canonical_revision(
    state_root: impl AsRef<std::path::Path>,
    project_id: &str,
) -> Result<Option<RevisionId>, CaptureError> {
    match DurableProjectStore::observe_scoped(state_root, project_id) {
        Ok(Some(state)) => Ok(Some(state.head)),
        Ok(None)
        | Err(
            takegraph_service::project_store::ProjectStoreError::InitializationReserved
            | takegraph_service::project_store::ProjectStoreError::NotInitialized { .. },
        ) => Ok(None),
        Err(error) => Err(CaptureError::Composition(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_node::{
        YMM4_SCENE_COMPOSITION_SCHEMA_VERSION, Ymm4CompositionAvailability,
        Ymm4CompositionCompleteness, Ymm4CompositionViewport,
    };

    fn snapshot(frame: i32) -> Ymm4SceneCompositionSnapshot {
        Ymm4SceneCompositionSnapshot {
            schema_version: YMM4_SCENE_COMPOSITION_SCHEMA_VERSION,
            project_id: "project-a".into(),
            scene_id: "scene-1".into(),
            source_fingerprint: "sha256:source-a".into(),
            fps: 30,
            frame,
            viewport: Ymm4CompositionViewport {
                availability: Ymm4CompositionAvailability::Unavailable,
                width: None,
                height: None,
            },
            elements: Vec::new(),
            completeness: Ymm4CompositionCompleteness::Partial,
            unavailable_fields: vec!["elements".into(), "viewport".into()],
        }
    }

    #[test]
    fn maps_validated_composition_fields() {
        let anchor = source_anchor_from_composition(&snapshot(2531), Some(RevisionId(4))).unwrap();
        assert_eq!(anchor.project_id, "project-a");
        assert_eq!(anchor.scene_id, "scene-1");
        assert_eq!(anchor.source_fingerprint, "sha256:source-a");
        assert_eq!(anchor.fps, 30);
        assert_eq!(anchor.frame, 2531);
        assert_eq!(anchor.observed_canonical_revision, Some(RevisionId(4)));
    }

    #[test]
    fn rejects_negative_frame_from_composition() {
        let mut invalid = snapshot(0);
        invalid.frame = -1;
        assert!(source_anchor_from_composition(&invalid, None).is_err());
    }
}
