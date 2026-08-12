using System.Text;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal static class MarkerCodec
{
    private const string Start = "\U000E0001";
    private const string End = "\U000E007F";

    internal static string Append(string caption, ManagedMarker marker)
    {
        var json = JsonSerializer.Serialize(marker, BridgeJson.Options);
        var payload = Convert.ToBase64String(Encoding.UTF8.GetBytes(json))
            .TrimEnd('=')
            .Replace('+', '-')
            .Replace('/', '_');
        var encoded = new StringBuilder(Start);
        foreach (var character in $"takegraph:{payload}")
        {
            encoded.Append(char.ConvertFromUtf32(0xE0000 + character));
        }
        encoded.Append(End);
        return caption + encoded;
    }

    internal static bool TryDecode(string? value, out string caption, out ManagedMarker? marker)
    {
        caption = value ?? string.Empty;
        marker = null;
        if (string.IsNullOrEmpty(value))
        {
            return false;
        }

        var start = value.IndexOf(Start, StringComparison.Ordinal);
        if (start < 0)
        {
            return false;
        }

        caption = value[..start];
        var encoded = value[(start + Start.Length)..];
        var ascii = new StringBuilder();
        for (var index = 0; index < encoded.Length;)
        {
            var codepoint = char.ConvertToUtf32(encoded, index);
            index += char.IsSurrogatePair(encoded, index) ? 2 : 1;
            if (codepoint == 0xE007F)
            {
                break;
            }
            if (codepoint < 0xE0000 || codepoint > 0xE007E)
            {
                return false;
            }
            ascii.Append((char)(codepoint - 0xE0000));
        }

        const string prefix = "takegraph:";
        if (!ascii.ToString().StartsWith(prefix, StringComparison.Ordinal))
        {
            return false;
        }

        try
        {
            var payload = ascii.ToString()[prefix.Length..].Replace('-', '+').Replace('_', '/');
            payload = payload.PadRight(payload.Length + ((4 - payload.Length % 4) % 4), '=');
            var json = Encoding.UTF8.GetString(Convert.FromBase64String(payload));
            marker = JsonSerializer.Deserialize<ManagedMarker>(json, BridgeJson.Options);
            return marker is not null;
        }
        catch
        {
            marker = null;
            return false;
        }
    }
}
