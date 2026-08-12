use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use takegraph_core::{CanonicalError, canonical_sha256};
use thiserror::Error;

use crate::ymm4::{Ymm4Capabilities, Ymm4Capability, Ymm4Health};

/// Structured, digestable view of the bridge's transitional string feature list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredYmm4Capabilities {
    pub protocol: ProtocolContract,
    pub driver: DriverDescriptor,
    pub features: BTreeMap<String, FeatureDescriptor>,
    pub capability_digest: String,
}

impl StructuredYmm4Capabilities {
    /// Normalizes bridge health and string features into the v2 contract used
    /// by target-plan approval.
    ///
    /// # Errors
    ///
    /// Returns an error for inconsistent protocol reports or canonicalization.
    pub fn from_bridge(
        health: &Ymm4Health,
        raw: &Ymm4Capabilities,
    ) -> Result<Self, StructuredCapabilityError> {
        if health.protocol_version != raw.protocol_version {
            return Err(StructuredCapabilityError::InconsistentProtocol {
                health: health.protocol_version,
                capabilities: raw.protocol_version,
            });
        }

        let mutation_tested = raw
            .capabilities
            .contains(&Ymm4Capability::MutationProfileYmm4_4_55_1_1);
        let features = normalized_features(raw)?;

        let protocol = ProtocolContract {
            major: raw.protocol_version,
            min_minor: 0,
            max_minor: 0,
        };
        let driver = DriverDescriptor {
            id: if mutation_tested {
                "ymm4-4.55".into()
            } else {
                "ymm4-observation-only".into()
            },
            plugin_version: health.plugin_version.clone(),
            ymm4_version: health.ymm4_version.clone(),
            mutation_status: if mutation_tested {
                MutationStatus::Tested
            } else {
                MutationStatus::ObservationOnly
            },
        };
        let capability_digest = canonical_sha256(
            "takegraph-ymm4-capabilities",
            &CapabilityDigestPayload {
                protocol: &protocol,
                driver: &driver,
                features: &features,
            },
        )?;
        Ok(Self {
            protocol,
            driver,
            features,
            capability_digest,
        })
    }

    #[must_use]
    pub fn feature(&self, name: &str) -> Option<&FeatureDescriptor> {
        self.features.get(name)
    }

    /// Ensures every plan dependency is present with its approved schema.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/disabled feature, an old version, or a
    /// schema digest that differs from the staged plan.
    pub fn require(
        &self,
        requirements: &[CapabilityRequirement],
    ) -> Result<(), StructuredCapabilityError> {
        for requirement in requirements {
            let feature = self
                .features
                .get(&requirement.feature)
                .filter(|feature| feature.available)
                .ok_or_else(|| {
                    StructuredCapabilityError::MissingFeature(requirement.feature.clone())
                })?;
            if feature.version < requirement.minimum_version {
                return Err(StructuredCapabilityError::FeatureVersion {
                    feature: requirement.feature.clone(),
                    required: requirement.minimum_version,
                    actual: feature.version,
                });
            }
            if let Some(expected) = &requirement.schema_digest
                && expected != &feature.schema_digest
            {
                return Err(StructuredCapabilityError::SchemaDrift {
                    feature: requirement.feature.clone(),
                    expected: expected.clone(),
                    actual: feature.schema_digest.clone(),
                });
            }
        }
        Ok(())
    }
}

