//! Local capture host for human voice annotations.
//!
//! This crate owns the microphone, the toggle hotkey, WAV publish, and the
//! loopback HTTP API. It never writes the canonical project store, never
//! calls VOICEVOX, and never starts from a model-facing MCP tool.

mod anchor;
mod audio_input;
mod clock;
mod credentials;
mod error;
mod hotkey;
mod local_server;
mod recovery;
mod session;
mod wav;

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use takegraph_node::Ymm4BridgeClient;
use tokio::net::TcpListener;

pub use anchor::{ObservedComposition, observe_canonical_revision, source_anchor_from_composition};
pub use audio_input::{AudioInput, CaptureDevice, CpalMicrophone, SampleSink, ScriptedAudio};
pub use clock::{Clock, FixedClock, SystemClock};
pub use credentials::{
    CAPTURE_TOKEN_HEADER, CaptureHostCredentials, DEFAULT_CAPTURE_ENDPOINT,
    default_credentials_path,
};
pub use error::CaptureError;
pub use hotkey::{DEFAULT_HOTKEY, HotkeyGuard, SUPPORTED_HOTKEYS, parse_hotkey, spawn_hotkey};
pub use local_server::{parse_loopback_bind, serve};
pub use recovery::{RecoveryReport, incoming_meta};
pub use session::{
    AnnotationSummary, CaptureHost, CaptureLimits, CompositionObserver, DEFAULT_MAX_BYTE_LENGTH,
    DEFAULT_MAX_DURATION_SAMPLES, HostDerivePhase, HostDeriveStatus, HostStatus, PublicHostState,
    ScriptedObserver, Ymm4CompositionObserver,
};
pub use wav::{TARGET_BITS_PER_SAMPLE, TARGET_CHANNELS, TARGET_SAMPLE_RATE, encode_pcm_wav};

/// Configuration for `takegraph annotation listen`.
#[derive(Debug, Clone)]
pub struct ListenConfig {
    pub annotation_root: PathBuf,
    pub bind: String,
    pub credentials_path: PathBuf,
    pub configured_token: Option<String>,
    pub hotkey: String,
    pub device_id: Option<String>,
    pub ymm4_endpoint: String,
    pub ymm4_token: String,
    pub ymm4_expected_project_id: Option<String>,
    pub project_state_root: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListenBanner {
    endpoint: String,
    credentials_path: PathBuf,
    hotkey: String,
    annotation_root: PathBuf,
}

/// Runs the long-lived capture host until Ctrl-C.
///
/// Binds the loopback API before any YMM4 I/O so the panel's 3s poll can
/// see the host even when composition is slow or missing.
///
/// # Errors
///
/// Returns bind, credential, YMM4 client, or I/O failures. Recovery and
/// composition probes run in the background; the process stays up after a
/// YMM4 disconnect so the panel can retry.
pub async fn run_listen(config: ListenConfig) -> Result<(), CaptureError> {
    let bind = parse_loopback_bind(&config.bind)?;
    let mut credentials = CaptureHostCredentials::load_or_create(
        &config.credentials_path,
        format!("http://{bind}"),
        config.configured_token.clone(),
    )?;
    let hotkey = parse_hotkey(&config.hotkey)?.name.to_owned();
    let mut ymm4 = Ymm4BridgeClient::new(&config.ymm4_endpoint, config.ymm4_token.as_str())
        .map_err(|error| CaptureError::Composition(error.to_string()))?;
    if let Some(project_id) = config.ymm4_expected_project_id.clone() {
        ymm4 = ymm4
            .with_expected_project_id(project_id)
            .map_err(|error| CaptureError::Composition(error.to_string()))?;
    }
    let observer = Ymm4CompositionObserver::new(ymm4, config.project_state_root.clone());
    let audio = CpalMicrophone::new();
    if let Some(device_id) = &config.device_id {
        audio.set_device(Some(device_id))?;
    }
    let host = Arc::new(CaptureHost::new(
        Arc::new(observer),
        Arc::new(audio),
        Arc::new(SystemClock),
        config.annotation_root.clone(),
        hotkey.clone(),
        CaptureLimits::default(),
    ));

    let listener = TcpListener::bind(bind).await?;
    let bound = listener.local_addr().map_err(CaptureError::Io)?;
    let endpoint = format!("http://{bound}");
    credentials.endpoint = endpoint.clone();
    credentials.hotkey = Some(hotkey.clone());
    credentials.save(&config.credentials_path)?;

    let bootstrap = Arc::clone(&host);
    tokio::spawn(async move {
        if let Err(error) = bootstrap.recover().await {
            bootstrap.fail(error.to_string()).await;
        }
        loop {
            bootstrap.refresh_composition().await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });

    let banner = ListenBanner {
        endpoint,
        credentials_path: config.credentials_path.clone(),
        hotkey: hotkey.clone(),
        annotation_root: config.annotation_root.clone(),
    };
    writeln!(
        io::stdout(),
        "{}",
        serde_json::to_string(&banner).map_err(CaptureError::Decode)?
    )?;
    io::stdout().flush().map_err(CaptureError::Io)?;

    let (hotkey_tx, mut hotkey_rx) = tokio::sync::mpsc::unbounded_channel();
    let (rebind_tx, mut rebind_rx) = tokio::sync::watch::channel(hotkey);
    host.enable_hotkey_rebind(rebind_tx, config.credentials_path.clone());
    let mut guard = Some(spawn_hotkey(
        rebind_rx.borrow().as_str(),
        hotkey_tx.clone(),
    )?);
    let mut armed = host.hotkey();
    let serving = serve(listener, Arc::clone(&host), credentials.token);
    tokio::pin!(serving);
    loop {
        tokio::select! {
            result = &mut serving => {
                result?;
                break;
            }
            _ = tokio::signal::ctrl_c() => break,
            changed = rebind_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let next = rebind_rx.borrow().clone();
                if next == armed {
                    continue;
                }
                match spawn_hotkey(&next, hotkey_tx.clone()) {
                    Ok(replacement) => {
                        drop(guard.take());
                        guard = Some(replacement);
                        armed = next;
                        host.clear_error().await;
                    }
                    Err(error) => {
                        let _ = host.restore_hotkey(&armed);
                        host.fail(error.to_string()).await;
                    }
                }
            }
            toggle = hotkey_rx.recv() => {
                if toggle.is_none() {
                    break;
                }
                let _ = host.toggle().await;
            }
        }
    }
    Ok(())
}
