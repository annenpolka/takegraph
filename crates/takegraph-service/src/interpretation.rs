//! Attach interpretation revisions to a captured transcript.
//!
//! A failed interpretation never deletes the capture or the transcript.

use takegraph_core::{AnnotationId, AnnotationIntent, AnnotationInterpretation, TemporalReference};
use takegraph_node::{
    InterpretationError, InterpretationInput, InterpretationProvider, seal_interpretation,
};
use thiserror::Error;

use crate::annotation_store::{AnnotationStore, AnnotationStoreError};

/// Store and provider errors for one interpretation attempt.
#[derive(Debug, Error)]
pub enum InterpretCaptureError {
    #[error(transparent)]
    Store(#[from] AnnotationStoreError),
    #[error(transparent)]
    Interpretation(#[from] InterpretationError),
    #[error(transparent)]
    Domain(#[from] takegraph_core::AnnotationError),
    #[error("capture {0} has no transcript to interpret")]
    MissingTranscript(String),
}

/// Runs one provider against the current transcript and attaches a revision.
///
/// # Errors
///
/// Returns a store or provider error. The capture and transcript remain
/// loadable after a failed attempt.
pub fn interpret_capture(
    store: &AnnotationStore,
    provider: &dyn InterpretationProvider,
    capture_id: AnnotationId,
) -> Result<AnnotationInterpretation, InterpretCaptureError> {
    let projection = store.capture(capture_id)?;
    let transcript = projection
        .transcript
        .clone()
        .ok_or_else(|| InterpretCaptureError::MissingTranscript(capture_id.0.to_string()))?;
    let input = InterpretationInput {
        capture_id,
        transcript_digest: transcript.transcript_digest.clone(),
        text: transcript.text.clone(),
        start_frame: projection.capture.start_anchor.frame,
        end_frame: projection.capture.end_anchor.frame,
        fps: projection.capture.start_anchor.fps,
    };
    let draft = provider.interpret(&input)?;
    let interpretation = seal_interpretation(
        capture_id,
        &transcript,
        draft.temporal,
        draft.intents,
        provider.model_id(),
        provider.model_digest()?,
    )?;
    store.attach_interpretation(interpretation.clone())?;
    Ok(interpretation)
}

/// Attaches a human-corrected interpretation as a new revision.
///
/// # Errors
///
/// Returns store or validation errors. The capture is not deleted.
pub fn attach_human_interpretation(
    store: &AnnotationStore,
    capture_id: AnnotationId,
    temporal: TemporalReference,
    intents: Vec<AnnotationIntent>,
    reviewer: &str,
) -> Result<AnnotationInterpretation, InterpretCaptureError> {
    let provider = takegraph_node::HumanInterpretation::new(reviewer, temporal, intents);
    interpret_capture(store, &provider, capture_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        AnnotationCapture, AnnotationTranscript, CaptureSessionId, CapturedAudioEvidence,
        RevisionId, SourceAnchor,
    };
    use takegraph_node::HeuristicInterpreter;
    use uuid::Uuid;

    fn capture(id: AnnotationId) -> AnnotationCapture {
        AnnotationCapture {
            id,
            session_id: CaptureSessionId::new(),
            start_anchor: SourceAnchor {
                project_id: "project-a".into(),
                scene_id: "scene-1".into(),
                source_fingerprint: "fp-1".into(),
                fps: 60,
                frame: 2531,
                observed_canonical_revision: Some(RevisionId(1)),
            },
            end_anchor: SourceAnchor {
                project_id: "project-a".into(),
                scene_id: "scene-1".into(),
                source_fingerprint: "fp-1".into(),
                fps: 60,
                frame: 2698,
                observed_canonical_revision: Some(RevisionId(1)),
            },
            audio: CapturedAudioEvidence {
                audio_sha256: format!("sha256:{}", "a".repeat(64)),
                byte_length: 32_000,
                duration_samples: 16_000,
                sample_rate: 16_000,
                channels: 1,
                bits_per_sample: 16,
            },
            captured_at_utc: "2026-08-14T13:34:57Z".into(),
        }
    }

    fn transcript(id: AnnotationId, text: &str) -> AnnotationTranscript {
        AnnotationTranscript {
            id: Uuid::new_v4(),
            capture_id: id,
            audio_sha256: format!("sha256:{}", "a".repeat(64)),
            text: text.into(),
            provider_id: "whisper-cpp".into(),
            provider_digest: format!("sha256:{}", "b".repeat(64)),
            transcript_digest: format!("sha256:{}", "c".repeat(64)),
        }
    }

    #[test]
    fn missing_transcript_preserves_capture() {
        let root = std::env::temp_dir().join(format!("takegraph-interpret-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let error = interpret_capture(&store, &HeuristicInterpreter, id);
        assert!(matches!(
            error,
            Err(InterpretCaptureError::MissingTranscript(_))
        ));
        assert!(store.capture(id).unwrap().interpretation.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn heuristic_attaches_and_human_revises() {
        let root = std::env::temp_dir().join(format!("takegraph-interpret-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        store
            .attach_transcript(transcript(
                id,
                "今のところ三秒前から残す。ここはCompressionの説明を入れる",
            ))
            .unwrap();

        let first = interpret_capture(&store, &HeuristicInterpreter, id).unwrap();
        assert_eq!(first.model_id, "heuristic-v1");
        assert!(first.intents.iter().any(|intent| matches!(
            intent,
            AnnotationIntent::Highlight { .. } | AnnotationIntent::Narration { .. }
        )));

        let revised = attach_human_interpretation(
            &store,
            id,
            first.temporal.clone(),
            vec![AnnotationIntent::Note],
            "operator",
        )
        .unwrap();
        assert_eq!(revised.model_id, "human");
        assert_eq!(revised.intents, vec![AnnotationIntent::Note]);
        assert_ne!(revised.interpretation_digest, first.interpretation_digest);
        assert_eq!(
            store.capture(id).unwrap().interpretation.unwrap().intents,
            vec![AnnotationIntent::Note]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
