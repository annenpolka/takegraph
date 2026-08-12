//! Authenticated YMM4 Phase-4 native-extension transport and artifact boundary.
//!
//! The portable plan is path-free. Paths exist only in the node-owned bridge
//! request, after the source bytes have been copied to a content-addressed
//! artifact root and rehashed.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use takegraph_core::{
    AssetKind, CharacterDescriptor, EffectDescriptor, NativeDescriptorCatalog,
    NativeExtensionIntent, NativeExtensionPlan, PortraitPresentation, TemplateDescriptor,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{YMM4_BRIDGE_PROTOCOL_VERSION, Ymm4BridgeClient, Ymm4Error};

/// Existing bridge descriptor DTO (`TargetDescriptorDto`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4TargetDescriptor {
    pub descriptor_id: String,
    pub kind: String,
    pub name: String,
    pub config_digest: String,
    pub schema_digest: String,
    pub bindable: bool,
    pub mutation_allowed: bool,
    pub metadata: BTreeMap<String, String>,
}

/// Existing bridge descriptor catalog DTO (`DescriptorCatalogDto`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4DescriptorCatalog {
    pub protocol_version: u32,
    pub project_id: String,
    pub scene_id: String,
    pub driver_profile_digest: String,
    pub catalog_digest: String,
    pub descriptors: Vec<Ymm4TargetDescriptor>,
}

impl Ymm4DescriptorCatalog {
    /// Returns the portable digest callers put in `DescriptorReference` after
    /// choosing a target descriptor by stable ID and raw config/schema digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the target catalog cannot be normalized safely.
    pub fn planning_descriptor_digests(
        &self,
    ) -> Result<BTreeMap<String, String>, NativeExtensionNodeError> {
        let catalog = self.planning_catalog()?;
        let mut digests = BTreeMap::new();
        for (id, descriptor) in catalog.characters {
            digests.insert(
                id,
                descriptor
                    .canonical_digest()
                    .map_err(|error| NativeExtensionNodeError::InvalidCatalog(error.to_string()))?,
            );
        }
        for (id, descriptor) in catalog.templates {
            digests.insert(
                id,
                descriptor
                    .canonical_digest()
                    .map_err(|error| NativeExtensionNodeError::InvalidCatalog(error.to_string()))?,
            );
        }
        for (id, descriptor) in catalog.effects {
            digests.insert(
                id,
                descriptor
                    .canonical_digest()
                    .map_err(|error| NativeExtensionNodeError::InvalidCatalog(error.to_string()))?,
            );
        }
        Ok(digests)
    }

