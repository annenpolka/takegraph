//! Promote a narration interpretation through an existing timeline edit.

use takegraph_core::{AnnotationId, AnnotationIntent, CaptureStability, SourceEvidenceRef};
use takegraph_node::{
    ManagedItemKind, Ymm4AnnotationMarkerCue, Ymm4BridgeClient, Ymm4ManagedItem,
    Ymm4NativeVoiceCue, Ymm4ProjectSnapshot,
};
use thiserror::Error;
use uuid::Uuid;

use crate::annotation_store::{
    AnnotationStore, AnnotationStoreError, CaptureLifecycle, CaptureProjection,
};
use crate::project_store::DurableProjectStore;
use crate::ymm4_timeline_edit::{
    TimelineEditStageManifest, TimelineEditStageOperation, Ymm4TimelineEditTask,
};

/// Errors while turning an interpretation into a timeline-edit manifest.
#[derive(Debug, Error)]
pub enum PromotionError {
    #[error(transparent)]
    Store(#[from] AnnotationStoreError),
    #[error(transparent)]
    Evidence(#[from] takegraph_core::AnnotationError),
    #[error("capture {0} is dismissed")]
    Dismissed(String),
    #[error("source-changed capture {0} cannot be promoted")]
    SourceChanged(String),
    #[error("capture {0} has no interpretation")]
    MissingInterpretation(String),
    #[error("capture {0} has no narration intent to promote")]
    NoNarration(String),
    #[error("capture {0} has no interpretation to pin")]
    NoInterpretation(String),
    #[error("characterName must not be empty")]
    EmptyCharacter,
    #[error("entityId must not be empty")]
    EmptyEntity,
    #[error("capture {0} has no pinned annotation marker on the live target")]
    PinNotFound(String),
    #[error("capture {0} already has a staged promotion")]
    AlreadyStaged(String),
    #[error("capture {0} is stale against the current source")]
    StaleSource(String),
    #[error("capture {0} belongs to a different project")]
    ProjectMismatch(String),
    #[error(transparent)]
    TimelineEdit(#[from] crate::Ymm4TimelineEditError),
    #[error(transparent)]
    ProjectStore(#[from] crate::ProjectStoreError),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
}

/// Builds native-voice creates for each narration intent, bound to source evidence.
///
/// `CutCandidate`, `Highlight`, `Note`, and `Verify` stay evidence-only.
///
/// # Errors
///
/// Returns a guard failure for dismissed or source-changed captures, or when
/// no narration intent is present.
pub fn narration_promotion_operations(
    projection: &CaptureProjection,
    character_name: &str,
    layer: i32,
    default_max_length: i32,
) -> Result<Vec<TimelineEditStageOperation>, PromotionError> {
    if character_name.trim().is_empty() {
        return Err(PromotionError::EmptyCharacter);
    }
    let key = projection.capture.id.0.to_string();
    if projection.lifecycle == CaptureLifecycle::Dismissed {
        return Err(PromotionError::Dismissed(key));
    }
    if projection.stability == CaptureStability::SourceChanged {
        return Err(PromotionError::SourceChanged(key));
    }
    let interpretation = projection
        .interpretation
        .as_ref()
        .ok_or_else(|| PromotionError::MissingInterpretation(key.clone()))?;
    let transcript = projection
        .transcript
        .as_ref()
        .ok_or_else(|| PromotionError::MissingInterpretation(key.clone()))?;
    let evidence = SourceEvidenceRef {
        annotation_id: projection.capture.id,
        capture_audio_sha256: projection.capture.audio.audio_sha256.clone(),
        transcript_digest: transcript.transcript_digest.clone(),
        interpretation_digest: interpretation.interpretation_digest.clone(),
    };
    evidence.validate()?;
    let frame = projection
        .capture
        .start_anchor
        .frame
        .saturating_add(interpretation.temporal.start_offset_frames)
        .max(0);
    let span = interpretation
        .temporal
        .end_offset_frames
        .map(|end| (end - interpretation.temporal.start_offset_frames).max(1))
        .unwrap_or(default_max_length.max(1));
    let mut operations = Vec::new();
    for (index, intent) in interpretation.intents.iter().enumerate() {
        let AnnotationIntent::Narration { topic, draft_hint } = intent else {
            continue;
        };
        let display_text = draft_hint
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(topic)
            .to_owned();
        let short = projection.capture.id.0.simple().to_string();
        operations.push(TimelineEditStageOperation::NativeVoiceCreate {
            cue: Ymm4NativeVoiceCue {
                realization_id: Uuid::new_v4(),
                entity_id: format!("ann-{}-n{index}", &short[..8]),
                revision: 0,
                character_name: character_name.to_owned(),
                display_text,
                spoken_text: None,
                frame,
                layer,
                max_length: span,
            },
            source_evidence: Some(evidence.clone()),
        });
    }
    if operations.is_empty() {
        return Err(PromotionError::NoNarration(key));
    }
    Ok(operations)
}

/// Builds an annotation-marker create bound to source evidence.
///
/// Highlight, Note, Verify, and CutCandidate may be pinned. The pin is a
/// digest-bound `AnnotationItem`, not a VoiceItem, and never carries the
/// captured WAV.
///
/// # Errors
///
/// Returns a guard failure for dismissed or source-changed captures, or when
/// no interpretation is attached.
pub fn pin_promotion_operations(
    projection: &CaptureProjection,
    layer: i32,
) -> Result<Vec<TimelineEditStageOperation>, PromotionError> {
    let key = projection.capture.id.0.to_string();
    if projection.lifecycle == CaptureLifecycle::Dismissed {
        return Err(PromotionError::Dismissed(key));
    }
    if projection.stability == CaptureStability::SourceChanged {
        return Err(PromotionError::SourceChanged(key));
    }
    let interpretation = projection
        .interpretation
        .as_ref()
        .ok_or_else(|| PromotionError::NoInterpretation(key.clone()))?;
    let transcript = projection
        .transcript
        .as_ref()
        .ok_or_else(|| PromotionError::NoInterpretation(key.clone()))?;
    let evidence = SourceEvidenceRef {
        annotation_id: projection.capture.id,
        capture_audio_sha256: projection.capture.audio.audio_sha256.clone(),
        transcript_digest: transcript.transcript_digest.clone(),
        interpretation_digest: interpretation.interpretation_digest.clone(),
    };
    evidence.validate()?;
    let start = projection.capture.start_anchor.frame.max(0);
    let end = projection.capture.end_anchor.frame;
    let length = (end - start).max(1);
    let label = transcript
        .text
        .chars()
        .take(40)
        .collect::<String>()
        .trim()
        .to_owned();
    let label = if label.is_empty() {
        "メモ".to_owned()
    } else {
        label
    };
    Ok(vec![TimelineEditStageOperation::AnnotationMarkerCreate {
        marker: Ymm4AnnotationMarkerCue {
            realization_id: Uuid::new_v4(),
            entity_id: annotation_pin_entity_id(projection.capture.id),
            annotation_id: projection.capture.id.0,
            project_id: projection.capture.start_anchor.project_id.clone(),
            frame: start,
            layer,
            length,
            label,
        },
        source_evidence: Some(evidence),
    }])
}

/// Builds an annotation-marker delete bound to the existing pin identity.
///
/// Unpin is its own edit and is not implied by dismiss.
///
/// # Errors
///
/// Returns a guard failure for dismissed or source-changed captures.
pub fn unpin_promotion_operations(
    projection: &CaptureProjection,
    entity_id: &str,
    realization_id: Uuid,
    frame: i32,
    layer: i32,
    length: i32,
) -> Result<Vec<TimelineEditStageOperation>, PromotionError> {
    let key = projection.capture.id.0.to_string();
    if projection.lifecycle == CaptureLifecycle::Dismissed {
        return Err(PromotionError::Dismissed(key));
    }
    if projection.stability == CaptureStability::SourceChanged {
        return Err(PromotionError::SourceChanged(key));
    }
    if entity_id.trim().is_empty() {
        return Err(PromotionError::EmptyEntity);
    }
    let interpretation = projection
        .interpretation
        .as_ref()
        .ok_or_else(|| PromotionError::NoInterpretation(key.clone()))?;
    let transcript = projection
        .transcript
        .as_ref()
        .ok_or_else(|| PromotionError::NoInterpretation(key.clone()))?;
    let evidence = SourceEvidenceRef {
        annotation_id: projection.capture.id,
        capture_audio_sha256: projection.capture.audio.audio_sha256.clone(),
        transcript_digest: transcript.transcript_digest.clone(),
        interpretation_digest: interpretation.interpretation_digest.clone(),
    };
    evidence.validate()?;
    Ok(vec![TimelineEditStageOperation::AnnotationMarkerDelete {
        marker: Ymm4AnnotationMarkerCue {
            realization_id,
            entity_id: entity_id.to_owned(),
            annotation_id: projection.capture.id.0,
            project_id: projection.capture.start_anchor.project_id.clone(),
            frame,
            layer,
            length,
            label: "メモ".into(),
        },
        source_evidence: Some(evidence),
    }])
}

/// Deterministic write identity for a capture pin.
#[must_use]
pub fn annotation_pin_entity_id(capture_id: AnnotationId) -> String {
    let short = capture_id.0.simple().to_string();
    format!("ann-{}-pin", &short[..8])
}

/// Finds the live owned pin for `capture_id`, if the snapshot reports one.
#[must_use]
pub fn find_pin_item<'a>(
    target: &'a Ymm4ProjectSnapshot,
    capture_id: AnnotationId,
) -> Option<&'a Ymm4ManagedItem> {
    let entity_id = annotation_pin_entity_id(capture_id);
    target
        .managed_items
        .iter()
        .find(|item| item.kind == ManagedItemKind::Annotation && item.entity_id == entity_id)
}

/// Unpin only binds the live project, not the capture-time editorial fingerprint.
/// The pin itself is an editorial item, so the live fingerprint already moved.
///
/// # Errors
///
/// Returns [`PromotionError::ProjectMismatch`].
pub fn assert_pin_project(
    projection: &CaptureProjection,
    target: &Ymm4ProjectSnapshot,
) -> Result<(), PromotionError> {
    let key = projection.capture.id.0.to_string();
    if target.project_id != projection.capture.start_anchor.project_id {
        return Err(PromotionError::ProjectMismatch(key));
    }
    Ok(())
}

fn maybe_record_additional_promotion(
    annotation_store: &AnnotationStore,
    capture_id: AnnotationId,
    task_id: &str,
    plan_digest: &str,
    base_revision: takegraph_core::RevisionId,
) -> Result<(), PromotionError> {
    match record_staged_promotion(
        annotation_store,
        capture_id,
        task_id,
        plan_digest,
        base_revision,
    ) {
        Ok(()) => Ok(()),
        Err(PromotionError::Store(
            crate::annotation_store::AnnotationStoreError::PromotionAlreadyStaged(key),
        )) => {
            let existing = annotation_store.capture(capture_id)?;
            match existing
                .promotion
                .as_ref()
                .map(|promotion| promotion.status)
            {
                Some(crate::annotation_store::PromotionStatus::Committed) => Ok(()),
                _ => Err(PromotionError::AlreadyStaged(key)),
            }
        }
        Err(error) => Err(error),
    }
}

/// Refuses promotion when the live YMM4 source is not the captured one.
///
/// # Errors
///
/// Returns [`PromotionError::ProjectMismatch`] or
/// [`PromotionError::StaleSource`].
pub fn assert_promotion_target(
    projection: &CaptureProjection,
    target: &Ymm4ProjectSnapshot,
) -> Result<(), PromotionError> {
    let key = projection.capture.id.0.to_string();
    if target.project_id != projection.capture.start_anchor.project_id {
        return Err(PromotionError::ProjectMismatch(key));
    }
    if target.fingerprint != projection.capture.start_anchor.source_fingerprint {
        return Err(PromotionError::StaleSource(key));
    }
    Ok(())
}

/// Stages a pin as an ordinary `timeline_edit` and records promotion status.
///
/// # Errors
///
/// Returns a promotion guard, unsaved-project, or timeline-edit failure.
pub async fn stage_pin_promotion(
    annotation_store: &AnnotationStore,
    project_store: &DurableProjectStore,
    client: &Ymm4BridgeClient,
    target: Ymm4ProjectSnapshot,
    capture_id: AnnotationId,
    layer: i32,
) -> Result<StagedNarrationPromotion, PromotionError> {
    let projection = annotation_store.capture(capture_id)?;
    assert_promotion_target(&projection, &target)?;
    let operations = pin_promotion_operations(&projection, layer)?;
    let head = project_store.head()?;
    let task = Ymm4TimelineEditTask::stage_from_snapshot(
        client,
        head,
        target,
        TimelineEditStageManifest {
            operations: operations.clone(),
            max_changed_entities: None,
        },
    )
    .await?;
    let plan_digest = task
        .timeline_edit_plan
        .canonical_digest()
        .map_err(crate::Ymm4TimelineEditError::from)?;
    maybe_record_additional_promotion(
        annotation_store,
        capture_id,
        &task.operation_id.to_string(),
        &plan_digest,
        head,
    )?;
    Ok(StagedNarrationPromotion {
        operations,
        task,
        plan_digest,
    })
}

/// Stages an unpin as an ordinary `timeline_edit` against the live pin identity.
///
/// # Errors
///
/// Returns a promotion guard, missing pin, or timeline-edit failure.
pub async fn stage_unpin_promotion(
    annotation_store: &AnnotationStore,
    project_store: &DurableProjectStore,
    client: &Ymm4BridgeClient,
    target: Ymm4ProjectSnapshot,
    capture_id: AnnotationId,
) -> Result<StagedNarrationPromotion, PromotionError> {
    let projection = annotation_store.capture(capture_id)?;
    assert_pin_project(&projection, &target)?;
    let pin = find_pin_item(&target, capture_id)
        .ok_or_else(|| PromotionError::PinNotFound(projection.capture.id.0.to_string()))?;
    let realization_id = pin
        .realization_id
        .ok_or_else(|| PromotionError::PinNotFound(projection.capture.id.0.to_string()))?;
    let operations = unpin_promotion_operations(
        &projection,
        &pin.entity_id,
        realization_id,
        pin.frame,
        pin.layer,
        pin.length,
    )?;
    let head = project_store.head()?;
    let task = Ymm4TimelineEditTask::stage_from_snapshot(
        client,
        head,
        target,
        TimelineEditStageManifest {
            operations: operations.clone(),
            max_changed_entities: None,
        },
    )
    .await?;
    let plan_digest = task
        .timeline_edit_plan
        .canonical_digest()
        .map_err(crate::Ymm4TimelineEditError::from)?;
    maybe_record_additional_promotion(
        annotation_store,
        capture_id,
        &task.operation_id.to_string(),
        &plan_digest,
        head,
    )?;
    Ok(StagedNarrationPromotion {
        operations,
        task,
        plan_digest,
    })
}

/// One staged narration promotion bound to an ordinary timeline-edit task.
pub struct StagedNarrationPromotion {
    pub operations: Vec<TimelineEditStageOperation>,
    pub task: Ymm4TimelineEditTask,
    pub plan_digest: String,
}

/// Builds narration operations, stages a `timeline_edit`, and records
/// [`crate::annotation_store::PromotionStatus::Staged`].
///
/// # Errors
///
/// Returns a promotion guard, unsaved-project, or timeline-edit failure.
/// Staging does not execute the edit or advance the canonical head.
pub async fn stage_narration_promotion(
    annotation_store: &AnnotationStore,
    project_store: &DurableProjectStore,
    client: &Ymm4BridgeClient,
    target: Ymm4ProjectSnapshot,
    capture_id: AnnotationId,
    character_name: &str,
    layer: i32,
    default_max_length: i32,
) -> Result<StagedNarrationPromotion, PromotionError> {
    let projection = annotation_store.capture(capture_id)?;
    assert_promotion_target(&projection, &target)?;
    let operations =
        narration_promotion_operations(&projection, character_name, layer, default_max_length)?;
    let head = project_store.head()?;
    let task = Ymm4TimelineEditTask::stage_from_snapshot(
        client,
        head,
        target,
        TimelineEditStageManifest {
            operations: operations.clone(),
            max_changed_entities: None,
        },
    )
    .await?;
    let plan_digest = task
        .timeline_edit_plan
        .canonical_digest()
        .map_err(crate::Ymm4TimelineEditError::from)?;
    record_staged_promotion(
        annotation_store,
        capture_id,
        &task.operation_id.to_string(),
        &plan_digest,
        head,
    )?;
    Ok(StagedNarrationPromotion {
        operations,
        task,
        plan_digest,
    })
}

/// Records that a timeline-edit plan was staged from one capture.
///
/// # Errors
///
/// Returns store guard failures.
pub fn record_staged_promotion(
    store: &AnnotationStore,
    capture_id: AnnotationId,
    task_id: &str,
    plan_digest: &str,
    base_revision: takegraph_core::RevisionId,
) -> Result<(), PromotionError> {
    store.stage_promotion(capture_id, task_id, plan_digest, base_revision)?;
    Ok(())
}

/// Records committed promotions for every unique capture sealed into a plan.
///
/// Exact replay of the same task, revision, and receipt is idempotent.
///
/// # Errors
///
/// Returns store guard failures.
pub fn commit_promotions_from_plan(
    store: &AnnotationStore,
    source_evidence: &[SourceEvidenceRef],
    task_id: &str,
    committed_revision: takegraph_core::RevisionId,
    receipt_digest: &str,
) -> Result<usize, PromotionError> {
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    for evidence in source_evidence {
        if !seen.insert(evidence.annotation_id) {
            continue;
        }
        record_committed_promotion(
            store,
            evidence.annotation_id,
            task_id,
            committed_revision,
            receipt_digest,
        )?;
        count += 1;
    }
    Ok(count)
}

/// Records that a staged promotion committed.
///
/// # Errors
///
/// Returns store guard failures.
pub fn record_committed_promotion(
    store: &AnnotationStore,
    capture_id: AnnotationId,
    task_id: &str,
    committed_revision: takegraph_core::RevisionId,
    receipt_digest: &str,
) -> Result<(), PromotionError> {
    store.commit_promotion(capture_id, task_id, committed_revision, receipt_digest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        AnnotationCapture, AnnotationInterpretation, AnnotationTranscript, CaptureSessionId,
        CapturedAudioEvidence, RevisionId, SourceAnchor, TemporalReference, TemporalRelation,
    };

    fn projection(id: AnnotationId, intents: Vec<AnnotationIntent>) -> CaptureProjection {
        let store_root = std::env::temp_dir().join(format!("takegraph-promote-{}", Uuid::new_v4()));
        let store = AnnotationStore::open_scoped(&store_root, "project-a").unwrap();
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
        store
            .attach_transcript(AnnotationTranscript {
                id: Uuid::new_v4(),
                capture_id: id,
                audio_sha256: format!("sha256:{}", "a".repeat(64)),
                text: "今のところ三秒前から残す。ここはCompressionの説明を入れる".into(),
                provider_id: "human".into(),
                provider_digest: format!("sha256:{}", "b".repeat(64)),
                transcript_digest: format!("sha256:{}", "c".repeat(64)),
            })
            .unwrap();
        store
            .attach_interpretation(AnnotationInterpretation {
                id: Uuid::new_v4(),
                capture_id: id,
                transcript_digest: format!("sha256:{}", "c".repeat(64)),
                temporal: TemporalReference {
                    reference_frame: 2531,
                    start_offset_frames: -180,
                    end_offset_frames: Some(0),
                    relation: TemporalRelation::Range,
                },
                intents,
                model_id: "heuristic-v1".into(),
                model_digest: format!("sha256:{}", "d".repeat(64)),
                interpretation_digest: format!("sha256:{}", "e".repeat(64)),
            })
            .unwrap();
        let loaded = store.capture(id).unwrap();
        let _ = std::fs::remove_dir_all(store_root);
        loaded
    }

    #[test]
    fn narration_becomes_native_create_with_evidence() {
        let id = AnnotationId::new();
        let ops = narration_promotion_operations(
            &projection(
                id,
                vec![
                    AnnotationIntent::Highlight {
                        reason: Some("残す".into()),
                    },
                    AnnotationIntent::Narration {
                        topic: "Compression".into(),
                        draft_hint: Some("ここで重要なのがPrimary Compressionです。".into()),
                    },
                    AnnotationIntent::CutCandidate {
                        reason: Some("切る".into()),
                    },
                ],
            ),
            "ゆっくり霊夢",
            2,
            300,
        )
        .unwrap();
        assert_eq!(ops.len(), 1);
        let TimelineEditStageOperation::NativeVoiceCreate {
            cue,
            source_evidence,
        } = &ops[0]
        else {
            panic!("expected native create");
        };
        assert_eq!(cue.character_name, "ゆっくり霊夢");
        assert_eq!(cue.frame, 2351);
        assert_eq!(cue.max_length, 180);
        assert_eq!(
            cue.display_text,
            "ここで重要なのがPrimary Compressionです。"
        );
        assert_eq!(source_evidence.as_ref().unwrap().annotation_id, id);
    }

    #[test]
    fn stale_or_foreign_source_is_refused() {
        let id = AnnotationId::new();
        let loaded = projection(
            id,
            vec![AnnotationIntent::Narration {
                topic: "Compression".into(),
                draft_hint: None,
            }],
        );
        let mut target = takegraph_node::Ymm4ProjectSnapshot {
            project_id: "project-a".into(),
            project_name: "project-a".into(),
            project_path: "project.ymmp".into(),
            scene_id: "scene-1".into(),
            fps: 60,
            fingerprint: "fp-other".into(),
            managed_items: vec![],
            native_extensions: vec![],
            unmanaged_context_count: 0,
        };
        assert!(matches!(
            assert_promotion_target(&loaded, &target),
            Err(PromotionError::StaleSource(_))
        ));
        target.fingerprint = "fp-1".into();
        target.project_id = "project-b".into();
        assert!(matches!(
            assert_promotion_target(&loaded, &target),
            Err(PromotionError::ProjectMismatch(_))
        ));
        target.project_id = "project-a".into();
        assert!(assert_promotion_target(&loaded, &target).is_ok());
    }

    #[test]
    fn pin_is_an_annotation_marker_with_evidence() {
        let id = AnnotationId::new();
        let ops = pin_promotion_operations(
            &projection(
                id,
                vec![AnnotationIntent::Highlight {
                    reason: Some("残す".into()),
                }],
            ),
            90,
        )
        .unwrap();
        assert_eq!(ops.len(), 1);
        let TimelineEditStageOperation::AnnotationMarkerCreate {
            marker,
            source_evidence,
        } = &ops[0]
        else {
            panic!("expected pin create");
        };
        assert_eq!(marker.annotation_id, id.0);
        assert_eq!(marker.project_id, "project-a");
        assert_eq!(marker.frame, 2531);
        assert_eq!(marker.length, 167);
        assert_eq!(marker.layer, 90);
        assert_eq!(marker.entity_id, annotation_pin_entity_id(id));
        assert_eq!(source_evidence.as_ref().unwrap().annotation_id, id);
    }

    #[test]
    fn unpin_uses_live_pin_identity_and_evidence() {
        let id = AnnotationId::new();
        let realization = Uuid::from_u128(99);
        let ops = unpin_promotion_operations(
            &projection(id, vec![AnnotationIntent::Note]),
            &annotation_pin_entity_id(id),
            realization,
            2531,
            90,
            167,
        )
        .unwrap();
        let TimelineEditStageOperation::AnnotationMarkerDelete {
            marker,
            source_evidence,
        } = &ops[0]
        else {
            panic!("expected unpin delete");
        };
        assert_eq!(marker.entity_id, annotation_pin_entity_id(id));
        assert_eq!(marker.realization_id, realization);
        assert_eq!(marker.frame, 2531);
        assert_eq!(source_evidence.as_ref().unwrap().annotation_id, id);
    }

    #[test]
    fn find_pin_item_matches_deterministic_entity() {
        let id = AnnotationId::new();
        let entity = annotation_pin_entity_id(id);
        let realization = Uuid::from_u128(7);
        let target = takegraph_node::Ymm4ProjectSnapshot {
            project_id: "project-a".into(),
            project_name: "project-a".into(),
            project_path: "project.ymmp".into(),
            scene_id: "scene-1".into(),
            fps: 60,
            fingerprint: "fp-1".into(),
            managed_items: vec![takegraph_node::Ymm4ManagedItem {
                entity_id: entity.clone(),
                revision: 0,
                kind: ManagedItemKind::Annotation,
                frame: 12,
                layer: 90,
                length: 30,
                text: Some("メモ".into()),
                spoken_text: None,
                audio_path: None,
                artifact_hash: None,
                speaker: None,
                realization_id: Some(realization),
            }],
            native_extensions: vec![],
            unmanaged_context_count: 0,
        };
        let found = find_pin_item(&target, id).expect("pin");
        assert_eq!(found.entity_id, entity);
        assert_eq!(found.realization_id, Some(realization));
        assert!(find_pin_item(&target, AnnotationId::new()).is_none());
    }

    #[test]
    fn pin_refuses_dismissed_and_source_changed() {
        let id = AnnotationId::new();
        let loaded = projection(id, vec![AnnotationIntent::Note]);
        let mut dismissed = loaded.clone();
        dismissed.lifecycle = CaptureLifecycle::Dismissed;
        assert!(matches!(
            pin_promotion_operations(&dismissed, 90),
            Err(PromotionError::Dismissed(_))
        ));
        let mut changed = loaded;
        changed.stability = CaptureStability::SourceChanged;
        assert!(matches!(
            pin_promotion_operations(&changed, 90),
            Err(PromotionError::SourceChanged(_))
        ));
    }

    #[test]
    fn highlight_only_is_not_promoted() {
        let id = AnnotationId::new();
        let error = narration_promotion_operations(
            &projection(
                id,
                vec![AnnotationIntent::Highlight {
                    reason: Some("残す".into()),
                }],
            ),
            "ゆっくり霊夢",
            2,
            300,
        );
        assert!(matches!(error, Err(PromotionError::NoNarration(_))));
    }
}
