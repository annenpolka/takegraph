use std::{fs, io::Write as _, path::PathBuf};

use clap::{Args, Parser, Subcommand};
use takegraph_core::ProjectInitializationMode;
use takegraph_core::{Patch, PatchStatus, ReconciliationDecision, RevisionId};
use takegraph_node::{
    ManagedUtterance, RenderOverwritePolicy, VoiceProvider, VoicevoxClient, Ymm4BridgeClient,
    Ymm4Error, Ymm4NativeVoiceCue, Ymm4NativeVoiceMutation,
};
use takegraph_service::{
    DurableProjectStore, NativeExtensionStageManifest, ProjectOperationStore,
    ReconciliationChildTask, ReconciliationDownstreamPreview, SceneReviewDecision,
    TimelineEditStageManifest, Ymm4ExportPatch, Ymm4NativeExtensionTask,
    Ymm4NativeVoiceExportPatch, Ymm4NativeVoiceMutationPatch, Ymm4TimelineEditTask,
};
use uuid::Uuid;

mod scene;

#[derive(Debug, Parser)]
#[command(name = "takegraph", version, about = "TakeGraph headless tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run deterministic patch lifecycle guards for local adapters.
    PatchCommit {
        #[arg(long)]
        base: u64,
        #[arg(long)]
        head: u64,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        approved_digest: String,
    },
    /// Inspect an existing VOICEVOX ENGINE.
    Voicevox {
        #[command(subcommand)]
        command: VoicevoxCommand,
    },
    /// Inspect and operate the TakeGraph-owned YMM4 bridge.
    Ymm4 {
        #[command(subcommand)]
        command: Ymm4Command,
    },
}

#[derive(Debug, Subcommand)]
enum VoicevoxCommand {
    /// Probe manifest, versions, devices, and speakers.
    Probe {
        #[arg(
            long,
            env = "TAKEGRAPH_VOICEVOX_ENDPOINT",
            default_value = "http://127.0.0.1:50021"
        )]
        endpoint: String,
    },
    /// List available speakers and styles.
    Speakers {
        #[arg(
            long,
            env = "TAKEGRAPH_VOICEVOX_ENDPOINT",
            default_value = "http://127.0.0.1:50021"
        )]
        endpoint: String,
    },
    /// Materialize an immutable query and WAV artifact by speaker/style name.
    Materialize {
        #[arg(
            long,
            env = "TAKEGRAPH_VOICEVOX_ENDPOINT",
            default_value = "http://127.0.0.1:50021"
        )]
        endpoint: String,
        #[arg(long, default_value = "春日部つむぎ")]
        speaker: String,
        #[arg(long, default_value = "ノーマル")]
        style: String,
        #[arg(long)]
        text: String,
        #[arg(long, default_value = "artifacts/voicevox")]
        artifact_root: PathBuf,
    },
}

#[derive(Debug, Args)]
struct Ymm4Connection {
    #[arg(
        long,
        env = "TAKEGRAPH_YMM4_ENDPOINT",
        default_value = "http://127.0.0.1:8766"
    )]
    endpoint: String,
    #[arg(long, env = "TAKEGRAPH_YMM4_TOKEN")]
    token: Option<String>,
    #[arg(long, env = "TAKEGRAPH_YMM4_CREDENTIALS")]
    credentials: Option<PathBuf>,
    /// Refuse snapshot-dependent work if YMM4 switched away from this project.
    #[arg(long, env = "TAKEGRAPH_YMM4_EXPECTED_PROJECT_ID")]
    expected_project_id: Option<String>,
}

#[derive(Debug, Args)]
struct ProjectStateOptions {
    /// Shared root for service-owned canonical project revision stores.
    #[arg(
        long,
        env = "TAKEGRAPH_PROJECT_STATE_ROOT",
        default_value = ".takegraph/project-store"
    )]
    state_root: PathBuf,
}

#[derive(Debug, Args)]
struct ProjectOperationOptions {
    /// Service-owned append-only checkpoint/render/reconciliation journals.
    #[arg(
        long,
        env = "TAKEGRAPH_PROJECT_OPERATION_ROOT",
        default_value = ".takegraph/project-operations"
    )]
    operation_root: PathBuf,
}

