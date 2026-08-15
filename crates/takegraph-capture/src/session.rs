//! Capture-host session: start/stop, listing, dismiss, and restart recovery.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use takegraph_core::{
    AnnotationCapture, AnnotationId, CaptureSessionId, CaptureStability, SourceAnchor,
};
use takegraph_node::Ymm4BridgeClient;
use takegraph_node::{HeuristicInterpreter, WhisperCppProvider};
use takegraph_service::annotation_store::{AnnotationStore, CaptureLifecycle, CaptureProjection};
use takegraph_service::{
    TranscriptionJob, TranscriptionJobStore, interpret_capture, resolve_transcription_host,
    transcribe_capture, whisper_host_configured,
};

use crate::anchor::{
    ObservedComposition, observe_canonical_revision, source_anchor_from_composition,
};
use crate::audio_input::{AudioInput, CaptureDevice, SampleSink};
use crate::clock::Clock;
use crate::credentials::CaptureHostCredentials;
use crate::error::CaptureError;
use crate::hotkey::parse_hotkey;
use crate::recovery::{
    RecoveryReport, begin_incoming, cleanup_incoming, incoming_capture_ids, incoming_meta,
    read_project_id, recover_capture, remember_project_id, write_committed,
};
use crate::wav::{encode_pcm_wav, publish_wav};

/// Default recording cap: two minutes at 16 kHz mono.
pub const DEFAULT_MAX_DURATION_SAMPLES: u64 = 16_000 * 120;
/// Default published WAV size cap.
pub const DEFAULT_MAX_BYTE_LENGTH: u64 = 8 * 1024 * 1024;

/// Recording budget enforced before a capture is imported.
#[derive(Debug, Clone, Copy)]
pub struct CaptureLimits {
    pub max_duration_samples: u64,
    pub max_byte_length: u64,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_duration_samples: DEFAULT_MAX_DURATION_SAMPLES,
            max_byte_length: DEFAULT_MAX_BYTE_LENGTH,
        }
    }
}

/// Operator-visible host state. `ymm4Disconnected` is idle plus a failed
/// last composition probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PublicHostState {
    Idle,
    Recording,
    Ymm4Disconnected,
    Failed,
}

/// Operator-visible derive progress. Messages are a closed set; no paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostDerivePhase {
    Idle,
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl HostDerivePhase {
    /// Inspect/list token matching [`takegraph_service::TranscriptionJobStatus::as_phase`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

/// Path-free derive status on [`HostStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDeriveStatus {
    pub phase: HostDerivePhase,
    pub capture_id: Option<AnnotationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl HostDeriveStatus {
    fn idle() -> Self {
        Self {
            phase: HostDerivePhase::Idle,
            capture_id: None,
            message: None,
        }
    }

    fn for_phase(phase: HostDerivePhase, capture_id: AnnotationId) -> Self {
        let message = match phase {
            HostDerivePhase::Idle => return Self::idle(),
            HostDerivePhase::Queued | HostDerivePhase::Running => "起こし中",
            HostDerivePhase::Succeeded => "起こし完了",
            HostDerivePhase::Failed => "起こしに失敗しました",
        };
        Self {
            phase,
            capture_id: Some(capture_id),
            message: Some(message.to_owned()),
        }
    }

    fn unconfigured(capture_id: AnnotationId) -> Self {
        Self {
            phase: HostDerivePhase::Failed,
            capture_id: Some(capture_id),
            message: Some("起こし未設定".into()),
        }
    }
}

impl Default for HostDeriveStatus {
    fn default() -> Self {
        Self::idle()
    }
}

/// `GET /v1/status` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub state: PublicHostState,
    pub capture_id: Option<AnnotationId>,
    pub start_frame: Option<i32>,
    pub device_id: Option<String>,
    pub hotkey: String,
    pub project_id: Option<String>,
    pub message: Option<String>,
    pub derive: HostDeriveStatus,
}

/// Model-safe list row: identities, frames, and the audio digest. No paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationSummary {
    pub annotation_id: AnnotationId,
    pub session_id: CaptureSessionId,
    pub start_frame: i32,
    pub end_frame: i32,
    pub scene_id: String,
    pub project_id: String,
    pub source_fingerprint: String,
    pub fps: u32,
    pub stability: CaptureStability,
    pub lifecycle: CaptureLifecycle,
    pub captured_at_utc: String,
    pub audio_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derive_phase: Option<String>,
}

impl AnnotationSummary {
    /// Projects a store row without filesystem fields.
    #[must_use]
    pub fn from_projection(projection: &CaptureProjection) -> Self {
        Self::from_projection_with_job(projection, None)
    }

    /// Projects a store row and the latest derive job, still without paths.
    #[must_use]
    pub fn from_projection_with_job(
        projection: &CaptureProjection,
        job: Option<&TranscriptionJob>,
    ) -> Self {
        Self {
            annotation_id: projection.capture.id,
            session_id: projection.capture.session_id,
            start_frame: projection.capture.start_anchor.frame,
            end_frame: projection.capture.end_anchor.frame,
            scene_id: projection.capture.start_anchor.scene_id.clone(),
            project_id: projection.capture.start_anchor.project_id.clone(),
            source_fingerprint: projection.capture.start_anchor.source_fingerprint.clone(),
            fps: projection.capture.start_anchor.fps,
            stability: projection.stability,
            lifecycle: projection.lifecycle,
            captured_at_utc: projection.capture.captured_at_utc.clone(),
            audio_sha256: projection.capture.audio.audio_sha256.clone(),
            transcript_summary: projection
                .transcript
                .as_ref()
                .map(|transcript| summarize_transcript(&transcript.text)),
            transcript_digest: projection
                .transcript
                .as_ref()
                .map(|transcript| transcript.transcript_digest.clone()),
            derive_phase: job.map(|job| job.status.as_phase().to_owned()),
        }
    }
}