    /// Converts the target catalog into the portable planner's descriptor
    /// shape. The original catalog remains approval-bound separately because
    /// it contains target schema/config digests that have no portable analogue.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed/duplicate target descriptors.
    pub fn planning_catalog(&self) -> Result<NativeDescriptorCatalog, NativeExtensionNodeError> {
        self.validate()?;
        let mut result = NativeDescriptorCatalog::default();
        for descriptor in &self.descriptors {
            if !descriptor.bindable {
                continue;
            }
            match descriptor.kind.as_str() {
                "character" => {
                    let mut configuration = descriptor
                        .metadata
                        .iter()
                        .filter(|(key, value)| !key.trim().is_empty() && !value.trim().is_empty())
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<BTreeMap<_, _>>();
                    configuration.insert(
                        "takegraph.targetConfigDigest".into(),
                        prefixed_sha256(&descriptor.config_digest)?,
                    );
                    configuration.insert(
                        "takegraph.targetSchemaDigest".into(),
                        prefixed_sha256(&descriptor.schema_digest)?,
                    );
                    result.characters.insert(
                        descriptor.descriptor_id.clone(),
                        CharacterDescriptor {
                            descriptor_id: descriptor.descriptor_id.clone(),
                            display_name: descriptor.name.clone(),
                            supported_presentations: BTreeSet::from([
                                PortraitPresentation::Portrait,
                                PortraitPresentation::Face,
                            ]),
                            configuration,
                        },
                    );
                }
                "template" => {
                    let produced_item_kinds = descriptor
                        .metadata
                        .get("itemTypes")
                        .into_iter()
                        .flat_map(|value| value.lines())
                        .filter(|value| !value.trim().is_empty())
                        .map(ToOwned::to_owned)
                        .collect::<BTreeSet<_>>();
                    result.templates.insert(
                        descriptor.descriptor_id.clone(),
                        TemplateDescriptor {
                            descriptor_id: descriptor.descriptor_id.clone(),
                            display_name: descriptor.name.clone(),
                            // Bind both target configuration and schema. Native
                            // template content is opaque to the portable core.
                            content_digest: target_descriptor_digest(descriptor)?,
                            produced_item_kinds: if produced_item_kinds.is_empty() {
                                BTreeSet::from(["opaque-native-item".into()])
                            } else {
                                produced_item_kinds
                            },
                        },
                    );
                }
                "video-effect" | "audio-effect" => {
                    if !descriptor.mutation_allowed {
                        continue;
                    }
                    // The first allowlisted YMM4 effect is parameterless. A
                    // future writable effect must expose a typed schema in the
                    // bridge DTO before parameters can be accepted here.
                    result.effects.insert(
                        descriptor.descriptor_id.clone(),
                        EffectDescriptor {
                            descriptor_id: descriptor.descriptor_id.clone(),
                            stable_type_id: format!(
                                "{}#{}",
                                descriptor
                                    .metadata
                                    .get("type")
                                    .map_or("unknown", String::as_str),
                                target_descriptor_digest(descriptor)?
                            ),
                            display_name: descriptor.name.clone(),
                            schema_version: 1,
                            parameters: BTreeMap::new(),
                        },
                    );
                }
                _ => {}
            }
        }
        result
            .validate()
            .map_err(|error| NativeExtensionNodeError::InvalidCatalog(error.to_string()))?;
        Ok(result)
    }

    /// Validates identity, protocol, digest syntax, and descriptor uniqueness.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog is not safe to approval-bind.
    pub fn validate(&self) -> Result<(), NativeExtensionNodeError> {
        if self.protocol_version != YMM4_BRIDGE_PROTOCOL_VERSION {
            return Err(NativeExtensionNodeError::ProtocolMismatch {
                expected: YMM4_BRIDGE_PROTOCOL_VERSION,
                actual: self.protocol_version,
            });
        }
        require_non_empty(&self.project_id, "projectId")?;
        require_non_empty(&self.scene_id, "sceneId")?;
        require_raw_sha256(&self.driver_profile_digest, "driverProfileDigest")?;
        require_raw_sha256(&self.catalog_digest, "catalogDigest")?;
        let mut ids = BTreeSet::new();
        for descriptor in &self.descriptors {
            require_non_empty(&descriptor.descriptor_id, "descriptorId")?;
            require_non_empty(&descriptor.kind, "descriptor.kind")?;
            require_non_empty(&descriptor.name, "descriptor.name")?;
            require_raw_sha256(&descriptor.config_digest, "descriptor.configDigest")?;
            require_raw_sha256(&descriptor.schema_digest, "descriptor.schemaDigest")?;
            if !ids.insert(&descriptor.descriptor_id) {
                return Err(NativeExtensionNodeError::DuplicateDescriptor(
                    descriptor.descriptor_id.clone(),
                ));
            }
        }
        Ok(())
    }

    /// Requires the complete target catalog, including every config/schema
    /// digest, to remain byte-for-byte equivalent to the staged view.
    ///
    /// # Errors
    ///
    /// Returns a drift error if project, scene, driver, catalog, descriptor
    /// configuration, schema, or mutation policy changed.
    pub fn require_unchanged(&self, current: &Self) -> Result<(), NativeExtensionNodeError> {
        self.validate()?;
        current.validate()?;
        if self != current {
            return Err(NativeExtensionNodeError::DescriptorDrift {
                expected: self.catalog_digest.clone(),
                actual: current.catalog_digest.clone(),
            });
        }
        Ok(())
    }
}

