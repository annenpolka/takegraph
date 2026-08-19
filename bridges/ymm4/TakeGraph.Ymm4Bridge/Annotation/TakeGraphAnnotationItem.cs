#if !TAKEGRAPH_YMM4_CONTRACT_STUB
using System.Windows.Media;
using YukkuriMovieMaker.Commons;
using YukkuriMovieMaker.Exo;
using YukkuriMovieMaker.Project.Items;

namespace TakeGraph.Ymm4Bridge;

/// Timeline-only annotation decoration. Not a <c>TextItem</c>: it is not
/// an <see cref="IVideoItem"/> and has no preview or render source.
[Item("TakeGraph メモ", "", "")]
public sealed class TakeGraphAnnotationItem : BaseItem
{
    private string label = "メモ";
    private Color itemColor = Color.FromRgb(0x2F, 0x80, 0xED);

    public override string Label => label;

    public override string Description => "TakeGraph annotation";

    internal void SetLabel(string value) =>
        Set(ref label, string.IsNullOrWhiteSpace(value) ? "メモ" : value, nameof(Label));

    public override Color ItemColor
    {
        get => itemColor;
        set => Set(ref itemColor, value);
    }

    public override TimeSpan OriginalContentLength => ContentLength;

    public override TimeSpan ContentLength =>
        TimeSpan.FromSeconds(Math.Max(Length, 1) / 30.0);

    protected override IEnumerable<IAnimatable> GetAnimatables() => [];

    public override IEnumerable<string> GetFiles() => [];

    public override void ReplaceFile(string from, string to)
    {
    }

    public override async IAsyncEnumerable<ExoItem> GetExoItemsAsync(
        ExoOutputDescription outputDescription)
    {
        await Task.CompletedTask.ConfigureAwait(false);
        yield break;
    }
}
#endif