fn summary_from_store(
    store: &AnnotationStore,
    projection: &CaptureProjection,
) -> AnnotationSummary {
    let job = TranscriptionJobStore::open(store)
        .latest_for_capture(projection.capture.id)
        .ok()
        .flatten();
    AnnotationSummary::from_projection_with_job(projection, job.as_ref())
}

fn summarize_transcript(text: &str) -> String {
    let trimmed = text.trim();
    let mut characters = trimmed.chars();
    let prefix: String = characters.by_ref().take(80).collect();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

async fn derive_imported_capture(
    annotation_root: PathBuf,
    project_id: String,
    capture_id: AnnotationId,
    now_utc: String,
) -> Result<(), String> {
    let store = AnnotationStore::open_scoped(&annotation_root, &project_id)
        .map_err(|error| error.to_string())?;
    let host = resolve_transcription_host(None).map_err(|error| error.to_string())?;
    let provider = WhisperCppProvider::new(
        &host.executable,
        &host.model,
        host.language.clone(),
        host.extra_args,
    )
    .map_err(|error| error.to_string())?;
    let projection = store
        .capture(capture_id)
        .map_err(|error| error.to_string())?;
    let audio_path = store
        .audio_artifact_path(&projection.capture.audio.audio_sha256)
        .ok_or_else(|| "captured audio artifact is missing".to_owned())?;
    let jobs = TranscriptionJobStore::open(&store);
    transcribe_capture(
        &store,
        &jobs,
        &provider,
        capture_id,
        &audio_path,
        &host.language,
        host.executable,
        host.model,
        provider.extra_args().to_vec(),
        &now_utc,
    )
    .await
    .map_err(|error| error.to_string())?;
    let _ = interpret_capture(&store, &HeuristicInterpreter, capture_id);
    Ok(())
}

/// Reads the current YMM4 scene. Implementations must not seek or mutate.
#[async_trait]
pub trait CompositionObserver: Send + Sync {
    /// Returns the current composition and an optional observed head.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Composition`] when the scene cannot be observed.
    async fn current(&self) -> Result<ObservedComposition, CaptureError>;
}

/// Bridge-backed observer. Uses `observe_scoped` for the optional revision.
pub struct Ymm4CompositionObserver {
    client: Ymm4BridgeClient,
    state_root: PathBuf,
}

impl Ymm4CompositionObserver {
    /// Wraps an already-authenticated loopback client.
    #[must_use]
    pub fn new(client: Ymm4BridgeClient, state_root: PathBuf) -> Self {
        Self { client, state_root }
    }
}

#[async_trait]
impl CompositionObserver for Ymm4CompositionObserver {
    async fn current(&self) -> Result<ObservedComposition, CaptureError> {
        let snapshot = self
            .client
            .current_scene_composition()
            .await
            .map_err(|error| CaptureError::Composition(error.to_string()))?;
        let observed_canonical_revision =
            observe_canonical_revision(&self.state_root, &snapshot.project_id)?;
        Ok(ObservedComposition {
            snapshot,
            observed_canonical_revision,
        })
    }
}

/// Scripted observer for tests.
pub struct ScriptedObserver {
    queue: Mutex<Vec<Result<ObservedComposition, CaptureError>>>,
    fallback: Mutex<Option<ObservedComposition>>,
}

impl ScriptedObserver {
    /// Repeats the same observation until a queued error or replacement is pushed.
    #[must_use]
    pub fn repeating(observed: ObservedComposition) -> Self {
        Self {
            queue: Mutex::new(Vec::new()),
            fallback: Mutex::new(Some(observed)),
        }
    }

    /// Queues the next observation, consumed before the fallback.
    pub fn push(&self, next: Result<ObservedComposition, CaptureError>) {
        lock(&self.queue).push(next);
    }
}

#[async_trait]
impl CompositionObserver for ScriptedObserver {
    async fn current(&self) -> Result<ObservedComposition, CaptureError> {
        let queued = {
            let mut queue = lock(&self.queue);
            if queue.is_empty() {
                None
            } else {
                Some(queue.remove(0))
            }
        };
        if let Some(next) = queued {
            return next;
        }
        lock(&self.fallback)
            .clone()
            .ok_or_else(|| CaptureError::Composition("no composition observation".into()))
    }
}

/// Long-lived capture session shared by the HTTP API and the hotkey.
pub struct CaptureHost {
    observer: Arc<dyn CompositionObserver>,
    audio: Arc<dyn AudioInput>,
    clock: Arc<dyn Clock>,
    annotation_root: PathBuf,
    hotkey: Mutex<String>,
    hotkey_rebind: Mutex<Option<tokio::sync::watch::Sender<String>>>,
    credentials_path: Mutex<Option<PathBuf>>,
    limits: CaptureLimits,
    session_id: CaptureSessionId,
    auto_transcribe: bool,
    inner: Arc<tokio::sync::Mutex<HostInner>>,
}

struct HostInner {
    recording: Option<ActiveRecording>,
    last_project_id: Option<String>,
    ymm4_reachable: bool,
    last_error: Option<String>,
    last_derive: HostDeriveStatus,
}

struct ActiveRecording {
    capture_id: AnnotationId,
    start_anchor: SourceAnchor,
    buffer: Arc<RecordingBuffer>,
}

struct RecordingBuffer {
    samples: Mutex<Vec<i16>>,
    max_samples: usize,
    max_bytes: u64,
    overflowed: AtomicBool,
}

impl SampleSink for RecordingBuffer {
    fn append_i16(&self, samples: &[i16]) -> Result<(), CaptureError> {
        if self.overflowed.load(Ordering::Relaxed) {
            return Err(CaptureError::DurationLimit);
        }
        let mut held = lock(&self.samples);
        let next_len = held.len().saturating_add(samples.len());
        let wav_bytes = 44u64.saturating_add((next_len as u64).saturating_mul(2));
        if next_len > self.max_samples || wav_bytes > self.max_bytes {
            self.overflowed.store(true, Ordering::Relaxed);
            return if wav_bytes > self.max_bytes {
                Err(CaptureError::SizeLimit)
            } else {
                Err(CaptureError::DurationLimit)
            };
        }
        held.extend_from_slice(samples);
        Ok(())
    }
}

impl CaptureHost {
    /// Builds a host that records into `annotation_root`.
    #[must_use]
    pub fn new(
        observer: Arc<dyn CompositionObserver>,
        audio: Arc<dyn AudioInput>,
        clock: Arc<dyn Clock>,
        annotation_root: PathBuf,
        hotkey: impl Into<String>,
        limits: CaptureLimits,
    ) -> Self {
        Self {
            observer,
            audio,
            clock,
            annotation_root,
            hotkey: Mutex::new(hotkey.into()),
            hotkey_rebind: Mutex::new(None),
            credentials_path: Mutex::new(None),
            limits,
            session_id: CaptureSessionId::new(),
            auto_transcribe: true,
            inner: Arc::new(tokio::sync::Mutex::new(HostInner {
                recording: None,
                last_project_id: None,
                ymm4_reachable: false,
                last_error: None,
                last_derive: HostDeriveStatus::idle(),
            })),
        }
    }

    /// Disables host-local ASR after stop. Tests use this so they never
    /// invoke the operator's whisper binding.
    #[must_use]
    pub fn with_auto_transcribe(mut self, enabled: bool) -> Self {
        self.auto_transcribe = enabled;
        self
    }

    /// Shared annotation-store root.
    #[must_use]
    pub fn annotation_root(&self) -> &Path {
        &self.annotation_root
    }

    /// Configured toggle hotkey label.
    #[must_use]
    pub fn hotkey(&self) -> String {
        lock(&self.hotkey).clone()
    }

    /// Connects panel/HTTP hotkey changes to the listen-loop rebind channel.
    pub fn enable_hotkey_rebind(
        &self,
        rebind: tokio::sync::watch::Sender<String>,
        credentials_path: PathBuf,
    ) {
        *lock(&self.hotkey_rebind) = Some(rebind);
        *lock(&self.credentials_path) = Some(credentials_path);
    }

    /// Validates, persists, and applies a new toggle key.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::UnsupportedHotkey`] for a modifier-only or
    /// unknown binding.
    pub fn set_hotkey(&self, spec: &str) -> Result<String, CaptureError> {
        let parsed = parse_hotkey(spec)?;
        let name = parsed.name.to_owned();
        if let Some(path) = lock(&self.credentials_path).clone() {
            CaptureHostCredentials::persist_hotkey(&path, &name)?;
        }
        *lock(&self.hotkey) = name.clone();
        if let Some(rebind) = lock(&self.hotkey_rebind).as_ref() {
            let _ = rebind.send(name.clone());
        }
        Ok(name)
    }

    /// Restores a previously armed binding without notifying the listen loop.
    pub fn restore_hotkey(&self, spec: &str) -> Result<String, CaptureError> {
        let parsed = parse_hotkey(spec)?;
        let name = parsed.name.to_owned();
        if let Some(path) = lock(&self.credentials_path).clone() {
            CaptureHostCredentials::persist_hotkey(&path, &name)?;
        }
        *lock(&self.hotkey) = name.clone();
        Ok(name)
    }

    /// Clears a prior operator-visible failure after a successful rebind.
    pub async fn clear_error(&self) {
        self.inner.lock().await.last_error = None;
    }

    /// Lists microphones.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Audio`] when devices cannot be enumerated.
    pub fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        self.audio.list_devices()
    }

    /// Selects a microphone while idle.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::DeviceChangeWhileRecording`] or an unknown id.
    pub async fn set_device(&self, id: Option<&str>) -> Result<(), CaptureError> {
        if self.inner.lock().await.recording.is_some() {
            return Err(CaptureError::DeviceChangeWhileRecording);
        }
        self.audio.set_device(id)
    }

    /// Refreshes the YMM4 reachability bit used by [`Self::status`].
    pub async fn refresh_composition(&self) {
        let reachable = self.observer.current().await.is_ok();
        let mut inner = self.inner.lock().await;
        inner.ymm4_reachable = reachable;
        if reachable && inner.recording.is_none() {
            inner.last_error = None;
        }
    }

    /// Operator-visible status. Does not report tokens or filesystem paths.
    pub async fn status(&self) -> HostStatus {
        let inner = self.inner.lock().await;
        let recording = inner.recording.as_ref();
        let state = if recording.is_some() {
            PublicHostState::Recording
        } else if inner.last_error.is_some() {
            PublicHostState::Failed
        } else if inner.ymm4_reachable {
            PublicHostState::Idle
        } else {
            PublicHostState::Ymm4Disconnected
        };
        HostStatus {
            state,
            capture_id: recording.map(|active| active.capture_id),
            start_frame: recording.map(|active| active.start_anchor.frame),
            device_id: self.audio.selected_device_id(),
            hotkey: lock(&self.hotkey).clone(),
            project_id: recording
                .map(|active| active.start_anchor.project_id.clone())
                .or_else(|| inner.last_project_id.clone()),
            message: inner.last_error.clone(),
            derive: inner.last_derive.clone(),
        }
    }

    /// Starts a recording. Fails closed if composition or the microphone fails;
    /// no journal event is written until stop succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::AlreadyRecording`], composition, audio, or I/O
    /// errors.
    pub async fn start(&self) -> Result<AnnotationId, CaptureError> {
        if self.inner.lock().await.recording.is_some() {
            return Err(CaptureError::AlreadyRecording);
        }
        let observed = match self.observer.current().await {
            Ok(observed) => observed,
            Err(error) => {
                let mut inner = self.inner.lock().await;
                inner.ymm4_reachable = false;
                inner.last_error = Some(error.to_string());
                return Err(error);
            }
        };
        let start_anchor = source_anchor_from_composition(
            &observed.snapshot,
            observed.observed_canonical_revision,
        )?;
        let store = self.open_store(&start_anchor.project_id)?;
        let capture_id = AnnotationId::new();
        let started_at = self.clock.now_utc();
        begin_incoming(
            &store,
            &incoming_meta(
                capture_id,
                self.session_id,
                start_anchor.clone(),
                started_at,
            ),
        )?;
        let buffer = Arc::new(RecordingBuffer {
            samples: Mutex::new(Vec::new()),
            max_samples: usize::try_from(self.limits.max_duration_samples).unwrap_or(usize::MAX),
            max_bytes: self.limits.max_byte_length,
            overflowed: AtomicBool::new(false),
        });
        if let Err(error) = self.audio.start(Arc::clone(&buffer) as Arc<dyn SampleSink>) {
            cleanup_incoming(&store, capture_id);
            let mut inner = self.inner.lock().await;
            inner.ymm4_reachable = true;
            inner.last_error = Some(error.to_string());
            return Err(error);
        }
        let mut inner = self.inner.lock().await;
        if inner.recording.is_some() {
            let _ = self.audio.stop();
            cleanup_incoming(&store, capture_id);
            return Err(CaptureError::AlreadyRecording);
        }
        inner.ymm4_reachable = true;
        inner.last_error = None;
        inner.last_project_id = Some(start_anchor.project_id.clone());
        inner.recording = Some(ActiveRecording {
            capture_id,
            start_anchor,
            buffer,
        });
        Ok(capture_id)
    }

    /// Stops the active recording, publishes WAV, and imports the capture.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::NotRecording`], empty/oversize audio, or store
    /// errors. A failed import leaves the committed sidecar for recovery.
    pub async fn stop(&self) -> Result<AnnotationSummary, CaptureError> {
        let active = self
            .inner
            .lock()
            .await
            .recording
            .take()
            .ok_or(CaptureError::NotRecording)?;
        let audio_stop = self.audio.stop();
        let overflowed = active.buffer.overflowed.load(Ordering::Relaxed);
        let samples = lock(&active.buffer.samples).clone();
        let store = self.open_store(&active.start_anchor.project_id)?;
        if let Err(error) = audio_stop {
            cleanup_incoming(&store, active.capture_id);
            self.fail(error.to_string()).await;
            return Err(error);
        }
        if overflowed {
            cleanup_incoming(&store, active.capture_id);
            let error = CaptureError::DurationLimit;
            self.fail(error.to_string()).await;
            return Err(error);
        }
        if samples.is_empty() {
            cleanup_incoming(&store, active.capture_id);
            let error = CaptureError::EmptyRecording;
            self.fail(error.to_string()).await;
            return Err(error);
        }
        if crate::wav::is_silent_pcm(&samples) {
            cleanup_incoming(&store, active.capture_id);
            let error = CaptureError::SilentRecording;
            self.fail(error.to_string()).await;
            return Err(error);
        }
        let bytes = encode_pcm_wav(&samples);
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > self.limits.max_byte_length {
            cleanup_incoming(&store, active.capture_id);
            let error = CaptureError::SizeLimit;
            self.fail(error.to_string()).await;
            return Err(error);
        }
        let end_anchor = if let Ok(observed) = self.observer.current().await {
            self.inner.lock().await.ymm4_reachable = true;
            source_anchor_from_composition(&observed.snapshot, observed.observed_canonical_revision)
                .unwrap_or_else(|_| active.start_anchor.clone())
        } else {
            self.inner.lock().await.ymm4_reachable = false;
            active.start_anchor.clone()
        };
        let (evidence, _) = match publish_wav(store.directory(), &bytes) {
            Ok(published) => published,
            Err(error) => {
                cleanup_incoming(&store, active.capture_id);
                self.fail(error.to_string()).await;
                return Err(error);
            }
        };
        let capture = AnnotationCapture {
            id: active.capture_id,
            session_id: self.session_id,
            start_anchor: active.start_anchor,
            end_anchor,
            audio: evidence,
            captured_at_utc: self.clock.now_utc(),
        };
        if let Err(error) = capture.validate_and_classify() {
            cleanup_incoming(&store, active.capture_id);
            self.fail(error.to_string()).await;
            return Err(error.into());
        }
        if let Err(error) = write_committed(&store, &capture) {
            self.fail(error.to_string()).await;
            return Err(error);
        }
        match store.import_capture(capture.clone()) {
            Ok(_) => {
                cleanup_incoming(&store, active.capture_id);
                self.inner.lock().await.last_error = None;
                self.enqueue_auto_transcribe(
                    capture.start_anchor.project_id.clone(),
                    capture.id,
                    capture.captured_at_utc.clone(),
                )
                .await;
                let projection = store.capture(active.capture_id)?;
                let mut summary = summary_from_store(&store, &projection);
                let derive = self.inner.lock().await.last_derive.clone();
                if summary.derive_phase.is_none()
                    && derive.capture_id == Some(active.capture_id)
                    && derive.phase != HostDerivePhase::Idle
                {
                    summary.derive_phase = Some(derive.phase.as_str().to_owned());
                }
                Ok(summary)
            }
            Err(error) => {
                self.fail(error.to_string()).await;
                Err(error.into())
            }
        }
    }

    /// Abandons the active recording without importing it.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::NotRecording`] when idle.
    pub async fn cancel(&self) -> Result<(), CaptureError> {
        let active = self
            .inner
            .lock()
            .await
            .recording
            .take()
            .ok_or(CaptureError::NotRecording)?;
        let _ = self.audio.stop();
        if let Ok(store) = self.open_store(&active.start_anchor.project_id) {
            cleanup_incoming(&store, active.capture_id);
        }
        self.inner.lock().await.last_error = None;
        Ok(())
    }

    /// Toggle used by the global hotkey. Start/stop errors become `Failed`.
    ///
    /// # Errors
    ///
    /// Returns the underlying start or stop error after recording it on the
    /// host status.
    pub async fn toggle(&self) -> Result<HostStatus, CaptureError> {
        let recording = self.inner.lock().await.recording.is_some();
        let result = if recording {
            self.stop().await.map(|_| ())
        } else {
            self.start().await.map(|_| ())
        };
        if let Err(error) = result {
            self.fail(error.to_string()).await;
            return Err(error);
        }
        Ok(self.status().await)
    }

    async fn enqueue_auto_transcribe(
        &self,
        project_id: String,
        capture_id: AnnotationId,
        now_utc: String,
    ) {
        if !self.auto_transcribe {
            return;
        }
        if !whisper_host_configured() {
            self.inner.lock().await.last_derive = HostDeriveStatus::unconfigured(capture_id);
            return;
        }
        self.inner.lock().await.last_derive =
            HostDeriveStatus::for_phase(HostDerivePhase::Running, capture_id);
        let root = self.annotation_root.clone();
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let result = derive_imported_capture(root, project_id, capture_id, now_utc).await;
            let phase = if result.is_ok() {
                HostDerivePhase::Succeeded
            } else {
                HostDerivePhase::Failed
            };
            inner.lock().await.last_derive = HostDeriveStatus::for_phase(phase, capture_id);
        });
    }

    /// Lists recent captures for the active project.
    ///
    /// # Errors
    ///
    /// Returns a store error when the journal cannot be replayed.
    pub async fn list(&self, limit: usize) -> Result<Vec<AnnotationSummary>, CaptureError> {
        self.list_scoped(self.resolve_project_id().await.as_deref(), limit)
            .await
    }

    /// Lists recent captures for one project identity. Unknown or empty
    /// ids yield an empty list and do not create a store.
    ///
    /// # Errors
    ///
    /// Returns a store error when the journal cannot be replayed.
    pub async fn list_for_project(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<AnnotationSummary>, CaptureError> {
        self.list_scoped(Some(project_id), limit).await
    }

    async fn list_scoped(
        &self,
        project_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AnnotationSummary>, CaptureError> {
        let Some(project_id) = project_id.map(str::trim).filter(|id| !id.is_empty()) else {
            return Ok(Vec::new());
        };
        let store = AnnotationStore::open_scoped(&self.annotation_root, project_id)?;
        if !store.directory().is_dir() {
            return Ok(Vec::new());
        }
        let mut rows: Vec<AnnotationSummary> = store
            .load_state()?
            .captures
            .values()
            .map(|projection| summary_from_store(&store, projection))
            .collect();
        rows.sort_by(|left, right| right.captured_at_utc.cmp(&left.captured_at_utc));
        rows.truncate(limit.max(1));
        Ok(rows)
    }

    /// Loads one capture without filesystem paths.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::UnknownCapture`] when the id is not in any
    /// scanned project store.
    pub async fn get(&self, capture_id: AnnotationId) -> Result<AnnotationSummary, CaptureError> {
        let projection = self.find_projection(capture_id).await?;
        let store = self.open_store(&projection.capture.start_anchor.project_id)?;
        Ok(summary_from_store(&store, &projection))
    }

    /// Dismisses a capture. Audio evidence stays in the journal.
    ///
    /// # Errors
    ///
    /// Returns store or unknown-capture errors.
    pub async fn dismiss(
        &self,
        capture_id: AnnotationId,
        reason: Option<String>,
    ) -> Result<(), CaptureError> {
        let project_id = self.project_id_for(capture_id).await?;
        self.open_store(&project_id)?.dismiss(capture_id, reason)?;
        Ok(())
    }

    /// Explicitly reopens a dismissed capture.
    ///
    /// # Errors
    ///
    /// Returns store or unknown-capture errors.
    pub async fn reopen(&self, capture_id: AnnotationId) -> Result<(), CaptureError> {
        let project_id = self.project_id_for(capture_id).await?;
        self.open_store(&project_id)?.reopen(capture_id)?;
        Ok(())
    }

    /// Replays leftover incoming files after a host restart.
    ///
    /// # Errors
    ///
    /// Returns I/O or store errors. Incomplete WAVs are quarantined.
    pub async fn recover(&self) -> Result<RecoveryReport, CaptureError> {
        let mut report = RecoveryReport::default();
        if !self.annotation_root.is_dir() {
            return Ok(report);
        }
        let mut directories: Vec<PathBuf> = fs::read_dir(&self.annotation_root)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        directories.sort();
        let end_anchor = if let Ok(observed) = self.observer.current().await {
            self.inner.lock().await.ymm4_reachable = true;
            source_anchor_from_composition(&observed.snapshot, observed.observed_canonical_revision)
                .ok()
        } else {
            self.inner.lock().await.ymm4_reachable = false;
            None
        };
        let captured_at = self.clock.now_utc();
        for directory in directories {
            let Some(project_id) = project_id_for_directory(&directory) else {
                continue;
            };
            let store = AnnotationStore::open_scoped(&self.annotation_root, project_id)?;
            remember_project_id(&store)?;
            for capture_id in incoming_capture_ids(store.directory())? {
                recover_capture(
                    &store,
                    capture_id,
                    end_anchor.clone(),
                    &captured_at,
                    &mut report,
                )?;
            }
            if let Some(head) = store.load_state()?.captures.values().last() {
                self.inner.lock().await.last_project_id =
                    Some(head.capture.start_anchor.project_id.clone());
            }
        }
        Ok(report)
    }

    fn open_store(&self, project_id: &str) -> Result<AnnotationStore, CaptureError> {
        let store = AnnotationStore::open_scoped(&self.annotation_root, project_id)?;
        remember_project_id(&store)?;
        Ok(store)
    }

    async fn resolve_project_id(&self) -> Option<String> {
        if let Some(project_id) = self.inner.lock().await.last_project_id.clone() {
            return Some(project_id);
        }
        first_known_project(&self.annotation_root)
    }

    async fn find_projection(
        &self,
        capture_id: AnnotationId,
    ) -> Result<CaptureProjection, CaptureError> {
        if let Some(project_id) = self.inner.lock().await.last_project_id.clone()
            && let Ok(projection) = self.open_store(&project_id)?.capture(capture_id)
        {
            return Ok(projection);
        }
        for project_id in known_projects(&self.annotation_root) {
            if let Ok(projection) = self.open_store(&project_id)?.capture(capture_id) {
                return Ok(projection);
            }
        }
        Err(CaptureError::UnknownCapture(capture_id.0.to_string()))
    }

    async fn project_id_for(&self, capture_id: AnnotationId) -> Result<String, CaptureError> {
        Ok(self
            .find_projection(capture_id)
            .await?
            .capture
            .start_anchor
            .project_id)
    }

    pub(crate) async fn fail(&self, message: String) {
        self.inner.lock().await.last_error = Some(message);
    }

    #[cfg(test)]
    async fn set_derive_for_test(&self, derive: HostDeriveStatus) {
        self.inner.lock().await.last_derive = derive;
    }
}

fn project_id_for_directory(directory: &Path) -> Option<String> {
    if let Some(project_id) = read_project_id(directory) {
        return Some(project_id);
    }
    let committed = incoming_capture_ids(directory).ok()?;
    for capture_id in committed {
        let path = committed_path_for(directory, capture_id);
        if let Ok(record) =
            serde_json::from_slice::<crate::recovery::IncomingCommitted>(&fs::read(path).ok()?)
        {
            return Some(record.capture.start_anchor.project_id);
        }
        let path = meta_path_for(directory, capture_id);
        if let Ok(meta) =
            serde_json::from_slice::<crate::recovery::IncomingMeta>(&fs::read(path).ok()?)
        {
            return Some(meta.project_id);
        }
    }
    None
}

fn committed_path_for(directory: &Path, capture_id: AnnotationId) -> PathBuf {
    directory
        .join("incoming")
        .join(format!("{}.committed.json", capture_id.0))
}

fn meta_path_for(directory: &Path, capture_id: AnnotationId) -> PathBuf {
    directory
        .join("incoming")
        .join(format!("{}.meta.json", capture_id.0))
}

fn known_projects(annotation_root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(annotation_root) else {
        return Vec::new();
    };
    let mut projects = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir()
            && let Some(project_id) = project_id_for_directory(&path)
            && !projects.contains(&project_id)
        {
            projects.push(project_id);
        }
    }
    projects
}

