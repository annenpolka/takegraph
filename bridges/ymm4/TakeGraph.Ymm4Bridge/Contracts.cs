using System.Text.Json;
using System.Text.Json.Serialization;

namespace TakeGraph.Ymm4Bridge;

internal static class BridgeContract
{
    public const int ProtocolVersion = 2;
    public const int Port = 8766;
    public const string TokenHeader = "x-takegraph-token";
}

internal sealed record HealthDto(
    string Status,
    int ProtocolVersion,
    string PluginVersion,
    string Ymm4Version);

internal sealed record CapabilitiesDto(
    int ProtocolVersion,
    IReadOnlyList<string> Capabilities);

internal sealed record TargetDescriptorDto(
    string DescriptorId,
    string Kind,
    string Name,
    string ConfigDigest,
    string SchemaDigest,
    bool Bindable,
    bool MutationAllowed,
    IReadOnlyDictionary<string, string> Metadata);

internal sealed record DescriptorCatalogDto(
    int ProtocolVersion,
    string ProjectId,
    string SceneId,
    string DriverProfileDigest,
    string CatalogDigest,
    IReadOnlyList<TargetDescriptorDto> Descriptors);

internal sealed record ProjectControlDto(
    string Scope,
    string Name,
    bool CanExecute);

internal sealed record ProjectControlsDto(
    IReadOnlyList<ProjectControlDto> Commands);

internal sealed record ProjectControlResultDto(
    string Action,
    string Scope,
    string Command,
    bool Success);

internal sealed record ManagedItemDto(
    string EntityId,
    ulong Revision,
    string Kind,
    int Frame,
    int Layer,
    int Length,
    string? Text,
    string? AudioPath,
    string? ArtifactHash,
    string? Speaker = null,
    string? RealizationId = null);

/// Fresh managed-only native-extension projection. Preservation witnesses,
/// opaque effects, and host-local state are intentionally absent.
internal sealed record ManagedNativeExtensionDto(
    string LogicalKey,
    Guid RealizationId,
    string Kind,
    string ProjectId,
    string EntityId,
    ulong EntityRevision,
    IReadOnlyDictionary<string, string> OwnedFields);

internal sealed record ProjectSnapshotDto(
    string ProjectId,
    string ProjectName,
    string ProjectPath,
    string SceneId,
    uint Fps,
    string Fingerprint,
    IReadOnlyList<ManagedItemDto> ManagedItems,
    IReadOnlyList<ManagedNativeExtensionDto> NativeExtensions,
    int UnmanagedContextCount);

internal sealed record ProjectInitializationPrepareRequestDto(
    int ProtocolVersion,
    string DestinationPath);

internal sealed record ProjectInitializationPreparationDto(
    int ProtocolVersion,
    string DriverProfileDigest,
    string SourceProjectInstanceId,
    ProjectSnapshotDto Source,
    string DestinationPath,
    string DestinationPathDigest,
    string PredictedProjectId,
    string PredictedFingerprint,
    bool Overwrite);

internal sealed record ProjectInstanceBindingDto(
    int ProtocolVersion,
    string DriverProfileDigest,
    string SourceProjectInstanceId,
    ProjectSnapshotDto Source);

internal sealed record ProjectInitializationRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string DriverProfileDigest,
    string SourceProjectInstanceId,
    string SourceProjectId,
    string SourceSceneId,
    string ExpectedSourceFingerprint,
    string DestinationPath,
    string DestinationPathDigest,
    string PredictedProjectId,
    string PredictedFingerprint,
    bool Overwrite);

internal sealed record ProjectInitializationReceiptDto(
    Guid OperationId,
    string RequestDigest,
    string Status,
    string DriverProfileDigest,
    string SourceProjectInstanceId,
    string SourceProjectId,
    string SourceSceneId,
    string BeforeFingerprint,
    string DestinationPath,
    string DestinationPathDigest,
    string PredictedProjectId,
    string PredictedFingerprint,
    string? PreparedTemporaryPath,
    string? PreparedFileSha256,
    ulong? PreparedFileBytes,
    ProjectSnapshotDto? AfterSnapshot,
    string? FileSha256,
    ulong? FileBytes,
    string? Error);

/// A read-only observation of the active timeline at the preview's current frame.
/// Geometry is deliberately explicit about availability: the bridge must not
/// infer transforms or bounds that YMM4 does not expose as stable scalar values.
internal sealed record SceneCompositionViewportDto(
    string Availability,
    int? Width,
    int? Height);

