//! Durable transcription jobs stored beside the annotation journal.
//!
//! A failed job never deletes the capture. Retry uses the same job record or
//! a new enqueue.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use takegraph_core::{AnnotationId, AnnotationTranscript};
use takegraph_node::{
    TranscriptResult, TranscriptionError, TranscriptionProvider, seal_transcript,
};
use thiserror::Error;
use uuid::Uuid;

use crate::annotation_store::{AnnotationStore, AnnotationStoreError};

const JOB_PREFIX: &str = "job-";
const JOB_SUFFIX: &str = ".json";

/// Lifecycle of one transcription attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionJobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl TranscriptionJobStatus {
    /// Path-free inspect/list token for this attempt.
    #[must_use]
    pub const fn as_phase(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

/// One persistable ASR attempt. Host-local executable and model paths are for
/// retry only and must never be copied into a model-facing inspect view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptionJob {
    pub id: Uuid,
    pub capture_id: AnnotationId,
    pub audio_sha256: String,
    pub provider_id: String,
    pub provider_digest: Option<String>,
    pub language: String,
    pub status: TranscriptionJobStatus,
    pub error: Option<String>,
    pub transcript_id: Option<Uuid>,
    pub executable: PathBuf,
    pub model: PathBuf,
    pub extra_args: Vec<String>,
    pub updated_at_utc: String,
}

/// Project-scoped job directory under an annotation store.
#[derive(Debug, Clone)]
pub struct TranscriptionJobStore {
    root: PathBuf,
}

