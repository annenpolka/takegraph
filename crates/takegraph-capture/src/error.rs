//! Capture-host errors. These are adapter failures, not annotation-store
//! domain rules.

use takegraph_core::AnnotationError;
use takegraph_service::annotation_store::AnnotationStoreError;
use thiserror::Error;

/// Failures of the local capture host.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// `current_scene_composition()` was missing or invalid.
    #[error("composition observation failed: {0}")]
    Composition(String),
    /// The selected microphone could not be opened or enumerated.
    #[error("microphone is unavailable: {0}")]
    Audio(String),
    /// A second start arrived while a recording is already open.
    #[error("already recording")]
    AlreadyRecording,
    /// Stop, cancel, or finalize ran with no active recording.
    #[error("not recording")]
    NotRecording,
    /// The finalized WAV contained no PCM samples.
    #[error("recording is empty")]
    EmptyRecording,
    /// The stream produced only digital silence. Whisper would hallucinate.
    #[error(
        "microphone captured silence; enable Windows microphone access and select a real input, not Steam Streaming"
    )]
    SilentRecording,
    /// The recording exceeded [`crate::DEFAULT_MAX_DURATION_SAMPLES`].
    #[error("recording exceeded the maximum duration")]
    DurationLimit,
    /// The recording exceeded [`crate::DEFAULT_MAX_BYTE_LENGTH`].
    #[error("recording exceeded the maximum size")]
    SizeLimit,
    /// The configured bind address is not loopback.
    #[error("capture host bind must be loopback")]
    NonLoopbackBind,
    /// The request token did not match the host credential.
    #[error("unauthorized")]
    Unauthorized,
    /// No capture with this id exists in the active or scanned stores.
    #[error("capture {0} is unknown")]
    UnknownCapture(String),
    /// Device selection is refused while a recording is open.
    #[error("cannot change the microphone while recording")]
    DeviceChangeWhileRecording,
    /// A source-anchor or audio-evidence field failed validation.
    #[error(transparent)]
    Anchor(#[from] AnnotationError),
    /// The annotation journal refused the mutation.
    #[error(transparent)]
    Store(#[from] AnnotationStoreError),
    /// Filesystem failure.
    #[error("capture host I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// JSON codec failure.
    #[error("capture host payload could not be decoded: {0}")]
    Decode(#[from] serde_json::Error),
    /// WAV bytes were not a complete PCM file.
    #[error("captured audio is not a complete WAV file: {0}")]
    InvalidWav(String),
    /// The bind URL could not be parsed.
    #[error("invalid capture-host endpoint: {0}")]
    Endpoint(#[from] url::ParseError),
    /// Hotkey specification is not a supported toggle key.
    #[error("unsupported hotkey: {0}")]
    UnsupportedHotkey(String),
    /// Windows refused the binding, usually because another program owns it.
    #[error("hotkey {0} is already in use")]
    HotkeyInUse(String),
    /// The global listener thread could not be armed.
    #[error("hotkey listener failed: {0}")]
    HotkeyListener(String),
}

impl CaptureError {
    /// HTTP status used by the loopback API.
    #[must_use]
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::UnknownCapture(_) => 404,
            Self::AlreadyRecording | Self::NotRecording | Self::DeviceChangeWhileRecording => 409,
            Self::HotkeyInUse(_) => 409,
            Self::Composition(_) => 503,
            Self::Audio(_)
            | Self::EmptyRecording
            | Self::SilentRecording
            | Self::DurationLimit
            | Self::SizeLimit
            | Self::Anchor(_)
            | Self::InvalidWav(_)
            | Self::UnsupportedHotkey(_)
            | Self::HotkeyListener(_)
            | Self::NonLoopbackBind
            | Self::Endpoint(_) => 400,
            Self::Store(_) | Self::Io(_) | Self::Decode(_) => 500,
        }
    }
}
