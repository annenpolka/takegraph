use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use takegraph_core::RevisionId;
use takegraph_node::{
    SceneVisualCheckProfile, YMM4_SCENE_CAPTURE_DRIVER_PROFILE_DIGEST, Ymm4BridgeClient,
    Ymm4ProjectSnapshot,
};
use takegraph_service::{
    SceneCaptureProfile, SceneInspectionPlan, SceneInspectionSource, SceneInspectionStatus,
    SceneInspectionTask, SceneReviewDecision,
};

type SceneResult<T> = Result<T, Box<dyn std::error::Error>>;

pub struct StageSceneOptions<'a> {
    pub profile_path: &'a Path,
    pub profile_id: String,
    pub alpha: bool,
    pub max_actual_frame_delta: u32,
    pub frames: Vec<u32>,
    pub task_path: &'a Path,
    pub head: RevisionId,
}

pub async fn stage(
    client: &Ymm4BridgeClient,
    options: StageSceneOptions<'_>,
) -> SceneResult<serde_json::Value> {
    let snapshot = client.snapshot().await?;
    let visual_checks: SceneVisualCheckProfile =
        serde_json::from_slice(&fs::read(options.profile_path)?)?;
    let source = source_from_snapshot(&snapshot, options.head);
    let capture_profile = SceneCaptureProfile {
        profile_id: options.profile_id,
        driver_profile_digest: YMM4_SCENE_CAPTURE_DRIVER_PROFILE_DIGEST.into(),
        alpha: options.alpha,
        max_actual_frame_delta: options.max_actual_frame_delta,
        visual_checks,
    };
    let plan = SceneInspectionPlan::stage(source, capture_profile, options.frames)?;
    let task = SceneInspectionTask::new(plan);
    save_task(options.task_path, &task)?;
    let profile_digest = task.plan.capture_profile_digest.clone();
    output(
        &task,
        options.task_path,
        &snapshot,
        options.head,
        &profile_digest,
        false,
    )
}

pub async fn approve(
    client: &Ymm4BridgeClient,
    task_path: &Path,
    approved_digest: &str,
    current_profile_path: Option<&Path>,
    head: RevisionId,
) -> SceneResult<serde_json::Value> {
    let (mut task, snapshot, source) = load_current(client, task_path, head).await?;
    let profile_digest = current_profile_digest(&task, current_profile_path)?;
    require_not_stale(&mut task, task_path, &source, &profile_digest)?;
    task.approve(approved_digest, &source)?;
    save_task(task_path, &task)?;
    output(&task, task_path, &snapshot, head, &profile_digest, false)
}

pub async fn capture(
    client: &Ymm4BridgeClient,
    task_path: &Path,
    current_profile_path: Option<&Path>,
    head: RevisionId,
) -> SceneResult<serde_json::Value> {
    let (mut task, snapshot, source) = load_current(client, task_path, head).await?;
    let profile_digest = current_profile_digest(&task, current_profile_path)?;
    require_not_stale(&mut task, task_path, &source, &profile_digest)?;
    capture_and_persist(&mut task, client, task_path).await?;
    output(&task, task_path, &snapshot, head, &profile_digest, true)
}

pub async fn replay(
    client: &Ymm4BridgeClient,
    task_path: &Path,
    current_profile_path: Option<&Path>,
    head: RevisionId,
) -> SceneResult<serde_json::Value> {
    let (mut task, snapshot, source) = load_current(client, task_path, head).await?;
    let profile_digest = current_profile_digest(&task, current_profile_path)?;
    require_not_stale(&mut task, task_path, &source, &profile_digest)?;
    capture_and_persist(&mut task, client, task_path).await?;
    let mut value = output(&task, task_path, &snapshot, head, &profile_digest, true)?;
    value["replayed"] = serde_json::Value::Bool(true);
    Ok(value)
}

pub async fn review(
    client: &Ymm4BridgeClient,
    task_path: &Path,
    reviewer: String,
    current_profile_path: Option<&Path>,
    head: RevisionId,
) -> SceneResult<serde_json::Value> {
    let (mut task, snapshot, source) = load_current(client, task_path, head).await?;
    let profile_digest = current_profile_digest(&task, current_profile_path)?;
    require_not_stale(&mut task, task_path, &source, &profile_digest)?;
    // Persisted bridge receipts are deliberately distrusted. Replaying the same
    // bound operation authenticates the receipt and re-reads every PNG.
    capture_and_persist(&mut task, client, task_path).await?;
    task.begin_review(reviewer)?;
    save_task(task_path, &task)?;
    output(&task, task_path, &snapshot, head, &profile_digest, true)
}

pub async fn decide(
    client: &Ymm4BridgeClient,
    task_path: &Path,
    decision: SceneReviewDecision,
    note: String,
    current_profile_path: Option<&Path>,
    head: RevisionId,
) -> SceneResult<serde_json::Value> {
    let (mut task, snapshot, source) = load_current(client, task_path, head).await?;
    let profile_digest = current_profile_digest(&task, current_profile_path)?;
    require_not_stale(&mut task, task_path, &source, &profile_digest)?;
    // A decision is never accepted solely from persisted state. This replay
    // restores authenticated receipt trust immediately before the human action.
    capture_and_persist(&mut task, client, task_path).await?;
    task.decide(decision, note)?;
    save_task(task_path, &task)?;
    output(&task, task_path, &snapshot, head, &profile_digest, true)
}

pub async fn status(
    client: &Ymm4BridgeClient,
    task_path: &Path,
    current_profile_path: Option<&Path>,
    head: RevisionId,
) -> SceneResult<serde_json::Value> {
    let (mut task, snapshot, source) = load_current(client, task_path, head).await?;
    let profile_digest = current_profile_digest(&task, current_profile_path)?;
    if task.invalidate_if_changed(&source, &profile_digest) {
        save_task(task_path, &task)?;
    }
    output(&task, task_path, &snapshot, head, &profile_digest, false)
}