/// Job-store and transcription orchestration errors.
#[derive(Debug, Error)]
pub enum TranscriptionJobError {
    #[error(transparent)]
    Store(#[from] AnnotationStoreError),
    #[error(transparent)]
    Transcription(#[from] TranscriptionError),
    #[error(transparent)]
    Domain(#[from] takegraph_core::AnnotationError),
    #[error("transcription job I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("transcription job payload could not be decoded: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("transcription job {0} is unknown")]
    UnknownJob(Uuid),
    #[error("captured audio artifact is missing")]
    MissingAudio,
}

impl TranscriptionJobStore {
    /// Opens the job directory beside an annotation journal.
    #[must_use]
    pub fn open(store: &AnnotationStore) -> Self {
        Self {
            root: store.directory().join("transcription-jobs"),
        }
    }

    /// Records a queued attempt.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the job file cannot be published.
    pub fn enqueue(
        &self,
        job: TranscriptionJob,
    ) -> Result<TranscriptionJob, TranscriptionJobError> {
        self.write_job(&job)?;
        Ok(job)
    }

    /// Loads one job.
    ///
    /// # Errors
    ///
    /// Returns [`TranscriptionJobError::UnknownJob`] when the file is missing.
    pub fn get(&self, job_id: Uuid) -> Result<TranscriptionJob, TranscriptionJobError> {
        let path = self.job_path(job_id);
        if !path.is_file() {
            return Err(TranscriptionJobError::UnknownJob(job_id));
        }
        let mut bytes = Vec::new();
        File::open(path)?.read_to_end(&mut bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Latest job for a capture, if any.
    ///
    /// # Errors
    ///
    /// Returns I/O or decode errors.
    pub fn latest_for_capture(
        &self,
        capture_id: AnnotationId,
    ) -> Result<Option<TranscriptionJob>, TranscriptionJobError> {
        let mut latest: Option<TranscriptionJob> = None;
        for job in self.all()? {
            if job.capture_id != capture_id {
                continue;
            }
            if latest
                .as_ref()
                .is_none_or(|current| current.updated_at_utc <= job.updated_at_utc)
            {
                latest = Some(job);
            }
        }
        Ok(latest)
    }

    fn all(&self) -> Result<Vec<TranscriptionJob>, TranscriptionJobError> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if !name.starts_with(JOB_PREFIX) || !name.ends_with(JOB_SUFFIX) {
                continue;
            }
            let mut bytes = Vec::new();
            File::open(&path)?.read_to_end(&mut bytes)?;
            jobs.push(serde_json::from_slice(&bytes)?);
        }
        Ok(jobs)
    }

    fn write_job(&self, job: &TranscriptionJob) -> Result<(), TranscriptionJobError> {
        fs::create_dir_all(&self.root)?;
        let final_path = self.job_path(job.id);
        let temporary = self.root.join(format!(".job-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<(), TranscriptionJobError> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(job)?)?;
            file.sync_all()?;
            fs::rename(&temporary, final_path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn job_path(&self, job_id: Uuid) -> PathBuf {
        self.root.join(format!("{JOB_PREFIX}{job_id}{JOB_SUFFIX}"))
    }
}

/// Runs one provider against a capture and attaches a transcript on success.
///
/// # Errors
///
/// Returns store, I/O, or provider errors. The capture remains loadable after
/// a failed attempt.
pub async fn transcribe_capture(
    store: &AnnotationStore,
    jobs: &TranscriptionJobStore,
    provider: &dyn TranscriptionProvider,
    capture_id: AnnotationId,
    audio_path: &Path,
    language: &str,
    executable: PathBuf,
    model: PathBuf,
    extra_args: Vec<String>,
    now_utc: &str,
) -> Result<AnnotationTranscript, TranscriptionJobError> {
    let projection = store.capture(capture_id)?;
    let audio = projection.capture.audio.clone();
    let mut job = TranscriptionJob {
        id: Uuid::new_v4(),
        capture_id,
        audio_sha256: audio.audio_sha256.clone(),
        provider_id: provider.provider_id().to_owned(),
        provider_digest: provider.provider_digest().ok(),
        language: language.to_owned(),
        status: TranscriptionJobStatus::Running,
        error: None,
        transcript_id: None,
        executable,
        model,
        extra_args,
        updated_at_utc: now_utc.to_owned(),
    };
    jobs.enqueue(job.clone())?;
    if !audio_path.is_file() {
        job.status = TranscriptionJobStatus::Failed;
        job.error = Some("captured audio artifact is missing".into());
        job.updated_at_utc = now_utc.to_owned();
        jobs.enqueue(job)?;
        return Err(TranscriptionJobError::MissingAudio);
    }
    match provider.transcribe(&audio, audio_path).await {
        Ok(TranscriptResult { text }) => {
            let digest = provider.provider_digest()?;
            let transcript =
                seal_transcript(capture_id, &audio, text, provider.provider_id(), digest)?;
            store.attach_transcript(transcript.clone())?;
            job.status = TranscriptionJobStatus::Succeeded;
            job.provider_digest = Some(transcript.provider_digest.clone());
            job.transcript_id = Some(transcript.id);
            job.error = None;
            job.updated_at_utc = now_utc.to_owned();
            jobs.enqueue(job)?;
            Ok(transcript)
        }
        Err(error) => {
            job.status = TranscriptionJobStatus::Failed;
            job.error = Some(error.to_string());
            job.updated_at_utc = now_utc.to_owned();
            jobs.enqueue(job)?;
            Err(error.into())
        }
    }
}

/// Attaches a human-corrected transcript as a new revision.
///
/// # Errors
///
/// Returns store or validation errors. The capture is not deleted.
pub fn attach_human_transcript(
    store: &AnnotationStore,
    jobs: &TranscriptionJobStore,
    capture_id: AnnotationId,
    text: &str,
    reviewer: &str,
    now_utc: &str,
) -> Result<AnnotationTranscript, TranscriptionJobError> {
    let projection = store.capture(capture_id)?;
    let provider_digest = takegraph_core::canonical_sha256(
        "takegraph-transcription-provider-v1",
        &("human", reviewer),
    )
    .map_err(|error| TranscriptionJobError::Transcription(TranscriptionError::from(error)))?;
    let transcript = seal_transcript(
        capture_id,
        &projection.capture.audio,
        text,
        "human",
        provider_digest.clone(),
    )?;
    store.attach_transcript(transcript.clone())?;
    let job = TranscriptionJob {
        id: Uuid::new_v4(),
        capture_id,
        audio_sha256: projection.capture.audio.audio_sha256,
        provider_id: "human".into(),
        provider_digest: Some(provider_digest),
        language: String::new(),
        status: TranscriptionJobStatus::Succeeded,
        error: None,
        transcript_id: Some(transcript.id),
        executable: PathBuf::new(),
        model: PathBuf::new(),
        extra_args: Vec::new(),
        updated_at_utc: now_utc.to_owned(),
    };
    jobs.enqueue(job)?;
    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        AnnotationCapture, CaptureSessionId, CapturedAudioEvidence, RevisionId, SourceAnchor,
    };
    use takegraph_node::ScriptedTranscription;

    fn capture(id: AnnotationId) -> AnnotationCapture {
        AnnotationCapture {
            id,
            session_id: CaptureSessionId::new(),
            start_anchor: SourceAnchor {
                project_id: "project-a".into(),
                scene_id: "scene-1".into(),
                source_fingerprint: "fp-1".into(),
                fps: 30,
                frame: 10,
                observed_canonical_revision: Some(RevisionId(1)),
            },
            end_anchor: SourceAnchor {
                project_id: "project-a".into(),
                scene_id: "scene-1".into(),
                source_fingerprint: "fp-1".into(),
                fps: 30,
                frame: 20,
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

    #[tokio::test]
    async fn success_attaches_and_failure_preserves_capture() {
        let root = std::env::temp_dir().join(format!("takegraph-asr-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let jobs = TranscriptionJobStore::open(&store);
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let wav = store.directory().join("note.wav");
        fs::create_dir_all(store.directory()).unwrap();
        fs::write(&wav, b"RIFF").unwrap();

        let failed = transcribe_capture(
            &store,
            &jobs,
            &ScriptedTranscription::failing("asr down"),
            id,
            &wav,
            "ja",
            PathBuf::from("whisper"),
            PathBuf::from("model.bin"),
            Vec::new(),
            "2026-08-14T13:34:57Z",
        )
        .await;
        assert!(failed.is_err());
        assert!(store.capture(id).unwrap().transcript.is_none());
        assert_eq!(
            jobs.latest_for_capture(id).unwrap().unwrap().status,
            TranscriptionJobStatus::Failed
        );

        let attached = transcribe_capture(
            &store,
            &jobs,
            &ScriptedTranscription::succeeding("今のところ残す"),
            id,
            &wav,
            "ja",
            PathBuf::from("whisper"),
            PathBuf::from("model.bin"),
            Vec::new(),
            "2026-08-14T13:35:00Z",
        )
        .await
        .unwrap();
        assert_eq!(
            store.capture(id).unwrap().transcript.unwrap().text,
            "今のところ残す"
        );
        assert_eq!(attached.audio_sha256, format!("sha256:{}", "a".repeat(64)));

        let revised = transcribe_capture(
            &store,
            &jobs,
            &ScriptedTranscription::succeeding("今のところ残す")
                .with_digest(format!("sha256:{}", "c".repeat(64))),
            id,
            &wav,
            "ja",
            PathBuf::from("whisper"),
            PathBuf::from("model.bin"),
            Vec::new(),
            "2026-08-14T13:36:00Z",
        )
        .await
        .unwrap();
        assert_ne!(revised.transcript_digest, attached.transcript_digest);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_audio_fails_without_deleting_capture() {
        let root = std::env::temp_dir().join(format!("takegraph-asr-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let jobs = TranscriptionJobStore::open(&store);
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime.block_on(transcribe_capture(
            &store,
            &jobs,
            &ScriptedTranscription::succeeding("unused"),
            id,
            &store.directory().join("missing.wav"),
            "ja",
            PathBuf::new(),
            PathBuf::new(),
            Vec::new(),
            "2026-08-14T13:34:57Z",
        ));
        assert!(matches!(error, Err(TranscriptionJobError::MissingAudio)));
        assert!(store.capture(id).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
