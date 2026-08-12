//! Pure semantic drift detection for managed external realizations.
//!
//! Callers must first project an adapter snapshot onto the fields `TakeGraph`
//! explicitly owns. Unmanaged YMM4 items and host-local fields are therefore
//! outside both the digest and every action produced by this module.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{CanonicalError, RevisionId, canonical_sha256};

/// Canonical schema version for drift reports and reconciliation previews.
pub const RECONCILIATION_SCHEMA_VERSION: u32 = 2;

/// Stable identity embedded in one TakeGraph-managed external realization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSemanticIdentity {
    pub entity_id: String,
    pub realization_id: Option<Uuid>,
}

/// JSON-safe values admitted to the semantic ownership projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum ManagedSemanticValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Text(String),
    TextList(Vec<String>),
}

/// Adapter-independent projection of a managed realization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSemanticItem {
    pub identity: ManagedSemanticIdentity,
    pub entity_revision: u64,
    pub realization_kind: String,
    /// Only strict/derived fields owned by `TakeGraph` belong here. Preserved,
    /// global, and unmanaged fields must never be inserted by an adapter.
    pub owned_fields: BTreeMap<String, ManagedSemanticValue>,
}

impl ManagedSemanticItem {
    fn validate(&self) -> Result<(), ReconciliationError> {
        if self.identity.entity_id.trim().is_empty() {
            return Err(ReconciliationError::EmptyField("entityId"));
        }
        if self.realization_kind.trim().is_empty() {
            return Err(ReconciliationError::EmptyField("realizationKind"));
        }
        if self.owned_fields.keys().any(|name| name.trim().is_empty()) {
            return Err(ReconciliationError::EmptyField("ownedFieldName"));
        }
        Ok(())
    }
}

/// Revision and target evidence that makes a drift report stale-detectable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationSource {
    pub source_revision: RevisionId,
    pub project_id: String,
    pub scene_id: String,
    pub target_identity_digest: String,
    pub state_profile_digest: String,
    /// Full normalized adapter capability contract observed during staging.
    pub capability_digest: String,
    /// Exact reconciliation mutation feature version sealed by approval.
    pub mutation_feature_version: u32,
    /// Schema and property values are bound independently so a bridge cannot
    /// preserve its version label while changing mutation semantics.
    pub mutation_feature_schema_digest: String,
    pub mutation_feature_properties_digest: String,
}

impl ReconciliationSource {
    fn validate(&self) -> Result<(), ReconciliationError> {
        for (name, value) in [
            ("projectId", self.project_id.as_str()),
            ("sceneId", self.scene_id.as_str()),
            ("targetIdentityDigest", self.target_identity_digest.as_str()),
            ("stateProfileDigest", self.state_profile_digest.as_str()),
            ("capabilityDigest", self.capability_digest.as_str()),
            (
                "mutationFeatureSchemaDigest",
                self.mutation_feature_schema_digest.as_str(),
            ),
            (
                "mutationFeaturePropertiesDigest",
                self.mutation_feature_properties_digest.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ReconciliationError::EmptyField(name));
            }
        }
        if self.mutation_feature_version == 0 {
            return Err(ReconciliationError::InvalidMutationFeatureVersion);
        }
        Ok(())
    }
}

/// One owned-field mismatch on an otherwise matching identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedFieldDrift {
    pub field: String,
    pub expected: Option<ManagedSemanticValue>,
    pub actual: Option<ManagedSemanticValue>,
}

/// Why one managed identity differs from the canonical projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticDriftKind {
    MissingFromTarget,
    UnexpectedManagedTarget,
    ChangedOwnedFields,
    DuplicateManagedIdentity,
}

/// Reviewable difference for exactly one managed identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticDriftEntry {
    pub entry_id: String,
    pub identity: ManagedSemanticIdentity,
    pub kind: SemanticDriftKind,
    pub expected: Vec<ManagedSemanticItem>,
    pub actual: Vec<ManagedSemanticItem>,
    pub fields: Vec<ManagedFieldDrift>,
}

/// Deterministic managed-subset drift report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticDriftReport {
    pub schema_version: u32,
    pub source: ReconciliationSource,
    pub expected_managed_state_digest: String,
    pub actual_managed_state_digest: String,
    pub entries: Vec<SemanticDriftEntry>,
    pub report_digest: String,
}

