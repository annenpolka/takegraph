//! User-managed speech-to-text adapters.
//!
//! Transcription derives text from captured audio. A failure must leave the
//! capture intact so another provider or a human correction can retry.

use std::path::Path;

use async_trait::async_trait;
use takegraph_core::{
    AnnotationError, AnnotationId, AnnotationTranscript, CapturedAudioEvidence, canonical_sha256,
};
use thiserror::Error;
use uuid::Uuid;

/// Result of one successful ASR run. Binding and digest sealing happen in
/// [`seal_transcript`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptResult {
    pub text: String,
}

/// Provider-side transcription failure. This never deletes capture evidence.
#[derive(Debug, Error)]
pub enum TranscriptionError {
    /// The executable, model, or audio file is missing or unreadable.
    #[error("transcription input is unavailable: {0}")]
    Unavailable(String),
    /// The provider ran but produced no usable text.
    #[error("transcription produced no text: {0}")]
    Empty(String),
    /// The provider process or I/O failed.
    #[error("transcription failed: {0}")]
    Failed(String),
    /// Digest construction failed.
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
}

/// One ASR backend. Implementations must not mutate the annotation store.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Stable provider identity recorded on the transcript (`whisper-cpp`,
    /// `human`, …).
    fn provider_id(&self) -> &str;

    /// Digest over executable, model, language, and parameters.
    ///
    /// # Errors
    ///
    /// Returns [`TranscriptionError::Unavailable`] when provider files cannot
    /// be hashed.
    fn provider_digest(&self) -> Result<String, TranscriptionError>;

    /// Derives text from one captured WAV. Must not delete or rewrite `audio_path`.
    ///
    /// # Errors
    ///
    /// Returns a provider failure. Empty text is [`TranscriptionError::Empty`].
    async fn transcribe(
        &self,
        audio: &CapturedAudioEvidence,
        audio_path: &Path,
    ) -> Result<TranscriptResult, TranscriptionError>;
}

/// Seals a validated transcript revision bound to the captured audio.
///
/// # Errors
///
/// Returns [`AnnotationError::InvalidTranscript`] for empty text or a digest
/// construction failure.
pub fn seal_transcript(
    capture_id: AnnotationId,
    audio: &CapturedAudioEvidence,
    text: impl Into<String>,
    provider_id: impl Into<String>,
    provider_digest: impl Into<String>,
) -> Result<AnnotationTranscript, AnnotationError> {
    let text = text.into();
    let provider_id = provider_id.into();
    let provider_digest = provider_digest.into();
    let transcript_digest = canonical_sha256(
        "takegraph-annotation-transcript-v1",
        &(
            capture_id,
            audio.audio_sha256.as_str(),
            text.as_str(),
            provider_id.as_str(),
            provider_digest.as_str(),
        ),
    )
    .map_err(|error| AnnotationError::InvalidTranscript(error.to_string()))?;
    let transcript = AnnotationTranscript {
        id: Uuid::new_v4(),
        capture_id,
        audio_sha256: audio.audio_sha256.clone(),
        text,
        provider_id,
        provider_digest,
        transcript_digest,
    };
    transcript.validate()?;
    Ok(transcript)
}

/// In-memory provider used by unit tests.
pub struct ScriptedTranscription {
    provider_id: String,
    provider_digest: String,
    result: Result<TranscriptResult, String>,
}

impl ScriptedTranscription {
    /// Succeeds with `text`.
    #[must_use]
    pub fn succeeding(text: impl Into<String>) -> Self {
        Self {
            provider_id: "scripted".into(),
            provider_digest: format!("sha256:{}", "b".repeat(64)),
            result: Ok(TranscriptResult { text: text.into() }),
        }
    }

    /// Fails without producing text.
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            provider_id: "scripted".into(),
            provider_digest: format!("sha256:{}", "b".repeat(64)),
            result: Err(message.into()),
        }
    }

    /// Overrides the recorded provider digest.
    #[must_use]
    pub fn with_digest(mut self, digest: impl Into<String>) -> Self {
        self.provider_digest = digest.into();
        self
    }
}

#[async_trait]
impl TranscriptionProvider for ScriptedTranscription {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn provider_digest(&self) -> Result<String, TranscriptionError> {
        Ok(self.provider_digest.clone())
    }

    async fn transcribe(
        &self,
        _audio: &CapturedAudioEvidence,
        _audio_path: &Path,
    ) -> Result<TranscriptResult, TranscriptionError> {
        self.result.clone().map_err(TranscriptionError::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> CapturedAudioEvidence {
        CapturedAudioEvidence {
            audio_sha256: format!("sha256:{}", "a".repeat(64)),
            byte_length: 32_000,
            duration_samples: 16_000,
            sample_rate: 16_000,
            channels: 1,
            bits_per_sample: 16,
        }
    }

    #[test]
    fn sealed_transcript_binds_audio_and_provider() {
        let capture = AnnotationId::new();
        let first = seal_transcript(
            capture,
            &audio(),
            "今のところ残す",
            "whisper-cpp",
            format!("sha256:{}", "c".repeat(64)),
        )
        .unwrap();
        let second = seal_transcript(
            capture,
            &audio(),
            "今のところ残す",
            "whisper-cpp",
            format!("sha256:{}", "d".repeat(64)),
        )
        .unwrap();
        assert_ne!(first.transcript_digest, second.transcript_digest);
        assert_eq!(first.audio_sha256, audio().audio_sha256);
    }

    #[tokio::test]
    async fn scripted_failure_does_not_invent_text() {
        let provider = ScriptedTranscription::failing("asr down");
        let error = provider
            .transcribe(&audio(), Path::new("missing.wav"))
            .await;
        assert!(matches!(error, Err(TranscriptionError::Failed(_))));
    }
}
