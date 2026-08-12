use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use takegraph_core::{
    ChangeBudget, DescriptorReference, NativeExtensionIntent, PatchStatus, PortraitIntent,
    PortraitPresentation, ReconciliationChoice, ReconciliationDecision, ReplacementGuard,
    ResolvedPlacement, RevisionId, canonical_sha256,
};
use takegraph_node::{
    Ymm4BridgeClient, Ymm4DescriptorCatalog, Ymm4MetadataDetachStatus, Ymm4TargetDescriptor,
};
use takegraph_service::{
    DurableProjectStore, NativeExtensionArtifactSource, NativeExtensionStageManifest,
    ProjectOperationError, ProjectOperationStore, ReconciliationChildTask,
    ReconciliationDetachStatus, ReconciliationDownstreamPreview, ReconciliationReExportManifest,
    ReconciliationReExportStatus, Ymm4NativeExtensionTask,
};
use uuid::Uuid;

const PROJECT_ID: &str = "project-native-reconcile";
const SCENE_ID: &str = "scene-a";
const BEFORE: &str = "fingerprint-before";
const AFTER: &str = "fingerprint-after";
const DRIFTED: &str = "fingerprint-drifted";
const DETACHED: &str = "fingerprint-detached";
const DESCRIPTOR_ID: &str = "character.marisa";