fn normalized_features(
    raw: &Ymm4Capabilities,
) -> Result<BTreeMap<String, FeatureDescriptor>, CanonicalError> {
    let has = |capability| raw.capabilities.contains(&capability);
    let mut features = BTreeMap::new();
    insert_feature(
        &mut features,
        "managedPair.apply",
        has(Ymm4Capability::ManagedAudio) && has(Ymm4Capability::ManagedCaption),
        [
            (
                "identityCarrier",
                CapabilityValue::Text("caption_marker".into()),
            ),
            ("semanticItemCount", CapabilityValue::Integer(2)),
        ],
    )?;
    insert_feature(
        &mut features,
        "targetPlan.apply",
        has(Ymm4Capability::UnifiedTargetPlan),
        [
            ("canonicalVersion", CapabilityValue::Integer(1)),
            ("resolvedPlacement", CapabilityValue::Boolean(true)),
            ("resolvedBindings", CapabilityValue::Boolean(true)),
            ("mixedStrategies", CapabilityValue::Boolean(false)),
        ],
    )?;
    insert_feature(
        &mut features,
        "managedIdentity.detach",
        has(Ymm4Capability::MetadataRemarkDetach)
            && has(Ymm4Capability::RequestBoundReceipts)
            && has(Ymm4Capability::WriteAheadApply)
            && has(Ymm4Capability::RecoveryReadback),
        [
            ("identityCarrier", CapabilityValue::Text("remark".into())),
            (
                "mutationScope",
                CapabilityValue::Text("metadata_only".into()),
            ),
            ("projectScopedIdentity", CapabilityValue::Boolean(true)),
            ("freshReadback", CapabilityValue::Boolean(true)),
            ("nonRemarkDigestPreserved", CapabilityValue::Boolean(true)),
            ("notStartedTombstone", CapabilityValue::Boolean(true)),
        ],
    )?;
    insert_voice_features(&mut features, raw)?;
    insert_transaction_feature(&mut features, raw)?;
    insert_scene_capture_feature(&mut features, raw)?;
    insert_native_extension_features(&mut features, raw)?;
    insert_feature(
        &mut features,
        "readback.semantic",
        has(Ymm4Capability::ReadbackVerification),
        std::iter::empty(),
    )?;
    insert_project_output_features(&mut features, raw)?;
    Ok(features)
}

fn insert_project_output_features(
    features: &mut BTreeMap<String, FeatureDescriptor>,
    raw: &Ymm4Capabilities,
) -> Result<(), CanonicalError> {
    let has = |capability| raw.capabilities.contains(&capability);
    insert_feature(
        features,
        "project.checkpoint",
        has(Ymm4Capability::ProjectCheckpointVerified),
        [
            ("existingPathOnly", CapabilityValue::Boolean(true)),
            ("fileHash", CapabilityValue::Text("sha256".into())),
            ("canonicalRevisionAdvances", CapabilityValue::Boolean(false)),
        ],
    )?;
    insert_feature(
        features,
        "project.render",
        has(Ymm4Capability::ProjectRender)
            && has(Ymm4Capability::ProjectRenderCancel)
            && has(Ymm4Capability::ProjectRenderMediaReceipt),
        [
            ("explicitOutputPath", CapabilityValue::Boolean(true)),
            ("explicitOverwritePolicy", CapabilityValue::Boolean(true)),
            ("cancellation", CapabilityValue::Boolean(true)),
            ("verifiedCheckpointRequired", CapabilityValue::Boolean(true)),
            (
                "immutableCheckpointSnapshot",
                CapabilityValue::Boolean(true),
            ),
            (
                "replaceExistingWriteAheadLog",
                CapabilityValue::Boolean(true),
            ),
            ("exactEncoderProfileBinding", CapabilityValue::Boolean(true)),
            (
                "finalMediaReceipt",
                CapabilityValue::Text("sha256_and_probe".into()),
            ),
            ("canonicalRevisionAdvances", CapabilityValue::Boolean(false)),
        ],
    )
}

