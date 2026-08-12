using YukkuriMovieMaker.Plugin;

namespace TakeGraph.Ymm4Bridge;

public sealed class TakeGraphToolPlugin : IToolPlugin
{
    private static readonly Lazy<BridgeHost> Host = new(CreateAndStart);

    public TakeGraphToolPlugin()
    {
        _ = Host.Value;
    }

    public string Name => "TakeGraph Bridge";
    public Type ViewModelType => typeof(TakeGraphBridgeViewModel);
    public Type ViewType => typeof(TakeGraphBridgeView);

    internal static BridgeHost Current => Host.Value;

    private static BridgeHost CreateAndStart()
    {
        var host = new BridgeHost();
        host.Start();
        System.Windows.Application.Current.Exit += (_, _) => host.Dispose();
        return host;
    }
}