#[tokio::test]
// The integration fixture intentionally shows the full finalize-to-reconcile contract.
#[allow(clippy::too_many_lines)]
async fn verified_native_extension_finalize_is_immediately_reconcilable() {
    let catalog = test_catalog();
    let descriptor_digest = catalog.planning_descriptor_digests().unwrap()[DESCRIPTOR_ID].clone();
    let state = Arc::new(Mutex::new(MockState::default()));
    let server = MockBridge::start(catalog.clone(), Arc::clone(&state));
    let client = Ymm4BridgeClient::new(&server.endpoint, "test-token").unwrap();
    let root = test_root();
    let artifact_root = root.join("artifacts");
    let canonical = DurableProjectStore::open(root.join("project"), PROJECT_ID).unwrap();

    let mut task = Ymm4NativeExtensionTask::stage(
        &client,
        RevisionId(0),
        NativeExtensionStageManifest {
            intents: vec![NativeExtensionIntent::UpsertPortrait(PortraitIntent {
                entity_id: "portrait-01".into(),
                entity_revision: 3,
                presentation: PortraitPresentation::Portrait,
                character_binding: DescriptorReference {
                    descriptor_id: DESCRIPTOR_ID.into(),
                    expected_digest: descriptor_digest.clone(),
                },
                placement: ResolvedPlacement {
                    frame: 120,
                    primary_layer: 30,
                    secondary_layer: None,
                },
                duration_frames: 90,
                replacement_guard: ReplacementGuard::default(),
            })],
            artifact_sources: Vec::<NativeExtensionArtifactSource>::new(),
            change_budget: ChangeBudget::create_only(1),
        },
        artifact_root,
    )
    .await
    .unwrap();
    let approved = task.patch.digest.clone();
    task.approve(&approved, RevisionId(0)).unwrap();
    assert_eq!(
        task.apply_and_finalize_durable(&client, &canonical, RevisionId(0))
            .await
            .unwrap()
            .revision,
        RevisionId(1)
    );

    let durable = canonical.snapshot().unwrap();
    let projection = durable.managed_target_states.values().next().unwrap();
    assert_eq!(projection.items.len(), 1);
    assert_eq!(projection.items[0].realization_kind, "ymm4_native_portrait");

    let operations = ProjectOperationStore::new(root.join("operations"));
    let report = operations
        .stage_reconciliation_from_durable(&canonical, &client, RevisionId(1))
        .await
        .unwrap();
    assert!(report.payload.report.entries.is_empty());
    assert_eq!(
        report.payload.report.expected_managed_state_digest,
        report.payload.report.actual_managed_state_digest
    );

    // A live owned-field edit creates a detach choice for the same canonical
    // native realization. Reconciliation acceptance itself must remain inert.
    {
        let mut state = state.lock().unwrap();
        state.drifted = true;
        state.realization.as_mut().unwrap()["ownedFields"]["descriptorId"] =
            serde_json::json!("character.user-edited");
    }
    let drift = operations
        .stage_reconciliation_from_durable(&canonical, &client, RevisionId(1))
        .await
        .unwrap();
    assert_eq!(drift.payload.report.entries.len(), 1);
    let entry = &drift.payload.report.entries[0];
    let preview = operations
        .preview_reconciliation(
            &drift.payload.report.report_digest,
            vec![ReconciliationDecision {
                entry_id: entry.entry_id.clone(),
                choice: ReconciliationChoice::DetachFromTakeGraph,
            }],
        )
        .unwrap();
    let approval_digest = preview
        .payload
        .preview
        .as_ref()
        .unwrap()
        .approval_digest
        .clone();
    let accepted = operations
        .accept_reconciliation(
            &canonical,
            &client,
            &drift.payload.report.report_digest,
            &approval_digest,
        )
        .await
        .unwrap();
    assert_eq!(canonical.head().unwrap(), RevisionId(1));
    let child_task_id = &accepted.payload.materialized_children[0].child_task_id;
    let child = operations
        .reconciliation_child_status(child_task_id)
        .unwrap();
    let ReconciliationChildTask::MetadataDetach(detach) = &child.payload else {
        panic!("expected metadata detach child");
    };
    assert_eq!(detach.status, ReconciliationDetachStatus::PreviewReady);
    assert!(detach.patch.approved_digest.is_none());
    let detach_digest = detach.patch.digest.clone();

    let approved = operations
        .approve_reconciliation_detach(&canonical, child_task_id, &detach_digest)
        .unwrap();
    let ReconciliationChildTask::MetadataDetach(detach) = &approved.payload else {
        panic!("expected metadata detach child");
    };
    assert_eq!(detach.status, ReconciliationDetachStatus::Approved);

    // A contradictory envelope must never turn a durable child Verified,
    // even when the embedded receipt falsely claims successful verification.
    state
        .lock()
        .unwrap()
        .contradictory_detach_responses_remaining = 1;
    assert!(matches!(
        operations
            .execute_reconciliation_detach(&canonical, &client, child_task_id)
            .await,
        Err(ProjectOperationError::MetadataDetachResponseInconsistent)
    ));
    let rejected = operations
        .reconciliation_child_status(child_task_id)
        .unwrap();
    let ReconciliationChildTask::MetadataDetach(detach) = &rejected.payload else {
        panic!("expected metadata detach child");
    };
    assert_eq!(detach.status, ReconciliationDetachStatus::Applying);
    assert!(detach.receipt.is_none());
    assert_eq!(
        canonical
            .snapshot()
            .unwrap()
            .pending_external_commit
            .as_ref()
            .map(|reservation| reservation.operation_id),
        Some(detach.operation_id)
    );

    // Simulate a process restart after YMM4 committed but the response was
    // rejected/lost. The persisted request plus canonical reservation must be
    // sufficient for authenticated bridge replay and same-revision finalize.
    drop(operations);
    drop(canonical);
    let canonical = DurableProjectStore::open(root.join("project"), PROJECT_ID).unwrap();
    let operations = ProjectOperationStore::new(root.join("operations"));

    // The bridge already sealed a Verified receipt, but its plugin/capability
    // contract changed before the service could finalize canonical ownership.
    // Exact pending recovery must authenticate status first and must not wedge
    // behind today's capability digest.
    state.lock().unwrap().capability_drifted = true;

    let executed = operations
        .execute_reconciliation_detach(&canonical, &client, child_task_id)
        .await
        .unwrap();
    state.lock().unwrap().capability_drifted = false;
    let ReconciliationChildTask::MetadataDetach(detach) = &executed.payload else {
        panic!("expected metadata detach child");
    };
    assert_eq!(detach.status, ReconciliationDetachStatus::Verified);
    assert_eq!(detach.committed_revision, Some(RevisionId(2)));
    assert!(
        detach
            .receipt
            .as_ref()
            .is_some_and(|receipt| receipt.verified)
    );
    assert_eq!(canonical.head().unwrap(), RevisionId(2));
    assert!(
        canonical
            .snapshot()
            .unwrap()
            .pending_external_commit
            .is_none()
    );

    // Simulate commit success followed by loss of only the child terminal
    // generation. Later canonical work must be allowed to advance before the
    // child is recovered from the immutable historical commit record.
    remove_latest_reconciliation_child_generation(&root.join("operations"));

    // Permanent detach removes canonical ownership in the same receipt-bound
    // CAS commit, so the next reconciliation cannot report the identity again.
    let after_detach = operations
        .stage_reconciliation_from_durable(&canonical, &client, RevisionId(2))
        .await
        .unwrap();
    assert!(after_detach.payload.report.entries.is_empty());

    // Seed one new canonical native realization through the ordinary exporter
    // to keep the independent re-export dispatch contract in this live-style
    // end-to-end test.
    {
        let mut state = state.lock().unwrap();
        state.detached = false;
        state.drifted = false;
    }
    let mut replacement = Ymm4NativeExtensionTask::stage(
        &client,
        RevisionId(2),
        portrait_manifest(
            descriptor_digest.clone(),
            root.join("replacement-artifacts"),
        ),
        root.join("replacement-artifacts"),
    )
    .await
    .unwrap();
    let replacement_digest = replacement.patch.digest.clone();
    replacement
        .approve(&replacement_digest, RevisionId(2))
        .unwrap();
    assert_eq!(
        replacement
            .apply_and_finalize_durable(&client, &canonical, RevisionId(2))
            .await
            .unwrap()
            .revision,
        RevisionId(3)
    );
    let replacement_realization = state.lock().unwrap().realization.clone().unwrap();

    // Recovery authenticates the exact bridge replay and historical commit
    // binding. It deliberately does not require the current fingerprint or
    // capability snapshot to still equal the old detach post-state.
    let replayed = operations
        .execute_reconciliation_detach(&canonical, &client, child_task_id)
        .await
        .unwrap();
    assert_eq!(replayed, executed);
    assert_eq!(canonical.head().unwrap(), RevisionId(3));
    {
        let state_after_detach = state.lock().unwrap();
        assert_eq!(state_after_detach.detach_apply_count, 1);
        assert_eq!(state_after_detach.detach_request_count, 2);
    }
    {
        let mut state = state.lock().unwrap();
        state.detached = true;
        state.realization = None;
    }

    // A missing canonical realization is dispatched through the existing
    // native-extension stage path. Dispatch persists a brand-new preview but
    // cannot reuse reconciliation approval or call the mutation route.
    let missing = operations
        .stage_reconciliation_from_durable(&canonical, &client, RevisionId(3))
        .await
        .unwrap();
    assert_eq!(missing.payload.report.entries.len(), 1);
    let entry = &missing.payload.report.entries[0];
    let preview = operations
        .preview_reconciliation(
            &missing.payload.report.report_digest,
            vec![ReconciliationDecision {
                entry_id: entry.entry_id.clone(),
                choice: ReconciliationChoice::ReExportCanonical,
            }],
        )
        .unwrap();
    let approval_digest = preview
        .payload
        .preview
        .as_ref()
        .unwrap()
        .approval_digest
        .clone();
    let accepted = operations
        .accept_reconciliation(
            &canonical,
            &client,
            &missing.payload.report.report_digest,
            &approval_digest,
        )
        .await
        .unwrap();
    let re_export_child_id = &accepted.payload.materialized_children[0].child_task_id;
    let manifest = ReconciliationReExportManifest::NativeExtension {
        manifest: portrait_manifest(descriptor_digest, root.join("re-export-artifacts")),
        artifact_root: root.join("re-export-artifacts"),
    };
    let dispatched = operations
        .dispatch_reconciliation_re_export(
            &canonical,
            &client,
            re_export_child_id,
            manifest.clone(),
        )
        .await
        .unwrap();
    let ReconciliationChildTask::CanonicalReExport(re_export) = &dispatched.payload else {
        panic!("expected canonical re-export child");
    };
    assert_eq!(re_export.status, ReconciliationReExportStatus::PreviewReady);
    assert!(re_export.dispatch_manifest_digest.is_some());
    let Some(ReconciliationDownstreamPreview::NativeExtension(downstream)) =
        re_export.downstream_preview.as_ref()
    else {
        panic!("expected native-extension downstream preview");
    };
    assert_eq!(downstream.patch.status, PatchStatus::Previewable);
    assert!(downstream.patch.approved_digest.is_none());
    assert_eq!(downstream.operation_id, re_export.downstream_task_id);

    // Exact replay returns the durable preview. Rebinding even a path-only
    // manifest field is rejected before another exporter staging call.
    assert_eq!(
        operations
            .dispatch_reconciliation_re_export(
                &canonical,
                &client,
                re_export_child_id,
                manifest.clone(),
            )
            .await
            .unwrap(),
        dispatched
    );
    let rebound = ReconciliationReExportManifest::NativeExtension {
        manifest: portrait_manifest(
            test_catalog().planning_descriptor_digests().unwrap()[DESCRIPTOR_ID].clone(),
            root.join("rebound-artifacts"),
        ),
        artifact_root: root.join("rebound-artifacts"),
    };
    assert!(matches!(
        operations
            .dispatch_reconciliation_re_export(&canonical, &client, re_export_child_id, rebound,)
            .await,
        Err(ProjectOperationError::ReExportManifestReplayMismatch)
    ));
    assert_eq!(state.lock().unwrap().native_apply_count, 2);

    // A bridge conflict before its WAL must not wedge the canonical project
    // reservation. The apply-gate-serialized not-started endpoint durably
    // tombstones the exact request; fresh preimage read-back then authorizes
    // releasing only this reservation.
    {
        let mut state = state.lock().unwrap();
        state.detached = false;
        state.drifted = true;
        state.detach_request = None;
        state.not_started_request = None;
        state.realization = Some(replacement_realization);
        state.realization.as_mut().unwrap()["ownedFields"]["descriptorId"] =
            serde_json::json!("character.pre-wal-conflict");
        state.reject_detach_before_wal_remaining = 1;
        state.reject_not_started_seal_remaining = 1;
    }
    let rejected_report = operations
        .stage_reconciliation_from_durable(&canonical, &client, RevisionId(3))
        .await
        .unwrap();
    let rejected_entry = &rejected_report.payload.report.entries[0];
    let rejected_preview = operations
        .preview_reconciliation(
            &rejected_report.payload.report.report_digest,
            vec![ReconciliationDecision {
                entry_id: rejected_entry.entry_id.clone(),
                choice: ReconciliationChoice::DetachFromTakeGraph,
            }],
        )
        .unwrap();
    let rejected_approval = rejected_preview
        .payload
        .preview
        .as_ref()
        .unwrap()
        .approval_digest
        .clone();
    let rejected_accept = operations
        .accept_reconciliation(
            &canonical,
            &client,
            &rejected_report.payload.report.report_digest,
            &rejected_approval,
        )
        .await
        .unwrap();
    let rejected_child_id = &rejected_accept.payload.materialized_children[0].child_task_id;
    let rejected_child = operations
        .reconciliation_child_status(rejected_child_id)
        .unwrap();
    let ReconciliationChildTask::MetadataDetach(rejected_draft) = &rejected_child.payload else {
        panic!("expected rejected metadata detach child");
    };
    let rejected_digest = rejected_draft.patch.digest.clone();
    operations
        .approve_reconciliation_detach(&canonical, rejected_child_id, &rejected_digest)
        .unwrap();
    // First recovery attempt loses the tombstone response as well, leaving the
    // exact reservation durable across restart/plugin drift.
    assert!(matches!(
        operations
            .execute_reconciliation_detach(&canonical, &client, rejected_child_id)
            .await,
        Err(ProjectOperationError::Bridge(_))
    ));
    let applying = operations
        .reconciliation_child_status(rejected_child_id)
        .unwrap();
    let ReconciliationChildTask::MetadataDetach(applying) = applying.payload else {
        panic!("expected applying metadata detach child");
    };
    let original_request = applying.request.expect("durable original detach request");
    assert!(
        canonical
            .snapshot()
            .unwrap()
            .pending_external_commit
            .is_some()
    );
    state.lock().unwrap().capability_drifted = true;
    let drift_recovery = operations
        .execute_reconciliation_detach(&canonical, &client, rejected_child_id)
        .await;
    assert!(
        matches!(
            drift_recovery,
            Err(ProjectOperationError::MetadataDetachNotVerified(
                ReconciliationDetachStatus::PreviewReady,
                _
            ))
        ),
        "unexpected drift recovery result: {drift_recovery:?}"
    );
    state.lock().unwrap().capability_drifted = false;
    let reissued = operations
        .reconciliation_child_status(rejected_child_id)
        .unwrap();
    let ReconciliationChildTask::MetadataDetach(reissued) = &reissued.payload else {
        panic!("expected reissued metadata detach child");
    };
    assert_eq!(reissued.status, ReconciliationDetachStatus::PreviewReady);
    assert_ne!(reissued.operation_id, rejected_draft.operation_id);
    assert_ne!(reissued.patch.id, rejected_draft.patch.id);
    assert_ne!(reissued.patch.digest, rejected_digest);
    assert!(reissued.patch.approved_digest.is_none());
    assert!(reissued.request.is_none());
    assert!(reissued.receipt.is_none());
    assert!(reissued.error.is_none());
    assert!(
        canonical
            .snapshot()
            .unwrap()
            .pending_external_commit
            .is_none()
    );
    assert_eq!(canonical.head().unwrap(), RevisionId(3));

    // A delayed copy of the original POST can only replay the durable
    // tombstone and cannot mutate after canonical released its reservation.
    let delayed = client
        .detach_managed_metadata(&original_request)
        .await
        .unwrap();
    assert!(delayed.replayed);
    assert_eq!(delayed.receipt.status, Ymm4MetadataDetachStatus::NotStarted);
    assert!(!state.lock().unwrap().detached);

    // The replacement attempt is inert until its new digest is explicitly
    // approved; the old approval cannot authorize it.
    assert!(matches!(
        operations
            .execute_reconciliation_detach(&canonical, &client, rejected_child_id)
            .await,
        Err(ProjectOperationError::Patch(_)
            | ProjectOperationError::InvalidDetachTransition(
                ReconciliationDetachStatus::PreviewReady
            ))
    ));
    operations
        .approve_reconciliation_detach(&canonical, rejected_child_id, &reissued.patch.digest)
        .unwrap();
    {
        let mut state = state.lock().unwrap();
        state.detach_request = None;
        state.not_started_request = None;
    }
    let replacement = operations
        .execute_reconciliation_detach(&canonical, &client, rejected_child_id)
        .await
        .unwrap();
    let ReconciliationChildTask::MetadataDetach(replacement) = replacement.payload else {
        panic!("expected verified replacement detach child");
    };
    assert_eq!(replacement.status, ReconciliationDetachStatus::Verified);
    assert_eq!(canonical.head().unwrap(), RevisionId(4));

    server.stop();
    std::fs::remove_dir_all(root).unwrap();
}

