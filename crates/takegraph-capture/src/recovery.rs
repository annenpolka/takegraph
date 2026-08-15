//! Incoming `*.partial` recovery. Incomplete files are quarantined; complete
//! WAV plus metadata is imported without advancing the canonical head.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use takegraph_core::{AnnotationCapture, AnnotationId, CaptureSessionId, SourceAnchor};
use takegraph_service::annotation_store::AnnotationStore;
use uuid::Uuid;

use crate::error::CaptureError;
use crate::wav::{evidence_from_wav, publish_wav};

pub const PROJECT_ID_FILE: &str = "project-id";
pub const INCOMING_DIR: &str = "incoming";
pub const QUARANTINE_DIR: &str = "quarantine";
const META_SCHEMA_VERSION: u32 = 1;

/// Sidecar written when a recording starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncomingMeta {
    pub schema_version: u32,
    pub capture_id: AnnotationId,
    pub session_id: CaptureSessionId,
    pub project_id: String,
    pub start_anchor: SourceAnchor,
    pub started_at_utc: String,
}

/// Sidecar written after CAS publish and before or after journal import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IncomingCommitted {
    pub schema_version: u32,
    pub capture: AnnotationCapture,
}

/// Outcome of scanning incoming files under one annotation root.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub imported: Vec<AnnotationId>,
    pub replayed: Vec<AnnotationId>,
    pub quarantined: Vec<AnnotationId>,
}

/// Writes `project-id` so later scans can reopen the store without hashing
/// every known project identity.
///
/// # Errors
///
/// Returns an I/O error when the file cannot be created.
pub fn remember_project_id(store: &AnnotationStore) -> Result<(), CaptureError> {
    fs::create_dir_all(store.directory())?;
    let path = store.directory().join(PROJECT_ID_FILE);
    if !path.exists() {
        fs::write(path, store.project_id())?;
    }
    Ok(())
}