internal sealed record SceneCompositionVisualDto(
    string Availability,
    int? X,
    int? Y,
    int? Width,
    int? Height);

internal sealed record SceneCompositionElementDto(
    string ElementId,
    string Stability,
    string Kind,
    int Frame,
    int Layer,
    int Length,
    bool Active,
    bool? Selected,
    string? Text,
    SceneCompositionVisualDto Visual);

internal sealed record SceneCompositionSnapshotDto(
    uint SchemaVersion,
    string ProjectId,
    string SceneId,
    string SourceFingerprint,
    uint Fps,
    int Frame,
    SceneCompositionViewportDto Viewport,
    IReadOnlyList<SceneCompositionElementDto> Elements,
    string Completeness,
    IReadOnlyList<string> UnavailableFields);

internal sealed record ManagedUtteranceDto(
    string EntityId,
    ulong Revision,
    string Speaker,
    string Caption,
    string AudioPath,
    string ArtifactHash,
    int Frame,
    int Length,
    int AudioLayer,
    int CaptionLayer,
    // Optional on legacy physical-route JSON. A missing value means the old
    // caption-equals-spoken contract; sealed target plans always provide it.
    string? SpokenText = null);

internal sealed record PlanRequestDto(
    int ProtocolVersion,
    string ExpectedFingerprint,
    IReadOnlyList<ManagedUtteranceDto> Utterances);

internal sealed record PlanResponseDto(
    string Fingerprint,
    int OperationCount,
    int CreateCount,
    int ReplaceCount,
    int UnchangedCount,
    IReadOnlyList<ManagedItemDto> ManagedItemsAfter);

internal sealed record ApplyRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    IReadOnlyList<ManagedUtteranceDto> Utterances);

internal sealed record OperationReceiptDto(
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    string Status,
    string BeforeFingerprint,
    string AfterFingerprint,
    IReadOnlyList<ManagedItemDto> AppliedItems,
    bool Verified,
    string? Error);

internal sealed record ApplyResponseDto(
    bool Success,
    bool Replayed,
    OperationReceiptDto Receipt);

internal sealed record RecoveryItemDto(
    string TypeName,
    string Json,
    string Sha256);

internal sealed record RecoveryJournalEntryDto(
    int SchemaVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    string BeforeFingerprint,
    string State,
    string Driver,
    IReadOnlyList<string> EntityIds,
    IReadOnlyList<Guid> RealizationIds,
    IReadOnlyList<ManagedItemDto> ExpectedItems,
    IReadOnlyDictionary<Guid, string> PreservedStateDigests,
    IReadOnlyList<RecoveryItemDto> BeforeItems,
    DateTimeOffset CreatedAt,
    DateTimeOffset UpdatedAt,
    string? AfterFingerprint,
    string? Error,
    // Written atomically with the applied_unverified transition. These are
    // historical read-back facts, not a plan that startup may re-evaluate
    // against the current project.
    IReadOnlyList<ManagedItemDto>? VerifiedItems = null,
    NativeExtensionApplyResponseDto? NativeExtensionReceipt = null);

internal sealed record RecoveryStatusDto(
    int Pending,
    int Verified,
    int RolledBack,
    int RecoveryRequired,
    IReadOnlyList<RecoveryJournalEntryDto> Entries);

internal sealed record NativeVoiceCueDto(
    Guid RealizationId,
    string EntityId,
    ulong Revision,
    string CharacterName,
    string DisplayText,
    string SpokenText,
    int Frame,
    int Layer,
    int MaxLength);

internal sealed record NativeVoiceMutationDto(
    Guid RealizationId,
    string EntityId,
    ulong Revision,
    string CharacterName,
    string DisplayText,
    string SpokenText,
    int Frame,
    int Layer,
    int MaxLength,
    string Action);

internal sealed record NativeVoiceMutationPlanRequestDto(
    int ProtocolVersion,
    string ExpectedFingerprint,
    IReadOnlyList<NativeVoiceMutationDto> Mutations);

internal sealed record NativeVoiceMutationPlanResponseDto(
    string Fingerprint,
    int CreateCount,
    int UpdateCount,
    int DeleteCount,
    string DurationResolution,
    IReadOnlyList<string> PreservedFields);

internal sealed record NativeVoicePlanRequestDto(
    int ProtocolVersion,
    string ExpectedFingerprint,
    IReadOnlyList<NativeVoiceCueDto> Cues);

internal sealed record NativeVoicePlanResponseDto(
    string Fingerprint,
    int CreateCount,
    string DurationResolution);