fn portrait_manifest(
    descriptor_digest: String,
    _artifact_root: PathBuf,
) -> NativeExtensionStageManifest {
    NativeExtensionStageManifest {
        intents: vec![NativeExtensionIntent::UpsertPortrait(PortraitIntent {
            entity_id: "portrait-01".into(),
            entity_revision: 3,
            presentation: PortraitPresentation::Portrait,
            character_binding: DescriptorReference {
                descriptor_id: DESCRIPTOR_ID.into(),
                expected_digest: descriptor_digest,
            },
            placement: ResolvedPlacement {
                frame: 120,
                primary_layer: 30,
                secondary_layer: None,
            },
            duration_frames: 90,
            replacement_guard: ReplacementGuard::default(),
        })],
        artifact_sources: Vec::new(),
        change_budget: ChangeBudget::create_only(1),
    }
}

fn test_catalog() -> Ymm4DescriptorCatalog {
    Ymm4DescriptorCatalog {
        protocol_version: 2,
        project_id: PROJECT_ID.into(),
        scene_id: SCENE_ID.into(),
        driver_profile_digest: "d".repeat(64),
        catalog_digest: "c".repeat(64),
        descriptors: vec![Ymm4TargetDescriptor {
            descriptor_id: DESCRIPTOR_ID.into(),
            kind: "character".into(),
            name: "霧雨魔理沙".into(),
            config_digest: "a".repeat(64),
            schema_digest: "b".repeat(64),
            bindable: true,
            mutation_allowed: false,
            metadata: BTreeMap::from([
                ("groupName".into(), "TakeGraph".into()),
                ("tachieType".into(), "Ymm4".into()),
            ]),
        }],
    }
}

fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!("takegraph-native-reconcile-{}", Uuid::new_v4()))
}

fn remove_latest_reconciliation_child_generation(operation_root: &Path) {
    let children = operation_root.join("reconciliation-children");
    let child = std::fs::read_dir(children)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .expect("reconciliation child journal directory");
    let mut generations = std::fs::read_dir(child.path().join("generations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("record-"))
        })
        .collect::<Vec<_>>();
    generations.sort();
    std::fs::remove_file(generations.pop().expect("terminal child generation")).unwrap();
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // Independent fault toggles make the single mock bridge deterministic.
struct MockState {
    applied: bool,
    drifted: bool,
    detached: bool,
    detach_apply_count: usize,
    detach_request_count: usize,
    contradictory_detach_responses_remaining: usize,
    reject_detach_before_wal_remaining: usize,
    reject_not_started_seal_remaining: usize,
    capability_drifted: bool,
    native_apply_count: usize,
    detach_request: Option<serde_json::Value>,
    not_started_request: Option<serde_json::Value>,
    realization: Option<serde_json::Value>,
}

struct MockBridge {
    endpoint: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockBridge {
    fn start(catalog: Ymm4DescriptorCatalog, state: Arc<Mutex<MockState>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => handle_request(&mut stream, &catalog, &state),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("mock bridge accept failed: {error}"),
                }
            }
        });
        Self {
            endpoint,
            stop,
            thread: Some(thread),
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for MockBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn handle_request(
    stream: &mut TcpStream,
    catalog: &Ymm4DescriptorCatalog,
    state: &Arc<Mutex<MockState>>,
) {
    // Windows accepted sockets may inherit the listener's nonblocking mode.
    // Read one complete request synchronously so parallel capability probes do
    // not race the first payload byte.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let request = read_request(stream);
    assert_eq!(
        request.headers.get("x-takegraph-token").map(String::as_str),
        Some("test-token")
    );
    let (status, response) = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/health") => (
            200,
            serde_json::json!({
                "status": "running",
                "protocolVersion": 2,
                "pluginVersion": if state.lock().unwrap().capability_drifted {
                    "test-plugin-upgraded"
                } else {
                    "test-plugin"
                },
                "ymm4Version": "4.55.1.1"
            }),
        ),
        ("GET", "/v1/capabilities") => (
            200,
            serde_json::json!({
                "protocolVersion": 2,
                "capabilities": [
                    "readback_verification",
                    "idempotent_apply",
                    "undo_batch",
                    "request_bound_receipts",
                    "write_ahead_apply",
                    "recovery_readback",
                    "mutation_profile_ymm4_4_55_1_1",
                    "metadata_remark_detach",
                    "native_portrait_upsert"
                ]
            }),
        ),
        ("GET", "/v1/project/snapshot") => (200, snapshot_json(state)),
        ("GET", "/v2/descriptors") => (200, serde_json::to_value(catalog).unwrap()),
        ("POST", "/v2/native-extension/plan") => (
            200,
            serde_json::json!({
                "fingerprint": current_fingerprint(&state.lock().unwrap()),
                "descriptorCatalogDigest": catalog.catalog_digest,
                "driverProfileDigest": catalog.driver_profile_digest,
                "observation": { "existing": {} },
                "warnings": []
            }),
        ),
        ("POST", "/v2/native-extension/apply") => (200, apply_json(&request.body, catalog, state)),
        ("POST", "/v2/reconciliation/detach") if take_detach_pre_wal_rejection(state) => (
            409,
            serde_json::json!({
                "error": "metadata detach was rejected before WAL",
                "actualFingerprint": current_fingerprint(&state.lock().unwrap())
            }),
        ),
        ("POST", "/v2/reconciliation/detach") => (200, detach_json(&request.body, state)),
        ("POST", "/v2/reconciliation/detach/not-started")
            if take_not_started_seal_rejection(state) =>
        {
            (503, serde_json::json!({ "error": "seal response lost" }))
        }
        ("POST", "/v2/reconciliation/detach/not-started") => {
            (200, not_started_json(&request.body, state))
        }
        ("GET", path) if path.starts_with("/v2/reconciliation/detach/") => {
            detach_operation_json(state)
        }
        _ => panic!(
            "unexpected mock bridge request: {} {}",
            request.method, request.path
        ),
    };
    let body = serde_json::to_vec(&response).unwrap();
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(&body).unwrap();
    stream.flush().unwrap();
}