/// Reads a previously written project identity.
#[must_use]
pub fn read_project_id(directory: &Path) -> Option<String> {
    fs::read_to_string(directory.join(PROJECT_ID_FILE))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Incoming directory for one project store.
#[must_use]
pub fn incoming_dir(store: &AnnotationStore) -> PathBuf {
    store.directory().join(INCOMING_DIR)
}

/// In-progress WAV path.
#[must_use]
pub fn partial_path(store: &AnnotationStore, capture_id: AnnotationId) -> PathBuf {
    incoming_dir(store).join(format!("{}.partial", capture_id.0))
}

/// Start-anchor sidecar path.
#[must_use]
pub fn meta_path(store: &AnnotationStore, capture_id: AnnotationId) -> PathBuf {
    incoming_dir(store).join(format!("{}.meta.json", capture_id.0))
}

/// Post-CAS sidecar path.
#[must_use]
pub fn committed_path(store: &AnnotationStore, capture_id: AnnotationId) -> PathBuf {
    incoming_dir(store).join(format!("{}.committed.json", capture_id.0))
}

/// Creates the incoming directory and writes the start sidecar.
///
/// # Errors
///
/// Returns an I/O or serialization error.
pub fn begin_incoming(
    store: &AnnotationStore,
    meta: &IncomingMeta,
) -> Result<PathBuf, CaptureError> {
    remember_project_id(store)?;
    fs::create_dir_all(incoming_dir(store))?;
    fs::write(
        meta_path(store, meta.capture_id),
        serde_json::to_vec_pretty(meta)?,
    )?;
    Ok(partial_path(store, meta.capture_id))
}

/// Writes the committed sidecar after CAS publish.
///
/// # Errors
///
/// Returns an I/O or serialization error.
pub fn write_committed(
    store: &AnnotationStore,
    capture: &AnnotationCapture,
) -> Result<(), CaptureError> {
    let record = IncomingCommitted {
        schema_version: META_SCHEMA_VERSION,
        capture: capture.clone(),
    };
    fs::write(
        committed_path(store, capture.id),
        serde_json::to_vec_pretty(&record)?,
    )?;
    Ok(())
}

/// Removes incoming files for one capture after a successful import.
pub fn cleanup_incoming(store: &AnnotationStore, capture_id: AnnotationId) {
    let _ = fs::remove_file(partial_path(store, capture_id));
    let _ = fs::remove_file(meta_path(store, capture_id));
    let _ = fs::remove_file(committed_path(store, capture_id));
}

/// Moves leftover incoming files out of the recordable path.
///
/// # Errors
///
/// Returns an I/O error when quarantine cannot be created.
pub fn quarantine_incoming(
    store_directory: &Path,
    capture_id: AnnotationId,
) -> Result<(), CaptureError> {
    let incoming = store_directory.join(INCOMING_DIR);
    let quarantine = store_directory.join(QUARANTINE_DIR);
    fs::create_dir_all(&quarantine)?;
    let suffix = Uuid::new_v4().simple();
    for name in [
        format!("{}.partial", capture_id.0),
        format!("{}.meta.json", capture_id.0),
        format!("{}.committed.json", capture_id.0),
    ] {
        let source = incoming.join(&name);
        if source.exists() {
            let destination = quarantine.join(format!("{name}.{suffix}"));
            fs::rename(source, destination)?;
        }
    }
    Ok(())
}

/// Recovers one incoming capture id inside an already-opened store.
///
/// `end_anchor` is used when a complete WAV has metadata but no committed
/// sidecar. Pass the start anchor when composition is unavailable.
///
/// # Errors
///
/// Returns store, I/O, or WAV errors. Incomplete files are quarantined and
/// reported, not returned as errors.
pub fn recover_capture(
    store: &AnnotationStore,
    capture_id: AnnotationId,
    end_anchor: Option<SourceAnchor>,
    captured_at_utc: &str,
    report: &mut RecoveryReport,
) -> Result<(), CaptureError> {
    let committed_file = committed_path(store, capture_id);
    if committed_file.is_file() {
        let record: IncomingCommitted = serde_json::from_slice(&fs::read(&committed_file)?)?;
        ensure_published(store, &record.capture)?;
        match store.import_capture(record.capture.clone())? {
            takegraph_service::annotation_store::CaptureImportOutcome::Imported => {
                report.imported.push(capture_id);
            }
            takegraph_service::annotation_store::CaptureImportOutcome::Replayed => {
                report.replayed.push(capture_id);
            }
        }
        cleanup_incoming(store, capture_id);
        return Ok(());
    }

    let meta_file = meta_path(store, capture_id);
    let partial = partial_path(store, capture_id);
    if !meta_file.is_file() || !partial.is_file() {
        quarantine_incoming(store.directory(), capture_id)?;
        report.quarantined.push(capture_id);
        return Ok(());
    }

    let bytes = fs::read(&partial)?;
    let Ok(evidence) = evidence_from_wav(&bytes) else {
        quarantine_incoming(store.directory(), capture_id)?;
        report.quarantined.push(capture_id);
        return Ok(());
    };
    publish_wav(store.directory(), &bytes)?;
    let meta: IncomingMeta = serde_json::from_slice(&fs::read(&meta_file)?)?;
    let end_anchor = end_anchor.unwrap_or_else(|| meta.start_anchor.clone());
    let capture = AnnotationCapture {
        id: meta.capture_id,
        session_id: meta.session_id,
        start_anchor: meta.start_anchor,
        end_anchor,
        audio: evidence,
        captured_at_utc: captured_at_utc.to_owned(),
    };
    capture.validate_and_classify()?;
    write_committed(store, &capture)?;
    match store.import_capture(capture)? {
        takegraph_service::annotation_store::CaptureImportOutcome::Imported => {
            report.imported.push(capture_id);
        }
        takegraph_service::annotation_store::CaptureImportOutcome::Replayed => {
            report.replayed.push(capture_id);
        }
    }
    cleanup_incoming(store, capture_id);
    Ok(())
}

fn ensure_published(
    store: &AnnotationStore,
    capture: &AnnotationCapture,
) -> Result<(), CaptureError> {
    let Some(path) = crate::wav::audio_cas_path(store.directory(), &capture.audio.audio_sha256)
    else {
        return Err(CaptureError::InvalidWav(
            "committed capture has a malformed audio digest".into(),
        ));
    };
    if path.is_file() {
        return Ok(());
    }
    let partial = partial_path(store, capture.id);
    if partial.is_file() {
        let bytes = fs::read(partial)?;
        publish_wav(store.directory(), &bytes)?;
    }
    Ok(())
}

/// Collects capture ids that have any incoming artifact.
///
/// # Errors
///
/// Returns an I/O error when the incoming directory cannot be read.
pub fn incoming_capture_ids(store_directory: &Path) -> Result<Vec<AnnotationId>, CaptureError> {
    let incoming = store_directory.join(INCOMING_DIR);
    if !incoming.is_dir() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(incoming)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        let stem = name
            .strip_suffix(".committed.json")
            .or_else(|| name.strip_suffix(".meta.json"))
            .or_else(|| name.strip_suffix(".partial"))
            .unwrap_or("");
        if let Ok(uuid) = Uuid::parse_str(stem) {
            let id = AnnotationId(uuid);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids.sort_by_key(|id| id.0);
    Ok(ids)
}

/// Builds a start sidecar.
#[must_use]
pub fn incoming_meta(
    capture_id: AnnotationId,
    session_id: CaptureSessionId,
    start_anchor: SourceAnchor,
    started_at_utc: String,
) -> IncomingMeta {
    IncomingMeta {
        schema_version: META_SCHEMA_VERSION,
        capture_id,
        session_id,
        project_id: start_anchor.project_id.clone(),
        start_anchor,
        started_at_utc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::encode_pcm_wav;
    use takegraph_core::{CapturedAudioEvidence, RevisionId};

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

    fn store() -> (PathBuf, AnnotationStore) {
        let root = std::env::temp_dir().join(format!("takegraph-recover-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        (root, store)
    }

    #[test]
    fn incomplete_partial_is_quarantined() {
        let (root, store) = store();
        let id = AnnotationId::new();
        fs::create_dir_all(incoming_dir(&store)).unwrap();
        fs::write(partial_path(&store, id), b"not a wav").unwrap();
        fs::write(
            meta_path(&store, id),
            serde_json::to_vec(&incoming_meta(
                id,
                CaptureSessionId::new(),
                anchor(10),
                "2026-08-14T13:34:57Z".into(),
            ))
            .unwrap(),
        )
        .unwrap();

        let mut report = RecoveryReport::default();
        recover_capture(&store, id, None, "2026-08-14T13:34:57Z", &mut report).unwrap();
        assert_eq!(report.quarantined, vec![id]);
        assert!(!partial_path(&store, id).exists());
        assert!(
            store
                .directory()
                .join(QUARANTINE_DIR)
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn complete_wav_without_event_is_imported() {
        let (root, store) = store();
        let id = AnnotationId::new();
        let session = CaptureSessionId::new();
        begin_incoming(
            &store,
            &incoming_meta(id, session, anchor(2531), "2026-08-14T13:34:57Z".into()),
        )
        .unwrap();
        fs::write(partial_path(&store, id), encode_pcm_wav(&[1; 1600])).unwrap();

        let mut report = RecoveryReport::default();
        recover_capture(
            &store,
            id,
            Some(anchor(2698)),
            "2026-08-14T13:34:57Z",
            &mut report,
        )
        .unwrap();
        assert_eq!(report.imported, vec![id]);
        let projection = store.capture(id).unwrap();
        assert_eq!(projection.capture.end_anchor.frame, 2698);
        assert!(!partial_path(&store, id).exists());

        let mut replay = RecoveryReport::default();
        write_committed(&store, &projection.capture).unwrap();
        recover_capture(&store, id, None, "2026-08-14T13:34:57Z", &mut replay).unwrap();
        assert_eq!(replay.replayed, vec![id]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evidence_rejects_empty_hash_shape() {
        let evidence = CapturedAudioEvidence {
            audio_sha256: String::new(),
            byte_length: 100,
            duration_samples: 10,
            sample_rate: 16_000,
            channels: 1,
            bits_per_sample: 16,
        };
        assert!(evidence.validate().is_err());
    }
}