fn insert_native_extension_features(
    features: &mut BTreeMap<String, FeatureDescriptor>,
    raw: &Ymm4Capabilities,
) -> Result<(), CanonicalError> {
    let has = |capability| raw.capabilities.contains(&capability);
    let definitions = [
        (
            "portraitItem.upsert",
            has(Ymm4Capability::NativePortraitUpsert),
            "portrait",
        ),
        (
            "faceItem.upsert",
            has(Ymm4Capability::NativeFaceUpsert),
            "face",
        ),
        (
            "imageItem.upsert",
            has(Ymm4Capability::NativeImageUpsert),
            "image",
        ),
        (
            "videoItem.upsert",
            has(Ymm4Capability::NativeVideoUpsert),
            "video",
        ),
        (
            "audioItem.upsert",
            has(Ymm4Capability::NativeAudioUpsert),
            "audio_or_bgm",
        ),
        (
            "effect.typedMutation",
            has(Ymm4Capability::NativeEffectTypedMutation),
            "typed_effect",
        ),
        (
            "template.instantiate",
            has(Ymm4Capability::NativeTemplateInstantiate),
            "native_template",
        ),
    ];
    for (feature, available, driver_operation) in definitions {
        insert_feature(
            features,
            feature,
            available,
            [
                (
                    "driverOperation",
                    CapabilityValue::Text(driver_operation.into()),
                ),
                ("identityCarrier", CapabilityValue::Text("remark".into())),
                ("unknownEffectsPreserved", CapabilityValue::Boolean(true)),
                ("exactLossAllowlist", CapabilityValue::Boolean(true)),
            ],
        )?;
    }
    Ok(())
}

fn insert_voice_features(
    features: &mut BTreeMap<String, FeatureDescriptor>,
    raw: &Ymm4Capabilities,
) -> Result<(), CanonicalError> {
    let has = |capability| raw.capabilities.contains(&capability);
    let identity = has(Ymm4Capability::NativeVoiceRemarkIdentity);
    let bounded = has(Ymm4Capability::NativeVoiceBoundedDuration);
    let exact_wav = has(Ymm4Capability::NativeVoiceExactWavExport);
    let host_bound_provenance = has(Ymm4Capability::NativeVoiceHostBoundProvenance);
    insert_feature(
        features,
        "voiceItem.create",
        has(Ymm4Capability::NativeVoiceCreate) && identity && bounded,
        [
            ("artifactCapture", CapabilityValue::Boolean(false)),
            (
                "artifactExportAvailable",
                CapabilityValue::Boolean(exact_wav && host_bound_provenance),
            ),
            ("identityCarrier", CapabilityValue::Text("remark".into())),
            ("prepare", CapabilityValue::Boolean(false)),
            (
                "separateDisplayAndSpokenText",
                CapabilityValue::Boolean(false),
            ),
            (
                "durationResolution",
                CapabilityValue::Text("bounded".into()),
            ),
        ],
    )?;
    insert_feature(
        features,
        "voiceItem.update",
        has(Ymm4Capability::NativeVoiceUpdateReplacePreservingUserState) && identity && bounded,
        [
            (
                "mutationMode",
                CapabilityValue::Text("replace_preserving_user_state".into()),
            ),
            ("preservedStateVerified", CapabilityValue::Boolean(true)),
            ("identityCarrier", CapabilityValue::Text("remark".into())),
            (
                "separateDisplayAndSpokenText",
                CapabilityValue::Boolean(false),
            ),
            (
                "durationResolution",
                CapabilityValue::Text("bounded".into()),
            ),
        ],
    )?;
    insert_feature(
        features,
        "voiceItem.delete",
        has(Ymm4Capability::NativeVoiceDelete) && identity,
        [
            ("identityCarrier", CapabilityValue::Text("remark".into())),
            (
                "deleteReadback",
                CapabilityValue::Text("realization_absent".into()),
            ),
        ],
    )?;
    insert_feature(
        features,
        "voiceItem.artifactExport",
        exact_wav && host_bound_provenance,
        [
            ("audio", CapabilityValue::Text("exact_wav".into())),
            (
                "provenance",
                CapabilityValue::Text("normalized_host_bound_voice_state".into()),
            ),
            ("portableSynthesisQuery", CapabilityValue::Boolean(false)),
            ("contentHash", CapabilityValue::Text("sha256".into())),
        ],
    )
}

