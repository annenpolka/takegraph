using System.Reflection;
using System.Windows;

namespace TakeGraph.Ymm4Bridge;

internal sealed partial class Ymm4Facade
{
    internal LiveAnnotationScope? TryReadLiveAnnotationScope()
    {
        try
        {
            var main = RequireMainViewModel();
            var timelineViewModel = GetMember(main, "ActiveTimelineViewModel");
            if (timelineViewModel is null)
            {
                return null;
            }
            var projectPath = GetString(main, "ProjectFilePath", "ProjectPath");
            if (string.IsNullOrWhiteSpace(projectPath))
            {
                return null;
            }
            var projectId = Hash($"project|{projectPath}");
            if (string.IsNullOrWhiteSpace(projectId))
            {
                return null;
            }
            var sceneId = GetString(timelineViewModel, "ID", "Id", "SceneId");
            return new LiveAnnotationScope(
                projectId,
                string.IsNullOrWhiteSpace(sceneId) ? null : sceneId);
        }
        catch (Exception)
        {
            return null;
        }
    }

    internal async Task SyncAnnotationDecorationsAsync(IReadOnlyList<CaptureAnnotationDto> annotations)
    {
        ValidateMutationRuntime();
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            await Application.Current.Dispatcher.InvokeAsync(() =>
                SyncAnnotationDecorations(annotations));
        }
        catch (BridgeUnavailableException)
        {
            // No open timeline. The panel overlay remains the fallback.
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal static void SyncAnnotationDecorations(
        IReadOnlyList<CaptureAnnotationDto> annotations,
        IReadOnlyList<RawItem> items,
        string liveProjectId,
        string? liveSceneId,
        Action<object[]> add,
        Action<object[]> delete,
        Action<RawItem, AnnotationDecorationPlan> update,
        Func<AnnotationDecorationPlan, object> create)
    {
        var desired = AnnotationDecorationCodec.PlansFrom(annotations, liveProjectId, liveSceneId);
        var desiredIds = desired.Select(plan => plan.AnnotationId).ToHashSet();
        var existing = new Dictionary<Guid, RawItem>();
        var stale = new List<object>();
        foreach (var item in items)
        {
            if ((!AnnotationDecorationCodec.TryDecode(item.Remark, out var marker)
                    && !AnnotationDecorationCodec.TryDecode(item.Text, out marker))
                || marker is null)
            {
                continue;
            }
            if (IsLegacyTextDecoration(item))
            {
                stale.Add(item.Item);
                continue;
            }
            if (!existing.ContainsKey(marker.AnnotationId))
            {
                existing[marker.AnnotationId] = item;
            }
        }

        var removals = existing
            .Where(pair => !desiredIds.Contains(pair.Key))
            .Select(pair => pair.Value.Item)
            .Concat(stale)
            .ToArray();
        if (removals.Length > 0)
        {
            delete(removals);
        }

        var additions = new List<object>();
        foreach (var plan in desired)
        {
            if (existing.TryGetValue(plan.AnnotationId, out var current))
            {
                if (current.Frame != plan.Frame
                    || current.Layer != AnnotationDecorationCodec.Layer
                    || current.Length != plan.Length)
                {
                    update(current, plan);
                }
                continue;
            }
            additions.Add(create(plan));
        }
        if (additions.Count > 0)
        {
            add(additions.ToArray());
        }
    }

    private void SyncAnnotationDecorations(IReadOnlyList<CaptureAnnotationDto> annotations)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var timeline = GetField(timelineViewModel, "timeline")
            ?? GetMember(timelineViewModel, "Timeline")
            ?? timelineViewModel;
        var items = ReadItems(timelineViewModel);
        var timelineDomain = GetField(timelineViewModel, "timeline") ?? timeline;
        var sceneId = GetString(timelineDomain, "ID", "Id", "SceneId");
        if (string.IsNullOrWhiteSpace(sceneId))
        {
            sceneId = GetString(timelineViewModel, "ID", "Id", "SceneId");
        }
        var mainModel = RequireMainModel(main);
        SyncAnnotationDecorations(
            annotations,
            items,
            CurrentProjectId(),
            sceneId,
            add => AddDecorationItems(mainModel, timeline, add),
            delete => InvokeItemsMethod(timeline, "DeleteItems", delete),
            (item, plan) => ApplyDecorationPlacement(item.Item, plan),
            CreateDecorationItem);
    }

    private static void AddDecorationItems(object mainModel, object timeline, object[] items)
    {
        try
        {
            InvokeItemsMethod(timeline, "AddItems", items);
            return;
        }
        catch (Exception timelineError)
        {
            try
            {
                foreach (var item in items)
                {
                    var frame = GetInt(item, item, "Frame");
                    InvokeNamed(
                        mainModel,
                        "AddItems",
                        [frame, AnnotationDecorationCodec.Layer, new[] { item }]);
                }
            }
            catch (Exception mainError)
            {
                throw new BridgeUnavailableException(
                    $"annotation decoration insert failed: {timelineError.Message}; {mainError.Message}");
            }
        }
    }

    private static void InvokeNamed(object target, string name, object[] arguments)
    {
        var method = target.GetType()
            .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .FirstOrDefault(candidate =>
                candidate.Name == name && candidate.GetParameters().Length == arguments.Length)
            ?? throw new MissingMethodException(target.GetType().FullName, name);
        try
        {
            method.Invoke(target, arguments);
        }
        catch (TargetInvocationException error)
        {
            throw error.InnerException ?? error;
        }
    }

    private static object CreateDecorationItem(AnnotationDecorationPlan plan)
    {
#if TAKEGRAPH_YMM4_CONTRACT_STUB
        throw new BridgeUnavailableException("annotation decorations require the live YMM4 contract");
#else
        var item = new TakeGraphAnnotationItem();
        ApplyDecorationPlacement(item, plan);
        return item;
#endif
    }

    private static void ApplyDecorationPlacement(object item, AnnotationDecorationPlan plan)
    {
        var marker = new AnnotationDecorationMarker(
            AnnotationDecorationCodec.Namespace,
            plan.ProjectId,
            plan.AnnotationId,
            RealizationIdFor(plan.AnnotationId));
#if !TAKEGRAPH_YMM4_CONTRACT_STUB
        if (item is TakeGraphAnnotationItem annotation)
        {
            annotation.SetLabel(plan.Label);
        }
#endif
        SetRequired(item, plan.Frame, "Frame");
        SetRequired(item, AnnotationDecorationCodec.Layer, "Layer");
        SetRequired(item, plan.Length, "Length");
        SetRequired(item, AnnotationDecorationCodec.Append(null, marker), "Remark");
    }

    private static bool IsLegacyTextDecoration(RawItem item) =>
        item.TypeName.Contains("TextItem", StringComparison.Ordinal);

    private static Guid RealizationIdFor(Guid annotationId)
    {
        var bytes = annotationId.ToByteArray();
        bytes[8] = (byte)((bytes[8] & 0x3f) | 0x80);
        bytes[7] = (byte)((bytes[7] & 0x0f) | 0x40);
        return new Guid(bytes);
    }
}
