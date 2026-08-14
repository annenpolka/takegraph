//! Portable `TakeGraph` domain model.
//!
//! This crate contains deterministic state transitions only. It intentionally
//! has no filesystem, network, database, or operating-system dependencies.

pub mod composition_graph;
pub mod edit_surface;
pub mod edit_transaction;
pub mod managed_cue;
pub mod native_extension;
pub mod project_edit;
pub mod project_initialization;
pub mod reconciliation;
pub mod revision;
pub mod timeline_edit;
pub mod voice;

pub use composition_graph::{
    CompositionError, CompositionGraph, CompositionIntent, CompositionOutcome, EntityId,
    FRAME_BUFFER_ID, GraphBinding, GraphEntity, INITIAL_FOCUS_ID, INITIAL_REVISION, Interpolation,
    ItemKind, Keyframe, ROOT_GROUP_ID, SCENE_A_ID, STABLE_PAINT_SIBLING_ID, TRANSITION_ID,
};
pub use edit_surface::{
    ApplicationEvidence, Availability, CrudKind, EditSurfaceError, EditSurfaceRejectReason,
    EditSurfaceSession, EditSurfaceStatus, FieldClass, INITIAL_CANONICAL_REVISION, ItemFamily,
    NormalizedPatch, Ownership, PatchInput, PlanBinding, TargetLock,
};
pub use edit_transaction::{
    EDIT_TRANSACTION_MAX_OPERATIONS, EditTransactionError, EditTransactionPlan,
    EditTransactionRequest, EditTransactionSession, EndpointMode, LaterTaskStatus,
    ReadbackEvidence, SemanticFamily, TargetScope, TransactionOperation, TransactionRejectReason,
    TransactionStatus, representative_operations,
};
pub use managed_cue::{
    BindingDependency, CanonicalError, CapabilityDependency, ChangeBudget, DurationResolution,
    FallbackPolicy, FallbackReason, ManagedCueIntent, NativeRealization, OrderingPolicy,
    OwnershipMask, PlacementIntent, PlanWarning, PlannedAction, PlannedCue,
    RealizationAvailability, RealizationPreference, RealizationStrategy, ResolvedPlacement,
    ResolvedRealization, ScopeFingerprints, StrategySelection, TARGET_PLAN_CANONICAL_VERSION,
    TargetIdentity, TargetPlan, TargetPlanError, TargetReference, TimingAnchor,
    approval_digests_match, canonical_sha256,
};
pub use native_extension::{
    AssetClipIntent, AssetKind, CharacterDescriptor, DescriptorDependency, DescriptorKind,
    DescriptorReference, EffectDescriptor, EffectOperation, EffectParameterSchema,
    EffectParameterSpec, EffectParameterValue, ImmutableAssetReference, ManagedEffectIntent,
    NATIVE_EXTENSION_PLAN_CANONICAL_VERSION, NativeDescriptorCatalog, NativeExtensionAction,
    NativeExtensionError, NativeExtensionIntent, NativeExtensionPlan, NativeMutationMode,
    NativeTemplateIntent, OpaqueNativeEffect, PlannedNativeExtension, PortraitIntent,
    PortraitPresentation, PreservationPlan, PreservedNativeField, ReplacementGuard,
    TemplateDescriptor,
};
pub use project_edit::{
    CharacterSettingClass, DependentPolicy, INITIAL_HEAD_REVISION, ProjectEditCapability,
    ProjectEditError, ProjectEditKind, ProjectEditPlan, ProjectEditSession, ProjectEditStatus,
    ProjectSettingClass,
};
pub use project_initialization::{
    PROJECT_INITIALIZATION_PLAN_SCHEMA_VERSION, ProjectInitializationDestination,
    ProjectInitializationMode, ProjectInitializationPlan, ProjectInitializationPlanError,
    ProjectInitializationSource,
};
pub use reconciliation::{
    ManagedFieldDrift, ManagedSemanticIdentity, ManagedSemanticItem, ManagedSemanticValue,
    RECONCILIATION_SCHEMA_VERSION, ReconciliationAction, ReconciliationChoice,
    ReconciliationDecision, ReconciliationError, ReconciliationPreview, ReconciliationSource,
    SemanticDriftEntry, SemanticDriftKind, SemanticDriftReport,
};
pub use revision::{Patch, PatchError, PatchId, PatchStatus, RevisionId};
pub use timeline_edit::{
    TIMELINE_EDIT_MAX_OPERATIONS, TIMELINE_EDIT_PLAN_CANONICAL_VERSION, TimelineEditError,
    TimelineEditOperation, TimelineEditPlan,
};
pub use voice::{AudioArtifact, VoiceTake, VoiceTakeError, VoiceTakeStatus, VoiceTaskIdentity};
