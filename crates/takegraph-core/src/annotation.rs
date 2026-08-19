//! Voice annotation capture domain.
//!
//! An annotation capture is immutable evidence of something a human said
//! while watching a source: raw microphone audio plus the source position it
//! refers to. Transcripts and interpretations are revisable derivations and
//! are never stored inside the capture. Captured human voice is distinct
//! from generated speech and never becomes a [`crate::VoiceTake`].

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::RevisionId;

/// Stable annotation capture identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnnotationId(pub Uuid);

impl AnnotationId {
    /// Creates a fresh capture identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for AnnotationId {
    fn default() -> Self {
        Self::new()
    }
}

/// Groups captures recorded during one viewing session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CaptureSessionId(pub Uuid);

impl CaptureSessionId {
    /// Creates a fresh session identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CaptureSessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// The validated source identity a capture is bound to.
///
/// Mirrors the validated fields of a `current_scene_composition()`
/// observation so a frame refers to the exact source state that was on
/// screen when the human spoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceAnchor {
    pub project_id: String,
    pub scene_id: String,
    pub source_fingerprint: String,
    pub fps: u32,
    pub frame: i32,
    /// Canonical revision observed at capture time, when the caller could
    /// read one. Absence must not be interpreted as revision zero.
    pub observed_canonical_revision: Option<RevisionId>,
}

impl SourceAnchor {
    /// Validates identity, fingerprint portability, fps, and frame.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationError::InvalidAnchor`] with the offending field.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        require_non_empty(&self.project_id, "projectId")?;
        require_non_empty(&self.scene_id, "sceneId")?;
        if self.source_fingerprint.is_empty()
            || self.source_fingerprint.chars().any(char::is_whitespace)
        {
            return Err(AnnotationError::InvalidAnchor {
                field: "sourceFingerprint".into(),
                reason: "must be a non-empty fingerprint without whitespace".into(),
            });
        }
        if self.fps == 0 {
            return Err(AnnotationError::InvalidAnchor {
                field: "fps".into(),
                reason: "must be greater than zero".into(),
            });
        }
        if self.frame < 0 {
            return Err(AnnotationError::InvalidAnchor {
                field: "frame".into(),
                reason: "must be zero or greater".into(),
            });
        }
        Ok(())
    }

    /// Returns whether both anchors observe the same project, scene, and
    /// source fingerprint. Frame may differ: playback continues while the
    /// human speaks.
    #[must_use]
    pub fn same_source(&self, other: &Self) -> bool {
        self.project_id == other.project_id
            && self.scene_id == other.scene_id
            && self.source_fingerprint == other.source_fingerprint
            && self.fps == other.fps
    }
}

/// Content-addressed raw microphone audio evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturedAudioEvidence {
    pub audio_sha256: String,
    pub byte_length: u64,
    pub duration_samples: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl CapturedAudioEvidence {
    /// Validates digest shape and PCM metadata consistency.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationError::InvalidAudio`] for an empty or malformed
    /// digest, non-positive PCM fields, or a byte length that cannot hold
    /// the claimed samples.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        if !is_sha256_digest(&self.audio_sha256) {
            return Err(AnnotationError::InvalidAudio(
                "audioSha256 must be a sha256:<hex> digest".into(),
            ));
        }
        if self.sample_rate == 0 {
            return Err(AnnotationError::InvalidAudio(
                "sampleRate must be greater than zero".into(),
            ));
        }
        if self.channels == 0 {
            return Err(AnnotationError::InvalidAudio(
                "channels must be greater than zero".into(),
            ));
        }
        if self.bits_per_sample == 0 || !self.bits_per_sample.is_multiple_of(8) {
            return Err(AnnotationError::InvalidAudio(
                "bitsPerSample must be a positive multiple of eight".into(),
            ));
        }
        let bytes_per_frame = u64::from(self.channels) * u64::from(self.bits_per_sample / 8);
        let required = self.duration_samples.checked_mul(bytes_per_frame).ok_or(
            AnnotationError::InvalidAudio("durationSamples overflows the byte length".into()),
        )?;
        if self.byte_length < required {
            return Err(AnnotationError::InvalidAudio(
                "byteLength cannot hold durationSamples at the declared PCM layout".into(),
            ));
        }
        Ok(())
    }
}

/// Classification of the start and end anchors of one capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStability {
    /// Both anchors observe the same project, scene, and fingerprint.
    Stable,
    /// The observed source changed while the human spoke. The audio is kept
    /// as evidence, but automatic placement and promotion must refuse.
    SourceChanged,
}