internal sealed record NativeVoiceApplyRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    IReadOnlyList<NativeVoiceCueDto> Cues);

internal sealed record NativeVoiceMutationApplyRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    IReadOnlyList<NativeVoiceMutationDto> Mutations);

internal sealed record NativeVoiceArtifactRequestDto(
    int ProtocolVersion,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    Guid RealizationId);

internal sealed record NativeVoiceArtifactDto(
    Guid RealizationId,
    string AudioPath,
    string AudioSha256,
    long AudioBytes,
    string QueryPath,
    string QuerySha256,
    long QueryBytes,
    string Provenance);

internal sealed record MetadataDetachRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string ExpectedFingerprint,
    string EntityId,
    Guid RealizationId,
    string IdentityCarrier);

internal sealed record MetadataDetachTargetDto(
    string TypeName,
    int Frame,
    int Layer,
    int Length,
    string OriginalRemark,
    string RemarkSha256,
    string NonRemarkContentDigest);

internal sealed record MetadataDetachSceneRemarkDto(
    string StableItemWitness,
    string TypeName,
    int Frame,
    int Layer,
    int Length,
    string NonRemarkContentDigest,
    string OriginalRemark,
    string OriginalRemarkSha256,
    string ExpectedRemark,
    string ExpectedRemarkSha256);

internal sealed record MetadataDetachReceiptDto(
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string ExpectedFingerprint,
    string EntityId,
    Guid RealizationId,
    string IdentityCarrier,
    string Status,
    string BeforeFingerprint,
    string AfterFingerprint,
    uint DetachedItemCount,
    string BeforeRemarkDigest,
    string ExpectedAfterRemarkDigest,
    string RemarkDigestAfter,
    string NonRemarkContentDigestBefore,
    string NonRemarkContentDigestAfter,
    bool RemarkAbsent,
    bool Verified,
    string? Error);

internal sealed record MetadataDetachResponseDto(
    bool Success,
    bool Replayed,
    MetadataDetachReceiptDto Receipt);

internal sealed record MetadataDetachJournalDto(
    int SchemaVersion,
    MetadataDetachRequestDto Request,
    string State,
    string BeforeFingerprint,
    string? AfterFingerprint,
    string BeforeRemarkDigest,
    string ExpectedAfterRemarkDigest,
    string? RemarkDigestAfter,
    string NonRemarkContentDigestBefore,
    string? NonRemarkContentDigestAfter,
    IReadOnlyList<MetadataDetachTargetDto> Targets,
    IReadOnlyList<MetadataDetachSceneRemarkDto> SceneRemarks,
    bool RemarkAbsent,
    DateTimeOffset CreatedAt,
    DateTimeOffset UpdatedAt,
    string? Error);

internal sealed record SceneCaptureRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    ulong SourceRevision,
    string CaptureProfileDigest,
    IReadOnlyList<int> Frames,
    bool Alpha);

internal sealed record SceneCaptureFrameDto(
    int RequestedFrame,
    int ActualFrame,
    string Path,
    string Sha256,
    int Width,
    int Height,
    string MediaType);

internal sealed record SceneCaptureReceiptDto(
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string ExpectedFingerprint,
    string CaptureProfileDigest,
    string Status,
    string BeforeFingerprint,
    string AfterFingerprint,
    IReadOnlyList<SceneCaptureFrameDto> Frames,
    string Driver,
    string DriverProfileDigest,
    bool TransientStateRestored,
    bool ProjectDirtyBefore,
    bool ProjectDirtyAfter,
    string? Error);

internal sealed record CheckpointRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string TargetIdentityDigest,
    string ExpectedStateDigest,
    string CheckpointProfileDigest);

internal sealed record CheckpointProfileDto(
    string ProfileDigest,
    string DriverProfileDigest,
    bool ExistingPathOnly);

internal sealed record CheckpointReceiptDto(
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string TargetIdentityDigest,
    string ExpectedStateDigest,
    string CheckpointProfileDigest,
    string Status,
    string ProjectPath,
    string? PreFileSha256,
    string? PostFileSha256,
    ulong? PostFileBytes,
    string BeforeStateDigest,
    string AfterStateDigest,
    string DriverProfileDigest,
    string? Error);

