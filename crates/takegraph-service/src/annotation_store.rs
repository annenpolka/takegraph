//! Project-scoped append-only annotation store.
//!
//! Voice annotation captures, transcripts, and interpretations live in their
//! own hash-chained journal series, deliberately separate from the canonical
//! project store: recording an observation must never advance or invalidate
//! a canonical revision. Re-importing the same capture with the same audio
//! hash is an idempotent replay; the same ID with different audio is a
//! conflict. Derived revisions append; they never overwrite.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use takegraph_core::{
    AnnotationCapture, AnnotationError, AnnotationId, AnnotationInterpretation,
    AnnotationTranscript, CanonicalError, RevisionId, canonical_sha256,
};
use thiserror::Error;
use uuid::Uuid;

const ANNOTATION_STORE_SCHEMA_VERSION: u32 = 1;
const EVENT_PREFIX: &str = "event-";
const EVENT_SUFFIX: &str = ".json";

/// Journal events. Every mutation of annotation state is one appended
/// record; the projection is rebuilt by replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum AnnotationEvent {
    CaptureImported {
        capture: AnnotationCapture,
    },
    TranscriptAttached {
        capture_id: AnnotationId,
        transcript: AnnotationTranscript,
    },
    InterpretationAttached {
        capture_id: AnnotationId,
        interpretation: AnnotationInterpretation,
    },
    Dismissed {
        capture_id: AnnotationId,
        reason: Option<String>,
    },
    Reopened {
        capture_id: AnnotationId,
    },
    PromotionStaged {
        capture_id: AnnotationId,
        interpretation_digest: String,
        task_id: String,
        plan_digest: String,
        base_revision: RevisionId,
    },
    PromotionCommitted {
        capture_id: AnnotationId,
        task_id: String,
        committed_revision: RevisionId,
        receipt_digest: String,
    },
}

/// Hash-chained journal record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnnotationEventRecord {
    schema_version: u32,
    generation: u64,
    previous_record_digest: Option<String>,
    payload: AnnotationEvent,
    record_digest: String,
}

/// Lifecycle of one capture in the projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureLifecycle {
    Active,
    Dismissed,
}

/// Promotion tracking inside the annotation projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionState {
    pub status: PromotionStatus,
    pub interpretation_digest: String,
    pub task_id: String,
    pub plan_digest: String,
    pub base_revision: RevisionId,
    pub committed_revision: Option<RevisionId>,
    pub receipt_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionStatus {
    Staged,
    Committed,
}

/// Replayed per-capture state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureProjection {
    pub capture: AnnotationCapture,
    pub lifecycle: CaptureLifecycle,
    pub stability: takegraph_core::CaptureStability,
    /// Latest transcript revision for this capture.
    pub transcript: Option<AnnotationTranscript>,
    /// Latest interpretation revision, bound to the latest transcript.
    pub interpretation: Option<AnnotationInterpretation>,
    pub promotion: Option<PromotionState>,
}

/// Full replayed store projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnotationStoreState {
    pub schema_version: u32,
    pub generation: u64,
    pub captures: BTreeMap<String, CaptureProjection>,
}

/// Outcome of importing a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureImportOutcome {
    Imported,
    Replayed,
}

/// Outcome of attaching a derived artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    Attached,
    AttachedRevised,
}

/// Append-only, hash-chained annotation journal for one project.
///
/// Writers take an OS file lock and atomically publish immutable event
/// records. Readers replay the full chain and fail closed on any digest or
/// sequencing break.
#[derive(Debug, Clone)]
pub struct AnnotationStore {
    root: PathBuf,
    project_id: String,
}