fn first_known_project(annotation_root: &Path) -> Option<String> {
    known_projects(annotation_root).into_iter().next()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::RevisionId;
    use takegraph_node::{
        YMM4_SCENE_COMPOSITION_SCHEMA_VERSION, Ymm4CompositionAvailability,
        Ymm4CompositionCompleteness, Ymm4CompositionViewport, Ymm4SceneCompositionSnapshot,
    };
    use takegraph_service::project_store::DurableProjectStore;
    use uuid::Uuid;

    use crate::audio_input::ScriptedAudio;
    use crate::clock::FixedClock;

    fn snapshot(project: &str, fingerprint: &str, frame: i32) -> Ymm4SceneCompositionSnapshot {
        Ymm4SceneCompositionSnapshot {
            schema_version: YMM4_SCENE_COMPOSITION_SCHEMA_VERSION,
            project_id: project.into(),
            scene_id: "scene-1".into(),
            source_fingerprint: fingerprint.into(),
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

    fn observed(
        project: &str,
        fingerprint: &str,
        frame: i32,
        revision: Option<RevisionId>,
    ) -> ObservedComposition {
        ObservedComposition {
            snapshot: snapshot(project, fingerprint, frame),
            observed_canonical_revision: revision,
        }
    }

    fn host(root: &Path, observer: ScriptedObserver, audio: ScriptedAudio) -> CaptureHost {
        CaptureHost::new(
            Arc::new(observer),
            Arc::new(audio),
            Arc::new(FixedClock("2026-08-14T13:34:57Z".into())),
            root.to_path_buf(),
            "F8",
            CaptureLimits::default(),
        )
        .with_auto_transcribe(false)
    }

    #[tokio::test]
    async fn start_is_refused_without_composition() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        observer.push(Err(CaptureError::Composition("ymm4 down".into())));
        let host = host(&root, observer, ScriptedAudio::new());
        assert!(matches!(
            host.start().await,
            Err(CaptureError::Composition(_))
        ));
        assert!(!root.exists() || incoming_capture_ids(&root).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn happy_path_imports_wav_and_survives_reopen() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 2531, None));
        observer.push(Ok(observed("project-a", "fp-1", 2531, None)));
        observer.push(Ok(observed("project-a", "fp-1", 2698, None)));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![1; 1_600]);
        let host = host(&root, observer, audio);
        let id = host.start().await.unwrap();
        let summary = host.stop().await.unwrap();
        assert_eq!(summary.annotation_id, id);
        assert_eq!(summary.start_frame, 2531);
        assert_eq!(summary.end_frame, 2698);
        assert_eq!(summary.source_fingerprint, "fp-1");
        assert_eq!(summary.fps, 30);
        assert_eq!(summary.stability, CaptureStability::Stable);
        assert!(summary.audio_sha256.starts_with("sha256:"));
        assert!(summary.transcript_summary.is_none());
        assert!(summary.transcript_digest.is_none());
        assert!(summary.derive_phase.is_none());
        assert_eq!(host.status().await.derive.phase, HostDerivePhase::Idle);

        let listed = host.list(20).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(host.list_for_project("project-b", 20).await.unwrap().is_empty());
        assert_eq!(host.list_for_project("project-a", 20).await.unwrap().len(), 1);
        let before = fs::read_dir(&root).map(|entries| entries.count()).unwrap_or(0);
        assert!(host.list_for_project("project-missing", 20).await.unwrap().is_empty());
        let after = fs::read_dir(&root).map(|entries| entries.count()).unwrap_or(0);
        assert_eq!(before, after);
        let reopened = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        assert_eq!(reopened.capture(id).unwrap().capture.id, id);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn silent_recording_is_not_imported() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![0; 1_600]);
        let host = host(&root, observer, audio);
        host.start().await.unwrap();
        assert!(matches!(
            host.stop().await,
            Err(CaptureError::SilentRecording)
        ));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        assert!(store.load_state().unwrap().captures.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn empty_recording_is_not_imported() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        let host = host(&root, observer, ScriptedAudio::new());
        host.start().await.unwrap();
        assert!(matches!(
            host.stop().await,
            Err(CaptureError::EmptyRecording)
        ));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        assert!(store.load_state().unwrap().captures.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn double_start_is_refused() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![3; 800]);
        let host = host(&root, observer, audio);
        host.start().await.unwrap();
        assert!(matches!(
            host.start().await,
            Err(CaptureError::AlreadyRecording)
        ));
        host.cancel().await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn source_changed_is_kept_not_rejected() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        observer.push(Ok(observed("project-a", "fp-1", 10, None)));
        observer.push(Ok(observed("project-a", "fp-2", 40, None)));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![4; 800]);
        let host = host(&root, observer, audio);
        host.start().await.unwrap();
        let summary = host.stop().await.unwrap();
        assert_eq!(summary.stability, CaptureStability::SourceChanged);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn microphone_failure_does_not_create_a_capture() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        let audio = ScriptedAudio::new();
        audio.fail_next_start();
        let host = host(&root, observer, audio);
        assert!(matches!(host.start().await, Err(CaptureError::Audio(_))));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        assert!(store.load_state().unwrap().captures.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn annotation_events_do_not_move_canonical_head() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let state_root = root.join("project-store");
        let annotation_root = root.join("annotations");
        let canonical =
            DurableProjectStore::open_scoped_or_bootstrap(&state_root, "project-a", RevisionId(0))
                .unwrap();
        let head_before = canonical.head().unwrap();
        let observer =
            ScriptedObserver::repeating(observed("project-a", "fp-1", 10, Some(head_before)));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![5; 800]);
        let host = CaptureHost::new(
            Arc::new(observer),
            Arc::new(audio),
            Arc::new(FixedClock("2026-08-14T13:34:57Z".into())),
            annotation_root,
            "F8",
            CaptureLimits::default(),
        )
        .with_auto_transcribe(false);
        host.start().await.unwrap();
        host.stop().await.unwrap();
        assert_eq!(canonical.head().unwrap(), head_before);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn recover_imports_a_complete_partial_after_restart() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 88, None));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![9; 800]);
        let host = host(&root, observer, audio);
        let id = host.start().await.unwrap();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        fs::write(
            crate::recovery::partial_path(&store, id),
            encode_pcm_wav(&[9; 800]),
        )
        .unwrap();
        drop(host);

        let recovered = host_from_root(&root);
        let report = recovered.recover().await.unwrap();
        assert!(report.imported.contains(&id) || recovered.get(id).await.is_ok());
        recovered.get(id).await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn host_from_root(root: &Path) -> CaptureHost {
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 88, None));
        host(root, observer, ScriptedAudio::new())
    }

    #[tokio::test]
    async fn dismiss_requires_explicit_reopen_in_listing() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![6; 800]);
        let host = host(&root, observer, audio);
        let id = host.start().await.unwrap();
        host.stop().await.unwrap();
        host.dismiss(id, Some("not useful".into())).await.unwrap();
        assert_eq!(
            host.get(id).await.unwrap().lifecycle,
            CaptureLifecycle::Dismissed
        );
        host.reopen(id).await.unwrap();
        assert_eq!(
            host.get(id).await.unwrap().lifecycle,
            CaptureLifecycle::Active
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn set_hotkey_updates_status_and_rejects_chords() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let host = host(
            &root,
            ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None)),
            ScriptedAudio::new(),
        );
        assert_eq!(host.hotkey(), "F8");
        assert_eq!(host.set_hotkey("f9").unwrap(), "F9");
        assert_eq!(host.status().await.hotkey, "F9");
        assert_eq!(host.set_hotkey("Ctrl+R").unwrap(), "Ctrl+R");
        assert!(matches!(
            host.set_hotkey("hold"),
            Err(CaptureError::UnsupportedHotkey(_))
        ));
        assert_eq!(host.hotkey(), "Ctrl+R");
        let _ = fs::remove_dir_all(root);
    }

    fn plant_job(
        store: &AnnotationStore,
        capture_id: AnnotationId,
        status: takegraph_service::TranscriptionJobStatus,
    ) {
        let projection = store.capture(capture_id).unwrap();
        TranscriptionJobStore::open(store)
            .enqueue(TranscriptionJob {
                id: Uuid::new_v4(),
                capture_id,
                audio_sha256: projection.capture.audio.audio_sha256,
                provider_id: "whisper-cpp".into(),
                provider_digest: None,
                language: "ja".into(),
                status,
                error: Some(r"C:\secrets\whisper-cli.exe failed".into()),
                transcript_id: None,
                executable: PathBuf::from(r"C:\secrets\whisper-cli.exe"),
                model: PathBuf::from(r"C:\models\ggml-large.bin"),
                extra_args: Vec::new(),
                updated_at_utc: "2026-08-14T13:35:00Z".into(),
            })
            .unwrap();
    }

    #[tokio::test]
    async fn list_reports_job_phase_without_host_paths() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let observer = ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None));
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![8; 800]);
        let host = host(&root, observer, audio);
        let id = host.start().await.unwrap();
        host.stop().await.unwrap();
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        plant_job(
            &store,
            id,
            takegraph_service::TranscriptionJobStatus::Running,
        );
        let listed = host.list(20).await.unwrap();
        assert_eq!(listed[0].derive_phase.as_deref(), Some("running"));
        assert!(listed[0].audio_sha256.starts_with("sha256:"));
        let json = serde_json::to_string(&listed[0]).unwrap();
        assert!(!json.contains("whisper-cli"));
        assert!(!json.contains("ggml-large"));
        assert!(!json.contains(r"C:\\secrets"));
        assert!(!json.contains("executable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn status_derive_is_path_free() {
        let root = std::env::temp_dir().join(format!("takegraph-cap-{}", Uuid::new_v4()));
        let host = host(
            &root,
            ScriptedObserver::repeating(observed("project-a", "fp-1", 10, None)),
            ScriptedAudio::new(),
        );
        let id = AnnotationId::new();
        host.set_derive_for_test(HostDeriveStatus::for_phase(HostDerivePhase::Running, id))
            .await;
        let status = host.status().await;
        assert_eq!(status.derive.phase, HostDerivePhase::Running);
        assert_eq!(status.derive.capture_id, Some(id));
        assert_eq!(status.derive.message.as_deref(), Some("起こし中"));
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("起こし中"));
        assert!(!json.contains("whisper"));
        assert!(!json.contains("executable"));
        assert!(!json.contains(".wav"));
        host.set_derive_for_test(HostDeriveStatus::for_phase(HostDerivePhase::Failed, id))
            .await;
        assert_eq!(
            host.status().await.derive.message.as_deref(),
            Some("起こしに失敗しました")
        );
        let _ = fs::remove_dir_all(root);
    }
}
