//! Loopback HTTP API. Authenticated by `x-takegraph-token`. Recording start
//! is not exposed through MCP.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, middleware, middleware::Next};
use serde::{Deserialize, Serialize};
use takegraph_core::AnnotationId;
use tokio::net::TcpListener;
use url::{Host, Url};
use uuid::Uuid;

use crate::credentials::{CAPTURE_TOKEN_HEADER, tokens_equal};
use crate::error::CaptureError;
use crate::hotkey::SUPPORTED_HOTKEYS;
use crate::session::{AnnotationSummary, CaptureHost};

/// Shared HTTP state.
struct AppState {
    host: Arc<CaptureHost>,
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    error: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    limit: Option<usize>,
    project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceBody {
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HotkeyBody {
    hotkey: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HotkeysBody {
    hotkeys: Vec<String>,
    selected: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthBody {
    status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DevicesBody {
    devices: Vec<crate::audio_input::CaptureDevice>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationsBody {
    annotations: Vec<AnnotationSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    capture_id: AnnotationId,
}

/// Parses `http://127.0.0.1:8767` into a loopback bind address.
///
/// # Errors
///
/// Returns [`CaptureError::NonLoopbackBind`] for any non-loopback host.
pub fn parse_loopback_bind(endpoint: &str) -> Result<SocketAddr, CaptureError> {
    let url = Url::parse(endpoint)?;
    let host = url.host().ok_or(CaptureError::NonLoopbackBind)?;
    let ip = match host {
        Host::Ipv4(ip) if ip.is_loopback() => IpAddr::V4(ip),
        Host::Ipv6(ip) if ip.is_loopback() => IpAddr::V6(ip),
        Host::Domain("localhost") => IpAddr::V4(Ipv4Addr::LOCALHOST),
        _ => return Err(CaptureError::NonLoopbackBind),
    };
    Ok(SocketAddr::new(ip, url.port().unwrap_or(8767)))
}

/// Serves the capture-host API on an already-bound loopback listener.
///
/// # Errors
///
/// Returns I/O errors from the accept loop.
pub async fn serve(
    listener: TcpListener,
    host: Arc<CaptureHost>,
    token: String,
) -> Result<(), CaptureError> {
    let state = Arc::new(AppState { host, token });
    let app = router(state);
    axum::serve(listener, app)
        .await
        .map_err(|error| CaptureError::Io(std::io::Error::other(error)))
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/devices", get(devices))
        .route("/v1/annotations", get(list_annotations))
        .route("/v1/annotations/{id}", get(get_annotation))
        .route("/v1/annotations/{id}/dismiss", post(dismiss))
        .route("/v1/annotations/{id}/reopen", post(reopen))
        .route("/v1/capture/start", post(start))
        .route("/v1/capture/stop", post(stop))
        .route("/v1/capture/cancel", post(cancel))
        .route("/v1/config/device", post(set_device))
        .route("/v1/hotkeys", get(list_hotkeys))
        .route("/v1/config/hotkey", post(set_hotkey))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_token,
        ))
        .with_state(state)
}

async fn require_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorBody>)> {
    let provided = headers
        .get(CAPTURE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !tokens_equal(provided, &state.token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "Unauthorized".into(),
            }),
        ));
    }
    Ok(next.run(request).await)
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { status: "ok" })
}

async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Cached reachability only. Probing YMM4 here would exceed the panel's
    // 3s timeout and show "not running" while the host is actually up.
    Json(state.host.status().await)
}

async fn devices(State(state): State<Arc<AppState>>) -> Result<Json<DevicesBody>, ApiError> {
    Ok(Json(DevicesBody {
        devices: state.host.devices()?,
    }))
}

async fn list_annotations(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<AnnotationsBody>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    let annotations = match query.project_id.as_deref() {
        Some(project_id) => state.host.list_for_project(project_id, limit).await?,
        None => state.host.list(limit).await?,
    };
    Ok(Json(AnnotationsBody { annotations }))
}

async fn get_annotation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<AnnotationSummary>, ApiError> {
    Ok(Json(state.host.get(AnnotationId(id)).await?))
}