fn take_detach_pre_wal_rejection(state: &Arc<Mutex<MockState>>) -> bool {
    let mut state = state.lock().unwrap();
    let reject = state.reject_detach_before_wal_remaining != 0;
    state.reject_detach_before_wal_remaining =
        state.reject_detach_before_wal_remaining.saturating_sub(1);
    reject
}

fn take_not_started_seal_rejection(state: &Arc<Mutex<MockState>>) -> bool {
    let mut state = state.lock().unwrap();
    let reject = state.reject_not_started_seal_remaining != 0;
    state.reject_not_started_seal_remaining =
        state.reject_not_started_seal_remaining.saturating_sub(1);
    reject
}

fn snapshot_json(state: &Arc<Mutex<MockState>>) -> serde_json::Value {
    let state = state.lock().unwrap();
    serde_json::json!({
        "projectId": PROJECT_ID,
        "projectName": "native reconcile",
        "projectPath": "test.ymmp",
        "sceneId": SCENE_ID,
        "fps": 60,
        "fingerprint": current_fingerprint(&state),
        "managedItems": [],
        "nativeExtensions": state.realization.iter().cloned().collect::<Vec<_>>(),
        "unmanagedContextCount": 0
    })
}

fn current_fingerprint(state: &MockState) -> &'static str {
    if state.detached {
        DETACHED
    } else if state.drifted {
        DRIFTED
    } else if state.applied {
        AFTER
    } else {
        BEFORE
    }
}