#[derive(Debug, Error)]
pub enum AnnotationStoreError {
    #[error("annotation domain violation: {0}")]
    Domain(#[from] AnnotationError),
    #[error("canonical serialization failed: {0}")]
    Canonical(#[from] CanonicalError),
    #[error("annotation store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("annotation store payload could not be decoded: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("annotation journal is corrupt: {0}")]
    Corrupt(String),
    #[error("projectId must not be empty")]
    EmptyProjectId,
    #[error("capture {0} already exists with different content")]
    CaptureConflict(String),
    #[error("capture {0} is unknown")]
    UnknownCapture(String),
    #[error("capture {0} transcript binding does not match captured audio")]
    TranscriptAudioMismatch(String),
    #[error("capture {0} interpretation binding does not match the current transcript digest")]
    InterpretationTranscriptMismatch(String),
    #[error("capture {0} already holds a promotion")]
    PromotionAlreadyStaged(String),
    #[error("dismissed capture {0} must be reopened explicitly before promotion")]
    DismissedCapture(String),
    #[error("source-changed capture {0} cannot be promoted")]
    SourceChangedCapture(String),
    #[error("capture {0} has no staged promotion")]
    NoStagedPromotion(String),
    #[error("promotion task binding does not match the staged promotion")]
    PromotionTaskMismatch,
    #[error("annotation generation counter overflowed")]
    GenerationOverflow,
}

impl AnnotationStore {
    /// Opens or creates the project-scoped annotation journal beneath a
    /// shared root, keyed by a content-addressed project directory exactly
    /// like the canonical project store.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty project ID, canonicalization failure,
    /// I/O failure, or a corrupt published chain.
    pub fn open_scoped(
        shared_root: impl AsRef<Path>,
        project_id: impl Into<String>,
    ) -> Result<Self, AnnotationStoreError> {
        let project_id = project_id.into();
        if project_id.trim().is_empty() {
            return Err(AnnotationStoreError::EmptyProjectId);
        }
        let digest = canonical_sha256("takegraph-annotation-store-key", &project_id)?;
        let directory = digest
            .strip_prefix("sha256:")
            .ok_or_else(|| AnnotationStoreError::Corrupt("invalid project key digest".into()))?;
        let store = Self {
            root: shared_root.as_ref().join(directory),
            project_id,
        };
        Ok(store)
    }

    /// Returns the canonical project identity bound to this store.
    #[must_use]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Imports a capture. Same ID with byte-identical content replays
    /// idempotently; same ID with different audio conflicts.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid capture, a conflicting re-import,
    /// I/O failure, or a corrupt published chain.
    pub fn import_capture(
        &self,
        capture: AnnotationCapture,
    ) -> Result<CaptureImportOutcome, AnnotationStoreError> {
        capture.validate_and_classify()?;
        self.with_lock(|store| {
            let mut state = store.load_state_unlocked()?;
            let key = capture.id.0.to_string();
            if let Some(existing) = state.captures.get(&key) {
                if existing.capture == capture {
                    return Ok(CaptureImportOutcome::Replayed);
                }
                return Err(AnnotationStoreError::CaptureConflict(key));
            }
            let stability = capture.validate_and_classify()?;
            store
                .append_event_unlocked(&mut state, AnnotationEvent::CaptureImported { capture })?;
            let _ = stability;
            Ok(CaptureImportOutcome::Imported)
        })
    }

    /// Attaches a transcript revision. Binding to the exact captured audio
    /// is enforced; attaching after a prior revision drops any
    /// interpretation and promotion bound to the previous digest.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid transcript, unknown capture, audio
    /// binding mismatch, I/O failure, or a corrupt published chain.
    pub fn attach_transcript(
        &self,
        transcript: AnnotationTranscript,
    ) -> Result<AttachOutcome, AnnotationStoreError> {
        transcript.validate()?;
        self.with_lock(|store| {
            let mut state = store.load_state_unlocked()?;
            let key = transcript.capture_id.0.to_string();
            let projection = state
                .captures
                .get(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            if projection.capture.audio.audio_sha256 != transcript.audio_sha256 {
                return Err(AnnotationStoreError::TranscriptAudioMismatch(key));
            }
            let revised = projection.transcript.is_some();
            store.append_event_unlocked(
                &mut state,
                AnnotationEvent::TranscriptAttached {
                    capture_id: transcript.capture_id,
                    transcript,
                },
            )?;
            Ok(if revised {
                AttachOutcome::AttachedRevised
            } else {
                AttachOutcome::Attached
            })
        })
    }

    /// Attaches an interpretation bound to the current transcript digest.
    /// Attaching after a prior interpretation drops any promotion bound to
    /// the previous interpretation digest.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid interpretation, unknown capture,
    /// transcript binding mismatch, I/O failure, or a corrupt published
    /// chain.
    pub fn attach_interpretation(
        &self,
        interpretation: AnnotationInterpretation,
    ) -> Result<AttachOutcome, AnnotationStoreError> {
        interpretation.validate()?;
        self.with_lock(|store| {
            let mut state = store.load_state_unlocked()?;
            let key = interpretation.capture_id.0.to_string();
            let projection = state
                .captures
                .get(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            let transcript = projection.transcript.as_ref().ok_or_else(|| {
                AnnotationStoreError::InterpretationTranscriptMismatch(key.clone())
            })?;
            if transcript.transcript_digest != interpretation.transcript_digest {
                return Err(AnnotationStoreError::InterpretationTranscriptMismatch(key));
            }
            let revised = projection.interpretation.is_some();
            store.append_event_unlocked(
                &mut state,
                AnnotationEvent::InterpretationAttached {
                    capture_id: interpretation.capture_id,
                    interpretation,
                },
            )?;
            Ok(if revised {
                AttachOutcome::AttachedRevised
            } else {
                AttachOutcome::Attached
            })
        })
    }

    /// Dismisses a capture. Audio evidence is kept; the promotion path ends.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown capture, double dismissal, I/O
    /// failure, or a corrupt published chain.
    pub fn dismiss(
        &self,
        capture_id: AnnotationId,
        reason: Option<String>,
    ) -> Result<(), AnnotationStoreError> {
        self.with_lock(|store| {
            let mut state = store.load_state_unlocked()?;
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            if projection.lifecycle == CaptureLifecycle::Dismissed {
                return Err(AnnotationStoreError::Corrupt(format!(
                    "capture {key} is already dismissed"
                )));
            }
            store.append_event_unlocked(
                &mut state,
                AnnotationEvent::Dismissed { capture_id, reason },
            )
        })
    }

    /// Explicitly returns a dismissed capture to the active path.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown capture or a capture that is not
    /// dismissed.
    pub fn reopen(&self, capture_id: AnnotationId) -> Result<(), AnnotationStoreError> {
        self.with_lock(|store| {
            let mut state = store.load_state_unlocked()?;
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            if projection.lifecycle != CaptureLifecycle::Dismissed {
                return Err(AnnotationStoreError::Corrupt(format!(
                    "capture {key} is not dismissed"
                )));
            }
            store.append_event_unlocked(&mut state, AnnotationEvent::Reopened { capture_id })
        })
    }

    /// Records that an interpretation was promoted as an existing
    /// `timeline_edit` task. Staging is refused for dismissed captures,
    /// source-changed captures, missing or unbound interpretations, and
    /// captures that already hold a promotion.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown capture or any guard failure.
    pub fn stage_promotion(
        &self,
        capture_id: AnnotationId,
        task_id: impl Into<String>,
        plan_digest: impl Into<String>,
        base_revision: RevisionId,
    ) -> Result<(), AnnotationStoreError> {
        let task_id = task_id.into();
        let plan_digest = plan_digest.into();
        if task_id.trim().is_empty() || !is_sha256_digest(&plan_digest) {
            return Err(AnnotationStoreError::Corrupt(
                "promotion requires a task ID and a sha256 plan digest".into(),
            ));
        }
        self.with_lock(|store| {
            let mut state = store.load_state_unlocked()?;
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            if projection.lifecycle == CaptureLifecycle::Dismissed {
                return Err(AnnotationStoreError::DismissedCapture(key));
            }
            if projection.stability == takegraph_core::CaptureStability::SourceChanged {
                return Err(AnnotationStoreError::SourceChangedCapture(key));
            }
            let interpretation = projection.interpretation.as_ref().ok_or_else(|| {
                AnnotationStoreError::InterpretationTranscriptMismatch(key.clone())
            })?;
            if projection.promotion.is_some() {
                return Err(AnnotationStoreError::PromotionAlreadyStaged(key));
            }
            let interpretation_digest = interpretation.interpretation_digest.clone();
            store.append_event_unlocked(
                &mut state,
                AnnotationEvent::PromotionStaged {
                    capture_id,
                    interpretation_digest,
                    task_id,
                    plan_digest,
                    base_revision,
                },
            )
        })
    }

    /// Records that a staged promotion completed with a verified receipt.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown capture, a missing or mismatched
    /// staged promotion, or a malformed receipt digest.
    pub fn commit_promotion(
        &self,
        capture_id: AnnotationId,
        task_id: &str,
        committed_revision: RevisionId,
        receipt_digest: impl Into<String>,
    ) -> Result<(), AnnotationStoreError> {
        let receipt_digest = receipt_digest.into();
        if !is_sha256_digest(&receipt_digest) {
            return Err(AnnotationStoreError::Corrupt(
                "receipt digest must be sha256:<hex>".into(),
            ));
        }
        self.with_lock(|store| {
            let mut state = store.load_state_unlocked()?;
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            let promotion = projection
                .promotion
                .as_ref()
                .ok_or_else(|| AnnotationStoreError::NoStagedPromotion(key.clone()))?;
            if promotion.status != PromotionStatus::Staged
                || promotion.task_id != task_id
                || promotion.base_revision >= committed_revision
            {
                return Err(AnnotationStoreError::PromotionTaskMismatch);
            }
            store.append_event_unlocked(
                &mut state,
                AnnotationEvent::PromotionCommitted {
                    capture_id,
                    task_id: task_id.into(),
                    committed_revision,
                    receipt_digest,
                },
            )
        })
    }

    /// Replays the journal and returns the full projection.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O failure or a corrupt published chain.
    pub fn load_state(&self) -> Result<AnnotationStoreState, AnnotationStoreError> {
        self.load_state_unlocked()
    }

    /// Returns the replayed projection for one capture.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O failure, a corrupt chain, or an unknown
    /// capture.
    pub fn capture(
        &self,
        capture_id: AnnotationId,
    ) -> Result<CaptureProjection, AnnotationStoreError> {
        let state = self.load_state()?;
        state
            .captures
            .get(&capture_id.0.to_string())
            .cloned()
            .ok_or_else(|| AnnotationStoreError::UnknownCapture(capture_id.0.to_string()))
    }

    fn events_path(&self) -> PathBuf {
        self.root.join("events")
    }

    fn with_lock<T>(
        &self,
        action: impl FnOnce(&Self) -> Result<T, AnnotationStoreError>,
    ) -> Result<T, AnnotationStoreError> {
        fs::create_dir_all(self.events_path())?;
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("annotation.lock"))?;
        lock_file.lock()?;
        let result = action(self);
        lock_file.unlock()?;
        result
    }

    fn load_state_unlocked(&self) -> Result<AnnotationStoreState, AnnotationStoreError> {
        let mut state = AnnotationStoreState {
            schema_version: ANNOTATION_STORE_SCHEMA_VERSION,
            generation: 0,
            captures: BTreeMap::new(),
        };
        let mut previous_digest: Option<String> = None;
        for (expected_generation, path) in self.event_record_paths()?.into_iter().enumerate() {
            let expected_generation = u64::try_from(expected_generation)
                .map_err(|_| AnnotationStoreError::GenerationOverflow)?;
            let record =
                verify_event_record(&path, expected_generation, previous_digest.as_deref())?;
            previous_digest = Some(record.record_digest.clone());
            state.generation = record.generation + 1;
            apply_event(&mut state, record.payload)?;
        }
        Ok(state)
    }

    fn event_record_paths(&self) -> Result<Vec<PathBuf>, AnnotationStoreError> {
        let events = self.events_path();
        if !events.is_dir() {
            return Ok(Vec::new());
        }
        let mut paths = fs::read_dir(&events)?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with(EVENT_PREFIX) && name.ends_with(EVENT_SUFFIX)
                })
            })
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }

    fn append_event_unlocked(
        &self,
        state: &mut AnnotationStoreState,
        payload: AnnotationEvent,
    ) -> Result<(), AnnotationStoreError> {
        let generation = state.generation;
        let previous_record_digest = if generation == 0 {
            None
        } else {
            last_record_digest(&self.events_path())?
        };
        let record_digest =
            event_record_digest(generation, previous_record_digest.as_deref(), &payload)?;
        let record = AnnotationEventRecord {
            schema_version: ANNOTATION_STORE_SCHEMA_VERSION,
            generation,
            previous_record_digest,
            payload,
            record_digest,
        };
        let events = self.events_path();
        fs::create_dir_all(&events)?;
        let final_path = events.join(format!(
            "{EVENT_PREFIX}{generation:020}-{}.json",
            record.record_digest.trim_start_matches("sha256:")
        ));
        if final_path.exists() {
            return Err(AnnotationStoreError::Corrupt(format!(
                "annotation event generation already exists: {}",
                final_path.display()
            )));
        }
        let temporary_path = events.join(format!(".event-{}.tmp", Uuid::new_v4()));
        let write_result = (|| -> Result<(), AnnotationStoreError> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)?;
            file.write_all(&serde_json::to_vec_pretty(&record)?)?;
            file.sync_all()?;
            fs::rename(&temporary_path, &final_path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        write_result?;
        apply_event(state, record.payload)?;
        state.generation = generation + 1;
        Ok(())
    }
}

