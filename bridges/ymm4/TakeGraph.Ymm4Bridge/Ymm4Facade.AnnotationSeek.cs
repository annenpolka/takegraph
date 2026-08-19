using System.Windows;

namespace TakeGraph.Ymm4Bridge;

internal sealed record AnnotationSeekRequest(
    string ProjectId,
    string SceneId,
    string ExpectedFingerprint,
    int Frame);

internal sealed record AnnotationSeekResult(
    int RequestedFrame,
    int ActualFrame,
    string SourceFingerprint,
    bool ProjectDirtyStateChanged);

internal sealed partial class Ymm4Facade
{
    /// Source-bound preview seek for an annotation jump. This is not a
    /// canonical edit: it must not flip project dirty state or selection.
    internal async Task<AnnotationSeekResult> SeekAnnotationFrameAsync(
        AnnotationSeekRequest request)
    {
        ValidateObservationRuntime();
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            var observed = await Application.Current.Dispatcher
                .InvokeAsync(ObserveCurrentSceneComposition);
            EnsureAnnotationSeekBinding(request, observed);

            var preview = await Application.Current.Dispatcher.InvokeAsync(RequirePreviewViewModel);
            var dirtyBefore = await Application.Current.Dispatcher.InvokeAsync(ReadProjectDirty);
            var selectionBefore = await Application.Current.Dispatcher.InvokeAsync(ReadSelectionDigest);

            var actual = await WaitForPreviewFrameAsync(preview, request.Frame, observed.Fps)
                .ConfigureAwait(false);
            if (!PreviewFrameSettled(actual, request.Frame) || actual is null)
            {
                throw new BridgeUnavailableException(
                    $"YMM4 preview seek did not settle at frame {request.Frame}");
            }

            var dirtyAfter = await Application.Current.Dispatcher.InvokeAsync(ReadProjectDirty);
            var selectionAfter = await Application.Current.Dispatcher.InvokeAsync(ReadSelectionDigest);
            EnsureAnnotationSeekDidNotMutate(dirtyBefore, dirtyAfter, selectionBefore, selectionAfter);

            return new AnnotationSeekResult(
                request.Frame,
                actual.Value,
                observed.SourceFingerprint,
                false);
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal static void EnsureAnnotationSeekBinding(
        AnnotationSeekRequest request,
        SceneCompositionSnapshotDto observed)
    {
        if (string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint))
        {
            throw new BridgeValidationException(
                "annotation jump requires project, scene, and source fingerprint");
        }
        if (request.Frame < 0)
        {
            throw new BridgeValidationException("annotation jump frame must be zero or greater");
        }
        if (!string.Equals(request.ProjectId, observed.ProjectId, StringComparison.Ordinal)
            || !string.Equals(request.SceneId, observed.SceneId, StringComparison.Ordinal)
            || !string.Equals(
                request.ExpectedFingerprint,
                observed.SourceFingerprint,
                StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "The active YMM4 source is not the annotation's captured source",
                observed.SourceFingerprint);
        }
    }

    internal static void EnsureAnnotationSeekDidNotMutate(
        bool dirtyBefore,
        bool dirtyAfter,
        string selectionBefore,
        string selectionAfter)
    {
        if (dirtyBefore != dirtyAfter
            || !string.Equals(selectionBefore, selectionAfter, StringComparison.Ordinal))
        {
            throw new BridgeValidationException(
                "annotation jump changed YMM4 dirty or selection state");
        }
    }
}