impl SemanticDriftReport {
    /// Compares canonical and observed projections without considering any
    /// unmanaged target state.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid managed records or canonicalization.
    pub fn build(
        source: ReconciliationSource,
        expected: Vec<ManagedSemanticItem>,
        actual: Vec<ManagedSemanticItem>,
    ) -> Result<Self, ReconciliationError> {
        source.validate()?;
        for item in expected.iter().chain(&actual) {
            item.validate()?;
        }

        let expected_managed_state_digest = managed_state_digest(&expected)?;
        let actual_managed_state_digest = managed_state_digest(&actual)?;
        let expected_groups = group_by_identity(expected);
        let actual_groups = group_by_identity(actual);
        let identities = expected_groups
            .keys()
            .chain(actual_groups.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut entries = Vec::new();
        for identity in identities {
            let expected = expected_groups.get(&identity).cloned().unwrap_or_default();
            let actual = actual_groups.get(&identity).cloned().unwrap_or_default();
            let (kind, fields) = if !is_legal_identity_group(&identity, &expected)
                || !is_legal_identity_group(&identity, &actual)
            {
                (SemanticDriftKind::DuplicateManagedIdentity, Vec::new())
            } else if actual.is_empty() {
                (SemanticDriftKind::MissingFromTarget, Vec::new())
            } else if expected.is_empty() {
                (SemanticDriftKind::UnexpectedManagedTarget, Vec::new())
            } else {
                let fields = compare_identity_groups(&expected, &actual);
                if fields.is_empty() {
                    continue;
                }
                (SemanticDriftKind::ChangedOwnedFields, fields)
            };
            let entry_id = canonical_sha256(
                "takegraph-reconciliation-entry-v1",
                &(&identity, kind, &expected, &actual, &fields),
            )?;
            entries.push(SemanticDriftEntry {
                entry_id,
                identity,
                kind,
                expected,
                actual,
                fields,
            });
        }
        let report_digest = canonical_sha256(
            "takegraph-semantic-drift-report-v1",
            &(
                RECONCILIATION_SCHEMA_VERSION,
                &source,
                &expected_managed_state_digest,
                &actual_managed_state_digest,
                &entries,
            ),
        )?;
        Ok(Self {
            schema_version: RECONCILIATION_SCHEMA_VERSION,
            source,
            expected_managed_state_digest,
            actual_managed_state_digest,
            entries,
            report_digest,
        })
    }

    /// Returns true if any source binding changed since preview.
    #[must_use]
    pub fn is_stale_against(&self, current: &ReconciliationSource) -> bool {
        &self.source != current
    }

    /// Recomputes the complete report digest to detect editable-file changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload was modified or cannot be hashed.
    pub fn verify_digest(&self) -> Result<(), ReconciliationError> {
        let actual = canonical_sha256(
            "takegraph-semantic-drift-report-v1",
            &(
                self.schema_version,
                &self.source,
                &self.expected_managed_state_digest,
                &self.actual_managed_state_digest,
                &self.entries,
            ),
        )?;
        if actual != self.report_digest {
            return Err(ReconciliationError::DigestMismatch {
                expected: self.report_digest.clone(),
                actual,
            });
        }
        Ok(())
    }
}

/// Explicit user choice for one drift entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationChoice {
    ImportIntoTakeGraph,
    DetachFromTakeGraph,
    ReExportCanonical,
}

/// A choice bound to one immutable report entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationDecision {
    pub entry_id: String,
    pub choice: ReconciliationChoice,
}

/// Fully bound preview of the actions resulting from user decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconciliationPreview {
    pub schema_version: u32,
    pub report_digest: String,
    pub source: ReconciliationSource,
    pub decisions: Vec<ReconciliationDecision>,
    pub actions: Vec<ReconciliationAction>,
    pub approval_digest: String,
}

/// Service-level action. Import remains a semantic patch proposal; detach and
/// re-export remain explicit adapter operations. None is silently executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ReconciliationAction {
    ProposeImportPatch {
        entry_id: String,
        identity: ManagedSemanticIdentity,
        observed: Vec<ManagedSemanticItem>,
    },
    DetachManagedIdentity {
        entry_id: String,
        identity: ManagedSemanticIdentity,
        observed: Vec<ManagedSemanticItem>,
    },
    ReExportCanonicalState {
        entry_id: String,
        identity: ManagedSemanticIdentity,
        canonical: Vec<ManagedSemanticItem>,
    },
}