fn verify_event_record(
    path: &Path,
    expected_generation: u64,
    previous_digest: Option<&str>,
) -> Result<AnnotationEventRecord, AnnotationStoreError> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let record: AnnotationEventRecord = serde_json::from_slice(&bytes).map_err(|error| {
        AnnotationStoreError::Corrupt(format!(
            "invalid annotation record {}: {error}",
            path.display()
        ))
    })?;
    if record.schema_version != ANNOTATION_STORE_SCHEMA_VERSION
        || record.generation != expected_generation
        || record.previous_record_digest.as_deref() != previous_digest
    {
        return Err(AnnotationStoreError::Corrupt(format!(
            "broken annotation journal chain at {}",
            path.display()
        )));
    }
    let digest = event_record_digest(
        record.generation,
        record.previous_record_digest.as_deref(),
        &record.payload,
    )?;
    if digest != record.record_digest
        || !path.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .contains(record.record_digest.trim_start_matches("sha256:"))
        })
    {
        return Err(AnnotationStoreError::Corrupt(format!(
            "annotation journal digest mismatch at {}",
            path.display()
        )));
    }
    Ok(record)
}

// One pass applies every journal variant and keeps replay binding checks and
// the projection transitions beside their event payload.
#[allow(clippy::too_many_lines)]
fn apply_event(
    state: &mut AnnotationStoreState,
    event: AnnotationEvent,
) -> Result<(), AnnotationStoreError> {
    match event {
        AnnotationEvent::CaptureImported { capture } => {
            let stability = capture.validate_and_classify()?;
            state.captures.insert(
                capture.id.0.to_string(),
                CaptureProjection {
                    lifecycle: CaptureLifecycle::Active,
                    stability,
                    capture,
                    transcript: None,
                    interpretation: None,
                    promotion: None,
                },
            );
        }
        AnnotationEvent::TranscriptAttached {
            capture_id,
            transcript,
        } => {
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get_mut(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            if projection.capture.audio.audio_sha256 != transcript.audio_sha256 {
                return Err(AnnotationStoreError::TranscriptAudioMismatch(
                    transcript.capture_id.0.to_string(),
                ));
            }
            // A new transcript revision invalidates derivations bound to the
            // previous digest. (Spec: transcript revision drops promotion.)
            projection.interpretation = None;
            projection.promotion = None;
            projection.transcript = Some(transcript);
        }
        AnnotationEvent::InterpretationAttached {
            capture_id,
            interpretation,
        } => {
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get_mut(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            let transcript_digest = projection
                .transcript
                .as_ref()
                .map(|transcript| transcript.transcript_digest.clone())
                .ok_or_else(|| {
                    AnnotationStoreError::InterpretationTranscriptMismatch(key.clone())
                })?;
            if transcript_digest != interpretation.transcript_digest {
                return Err(AnnotationStoreError::InterpretationTranscriptMismatch(key));
            }
            projection.promotion = None;
            projection.interpretation = Some(interpretation);
        }
        AnnotationEvent::Dismissed { capture_id, reason } => {
            let _ = reason;
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get_mut(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            projection.lifecycle = CaptureLifecycle::Dismissed;
            projection.promotion = None;
        }
        AnnotationEvent::Reopened { capture_id } => {
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get_mut(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            projection.lifecycle = CaptureLifecycle::Active;
        }
        AnnotationEvent::PromotionStaged {
            capture_id,
            interpretation_digest,
            task_id,
            plan_digest,
            base_revision,
        } => {
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get_mut(&key)
                .ok_or_else(|| AnnotationStoreError::UnknownCapture(key.clone()))?;
            let current = projection
                .interpretation
                .as_ref()
                .map(|interpretation| interpretation.interpretation_digest.clone())
                .ok_or_else(|| {
                    AnnotationStoreError::InterpretationTranscriptMismatch(key.clone())
                })?;
            if current != interpretation_digest || projection.promotion.is_some() {
                return Err(AnnotationStoreError::PromotionTaskMismatch);
            }
            projection.promotion = Some(PromotionState {
                status: PromotionStatus::Staged,
                interpretation_digest,
                task_id,
                plan_digest,
                base_revision,
                committed_revision: None,
                receipt_digest: None,
            });
        }
        AnnotationEvent::PromotionCommitted {
            capture_id,
            task_id,
            committed_revision,
            receipt_digest,
        } => {
            let key = capture_id.0.to_string();
            let projection = state
                .captures
                .get_mut(&key)
                .ok_or(AnnotationStoreError::UnknownCapture(key.clone()))?;
            let promotion = projection
                .promotion
                .as_mut()
                .ok_or(AnnotationStoreError::NoStagedPromotion(key))?;
            if promotion.status != PromotionStatus::Staged
                || promotion.task_id != task_id
                || promotion.base_revision >= committed_revision
            {
                return Err(AnnotationStoreError::PromotionTaskMismatch);
            }
            promotion.status = PromotionStatus::Committed;
            promotion.committed_revision = Some(committed_revision);
            promotion.receipt_digest = Some(receipt_digest);
        }
    }
    Ok(())
}

fn last_record_digest(events: &Path) -> Result<Option<String>, AnnotationStoreError> {
    let mut latest: Option<(u64, String)> = None;
    for entry in fs::read_dir(events)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stripped) = name.strip_prefix(EVENT_PREFIX) else {
            continue;
        };
        let Some(stripped) = stripped.strip_suffix(EVENT_SUFFIX) else {
            continue;
        };
        let Some((generation, digest)) = stripped.split_once('-') else {
            continue;
        };
        let Ok(generation) = generation.parse::<u64>() else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|(current, _)| *current < generation)
        {
            latest = Some((generation, format!("sha256:{digest}")));
        }
    }
    Ok(latest.map(|(_, digest)| digest))
}

fn event_record_digest(
    generation: u64,
    previous_record_digest: Option<&str>,
    payload: &AnnotationEvent,
) -> Result<String, CanonicalError> {
    canonical_sha256(
        "takegraph-annotation-event-v1",
        &(generation, previous_record_digest, payload),
    )
}

fn is_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{CaptureSessionId, CaptureStability, CapturedAudioEvidence, SourceAnchor};

    fn test_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("takegraph-annotation-{}", Uuid::new_v4()))
    }

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

    fn audio(hash_suffix: char) -> CapturedAudioEvidence {
        CapturedAudioEvidence {
            audio_sha256: format!("sha256:{}", hash_suffix.to_string().repeat(64)),
            byte_length: 32_000,
            duration_samples: 15_000,
            sample_rate: 16_000,
            channels: 1,
            bits_per_sample: 16,
        }
    }

    fn capture(id: AnnotationId) -> AnnotationCapture {
        AnnotationCapture {
            id,
            session_id: CaptureSessionId(Uuid::nil()),
            start_anchor: anchor(2531),
            end_anchor: anchor(2698),
            audio: audio('a'),
            captured_at_utc: "2026-08-14T13:34:57Z".into(),
        }
    }

    fn transcript(capture_id: AnnotationId, digest_char: char) -> AnnotationTranscript {
        AnnotationTranscript {
            id: Uuid::new_v4(),
            capture_id,
            audio_sha256: audio('a').audio_sha256,
            text: "今のところ三秒前から残す".into(),
            provider_id: "whisper-cpp".into(),
            provider_digest: format!("sha256:{}", "e".repeat(64)),
            transcript_digest: format!("sha256:{}", digest_char.to_string().repeat(64)),
        }
    }

    fn interpretation(
        capture_id: AnnotationId,
        transcript_digest: &str,
        digest_char: char,
    ) -> AnnotationInterpretation {
        AnnotationInterpretation {
            id: Uuid::new_v4(),
            capture_id,
            transcript_digest: transcript_digest.into(),
            intents: vec![takegraph_core::AnnotationIntent::Narration {
                topic: "Primary Compression".into(),
                draft_hint: None,
            }],
            model_id: "test-model".into(),
            model_digest: format!("sha256:{}", "f".repeat(64)),
            interpretation_digest: format!("sha256:{}", digest_char.to_string().repeat(64)),
        }
    }

    #[test]
    fn capture_import_replay_and_conflict() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();

        assert_eq!(
            store.import_capture(capture(id)).unwrap(),
            CaptureImportOutcome::Imported
        );
        assert_eq!(
            store.import_capture(capture(id)).unwrap(),
            CaptureImportOutcome::Replayed
        );

        let mut conflicting = capture(id);
        conflicting.audio = audio('b');
        assert!(matches!(
            store.import_capture(conflicting),
            Err(AnnotationStoreError::CaptureConflict(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistence_and_reopen() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        store.attach_transcript(transcript(id, 'c')).unwrap();

        let reopened = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let projection = reopened.capture(id).unwrap();
        assert_eq!(
            projection.transcript.as_ref().unwrap().text,
            "今のところ三秒前から残す"
        );
        assert_eq!(reopened.load_state().unwrap().generation, 2);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn annotation_events_never_touch_canonical_head() {
        let root = test_root();
        let canonical = crate::project_store::DurableProjectStore::open_scoped_or_bootstrap(
            &root,
            "project",
            RevisionId(0),
        )
        .unwrap();
        let head_before = canonical.head().unwrap();

        let annotations = AnnotationStore::open_scoped(&root, "project").unwrap();
        let id = AnnotationId::new();
        annotations.import_capture(capture(id)).unwrap();
        annotations.attach_transcript(transcript(id, 'c')).unwrap();
        annotations.dismiss(id, Some("not useful".into())).unwrap();

        assert_eq!(canonical.head().unwrap(), head_before);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_must_bind_captured_audio() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();

        let mut wrong_audio = transcript(id, 'c');
        wrong_audio.audio_sha256 = audio('b').audio_sha256;
        assert!(matches!(
            store.attach_transcript(wrong_audio),
            Err(AnnotationStoreError::TranscriptAudioMismatch(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_revision_drops_interpretation_and_promotion() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let first = transcript(id, 'c');
        store.attach_transcript(first.clone()).unwrap();
        store
            .attach_interpretation(interpretation(id, &first.transcript_digest, '1'))
            .unwrap();
        store
            .stage_promotion(
                id,
                "task-1",
                format!("sha256:{}", "0".repeat(64)),
                RevisionId(4),
            )
            .unwrap();

        assert!(store.capture(id).unwrap().promotion.is_some());

        store.attach_transcript(transcript(id, 'd')).unwrap();
        let projection = store.capture(id).unwrap();
        assert!(projection.interpretation.is_none());
        assert!(projection.promotion.is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interpretation_must_bind_current_transcript_digest() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let current = transcript(id, 'c');
        store.attach_transcript(current.clone()).unwrap();

        let stale = interpretation(id, &format!("sha256:{}", "9".repeat(64)), '1');
        assert!(matches!(
            store.attach_interpretation(stale),
            Err(AnnotationStoreError::InterpretationTranscriptMismatch(_))
        ));

        store
            .attach_interpretation(interpretation(id, &current.transcript_digest, '1'))
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dismissed_capture_requires_explicit_reopen_before_promotion() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let current = transcript(id, 'c');
        store.attach_transcript(current.clone()).unwrap();
        store
            .attach_interpretation(interpretation(id, &current.transcript_digest, '1'))
            .unwrap();

        store.dismiss(id, None).unwrap();
        assert!(matches!(
            store.stage_promotion(
                id,
                "task-1",
                format!("sha256:{}", "0".repeat(64)),
                RevisionId(4)
            ),
            Err(AnnotationStoreError::DismissedCapture(_))
        ));

        store.reopen(id).unwrap();
        store
            .stage_promotion(
                id,
                "task-1",
                format!("sha256:{}", "0".repeat(64)),
                RevisionId(4),
            )
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_changed_capture_cannot_be_promoted() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        let mut changed = capture(id);
        changed.end_anchor.source_fingerprint = "fp-2".into();
        store.import_capture(changed).unwrap();
        assert_eq!(
            store.capture(id).unwrap().stability,
            CaptureStability::SourceChanged
        );

        assert!(matches!(
            store.stage_promotion(
                id,
                "task-1",
                format!("sha256:{}", "0".repeat(64)),
                RevisionId(4)
            ),
            Err(AnnotationStoreError::SourceChangedCapture(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn promotion_commits_at_most_once_with_matching_task() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();
        let current = transcript(id, 'c');
        store.attach_transcript(current.clone()).unwrap();
        store
            .attach_interpretation(interpretation(id, &current.transcript_digest, '1'))
            .unwrap();
        store
            .stage_promotion(
                id,
                "task-1",
                format!("sha256:{}", "0".repeat(64)),
                RevisionId(4),
            )
            .unwrap();

        assert!(matches!(
            store.commit_promotion(
                id,
                "task-2",
                RevisionId(5),
                format!("sha256:{}", "2".repeat(64))
            ),
            Err(AnnotationStoreError::PromotionTaskMismatch)
        ));
        assert!(matches!(
            store.commit_promotion(
                id,
                "task-1",
                RevisionId(4),
                format!("sha256:{}", "2".repeat(64))
            ),
            Err(AnnotationStoreError::PromotionTaskMismatch)
        ));
        store
            .commit_promotion(
                id,
                "task-1",
                RevisionId(5),
                format!("sha256:{}", "2".repeat(64)),
            )
            .unwrap();

        let promotion = store.capture(id).unwrap().promotion.unwrap();
        assert_eq!(promotion.status, PromotionStatus::Committed);
        assert_eq!(promotion.committed_revision, Some(RevisionId(5)));

        assert!(matches!(
            store.stage_promotion(
                id,
                "task-3",
                format!("sha256:{}", "1".repeat(64)),
                RevisionId(5)
            ),
            Err(AnnotationStoreError::PromotionAlreadyStaged(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_journal_fails_closed() {
        let root = test_root();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store.import_capture(capture(id)).unwrap();

        let events = root
            .join(
                canonical_sha256("takegraph-annotation-store-key", "project-a")
                    .unwrap()
                    .trim_start_matches("sha256:"),
            )
            .join("events");
        let mut paths: Vec<_> = fs::read_dir(&events)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        let mut tampered = String::new();
        File::open(&paths[0])
            .unwrap()
            .read_to_string(&mut tampered)
            .unwrap();
        let tampered = tampered.replace("\"generation\": 0", "\"generation\": 7");
        fs::write(&paths[0], tampered).unwrap();

        assert!(matches!(
            store.load_state(),
            Err(AnnotationStoreError::Corrupt(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }
}