/// Content-addressed file mapping used only by the local YMM4 driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionArtifact {
    pub artifact_digest: String,
    pub media_type: String,
    pub byte_length: u64,
    pub kind: AssetKind,
    pub path: PathBuf,
    pub sha256: String,
}

impl Ymm4NativeExtensionArtifact {
    /// Rehashes the file and verifies it remains inside the authorized artifact
    /// root with exactly the staged length, digest, media type, and kind.
    ///
    /// # Errors
    ///
    /// Returns an error for path escape, missing/changed bytes, or an invalid
    /// semantic media declaration.
    pub fn verify(&self, artifact_root: &Path) -> Result<(), NativeExtensionNodeError> {
        let root = fs::canonicalize(artifact_root)?;
        let path = fs::canonicalize(&self.path)?;
        if !path.starts_with(&root) {
            return Err(NativeExtensionNodeError::ArtifactPathEscape(path));
        }
        let (digest, bytes) = hash_file(&path)?;
        if digest != self.artifact_digest || digest != self.sha256 {
            return Err(NativeExtensionNodeError::ArtifactDigestMismatch {
                expected: self.artifact_digest.clone(),
                actual: digest,
            });
        }
        if bytes != self.byte_length {
            return Err(NativeExtensionNodeError::ArtifactLengthMismatch {
                expected: self.byte_length,
                actual: bytes,
            });
        }
        validate_media_kind(self.kind, &self.media_type)?;
        Ok(())
    }
}

/// Copies and rehashes an immutable source file into a node-owned artifact root.
/// Existing content-addressed files are always reverified before reuse.
///
/// # Errors
///
/// Returns an error for unreadable bytes, claimed hash/length mismatch, unsafe
/// artifact paths, or invalid media-kind declarations.
pub fn materialize_native_extension_artifact(
    source: &Path,
    reference: &takegraph_core::ImmutableAssetReference,
    artifact_root: &Path,
) -> Result<Ymm4NativeExtensionArtifact, NativeExtensionNodeError> {
    reference
        .validate()
        .map_err(|error| NativeExtensionNodeError::InvalidArtifact(error.to_string()))?;
    let source = fs::canonicalize(source)?;
    if !source.is_file() {
        return Err(NativeExtensionNodeError::InvalidArtifact(format!(
            "source is not a regular file: {}",
            source.display()
        )));
    }
    let (actual_digest, actual_length) = hash_file(&source)?;
    if actual_digest != reference.artifact_digest {
        return Err(NativeExtensionNodeError::ArtifactDigestMismatch {
            expected: reference.artifact_digest.clone(),
            actual: actual_digest,
        });
    }
    if actual_length != reference.byte_length {
        return Err(NativeExtensionNodeError::ArtifactLengthMismatch {
            expected: reference.byte_length,
            actual: actual_length,
        });
    }

    let hex = reference
        .artifact_digest
        .strip_prefix("sha256:")
        .ok_or_else(|| NativeExtensionNodeError::InvalidDigest("artifactDigest".into()))?;
    let extension = extension_for_media_type(&reference.media_type);
    let directory = artifact_root.join("sha256").join(&hex[..2]);
    fs::create_dir_all(&directory)?;
    let destination = directory.join(format!("{hex}.{extension}"));
    if destination.exists() {
        let artifact = Ymm4NativeExtensionArtifact {
            artifact_digest: reference.artifact_digest.clone(),
            media_type: reference.media_type.clone(),
            byte_length: reference.byte_length,
            kind: reference.kind,
            path: destination,
            sha256: reference.artifact_digest.clone(),
        };
        artifact.verify(artifact_root)?;
        return Ok(artifact);
    }

    let temporary = directory.join(format!(".{hex}.{}.tmp", Uuid::new_v4()));
    copy_create_new(&source, &temporary)?;
    let temporary_artifact = Ymm4NativeExtensionArtifact {
        artifact_digest: reference.artifact_digest.clone(),
        media_type: reference.media_type.clone(),
        byte_length: reference.byte_length,
        kind: reference.kind,
        path: temporary.clone(),
        sha256: reference.artifact_digest.clone(),
    };
    temporary_artifact.verify(artifact_root)?;
    match fs::rename(&temporary, &destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(&temporary)?;
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
    }
    let artifact = Ymm4NativeExtensionArtifact {
        path: destination,
        ..temporary_artifact
    };
    artifact.verify(artifact_root)?;
    Ok(artifact)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionPlanRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub descriptor_catalog_digest: String,
    pub intents: Vec<NativeExtensionIntent>,
    pub artifacts: Vec<Ymm4NativeExtensionArtifact>,
}

