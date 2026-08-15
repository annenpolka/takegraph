using System.ComponentModel;
using System.Runtime.CompilerServices;

namespace TakeGraph.Ymm4Bridge;

public sealed class AnnotationRowViewModel : INotifyPropertyChanged
{
    internal AnnotationRowViewModel(CaptureAnnotationDto annotation)
    {
        Annotation = annotation;
    }

    internal CaptureAnnotationDto Annotation { get; private set; }

    public Guid AnnotationId => Annotation.AnnotationId;
    public int StartFrame => Annotation.StartFrame;
    public int EndFrame => Annotation.EndFrame;
    public string SceneId => Annotation.SceneId;
    public string ProjectId => Annotation.ProjectId;
    public string SourceFingerprint => Annotation.SourceFingerprint;
    public uint Fps => Annotation.Fps;
    public string Stability => Annotation.Stability;
    public string Lifecycle => Annotation.Lifecycle;

    public string DisplayLabel
    {
        get
        {
            var span = FormatFrame(Annotation.StartFrame, Annotation.Fps);
            if (Annotation.EndFrame != Annotation.StartFrame)
            {
                span += " → " + FormatFrame(Annotation.EndFrame, Annotation.Fps);
            }
            var label = $"{span}  {Annotation.Stability}  {Annotation.Lifecycle}";
            if (Annotation.DerivePhase is "queued" or "running")
            {
                label += "  起こし中";
            }
            else if (Annotation.DerivePhase == "failed")
            {
                label += "  起こし失敗";
            }
            else if (!string.IsNullOrWhiteSpace(Annotation.TranscriptSummary))
            {
                label += "  " + Annotation.TranscriptSummary;
            }
            return label;
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    internal void Replace(CaptureAnnotationDto annotation)
    {
        Annotation = annotation;
        OnPropertyChanged(string.Empty);
    }

    internal static string FormatFrame(int frame, uint fps)
    {
        if (fps == 0)
        {
            return $"f{frame}";
        }
        var seconds = frame / (double)fps;
        var minutes = (int)Math.Floor(seconds / 60);
        var remainder = seconds - (minutes * 60);
        return $"{minutes:00}:{remainder:00.00}";
    }

    private void OnPropertyChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}
