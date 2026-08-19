using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace TakeGraph.Ymm4Bridge;

internal sealed record AnnotationDecorationMarker(
    string Namespace,
    string ProjectId,
    Guid AnnotationId,
    Guid RealizationId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    string? EntityId = null);

internal static class AnnotationDecorationCodec
{
    internal const string Namespace = "takegraph/annotation/v1";
    internal const string PinNamespace = "takegraph/annotation-pin/v1";
    internal const int Layer = 90;
    private const string Start = "[[takegraph:";
    private const string End = "]]";

    internal static void ValidateRoundTrip()
    {
        var marker = new AnnotationDecorationMarker(
            Namespace,
            "project-a",
            Guid.Parse("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            Guid.Parse("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"));
        var encoded = Append(null, marker);
        if (!TryDecode(encoded, out var decoded) || decoded != marker)
        {
            throw new InvalidOperationException("annotation decoration codec round-trip failed");
        }
        if (TryDecode("user remark", out _))
        {
            throw new InvalidOperationException("plain remark decoded as an annotation decoration");
        }
    }

    internal static string Append(string? remark, AnnotationDecorationMarker marker)
    {
        var existing = Strip(remark).TrimEnd();
        var json = JsonSerializer.Serialize(marker, BridgeJson.Options);
        var payload = Convert.ToBase64String(Encoding.UTF8.GetBytes(json))
            .TrimEnd('=')
            .Replace('+', '-')
            .Replace('/', '_');
        return string.IsNullOrEmpty(existing)
            ? $"{Start}{payload}{End}"
            : $"{existing}{Environment.NewLine}{Start}{payload}{End}";
    }

    internal static bool TryDecode(string? remark, out AnnotationDecorationMarker? marker)
    {
        marker = Decode(remark);
        return IsComplete(marker)
            && string.Equals(marker!.Namespace, Namespace, StringComparison.Ordinal);
    }

    internal static bool TryDecodePin(string? remark, out AnnotationDecorationMarker? marker)
    {
        marker = Decode(remark);
        return IsComplete(marker)
            && string.Equals(marker!.Namespace, PinNamespace, StringComparison.Ordinal)
            && !string.IsNullOrWhiteSpace(marker.EntityId);
    }

    internal static bool IsDecoration(string? remark, string? text = null) =>
        TryDecode(remark, out _) || TryDecode(text, out _);

    internal static bool IsPin(string? remark) => TryDecodePin(remark, out _);

    private static bool IsComplete(AnnotationDecorationMarker? marker) =>
        marker is not null
        && !string.IsNullOrWhiteSpace(marker.ProjectId)
        && marker.AnnotationId != Guid.Empty
        && marker.RealizationId != Guid.Empty;

    internal static IReadOnlyList<AnnotationDecorationPlan> PlansFrom(
        IEnumerable<CaptureAnnotationDto> annotations,
        string liveProjectId,
        string? liveSceneId = null)
    {
        var plans = new List<AnnotationDecorationPlan>();
        if (string.IsNullOrWhiteSpace(liveProjectId))
        {
            return plans;
        }
        foreach (var annotation in annotations)
        {
            if (!string.Equals(annotation.Lifecycle, "active", StringComparison.Ordinal))
            {
                continue;
            }
            if (!string.Equals(annotation.ProjectId, liveProjectId, StringComparison.Ordinal))
            {
                continue;
            }
            if (!SameScene(liveSceneId, annotation.SceneId))
            {
                continue;
            }
            var length = annotation.EndFrame - annotation.StartFrame;
            plans.Add(new AnnotationDecorationPlan(
                annotation.ProjectId,
                annotation.AnnotationId,
                Math.Max(annotation.StartFrame, 0),
                Math.Max(length, 1),
                LabelOf(annotation.TranscriptSummary)));
        }
        return plans;
    }

    internal static bool SameScene(string? liveSceneId, string? captureSceneId)
    {
        if (string.IsNullOrWhiteSpace(liveSceneId) || string.IsNullOrWhiteSpace(captureSceneId))
        {
            return true;
        }
        if (string.Equals(liveSceneId, captureSceneId, StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }
        return Guid.TryParse(liveSceneId, out var live)
            && Guid.TryParse(captureSceneId, out var captured)
            && live == captured;
    }

    internal static string LabelOf(string? summary)
    {
        var trimmed = summary?.Trim() ?? string.Empty;
        if (trimmed.Length == 0)
        {
            return "メモ";
        }
        return trimmed.Length <= 40 ? trimmed : string.Concat(trimmed.AsSpan(0, 39), "…");
    }

    private static AnnotationDecorationMarker? Decode(string? remark)
    {
        if (string.IsNullOrEmpty(remark))
        {
            return null;
        }
        var start = remark.LastIndexOf(Start, StringComparison.Ordinal);
        if (start < 0)
        {
            return null;
        }
        var payloadStart = start + Start.Length;
        var end = remark.IndexOf(End, payloadStart, StringComparison.Ordinal);
        if (end < 0)
        {
            return null;
        }
        try
        {
            var payload = remark[payloadStart..end].Replace('-', '+').Replace('_', '/');
            payload = payload.PadRight(payload.Length + ((4 - payload.Length % 4) % 4), '=');
            return JsonSerializer.Deserialize<AnnotationDecorationMarker>(
                Encoding.UTF8.GetString(Convert.FromBase64String(payload)),
                BridgeJson.Options);
        }
        catch
        {
            return null;
        }
    }

    private static string Strip(string? remark)
    {
        if (string.IsNullOrEmpty(remark) || !TryDecode(remark, out _))
        {
            return remark ?? string.Empty;
        }
        var start = remark.LastIndexOf(Start, StringComparison.Ordinal);
        var end = remark.IndexOf(End, start + Start.Length, StringComparison.Ordinal);
        return remark.Remove(start, end + End.Length - start).TrimEnd();
    }
}

internal sealed record AnnotationDecorationPlan(
    string ProjectId,
    Guid AnnotationId,
    int Frame,
    int Length,
    string Label);