impl ReconciliationPreview {
    /// Binds exactly one decision to every drift entry.
    ///
    /// # Errors
    ///
    /// Returns an error for missing/duplicate decisions or a corrupt report.
    pub fn build(
        report: &SemanticDriftReport,
        mut decisions: Vec<ReconciliationDecision>,
    ) -> Result<Self, ReconciliationError> {
        report.verify_digest()?;
        decisions.sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
        let decision_ids = decisions
            .iter()
            .map(|decision| decision.entry_id.as_str())
            .collect::<BTreeSet<_>>();
        if decision_ids.len() != decisions.len() {
            return Err(ReconciliationError::DuplicateDecision);
        }
        let entry_ids = report
            .entries
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect::<BTreeSet<_>>();
        if decision_ids != entry_ids {
            return Err(ReconciliationError::IncompleteDecisionSet);
        }

        let by_id = report
            .entries
            .iter()
            .map(|entry| (entry.entry_id.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let actions = decisions
            .iter()
            .map(|decision| {
                let entry = by_id[decision.entry_id.as_str()];
                match decision.choice {
                    ReconciliationChoice::ImportIntoTakeGraph => {
                        ReconciliationAction::ProposeImportPatch {
                            entry_id: entry.entry_id.clone(),
                            identity: entry.identity.clone(),
                            observed: entry.actual.clone(),
                        }
                    }
                    ReconciliationChoice::DetachFromTakeGraph => {
                        ReconciliationAction::DetachManagedIdentity {
                            entry_id: entry.entry_id.clone(),
                            identity: entry.identity.clone(),
                            observed: entry.actual.clone(),
                        }
                    }
                    ReconciliationChoice::ReExportCanonical => {
                        ReconciliationAction::ReExportCanonicalState {
                            entry_id: entry.entry_id.clone(),
                            identity: entry.identity.clone(),
                            canonical: entry.expected.clone(),
                        }
                    }
                }
            })
            .collect::<Vec<_>>();
        let approval_digest = canonical_sha256(
            "takegraph-reconciliation-preview-v1",
            &(
                RECONCILIATION_SCHEMA_VERSION,
                &report.report_digest,
                &report.source,
                &decisions,
                &actions,
            ),
        )?;
        Ok(Self {
            schema_version: RECONCILIATION_SCHEMA_VERSION,
            report_digest: report.report_digest.clone(),
            source: report.source.clone(),
            decisions,
            actions,
            approval_digest,
        })
    }

    /// Rehashes a preview before its decisions are accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when any approval-bound field was edited.
    pub fn verify_approval_digest(&self, approved: &str) -> Result<(), ReconciliationError> {
        let actual = canonical_sha256(
            "takegraph-reconciliation-preview-v1",
            &(
                self.schema_version,
                &self.report_digest,
                &self.source,
                &self.decisions,
                &self.actions,
            ),
        )?;
        if actual != self.approval_digest || actual != approved {
            return Err(ReconciliationError::DigestMismatch {
                expected: approved.into(),
                actual,
            });
        }
        Ok(())
    }
}

fn group_by_identity(
    items: Vec<ManagedSemanticItem>,
) -> BTreeMap<ManagedSemanticIdentity, Vec<ManagedSemanticItem>> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for item in items {
        groups.entry(item.identity.clone()).or_default().push(item);
    }
    for group in groups.values_mut() {
        group.sort_by_key(|item| {
            (
                item.realization_kind.clone(),
                item.entity_revision,
                canonical_sha256("takegraph-managed-item-sort-v1", item).unwrap_or_default(),
            )
        });
    }
    groups
}

fn managed_state_digest(items: &[ManagedSemanticItem]) -> Result<String, CanonicalError> {
    let mut items = items.to_vec();
    items.sort_by_key(|item| {
        (
            item.identity.clone(),
            item.realization_kind.clone(),
            item.entity_revision,
            canonical_sha256("takegraph-managed-item-sort-v1", item).unwrap_or_default(),
        )
    });
    canonical_sha256("takegraph-managed-semantic-state-v1", &items)
}

/// A portable export intentionally realizes one semantic identity as an audio
/// item plus a caption item. Every other identity is one-to-one. Keeping this
/// rule in the adapter-independent comparison layer prevents the legal pair
/// from being mistaken for a duplicate while still failing closed on repeated
/// kinds and unrelated cross-kind collisions.
fn is_legal_identity_group(
    identity: &ManagedSemanticIdentity,
    items: &[ManagedSemanticItem],
) -> bool {
    if items.is_empty() {
        return true;
    }
    if identity.realization_id.is_some() {
        return items.len() == 1 && !is_portable_pair_kind(&items[0].realization_kind);
    }
    match items {
        [item] => is_portable_pair_kind(&item.realization_kind),
        [first, second] => matches!(
            (
                first.realization_kind.as_str(),
                second.realization_kind.as_str()
            ),
            ("portable_audio", "portable_caption") | ("portable_caption", "portable_audio")
        ),
        _ => false,
    }
}

fn is_portable_pair_kind(kind: &str) -> bool {
    matches!(kind, "portable_audio" | "portable_caption")
}

fn compare_identity_groups(
    expected: &[ManagedSemanticItem],
    actual: &[ManagedSemanticItem],
) -> Vec<ManagedFieldDrift> {
    let expected_by_kind = expected
        .iter()
        .map(|item| (item.realization_kind.as_str(), item))
        .collect::<BTreeMap<_, _>>();
    let actual_by_kind = actual
        .iter()
        .map(|item| (item.realization_kind.as_str(), item))
        .collect::<BTreeMap<_, _>>();
    let kinds = expected_by_kind
        .keys()
        .chain(actual_by_kind.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let qualify_fields =
        expected.len() > 1 || actual.len() > 1 || expected_by_kind.keys().ne(actual_by_kind.keys());
    let mut fields = Vec::new();
    for kind in kinds {
        match (expected_by_kind.get(kind), actual_by_kind.get(kind)) {
            (Some(expected), Some(actual)) => {
                fields.extend(
                    compare_fields(expected, actual)
                        .into_iter()
                        .map(|mut field| {
                            if qualify_fields {
                                field.field = format!("{kind}.{}", field.field);
                            }
                            field
                        }),
                );
            }
            (Some(_), None) => fields.push(ManagedFieldDrift {
                field: format!("{kind}.$present"),
                expected: Some(ManagedSemanticValue::Boolean(true)),
                actual: None,
            }),
            (None, Some(_)) => fields.push(ManagedFieldDrift {
                field: format!("{kind}.$present"),
                expected: None,
                actual: Some(ManagedSemanticValue::Boolean(true)),
            }),
            (None, None) => unreachable!("kind came from one of the maps"),
        }
    }
    fields
}

fn compare_fields(
    expected: &ManagedSemanticItem,
    actual: &ManagedSemanticItem,
) -> Vec<ManagedFieldDrift> {
    let mut expected_fields = expected.owned_fields.clone();
    let mut actual_fields = actual.owned_fields.clone();
    expected_fields.insert(
        "$entityRevision".into(),
        ManagedSemanticValue::Integer(i64::try_from(expected.entity_revision).unwrap_or(i64::MAX)),
    );
    actual_fields.insert(
        "$entityRevision".into(),
        ManagedSemanticValue::Integer(i64::try_from(actual.entity_revision).unwrap_or(i64::MAX)),
    );
    expected_fields.insert(
        "$realizationKind".into(),
        ManagedSemanticValue::Text(expected.realization_kind.clone()),
    );
    actual_fields.insert(
        "$realizationKind".into(),
        ManagedSemanticValue::Text(actual.realization_kind.clone()),
    );
    expected_fields
        .keys()
        .chain(actual_fields.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|field| {
            let expected = expected_fields.get(&field).cloned();
            let actual = actual_fields.get(&field).cloned();
            (expected != actual).then_some(ManagedFieldDrift {
                field,
                expected,
                actual,
            })
        })
        .collect()
}

#[derive(Debug, Error)]
pub enum ReconciliationError {
    #[error("required reconciliation field is empty: {0}")]
    EmptyField(&'static str),
    #[error("reconciliation mutation feature version must be positive")]
    InvalidMutationFeatureVersion,
    #[error("reconciliation payload digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error("a reconciliation entry has more than one decision")]
    DuplicateDecision,
    #[error("reconciliation decisions must cover every report entry exactly once")]
    IncompleteDecisionSet,
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> ReconciliationSource {
        ReconciliationSource {
            source_revision: RevisionId(7),
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

    fn item(text: &str) -> ManagedSemanticItem {
        ManagedSemanticItem {
            identity: ManagedSemanticIdentity {
                entity_id: "utt-01".into(),
                realization_id: Some(Uuid::nil()),
            },
            entity_revision: 3,
            realization_kind: "ymm4_native_voice".into(),
            owned_fields: BTreeMap::from([(
                "displayText".into(),
                ManagedSemanticValue::Text(text.into()),
            )]),
        }
    }

    fn portable(kind: &str, text: &str) -> ManagedSemanticItem {
        ManagedSemanticItem {
            identity: ManagedSemanticIdentity {
                entity_id: "utt-01".into(),
                realization_id: None,
            },
            entity_revision: 3,
            realization_kind: kind.into(),
            owned_fields: BTreeMap::from([(
                "content".into(),
                ManagedSemanticValue::Text(text.into()),
            )]),
        }
    }

    #[test]
    fn reports_only_owned_semantic_changes() {
        let report =
            SemanticDriftReport::build(source(), vec![item("before")], vec![item("after")])
                .unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::ChangedOwnedFields
        );
        assert_eq!(report.entries[0].fields[0].field, "displayText");
        report.verify_digest().unwrap();
    }

    #[test]
    fn order_does_not_change_managed_state_digest() {
        let mut second = item("two");
        second.identity.entity_id = "utt-02".into();
        let left = SemanticDriftReport::build(
            source(),
            vec![item("one"), second.clone()],
            vec![second.clone(), item("one")],
        )
        .unwrap();
        assert!(left.entries.is_empty());
        assert_eq!(
            left.expected_managed_state_digest,
            left.actual_managed_state_digest
        );
    }

    #[test]
    fn matching_portable_audio_caption_pair_is_not_duplicate_drift() {
        let expected = vec![
            portable("portable_audio", "audio"),
            portable("portable_caption", "caption"),
        ];
        let report = SemanticDriftReport::build(source(), expected.clone(), expected).unwrap();
        assert!(report.entries.is_empty());
    }

    #[test]
    fn missing_portable_pair_member_is_an_owned_field_change() {
        let report = SemanticDriftReport::build(
            source(),
            vec![
                portable("portable_audio", "audio"),
                portable("portable_caption", "caption"),
            ],
            vec![portable("portable_audio", "audio")],
        )
        .unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::ChangedOwnedFields
        );
        assert_eq!(report.entries[0].fields.len(), 1);
        assert_eq!(
            report.entries[0].fields[0].field,
            "portable_caption.$present"
        );
    }

    #[test]
    fn changed_portable_pair_member_is_kind_qualified() {
        let report = SemanticDriftReport::build(
            source(),
            vec![
                portable("portable_audio", "before"),
                portable("portable_caption", "caption"),
            ],
            vec![
                portable("portable_audio", "after"),
                portable("portable_caption", "caption"),
            ],
        )
        .unwrap();
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::ChangedOwnedFields
        );
        assert_eq!(report.entries[0].fields[0].field, "portable_audio.content");
    }

    #[test]
    fn duplicate_portable_kind_is_duplicate_identity_drift() {
        let audio = portable("portable_audio", "audio");
        let report = SemanticDriftReport::build(
            source(),
            vec![audio.clone(), portable("portable_caption", "caption")],
            vec![audio.clone(), audio],
        )
        .unwrap();
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::DuplicateManagedIdentity
        );
    }

    #[test]
    fn unrelated_cross_kind_collision_is_duplicate_identity_drift() {
        let report = SemanticDriftReport::build(
            source(),
            vec![
                portable("portable_audio", "audio"),
                portable("ymm4_native_voice", "voice"),
            ],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::DuplicateManagedIdentity
        );
    }

    #[test]
    fn realized_identity_cannot_claim_a_portable_pair() {
        let mut audio = portable("portable_audio", "audio");
        let mut caption = portable("portable_caption", "caption");
        let realization_id = Some(Uuid::new_v4());
        audio.identity.realization_id = realization_id;
        caption.identity.realization_id = realization_id;
        let report =
            SemanticDriftReport::build(source(), vec![audio, caption], Vec::new()).unwrap();
        assert_eq!(
            report.entries[0].kind,
            SemanticDriftKind::DuplicateManagedIdentity
        );
    }

    #[test]
    fn preview_requires_explicit_choice_for_every_entry() {
        let report = SemanticDriftReport::build(source(), vec![item("a")], vec![]).unwrap();
        assert!(matches!(
            ReconciliationPreview::build(&report, vec![]),
            Err(ReconciliationError::IncompleteDecisionSet)
        ));
        let preview = ReconciliationPreview::build(
            &report,
            vec![ReconciliationDecision {
                entry_id: report.entries[0].entry_id.clone(),
                choice: ReconciliationChoice::ReExportCanonical,
            }],
        )
        .unwrap();
        preview
            .verify_approval_digest(&preview.approval_digest)
            .unwrap();
    }

    #[test]
    fn report_becomes_stale_when_source_revision_changes() {
        let report = SemanticDriftReport::build(source(), vec![], vec![]).unwrap();
        let mut current = source();
        current.source_revision = RevisionId(8);
        assert!(report.is_stale_against(&current));
    }
}
