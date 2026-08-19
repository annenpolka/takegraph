//! `takegraph annotation` commands.

use std::path::PathBuf;

use clap::Subcommand;
use serde::Serialize;
use takegraph_capture::{
    CaptureHostCredentials, Clock, DEFAULT_CAPTURE_ENDPOINT, DEFAULT_HOTKEY, ListenConfig,
    SystemClock, default_credentials_path,
};
use takegraph_core::{
    AnnotationId, AnnotationIntent, RevisionId, TemporalReference, TemporalRelation,
};
use takegraph_node::{HeuristicInterpreter, WhisperCppProvider};
use takegraph_service::annotation_store::{AnnotationStore, PromotionStatus};
use takegraph_service::{
    AnnotationDeriveMode, AnnotationDerivePlan, AnnotationDeriveStore, DurableProjectStore,
    TranscriptionJobStore, attach_human_interpretation, attach_human_transcript, find_pin_item,
    interpret_capture, narration_promotion_operations, pin_promotion_operations,
    record_committed_promotion, record_staged_promotion, run_annotation_derive,
    stage_narration_promotion, stage_pin_promotion, stage_unpin_promotion, transcribe_capture,
    unpin_promotion_operations,
};
use uuid::Uuid;

use crate::{
    ProjectStateOptions, Ymm4Connection, commit_timeline_edit, load_ymm4_token, print_json,
    save_json, ymm4_client,
};