internal sealed record RenderProfileDescriptorDto(
    string DescriptorId,
    string DisplayName,
    string ProfileDigest,
    string Container,
    uint Width,
    uint Height,
    uint FpsNumerator,
    uint FpsDenominator,
    bool HasAudio,
    string VideoCodec,
    string AudioCodec,
    uint AudioSampleRate,
    string PixelFormat,
    string WriterPlugin,
    string BindingManifestDigest,
    string DriverProfileDigest,
    bool Bindable,
    string? BindingError);

internal sealed record RenderProfilesDto(
    IReadOnlyList<RenderProfileDescriptorDto> Profiles,
    string DescriptorSetDigest);

internal sealed record RenderRequestDto(
    int ProtocolVersion,
    Guid TaskId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string TargetIdentityDigest,
    string ExpectedStateDigest,
    Guid CheckpointOperationId,
    string CheckpointRequestDigest,
    string CheckpointProjectPath,
    string CheckpointFileSha256,
    ulong CheckpointFileBytes,
    string RenderProfileDigest,
    string OutputPath,
    string OverwritePolicy);

internal sealed record RenderCancelRequestDto(
    int ProtocolVersion,
    Guid TaskId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string ExpectedStateDigest,
    Guid CheckpointOperationId,
    string CheckpointRequestDigest,
    string CheckpointFileSha256,
    ulong CheckpointFileBytes,
    string RenderProfileDigest);

internal sealed record RenderedMediaReceiptDto(
    string OutputPath,
    string Sha256,
    ulong ByteLength,
    string Container,
    ulong DurationMillis,
    uint Width,
    uint Height,
    uint VideoStreams,
    uint AudioStreams,
    uint FpsNumerator,
    uint FpsDenominator,
    string VideoCodec,
    string? AudioCodec,
    uint? AudioSampleRate,
    string PixelFormat,
    string ProbeProfile,
    string ProbeDigest);

internal sealed record RenderOverwriteJournalDto(
    string State,
    bool OriginalExisted,
    string? OriginalSha256,
    ulong? OriginalByteLength,
    string BackupPath,
    string QuarantinePath,
    string CandidateDirectory,
    string CandidatePath,
    string? CandidateSha256,
    ulong? CandidateByteLength);

internal sealed record RenderTaskDto(
    Guid TaskId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    ulong SourceRevision,
    string TargetIdentityDigest,
    string ExpectedStateDigest,
    Guid CheckpointOperationId,
    string CheckpointRequestDigest,
    string CheckpointProjectPath,
    string CheckpointFileSha256,
    ulong CheckpointFileBytes,
    string RenderProfileDigest,
    string OutputPath,
    string OverwritePolicy,
    string EncodeSourcePath,
    RenderOverwriteJournalDto OverwriteJournal,
    string Status,
    ushort ProgressBasisPoints,
    string Phase,
    bool Cancellable,
    string BeforeStateDigest,
    string? AfterStateDigest,
    RenderedMediaReceiptDto? Media,
    string? Error);

internal sealed record NativeExtensionArtifactDto(
    string ArtifactDigest,
    string MediaType,
    ulong ByteLength,
    string Kind,
    string Path,
    string Sha256);

internal sealed record NativeExtensionPlanRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    string DescriptorCatalogDigest,
    IReadOnlyList<System.Text.Json.JsonElement> Intents,
    IReadOnlyList<NativeExtensionArtifactDto> Artifacts);

internal sealed record NativeExtensionUpdateModeDto(
    string Mode,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    IReadOnlyList<string>? LossyFields = null);

internal sealed record NativeExtensionPreservedFieldDto(
    string Field,
    string StateDigest);

internal sealed record NativeExtensionOpaqueEffectDto(
    string StableTypeId,
    string InstanceKey,
    string StateDigest);

internal sealed record NativeExtensionExistingDto(
    string LogicalKey,
    Guid RealizationId,
    string Kind,
    NativeExtensionUpdateModeDto UpdateMode,
    IReadOnlyList<NativeExtensionPreservedFieldDto> PreservedFields,
    IReadOnlyList<NativeExtensionOpaqueEffectDto> UnknownEffects);

internal sealed record NativeExtensionObservationDto(
    IReadOnlyDictionary<string, NativeExtensionExistingDto> Existing);

internal sealed record NativeExtensionPlanResponseDto(
    string Fingerprint,
    string DescriptorCatalogDigest,
    string DriverProfileDigest,
    NativeExtensionObservationDto Observation,
    IReadOnlyList<string> Warnings);

internal sealed record NativeExtensionApplyRequestDto(
    int ProtocolVersion,
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    string DescriptorCatalogDigest,
    string DriverProfileDigest,
    string PlanDigest,
    System.Text.Json.JsonElement Plan,
    IReadOnlyList<NativeExtensionArtifactDto> Artifacts);

