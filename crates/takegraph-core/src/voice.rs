use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Immutable, content-addressed generated audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioArtifact {
    pub audio_hash: String,
    pub duration_samples: u64,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Voice take lifecycle used by audition and patch review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceTakeStatus {
    QueryReady,
    Materialized,
    Candidate,
    Accepted,
    Rejected,
    Stale,
    Unavailable,
}

/// A generated take is never overwritten; regeneration creates another take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTake {
    pub id: Uuid,
    pub speech_realization_id: Uuid,
    pub input_hash: String,
    pub query_hash: String,
    pub status: VoiceTakeStatus,
    pub artifact: Option<AudioArtifact>,
}

impl VoiceTake {
    /// Creates a materialized candidate ready for A/B audition.
    #[must_use]
    pub fn candidate(
        speech_realization_id: Uuid,
        input_hash: impl Into<String>,
        query_hash: impl Into<String>,
        artifact: AudioArtifact,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            speech_realization_id,
            input_hash: input_hash.into(),
            query_hash: query_hash.into(),
            status: VoiceTakeStatus::Candidate,
            artifact: Some(artifact),
        }
    }

    /// Accepts only a generated candidate with an audio artifact.
    ///
    /// # Errors
    ///
    /// Returns an error when the take is not a candidate or has no artifact.
    pub fn accept(&mut self) -> Result<(), VoiceTakeError> {
        if self.status != VoiceTakeStatus::Candidate {
            return Err(VoiceTakeError::NotCandidate(self.status));
        }
        if self.artifact.is_none() {
            return Err(VoiceTakeError::MissingArtifact);
        }
        self.status = VoiceTakeStatus::Accepted;
        Ok(())
    }
}

/// Identity captured when an asynchronous voice generation task starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTaskIdentity {
    pub project_revision: u64,
    pub speech_input_hash: String,
    pub voice_query_hash: String,
}

impl VoiceTaskIdentity {
    /// Checks that a completed task still belongs to the current input.
    #[must_use]
    pub fn matches(
        &self,
        project_revision: u64,
        speech_input_hash: &str,
        voice_query_hash: &str,
    ) -> bool {
        self.project_revision == project_revision
            && self.speech_input_hash == speech_input_hash
            && self.voice_query_hash == voice_query_hash
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VoiceTakeError {
    #[error("voice take is not a candidate: {0:?}")]
    NotCandidate(VoiceTakeStatus),
    #[error("accepted voice take must have a materialized audio artifact")]
    MissingArtifact,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_completion_is_rejected_after_text_change() {
        let task = VoiceTaskIdentity {
            project_revision: 4,
            speech_input_hash: "old-text".into(),
            voice_query_hash: "query-a".into(),
        };

        assert!(!task.matches(5, "new-text", "query-a"));
    }
}