impl Ymm4NativeExtensionPlanRequest {
    #[must_use]
    pub fn new(
        operation_id: Uuid,
        project_id: impl Into<String>,
        scene_id: impl Into<String>,
        expected_fingerprint: impl Into<String>,
        descriptor_catalog_digest: impl Into<String>,
        intents: Vec<NativeExtensionIntent>,
        artifacts: Vec<Ymm4NativeExtensionArtifact>,
    ) -> Self {
        Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id,
            project_id: project_id.into(),
            scene_id: scene_id.into(),
            expected_fingerprint: expected_fingerprint.into(),
            descriptor_catalog_digest: descriptor_catalog_digest.into(),
            intents,
            artifacts,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ymm4ExistingUpdateMode {
    InPlace,
    Replace { lossy_fields: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4ExistingNativeExtensionKind {
    Portrait,
    Face,
    Image,
    Video,
    Audio,
    Bgm,
    ManagedEffect,
    Template,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4PreservedNativeField {
    pub field: String,
    pub state_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4OpaqueNativeEffect {
    pub stable_type_id: String,
    pub instance_key: String,
    pub state_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4ExistingNativeExtension {
    pub logical_key: String,
    pub realization_id: Uuid,
    pub kind: Ymm4ExistingNativeExtensionKind,
    pub update_mode: Ymm4ExistingUpdateMode,
    pub preserved_fields: Vec<Ymm4PreservedNativeField>,
    pub unknown_effects: Vec<Ymm4OpaqueNativeEffect>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionObservation {
    pub existing: BTreeMap<String, Ymm4ExistingNativeExtension>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionPlanResponse {
    pub fingerprint: String,
    pub descriptor_catalog_digest: String,
    pub driver_profile_digest: String,
    pub observation: Ymm4NativeExtensionObservation,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionApplyRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub descriptor_catalog_digest: String,
    pub driver_profile_digest: String,
    pub plan_digest: String,
    pub plan: NativeExtensionPlan,
    pub artifacts: Vec<Ymm4NativeExtensionArtifact>,
}

impl Ymm4NativeExtensionApplyRequest {
    /// Builds a digest-bound request. The canonical plan digest binds all
    /// intent, descriptor, capability, preservation, unknown-effect, and exact
    /// lossy-approval fields.
    ///
    /// # Errors
    ///
    /// Returns an error when the core plan cannot be validated/digested.
    pub fn new(
        project_id: impl Into<String>,
        scene_id: impl Into<String>,
        expected_fingerprint: impl Into<String>,
        descriptor_catalog_digest: impl Into<String>,
        driver_profile_digest: impl Into<String>,
        plan: NativeExtensionPlan,
        artifacts: Vec<Ymm4NativeExtensionArtifact>,
    ) -> Result<Self, NativeExtensionNodeError> {
        let project_id = project_id.into();
        let scene_id = scene_id.into();
        let expected_fingerprint = expected_fingerprint.into();
        let descriptor_catalog_digest = descriptor_catalog_digest.into();
        let driver_profile_digest = driver_profile_digest.into();
        if plan.target.project_id != project_id || plan.target.scene_id != scene_id {
            return Err(NativeExtensionNodeError::InvalidPlan(
                "apply project/scene differs from the portable plan target".into(),
            ));
        }
        let plan_digest = plan
            .canonical_digest()
            .map_err(|error| NativeExtensionNodeError::InvalidPlan(error.to_string()))?;
        let request_digest = native_extension_apply_digest(&ApplyDigestInput {
            operation_id: plan.operation_id,
            project_id: &project_id,
            scene_id: &scene_id,
            expected_fingerprint: &expected_fingerprint,
            descriptor_catalog_digest: &descriptor_catalog_digest,
            driver_profile_digest: &driver_profile_digest,
            plan_digest: &plan_digest,
            artifacts: &artifacts,
        });
        Ok(Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id: plan.operation_id,
            request_digest,
            project_id,
            scene_id,
            expected_fingerprint,
            descriptor_catalog_digest,
            driver_profile_digest,
            plan_digest,
            plan,
            artifacts,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4NativeExtensionStatus {
    NotStarted,
    Applying,
    Verified,
    Stale,
    RolledBack,
    RecoveryRequired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionRealization {
    pub logical_key: String,
    pub realization_id: Uuid,
    pub kind: Ymm4ExistingNativeExtensionKind,
    pub project_id: String,
    pub entity_id: String,
    pub entity_revision: u64,
    pub frame: i32,
    pub layer: i32,
    pub length: i32,
    pub owned_state_digest: String,
    pub owned_fields: BTreeMap<String, String>,
    pub preserved_fields: Vec<Ymm4PreservedNativeField>,
    pub state_digest: String,
    pub unknown_effects: Vec<Ymm4OpaqueNativeEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4NativeExtensionApplyResponse {
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub status: Ymm4NativeExtensionStatus,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub descriptor_catalog_digest: String,
    pub driver_profile_digest: String,
    pub realizations: Vec<Ymm4NativeExtensionRealization>,
    pub verified: bool,
    pub error: Option<String>,
}

impl Ymm4BridgeClient {
    /// Reads the complete target descriptor catalog.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, or bridge response error.
    pub async fn native_descriptors(&self) -> Result<Ymm4DescriptorCatalog, Ymm4Error> {
        self.get_json("v2/descriptors").await
    }

    /// Obtains a read-only native-extension observation and preservation preview.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-target, or bridge response error.
    pub async fn plan_native_extensions(
        &self,
        request: &Ymm4NativeExtensionPlanRequest,
    ) -> Result<Ymm4NativeExtensionPlanResponse, Ymm4Error> {
        self.post_json("v2/native-extension/plan", request).await
    }

    /// Applies or idempotently replays a fully approved native-extension plan.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-target, or bridge response error.
    pub async fn apply_native_extensions(
        &self,
        request: &Ymm4NativeExtensionApplyRequest,
    ) -> Result<Ymm4NativeExtensionApplyResponse, Ymm4Error> {
        self.post_json("v2/native-extension/apply", request).await
    }

    /// Seals or replays a durable, request-bound no-mutation result under the
    /// native-extension apply gate.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, stale-state, or bridge response error.
    pub async fn seal_native_extension_not_started(
        &self,
        request: &Ymm4NativeExtensionApplyRequest,
    ) -> Result<Ymm4NativeExtensionApplyResponse, Ymm4Error> {
        self.post_json("v2/native-extension/not-started", request)
            .await
    }

    /// Reads the durable bridge operation receipt. A 404 alone is not a safe
    /// no-mutation proof because an older POST may still be in transport; use
    /// `seal_native_extension_not_started` before releasing a reservation.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, not-found, or bridge response error.
    pub async fn native_extension_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Ymm4NativeExtensionApplyResponse, Ymm4Error> {
        self.get_json(&format!("v2/native-extension/operations/{operation_id}"))
            .await
    }
}

struct ApplyDigestInput<'a> {
    operation_id: Uuid,
    project_id: &'a str,
    scene_id: &'a str,
    expected_fingerprint: &'a str,
    descriptor_catalog_digest: &'a str,
    driver_profile_digest: &'a str,
    plan_digest: &'a str,
    artifacts: &'a [Ymm4NativeExtensionArtifact],
}

fn native_extension_apply_digest(input: &ApplyDigestInput<'_>) -> String {
    fn write_string(canonical: &mut String, label: &str, value: &str) {
        use std::fmt::Write as _;
        let _ = writeln!(canonical, "{label}:{}:{value}", value.len());
    }

    use std::fmt::Write as _;
    let mut canonical = String::from("takegraph-ymm4-native-extension-apply-v2\n");
    let _ = writeln!(canonical, "protocolVersion:{YMM4_BRIDGE_PROTOCOL_VERSION}");
    write_string(
        &mut canonical,
        "operationId",
        &input.operation_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", input.project_id);
    write_string(&mut canonical, "sceneId", input.scene_id);
    write_string(
        &mut canonical,
        "expectedFingerprint",
        input.expected_fingerprint,
    );
    write_string(
        &mut canonical,
        "descriptorCatalogDigest",
        input.descriptor_catalog_digest,
    );
    write_string(
        &mut canonical,
        "driverProfileDigest",
        input.driver_profile_digest,
    );
    write_string(&mut canonical, "planDigest", input.plan_digest);
    let _ = writeln!(canonical, "artifacts:{}", input.artifacts.len());
    for artifact in input.artifacts {
        write_string(&mut canonical, "artifactDigest", &artifact.artifact_digest);
        write_string(&mut canonical, "mediaType", &artifact.media_type);
        let _ = writeln!(canonical, "byteLength:{}", artifact.byte_length);
        write_string(
            &mut canonical,
            "kind",
            match artifact.kind {
                AssetKind::Image => "image",
                AssetKind::Video => "video",
                AssetKind::Audio => "audio",
                AssetKind::Bgm => "bgm",
            },
        );
        write_string(&mut canonical, "path", &artifact.path.to_string_lossy());
        write_string(&mut canonical, "sha256", &artifact.sha256);
    }
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn target_descriptor_digest(
    descriptor: &Ymm4TargetDescriptor,
) -> Result<String, NativeExtensionNodeError> {
    let config = prefixed_sha256(&descriptor.config_digest)?;
    let schema = prefixed_sha256(&descriptor.schema_digest)?;
    takegraph_core::canonical_sha256(
        "takegraph-ymm4-target-descriptor-v1",
        &serde_json::json!({
            "descriptorId": descriptor.descriptor_id,
            "kind": descriptor.kind,
            "configDigest": config,
            "schemaDigest": schema,
            "bindable": descriptor.bindable,
            "mutationAllowed": descriptor.mutation_allowed,
        }),
    )
    .map_err(|error| NativeExtensionNodeError::InvalidCatalog(error.to_string()))
}

fn prefixed_sha256(value: &str) -> Result<String, NativeExtensionNodeError> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    require_raw_sha256(value, "digest")?;
    Ok(format!("sha256:{}", value.to_ascii_lowercase()))
}

fn require_raw_sha256(value: &str, field: &str) -> Result<(), NativeExtensionNodeError> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(NativeExtensionNodeError::InvalidDigest(field.into()));
    }
    Ok(())
}

fn require_non_empty(value: &str, field: &str) -> Result<(), NativeExtensionNodeError> {
    if value.trim().is_empty() {
        return Err(NativeExtensionNodeError::EmptyField(field.into()));
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<(String, u64), NativeExtensionNodeError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        length = length
            .checked_add(u64::try_from(read).expect("buffer read length fits u64"))
            .ok_or_else(|| NativeExtensionNodeError::InvalidArtifact("file too large".into()))?;
    }
    Ok((format!("sha256:{:x}", hasher.finalize()), length))
}

fn copy_create_new(source: &Path, destination: &Path) -> Result<(), NativeExtensionNodeError> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.flush()?;
    output.sync_all()?;
    Ok(())
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "video/mp4" => "mp4",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" => "mp3",
        _ => "bin",
    }
}

fn validate_media_kind(kind: AssetKind, media_type: &str) -> Result<(), NativeExtensionNodeError> {
    let valid = match kind {
        AssetKind::Image => media_type.starts_with("image/"),
        AssetKind::Video => media_type.starts_with("video/"),
        AssetKind::Audio | AssetKind::Bgm => media_type.starts_with("audio/"),
    };
    if !valid {
        return Err(NativeExtensionNodeError::InvalidArtifact(format!(
            "media type {media_type} does not match {kind:?}"
        )));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum NativeExtensionNodeError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("YMM4 descriptor protocol mismatch: expected {expected}, got {actual}")]
    ProtocolMismatch { expected: u32, actual: u32 },
    #[error("empty native-extension field: {0}")]
    EmptyField(String),
    #[error("invalid SHA-256 field: {0}")]
    InvalidDigest(String),
    #[error("duplicate YMM4 descriptor ID: {0}")]
    DuplicateDescriptor(String),
    #[error("invalid YMM4 descriptor catalog: {0}")]
    InvalidCatalog(String),
    #[error("YMM4 descriptors changed after staging (expected {expected}, got {actual})")]
    DescriptorDrift { expected: String, actual: String },
    #[error("native-extension artifact escaped the authorized root: {0}")]
    ArtifactPathEscape(PathBuf),
    #[error("native-extension artifact digest mismatch: expected {expected}, got {actual}")]
    ArtifactDigestMismatch { expected: String, actual: String },
    #[error("native-extension artifact length mismatch: expected {expected}, got {actual}")]
    ArtifactLengthMismatch { expected: u64, actual: u64 },
    #[error("invalid native-extension artifact: {0}")]
    InvalidArtifact(String),
    #[error("invalid native-extension plan: {0}")]
    InvalidPlan(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use takegraph_core::ImmutableAssetReference;

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn descriptor_catalog() -> Ymm4DescriptorCatalog {
        Ymm4DescriptorCatalog {
            protocol_version: 2,
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            driver_profile_digest: digest('1'),
            catalog_digest: digest('2'),
            descriptors: vec![
                Ymm4TargetDescriptor {
                    descriptor_id: "character.marisa".into(),
                    kind: "character".into(),
                    name: "魔理沙".into(),
                    config_digest: digest('3'),
                    schema_digest: digest('4'),
                    bindable: true,
                    mutation_allowed: false,
                    metadata: BTreeMap::from([
                        ("tachieType".into(), "YMM4".into()),
                        ("groupName".into(), String::new()),
                    ]),
                },
                Ymm4TargetDescriptor {
                    descriptor_id: "effect.invert".into(),
                    kind: "video-effect".into(),
                    name: "色反転".into(),
                    config_digest: digest('5'),
                    schema_digest: digest('6'),
                    bindable: true,
                    mutation_allowed: true,
                    metadata: BTreeMap::from([(
                        "type".into(),
                        "YukkuriMovieMaker.Project.Effects.InvertEffect".into(),
                    )]),
                },
            ],
        }
    }

    #[test]
    fn target_catalog_maps_and_pins_config_and_schema() {
        let target = descriptor_catalog();
        let planning = target.planning_catalog().unwrap();
        let character = &planning.characters["character.marisa"];
        assert_eq!(
            character.configuration["takegraph.targetConfigDigest"],
            format!("sha256:{}", digest('3'))
        );
        assert_eq!(planning.effects["effect.invert"].parameters.len(), 0);
        assert!(!character.configuration.contains_key("groupName"));

        let mut drifted = target.clone();
        drifted.descriptors[0].schema_digest = digest('f');
        assert!(matches!(
            target.require_unchanged(&drifted),
            Err(NativeExtensionNodeError::DescriptorDrift { .. })
        ));
    }

    #[test]
    fn content_addressed_artifact_is_rehashed_and_path_bounded() {
        let root = std::env::temp_dir().join(format!(
            "takegraph-phase4-node-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.wav");
        fs::write(&source, b"not-a-real-wav-but-immutable").unwrap();
        let (digest, byte_length) = hash_file(&source).unwrap();
        let reference = ImmutableAssetReference {
            artifact_digest: digest,
            media_type: "audio/wav".into(),
            byte_length,
            kind: AssetKind::Bgm,
        };
        let artifacts = root.join("artifacts");
        let materialized =
            materialize_native_extension_artifact(&source, &reference, &artifacts).unwrap();
        materialized.verify(&artifacts).unwrap();
        fs::write(&materialized.path, b"changed").unwrap();
        assert!(matches!(
            materialized.verify(&artifacts),
            Err(NativeExtensionNodeError::ArtifactDigestMismatch { .. })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bridge_dtos_match_camel_case_contract() {
        let request = Ymm4NativeExtensionPlanRequest::new(
            Uuid::nil(),
            "project-a",
            "scene-a",
            "fingerprint-a",
            digest('2'),
            Vec::new(),
            Vec::new(),
        );
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["protocolVersion"], 2);
        assert_eq!(value["operationId"], Uuid::nil().to_string());
        assert!(value.get("operation_id").is_none());
    }

    #[test]
    fn native_extension_unicode_apply_cross_runtime_golden() {
        use takegraph_core::{
            AssetClipIntent, CapabilityDependency, ChangeBudget, NativeExtensionAction,
            NativeExtensionIntent, NativeExtensionPlan, PlannedNativeExtension, ResolvedPlacement,
            RevisionId, ScopeFingerprints, TargetIdentity,
        };

        let prefixed = |byte: char| format!("sha256:{}", digest(byte));
        let plan = NativeExtensionPlan {
            canonical_version: 1,
            operation_id: Uuid::nil(),
            base_revision: RevisionId(7),
            target: TargetIdentity {
                adapter_id: "ymm4-4.55".into(),
                project_id: "project-日本語".into(),
                scene_id: "scene-魔理沙".into(),
                fps: 60,
                driver_version: "4.55.1.1/0.3.0".into(),
            },
            capability_digest: prefixed('1'),
            descriptor_catalog_digest: prefixed('2'),
            expected_scope: ScopeFingerprints {
                target_identity_digest: prefixed('3'),
                managed_state_digest: prefixed('4'),
                conflict_scope_digest: prefixed('5'),
            },
            change_budget: ChangeBudget::create_only(1),
            operations: vec![PlannedNativeExtension {
                realization_id: Uuid::parse_str("11111111-2222-5333-8444-555555555555").unwrap(),
                action: NativeExtensionAction::Create,
                intent: NativeExtensionIntent::UpsertAsset(AssetClipIntent {
                    entity_id: "背景-第一形態".into(),
                    entity_revision: 2,
                    asset: ImmutableAssetReference {
                        artifact_digest: prefixed('b'),
                        media_type: "image/png".into(),
                        byte_length: 1234,
                        kind: AssetKind::Image,
                    },
                    placement: ResolvedPlacement {
                        frame: 120,
                        primary_layer: 10,
                        secondary_layer: None,
                    },
                    duration_frames: 180,
                    loop_playback: false,
                    replacement_guard: takegraph_core::ReplacementGuard::default(),
                }),
                capability_dependencies: vec![
                    CapabilityDependency {
                        feature: "imageItem.upsert".into(),
                        minimum_version: 1,
                        schema_digest: Some(prefixed('6')),
                    },
                    CapabilityDependency {
                        feature: "timeline.transaction".into(),
                        minimum_version: 1,
                        schema_digest: Some(prefixed('7')),
                    },
                ],
                descriptor_dependencies: Vec::new(),
                preservation: takegraph_core::PreservationPlan::create(),
            }],
            warnings: vec!["素材を新規作成: 背景".into()],
        };
        let artifact = Ymm4NativeExtensionArtifact {
            artifact_digest: prefixed('b'),
            media_type: "image/png".into(),
            byte_length: 1234,
            kind: AssetKind::Image,
            path: PathBuf::from(r"C:\TakeGraph\素材\背景.png"),
            sha256: prefixed('b'),
        };
        let request = Ymm4NativeExtensionApplyRequest::new(
            "project-日本語",
            "scene-魔理沙",
            "fingerprint-魔理沙",
            digest('8'),
            digest('9'),
            plan,
            vec![artifact],
        )
        .unwrap();
        assert_eq!(
            request.plan_digest,
            "sha256:9a4d6f6bb603df06c63140caf42f1db1732e9e48d57bcdb82b37c7a85b1665f8"
        );
        assert_eq!(
            request.request_digest,
            "e6a0faa5f40a5d37b08616203de9c049cba38cf2d28495bac6222460642ad907"
        );
    }
}
