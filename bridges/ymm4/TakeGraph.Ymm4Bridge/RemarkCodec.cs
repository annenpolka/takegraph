using System.Text;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal static class RemarkCodec
{
    private const string Start = "[[takegraph:";
    private const string End = "]]";
    private const string MarkerNamespace = "takegraph/v2";

    internal static void ValidateRoundTrip()
    {
        var marker = new NativeVoiceMarker(
            MarkerNamespace,
            "project-a",
            "utt-01",
            Guid.Parse("11111111-2222-3333-4444-555555555555"),
            4);
        var encoded = Append("user remark", marker);
        if (!encoded.StartsWith("user remark", StringComparison.Ordinal)
            || !TryDecode(encoded, out var decoded)
            || decoded != marker)
        {
            throw new InvalidOperationException("YMM4 Remark identity codec round-trip failed");
        }
    }

    internal static string Append(string? remark, NativeVoiceMarker marker)
    {
        var existing = Remove(remark, out _).TrimEnd();
        var json = JsonSerializer.Serialize(marker, BridgeJson.Options);
        var payload = Convert.ToBase64String(Encoding.UTF8.GetBytes(json))
            .TrimEnd('=')
            .Replace('+', '-')
            .Replace('/', '_');
        return string.IsNullOrEmpty(existing)
            ? $"{Start}{payload}{End}"
            : $"{existing}{Environment.NewLine}{Start}{payload}{End}";
    }

    internal static bool TryDecode(string? remark, out NativeVoiceMarker? marker)
    {
        _ = Remove(remark, out marker);
        return marker is not null
            && string.Equals(marker.Namespace, MarkerNamespace, StringComparison.Ordinal)
            && !string.IsNullOrWhiteSpace(marker.ProjectId)
            && !string.IsNullOrWhiteSpace(marker.EntityId)
            && marker.RealizationId != Guid.Empty;
    }

    internal static string Remove(string? remark, out NativeVoiceMarker? marker)
    {
        marker = null;
        if (string.IsNullOrEmpty(remark))
        {
            return remark ?? string.Empty;
        }
        var start = remark.LastIndexOf(Start, StringComparison.Ordinal);
        if (start < 0)
        {
            return remark;
        }
        var payloadStart = start + Start.Length;
        var end = remark.IndexOf(End, payloadStart, StringComparison.Ordinal);
        if (end < 0)
        {
            return remark;
        }
        try
        {
            var payload = remark[payloadStart..end].Replace('-', '+').Replace('_', '/');
            payload = payload.PadRight(payload.Length + ((4 - payload.Length % 4) % 4), '=');
            marker = JsonSerializer.Deserialize<NativeVoiceMarker>(
                Encoding.UTF8.GetString(Convert.FromBase64String(payload)),
                BridgeJson.Options);
        }
        catch
        {
            marker = null;
            return remark;
        }
        return remark.Remove(start, end + End.Length - start).TrimEnd();
    }
}