#[derive(Debug, Subcommand)]
enum Ymm4Command {
    /// Check plugin, YMM4, and protocol versions.
    Health {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Release the write gate for one operator-acknowledged `recovery_required` journal.
    RecoveryAcknowledge {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        operation_id: Uuid,
    },
    /// List the managed bridge capabilities.
    Capabilities {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Capture the active project fingerprint and managed items.
    Snapshot {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Read the active scene's evaluated composition at the current frame.
    Composition {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Read the service-owned canonical revision for the active YMM4 project.
    CanonicalHead {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
    },
    /// Stage a reviewable binding for the active YMM4 project.
    ProjectInitializationStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long, value_parser = ["adopt_active", "save_untitled"])]
        mode: String,
        /// New absolute `.ymmp` path, required only for `save_untitled`.
        #[arg(long)]
        destination: Option<PathBuf>,
    },
    /// Approve the exact staged project initialization digest.
    ProjectInitializationApprove {
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        operation_id: Uuid,
        #[arg(long)]
        digest: String,
    },
    /// Execute or resume an approved project initialization.
    ProjectInitializationExecute {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        operation_id: Uuid,
    },
    /// Read a durable project initialization lifecycle.
    ProjectInitializationStatus {
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        operation_id: Uuid,
    },
    /// List fixed project controls supported by the active YMM4 version.
    Controls {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Save the active project to its existing path.
    Save {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Undo the most recent YMM4 edit batch.
    Undo {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Redo the most recently undone YMM4 edit batch.
    Redo {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Request a normal YMM4 main-window close.
    Close {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Stage a digest-bound managed export from a JSON utterance manifest.
    ExportStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Approve, apply exactly once, verify by read-back, and finalize a staged export.
    ExportCommit {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Verify the current YMM4 managed items against a staged or committed export.
    ExportVerify {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        patch: PathBuf,
    },
    /// Stage one caller-ordered mixed portable/native voice transaction.
    TimelineEditStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        task: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Approve, atomically apply, verify, and publish one mixed transaction.
    TimelineEditCommit {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        task: PathBuf,
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Verify every managed cue in a staged or committed mixed transaction.
    TimelineEditVerify {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
    },
    /// Read and payload-validate persisted timeline-edit lifecycle state.
    TimelineEditStatus {
        #[arg(long)]
        task: PathBuf,
    },
    /// Stage native YMM4 `VoiceItem` creation from a JSON cue manifest.
    NativeVoiceStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Approve, apply, read back, and finalize a staged native `VoiceItem` batch.
    NativeVoiceCommit {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Verify current native YMM4 `VoiceItems` against a staged or committed patch.
    NativeVoiceVerify {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        patch: PathBuf,
    },
    /// Stage native `VoiceItem` create/update/delete operations from JSON.
    NativeVoiceMutationStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Approve, apply, replay, read back, and durably finalize mutations.
    NativeVoiceMutationCommit {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Verify current native voices and delete absence against the patch.
    NativeVoiceMutationVerify {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        patch: PathBuf,
    },
    /// Export exact WAV and host-bound provenance into TakeGraph-owned CAS.
    NativeVoiceMutationArtifacts {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long, default_value = ".takegraph/artifacts")]
        artifact_root: PathBuf,
        /// Authorized bridge-owned staging root. Defaults to LocalAppData/TakeGraph/native-voice-artifacts.
        #[arg(long)]
        bridge_artifact_root: Option<PathBuf>,
    },
    /// List target descriptors plus their portable planning digests.
    NativeExtensionDescriptors {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Stage portraits/faces/assets/effects/templates from a typed JSON manifest.
    NativeExtensionStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        task: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Recheck and approve the exact native-extension preview digest.
    NativeExtensionApprove {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        task: PathBuf,
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Apply an approved native-extension task and advance canonical revision.
    NativeExtensionApply {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[arg(long)]
        task: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Re-observe current native realizations and verify semantic identity/state.
    NativeExtensionVerify {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
    },
    /// Replay the authenticated receipt and report current verification status.
    NativeExtensionStatus {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
    },
    /// Stage a digest-bound native PNG scene-inspection plan.
    SceneStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        /// Deterministic visual-check profile JSON.
        #[arg(long)]
        profile: PathBuf,
        #[arg(long, default_value = "ymm4-preview-default")]
        profile_id: String,
        #[arg(long, default_value_t = false)]
        alpha: bool,
        #[arg(long, default_value_t = 0)]
        max_actual_frame_delta: u32,
        /// Exact preview frame to capture; repeat for multiple samples.
        #[arg(long, required = true)]
        frame: Vec<u32>,
        #[arg(long)]
        task: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Approve the exact staged scene-inspection digest.
    SceneApprove {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
        #[arg(long)]
        digest: String,
        /// Current visual-check profile; a changed digest makes the task stale.
        #[arg(long)]
        current_profile: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Capture approved frames and ingest immutable PNG evidence.
    SceneCapture {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
        #[arg(long)]
        current_profile: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Replay the same authenticated capture receipt and re-read artifacts.
    SceneReplay {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
        #[arg(long)]
        current_profile: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Verify images and open an explicit human review.
    SceneReview {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
        #[arg(long)]
        reviewer: String,
        #[arg(long)]
        current_profile: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Record a human accept/reject decision after authenticated replay.
    SceneDecide {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
        #[arg(long, value_parser = ["accept", "reject"])]
        decision: String,
        #[arg(long)]
        note: String,
        #[arg(long)]
        current_profile: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Re-evaluate scene/profile staleness and report review evidence.
    SceneStatus {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[arg(long)]
        task: PathBuf,
        #[arg(long)]
        current_profile: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Stage a verified save checkpoint without advancing canonical revision.
    CheckpointStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Execute or resume an existing-path save checkpoint.
    CheckpointExecute {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        operation_id: Uuid,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Read the durable checkpoint lifecycle and file receipt.
    CheckpointStatus {
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        operation_id: Uuid,
    },
    /// List exact render profile descriptors available in YMM4.
    RenderProfiles {
        #[command(flatten)]
        connection: Ymm4Connection,
    },
    /// Stage an authoritative render to an explicit absolute output path.
    RenderStage {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        /// Verified checkpoint operation whose exact saved bytes will be rendered.
        #[arg(long)]
        checkpoint_operation_id: Uuid,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        output: PathBuf,
        /// Explicitly authorize replacing an existing output file.
        #[arg(long, default_value_t = false)]
        overwrite: bool,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Submit or poll one persisted render task once.
    RenderExecute {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        task_id: Uuid,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Read durable render progress or final verified media evidence.
    RenderStatus {
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        task_id: Uuid,
    },
    /// Request cooperative cancellation for a persisted render.
    RenderCancel {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        task_id: Uuid,
    },
    /// Build a managed-subset semantic drift report from durable receipt evidence.
    ReconcileReport {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Preview explicit import/detach/re-export choices from JSON.
    ReconcilePreview {
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        report_digest: String,
        #[arg(long)]
        decisions: PathBuf,
    },
    /// Accept a digest-approved reconcile preview after fresh read-back.
    ReconcileApply {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        report_digest: String,
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Read one durable reconciliation child and its downstream lifecycle.
    ReconcileChildStatus {
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        child_task_id: String,
    },
    /// Independently approve an exact metadata-detach child digest.
    ReconcileDetachApprove {
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        child_task_id: String,
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Execute or idempotently replay an approved metadata detach.
    ReconcileDetachExecute {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        child_task_id: String,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
    /// Dispatch canonical state into an existing exporter's unapproved preview.
    ReconcileReExportDispatch {
        #[command(flatten)]
        connection: Ymm4Connection,
        #[command(flatten)]
        state: ProjectStateOptions,
        #[command(flatten)]
        operations: ProjectOperationOptions,
        #[arg(long)]
        child_task_id: String,
        /// Route-tagged reconciliation re-export manifest JSON.
        #[arg(long)]
        manifest: PathBuf,
        /// Existing-exporter task file to review and approve with that exporter's normal commands.
        #[arg(long)]
        output_task: PathBuf,
        #[arg(long, default_value_t = 0)]
        head: u64,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::PatchCommit {
            base,
            head,
            digest,
            approved_digest,
        } => {
            if approved_digest != digest {
                return Err("approval does not match the staged patch digest".into());
            }

            let mut patch = Patch::draft(RevisionId(base), digest);
            patch.validate()?;
            patch.materialize_preview()?;
            patch.approve()?;
            let revision = patch.commit(RevisionId(head))?;
            println!("{}", serde_json::json!({ "revision": revision.0 }));
        }
        Command::Voicevox { command } => run_voicevox(command).await?,
        Command::Ymm4 { command } => Box::pin(run_ymm4(command)).await?,
    }

    Ok(())
}

async fn run_voicevox(command: VoicevoxCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        VoicevoxCommand::Probe { endpoint } => {
            let capabilities = VoicevoxClient::new(&endpoint)?.probe().await?;
            println!("{}", serde_json::to_string_pretty(&capabilities)?);
        }
        VoicevoxCommand::Speakers { endpoint } => {
            let capabilities = VoicevoxClient::new(&endpoint)?.probe().await?;
            println!("{}", serde_json::to_string_pretty(&capabilities.speakers)?);
        }
        VoicevoxCommand::Materialize {
            endpoint,
            speaker,
            style,
            text,
            artifact_root,
        } => {
            let client = VoicevoxClient::new(&endpoint)?;
            let capabilities = client.probe().await?;
            let selected_speaker = capabilities
                .speakers
                .iter()
                .find(|candidate| candidate.name == speaker)
                .ok_or_else(|| format!("VOICEVOX speaker not found: {speaker}"))?;
            let selected_style = selected_speaker
                .styles
                .iter()
                .find(|candidate| candidate.name == style)
                .ok_or_else(|| {
                    format!(
                        "VOICEVOX style not found for {speaker}: {style}; available: {}",
                        selected_speaker
                            .styles
                            .iter()
                            .map(|value| value.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
            let artifact = client
                .materialize(&text, selected_style.id, &artifact_root)
                .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "speaker": selected_speaker.name,
                    "speakerUuid": selected_speaker.speaker_uuid,
                    "style": selected_style.name,
                    "artifact": artifact,
                }))?
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run_ymm4(command: Ymm4Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Ymm4Command::Health { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.health().await?)?;
        }
        Ymm4Command::RecoveryAcknowledge {
            connection,
            state,
            operation_id,
        } => {
            let client = ymm4_client(connection)?;
            let journal = match client.acknowledge_recovery(operation_id).await {
                Ok(journal) => Some(journal),
                Err(Ymm4Error::Bridge { status, message })
                    if status.as_u16() == 409 && message.contains("not operator-pending") =>
                {
                    None
                }
                Err(Ymm4Error::StaleExternalState { .. }) => None,
                Err(error) => return Err(error.into()),
            };
            let snapshot = client.snapshot().await?;
            let store = DurableProjectStore::open_scoped(&state.state_root, &snapshot.project_id)?;
            store.acknowledge_recovery_required_reservation(operation_id)?;
            print_json(&serde_json::json!({
                "journal": journal,
                "canonicalReservationReleased": true,
                "projectId": snapshot.project_id,
                "revision": store.head()?,
            }))?;
        }
        Ymm4Command::Capabilities { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.capabilities().await?)?;
        }
        Ymm4Command::Snapshot { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.snapshot().await?)?;
        }
        Ymm4Command::Composition { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.current_scene_composition().await?)?;
        }
        Ymm4Command::CanonicalHead { connection, state } => {
            let client = ymm4_client(connection)?;
            let snapshot = client.snapshot().await?;
            let canonical =
                DurableProjectStore::observe_scoped(&state.state_root, &snapshot.project_id)?;
            print_json(&serde_json::json!({
                "projectId": snapshot.project_id,
                "initialized": canonical.is_some(),
                "revision": canonical.map(|state| state.head),
            }))?;
        }
        Ymm4Command::ProjectInitializationStage {
            connection,
            state,
            operations,
            mode,
            destination,
        } => {
            let client = ymm4_client(connection)?;
            let mode = match mode.as_str() {
                "adopt_active" => ProjectInitializationMode::AdoptActive,
                "save_untitled" => ProjectInitializationMode::SaveUntitled,
                _ => return Err("unsupported project initialization mode".into()),
            };
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let record = operation_store
                .stage_project_initialization(&state.state_root, &client, mode, destination)
                .await?;
            print_json(&record)?;
        }
        Ymm4Command::ProjectInitializationApprove {
            operations,
            operation_id,
            digest,
        } => {
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(&operation_store.approve_project_initialization(operation_id, &digest)?)?;
        }
        Ymm4Command::ProjectInitializationExecute {
            connection,
            state,
            operations,
            operation_id,
        } => {
            let client = ymm4_client(connection)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(
                &operation_store
                    .execute_project_initialization(&state.state_root, &client, operation_id)
                    .await?,
            )?;
        }
        Ymm4Command::ProjectInitializationStatus {
            operations,
            operation_id,
        } => {
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(&operation_store.project_initialization_status(operation_id)?)?;
        }
        Ymm4Command::Controls { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.project_controls().await?)?;
        }
        Ymm4Command::Save { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.save_project().await?)?;
        }
        Ymm4Command::Undo { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.undo().await?)?;
        }
        Ymm4Command::Redo { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.redo().await?)?;
        }
        Ymm4Command::Close { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.close_application().await?)?;
        }
        Ymm4Command::ExportStage {
            connection,
            state,
            manifest,
            patch,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let utterances: Vec<ManagedUtterance> = serde_json::from_slice(&fs::read(manifest)?)?;
            let snapshot = client.snapshot().await?;
            let store = DurableProjectStore::open_scoped(&state.state_root, &snapshot.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            let export =
                Ymm4ExportPatch::stage_from_snapshot(&client, canonical_head, snapshot, utterances)
                    .await?;
            save_json(&patch, &export)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "patchId": export.patch.id,
                    "digest": export.patch.digest,
                    "baseRevision": export.patch.base,
                    "operationId": export.operation_id,
                    "project": export.target,
                    "plan": export.plan,
                    "targetPlanDigest": export.target_plan.canonical_digest()?,
                    "targetPlan": export.target_plan,
                    "patchFile": patch,
                }))?
            );
        }
        Ymm4Command::ExportCommit {
            connection,
            state,
            patch,
            digest,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let mut export: Ymm4ExportPatch = serde_json::from_slice(&fs::read(&patch)?)?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &export.target.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            export.approve(&digest, export.patch.base)?;
            save_json(&patch, &export)?;
            let outcome = export
                .apply_and_finalize_durable(&client, &store, canonical_head)
                .await?;
            save_json(&patch, &export)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "baseRevision": export.patch.base,
                    "revision": outcome.revision,
                    "canonicalReplay": outcome.canonical_replay,
                    "operationId": export.operation_id,
                    "receipt": export.receipt(),
                    "patchFile": patch,
                }))?
            );
        }
        Ymm4Command::ExportVerify { connection, patch } => {
            let client = ymm4_client(connection)?;
            let export: Ymm4ExportPatch = serde_json::from_slice(&fs::read(&patch)?)?;
            export.verify(&client).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "verified": true,
                    "operationId": export.operation_id,
                    "patchFile": patch,
                }))?
            );
        }
        Ymm4Command::TimelineEditStage {
            connection,
            state,
            manifest,
            task,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let manifest: TimelineEditStageManifest = serde_json::from_slice(&fs::read(manifest)?)?;
            let snapshot = client.snapshot().await?;
            let store = DurableProjectStore::open_scoped(&state.state_root, &snapshot.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            let staged = Ymm4TimelineEditTask::stage_from_snapshot(
                &client,
                canonical_head,
                snapshot,
                manifest,
            )
            .await?;
            save_json(&task, &staged)?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "patchId": staged.patch.id,
                "digest": staged.patch.digest,
                "baseRevision": staged.patch.base,
                "operationId": staged.operation_id,
                "project": staged.target,
                "planDigest": staged.timeline_edit_plan.canonical_digest()?,
                "timelineEditPlan": staged.timeline_edit_plan,
            }))?;
        }
        Ymm4Command::TimelineEditCommit {
            connection,
            state,
            task,
            digest,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let mut staged = Ymm4TimelineEditTask::from_json_slice(&fs::read(&task)?)?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &staged.target.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            let authorization_head =
                timeline_edit_authorization_head(&staged.patch, canonical_head);
            staged.approve(&digest, authorization_head)?;
            save_json(&task, &staged)?;
            let outcome = staged
                .apply_and_finalize_durable(&client, &store, canonical_head)
                .await;
            // Persist an authenticated terminal failure receipt as well as a
            // successful commit. Status recovery must not collapse
            // recovery-required or rolled-back work back to merely Approved.
            save_json(&task, &staged)?;
            let outcome = outcome?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "baseRevision": staged.patch.base,
                "revision": outcome.revision,
                "canonicalReplay": outcome.canonical_replay,
                "operationId": staged.operation_id,
                "receipt": staged.receipt(),
                "status": staged.patch.status,
            }))?;
        }
        Ymm4Command::TimelineEditVerify { connection, task } => {
            let client = ymm4_client(connection)?;
            let staged = Ymm4TimelineEditTask::from_json_slice(&fs::read(&task)?)?;
            staged.verify_current(&client).await?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "operationId": staged.operation_id,
                "verified": true,
                "receipt": staged.receipt(),
            }))?;
        }
        Ymm4Command::TimelineEditStatus { task } => {
            let staged = Ymm4TimelineEditTask::from_json_slice(&fs::read(&task)?)?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "patchStatus": staged.patch.status,
                "baseRevision": staged.patch.base,
                "digest": staged.patch.digest,
                "approvedDigest": staged.patch.approved_digest,
                "operationId": staged.operation_id,
                "receiptStatus": staged.receipt().map(|receipt| &receipt.status),
                "receipt": staged.receipt(),
            }))?;
        }
        Ymm4Command::NativeVoiceStage {
            connection,
            state,
            manifest,
            patch,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let cues: Vec<Ymm4NativeVoiceCue> = serde_json::from_slice(&fs::read(manifest)?)?;
            let snapshot = client.snapshot().await?;
            let store = DurableProjectStore::open_scoped(&state.state_root, &snapshot.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            let export = Ymm4NativeVoiceExportPatch::stage_from_snapshot(
                &client,
                canonical_head,
                snapshot,
                cues,
            )
            .await?;
            save_json(&patch, &export)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "patchId": export.patch.id,
                    "digest": export.patch.digest,
                    "baseRevision": export.patch.base,
                    "operationId": export.operation_id,
                    "project": export.target,
                    "plan": export.plan,
                    "targetPlanDigest": export.target_plan.canonical_digest()?,
                    "targetPlan": export.target_plan,
                    "patchFile": patch,
                }))?
            );
        }
        Ymm4Command::NativeVoiceCommit {
            connection,
            state,
            patch,
            digest,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let mut export: Ymm4NativeVoiceExportPatch =
                serde_json::from_slice(&fs::read(&patch)?)?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &export.target.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            export.approve(&digest, export.patch.base)?;
            save_json(&patch, &export)?;
            let outcome = export
                .apply_and_finalize_durable(&client, &store, canonical_head)
                .await?;
            save_json(&patch, &export)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "baseRevision": export.patch.base,
                    "revision": outcome.revision,
                    "canonicalReplay": outcome.canonical_replay,
                    "operationId": export.operation_id,
                    "receipt": export.receipt(),
                    "patchFile": patch,
                }))?
            );
        }
        Ymm4Command::NativeVoiceVerify { connection, patch } => {
            let client = ymm4_client(connection)?;
            let export: Ymm4NativeVoiceExportPatch = serde_json::from_slice(&fs::read(&patch)?)?;
            export.verify(&client).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "verified": true,
                    "operationId": export.operation_id,
                    "patchFile": patch,
                }))?
            );
        }
        Ymm4Command::NativeVoiceMutationStage {
            connection,
            state,
            manifest,
            patch,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let mutations: Vec<Ymm4NativeVoiceMutation> =
                serde_json::from_slice(&fs::read(manifest)?)?;
            let snapshot = client.snapshot().await?;
            let store = DurableProjectStore::open_scoped(&state.state_root, &snapshot.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            let staged = Ymm4NativeVoiceMutationPatch::stage_from_snapshot(
                &client,
                canonical_head,
                snapshot,
                mutations,
            )
            .await?;
            save_json(&patch, &staged)?;
            print_json(&serde_json::json!({
                "patchId": staged.patch.id,
                "digest": staged.patch.digest,
                "baseRevision": staged.patch.base,
                "operationId": staged.operation_id,
                "project": staged.target,
                "plan": staged.plan,
                "capabilityDigest": staged.capability_digest,
                "patchFile": patch,
            }))?;
        }
        Ymm4Command::NativeVoiceMutationCommit {
            connection,
            state,
            patch,
            digest,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let mut mutation = Ymm4NativeVoiceMutationPatch::from_json_slice(&fs::read(&patch)?)?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &mutation.target.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            mutation.approve(&digest, mutation.patch.base)?;
            save_json(&patch, &mutation)?;
            let outcome = mutation
                .apply_and_finalize_durable(&client, &store, canonical_head)
                .await?;
            save_json(&patch, &mutation)?;
            print_json(&serde_json::json!({
                "baseRevision": mutation.patch.base,
                "revision": outcome.revision,
                "canonicalReplay": outcome.canonical_replay,
                "operationId": mutation.operation_id,
                "replayVerified": true,
                "receipt": mutation.receipt(),
                "patchFile": patch,
            }))?;
        }
        Ymm4Command::NativeVoiceMutationVerify { connection, patch } => {
            let client = ymm4_client(connection)?;
            let mutation = Ymm4NativeVoiceMutationPatch::from_json_slice(&fs::read(&patch)?)?;
            mutation.verify(&client).await?;
            print_json(&serde_json::json!({
                "verified": true,
                "operationId": mutation.operation_id,
                "patchFile": patch,
            }))?;
        }
        Ymm4Command::NativeVoiceMutationArtifacts {
            connection,
            state,
            patch,
            artifact_root,
            bridge_artifact_root,
        } => {
            let client = ymm4_client(connection)?;
            let mut mutation = Ymm4NativeVoiceMutationPatch::from_json_slice(&fs::read(&patch)?)?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &mutation.target.project_id)?;
            let bridge_artifact_root = match bridge_artifact_root {
                Some(path) => path,
                None => ymm4_native_voice_bridge_artifact_root()?,
            };
            mutation
                .capture_artifacts(&client, &store, &bridge_artifact_root, &artifact_root)
                .await?;
            mutation.verify_artifacts(&artifact_root)?;
            save_json(&patch, &mutation)?;
            print_json(&serde_json::json!({
                "operationId": mutation.operation_id,
                "artifactSemantics": {
                    "audio": "exact_wav",
                    "provenance": "normalized_host_bound_voice_state",
                    "portableSynthesisQuery": false,
                },
                "artifacts": mutation.artifacts(),
                "patchFile": patch,
            }))?;
        }
        Ymm4Command::NativeExtensionDescriptors { connection } => {
            let client = ymm4_client(connection)?;
            let target_catalog = client.native_descriptors().await?;
            let planning_catalog = target_catalog.planning_catalog()?;
            let planning_descriptor_digests = target_catalog.planning_descriptor_digests()?;
            print_json(&serde_json::json!({
                "targetCatalog": target_catalog,
                "planningCatalog": planning_catalog,
                "planningDescriptorDigests": planning_descriptor_digests,
            }))?;
        }
        Ymm4Command::NativeExtensionStage {
            connection,
            state,
            manifest,
            task,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let manifest: NativeExtensionStageManifest =
                serde_json::from_slice(&fs::read(manifest)?)?;
            let staged = Ymm4NativeExtensionTask::stage(
                &client,
                RevisionId(head),
                manifest,
                ymm4_native_extension_artifact_root()?,
            )
            .await?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &staged.target.project_id)?;
            require_canonical_head(&store, RevisionId(head))?;
            save_json(&task, &staged)?;
            let lossy_approvals = staged
                .plan
                .operations
                .iter()
                .filter(|operation| !operation.preservation.lossy_fields.is_empty())
                .map(|operation| {
                    serde_json::json!({
                        "logicalKey": operation.intent.logical_key(),
                        "lossyFields": operation.preservation.lossy_fields,
                        "approvedLossyFields": operation.preservation.approved_lossy_fields,
                    })
                })
                .collect::<Vec<_>>();
            print_json(&serde_json::json!({
                "taskFile": task,
                "patchId": staged.patch.id,
                "digest": staged.patch.digest,
                "baseRevision": staged.patch.base,
                "operationId": staged.operation_id,
                "project": staged.target,
                "plan": staged.plan,
                "bridgeWarnings": staged.bridge_preview.warnings,
                "lossyApprovals": lossy_approvals,
                "artifacts": staged.artifacts,
            }))?;
        }
        Ymm4Command::NativeExtensionApprove {
            connection,
            state,
            task,
            digest,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let mut staged = Ymm4NativeExtensionTask::from_json_slice(&fs::read(&task)?)?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &staged.target.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            let approval_head = if staged.patch.status == takegraph_core::PatchStatus::Previewable {
                staged.revalidate_preview(&client).await?;
                canonical_head
            } else {
                staged.patch.base
            };
            staged.approve(&digest, approval_head)?;
            save_json(&task, &staged)?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "operationId": staged.operation_id,
                "digest": staged.patch.digest,
                "status": staged.patch.status,
            }))?;
        }
        Ymm4Command::NativeExtensionApply {
            connection,
            state,
            task,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let mut staged = Ymm4NativeExtensionTask::from_json_slice(&fs::read(&task)?)?;
            let store =
                DurableProjectStore::open_scoped(&state.state_root, &staged.target.project_id)?;
            let canonical_head = require_canonical_head(&store, RevisionId(head))?;
            let outcome = staged
                .apply_and_finalize_durable(&client, &store, canonical_head)
                .await?;
            save_json(&task, &staged)?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "operationId": staged.operation_id,
                "baseRevision": staged.patch.base,
                "revision": outcome.revision,
                "canonicalReplay": outcome.canonical_replay,
                "receipt": staged.receipt,
                "status": staged.patch.status,
            }))?;
        }
        Ymm4Command::NativeExtensionVerify { connection, task } => {
            let client = ymm4_client(connection)?;
            let mut staged = Ymm4NativeExtensionTask::from_json_slice(&fs::read(&task)?)?;
            if staged.receipt.is_some() {
                staged.replay_status(&client).await?;
            }
            staged.verify_current(&client).await?;
            save_json(&task, &staged)?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "operationId": staged.operation_id,
                "verified": true,
                "receipt": staged.receipt,
            }))?;
        }
        Ymm4Command::NativeExtensionStatus { connection, task } => {
            let client = ymm4_client(connection)?;
            let mut staged = Ymm4NativeExtensionTask::from_json_slice(&fs::read(&task)?)?;
            let current_verified = if staged.receipt.is_some() {
                staged.replay_status(&client).await?;
                staged.verify_current(&client).await?;
                true
            } else {
                staged.revalidate_preview(&client).await?;
                false
            };
            save_json(&task, &staged)?;
            print_json(&serde_json::json!({
                "taskFile": task,
                "operationId": staged.operation_id,
                "patchStatus": staged.patch.status,
                "currentVerified": current_verified,
                "receipt": staged.receipt,
                "planWarnings": staged.plan.warnings,
                "bridgeWarnings": staged.bridge_preview.warnings,
            }))?;
        }
        Ymm4Command::SceneStage {
            connection,
            profile,
            profile_id,
            alpha,
            max_actual_frame_delta,
            frame,
            task,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let result = scene::stage(
                &client,
                scene::StageSceneOptions {
                    profile_path: &profile,
                    profile_id,
                    alpha,
                    max_actual_frame_delta,
                    frames: frame,
                    task_path: &task,
                    head: RevisionId(head),
                },
            )
            .await?;
            print_json(&result)?;
        }
        Ymm4Command::SceneApprove {
            connection,
            task,
            digest,
            current_profile,
            head,
        } => {
            let client = ymm4_client(connection)?;
            print_json(
                &scene::approve(
                    &client,
                    &task,
                    &digest,
                    current_profile.as_deref(),
                    RevisionId(head),
                )
                .await?,
            )?;
        }
        Ymm4Command::SceneCapture {
            connection,
            task,
            current_profile,
            head,
        } => {
            let client = ymm4_client(connection)?;
            print_json(
                &scene::capture(&client, &task, current_profile.as_deref(), RevisionId(head))
                    .await?,
            )?;
        }
        Ymm4Command::SceneReplay {
            connection,
            task,
            current_profile,
            head,
        } => {
            let client = ymm4_client(connection)?;
            print_json(
                &scene::replay(&client, &task, current_profile.as_deref(), RevisionId(head))
                    .await?,
            )?;
        }
        Ymm4Command::SceneReview {
            connection,
            task,
            reviewer,
            current_profile,
            head,
        } => {
            let client = ymm4_client(connection)?;
            print_json(
                &scene::review(
                    &client,
                    &task,
                    reviewer,
                    current_profile.as_deref(),
                    RevisionId(head),
                )
                .await?,
            )?;
        }
        Ymm4Command::SceneDecide {
            connection,
            task,
            decision,
            note,
            current_profile,
            head,
        } => {
            if note.trim().is_empty() {
                return Err("scene review decision note must be non-empty".into());
            }
            let client = ymm4_client(connection)?;
            let decision = match decision.as_str() {
                "accept" => SceneReviewDecision::Accept,
                "reject" => SceneReviewDecision::Reject,
                _ => unreachable!("clap validates scene review decisions"),
            };
            print_json(
                &scene::decide(
                    &client,
                    &task,
                    decision,
                    note,
                    current_profile.as_deref(),
                    RevisionId(head),
                )
                .await?,
            )?;
        }
        Ymm4Command::SceneStatus {
            connection,
            task,
            current_profile,
            head,
        } => {
            let client = ymm4_client(connection)?;
            print_json(
                &scene::status(&client, &task, current_profile.as_deref(), RevisionId(head))
                    .await?,
            )?;
        }
        Ymm4Command::CheckpointStage {
            connection,
            state,
            operations,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let snapshot = client.snapshot().await?;
            let canonical = open_canonical(&state, &snapshot.project_id, RevisionId(head))?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let record = operation_store
                .stage_checkpoint(&canonical, &client, RevisionId(head))
                .await?;
            print_json(&record)?;
        }
        Ymm4Command::CheckpointExecute {
            connection,
            state,
            operations,
            operation_id,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let staged = operation_store.checkpoint_status(operation_id)?;
            let canonical =
                open_canonical(&state, &staged.payload.request.project_id, RevisionId(head))?;
            let record = operation_store
                .execute_checkpoint(&canonical, &client, operation_id)
                .await?;
            print_json(&record)?;
        }
        Ymm4Command::CheckpointStatus {
            operations,
            operation_id,
        } => {
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(&operation_store.checkpoint_status(operation_id)?)?;
        }
        Ymm4Command::RenderProfiles { connection } => {
            let client = ymm4_client(connection)?;
            print_json(&client.render_profiles().await?)?;
        }
        Ymm4Command::RenderStage {
            connection,
            state,
            operations,
            checkpoint_operation_id,
            profile,
            output,
            overwrite,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let snapshot = client.snapshot().await?;
            let canonical = open_canonical(&state, &snapshot.project_id, RevisionId(head))?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let overwrite_policy = if overwrite {
                RenderOverwritePolicy::ReplaceExisting
            } else {
                RenderOverwritePolicy::Deny
            };
            let record = operation_store
                .stage_render(
                    &canonical,
                    &client,
                    RevisionId(head),
                    checkpoint_operation_id,
                    &profile,
                    output,
                    overwrite_policy,
                )
                .await?;
            print_json(&record)?;
        }
        Ymm4Command::RenderExecute {
            connection,
            state,
            operations,
            task_id,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let staged = operation_store.render_status(task_id)?;
            let canonical =
                open_canonical(&state, &staged.payload.request.project_id, RevisionId(head))?;
            let record = operation_store
                .execute_render(&canonical, &client, task_id)
                .await?;
            print_json(&record)?;
        }
        Ymm4Command::RenderStatus {
            operations,
            task_id,
        } => {
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(&operation_store.render_status(task_id)?)?;
        }
        Ymm4Command::RenderCancel {
            connection,
            operations,
            task_id,
        } => {
            let client = ymm4_client(connection)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(&operation_store.cancel_render(&client, task_id).await?)?;
        }
        Ymm4Command::ReconcileReport {
            connection,
            state,
            operations,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let snapshot = client.snapshot().await?;
            let canonical = open_canonical(&state, &snapshot.project_id, RevisionId(head))?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let record = operation_store
                .stage_reconciliation_from_durable(&canonical, &client, RevisionId(head))
                .await?;
            print_json(&record)?;
        }
        Ymm4Command::ReconcilePreview {
            operations,
            report_digest,
            decisions,
        } => {
            let decisions: Vec<ReconciliationDecision> =
                serde_json::from_slice(&fs::read(decisions)?)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(&operation_store.preview_reconciliation(&report_digest, decisions)?)?;
        }
        Ymm4Command::ReconcileApply {
            connection,
            state,
            operations,
            report_digest,
            digest,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let staged = operation_store.reconciliation_status(&report_digest)?;
            let canonical = open_canonical(
                &state,
                &staged.payload.report.source.project_id,
                RevisionId(head),
            )?;
            let record = operation_store
                .accept_reconciliation(&canonical, &client, &report_digest, &digest)
                .await?;
            print_json(&record)?;
        }
        Ymm4Command::ReconcileChildStatus {
            operations,
            child_task_id,
        } => {
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            print_json(&operation_store.reconciliation_child_status(&child_task_id)?)?;
        }
        Ymm4Command::ReconcileDetachApprove {
            state,
            operations,
            child_task_id,
            digest,
            head,
        } => {
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let child = operation_store.reconciliation_child_status(&child_task_id)?;
            let project_id = reconciliation_child_project_id(&child.payload);
            let canonical = open_canonical(&state, project_id, RevisionId(head))?;
            print_json(&operation_store.approve_reconciliation_detach(
                &canonical,
                &child_task_id,
                &digest,
            )?)?;
        }
        Ymm4Command::ReconcileDetachExecute {
            connection,
            state,
            operations,
            child_task_id,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let child = operation_store.reconciliation_child_status(&child_task_id)?;
            let project_id = reconciliation_child_project_id(&child.payload);
            let canonical = open_canonical(&state, project_id, RevisionId(head))?;
            let executed = operation_store
                .execute_reconciliation_detach(&canonical, &client, &child_task_id)
                .await?;
            let ReconciliationChildTask::MetadataDetach(detach) = &executed.payload else {
                return Err("reconciliation detach execution returned another child kind".into());
            };
            let committed_revision = detach
                .committed_revision
                .ok_or("verified reconciliation detach omitted its committed revision")?;
            print_json(&serde_json::json!({
                "canonicalReplay": canonical_replay_from_revisions(
                    RevisionId(head),
                    committed_revision,
                ),
                "record": executed,
            }))?;
        }
        Ymm4Command::ReconcileReExportDispatch {
            connection,
            state,
            operations,
            child_task_id,
            manifest,
            output_task,
            head,
        } => {
            let client = ymm4_client(connection)?;
            let operation_store = ProjectOperationStore::new(operations.operation_root);
            let child = operation_store.reconciliation_child_status(&child_task_id)?;
            let project_id = reconciliation_child_project_id(&child.payload);
            let canonical = open_canonical(&state, project_id, RevisionId(head))?;
            let manifest: takegraph_service::ReconciliationReExportManifest =
                serde_json::from_slice(&fs::read(manifest)?)?;
            let dispatched = operation_store
                .dispatch_reconciliation_re_export(&canonical, &client, &child_task_id, manifest)
                .await?;
            save_reconciliation_downstream_task(&output_task, &dispatched.payload)?;
            print_json(&dispatched)?;
        }
    }
    Ok(())
}

