//! Shared conversion from bridge read-back DTOs into the adapter-independent
//! semantic ownership projection used by durable reconciliation.

use std::collections::BTreeMap;

use takegraph_core::{ManagedSemanticIdentity, ManagedSemanticItem, ManagedSemanticValue};
use takegraph_node::{
    ManagedItemKind, Ymm4ExistingNativeExtensionKind, Ymm4ManagedItem, Ymm4ManagedNativeExtension,
    Ymm4NativeExtensionRealization, Ymm4ProjectSnapshot,
};

/// Versioned field ownership profile used by durable native-extension
/// projection and fresh snapshot projection alike. In particular,
/// `hostStateDigest`, `footprintDigest`, preserved fields, state digests, and
/// unknown effects are observations/witnesses rather than TakeGraph-owned
/// semantics, so they are not admitted here.
pub(crate) const NATIVE_EXTENSION_PROJECTION_PROFILE: &str =
    "takegraph-ymm4-native-extension-owned-projection/v1";

pub(crate) fn project_managed_item(item: &Ymm4ManagedItem) -> ManagedSemanticItem {
    let mut owned_fields = BTreeMap::from([
        (
            "frame".into(),
            ManagedSemanticValue::Integer(i64::from(item.frame)),
        ),
        (
            "layer".into(),
            ManagedSemanticValue::Integer(i64::from(item.layer)),
        ),
        (
            "length".into(),
            ManagedSemanticValue::Integer(i64::from(item.length)),
        ),
    ]);
    for (name, value) in [
        ("text", item.text.as_ref()),
        ("audioPath", item.audio_path.as_ref()),
        ("artifactHash", item.artifact_hash.as_ref()),
        ("speaker", item.speaker.as_ref()),
    ] {
        if let Some(value) = value {
            owned_fields.insert(name.into(), ManagedSemanticValue::Text(value.clone()));
        }
    }
    ManagedSemanticItem {
        identity: ManagedSemanticIdentity {
            entity_id: item.entity_id.clone(),
            realization_id: item.realization_id,
        },
        entity_revision: item.revision,
        realization_kind: match item.kind {
            ManagedItemKind::Audio => "portable_audio".into(),
            ManagedItemKind::Caption => "portable_caption".into(),
            ManagedItemKind::Voice => "ymm4_native_voice".into(),
        },
        owned_fields,
    }
}

pub(crate) fn project_native_extension_realization(
    item: &Ymm4NativeExtensionRealization,
) -> ManagedSemanticItem {
    project_native_extension(
        item.entity_id.clone(),
        item.realization_id,
        item.entity_revision,
        item.kind,
        &item.owned_fields,
    )
}

pub(crate) fn project_native_extension_observation(
    item: &Ymm4ManagedNativeExtension,
) -> ManagedSemanticItem {
    project_native_extension(
        item.entity_id.clone(),
        item.realization_id,
        item.entity_revision,
        item.kind,
        &item.owned_fields,
    )
}

pub(crate) fn project_snapshot(snapshot: &Ymm4ProjectSnapshot) -> Vec<ManagedSemanticItem> {
    snapshot
        .managed_items
        .iter()
        .map(project_managed_item)
        .chain(
            snapshot
                .native_extensions
                .iter()
                .map(project_native_extension_observation),
        )
        .collect()
}

fn project_native_extension(
    entity_id: String,
    realization_id: uuid::Uuid,
    entity_revision: u64,
    kind: Ymm4ExistingNativeExtensionKind,
    source: &BTreeMap<String, String>,
) -> ManagedSemanticItem {
    let mut names = vec![
        "logicalKey",
        "projectId",
        "entityId",
        "entityRevision",
        "kind",
    ];
    names.extend(match kind {
        Ymm4ExistingNativeExtensionKind::Portrait | Ymm4ExistingNativeExtensionKind::Face => {
            &["frame", "layer", "length", "descriptorId"][..]
        }
        Ymm4ExistingNativeExtensionKind::Image
        | Ymm4ExistingNativeExtensionKind::Video
        | Ymm4ExistingNativeExtensionKind::Audio
        | Ymm4ExistingNativeExtensionKind::Bgm => &[
            "frame",
            "layer",
            "length",
            "artifactDigest",
            "byteLength",
            "loopPlayback",
        ],
        Ymm4ExistingNativeExtensionKind::ManagedEffect => &[
            "descriptorId",
            "stableTypeId",
            "collection",
            "parametersDigest",
        ],
        Ymm4ExistingNativeExtensionKind::Template => {
            &["frame", "layer", "descriptorId", "partCount"]
        }
    });
    let owned_fields = names
        .into_iter()
        .filter_map(|name| {
            source
                .get(name)
                .cloned()
                .map(|value| (name.into(), ManagedSemanticValue::Text(value)))
        })
        .collect();
    ManagedSemanticItem {
        identity: ManagedSemanticIdentity {
            entity_id,
            realization_id: Some(realization_id),
        },
        entity_revision,
        realization_kind: format!("ymm4_native_{}", native_extension_kind_name(kind)),
        owned_fields,
    }
}