/// Voice-annotation capture host commands.
#[derive(Debug, Subcommand)]
pub enum AnnotationCommand {
    /// Run the long-lived capture host (microphone, hotkey, loopback API).
    Listen {
        #[arg(long)]
        hotkey: Option<String>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long, default_value = DEFAULT_CAPTURE_ENDPOINT)]
        bind: String,
        #[arg(long = "capture-credentials", env = "TAKEGRAPH_CAPTURE_CREDENTIALS")]
        capture_credentials: Option<PathBuf>,
        #[arg(long, env = "TAKEGRAPH_CAPTURE_TOKEN")]
        capture_token: Option<String>,
        #[arg(long)]
        device: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
    },
    /// List captured annotations for one project as JSON. Does not start recording.
    List {
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        project_id: Option<String>,
        #[arg(long)]
        source_fingerprint: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Transcribe one capture with a user-managed whisper.cpp executable, or attach a human correction.
    Transcribe {
        #[arg(long)]
        capture: String,
        #[arg(long)]
        executable: Option<PathBuf>,
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value = "ja")]
        language: String,
        /// Extra whisper.cpp args. Silence decoder knobs are filled in;
        /// repeating `--no-speech-thold` / `--entropy-thold` / `--logprob-thold` overrides them.
        #[arg(long = "extra-arg")]
        extra_args: Vec<String>,
        /// Human-corrected text. When set, the executable is not invoked.
        #[arg(long)]
        text: Option<String>,
        #[arg(long, default_value = "operator")]
        reviewer: String,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Derive intent candidates from the current transcript, or attach a human correction.
    Interpret {
        #[arg(long)]
        capture: String,
        /// Intent kinds: note, highlight, narration, cut_candidate, verify.
        #[arg(long = "intent")]
        intents: Vec<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long)]
        question: Option<String>,
        #[arg(long = "draft-hint")]
        draft_hint: Option<String>,
        /// JSON array of AnnotationIntent objects. Overrides --intent.
        #[arg(long = "intents-json")]
        intents_json: Option<String>,
        #[arg(long, default_value = "operator")]
        reviewer: String,
        #[arg(long)]
        relation: Option<String>,
        #[arg(long = "start-offset")]
        start_offset: Option<i32>,
        #[arg(long = "end-offset")]
        end_offset: Option<i32>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Dismiss a capture. Audio evidence is kept.
    Dismiss {
        #[arg(long)]
        capture: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Return a dismissed capture to the active path.
    Reopen {
        #[arg(long)]
        capture: String,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Build a timeline_edit annotation_marker_create pin from one capture.
    Pin {
        #[arg(long)]
        capture: String,
        #[arg(long, default_value_t = 90)]
        layer: i32,
        /// Stage the ordinary timeline_edit plan. Does not execute or mutate YMM4.
        #[arg(long)]
        stage: bool,
        #[arg(long)]
        task: Option<PathBuf>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
    },
    /// Build a timeline_edit annotation_marker_delete unpin from one capture.
    Unpin {
        #[arg(long)]
        capture: String,
        #[arg(long = "entity-id")]
        entity_id: Option<String>,
        #[arg(long = "realization-id")]
        realization_id: Option<String>,
        #[arg(long)]
        frame: Option<i32>,
        #[arg(long)]
        layer: Option<i32>,
        #[arg(long)]
        length: Option<i32>,
        /// Stage the ordinary timeline_edit plan. Does not execute or mutate YMM4.
        #[arg(long)]
        stage: bool,
        #[arg(long)]
        task: Option<PathBuf>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
    },
    /// Build a timeline_edit native_voice_create set from narration intents.
    Promote {
        #[arg(long)]
        capture: String,
        #[arg(long = "character-name")]
        character_name: String,
        #[arg(long, default_value_t = 2)]
        layer: i32,
        #[arg(long = "max-length", default_value_t = 300)]
        max_length: i32,
        /// Stage the ordinary timeline_edit plan. Does not execute or mutate YMM4.
        #[arg(long)]
        stage: bool,
        #[arg(long)]
        task: Option<PathBuf>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
    },
    /// Execute a staged narration timeline_edit with the exact plan digest.
    PromoteCommit {
        #[arg(long)]
        capture: String,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        task: Option<PathBuf>,
        #[arg(long)]
        head: Option<u64>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
    },
    /// Record that a timeline_edit plan was staged from one capture.
    PromotionStage {
        #[arg(long)]
        capture: String,
        #[arg(long = "task-id")]
        task_id: String,
        #[arg(long = "plan-digest")]
        plan_digest: String,
        #[arg(long = "base-revision")]
        base_revision: u64,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Record that a staged promotion committed.
    PromotionCommit {
        #[arg(long)]
        capture: String,
        #[arg(long = "task-id")]
        task_id: String,
        #[arg(long = "committed-revision")]
        committed_revision: u64,
        #[arg(long = "receipt-digest")]
        receipt_digest: String,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Seal a path-free transcription or interpretation plan for later execute.
    DeriveStage {
        #[arg(long)]
        capture: String,
        #[arg(long)]
        mode: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        language: Option<String>,
        #[arg(long = "intents-json")]
        intents_json: Option<String>,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
        #[arg(long)]
        project_id: Option<String>,
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Execute one sealed derive plan. Host ASR paths stay local.
    DeriveRun {
        #[arg(long)]
        handle: String,
        #[arg(long)]
        digest: String,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
    },
    /// Read one derive plan without running it.
    DeriveStatus {
        #[arg(long)]
        handle: String,
        #[arg(
            long,
            env = "TAKEGRAPH_ANNOTATION_STORE_ROOT",
            default_value = ".takegraph/annotation-store"
        )]
        annotation_root: PathBuf,
    },
}

/// Dispatches annotation subcommands.
///
/// # Errors
///
/// Returns credential, bind, or host failures.
pub async fn run(command: AnnotationCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        AnnotationCommand::Listen {
            hotkey,
            annotation_root,
            bind,
            capture_credentials,
            capture_token,
            device,
            connection,
            state,
        } => {
            let ymm4_token = load_ymm4_token(connection.token, connection.credentials)?;
            let credentials_path = capture_credentials.unwrap_or_else(default_credentials_path);
            takegraph_capture::run_listen(ListenConfig {
                annotation_root,
                bind,
                credentials_path: credentials_path.clone(),
                configured_token: capture_token,
                hotkey: resolve_hotkey(hotkey, &credentials_path),
                device_id: device,
                ymm4_endpoint: connection.endpoint,
                ymm4_token,
                ymm4_expected_project_id: connection.expected_project_id,
                project_state_root: state.state_root,
            })
            .await?;
            Ok(())
        }
        AnnotationCommand::List {
            annotation_root,
            limit,
            project_id,
            source_fingerprint,
            connection,
        } => {
            print_json(
                &list_annotations(
                    annotation_root,
                    limit,
                    project_id,
                    source_fingerprint,
                    connection,
                )
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::Transcribe {
            capture,
            executable,
            model,
            language,
            extra_args,
            text,
            reviewer,
            annotation_root,
            project_id,
            connection,
        } => {
            print_json(
                &transcribe_annotation(TranscribeRequest {
                    capture,
                    executable,
                    model,
                    language,
                    extra_args,
                    text,
                    reviewer,
                    annotation_root,
                    project_id,
                    connection,
                })
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::Interpret {
            capture,
            intents,
            reason,
            topic,
            question,
            draft_hint,
            intents_json,
            reviewer,
            relation,
            start_offset,
            end_offset,
            annotation_root,
            project_id,
            connection,
        } => {
            print_json(
                &interpret_annotation(InterpretRequest {
                    capture,
                    intents,
                    reason,
                    topic,
                    question,
                    draft_hint,
                    intents_json,
                    reviewer,
                    relation,
                    start_offset,
                    end_offset,
                    annotation_root,
                    project_id,
                    connection,
                })
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::Dismiss {
            capture,
            reason,
            annotation_root,
            project_id,
            connection,
        } => {
            print_json(
                &lifecycle_annotation(
                    annotation_root,
                    project_id,
                    connection,
                    &capture,
                    LifecycleAction::Dismiss { reason },
                )
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::Reopen {
            capture,
            annotation_root,
            project_id,
            connection,
        } => {
            print_json(
                &lifecycle_annotation(
                    annotation_root,
                    project_id,
                    connection,
                    &capture,
                    LifecycleAction::Reopen,
                )
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::Pin {
            capture,
            layer,
            stage,
            task,
            annotation_root,
            project_id,
            connection,
            state,
        } => {
            print_json(
                &pin_annotation(PinRequest {
                    capture,
                    layer,
                    stage,
                    task,
                    annotation_root,
                    project_id,
                    connection,
                    state_root: state.state_root,
                })
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::Unpin {
            capture,
            entity_id,
            realization_id,
            frame,
            layer,
            length,
            stage,
            task,
            annotation_root,
            project_id,
            connection,
            state,
        } => {
            print_json(
                &unpin_annotation(UnpinRequest {
                    capture,
                    entity_id,
                    realization_id,
                    frame,
                    layer,
                    length,
                    stage,
                    task,
                    annotation_root,
                    project_id,
                    connection,
                    state_root: state.state_root,
                })
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::Promote {
            capture,
            character_name,
            layer,
            max_length,
            stage,
            task,
            annotation_root,
            project_id,
            connection,
            state,
        } => {
            print_json(
                &promote_annotation(PromoteRequest {
                    capture,
                    character_name,
                    layer,
                    max_length,
                    stage,
                    task,
                    annotation_root,
                    project_id,
                    connection,
                    state_root: state.state_root,
                })
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::PromoteCommit {
            capture,
            digest,
            task,
            head,
            annotation_root,
            project_id,
            connection,
            state,
        } => {
            print_json(
                &promote_commit_annotation(PromoteCommitRequest {
                    capture,
                    digest,
                    task,
                    head,
                    annotation_root,
                    project_id,
                    connection,
                    state,
                })
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::PromotionStage {
            capture,
            task_id,
            plan_digest,
            base_revision,
            annotation_root,
            project_id,
            connection,
        } => {
            print_json(
                &record_promotion(
                    annotation_root,
                    project_id,
                    connection,
                    &capture,
                    PromotionRecord::Staged {
                        task_id,
                        plan_digest,
                        base_revision,
                    },
                )
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::PromotionCommit {
            capture,
            task_id,
            committed_revision,
            receipt_digest,
            annotation_root,
            project_id,
            connection,
        } => {
            print_json(
                &record_promotion(
                    annotation_root,
                    project_id,
                    connection,
                    &capture,
                    PromotionRecord::Committed {
                        task_id,
                        committed_revision,
                        receipt_digest,
                    },
                )
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::DeriveStage {
            capture,
            mode,
            text,
            language,
            intents_json,
            annotation_root,
            project_id,
            connection,
        } => {
            print_json(
                &derive_stage(DeriveStageRequest {
                    capture,
                    mode,
                    text,
                    language,
                    intents_json,
                    annotation_root,
                    project_id,
                    connection,
                })
                .await?,
            )?;
            Ok(())
        }
        AnnotationCommand::DeriveRun {
            handle,
            digest,
            annotation_root,
        } => {
            print_json(&derive_run(&handle, &digest, annotation_root).await?)?;
            Ok(())
        }
        AnnotationCommand::DeriveStatus {
            handle,
            annotation_root,
        } => {
            print_json(&derive_status(&handle, annotation_root)?)?;
            Ok(())
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationListItem {
    annotation_id: String,
    session_id: String,
    start_frame: i32,
    end_frame: i32,
    scene_id: String,
    project_id: String,
    source_fingerprint: String,
    fps: u32,
    stability: takegraph_core::CaptureStability,
    lifecycle: takegraph_service::annotation_store::CaptureLifecycle,
    captured_at_utc: String,
    stale: Option<bool>,
    transcript_summary: Option<String>,
    audio_sha256: String,
    transcript_digest: Option<String>,
    intents: Vec<takegraph_core::AnnotationIntent>,
    temporal: Option<takegraph_core::TemporalReference>,
    interpretation_digest: Option<String>,
    derive_phase: Option<&'static str>,
    promotion_status: Option<&'static str>,
    promotion_task_id: Option<String>,
    promotion_plan_digest: Option<String>,
    promotion_base_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin_entity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin_realization_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin_frame: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin_layer: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin_length: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationList {
    project_id: String,
    source_fingerprint: Option<String>,
    annotations: Vec<AnnotationListItem>,
}

async fn list_annotations(
    annotation_root: PathBuf,
    limit: usize,
    project_id: Option<String>,
    source_fingerprint: Option<String>,
    connection: Ymm4Connection,
) -> Result<AnnotationList, Box<dyn std::error::Error>> {
    let (project_id, source_fingerprint) =
        resolve_project_scope(project_id.clone(), source_fingerprint, connection.clone()).await?;
    if project_id.trim().is_empty() {
        return Err("projectId must not be empty".into());
    }
    let store = AnnotationStore::open_scoped(&annotation_root, &project_id)?;
    let jobs = TranscriptionJobStore::open(&store);
    let mut annotations = store
        .load_state()?
        .captures
        .into_values()
        .map(|projection| {
            let job = jobs
                .latest_for_capture(projection.capture.id)
                .ok()
                .flatten();
            project_list_item(projection, source_fingerprint.as_deref(), job.as_ref())
        })
        .collect::<Vec<_>>();
    annotations.sort_by(|left, right| right.captured_at_utc.cmp(&left.captured_at_utc));
    annotations.truncate(limit.max(1));
    if let Ok(snapshot) = ymm4_client(connection)?.snapshot().await {
        for item in &mut annotations {
            if let Ok(capture_id) = parse_capture_id(&item.annotation_id)
                && let Some(pin) = find_pin_item(&snapshot, capture_id)
            {
                item.pin_entity_id = Some(pin.entity_id.clone());
                item.pin_realization_id = pin.realization_id.map(|id| id.to_string());
                item.pin_frame = Some(pin.frame);
                item.pin_layer = Some(pin.layer);
                item.pin_length = Some(pin.length);
            }
        }
    }
    Ok(AnnotationList {
        project_id,
        source_fingerprint,
        annotations,
    })
}

fn project_list_item(
    projection: takegraph_service::annotation_store::CaptureProjection,
    current_fingerprint: Option<&str>,
    job: Option<&takegraph_service::TranscriptionJob>,
) -> AnnotationListItem {
    let stale = current_fingerprint
        .map(|fingerprint| fingerprint != projection.capture.start_anchor.source_fingerprint);
    let transcript_summary = projection
        .transcript
        .as_ref()
        .map(|transcript| summarize_transcript(&transcript.text));
    let (intents, temporal, interpretation_digest) = match projection.interpretation {
        Some(interpretation) => (
            interpretation.intents,
            Some(interpretation.temporal),
            Some(interpretation.interpretation_digest),
        ),
        None => (Vec::new(), None, None),
    };
    let (promotion_status, promotion_task_id, promotion_plan_digest, promotion_base_revision) =
        match projection.promotion {
            Some(promotion) => (
                Some(match promotion.status {
                    PromotionStatus::Staged => "staged",
                    PromotionStatus::Committed => "committed",
                }),
                Some(promotion.task_id),
                Some(promotion.plan_digest),
                Some(promotion.base_revision.0),
            ),
            None => (None, None, None, None),
        };
    let transcript_digest = projection
        .transcript
        .as_ref()
        .map(|transcript| transcript.transcript_digest.clone());
    let derive_phase = job.map(|job| job.status.as_phase());
    AnnotationListItem {
        annotation_id: projection.capture.id.0.to_string(),
        session_id: projection.capture.session_id.0.to_string(),
        start_frame: projection.capture.start_anchor.frame,
        end_frame: projection.capture.end_anchor.frame,
        scene_id: projection.capture.start_anchor.scene_id,
        project_id: projection.capture.start_anchor.project_id,
        source_fingerprint: projection.capture.start_anchor.source_fingerprint,
        fps: projection.capture.start_anchor.fps,
        stability: projection.stability,
        lifecycle: projection.lifecycle,
        captured_at_utc: projection.capture.captured_at_utc,
        stale,
        transcript_summary,
        audio_sha256: projection.capture.audio.audio_sha256,
        transcript_digest,
        intents,
        temporal,
        interpretation_digest,
        derive_phase,
        promotion_status,
        promotion_task_id,
        promotion_plan_digest,
        promotion_base_revision,
        pin_entity_id: None,
        pin_realization_id: None,
        pin_frame: None,
        pin_layer: None,
        pin_length: None,
    }
}

#[derive(Debug)]
struct TranscribeRequest {
    capture: String,
    executable: Option<PathBuf>,
    model: Option<PathBuf>,
    language: String,
    extra_args: Vec<String>,
    text: Option<String>,
    reviewer: String,
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscribeReport {
    capture_id: String,
    transcript_id: String,
    transcript_digest: String,
    provider_id: String,
    provider_digest: String,
    text: String,
    job_status: &'static str,
}

async fn transcribe_annotation(
    request: TranscribeRequest,
) -> Result<TranscribeReport, Box<dyn std::error::Error>> {
    let (project_id, _) =
        resolve_project_scope(request.project_id, None, request.connection).await?;
    let store = AnnotationStore::open_scoped(&request.annotation_root, &project_id)?;
    let jobs = TranscriptionJobStore::open(&store);
    let capture_id = parse_capture_id(&request.capture)?;
    let now_utc = SystemClock.now_utc();
    let transcript = if let Some(text) = request.text.filter(|value| !value.trim().is_empty()) {
        attach_human_transcript(
            &store,
            &jobs,
            capture_id,
            &text,
            &request.reviewer,
            &now_utc,
        )?
    } else {
        let executable = request
            .executable
            .ok_or("transcribe requires --executable and --model unless --text is supplied")?;
        let model = request
            .model
            .ok_or("transcribe requires --executable and --model unless --text is supplied")?;
        let provider = WhisperCppProvider::new(
            &executable,
            &model,
            request.language.clone(),
            request.extra_args,
        )?;
        let projection = store.capture(capture_id)?;
        let audio_path = store
            .audio_artifact_path(&projection.capture.audio.audio_sha256)
            .ok_or("captured audio digest is not a sha256 path")?;
        transcribe_capture(
            &store,
            &jobs,
            &provider,
            capture_id,
            &audio_path,
            &request.language,
            executable,
            model,
            provider.extra_args().to_vec(),
            &now_utc,
        )
        .await?
    };
    Ok(TranscribeReport {
        capture_id: transcript.capture_id.0.to_string(),
        transcript_id: transcript.id.to_string(),
        transcript_digest: transcript.transcript_digest,
        provider_id: transcript.provider_id,
        provider_digest: transcript.provider_digest,
        text: transcript.text,
        job_status: "succeeded",
    })
}

async fn resolve_project_scope(
    project_id: Option<String>,
    source_fingerprint: Option<String>,
    connection: Ymm4Connection,
) -> Result<(String, Option<String>), Box<dyn std::error::Error>> {
    if let Some(project_id) = project_id {
        return Ok((project_id, source_fingerprint));
    }
    let snapshot = ymm4_client(connection)?.current_scene_composition().await?;
    Ok((
        snapshot.project_id,
        Some(source_fingerprint.unwrap_or(snapshot.source_fingerprint)),
    ))
}

fn parse_capture_id(value: &str) -> Result<AnnotationId, Box<dyn std::error::Error>> {
    Ok(AnnotationId(Uuid::parse_str(value.trim())?))
}

#[derive(Debug)]
struct InterpretRequest {
    capture: String,
    intents: Vec<String>,
    reason: Option<String>,
    topic: Option<String>,
    question: Option<String>,
    draft_hint: Option<String>,
    intents_json: Option<String>,
    reviewer: String,
    relation: Option<String>,
    start_offset: Option<i32>,
    end_offset: Option<i32>,
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InterpretReport {
    capture_id: String,
    interpretation_id: String,
    interpretation_digest: String,
    transcript_digest: String,
    model_id: String,
    temporal: TemporalReference,
    intents: Vec<AnnotationIntent>,
}

async fn interpret_annotation(
    request: InterpretRequest,
) -> Result<InterpretReport, Box<dyn std::error::Error>> {
    let (project_id, _) =
        resolve_project_scope(request.project_id, None, request.connection).await?;
    let store = AnnotationStore::open_scoped(&request.annotation_root, &project_id)?;
    let capture_id = parse_capture_id(&request.capture)?;
    let human_intents = parse_human_intents(
        request.intents_json.as_deref(),
        &request.intents,
        request.reason.as_deref(),
        request.topic.as_deref(),
        request.question.as_deref(),
        request.draft_hint.as_deref(),
    )?;
    let interpretation = if let Some(intents) = human_intents {
        let projection = store.capture(capture_id)?;
        let temporal = resolve_human_temporal(
            &projection.capture.start_anchor,
            request.relation.as_deref(),
            request.start_offset,
            request.end_offset,
        )?;
        attach_human_interpretation(&store, capture_id, temporal, intents, &request.reviewer)?
    } else {
        interpret_capture(&store, &HeuristicInterpreter, capture_id)?
    };
    Ok(InterpretReport {
        capture_id: interpretation.capture_id.0.to_string(),
        interpretation_id: interpretation.id.to_string(),
        interpretation_digest: interpretation.interpretation_digest,
        transcript_digest: interpretation.transcript_digest,
        model_id: interpretation.model_id,
        temporal: interpretation.temporal,
        intents: interpretation.intents,
    })
}

fn parse_human_intents(
    intents_json: Option<&str>,
    kinds: &[String],
    reason: Option<&str>,
    topic: Option<&str>,
    question: Option<&str>,
    draft_hint: Option<&str>,
) -> Result<Option<Vec<AnnotationIntent>>, Box<dyn std::error::Error>> {
    if let Some(json) = intents_json.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(serde_json::from_str(json)?));
    }
    if kinds.is_empty() {
        return Ok(None);
    }
    let mut intents = Vec::new();
    for kind in kinds {
        intents.push(match kind.trim() {
            "note" => AnnotationIntent::Note,
            "highlight" => AnnotationIntent::Highlight {
                reason: reason.map(str::to_owned),
            },
            "narration" => AnnotationIntent::Narration {
                topic: topic
                    .filter(|value| !value.trim().is_empty())
                    .ok_or("narration requires --topic")?
                    .to_owned(),
                draft_hint: draft_hint.map(str::to_owned),
            },
            "cut_candidate" => AnnotationIntent::CutCandidate {
                reason: reason.map(str::to_owned),
            },
            "verify" => AnnotationIntent::Verify {
                question: question
                    .filter(|value| !value.trim().is_empty())
                    .ok_or("verify requires --question")?
                    .to_owned(),
            },
            other => return Err(format!("unknown intent kind: {other}").into()),
        });
    }
    Ok(Some(intents))
}

enum LifecycleAction {
    Dismiss { reason: Option<String> },
    Reopen,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleReport {
    capture_id: String,
    lifecycle: takegraph_service::annotation_store::CaptureLifecycle,
}

async fn lifecycle_annotation(
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
    capture: &str,
    action: LifecycleAction,
) -> Result<LifecycleReport, Box<dyn std::error::Error>> {
    let (project_id, _) = resolve_project_scope(project_id, None, connection).await?;
    let store = AnnotationStore::open_scoped(&annotation_root, &project_id)?;
    let capture_id = parse_capture_id(capture)?;
    match action {
        LifecycleAction::Dismiss { reason } => store.dismiss(capture_id, reason)?,
        LifecycleAction::Reopen => store.reopen(capture_id)?,
    }
    Ok(LifecycleReport {
        capture_id: capture_id.0.to_string(),
        lifecycle: store.capture(capture_id)?.lifecycle,
    })
}

#[derive(Debug)]
struct PinRequest {
    capture: String,
    layer: i32,
    stage: bool,
    task: Option<PathBuf>,
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
    state_root: PathBuf,
}

async fn pin_annotation(request: PinRequest) -> Result<PromoteReport, Box<dyn std::error::Error>> {
    let (project_id, _) =
        resolve_project_scope(request.project_id, None, request.connection.clone()).await?;
    let store = AnnotationStore::open_scoped(&request.annotation_root, &project_id)?;
    let capture_id = parse_capture_id(&request.capture)?;
    let projection = store.capture(capture_id)?;
    let interpretation_digest = projection
        .interpretation
        .as_ref()
        .ok_or("capture has no interpretation")?
        .interpretation_digest
        .clone();
    if !request.stage {
        let operations = pin_promotion_operations(&projection, request.layer)?;
        return Ok(PromoteReport {
            capture_id: capture_id.0.to_string(),
            interpretation_digest,
            operations,
            staged: None,
        });
    }
    let client = ymm4_client(request.connection)?;
    let snapshot = client.snapshot().await?;
    let project_store =
        DurableProjectStore::open_scoped(&request.state_root, &snapshot.project_id)?;
    let staged = stage_pin_promotion(
        &store,
        &project_store,
        &client,
        snapshot,
        capture_id,
        request.layer,
    )
    .await?;
    let task_file = request.task.unwrap_or_else(|| {
        request
            .annotation_root
            .join(&project_id)
            .join("promotions")
            .join(format!("{}-pin.task.json", capture_id.0))
    });
    save_json(&task_file, &staged.task)?;
    Ok(PromoteReport {
        capture_id: capture_id.0.to_string(),
        interpretation_digest,
        operations: staged.operations,
        staged: Some(PromoteStagedReport {
            plan_digest: staged.plan_digest,
            operation_id: staged.task.operation_id.to_string(),
            patch_id: staged.task.patch.id.0.to_string(),
            base_revision: staged.task.patch.base.0,
            task_file,
            promotion_status: "staged",
        }),
    })
}

#[derive(Debug)]
struct UnpinRequest {
    capture: String,
    entity_id: Option<String>,
    realization_id: Option<String>,
    frame: Option<i32>,
    layer: Option<i32>,
    length: Option<i32>,
    stage: bool,
    task: Option<PathBuf>,
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
    state_root: PathBuf,
}

async fn unpin_annotation(
    request: UnpinRequest,
) -> Result<PromoteReport, Box<dyn std::error::Error>> {
    let (project_id, _) =
        resolve_project_scope(request.project_id, None, request.connection.clone()).await?;
    let store = AnnotationStore::open_scoped(&request.annotation_root, &project_id)?;
    let capture_id = parse_capture_id(&request.capture)?;
    let projection = store.capture(capture_id)?;
    let interpretation_digest = projection
        .interpretation
        .as_ref()
        .ok_or("capture has no interpretation")?
        .interpretation_digest
        .clone();
    if request.stage {
        let client = ymm4_client(request.connection)?;
        let snapshot = client.snapshot().await?;
        let project_store =
            DurableProjectStore::open_scoped(&request.state_root, &snapshot.project_id)?;
        let staged =
            stage_unpin_promotion(&store, &project_store, &client, snapshot, capture_id).await?;
        let task_file = request.task.unwrap_or_else(|| {
            request
                .annotation_root
                .join(&project_id)
                .join("promotions")
                .join(format!("{}-unpin.task.json", capture_id.0))
        });
        save_json(&task_file, &staged.task)?;
        return Ok(PromoteReport {
            capture_id: capture_id.0.to_string(),
            interpretation_digest,
            operations: staged.operations,
            staged: Some(PromoteStagedReport {
                plan_digest: staged.plan_digest,
                operation_id: staged.task.operation_id.to_string(),
                patch_id: staged.task.patch.id.0.to_string(),
                base_revision: staged.task.patch.base.0,
                task_file,
                promotion_status: "staged",
            }),
        });
    }
    let client = ymm4_client(request.connection)?;
    let snapshot = client.snapshot().await?;
    let pin = find_pin_item(&snapshot, capture_id);
    let entity_id = request
        .entity_id
        .or_else(|| pin.map(|item| item.entity_id.clone()))
        .ok_or("unpin requires a live pin or --entity-id")?;
    let realization_id = request
        .realization_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()?
        .or_else(|| pin.and_then(|item| item.realization_id))
        .ok_or("unpin requires a live pin or --realization-id")?;
    let frame = request
        .frame
        .or_else(|| pin.map(|item| item.frame))
        .ok_or("unpin requires a live pin or --frame")?;
    let layer = request
        .layer
        .or_else(|| pin.map(|item| item.layer))
        .ok_or("unpin requires a live pin or --layer")?;
    let length = request
        .length
        .or_else(|| pin.map(|item| item.length))
        .ok_or("unpin requires a live pin or --length")?;
    let operations = unpin_promotion_operations(
        &projection,
        &entity_id,
        realization_id,
        frame,
        layer,
        length,
    )?;
    Ok(PromoteReport {
        capture_id: capture_id.0.to_string(),
        interpretation_digest,
        operations,
        staged: None,
    })
}

#[derive(Debug)]
struct PromoteRequest {
    capture: String,
    character_name: String,
    layer: i32,
    max_length: i32,
    stage: bool,
    task: Option<PathBuf>,
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
    state_root: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PromoteStagedReport {
    plan_digest: String,
    operation_id: String,
    patch_id: String,
    base_revision: u64,
    task_file: PathBuf,
    promotion_status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PromoteReport {
    capture_id: String,
    interpretation_digest: String,
    operations: Vec<takegraph_service::TimelineEditStageOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    staged: Option<PromoteStagedReport>,
}

async fn promote_annotation(
    request: PromoteRequest,
) -> Result<PromoteReport, Box<dyn std::error::Error>> {
    let (project_id, _) =
        resolve_project_scope(request.project_id, None, request.connection.clone()).await?;
    let store = AnnotationStore::open_scoped(&request.annotation_root, &project_id)?;
    let capture_id = parse_capture_id(&request.capture)?;
    let projection = store.capture(capture_id)?;
    let interpretation_digest = projection
        .interpretation
        .as_ref()
        .ok_or("capture has no interpretation")?
        .interpretation_digest
        .clone();
    if !request.stage {
        let operations = narration_promotion_operations(
            &projection,
            &request.character_name,
            request.layer,
            request.max_length,
        )?;
        return Ok(PromoteReport {
            capture_id: capture_id.0.to_string(),
            interpretation_digest,
            operations,
            staged: None,
        });
    }
    let client = ymm4_client(request.connection)?;
    let snapshot = client.snapshot().await?;
    let project_store =
        DurableProjectStore::open_scoped(&request.state_root, &snapshot.project_id)?;
    let staged = stage_narration_promotion(
        &store,
        &project_store,
        &client,
        snapshot,
        capture_id,
        &request.character_name,
        request.layer,
        request.max_length,
    )
    .await?;
    let task_file = request.task.unwrap_or_else(|| {
        request
            .annotation_root
            .join(&project_id)
            .join("promotions")
            .join(format!("{}.task.json", capture_id.0))
    });
    save_json(&task_file, &staged.task)?;
    Ok(PromoteReport {
        capture_id: capture_id.0.to_string(),
        interpretation_digest,
        operations: staged.operations,
        staged: Some(PromoteStagedReport {
            plan_digest: staged.plan_digest,
            operation_id: staged.task.operation_id.to_string(),
            patch_id: staged.task.patch.id.0.to_string(),
            base_revision: staged.task.patch.base.0,
            task_file,
            promotion_status: "staged",
        }),
    })
}

struct PromoteCommitRequest {
    capture: String,
    digest: String,
    task: Option<PathBuf>,
    head: Option<u64>,
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
    state: ProjectStateOptions,
}

async fn promote_commit_annotation(
    request: PromoteCommitRequest,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let (project_id, _) =
        resolve_project_scope(request.project_id, None, request.connection.clone()).await?;
    let capture_id = parse_capture_id(&request.capture)?;
    let task = request.task.unwrap_or_else(|| {
        request
            .annotation_root
            .join(&project_id)
            .join("promotions")
            .join(format!("{}.task.json", capture_id.0))
    });
    commit_timeline_edit(
        request.connection,
        request.state,
        task,
        request.digest,
        request.head,
        request.annotation_root,
    )
    .await
}

enum PromotionRecord {
    Staged {
        task_id: String,
        plan_digest: String,
        base_revision: u64,
    },
    Committed {
        task_id: String,
        committed_revision: u64,
        receipt_digest: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PromotionRecordReport {
    capture_id: String,
    status: &'static str,
}

async fn record_promotion(
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
    capture: &str,
    record: PromotionRecord,
) -> Result<PromotionRecordReport, Box<dyn std::error::Error>> {
    let (project_id, _) = resolve_project_scope(project_id, None, connection).await?;
    let store = AnnotationStore::open_scoped(&annotation_root, &project_id)?;
    let capture_id = parse_capture_id(capture)?;
    let status = match record {
        PromotionRecord::Staged {
            task_id,
            plan_digest,
            base_revision,
        } => {
            record_staged_promotion(
                &store,
                capture_id,
                &task_id,
                &plan_digest,
                RevisionId(base_revision),
            )?;
            "staged"
        }
        PromotionRecord::Committed {
            task_id,
            committed_revision,
            receipt_digest,
        } => {
            record_committed_promotion(
                &store,
                capture_id,
                &task_id,
                RevisionId(committed_revision),
                &receipt_digest,
            )?;
            "committed"
        }
    };
    Ok(PromotionRecordReport {
        capture_id: capture_id.0.to_string(),
        status,
    })
}

#[derive(Debug)]
struct DeriveStageRequest {
    capture: String,
    mode: String,
    text: Option<String>,
    language: Option<String>,
    intents_json: Option<String>,
    annotation_root: PathBuf,
    project_id: Option<String>,
    connection: Ymm4Connection,
}

async fn derive_stage(
    request: DeriveStageRequest,
) -> Result<takegraph_service::AnnotationDeriveReport, Box<dyn std::error::Error>> {
    let (project_id, _) =
        resolve_project_scope(request.project_id, None, request.connection).await?;
    let store = AnnotationStore::open_scoped(&request.annotation_root, &project_id)?;
    let capture_id = parse_capture_id(&request.capture)?;
    let _ = store.capture(capture_id)?;
    let mode = parse_derive_mode(&request.mode)?;
    let intents = request
        .intents_json
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(serde_json::from_str)
        .transpose()?;
    let plan = AnnotationDerivePlan::stage(
        capture_id,
        project_id,
        mode,
        request.text,
        request.language,
        intents,
    )?;
    AnnotationDeriveStore::open(&request.annotation_root).save(&plan)?;
    Ok(plan.report(None))
}

async fn derive_run(
    handle: &str,
    digest: &str,
    annotation_root: PathBuf,
) -> Result<takegraph_service::AnnotationDeriveReport, Box<dyn std::error::Error>> {
    let handle = Uuid::parse_str(handle.trim())?;
    Ok(run_annotation_derive(&annotation_root, handle, digest, &SystemClock.now_utc()).await?)
}

fn derive_status(
    handle: &str,
    annotation_root: PathBuf,
) -> Result<takegraph_service::AnnotationDeriveReport, Box<dyn std::error::Error>> {
    let handle = Uuid::parse_str(handle.trim())?;
    let plan = AnnotationDeriveStore::open(annotation_root).get(handle)?;
    Ok(plan.report(None))
}

fn parse_derive_mode(value: &str) -> Result<AnnotationDeriveMode, Box<dyn std::error::Error>> {
    match value.trim() {
        "transcribe" => Ok(AnnotationDeriveMode::Transcribe),
        "interpret" => Ok(AnnotationDeriveMode::Interpret),
        "correct" => Ok(AnnotationDeriveMode::Correct),
        other => Err(format!("unknown derive mode: {other}").into()),
    }
}

fn resolve_human_temporal(
    start: &takegraph_core::SourceAnchor,
    relation: Option<&str>,
    start_offset: Option<i32>,
    end_offset: Option<i32>,
) -> Result<TemporalReference, Box<dyn std::error::Error>> {
    let relation = match relation.unwrap_or("at") {
        "before" => TemporalRelation::Before,
        "at" => TemporalRelation::At,
        "after" => TemporalRelation::After,
        "range" => TemporalRelation::Range,
        other => return Err(format!("unknown temporal relation: {other}").into()),
    };
    Ok(TemporalReference {
        reference_frame: start.frame,
        start_offset_frames: start_offset.unwrap_or(0),
        end_offset_frames: end_offset,
        relation,
    })
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

fn resolve_hotkey(configured: Option<String>, credentials_path: &std::path::Path) -> String {
    if let Some(hotkey) = configured.filter(|value| !value.trim().is_empty()) {
        return hotkey;
    }
    std::fs::read(credentials_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CaptureHostCredentials>(&bytes).ok())
        .and_then(|credentials| credentials.hotkey)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::RevisionId;
    use takegraph_core::{
        AnnotationCapture, AnnotationId, CaptureSessionId, CapturedAudioEvidence, SourceAnchor,
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn list_reads_store_without_paths() {
        let root = std::env::temp_dir().join(format!("takegraph-list-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store
            .import_capture(AnnotationCapture {
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
            })
            .unwrap();

        let listed = list_annotations(
            root.clone(),
            20,
            Some("project-a".into()),
            Some("fp-2".into()),
            Ymm4Connection {
                endpoint: "http://127.0.0.1:8766".into(),
                token: Some("unused".into()),
                credentials: None,
                expected_project_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(listed.annotations.len(), 1);
        assert_eq!(listed.annotations[0].start_frame, 10);
        assert_eq!(listed.annotations[0].stale, Some(true));
        assert!(listed.annotations[0].audio_sha256.starts_with("sha256:"));
        assert!(listed.annotations[0].derive_phase.is_none());
        let json = serde_json::to_string(&listed).unwrap();
        assert!(!json.contains("audioPath"));
        assert!(!json.contains(".partial"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn derive_correct_round_trips_without_paths() {
        let root = std::env::temp_dir().join(format!("takegraph-derive-cli-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store
            .import_capture(AnnotationCapture {
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
            })
            .unwrap();
        let unused = || Ymm4Connection {
            endpoint: "http://127.0.0.1:8766".into(),
            token: Some("unused".into()),
            credentials: None,
            expected_project_id: None,
        };
        let staged = derive_stage(DeriveStageRequest {
            capture: id.0.to_string(),
            mode: "correct".into(),
            text: Some("今のところ残す".into()),
            language: None,
            intents_json: None,
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused(),
        })
        .await
        .unwrap();
        assert_eq!(staged.capture_id, id.0.to_string());
        let ran = derive_run(&staged.handle, &staged.plan_digest, root.clone())
            .await
            .unwrap();
        assert_eq!(ran.transcript_summary.as_deref(), Some("今のところ残す"));
        let json = serde_json::to_string(&ran).unwrap();
        assert!(!json.contains("executable"));
        assert!(!json.contains("audioPath"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn transcribe_attaches_text_and_preserves_capture_on_failure() {
        let root = std::env::temp_dir().join(format!("takegraph-transcribe-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store
            .import_capture(AnnotationCapture {
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
            })
            .unwrap();
        let audio_path = store
            .audio_artifact_path(&format!("sha256:{}", "a".repeat(64)))
            .unwrap();
        std::fs::create_dir_all(audio_path.parent().unwrap()).unwrap();
        std::fs::write(&audio_path, b"RIFF").unwrap();

        let unused_connection = || Ymm4Connection {
            endpoint: "http://127.0.0.1:8766".into(),
            token: Some("unused".into()),
            credentials: None,
            expected_project_id: None,
        };
        let missing = transcribe_annotation(TranscribeRequest {
            capture: id.0.to_string(),
            executable: Some(root.join("missing-whisper")),
            model: Some(root.join("missing-model.bin")),
            language: "ja".into(),
            extra_args: Vec::new(),
            text: None,
            reviewer: "operator".into(),
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await;
        assert!(missing.is_err());
        assert!(store.capture(id).unwrap().transcript.is_none());

        let human = transcribe_annotation(TranscribeRequest {
            capture: id.0.to_string(),
            executable: None,
            model: None,
            language: "ja".into(),
            extra_args: Vec::new(),
            text: Some("今のところ残す".into()),
            reviewer: "operator".into(),
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await
        .unwrap();
        assert_eq!(human.text, "今のところ残す");
        assert_eq!(human.provider_id, "human");
        assert_eq!(human.job_status, "succeeded");
        let json = serde_json::to_string(&human).unwrap();
        assert!(!json.contains("executable"));
        assert!(!json.contains("modelPath"));
        assert!(!json.contains("audioPath"));
        assert_eq!(
            store.capture(id).unwrap().transcript.unwrap().text,
            "今のところ残す"
        );
        let listed = list_annotations(
            root.clone(),
            20,
            Some("project-a".into()),
            Some("fp-1".into()),
            unused_connection(),
        )
        .await
        .unwrap();
        assert_eq!(
            listed.annotations[0].transcript_summary.as_deref(),
            Some("今のところ残す")
        );
        assert_eq!(listed.annotations[0].derive_phase, Some("succeeded"));
        assert!(listed.annotations[0].transcript_digest.is_some());

        let executable = write_fake_whisper(&root);
        let model = root.join("model.bin");
        std::fs::write(&model, b"not-a-real-model").unwrap();
        let machine = transcribe_annotation(TranscribeRequest {
            capture: id.0.to_string(),
            executable: Some(executable),
            model: Some(model),
            language: "ja".into(),
            extra_args: Vec::new(),
            text: None,
            reviewer: "operator".into(),
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await
        .unwrap();
        assert_eq!(machine.text, "hello from whisper");
        assert_eq!(machine.provider_id, "whisper-cpp");
        assert_eq!(
            store.capture(id).unwrap().transcript.unwrap().text,
            "hello from whisper"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn interpret_attaches_heuristic_and_human_intents() {
        let root = std::env::temp_dir().join(format!("takegraph-interpret-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&root, "project-a").unwrap();
        let id = AnnotationId::new();
        store
            .import_capture(AnnotationCapture {
                id,
                session_id: CaptureSessionId::new(),
                start_anchor: SourceAnchor {
                    project_id: "project-a".into(),
                    scene_id: "scene-1".into(),
                    source_fingerprint: "fp-1".into(),
                    fps: 60,
                    frame: 2531,
                    observed_canonical_revision: Some(RevisionId(1)),
                },
                end_anchor: SourceAnchor {
                    project_id: "project-a".into(),
                    scene_id: "scene-1".into(),
                    source_fingerprint: "fp-1".into(),
                    fps: 60,
                    frame: 2698,
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
            })
            .unwrap();
        let unused_connection = || Ymm4Connection {
            endpoint: "http://127.0.0.1:8766".into(),
            token: Some("unused".into()),
            credentials: None,
            expected_project_id: None,
        };
        let missing = interpret_annotation(InterpretRequest {
            capture: id.0.to_string(),
            intents: Vec::new(),
            reason: None,
            topic: None,
            question: None,
            draft_hint: None,
            intents_json: None,
            reviewer: "operator".into(),
            relation: None,
            start_offset: None,
            end_offset: None,
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await;
        assert!(missing.is_err());
        assert!(store.capture(id).is_ok());

        transcribe_annotation(TranscribeRequest {
            capture: id.0.to_string(),
            executable: None,
            model: None,
            language: "ja".into(),
            extra_args: Vec::new(),
            text: Some("今のところ三秒前から残す。ここはCompressionの説明を入れる".into()),
            reviewer: "operator".into(),
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await
        .unwrap();

        let heuristic = interpret_annotation(InterpretRequest {
            capture: id.0.to_string(),
            intents: Vec::new(),
            reason: None,
            topic: None,
            question: None,
            draft_hint: None,
            intents_json: None,
            reviewer: "operator".into(),
            relation: None,
            start_offset: None,
            end_offset: None,
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await
        .unwrap();
        assert_eq!(heuristic.model_id, "heuristic-v1");
        assert_eq!(heuristic.temporal.relation, TemporalRelation::Range);
        assert_eq!(heuristic.temporal.start_offset_frames, -180);
        let listed = list_annotations(
            root.clone(),
            20,
            Some("project-a".into()),
            Some("fp-1".into()),
            unused_connection(),
        )
        .await
        .unwrap();
        assert!(!listed.annotations[0].intents.is_empty());
        assert!(listed.annotations[0].temporal.is_some());
        let json = serde_json::to_string(&listed).unwrap();
        assert!(!json.contains("audioPath"));
        assert!(!json.contains("executable"));

        let human = interpret_annotation(InterpretRequest {
            capture: id.0.to_string(),
            intents: vec!["note".into()],
            reason: None,
            topic: None,
            question: None,
            draft_hint: None,
            intents_json: None,
            reviewer: "operator".into(),
            relation: Some("at".into()),
            start_offset: Some(0),
            end_offset: None,
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await
        .unwrap();
        assert_eq!(human.model_id, "human");
        assert_eq!(human.intents, vec![AnnotationIntent::Note]);

        interpret_annotation(InterpretRequest {
            capture: id.0.to_string(),
            intents: vec!["narration".into()],
            reason: None,
            topic: Some("Compression".into()),
            question: None,
            draft_hint: Some("ここで重要なのがPrimary Compressionです。".into()),
            intents_json: None,
            reviewer: "operator".into(),
            relation: Some("range".into()),
            start_offset: Some(-180),
            end_offset: Some(0),
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
        })
        .await
        .unwrap();
        let promoted = promote_annotation(PromoteRequest {
            capture: id.0.to_string(),
            character_name: "ゆっくり霊夢".into(),
            layer: 2,
            max_length: 300,
            stage: false,
            task: None,
            annotation_root: root.clone(),
            project_id: Some("project-a".into()),
            connection: unused_connection(),
            state_root: root.join("project-store"),
        })
        .await
        .unwrap();
        assert_eq!(promoted.operations.len(), 1);
        let json = serde_json::to_string(&promoted).unwrap();
        assert!(json.contains("source_evidence") || json.contains("sourceEvidence"));
        assert!(json.contains("native_voice_create"));
        assert!(!json.contains("audioPath"));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn write_fake_whisper(root: &std::path::Path) -> PathBuf {
        #[cfg(windows)]
        {
            let path = root.join("fake-whisper.cmd");
            std::fs::write(
                &path,
                "@echo off\r\nsetlocal EnableDelayedExpansion\r\nset \"OF=\"\r\n:parse\r\nif \"%~1\"==\"\" goto done\r\nif /I \"%~1\"==\"-of\" (\r\n  set \"OF=%~2\"\r\n  shift\r\n  shift\r\n  goto parse\r\n)\r\nshift\r\ngoto parse\r\n:done\r\nif defined OF (\r\n  >\"%OF%.txt\" echo hello from whisper\r\n) else (\r\n  echo hello from whisper\r\n)\r\n",
            )
            .unwrap();
            path
        }
        #[cfg(not(windows))]
        {
            let path = root.join("fake-whisper.sh");
            std::fs::write(
                &path,
                "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-of\" ]; then OF=$2; shift 2; continue; fi\n  shift\ndone\nif [ -n \"$OF\" ]; then printf 'hello from whisper\\n' > \"$OF.txt\"; else printf 'hello from whisper\\n'; fi\n",
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
            path
        }
    }
}
