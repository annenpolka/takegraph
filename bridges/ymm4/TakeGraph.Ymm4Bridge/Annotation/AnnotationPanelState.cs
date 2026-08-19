namespace TakeGraph.Ymm4Bridge;

internal enum CaptureHostConnection
{
    Missing,
    Connected,
    Recording,
    Failed,
}

internal sealed class AnnotationPanelState
{
    internal CaptureHostConnection Connection { get; set; } = CaptureHostConnection.Missing;
    internal string Hotkey { get; set; } = "F8";
    internal string? ProjectId { get; set; }
    internal string? Message { get; set; }
    internal Guid? ActiveCaptureId { get; set; }
    internal int? RecordingStartFrame { get; set; }
    internal CaptureHostDeriveDto? Derive { get; set; }

    internal string StatusLabel => Connection switch
    {
        CaptureHostConnection.Connected when IsDeriving => "Capture Host: 起こし中",
        CaptureHostConnection.Connected when IsDeriveFailed => "Capture Host: 起こし失敗",
        CaptureHostConnection.Connected => "Capture Host: connected",
        CaptureHostConnection.Recording => "Capture Host: recording",
        CaptureHostConnection.Failed => "Capture Host: failed",
        _ => "Capture Host: not running",
    };

    internal string Hint
    {
        get
        {
            if (Connection == CaptureHostConnection.Missing)
            {
                return "Start with: takegraph annotation listen --hotkey F8";
            }
            if ((IsDeriving || IsDeriveFailed) && !string.IsNullOrWhiteSpace(Derive?.Message))
            {
                return Derive.Message;
            }
            return Message ?? string.Empty;
        }
    }

    internal bool IsDeriving => Derive?.Phase is "queued" or "running";
    internal bool IsDeriveFailed => Derive?.Phase == "failed";

    internal static CaptureHostConnection FromStatus(CaptureHostStatusDto status) =>
        status.State switch
        {
            "recording" => CaptureHostConnection.Recording,
            "failed" => CaptureHostConnection.Failed,
            "idle" or "ymm4Disconnected" => CaptureHostConnection.Connected,
            _ => CaptureHostConnection.Connected,
        };
}
