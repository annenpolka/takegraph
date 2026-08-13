//! Local media-node adapters for `TakeGraph`.

pub mod artifacts;
pub mod native_extension;
pub mod project_operations;
pub mod reconciliation;
pub mod scene_composition;
pub mod scene_inspection;
pub mod voicevox;
pub mod ymm4;
pub mod ymm4_capabilities;

pub use artifacts::{
    ArtifactError, ImportedYmm4NativeVoiceArtifact, MaterializedVoiceArtifact, WavMetadata,
    import_ymm4_native_voice_artifact, parse_wav,
};
pub use native_extension::{
    NativeExtensionNodeError, Ymm4DescriptorCatalog, Ymm4ExistingNativeExtension,
    Ymm4ExistingNativeExtensionKind, Ymm4ExistingUpdateMode, Ymm4NativeExtensionApplyRequest,
    Ymm4NativeExtensionApplyResponse, Ymm4NativeExtensionArtifact, Ymm4NativeExtensionObservation,
    Ymm4NativeExtensionPlanRequest, Ymm4NativeExtensionPlanResponse,
    Ymm4NativeExtensionRealization, Ymm4NativeExtensionStatus, Ymm4OpaqueNativeEffect,
    Ymm4PreservedNativeField, Ymm4TargetDescriptor, materialize_native_extension_artifact,
};
pub use project_operations::{
    CHECKPOINT_REQUEST_DOMAIN, FINAL_MEDIA_PROBE_PROFILE, MediaContainer,
    ProjectOperationNodeError, RENDER_REQUEST_DOMAIN, RenderOverwritePolicy, VerifiedMediaProbe,
    Ymm4CheckpointProfile, Ymm4CheckpointReceipt, Ymm4CheckpointRequest,
    Ymm4CheckpointRequestInput, Ymm4CheckpointStatus, Ymm4RenderCancelRequest,
    Ymm4RenderOverwriteJournal, Ymm4RenderProfileDescriptor, Ymm4RenderProfiles, Ymm4RenderRequest,
    Ymm4RenderRequestInput, Ymm4RenderStatus, Ymm4RenderTask, Ymm4RenderedMediaReceipt,
    validate_render_task, verify_checkpoint, verify_render_checkpoint_file, verify_rendered_media,
};
pub use reconciliation::{
    METADATA_DETACH_REQUEST_DOMAIN, MetadataDetachNodeError, Ymm4MetadataDetachReceipt,
    Ymm4MetadataDetachRequest, Ymm4MetadataDetachRequestInput, Ymm4MetadataDetachResponse,
    Ymm4MetadataDetachStatus, verify_metadata_detach, verify_metadata_detach_not_started,
    verify_metadata_detach_rollback,
};
pub use scene_composition::{
    YMM4_SCENE_COMPOSITION_SCHEMA_VERSION, Ymm4CompositionAvailability,
    Ymm4CompositionCompleteness, Ymm4CompositionElement, Ymm4CompositionElementStability,
    Ymm4CompositionViewport, Ymm4CompositionVisual, Ymm4SceneCompositionError,
    Ymm4SceneCompositionSnapshot,
};
pub use scene_inspection::{
    ImportedSceneCapture, PNG_MEDIA_TYPE, PixelRect, Rgba8, SCENE_PIXEL_DETECTOR_VERSION,
    SceneFindingCode, SceneFindingSeverity, SceneFrameInspection, SceneInspectionFinding,
    SceneInspectionNodeError, SceneVisualCheckProfile, VisualRegionDetection,
    VisualRegionExpectation, VisualRegionKind, YMM4_SCENE_CAPTURE_DRIVER_ID,
    YMM4_SCENE_CAPTURE_DRIVER_PROFILE_DIGEST, Ymm4SceneCaptureFrameReceipt,
    Ymm4SceneCaptureReceipt, Ymm4SceneCaptureRequest, Ymm4SceneCaptureRequestInput,
    Ymm4SceneCaptureStatus, import_png_capture, verify_imported_capture,
};
pub use voicevox::{
    Speaker, SpeakerStyle, VoiceProvider, VoicevoxCapabilities, VoicevoxClient, VoicevoxError,
};
pub use ymm4::{
    ManagedItemKind, ManagedUtterance, YMM4_BRIDGE_PROTOCOL_VERSION, Ymm4ApplyRequest,
    Ymm4ApplyResponse, Ymm4BridgeClient, Ymm4Capabilities, Ymm4Capability, Ymm4Error, Ymm4Health,
    Ymm4ManagedItem, Ymm4ManagedNativeExtension, Ymm4NativeVoiceApplyRequest,
    Ymm4NativeVoiceArtifact, Ymm4NativeVoiceArtifactRequest, Ymm4NativeVoiceCue,
    Ymm4NativeVoiceMutation, Ymm4NativeVoiceMutationAction, Ymm4NativeVoiceMutationApplyRequest,
    Ymm4NativeVoiceMutationPlanRequest, Ymm4NativeVoiceMutationPlanResponse,
    Ymm4NativeVoicePlanRequest, Ymm4NativeVoicePlanResponse, Ymm4OperationReceipt,
    Ymm4OperationStatus, Ymm4PlanRequest, Ymm4PlanResponse, Ymm4ProjectControl,
    Ymm4ProjectControlResult, Ymm4ProjectControls, Ymm4ProjectInitializationPreparation,
    Ymm4ProjectInitializationPrepareRequest, Ymm4ProjectInitializationReceipt,
    Ymm4ProjectInitializationRequest, Ymm4ProjectInitializationStatus, Ymm4ProjectInstanceBinding,
    Ymm4ProjectSnapshot, Ymm4TargetPlanApplyRequest, Ymm4TargetPlanRequest,
    Ymm4TargetPlanValidation, Ymm4TimelineEditApplyRequest, Ymm4TimelineEditApplyResponse,
    Ymm4TimelineEditReceipt, Ymm4TimelineEditValidation, Ymm4TimelineEditValidationRequest,
};
pub use ymm4_capabilities::{
    CapabilityRequirement, CapabilityValue, DriverDescriptor, FeatureDescriptor, MutationStatus,
    ProtocolContract, StructuredCapabilityError, StructuredYmm4Capabilities,
};