fn reconciliation_child_project_id(child: &takegraph_service::ReconciliationChildTask) -> &str {
    match child {
        takegraph_service::ReconciliationChildTask::ImportPatch(task) => &task.source.project_id,
        takegraph_service::ReconciliationChildTask::MetadataDetach(task) => &task.source.project_id,
        takegraph_service::ReconciliationChildTask::CanonicalReExport(task) => {
            &task.source.project_id
        }
    }
}

fn canonical_replay_from_revisions(
    starting_head: RevisionId,
    committed_revision: RevisionId,
) -> bool {
    committed_revision <= starting_head
}

fn save_reconciliation_downstream_task(
    output: &PathBuf,
    child: &ReconciliationChildTask,
) -> Result<(), Box<dyn std::error::Error>> {
    let ReconciliationChildTask::CanonicalReExport(task) = child else {
        return Err("reconciliation dispatch did not return a canonical re-export child".into());
    };
    let preview = task
        .downstream_preview
        .as_ref()
        .ok_or("reconciliation dispatch did not return a downstream preview")?;
    match preview {
        ReconciliationDownstreamPreview::PortablePair(patch) => save_json(output, patch),
        ReconciliationDownstreamPreview::NativeVoiceMutation(patch) => save_json(output, patch),
        ReconciliationDownstreamPreview::NativeExtension(task) => save_json(output, task),
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn require_canonical_head(
    store: &DurableProjectStore,
    requested: RevisionId,
) -> Result<RevisionId, Box<dyn std::error::Error>> {
    let canonical = store.head()?;
    if canonical != requested {
        return Err(std::io::Error::other(format!(
            "requested head {requested:?} does not match durable canonical head {canonical:?}"
        ))
        .into());
    }
    Ok(canonical)
}

/// Keeps first-time approval bound to the current canonical head while letting
/// a persisted committed task re-authorize only its original exact digest.
/// Durable replay still receives and checks the caller's current canonical
/// head before consulting the operation-bound commit record and bridge receipt.
fn timeline_edit_authorization_head(patch: &Patch, canonical_head: RevisionId) -> RevisionId {
    if patch.status == PatchStatus::Committed {
        patch.base
    } else {
        canonical_head
    }
}

fn open_canonical(
    state: &ProjectStateOptions,
    project_id: &str,
    requested: RevisionId,
) -> Result<DurableProjectStore, Box<dyn std::error::Error>> {
    let store = DurableProjectStore::open_scoped(&state.state_root, project_id)?;
    require_canonical_head(&store, requested)?;
    Ok(store)
}

fn ymm4_client(connection: Ymm4Connection) -> Result<Ymm4BridgeClient, Box<dyn std::error::Error>> {
    let token = load_ymm4_token(connection.token, connection.credentials)?;
    let client = Ymm4BridgeClient::new(&connection.endpoint, token)?;
    Ok(match connection.expected_project_id {
        Some(project_id) => client.with_expected_project_id(project_id)?,
        None => client,
    })
}

fn save_json<T: serde::Serialize>(
    path: &PathBuf,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("task path has no file name: {}", path.display()))?;
    let temporary_name = format!(".{}.{}.tmp", file_name.to_string_lossy(), Uuid::new_v4());
    let temporary_path = path.with_file_name(temporary_name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error.into());
    }
    Ok(())
}