const fn native_extension_kind_name(kind: Ymm4ExistingNativeExtensionKind) -> &'static str {
    match kind {
        Ymm4ExistingNativeExtensionKind::Portrait => "portrait",
        Ymm4ExistingNativeExtensionKind::Face => "face",
        Ymm4ExistingNativeExtensionKind::Image => "image",
        Ymm4ExistingNativeExtensionKind::Video => "video",
        Ymm4ExistingNativeExtensionKind::Audio => "audio",
        Ymm4ExistingNativeExtensionKind::Bgm => "bgm",
        Ymm4ExistingNativeExtensionKind::ManagedEffect => "managed_effect",
        Ymm4ExistingNativeExtensionKind::Template => "template",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use takegraph_core::{
        ReconciliationSource, RevisionId, SemanticDriftKind, SemanticDriftReport,
    };
    use takegraph_node::{
        Ymm4ExistingNativeExtensionKind, Ymm4ManagedNativeExtension,
        Ymm4NativeExtensionRealization, Ymm4OpaqueNativeEffect, Ymm4PreservedNativeField,
    };
    use uuid::Uuid;

    fn bridge_item(kind: ManagedItemKind) -> Ymm4ManagedItem {
        Ymm4ManagedItem {
            entity_id: "utt-01".into(),
            revision: 3,
            kind,
            frame: 120,
            layer: match kind {
                ManagedItemKind::Audio => 10,
                ManagedItemKind::Caption => 20,
                ManagedItemKind::Voice => 30,
            },
            length: 90,
            text: matches!(kind, ManagedItemKind::Caption | ManagedItemKind::Voice)
                .then(|| "caption".into()),
            spoken_text: matches!(kind, ManagedItemKind::Voice).then(|| "caption".into()),
            audio_path: matches!(kind, ManagedItemKind::Audio).then(|| "voice.wav".into()),
            artifact_hash: (!matches!(kind, ManagedItemKind::Voice))
                .then(|| "sha256:artifact".into()),
            speaker: matches!(kind, ManagedItemKind::Voice).then(|| "speaker".into()),
            realization_id: matches!(kind, ManagedItemKind::Voice).then(Uuid::new_v4),
        }
    }

    fn source() -> ReconciliationSource {
        ReconciliationSource {
            source_revision: RevisionId(3),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            target_identity_digest: "sha256:target".into(),
            state_profile_digest: "sha256:profile".into(),
            capability_digest: "sha256:capabilities".into(),
            mutation_feature_version: 1,
            mutation_feature_schema_digest: "sha256:feature-schema".into(),
            mutation_feature_properties_digest: "sha256:feature-properties".into(),
        }
    }

    fn portable_pair() -> Vec<ManagedSemanticItem> {
        [ManagedItemKind::Audio, ManagedItemKind::Caption]
            .into_iter()
            .map(|kind| project_managed_item(&bridge_item(kind)))
            .collect()
    }

    #[test]
    fn bridge_portable_pair_round_trips_without_false_drift() {
        let expected = portable_pair();
        let report = SemanticDriftReport::build(source(), expected.clone(), expected).unwrap();
        assert!(report.entries.is_empty());
    }

    #[test]
    fn bridge_portable_pair_reports_a_missing_caption() {
        let expected = portable_pair();
        let report =
            SemanticDriftReport::build(source(), expected.clone(), vec![expected[0].clone()])
                .unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::ChangedOwnedFields
        );
        assert_eq!(
            report.entries[0].fields[0].field,
            "portable_caption.$present"
        );
    }

    #[test]
    fn bridge_portable_pair_reports_one_changed_member() {
        let expected = portable_pair();
        let mut actual = expected.clone();
        actual[1]
            .owned_fields
            .insert("text".into(), ManagedSemanticValue::Text("edited".into()));
        let report = SemanticDriftReport::build(source(), expected, actual).unwrap();
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::ChangedOwnedFields
        );
        assert_eq!(report.entries[0].fields[0].field, "portable_caption.text");
    }

    #[test]
    fn bridge_duplicate_kind_fails_closed() {
        let expected = portable_pair();
        let report = SemanticDriftReport::build(
            source(),
            expected.clone(),
            vec![expected[0].clone(), expected[0].clone()],
        )
        .unwrap();
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::DuplicateManagedIdentity
        );
    }

    #[test]
    fn bridge_unrelated_cross_kind_collision_fails_closed() {
        let mut voice = project_managed_item(&bridge_item(ManagedItemKind::Voice));
        voice.identity = portable_pair()[0].identity.clone();
        let report = SemanticDriftReport::build(
            source(),
            vec![portable_pair()[0].clone(), voice],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::DuplicateManagedIdentity
        );
    }

    fn native_owned_fields(descriptor: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("logicalKey".into(), "portrait:portrait-01".into()),
            ("projectId".into(), "project-a".into()),
            ("entityId".into(), "portrait-01".into()),
            ("entityRevision".into(), "3".into()),
            ("kind".into(), "portrait".into()),
            ("frame".into(), "120".into()),
            ("layer".into(), "30".into()),
            ("length".into(), "90".into()),
            ("descriptorId".into(), descriptor.into()),
            ("hostStateDigest".into(), "sha256:preserved".into()),
            ("footprintDigest".into(), "sha256:opaque".into()),
        ])
    }

    #[test]
    fn native_extension_snapshot_reports_owned_drift_only() {
        let realization_id = Uuid::new_v4();
        let expected = project_native_extension_realization(&Ymm4NativeExtensionRealization {
            logical_key: "portrait:portrait-01".into(),
            realization_id,
            kind: Ymm4ExistingNativeExtensionKind::Portrait,
            project_id: "project-a".into(),
            entity_id: "portrait-01".into(),
            entity_revision: 3,
            frame: 120,
            layer: 30,
            length: 90,
            owned_state_digest: "sha256:owned".into(),
            owned_fields: native_owned_fields("character.marisa"),
            preserved_fields: vec![Ymm4PreservedNativeField {
                field: "remark".into(),
                state_digest: "sha256:preserved".into(),
            }],
            state_digest: "sha256:state".into(),
            unknown_effects: vec![Ymm4OpaqueNativeEffect {
                stable_type_id: "third.party.Glow".into(),
                instance_key: "fx-1".into(),
                state_digest: "sha256:unknown".into(),
            }],
        });
        assert!(!expected.owned_fields.contains_key("hostStateDigest"));
        assert!(!expected.owned_fields.contains_key("footprintDigest"));

        let observed = Ymm4ManagedNativeExtension {
            logical_key: "portrait:portrait-01".into(),
            realization_id,
            kind: Ymm4ExistingNativeExtensionKind::Portrait,
            project_id: "project-a".into(),
            entity_id: "portrait-01".into(),
            entity_revision: 3,
            owned_fields: native_owned_fields("character.marisa"),
        };
        let actual = project_native_extension_observation(&observed);
        let matching =
            SemanticDriftReport::build(source(), vec![expected.clone()], vec![actual]).unwrap();
        assert!(matching.entries.is_empty());

        let mut edited = observed;
        edited
            .owned_fields
            .insert("descriptorId".into(), "character.alice".into());
        // A preservation/unknown-effect change cannot be injected into this
        // managed-only DTO; an owned descriptor edit remains visible.
        let report = SemanticDriftReport::build(
            source(),
            vec![expected],
            vec![project_native_extension_observation(&edited)],
        )
        .unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::ChangedOwnedFields
        );
        assert_eq!(report.entries[0].fields[0].field, "descriptorId");
    }
}