/// Immutable human voice observation recorded during playback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnotationCapture {
    pub id: AnnotationId,
    pub session_id: CaptureSessionId,
    pub start_anchor: SourceAnchor,
    pub end_anchor: SourceAnchor,
    pub audio: CapturedAudioEvidence,
    /// RFC 3339 UTC timestamp of the capture commit.
    pub captured_at_utc: String,
}

impl AnnotationCapture {
    /// Validates anchors, audio metadata, and anchor consistency, then
    /// classifies the capture.
    ///
    /// # Errors
    ///
    /// Returns an error when any anchor or audio field is invalid, or when
    /// the two anchors disagree on direction (end before start frame on the
    /// same source is tolerated because seeks may pause or rewind while a
    /// human speaks; identity disagreement is reported as `SourceChanged`,
    /// not as an error).
    pub fn validate_and_classify(&self) -> Result<CaptureStability, AnnotationError> {
        self.start_anchor.validate()?;
        self.end_anchor.validate()?;
        self.audio.validate()?;
        if self.captured_at_utc.trim().is_empty() {
            return Err(AnnotationError::InvalidAnchor {
                field: "capturedAtUtc".into(),
                reason: "must not be empty".into(),
            });
        }
        if self.start_anchor.same_source(&self.end_anchor) {
            Ok(CaptureStability::Stable)
        } else {
            Ok(CaptureStability::SourceChanged)
        }
    }
}

/// One machine derivation of a capture's audio, produced by a specific ASR
/// provider run. Corrections and re-runs append new revisions; they never
/// overwrite prior ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnotationTranscript {
    pub id: Uuid,
    pub capture_id: AnnotationId,
    /// Exact audio digest this transcript was derived from.
    pub audio_sha256: String,
    pub text: String,
    pub provider_id: String,
    /// Digest over the provider identity, executable, model, and parameters
    /// so the same audio can be re-derived deterministically.
    pub provider_digest: String,
    pub transcript_digest: String,
}

impl AnnotationTranscript {
    /// Validates identifiers, digest shapes, and binding fields.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationError::InvalidTranscript`] for malformed fields.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        if self.id.is_nil() {
            return Err(AnnotationError::InvalidTranscript(
                "id must not be nil".into(),
            ));
        }
        if self.capture_id.0.is_nil() {
            return Err(AnnotationError::InvalidTranscript(
                "captureId must not be nil".into(),
            ));
        }
        if !is_sha256_digest(&self.audio_sha256) {
            return Err(AnnotationError::InvalidTranscript(
                "audioSha256 must be a sha256:<hex> digest".into(),
            ));
        }
        if self.text.trim().is_empty() {
            return Err(AnnotationError::InvalidTranscript(
                "text must not be empty".into(),
            ));
        }
        if self.provider_id.trim().is_empty() {
            return Err(AnnotationError::InvalidTranscript(
                "providerId must not be empty".into(),
            ));
        }
        if !is_sha256_digest(&self.provider_digest) {
            return Err(AnnotationError::InvalidTranscript(
                "providerDigest must be a sha256:<hex> digest".into(),
            ));
        }
        if !is_sha256_digest(&self.transcript_digest) {
            return Err(AnnotationError::InvalidTranscript(
                "transcriptDigest must be a sha256:<hex> digest".into(),
            ));
        }
        Ok(())
    }
}

/// Relative placement of an interpretation against the capture start frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemporalReference {
    pub reference_frame: i32,
    pub start_offset_frames: i32,
    pub end_offset_frames: Option<i32>,
    pub relation: TemporalRelation,
}

/// How a spoken note sits relative to the captured frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalRelation {
    Before,
    At,
    After,
    Range,
}

impl TemporalReference {
    /// Validates frame polarity and range ordering.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationError::InvalidInterpretation`] when the reference
    /// frame is negative or a range is inverted / missing its end.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        if self.reference_frame < 0 {
            return Err(AnnotationError::InvalidInterpretation(
                "temporal referenceFrame must be zero or greater".into(),
            ));
        }
        if self.relation == TemporalRelation::Range && self.end_offset_frames.is_none() {
            return Err(AnnotationError::InvalidInterpretation(
                "range temporal reference requires endOffsetFrames".into(),
            ));
        }
        if let Some(end) = self.end_offset_frames {
            if end < self.start_offset_frames {
                return Err(AnnotationError::InvalidInterpretation(
                    "endOffsetFrames must be greater than or equal to startOffsetFrames".into(),
                ));
            }
        }
        Ok(())
    }
}