fn insert_transaction_feature(
    features: &mut BTreeMap<String, FeatureDescriptor>,
    raw: &Ymm4Capabilities,
) -> Result<(), CanonicalError> {
    let has = |capability| raw.capabilities.contains(&capability);
    insert_feature(
        features,
        "timeline.transaction",
        has(Ymm4Capability::IdempotentApply)
            && has(Ymm4Capability::RequestBoundReceipts)
            && has(Ymm4Capability::WriteAheadApply),
        [
            (
                "recoveryReadback",
                CapabilityValue::Boolean(has(Ymm4Capability::RecoveryReadback)),
            ),
            ("durableRollback", CapabilityValue::Boolean(false)),
            (
                "undoBatch",
                CapabilityValue::Boolean(has(Ymm4Capability::UndoBatch)),
            ),
        ],
    )
}

fn insert_scene_capture_feature(
    features: &mut BTreeMap<String, FeatureDescriptor>,
    raw: &Ymm4Capabilities,
) -> Result<(), CanonicalError> {
    let has = |capability| raw.capabilities.contains(&capability);
    insert_feature(
        features,
        "scene.capture",
        has(Ymm4Capability::SceneCaptureNativePng)
            && has(Ymm4Capability::SceneCapturePlayheadRestore)
            && has(Ymm4Capability::SceneCaptureContentHash),
        [
            ("mediaType", CapabilityValue::Text("image/png".into())),
            ("transientStateRestore", CapabilityValue::Boolean(true)),
            ("contentHash", CapabilityValue::Text("sha256".into())),
            ("maxFrames", CapabilityValue::Integer(64)),
        ],
    )
}

