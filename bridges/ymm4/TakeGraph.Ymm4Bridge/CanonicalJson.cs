using System.Security.Cryptography;
using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal static class CanonicalJson
{
    private static readonly JsonSerializerOptions StringOptions = new()
    {
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
        WriteIndented = false,
    };

    internal static string Sha256(string domain, JsonElement value)
    {
        using var stream = new MemoryStream();
        Write(value, stream);
        using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        hasher.AppendData(Encoding.UTF8.GetBytes(domain));
        hasher.AppendData([0]);
        hasher.AppendData(stream.GetBuffer().AsSpan(0, checked((int)stream.Length)));
        return $"sha256:{Convert.ToHexStringLower(hasher.GetHashAndReset())}";
    }

    internal static string Sha256<T>(string domain, T value)
    {
        using var document = JsonDocument.Parse(JsonSerializer.SerializeToUtf8Bytes(
            value,
            BridgeJson.Options));
        return Sha256(domain, document.RootElement);
    }

    internal static string Canonicalize(JsonElement value)
    {
        using var stream = new MemoryStream();
        Write(value, stream);
        return Encoding.UTF8.GetString(
            stream.GetBuffer().AsSpan(0, checked((int)stream.Length)));
    }

    private static void Write(JsonElement value, Stream output)
    {
        switch (value.ValueKind)
        {
            case JsonValueKind.Null:
                WriteAscii(output, "null");
                break;
            case JsonValueKind.True:
                WriteAscii(output, "true");
                break;
            case JsonValueKind.False:
                WriteAscii(output, "false");
                break;
            case JsonValueKind.Number:
                WriteAscii(output, value.GetRawText());
                break;
            case JsonValueKind.String:
                WriteUtf8(output, JsonSerializer.Serialize(value.GetString(), StringOptions));
                break;
            case JsonValueKind.Array:
                output.WriteByte((byte)'[');
                var firstItem = true;
                foreach (var item in value.EnumerateArray())
                {
                    if (!firstItem)
                    {
                        output.WriteByte((byte)',');
                    }
                    Write(item, output);
                    firstItem = false;
                }
                output.WriteByte((byte)']');
                break;
            case JsonValueKind.Object:
                output.WriteByte((byte)'{');
                var firstProperty = true;
                foreach (var property in value.EnumerateObject()
                             .OrderBy(property => property.Name, StringComparer.Ordinal))
                {
                    if (!firstProperty)
                    {
                        output.WriteByte((byte)',');
                    }
                    WriteUtf8(output, JsonSerializer.Serialize(property.Name, StringOptions));
                    output.WriteByte((byte)':');
                    Write(property.Value, output);
                    firstProperty = false;
                }
                output.WriteByte((byte)'}');
                break;
            default:
                throw new BridgeValidationException(
                    $"Unsupported JSON value in canonical plan: {value.ValueKind}");
        }
    }

    private static void WriteAscii(Stream output, string value)
    {
        foreach (var character in value)
        {
            if (character > 0x7f)
            {
                throw new InvalidOperationException("Expected ASCII canonical value");
            }
            output.WriteByte(checked((byte)character));
        }
    }

    private static void WriteUtf8(Stream output, string value)
    {
        var bytes = Encoding.UTF8.GetBytes(value);
        output.Write(bytes);
    }
}