/// One structured intent candidate derived from an exact transcript digest.
/// This is an AI candidate, never capture evidence and never an edit patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnotationInterpretation {
    pub id: Uuid,
    pub capture_id: AnnotationId,
    /// Exact transcript digest this interpretation was derived from.
    pub transcript_digest: String,
    pub temporal: TemporalReference,
    pub intents: Vec<AnnotationIntent>,
    pub model_id: String,
    pub model_digest: String,
    pub interpretation_digest: String,
}

impl AnnotationInterpretation {
    /// Validates identifiers, digest shapes, and the model binding.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationError::InvalidInterpretation`] for malformed
    /// fields.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        if self.id.is_nil() {
            return Err(AnnotationError::InvalidInterpretation(
                "id must not be nil".into(),
            ));
        }
        if self.capture_id.0.is_nil() {
            return Err(AnnotationError::InvalidInterpretation(
                "captureId must not be nil".into(),
            ));
        }
        if !is_sha256_digest(&self.transcript_digest) {
            return Err(AnnotationError::InvalidInterpretation(
                "transcriptDigest must be a sha256:<hex> digest".into(),
            ));
        }
        self.temporal.validate()?;
        if self.intents.is_empty() {
            return Err(AnnotationError::InvalidInterpretation(
                "at least one intent is required".into(),
            ));
        }
        if self.model_id.trim().is_empty() {
            return Err(AnnotationError::InvalidInterpretation(
                "modelId must not be empty".into(),
            ));
        }
        if !is_sha256_digest(&self.model_digest) {
            return Err(AnnotationError::InvalidInterpretation(
                "modelDigest must be a sha256:<hex> digest".into(),
            ));
        }
        if !is_sha256_digest(&self.interpretation_digest) {
            return Err(AnnotationError::InvalidInterpretation(
                "interpretationDigest must be a sha256:<hex> digest".into(),
            ));
        }
        for intent in &self.intents {
            intent.validate()?;
        }
        Ok(())
    }
}

/// Structured intent extracted from a transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationIntent {
    Note,
    Highlight {
        reason: Option<String>,
    },
    Narration {
        topic: String,
        draft_hint: Option<String>,
    },
    CutCandidate {
        reason: Option<String>,
    },
    Verify {
        question: String,
    },
}

