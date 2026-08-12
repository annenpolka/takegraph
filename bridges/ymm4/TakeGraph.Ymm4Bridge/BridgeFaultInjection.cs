namespace TakeGraph.Ymm4Bridge;

internal static class BridgeFaultInjection
{
    private const string EnvironmentVariable = "TAKEGRAPH_YMM4_FAULT_POINT";

    internal static void ThrowIf(string point)
    {
        var configured = Environment.GetEnvironmentVariable(EnvironmentVariable);
        if (string.IsNullOrWhiteSpace(configured))
        {
            return;
        }
        var enabled = configured.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Contains(point, StringComparer.Ordinal);
        if (enabled)
        {
            throw new BridgeSimulatedCrashException(
                $"Simulated YMM4 bridge process exit at fault point '{point}'");
        }
    }
}

internal sealed class BridgeSimulatedCrashException(string message) : Exception(message);
