using System.Text.Json.Serialization;

namespace TakeGraph.Ymm4Bridge;

internal sealed record CaptureHostCredentialsFile(string Endpoint, string Token);

internal sealed record CaptureHostHealthDto(string Status);

internal sealed record CaptureHostDeriveDto(
    string Phase,
    Guid? CaptureId,
    string? Message);

internal sealed record CaptureHostStatusDto(
    string State,
    Guid? CaptureId,
    int? StartFrame,
    string? DeviceId,
    string Hotkey,
    string? ProjectId,
    string? Message,
    CaptureHostDeriveDto? Derive = null);

public sealed record CaptureDeviceDto(string Id, string Name, bool IsDefault);

internal sealed record CaptureDevicesDto(IReadOnlyList<CaptureDeviceDto> Devices);

internal sealed record CaptureAnnotationDto(
    Guid AnnotationId,
    Guid SessionId,
    int StartFrame,
    int EndFrame,
    string SceneId,
    string ProjectId,
    string SourceFingerprint,
    uint Fps,
    string Stability,
    string Lifecycle,
    string CapturedAtUtc,
    string AudioSha256,
    string? TranscriptSummary,
    string? TranscriptDigest = null,
    string? DerivePhase = null);

internal sealed record CaptureAnnotationsDto(IReadOnlyList<CaptureAnnotationDto> Annotations);

internal sealed record CaptureStartDto(Guid CaptureId);

internal sealed record CaptureDeviceUpdateDto(
    [property: JsonPropertyName("deviceId")] string? DeviceId);

internal sealed record CaptureHotkeyUpdateDto(
    [property: JsonPropertyName("hotkey")] string Hotkey);

internal sealed record CaptureHotkeysDto(IReadOnlyList<string> Hotkeys, string Selected);

internal sealed record CaptureHostErrorDto(string Error);