impl AnnotationIntent {
    /// Validates variant payloads.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationError::InvalidInterpretation`] for empty topics
    /// or questions.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        match self {
            Self::Note | Self::Highlight { .. } | Self::CutCandidate { .. } => Ok(()),
            Self::Narration { topic, .. } => {
                if topic.trim().is_empty() {
                    return Err(AnnotationError::InvalidInterpretation(
                        "narration topic must not be empty".into(),
                    ));
                }
                Ok(())
            }
            Self::Verify { question } => {
                if question.trim().is_empty() {
                    return Err(AnnotationError::InvalidInterpretation(
                        "verify question must not be empty".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

/// Provenance block sealed into a promoted edit plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceEvidenceRef {
    pub annotation_id: AnnotationId,
    pub capture_audio_sha256: String,
    pub transcript_digest: String,
    pub interpretation_digest: String,
}

impl SourceEvidenceRef {
    /// Validates identifiers and digest shapes.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationError::InvalidSourceEvidence`] for a nil capture
    /// or a malformed digest.
    pub fn validate(&self) -> Result<(), AnnotationError> {
        if self.annotation_id.0.is_nil() {
            return Err(AnnotationError::InvalidSourceEvidence(
                "annotationId must not be nil".into(),
            ));
        }
        if !is_sha256_digest(&self.capture_audio_sha256) {
            return Err(AnnotationError::InvalidSourceEvidence(
                "captureAudioSha256 must be a sha256:<hex> digest".into(),
            ));
        }
        if !is_sha256_digest(&self.transcript_digest) {
            return Err(AnnotationError::InvalidSourceEvidence(
                "transcriptDigest must be a sha256:<hex> digest".into(),
            ));
        }
        if !is_sha256_digest(&self.interpretation_digest) {
            return Err(AnnotationError::InvalidSourceEvidence(
                "interpretationDigest must be a sha256:<hex> digest".into(),
            ));
        }
        Ok(())
    }
}

fn require_non_empty(value: &str, field: &str) -> Result<(), AnnotationError> {
    if value.trim().is_empty() {
        return Err(AnnotationError::InvalidAnchor {
            field: field.into(),
            reason: "must not be empty".into(),
        });
    }
    Ok(())
}

fn is_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnnotationError {
    #[error("invalid source anchor field {field}: {reason}")]
    InvalidAnchor { field: String, reason: String },
    #[error("invalid captured audio evidence: {0}")]
    InvalidAudio(String),
    #[error("invalid transcript: {0}")]
    InvalidTranscript(String),
    #[error("invalid interpretation: {0}")]
    InvalidInterpretation(String),
    #[error("invalid source evidence: {0}")]
    InvalidSourceEvidence(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(frame: i32) -> SourceAnchor {
        SourceAnchor {
            project_id: "project-a".into(),
            scene_id: "scene-1".into(),
            source_fingerprint: "fp-1".into(),
            fps: 30,
            frame,
            observed_canonical_revision: Some(RevisionId(4)),
        }
    }

    fn audio() -> CapturedAudioEvidence {
        CapturedAudioEvidence {
            audio_sha256: format!("sha256:{}", "a".repeat(64)),
            byte_length: 32_000,
            duration_samples: 15_000,
            sample_rate: 16_000,
            channels: 1,
            bits_per_sample: 16,
        }
    }

    fn capture() -> AnnotationCapture {
        AnnotationCapture {
            id: AnnotationId::new(),
            session_id: CaptureSessionId::new(),
            start_anchor: anchor(2531),
            end_anchor: anchor(2698),
            audio: audio(),
            captured_at_utc: "2026-08-14T13:34:57Z".into(),
        }
    }

    #[test]
    fn rejects_negative_frame() {
        let mut instance = capture();
        instance.start_anchor.frame = -1;
        assert!(matches!(
            instance.validate_and_classify(),
            Err(AnnotationError::InvalidAnchor { field, .. }) if field == "frame"
        ));
    }

    #[test]
    fn rejects_zero_fps() {
        let mut instance = capture();
        instance.end_anchor.fps = 0;
        assert!(matches!(
            instance.validate_and_classify(),
            Err(AnnotationError::InvalidAnchor { field, .. }) if field == "fps"
        ));
    }

    #[test]
    fn rejects_empty_hash() {
        let mut instance = capture();
        instance.audio.audio_sha256 = String::new();
        assert!(matches!(
            instance.validate_and_classify(),
            Err(AnnotationError::InvalidAudio(_))
        ));
    }

    #[test]
    fn rejects_audio_byte_length_shorter_than_samples() {
        let mut instance = capture();
        instance.audio.byte_length = 100;
        assert!(matches!(
            instance.validate_and_classify(),
            Err(AnnotationError::InvalidAudio(_))
        ));
    }

    #[test]
    fn same_source_later_frame_is_stable() {
        let instance = capture();
        assert_eq!(
            instance.validate_and_classify(),
            Ok(CaptureStability::Stable)
        );
    }

    #[test]
    fn changed_fingerprint_is_source_changed_not_rejected() {
        let mut instance = capture();
        instance.end_anchor.source_fingerprint = "fp-2".into();
        assert_eq!(
            instance.validate_and_classify(),
            Ok(CaptureStability::SourceChanged)
        );
    }

    #[test]
    fn changed_project_is_source_changed() {
        let mut instance = capture();
        instance.end_anchor.project_id = "project-b".into();
        assert_eq!(
            instance.validate_and_classify(),
            Ok(CaptureStability::SourceChanged)
        );
    }

    #[test]
    fn transcript_requires_sha256_binding() {
        let instance = AnnotationTranscript {
            id: Uuid::new_v4(),
            capture_id: AnnotationId::new(),
            audio_sha256: "not-a-digest".into(),
            text: "今のところ残す".into(),
            provider_id: "whisper-cpp".into(),
            provider_digest: format!("sha256:{}", "b".repeat(64)),
            transcript_digest: format!("sha256:{}", "c".repeat(64)),
        };
        assert!(instance.validate().is_err());
    }

    #[test]
    fn interpretation_requires_intents() {
        let instance = AnnotationInterpretation {
            id: Uuid::new_v4(),
            capture_id: AnnotationId::new(),
            transcript_digest: format!("sha256:{}", "c".repeat(64)),
            temporal: TemporalReference {
                reference_frame: 2531,
                start_offset_frames: 0,
                end_offset_frames: None,
                relation: TemporalRelation::At,
            },
            intents: Vec::new(),
            model_id: "test-model".into(),
            model_digest: format!("sha256:{}", "d".repeat(64)),
            interpretation_digest: format!("sha256:{}", "e".repeat(64)),
        };
        assert!(instance.validate().is_err());
    }

    #[test]
    fn range_temporal_requires_ordered_end() {
        let temporal = TemporalReference {
            reference_frame: 10,
            start_offset_frames: 0,
            end_offset_frames: Some(-4),
            relation: TemporalRelation::Range,
        };
        assert!(temporal.validate().is_err());
    }
}