fn insert_feature<I>(
    features: &mut BTreeMap<String, FeatureDescriptor>,
    name: &str,
    available: bool,
    properties: I,
) -> Result<(), CanonicalError>
where
    I: IntoIterator<Item = (&'static str, CapabilityValue)>,
{
    let properties = properties
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect::<BTreeMap<_, _>>();
    let schema_digest = canonical_sha256(
        "takegraph-ymm4-feature-schema",
        &FeatureSchema {
            name,
            version: 1,
            property_names: properties.keys().collect::<Vec<_>>(),
        },
    )?;
    features.insert(
        name.into(),
        FeatureDescriptor {
            version: 1,
            available,
            schema_digest,
            properties,
        },
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolContract {
    pub major: u32,
    pub min_minor: u32,
    pub max_minor: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverDescriptor {
    pub id: String,
    pub plugin_version: String,
    pub ymm4_version: String,
    pub mutation_status: MutationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationStatus {
    Tested,
    ObservationOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureDescriptor {
    pub version: u32,
    pub available: bool,
    pub schema_digest: String,
    pub properties: BTreeMap<String, CapabilityValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CapabilityValue {
    Boolean(bool),
    Integer(i64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequirement {
    pub feature: String,
    pub minimum_version: u32,
    pub schema_digest: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityDigestPayload<'a> {
    protocol: &'a ProtocolContract,
    driver: &'a DriverDescriptor,
    features: &'a BTreeMap<String, FeatureDescriptor>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureSchema<'a> {
    name: &'a str,
    version: u32,
    property_names: Vec<&'a String>,
}

#[derive(Debug, Error)]
pub enum StructuredCapabilityError {
    #[error("YMM4 bridge protocol reports disagree: health {health}, capabilities {capabilities}")]
    InconsistentProtocol { health: u32, capabilities: u32 },
    #[error("required YMM4 feature is unavailable: {0}")]
    MissingFeature(String),
    #[error("YMM4 feature {feature} requires version {required}, got {actual}")]
    FeatureVersion {
        feature: String,
        required: u32,
        actual: u32,
    },
    #[error("YMM4 feature schema changed for {feature}: expected {expected}, got {actual}")]
    SchemaDrift {
        feature: String,
        expected: String,
        actual: String,
    },
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health() -> Ymm4Health {
        Ymm4Health {
            status: "running".into(),
            protocol_version: 2,
            plugin_version: "0.2.0".into(),
            ymm4_version: "4.55.1.1".into(),
        }
    }

    #[test]
    fn normalization_is_order_independent_and_truthful() {
        let raw = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![
                Ymm4Capability::NativeVoiceCreate,
                Ymm4Capability::NativeVoiceUpdateReplacePreservingUserState,
                Ymm4Capability::NativeVoiceDelete,
                Ymm4Capability::NativeVoiceExactWavExport,
                Ymm4Capability::NativeVoiceHostBoundProvenance,
                Ymm4Capability::NativeVoiceRemarkIdentity,
                Ymm4Capability::NativeVoiceBoundedDuration,
                Ymm4Capability::ManagedCaption,
                Ymm4Capability::ManagedAudio,
                Ymm4Capability::UnifiedTargetPlan,
                Ymm4Capability::MutationProfileYmm4_4_55_1_1,
                Ymm4Capability::SceneCaptureNativePng,
                Ymm4Capability::SceneCapturePlayheadRestore,
                Ymm4Capability::SceneCaptureContentHash,
                Ymm4Capability::MetadataRemarkDetach,
            ],
        };
        let mut reversed = raw.clone();
        reversed.capabilities.reverse();
        let left = StructuredYmm4Capabilities::from_bridge(&health(), &raw).unwrap();
        let right = StructuredYmm4Capabilities::from_bridge(&health(), &reversed).unwrap();

        assert_eq!(left.capability_digest, right.capability_digest);
        let voice = left.feature("voiceItem.create").unwrap();
        assert!(voice.available);
        assert_eq!(
            voice.properties["artifactCapture"],
            CapabilityValue::Boolean(false)
        );
        assert_eq!(
            voice.properties["separateDisplayAndSpokenText"],
            CapabilityValue::Boolean(false)
        );
        assert_eq!(
            voice.properties["artifactExportAvailable"],
            CapabilityValue::Boolean(true)
        );
        assert!(left.feature("voiceItem.update").unwrap().available);
        assert!(left.feature("voiceItem.delete").unwrap().available);
        let artifact = left.feature("voiceItem.artifactExport").unwrap();
        assert!(artifact.available);
        assert_eq!(
            artifact.properties["portableSynthesisQuery"],
            CapabilityValue::Boolean(false)
        );
        let capture = left.feature("scene.capture").unwrap();
        assert!(capture.available);
        assert_eq!(
            capture.properties["transientStateRestore"],
            CapabilityValue::Boolean(true)
        );
    }

    #[test]
    fn driver_version_changes_capability_digest() {
        let raw = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![],
        };
        let first = StructuredYmm4Capabilities::from_bridge(&health(), &raw).unwrap();
        let mut changed = health();
        changed.plugin_version = "0.2.1".into();
        let second = StructuredYmm4Capabilities::from_bridge(&changed, &raw).unwrap();
        assert_ne!(first.capability_digest, second.capability_digest);
    }

    #[test]
    fn native_voice_requires_create_identity_and_duration_contracts() {
        let raw = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![Ymm4Capability::NativeVoiceCreate],
        };
        let capabilities = StructuredYmm4Capabilities::from_bridge(&health(), &raw).unwrap();
        assert!(!capabilities.feature("voiceItem.create").unwrap().available);
    }

    #[test]
    fn native_voice_mutation_and_artifact_features_fail_closed_independently() {
        let raw = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![
                Ymm4Capability::NativeVoiceUpdateReplacePreservingUserState,
                Ymm4Capability::NativeVoiceDelete,
                Ymm4Capability::NativeVoiceExactWavExport,
            ],
        };
        let capabilities = StructuredYmm4Capabilities::from_bridge(&health(), &raw).unwrap();
        assert!(!capabilities.feature("voiceItem.update").unwrap().available);
        assert!(!capabilities.feature("voiceItem.delete").unwrap().available);
        assert!(
            !capabilities
                .feature("voiceItem.artifactExport")
                .unwrap()
                .available
        );
    }

    #[test]
    fn required_feature_binds_schema_digest() {
        let raw = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![
                Ymm4Capability::NativeVoiceCreate,
                Ymm4Capability::NativeVoiceRemarkIdentity,
                Ymm4Capability::NativeVoiceBoundedDuration,
            ],
        };
        let capabilities = StructuredYmm4Capabilities::from_bridge(&health(), &raw).unwrap();
        let feature = capabilities.feature("voiceItem.create").unwrap();
        capabilities
            .require(&[CapabilityRequirement {
                feature: "voiceItem.create".into(),
                minimum_version: 1,
                schema_digest: Some(feature.schema_digest.clone()),
            }])
            .unwrap();
        assert!(matches!(
            capabilities.require(&[CapabilityRequirement {
                feature: "voiceItem.create".into(),
                minimum_version: 1,
                schema_digest: Some("sha256:changed".into()),
            }]),
            Err(StructuredCapabilityError::SchemaDrift { .. })
        ));
    }

    #[test]
    fn checkpoint_and_render_require_complete_tier_two_capabilities() {
        let partial = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![
                Ymm4Capability::ProjectCheckpointVerified,
                Ymm4Capability::ProjectRender,
            ],
        };
        let partial = StructuredYmm4Capabilities::from_bridge(&health(), &partial).unwrap();
        assert!(partial.feature("project.checkpoint").unwrap().available);
        assert!(!partial.feature("project.render").unwrap().available);

        let complete = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![
                Ymm4Capability::ProjectCheckpointVerified,
                Ymm4Capability::ProjectRender,
                Ymm4Capability::ProjectRenderCancel,
                Ymm4Capability::ProjectRenderMediaReceipt,
            ],
        };
        let complete = StructuredYmm4Capabilities::from_bridge(&health(), &complete).unwrap();
        assert!(complete.feature("project.render").unwrap().available);
        assert_eq!(
            complete.feature("project.render").unwrap().properties["canonicalRevisionAdvances"],
            CapabilityValue::Boolean(false)
        );
    }

    #[test]
    fn full_bridge_fixture_has_cross_runtime_capability_digest() {
        let raw = Ymm4Capabilities {
            protocol_version: 2,
            capabilities: vec![
                Ymm4Capability::ReadbackVerification,
                Ymm4Capability::RequestBoundReceipts,
                Ymm4Capability::WriteAheadApply,
                Ymm4Capability::RecoveryReadback,
                Ymm4Capability::ManagedAudio,
                Ymm4Capability::ManagedCaption,
                Ymm4Capability::UnifiedTargetPlan,
                Ymm4Capability::IdempotentApply,
                Ymm4Capability::UndoBatch,
                Ymm4Capability::MutationProfileYmm4_4_55_1_1,
                Ymm4Capability::ProjectCheckpointVerified,
                Ymm4Capability::NativePortraitUpsert,
                Ymm4Capability::NativeFaceUpsert,
                Ymm4Capability::NativeImageUpsert,
                Ymm4Capability::NativeVideoUpsert,
                Ymm4Capability::NativeAudioUpsert,
                Ymm4Capability::NativeEffectTypedMutation,
                Ymm4Capability::NativeTemplateInstantiate,
                Ymm4Capability::NativeVoiceCreate,
                Ymm4Capability::NativeVoiceUpdateReplacePreservingUserState,
                Ymm4Capability::NativeVoiceDelete,
                Ymm4Capability::NativeVoiceExactWavExport,
                Ymm4Capability::NativeVoiceHostBoundProvenance,
                Ymm4Capability::NativeVoiceRemarkIdentity,
                Ymm4Capability::NativeVoiceBoundedDuration,
                Ymm4Capability::SceneCaptureNativePng,
                Ymm4Capability::SceneCapturePlayheadRestore,
                Ymm4Capability::SceneCaptureContentHash,
                Ymm4Capability::MetadataRemarkDetach,
            ],
        };
        let capabilities = StructuredYmm4Capabilities::from_bridge(&health(), &raw).unwrap();
        assert_eq!(
            capabilities.capability_digest,
            "sha256:8996754be297a75298d9f3b9a0650ba8de7ea0053087546aa2c55a38f8493038"
        );
    }
}