fn detach_json(body: &[u8], state: &Arc<Mutex<MockState>>) -> serde_json::Value {
    let request: serde_json::Value = serde_json::from_slice(body).unwrap();
    let mut state = state.lock().unwrap();
    state.detach_request_count += 1;
    if state.not_started_request.as_ref() == Some(&request) {
        return metadata_detach_not_started_json(&request, true);
    }
    let replayed = if let Some(existing) = &state.detach_request {
        assert_eq!(existing, &request, "detach operation was rebound on replay");
        true
    } else {
        assert_eq!(request["projectId"], PROJECT_ID);
        assert_eq!(request["sceneId"], SCENE_ID);
        assert_eq!(request["expectedFingerprint"], DRIFTED);
        state.detach_request = Some(request.clone());
        state.detach_apply_count += 1;
        state.detached = true;
        state.realization = None;
        false
    };
    let success = state.contradictory_detach_responses_remaining == 0;
    state.contradictory_detach_responses_remaining = state
        .contradictory_detach_responses_remaining
        .saturating_sub(1);
    serde_json::json!({
        "success": success,
        "replayed": replayed,
        "receipt": metadata_detach_verified_receipt(&request)
    })
}

fn detach_operation_json(state: &Arc<Mutex<MockState>>) -> (u16, serde_json::Value) {
    let state = state.lock().unwrap();
    if let Some(request) = &state.detach_request {
        return (200, metadata_detach_verified_receipt(request));
    }
    if let Some(request) = &state.not_started_request {
        return (
            200,
            metadata_detach_not_started_json(request, true)["receipt"].clone(),
        );
    }
    (404, serde_json::json!({ "error": "not found" }))
}

