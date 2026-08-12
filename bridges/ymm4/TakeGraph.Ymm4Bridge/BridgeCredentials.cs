using System.Security.Cryptography;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal sealed record BridgeCredentials(string Endpoint, string Token)
{
    internal static string CredentialsPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "TakeGraph",
        "ymm4-bridge.json");

    internal static BridgeCredentials LoadOrCreate()
    {
        var configured = Environment.GetEnvironmentVariable("TAKEGRAPH_YMM4_TOKEN");
        if (!string.IsNullOrWhiteSpace(configured))
        {
            var environmentCredentials = new BridgeCredentials(BridgeHost.BaseUrl, configured);
            environmentCredentials.Save();
            return environmentCredentials;
        }

        try
        {
            if (File.Exists(CredentialsPath))
            {
                var existing = JsonSerializer.Deserialize<BridgeCredentials>(
                    File.ReadAllText(CredentialsPath), BridgeJson.Options);
                if (existing is { Token.Length: > 0 })
                {
                    return existing with { Endpoint = BridgeHost.BaseUrl };
                }
            }
        }
        catch
        {
            // Regenerate an unreadable or stale local credential file.
        }

        var token = Convert.ToHexString(RandomNumberGenerator.GetBytes(32)).ToLowerInvariant();
        var created = new BridgeCredentials(BridgeHost.BaseUrl, token);
        created.Save();
        return created;
    }

    private void Save()
    {
        var directory = Path.GetDirectoryName(CredentialsPath)!;
        Directory.CreateDirectory(directory);
        var temporary = CredentialsPath + ".tmp";
        File.WriteAllText(temporary, JsonSerializer.Serialize(this, BridgeJson.Options));
        File.Move(temporary, CredentialsPath, true);
    }
}