async fn dismiss(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.host.dismiss(AnnotationId(id), None).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reopen(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.host.reopen(AnnotationId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start(State(state): State<Arc<AppState>>) -> Result<Json<StartBody>, ApiError> {
    Ok(Json(StartBody {
        capture_id: state.host.start().await?,
    }))
}

async fn stop(State(state): State<Arc<AppState>>) -> Result<Json<AnnotationSummary>, ApiError> {
    Ok(Json(state.host.stop().await?))
}

async fn cancel(State(state): State<Arc<AppState>>) -> Result<StatusCode, ApiError> {
    state.host.cancel().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_device(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeviceBody>,
) -> Result<StatusCode, ApiError> {
    state.host.set_device(body.device_id.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_hotkeys(State(state): State<Arc<AppState>>) -> Json<HotkeysBody> {
    Json(hotkeys_body(state.host.hotkey()))
}

async fn set_hotkey(
    State(state): State<Arc<AppState>>,
    Json(body): Json<HotkeyBody>,
) -> Result<Json<HotkeysBody>, ApiError> {
    Ok(Json(hotkeys_body(state.host.set_hotkey(&body.hotkey)?)))
}

fn hotkeys_body(selected: String) -> HotkeysBody {
    HotkeysBody {
        hotkeys: SUPPORTED_HOTKEYS
            .iter()
            .map(|key| (*key).to_string())
            .collect(),
        selected,
    }
}

struct ApiError(CaptureError);

impl From<CaptureError> for ApiError {
    fn from(value: CaptureError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (
            status,
            Json(ErrorBody {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use async_trait::async_trait;
    use crate::anchor::ObservedComposition;
    use crate::audio_input::ScriptedAudio;
    use crate::clock::FixedClock;
    use crate::error::CaptureError;
    use crate::session::{CaptureLimits, CompositionObserver, PublicHostState, ScriptedObserver};
    use takegraph_node::{
        YMM4_SCENE_COMPOSITION_SCHEMA_VERSION, Ymm4CompositionAvailability,
        Ymm4CompositionCompleteness, Ymm4CompositionViewport, Ymm4SceneCompositionSnapshot,
    };

    fn snapshot(frame: i32) -> Ymm4SceneCompositionSnapshot {
        Ymm4SceneCompositionSnapshot {
            schema_version: YMM4_SCENE_COMPOSITION_SCHEMA_VERSION,
            project_id: "project-a".into(),
            scene_id: "scene-1".into(),
            source_fingerprint: "fp-1".into(),
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

    fn test_host(root: PathBuf) -> Arc<CaptureHost> {
        let observer = ScriptedObserver::repeating(ObservedComposition {
            snapshot: snapshot(12),
            observed_canonical_revision: None,
        });
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![7; 800]);
        Arc::new(
            CaptureHost::new(
                Arc::new(observer),
                Arc::new(audio),
                Arc::new(FixedClock("2026-08-14T13:34:57Z".into())),
                root,
                "F8",
                CaptureLimits::default(),
            )
            .with_auto_transcribe(false),
        )
    }

    #[test]
    fn refuses_non_loopback_binds() {
        assert!(parse_loopback_bind("http://127.0.0.1:8767").is_ok());
        assert!(parse_loopback_bind("http://localhost:8767").is_ok());
        assert!(parse_loopback_bind("http://0.0.0.0:8767").is_err());
        assert!(parse_loopback_bind("http://192.168.1.4:8767").is_err());
    }

    #[tokio::test]
    async fn http_requires_token_and_can_record() {
        let root = std::env::temp_dir().join(format!("takegraph-http-{}", Uuid::new_v4()));
        let host = test_host(root.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let token = "loopback-test-token";
        tokio::spawn(serve(listener, Arc::clone(&host), token.into()));

        let client = reqwest::Client::new();
        let denied = client
            .get(format!("http://{addr}/v1/health"))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);

        let health = client
            .get(format!("http://{addr}/v1/health"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), reqwest::StatusCode::OK);

        let started = client
            .post(format!("http://{addr}/v1/capture/start"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        assert_eq!(started.status(), reqwest::StatusCode::OK);

        let stopped = client
            .post(format!("http://{addr}/v1/capture/stop"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        assert_eq!(stopped.status(), reqwest::StatusCode::OK);
        let body: AnnotationSummary = stopped.json().await.unwrap();
        assert_eq!(body.start_frame, 12);
        assert!(body.audio_sha256.starts_with("sha256:"));

        let status = client
            .get(format!("http://{addr}/v1/status"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        assert_eq!(status.status(), reqwest::StatusCode::OK);
        let status: crate::session::HostStatus = status.json().await.unwrap();
        assert_eq!(status.derive.phase, crate::session::HostDerivePhase::Idle);
        let status_json = serde_json::to_string(&status).unwrap();
        assert!(!status_json.contains("executable"));
        assert!(!status_json.contains(".wav"));

        let listed = client
            .get(format!("http://{addr}/v1/annotations?limit=20"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        let listed: AnnotationsBody = listed.json().await.unwrap();
        assert_eq!(listed.annotations.len(), 1);

        let scoped = client
            .get(format!(
                "http://{addr}/v1/annotations?limit=20&projectId=project-a"
            ))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        let scoped: AnnotationsBody = scoped.json().await.unwrap();
        assert_eq!(scoped.annotations.len(), 1);
        let foreign = client
            .get(format!(
                "http://{addr}/v1/annotations?limit=20&projectId=project-other"
            ))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        let foreign: AnnotationsBody = foreign.json().await.unwrap();
        assert!(foreign.annotations.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn http_can_change_the_toggle_hotkey() {
        let root = std::env::temp_dir().join(format!("takegraph-hotkey-{}", Uuid::new_v4()));
        let host = test_host(root.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let token = "loopback-test-token";
        tokio::spawn(serve(listener, Arc::clone(&host), token.into()));

        let client = reqwest::Client::new();
        let listed = client
            .get(format!("http://{addr}/v1/hotkeys"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        let listed: HotkeysBody = listed.json().await.unwrap();
        assert_eq!(listed.selected, "F8");
        assert!(listed.hotkeys.iter().any(|key| key == "F9"));

        let changed = client
            .post(format!("http://{addr}/v1/config/hotkey"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .json(&serde_json::json!({ "hotkey": "F9" }))
            .send()
            .await
            .unwrap();
        assert_eq!(changed.status(), reqwest::StatusCode::OK);
        let changed: HotkeysBody = changed.json().await.unwrap();
        assert_eq!(changed.selected, "F9");
        assert_eq!(host.status().await.hotkey, "F9");

        let chord = client
            .post(format!("http://{addr}/v1/config/hotkey"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .json(&serde_json::json!({ "hotkey": "Ctrl+R" }))
            .send()
            .await
            .unwrap();
        assert_eq!(chord.status(), reqwest::StatusCode::OK);
        let chord: HotkeysBody = chord.json().await.unwrap();
        assert_eq!(chord.selected, "Ctrl+R");

        let refused = client
            .post(format!("http://{addr}/v1/config/hotkey"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .json(&serde_json::json!({ "hotkey": "hold" }))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), reqwest::StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(root);
    }

    struct SlowObserver;

    #[async_trait]
    impl CompositionObserver for SlowObserver {
        async fn current(&self) -> Result<ObservedComposition, CaptureError> {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Err(CaptureError::Composition("slow observer".into()))
        }
    }

    #[tokio::test]
    async fn status_does_not_wait_for_composition() {
        let root = std::env::temp_dir().join(format!("takegraph-status-{}", Uuid::new_v4()));
        let host = Arc::new(
            CaptureHost::new(
                Arc::new(SlowObserver),
                Arc::new(ScriptedAudio::new()),
                Arc::new(FixedClock("2026-08-14T13:34:57Z".into())),
                root.clone(),
                "F8",
                CaptureLimits::default(),
            )
            .with_auto_transcribe(false),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let token = "loopback-test-token";
        tokio::spawn(serve(listener, Arc::clone(&host), token.into()));

        let started = std::time::Instant::now();
        let status = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(800))
            .build()
            .unwrap()
            .get(format!("http://{addr}/v1/status"))
            .header(CAPTURE_TOKEN_HEADER, token)
            .send()
            .await
            .unwrap();
        assert_eq!(status.status(), reqwest::StatusCode::OK);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let body: crate::session::HostStatus = status.json().await.unwrap();
        assert_eq!(body.state, PublicHostState::Ymm4Disconnected);
        let _ = std::fs::remove_dir_all(root);
    }
}