fn metadata_detach_verified_receipt(request: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "operationId": request["operationId"],
        "requestDigest": request["requestDigest"],
        "projectId": request["projectId"],
        "sceneId": request["sceneId"],
        "sourceRevision": request["sourceRevision"],
        "expectedFingerprint": request["expectedFingerprint"],
        "entityId": request["entityId"],
        "realizationId": request["realizationId"],
        "identityCarrier": request["identityCarrier"],
        "status": "verified",
        "beforeFingerprint": request["expectedFingerprint"],
        "afterFingerprint": DETACHED,
        "detachedItemCount": 1,
        "beforeRemarkDigest": "e".repeat(64),
        "expectedAfterRemarkDigest": "d".repeat(64),
        "remarkDigestAfter": "d".repeat(64),
        "nonRemarkContentDigestBefore": "f".repeat(64),
        "nonRemarkContentDigestAfter": "f".repeat(64),
        "remarkAbsent": true,
        "verified": true,
        "error": null
    })
}

fn not_started_json(body: &[u8], state: &Arc<Mutex<MockState>>) -> serde_json::Value {
    let request: serde_json::Value = serde_json::from_slice(body).unwrap();
    let mut state = state.lock().unwrap();
    if let Some(existing) = &state.not_started_request {
        assert_eq!(existing, &request, "not-started operation was rebound");
        return metadata_detach_not_started_json(&request, true);
    }
    state.not_started_request = Some(request.clone());
    metadata_detach_not_started_json(&request, false)
}