internal sealed record NativeExtensionRealizationDto(
    string LogicalKey,
    Guid RealizationId,
    string Kind,
    string ProjectId,
    string EntityId,
    ulong EntityRevision,
    int Frame,
    int Layer,
    int Length,
    string OwnedStateDigest,
    IReadOnlyDictionary<string, string> OwnedFields,
    IReadOnlyList<NativeExtensionPreservedFieldDto> PreservedFields,
    string StateDigest,
    IReadOnlyList<NativeExtensionOpaqueEffectDto> UnknownEffects);

internal sealed record NativeExtensionApplyResponseDto(
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string Status,
    string BeforeFingerprint,
    string AfterFingerprint,
    string DescriptorCatalogDigest,
    string DriverProfileDigest,
    IReadOnlyList<NativeExtensionRealizationDto> Realizations,
    bool Verified,
    string? Error);

/// Unified v2 semantic plan envelope. Target-local placement and driver
/// bindings are contained exclusively in TargetPlan and covered by its digest.
internal sealed record TargetPlanRequestDto(
    int ProtocolVersion,
    string TargetPlanDigest,
    JsonElement TargetPlan);

internal sealed record TargetPlanApplyRequestDto(
    int ProtocolVersion,
    string RequestDigest,
    string ExpectedFingerprint,
    string TargetPlanDigest,
    JsonElement TargetPlan);

internal sealed record TargetPlanValidationDto(
    Guid OperationId,
    string TargetPlanDigest,
    string Fingerprint,
    IReadOnlyDictionary<string, int> StrategyCounts,
    int CreateCount,
    int UpdateCount,
    int DeleteCount,
    int PhysicalItemCount);

/// One approval-bound, ordered transaction over heterogeneous managed cue
/// strategies. Native-extension payloads are represented on the wire for
/// forward compatibility, but the current bridge capability deliberately
/// rejects them until their richer preservation receipt can share this WAL.
internal sealed record TimelineEditApplyRequestDto(
    int ProtocolVersion,
    string RequestDigest,
    string ExpectedFingerprint,
    string PlanDigest,
    JsonElement TimelineEditPlan,
    IReadOnlyList<NativeExtensionArtifactDto> Artifacts);

internal sealed record TimelineEditValidationRequestDto(
    int ProtocolVersion,
    string ExpectedFingerprint,
    string PlanDigest,
    JsonElement TimelineEditPlan,
    IReadOnlyList<NativeExtensionArtifactDto> Artifacts);

internal sealed record TimelineEditValidationDto(
    Guid OperationId,
    string PlanDigest,
    string Fingerprint,
    IReadOnlyDictionary<string, int> StrategyCounts,
    int CreateCount,
    int UpdateCount,
    int DeleteCount,
    int PhysicalItemCount);

internal sealed record TimelineEditReceiptDto(
    Guid OperationId,
    string RequestDigest,
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    string PlanDigest,
    string Status,
    string BeforeFingerprint,
    string AfterFingerprint,
    IReadOnlyList<ManagedItemDto> AppliedItems,
    IReadOnlyList<NativeExtensionRealizationDto> AppliedNativeExtensions,
    int AppliedOperationCount,
    bool Verified,
    string? Error);

internal sealed record TimelineEditApplyResponseDto(
    bool Success,
    bool Replayed,
    TimelineEditReceiptDto Receipt);

internal sealed record ErrorDto(
    string Error,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] string? ActualFingerprint = null);

internal sealed record ManagedMarker(
    string EntityId,
    ulong Revision,
    string ArtifactHash,
    string Speaker,
    string AudioPath,
    int AudioLayer);

internal sealed record NativeVoiceMarker(
    string Namespace,
    string ProjectId,
    string EntityId,
    Guid RealizationId,
    ulong Revision);

internal sealed record NativeExtensionEffectMarker(
    string EffectInstanceId,
    Guid RealizationId,
    string DescriptorId,
    string StableTypeId,
    string Collection,
    int Index);

internal sealed record NativeExtensionMarker(
    string Namespace,
    string ProjectId,
    string EntityId,
    ulong Revision,
    string LogicalKey,
    Guid RealizationId,
    string Kind,
    int PartIndex,
    int PartCount,
    IReadOnlyDictionary<string, NativeExtensionEffectMarker> Effects,
    string? DescriptorId = null);
