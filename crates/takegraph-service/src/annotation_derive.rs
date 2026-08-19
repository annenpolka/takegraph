//! Digest-bound annotation transcription and interpretation plans.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use takegraph_core::{AnnotationId, AnnotationIntent, canonical_sha256};
use takegraph_node::{HeuristicInterpreter, HumanInterpretation, WhisperCppProvider};
use thiserror::Error;
use uuid::Uuid;

use crate::annotation_store::{AnnotationStore, AnnotationStoreError};
use crate::interpret_capture;
use crate::interpretation::InterpretCaptureError;
use crate::transcription_config::{
    TranscriptionConfigError, resolve_transcription_host, whisper_host_configured,
};
use crate::transcription_jobs::{
    TranscriptionJobError, TranscriptionJobStore, attach_human_transcript, transcribe_capture,
};

const PLAN_PREFIX: &str = "derive-";
const PLAN_SUFFIX: &str = ".json";

/// One sealed derivation against an existing capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationDeriveMode {
    Transcribe,
    Interpret,
    Correct,
}

/// Lifecycle of one derive plan. This never advances the canonical head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationDerivePhase {
    Staged,
    Completed,
    Failed,
}

/// Persistable, path-free derive plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnotationDerivePlan {
    pub handle: Uuid,
    pub capture_id: AnnotationId,
    pub project_id: String,
    pub mode: AnnotationDeriveMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intents: Option<Vec<AnnotationIntent>>,
    pub plan_digest: String,
    pub phase: AnnotationDerivePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Public, path-free report for MCP and CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationDeriveReport {
    pub handle: String,
    pub capture_id: String,
    pub mode: AnnotationDeriveMode,
    pub phase: AnnotationDerivePhase,
    pub plan_digest: String,
    pub whisper_configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<AnnotationIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Derive-plan errors.
#[derive(Debug, Error)]
pub enum AnnotationDeriveError {
    #[error(transparent)]
    Store(#[from] AnnotationStoreError),
    #[error(transparent)]
    Transcription(#[from] TranscriptionJobError),
    #[error(transparent)]
    Interpretation(#[from] InterpretCaptureError),
    #[error(transparent)]
    Whisper(#[from] TranscriptionConfigError),
    #[error(transparent)]
    Domain(#[from] takegraph_core::AnnotationError),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
    #[error("annotation derive I/O failed")]
    Io(#[from] std::io::Error),
    #[error("annotation derive plan could not be decoded")]
    Decode(#[from] serde_json::Error),
    #[error("annotation derive plan {0} is unknown")]
    UnknownPlan(Uuid),
    #[error("annotation derive plan digest does not match")]
    DigestMismatch,
    #[error("correct mode requires non-empty text")]
    MissingText,
    #[error("capture {0} has no transcript to interpret")]
    MissingTranscript(String),
}

impl AnnotationDerivePlan {
    /// Seals a path-free plan. Transcribe requires a configured host ASR.
    ///
    /// # Errors
    ///
    /// Returns a validation or host-configuration error.
    pub fn stage(
        capture_id: AnnotationId,
        project_id: impl Into<String>,
        mode: AnnotationDeriveMode,
        text: Option<String>,
        language: Option<String>,
        intents: Option<Vec<AnnotationIntent>>,
    ) -> Result<Self, AnnotationDeriveError> {
        let project_id = project_id.into();
        let text = text.and_then(|value| {
            let trimmed = value.trim().to_owned();
            (!trimmed.is_empty()).then_some(trimmed)
        });
        if mode == AnnotationDeriveMode::Correct && text.is_none() {
            return Err(AnnotationDeriveError::MissingText);
        }
        if mode == AnnotationDeriveMode::Transcribe {
            resolve_transcription_host(language.as_deref())?;
        }
        if let Some(intents) = &intents {
            for intent in intents {
                intent.validate()?;
            }
        }
        let handle = Uuid::new_v4();
        let plan_digest = seal_derive_digest(
            capture_id,
            &project_id,
            mode,
            text.as_deref(),
            language.as_deref(),
            intents.as_deref(),
        )?;
        Ok(Self {
            handle,
            capture_id,
            project_id,
            mode,
            text,
            language,
            intents,
            plan_digest,
            phase: AnnotationDerivePhase::Staged,
            error: None,
        })
    }

    /// Path-free inspect/MCP projection.
    #[must_use]
    pub fn report(&self, transcript_summary: Option<String>) -> AnnotationDeriveReport {
        AnnotationDeriveReport {
            handle: self.handle.to_string(),
            capture_id: self.capture_id.0.to_string(),
            mode: self.mode,
            phase: self.phase,
            plan_digest: self.plan_digest.clone(),
            whisper_configured: whisper_host_configured(),
            transcript_summary,
            intents: self.intents.clone().unwrap_or_default(),
            error: self.error.clone(),
        }
    }
}

/// Directory of derive plans beside the shared annotation root.
#[derive(Debug, Clone)]
pub struct AnnotationDeriveStore {
    root: PathBuf,
}

impl AnnotationDeriveStore {
    /// Opens `{annotation_root}/derive`.
    #[must_use]
    pub fn open(annotation_root: impl AsRef<Path>) -> Self {
        Self {
            root: annotation_root.as_ref().join("derive"),
        }
    }

    /// Publishes one sealed plan.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the plan file cannot be written.
    pub fn save(&self, plan: &AnnotationDerivePlan) -> Result<(), AnnotationDeriveError> {
        fs::create_dir_all(&self.root)?;
        let final_path = self.plan_path(plan.handle);
        let temporary = self.root.join(format!(".derive-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<(), AnnotationDeriveError> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(plan)?)?;
            file.sync_all()?;
            fs::rename(&temporary, final_path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Loads one plan.
    ///
    /// # Errors
    ///
    /// Returns [`AnnotationDeriveError::UnknownPlan`] when missing.
    pub fn get(&self, handle: Uuid) -> Result<AnnotationDerivePlan, AnnotationDeriveError> {
        let path = self.plan_path(handle);
        if !path.is_file() {
            return Err(AnnotationDeriveError::UnknownPlan(handle));
        }
        let mut bytes = Vec::new();
        File::open(path)?.read_to_end(&mut bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn plan_path(&self, handle: Uuid) -> PathBuf {
        self.root
            .join(format!("{PLAN_PREFIX}{handle}{PLAN_SUFFIX}"))
    }
}

/// Executes a staged plan against the current capture. ASR paths stay host-local.
///
/// # Errors
///
/// Returns store, provider, or digest errors. The capture remains loadable.
pub async fn run_annotation_derive(
    annotation_root: impl AsRef<Path>,
    handle: Uuid,
    expected_digest: &str,
    now_utc: &str,
) -> Result<AnnotationDeriveReport, AnnotationDeriveError> {
    let plans = AnnotationDeriveStore::open(&annotation_root);
    let mut plan = plans.get(handle)?;
    if plan.plan_digest != expected_digest {
        return Err(AnnotationDeriveError::DigestMismatch);
    }
    let store = AnnotationStore::open_scoped(&annotation_root, &plan.project_id)?;
    let outcome = execute_plan(&store, &plan, now_utc).await;
    match outcome {
        Ok(summary) => {
            plan.phase = AnnotationDerivePhase::Completed;
            plan.error = None;
            plans.save(&plan)?;
            let mut report = plan.report(summary.transcript_summary);
            report.intents = summary.intents;
            Ok(report)
        }
        Err(error) => {
            plan.phase = AnnotationDerivePhase::Failed;
            plan.error = Some(error.to_string());
            plans.save(&plan)?;
            Err(error)
        }
    }
}

struct DeriveOutcome {
    transcript_summary: Option<String>,
    intents: Vec<AnnotationIntent>,
}

async fn execute_plan(
    store: &AnnotationStore,
    plan: &AnnotationDerivePlan,
    now_utc: &str,
) -> Result<DeriveOutcome, AnnotationDeriveError> {
    match plan.mode {
        AnnotationDeriveMode::Transcribe => {
            let host = resolve_transcription_host(plan.language.as_deref())?;
            let provider = WhisperCppProvider::new(
                &host.executable,
                &host.model,
                host.language.clone(),
                host.extra_args,
            )
            .map_err(TranscriptionJobError::from)?;
            let projection = store.capture(plan.capture_id)?;
            let audio_path = store
                .audio_artifact_path(&projection.capture.audio.audio_sha256)
                .ok_or(AnnotationDeriveError::Store(
                    AnnotationStoreError::UnknownCapture(plan.capture_id.0.to_string()),
                ))?;
            let jobs = TranscriptionJobStore::open(store);
            let transcript = transcribe_capture(
                store,
                &jobs,
                &provider,
                plan.capture_id,
                &audio_path,
                &host.language,
                host.executable,
                host.model,
                provider.extra_args().to_vec(),
                now_utc,
            )
            .await?;
            Ok(DeriveOutcome {
                transcript_summary: Some(summarize(&transcript.text)),
                intents: Vec::new(),
            })
        }
        AnnotationDeriveMode::Correct => {
            let text = plan
                .text
                .as_deref()
                .ok_or(AnnotationDeriveError::MissingText)?;
            let jobs = TranscriptionJobStore::open(store);
            let transcript =
                attach_human_transcript(store, &jobs, plan.capture_id, text, "agent", now_utc)?;
            Ok(DeriveOutcome {
                transcript_summary: Some(summarize(&transcript.text)),
                intents: Vec::new(),
            })
        }
        AnnotationDeriveMode::Interpret => {
            let projection = store.capture(plan.capture_id)?;
            if projection.transcript.is_none() {
                return Err(AnnotationDeriveError::MissingTranscript(
                    plan.capture_id.0.to_string(),
                ));
            }
            let interpretation = if let Some(intents) = &plan.intents {
                let temporal = projection
                    .interpretation
                    .as_ref()
                    .map(|value| value.temporal.clone())
                    .unwrap_or(takegraph_core::TemporalReference {
                        reference_frame: projection.capture.start_anchor.frame,
                        start_offset_frames: 0,
                        end_offset_frames: None,
                        relation: takegraph_core::TemporalRelation::At,
                    });
                let provider = HumanInterpretation::new("agent", temporal, intents.clone());
                interpret_capture(store, &provider, plan.capture_id)?
            } else {
                interpret_capture(store, &HeuristicInterpreter, plan.capture_id)?
            };
            let summary = store
                .capture(plan.capture_id)?
                .transcript
                .map(|transcript| summarize(&transcript.text));
            Ok(DeriveOutcome {
                transcript_summary: summary,
                intents: interpretation.intents,
            })
        }
    }
}

fn seal_derive_digest(
    capture_id: AnnotationId,
    project_id: &str,
    mode: AnnotationDeriveMode,
    text: Option<&str>,
    language: Option<&str>,
    intents: Option<&[AnnotationIntent]>,
) -> Result<String, AnnotationDeriveError> {
    Ok(canonical_sha256(
        "takegraph-annotation-derive-v1",
        &(capture_id, project_id, mode, text, language, intents),
    )?)
}

fn summarize(text: &str) -> String {
    let trimmed = text.trim();
    let mut characters = trimmed.chars();
    let prefix: String = characters.by_ref().take(80).collect();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        AnnotationCapture, CaptureSessionId, CapturedAudioEvidence, RevisionId, SourceAnchor,
    };

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

    #[test]
    fn correct_stage_and_run_preserves_capture() {
        let root = std::env::temp_dir().join(format!("takegraph-derive-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let plan = AnnotationDerivePlan::stage(
            id,
            "project-a",
            AnnotationDeriveMode::Correct,
            Some("今のところ残す".into()),
            None,
            None,
        )
        .unwrap();
        let plans = AnnotationDeriveStore::open(&root);
        plans.save(&plan).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = runtime
            .block_on(run_annotation_derive(
                &root,
                plan.handle,
                &plan.plan_digest,
                "2026-08-14T13:34:57Z",
            ))
            .unwrap();
        assert_eq!(report.phase, AnnotationDerivePhase::Completed);
        assert_eq!(report.transcript_summary.as_deref(), Some("今のところ残す"));
        assert!(store.capture(id).is_ok());
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("executable"));
        assert!(!json.contains("modelPath"));
        assert!(!json.contains("audioPath"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