fn metadata_detach_not_started_json(
    request: &serde_json::Value,
    replayed: bool,
) -> serde_json::Value {
    serde_json::json!({
        "success": false,
        "replayed": replayed,
        "receipt": {
            "operationId": request["operationId"],
            "requestDigest": request["requestDigest"],
            "projectId": request["projectId"],
            "sceneId": request["sceneId"],
            "sourceRevision": request["sourceRevision"],
            "expectedFingerprint": request["expectedFingerprint"],
            "entityId": request["entityId"],
            "realizationId": request["realizationId"],
            "identityCarrier": request["identityCarrier"],
            "status": "not_started",
            "beforeFingerprint": request["expectedFingerprint"],
            "afterFingerprint": request["expectedFingerprint"],
            "detachedItemCount": 0,
            "beforeRemarkDigest": "a".repeat(64),
            "expectedAfterRemarkDigest": "a".repeat(64),
            "remarkDigestAfter": "a".repeat(64),
            "nonRemarkContentDigestBefore": "a".repeat(64),
            "nonRemarkContentDigestAfter": "a".repeat(64),
            "remarkAbsent": false,
            "verified": false,
            "error": "durable no-mutation tombstone"
        }
    })
}

fn apply_json(
    body: &[u8],
    catalog: &Ymm4DescriptorCatalog,
    state: &Arc<Mutex<MockState>>,
) -> serde_json::Value {
    let request: serde_json::Value = serde_json::from_slice(body).unwrap();
    let before_fingerprint = current_fingerprint(&state.lock().unwrap());
    let operation = &request["plan"]["operations"][0];
    let realization_id = operation["realizationId"].as_str().unwrap();
    let owned_fields = BTreeMap::from([
        ("logicalKey".to_owned(), "portrait:portrait-01".to_owned()),
        ("projectId".to_owned(), PROJECT_ID.to_owned()),
        ("entityId".to_owned(), "portrait-01".to_owned()),
        ("entityRevision".to_owned(), "3".to_owned()),
        ("kind".to_owned(), "portrait".to_owned()),
        ("frame".to_owned(), "120".to_owned()),
        ("layer".to_owned(), "30".to_owned()),
        ("length".to_owned(), "90".to_owned()),
        ("descriptorId".to_owned(), DESCRIPTOR_ID.to_owned()),
    ]);
    let owned_state_digest = canonical_sha256(
        "takegraph-ymm4-native-extension-owned-state-v1",
        &owned_fields,
    )
    .unwrap();
    let realization = serde_json::json!({
        "logicalKey": "portrait:portrait-01",
        "realizationId": realization_id,
        "kind": "portrait",
        "projectId": PROJECT_ID,
        "entityId": "portrait-01",
        "entityRevision": 3,
        "ownedFields": owned_fields,
    });
    let receipt_realization = serde_json::json!({
        "logicalKey": "portrait:portrait-01",
        "realizationId": realization_id,
        "kind": "portrait",
        "projectId": PROJECT_ID,
        "entityId": "portrait-01",
        "entityRevision": 3,
        "frame": 120,
        "layer": 30,
        "length": 90,
        "ownedStateDigest": owned_state_digest,
        "ownedFields": owned_fields,
        "preservedFields": [],
        "stateDigest": format!("sha256:{}", "e".repeat(64)),
        "unknownEffects": []
    });
    let mut state = state.lock().unwrap();
    state.native_apply_count += 1;
    state.applied = true;
    state.realization = Some(realization);
    serde_json::json!({
        "operationId": request["operationId"],
        "requestDigest": request["requestDigest"],
        "projectId": PROJECT_ID,
        "sceneId": SCENE_ID,
        "status": "verified",
        "beforeFingerprint": before_fingerprint,
        "afterFingerprint": AFTER,
        "descriptorCatalogDigest": catalog.catalog_digest,
        "driverProfileDigest": catalog.driver_profile_digest,
        "realizations": [receipt_realization],
        "verified": true,
        "error": null
    })
}

struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> HttpRequest {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "mock bridge request ended before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header_text = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let mut lines = header_text.split("\r\n");
    let mut start = lines.next().unwrap().split_whitespace();
    let method = start.next().unwrap().to_owned();
    let path = start.next().unwrap().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "mock bridge request body ended early");
        bytes.extend_from_slice(&chunk[..read]);
    }
    HttpRequest {
        method,
        path,
        headers,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}
