using System.Text;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal static class NativeExtensionRemarkCodec
{
    private const string Start = "[[takegraph-native-extension:";
    private const string End = "]]";
    private const string MarkerNamespace = "takegraph/native-extension/v1";

    internal static string Append(string? remark, NativeExtensionMarker marker)
    {
        if (!string.Equals(marker.Namespace, MarkerNamespace, StringComparison.Ordinal))
        {
            throw new BridgeValidationException("Invalid native-extension marker namespace");
        }
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

    internal static bool TryDecode(string? remark, out NativeExtensionMarker? marker)
    {
        _ = Remove(remark, out marker);
        return marker is not null
            && string.Equals(marker.Namespace, MarkerNamespace, StringComparison.Ordinal)
            && !string.IsNullOrWhiteSpace(marker.ProjectId)
            && !string.IsNullOrWhiteSpace(marker.EntityId)
            && !string.IsNullOrWhiteSpace(marker.LogicalKey)
            && !string.IsNullOrWhiteSpace(marker.Kind)
            && marker.RealizationId != Guid.Empty
            && marker.PartIndex >= 0
            && marker.PartCount > marker.PartIndex
            && marker.Effects is not null
            && marker.Effects.All(pair =>
                !string.IsNullOrWhiteSpace(pair.Key)
                && pair.Value is not null
                && string.Equals(pair.Key, pair.Value.EffectInstanceId, StringComparison.Ordinal)
                && pair.Value.RealizationId != Guid.Empty
                && !string.IsNullOrWhiteSpace(pair.Value.DescriptorId)
                && !string.IsNullOrWhiteSpace(pair.Value.StableTypeId)
                && !string.IsNullOrWhiteSpace(pair.Value.Collection)
                && pair.Value.Index >= 0);
    }

    internal static NativeExtensionMarker Create(
        string projectId,
        string entityId,
        ulong revision,
        string logicalKey,
        Guid realizationId,
        string kind,
        int partIndex,
        int partCount,
        IReadOnlyDictionary<string, NativeExtensionEffectMarker>? effects = null,
        string? descriptorId = null)
    {
        return new NativeExtensionMarker(
            MarkerNamespace,
            projectId,
            entityId,
            revision,
            logicalKey,
            realizationId,
            kind,
            partIndex,
            partCount,
            effects ?? new Dictionary<string, NativeExtensionEffectMarker>(StringComparer.Ordinal),
            descriptorId);
    }

    internal static string Remove(string? remark, out NativeExtensionMarker? marker)
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
            marker = JsonSerializer.Deserialize<NativeExtensionMarker>(
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