async fn load_current(
    client: &Ymm4BridgeClient,
    task_path: &Path,
    head: RevisionId,
) -> SceneResult<(
    SceneInspectionTask,
    Ymm4ProjectSnapshot,
    SceneInspectionSource,
)> {
    let task = SceneInspectionTask::from_json_slice(&fs::read(task_path)?)?;
    let snapshot = client.snapshot().await?;
    let source = source_from_snapshot(&snapshot, head);
    Ok((task, snapshot, source))
}

fn source_from_snapshot(snapshot: &Ymm4ProjectSnapshot, head: RevisionId) -> SceneInspectionSource {
    // Protocol v2 snapshots expose one aggregate fingerprint. Bind all three
    // semantic scopes to it until the bridge exposes scoped values; this is
    // conservative because any managed or conflict-context change goes stale.
    SceneInspectionSource {
        project_id: snapshot.project_id.clone(),
        scene_id: snapshot.scene_id.clone(),
        source_revision: head,
        expected_fingerprint: snapshot.fingerprint.clone(),
        managed_fingerprint: snapshot.fingerprint.clone(),
        conflict_fingerprint: snapshot.fingerprint.clone(),
    }
}

fn require_not_stale(
    task: &mut SceneInspectionTask,
    task_path: &Path,
    current_source: &SceneInspectionSource,
    current_profile_digest: &str,
) -> SceneResult<()> {
    if task.invalidate_if_changed(current_source, current_profile_digest) {
        save_task(task_path, task)?;
        return Err(std::io::Error::other(
            "scene inspection became stale; inspect scene-status and stage a new plan",
        )
        .into());
    }
    Ok(())
}

fn current_profile_digest(
    task: &SceneInspectionTask,
    current_profile_path: Option<&Path>,
) -> SceneResult<String> {
    let Some(path) = current_profile_path else {
        return Ok(task.plan.capture_profile.digest()?);
    };
    let mut profile = task.plan.capture_profile.clone();
    profile.visual_checks = serde_json::from_slice(&fs::read(path)?)?;
    Ok(profile.digest()?)
}

async fn capture_and_persist(
    task: &mut SceneInspectionTask,
    client: &Ymm4BridgeClient,
    task_path: &Path,
) -> SceneResult<()> {
    let staging_root = authorized_staging_root()?;
    let artifact_root = artifact_root(task_path);
    task.capture_and_ingest(client, &staging_root, &artifact_root)
        .await?;
    save_task(task_path, task)?;
    Ok(())
}

fn authorized_staging_root() -> SceneResult<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| std::io::Error::other("LOCALAPPDATA is required for YMM4 scene capture"))?;
    Ok(PathBuf::from(local_app_data)
        .join("TakeGraph")
        .join("scene-captures"))
}

fn artifact_root(task_path: &Path) -> PathBuf {
    task_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("artifacts")
}

fn output(
    task: &SceneInspectionTask,
    task_path: &Path,
    snapshot: &Ymm4ProjectSnapshot,
    head: RevisionId,
    current_profile_digest: &str,
    authenticated_replay_performed: bool,
) -> SceneResult<serde_json::Value> {
    let current_source = source_from_snapshot(snapshot, head);
    let source = &task.plan.source;
    let semantic_changed =
        source != &current_source || task.plan.capture_profile_digest != current_profile_digest;
    let trusted_acceptance =
        task.is_trusted_acceptance_for(&current_source, current_profile_digest)?;
    let images = task
        .receipt
        .captures
        .iter()
        .map(|capture| {
            serde_json::json!({
                "sampleId": capture.sample_id,
                "requestedFrame": capture.requested_frame,
                "actualFrame": capture.actual_frame,
                "artifactPath": capture.artifact_path,
                "sha256": capture.sha256,
                "width": capture.width,
                "height": capture.height,
                "findings": capture.inspection.findings,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "taskFile": task_path,
        "digest": task.plan.digest,
        "receipt": task.receipt,
        "images": images,
        "automatedFindings": task.receipt.findings,
        "automatedFindingsAreAdvisory": true,
        "humanReviewRequired": !matches!(
            task.receipt.status,
            SceneInspectionStatus::Accepted | SceneInspectionStatus::Rejected
        ),
        "authenticatedReplayPerformed": authenticated_replay_performed,
        "trustedAcceptance": trusted_acceptance,
        "semanticDiff": {
            "changed": semantic_changed,
            "projectId": { "expected": source.project_id, "actual": snapshot.project_id, "changed": source.project_id != snapshot.project_id },
            "sceneId": { "expected": source.scene_id, "actual": snapshot.scene_id, "changed": source.scene_id != snapshot.scene_id },
            "sourceRevision": { "expected": source.source_revision.0, "actual": head.0, "changed": source.source_revision != head },
            "aggregateFingerprint": { "expected": source.expected_fingerprint, "actual": snapshot.fingerprint, "changed": source.expected_fingerprint != snapshot.fingerprint },
            "managedFingerprint": { "expected": source.managed_fingerprint, "actual": snapshot.fingerprint, "changed": source.managed_fingerprint != snapshot.fingerprint },
            "conflictFingerprint": { "expected": source.conflict_fingerprint, "actual": snapshot.fingerprint, "changed": source.conflict_fingerprint != snapshot.fingerprint },
            "captureProfileDigest": { "expected": task.plan.capture_profile_digest, "actual": current_profile_digest, "changed": task.plan.capture_profile_digest != current_profile_digest },
        },
    }))
}

fn save_task(path: &Path, task: &SceneInspectionTask) -> SceneResult<()> {
    if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(task)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}