fn load_ymm4_token(
    configured_token: Option<String>,
    configured_credentials: Option<PathBuf>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(token) = configured_token.filter(|value| !value.trim().is_empty()) {
        return Ok(token);
    }
    let credentials_path = configured_credentials.unwrap_or_else(default_ymm4_credentials_path);
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&credentials_path).map_err(|error| {
            format!(
                "unable to read YMM4 bridge credentials at {}: {error}",
                credentials_path.display()
            )
        })?)?;
    value
        .get("token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "YMM4 bridge credential file has no token".into())
}

fn default_ymm4_credentials_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map_or_else(
        || PathBuf::from("ymm4-bridge.json"),
        |root| {
            PathBuf::from(root)
                .join("TakeGraph")
                .join("ymm4-bridge.json")
        },
    )
}

fn ymm4_native_extension_artifact_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or("LOCALAPPDATA is required for the YMM4 native-extension artifact boundary")?;
    Ok(PathBuf::from(local_app_data)
        .join("TakeGraph")
        .join("native-extension-artifacts"))
}

fn ymm4_native_voice_bridge_artifact_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or("LOCALAPPDATA is required for the YMM4 native-voice artifact boundary")?;
    Ok(PathBuf::from(local_app_data)
        .join("TakeGraph")
        .join("native-voice-artifacts"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_frame_composition_command_with_project_binding() {
        let cli = Cli::try_parse_from([
            "takegraph",
            "ymm4",
            "composition",
            "--endpoint",
            "http://127.0.0.1:8766",
            "--token",
            "secret",
            "--expected-project-id",
            "project-1",
        ])
        .unwrap();

        let Command::Ymm4 {
            command: Ymm4Command::Composition { connection },
        } = cli.command
        else {
            panic!("expected the YMM4 composition command");
        };
        assert_eq!(connection.expected_project_id.as_deref(), Some("project-1"));
    }

    #[test]
    fn parses_project_initialization_stage_without_adding_a_new_tool_family() {
        let cli = Cli::try_parse_from([
            "takegraph",
            "ymm4",
            "project-initialization-stage",
            "--mode",
            "save_untitled",
            "--destination",
            r"C:\projects\movie.ymmp",
            "--state-root",
            r"C:\state",
            "--operation-root",
            r"C:\operations",
        ])
        .unwrap();

        let Command::Ymm4 {
            command:
                Ymm4Command::ProjectInitializationStage {
                    mode, destination, ..
                },
        } = cli.command
        else {
            panic!("expected project initialization stage");
        };
        assert_eq!(mode, "save_untitled");
        assert_eq!(destination, Some(PathBuf::from(r"C:\projects\movie.ymmp")));
    }

    #[test]
    fn parses_timeline_edit_stage_contract() {
        let cli = Cli::try_parse_from([
            "takegraph",
            "ymm4",
            "timeline-edit-stage",
            "--manifest",
            "manifest.json",
            "--task",
            "task.json",
            "--head",
            "7",
        ])
        .unwrap();

        let Command::Ymm4 {
            command:
                Ymm4Command::TimelineEditStage {
                    manifest,
                    task,
                    head,
                    ..
                },
        } = cli.command
        else {
            panic!("expected timeline-edit-stage");
        };
        assert_eq!(manifest, PathBuf::from("manifest.json"));
        assert_eq!(task, PathBuf::from("task.json"));
        assert_eq!(head, 7);
    }

    #[test]
    fn parses_pure_timeline_edit_status_without_bridge_options() {
        let cli = Cli::try_parse_from([
            "takegraph",
            "ymm4",
            "timeline-edit-status",
            "--task",
            "task.json",
        ])
        .unwrap();
        let Command::Ymm4 {
            command: Ymm4Command::TimelineEditStatus { task },
        } = cli.command
        else {
            panic!("expected pure timeline-edit-status");
        };
        assert_eq!(task, PathBuf::from("task.json"));
    }

    #[test]
    fn timeline_edit_retry_uses_original_base_only_after_commit() {
        let base = RevisionId(7);
        let canonical_head = RevisionId(8);
        let mut patch = Patch::draft(base, "digest");
        patch.validate().unwrap();
        patch.materialize_preview().unwrap();

        assert_eq!(
            timeline_edit_authorization_head(&patch, canonical_head),
            canonical_head,
            "a previewable task must still fail approval against a stale base"
        );

        patch.approve().unwrap();
        assert_eq!(
            timeline_edit_authorization_head(&patch, canonical_head),
            canonical_head,
            "an approved but uncommitted task must still fail against a stale base"
        );

        assert_eq!(patch.commit(base).unwrap(), canonical_head);
        assert_eq!(
            timeline_edit_authorization_head(&patch, canonical_head),
            base,
            "only a persisted committed task may re-authorize its exact original approval"
        );
    }

    #[test]
    fn reconciliation_detach_replay_is_derived_from_committed_revision() {
        assert!(!canonical_replay_from_revisions(
            RevisionId(4),
            RevisionId(5)
        ));
        assert!(canonical_replay_from_revisions(
            RevisionId(5),
            RevisionId(5)
        ));
        assert!(canonical_replay_from_revisions(
            RevisionId(8),
            RevisionId(5)
        ));
    }

    #[test]
    fn ordinary_canonical_open_never_bootstraps_an_uninitialized_project() {
        let root =
            std::env::temp_dir().join(format!("takegraph-cli-uninitialized-{}", Uuid::new_v4()));
        let state = ProjectStateOptions {
            state_root: root.clone(),
        };

        let error = open_canonical(&state, "project-uninitialized", RevisionId(0))
            .expect_err("ordinary workflow must require explicit project initialization");
        assert!(error.to_string().contains("not initialized"));
        assert_eq!(
            DurableProjectStore::observe_scoped(&root, "project-uninitialized").unwrap(),
            None
        );
        assert!(!root.exists());
    }

    #[test]
    fn save_json_atomically_replaces_an_existing_task_file() {
        let root = std::env::temp_dir().join(format!("takegraph-cli-save-{}", Uuid::new_v4()));
        let task = root.join("task.json");

        save_json(&task, &serde_json::json!({ "generation": 1 })).unwrap();
        save_json(&task, &serde_json::json!({ "generation": 2 })).unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&task).unwrap()).unwrap();
        assert_eq!(saved["generation"], 2);
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
        fs::remove_dir_all(root).unwrap();
    }
}
