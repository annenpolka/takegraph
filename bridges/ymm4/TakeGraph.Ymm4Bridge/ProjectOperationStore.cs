using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal sealed class ProjectOperationStore
{
    private readonly object gate = new();
    private readonly string root;
    private readonly Dictionary<Guid, CheckpointReceiptDto> checkpoints;
    private readonly Dictionary<Guid, RenderTaskDto> renders;
    private readonly Dictionary<Guid, NativeExtensionApplyResponseDto> nativeExtensions;
    private readonly Exception? loadError;

    internal ProjectOperationStore(string? storeRoot = null)
    {
        root = storeRoot ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "TakeGraph",
            "project-operations");
        try
        {
            checkpoints = Load<CheckpointReceiptDto>(
                Path.Combine(root, "checkpoints"),
                value => value.OperationId);
            renders = Load<RenderTaskDto>(
                Path.Combine(root, "renders"),
                value => value.TaskId,
                Ymm4Facade.ValidateStoredRenderTask);
            nativeExtensions = Load<NativeExtensionApplyResponseDto>(
                Path.Combine(root, "native-extensions"),
                value => value.OperationId,
                ValidateNativeExtensionReceipt);
        }
        catch (Exception error)
        {
            checkpoints = [];
            renders = [];
            nativeExtensions = [];
            loadError = error;
        }
    }

    internal bool TryGetCheckpoint(Guid operationId, out CheckpointReceiptDto? receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            return checkpoints.TryGetValue(operationId, out receipt);
        }
    }

    internal void PutCheckpoint(CheckpointReceiptDto receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            Persist(Path.Combine(root, "checkpoints"), receipt.OperationId, receipt);
            checkpoints[receipt.OperationId] = receipt;
        }
    }

    internal bool TryGetRender(Guid taskId, out RenderTaskDto? task)
    {
        lock (gate)
        {
            EnsureHealthy();
            return renders.TryGetValue(taskId, out task);
        }
    }

    internal IReadOnlyList<RenderTaskDto> ReadRenders()
    {
        lock (gate)
        {
            EnsureHealthy();
            return renders.Values.OrderBy(value => value.TaskId).ToArray();
        }
    }

    internal void PutRender(RenderTaskDto task)
    {
        lock (gate)
        {
            EnsureHealthy();
            Persist(Path.Combine(root, "renders"), task.TaskId, task);
            renders[task.TaskId] = task;
        }
    }

    internal bool TryGetNativeExtension(
        Guid operationId,
        out NativeExtensionApplyResponseDto? receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            return nativeExtensions.TryGetValue(operationId, out receipt);
        }
    }

    internal void PutNativeExtension(NativeExtensionApplyResponseDto receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            ValidateNativeExtensionReceipt(receipt);
            if (nativeExtensions.TryGetValue(receipt.OperationId, out var existing))
            {
                if (!string.Equals(existing.RequestDigest, receipt.RequestDigest, StringComparison.Ordinal)
                    || !string.Equals(existing.ProjectId, receipt.ProjectId, StringComparison.Ordinal)
                    || !string.Equals(existing.SceneId, receipt.SceneId, StringComparison.Ordinal)
                    || !string.Equals(
                        existing.BeforeFingerprint,
                        receipt.BeforeFingerprint,
                        StringComparison.Ordinal)
                    || !string.Equals(
                        existing.DescriptorCatalogDigest,
                        receipt.DescriptorCatalogDigest,
                        StringComparison.Ordinal)
                    || !string.Equals(
                        existing.DriverProfileDigest,
                        receipt.DriverProfileDigest,
                        StringComparison.Ordinal))
                {
                    throw new BridgeConflictException(
                        "Native-extension operation ID is already bound to another request",
                        existing.BeforeFingerprint);
                }
                if (existing.Status != "applying"
                    && !JsonSerializer.SerializeToUtf8Bytes(existing, BridgeJson.Options)
                        .SequenceEqual(JsonSerializer.SerializeToUtf8Bytes(receipt, BridgeJson.Options)))
                {
                    throw new BridgeConflictException(
                        $"Invalid native-extension receipt transition: {existing.Status} -> {receipt.Status}",
                        existing.AfterFingerprint);
                }
            }
            Persist(Path.Combine(root, "native-extensions"), receipt.OperationId, receipt);
            nativeExtensions[receipt.OperationId] = receipt;
        }
    }

    private static void ValidateNativeExtensionReceipt(NativeExtensionApplyResponseDto receipt)
    {
        if (receipt.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(receipt.RequestDigest)
            || string.IsNullOrWhiteSpace(receipt.ProjectId)
            || string.IsNullOrWhiteSpace(receipt.SceneId)
            || string.IsNullOrWhiteSpace(receipt.BeforeFingerprint)
            || string.IsNullOrWhiteSpace(receipt.AfterFingerprint)
            || string.IsNullOrWhiteSpace(receipt.DescriptorCatalogDigest)
            || string.IsNullOrWhiteSpace(receipt.DriverProfileDigest)
            || receipt.Status is not (
                "not_started" or "applying" or "verified" or "stale" or
                "rolled_back" or "recovery_required" or "failed")
            || receipt.Realizations is null
            || receipt.Verified != (receipt.Status == "verified")
            || ((receipt.Status is "applying" or "verified") && receipt.Error is not null)
            || (receipt.Status == "not_started"
                && (receipt.Realizations.Count != 0
                    || receipt.AfterFingerprint != receipt.BeforeFingerprint
                    || string.IsNullOrWhiteSpace(receipt.Error))))
        {
            throw new InvalidDataException(
                $"Native-extension operation record is malformed: {receipt.OperationId}");
        }
    }

    private static Dictionary<Guid, T> Load<T>(
        string directory,
        Func<T, Guid> getId,
        Action<T>? validate = null)
    {
        if (!Directory.Exists(directory))
        {
            return [];
        }
        var loaded = new Dictionary<Guid, T>();
        foreach (var path in Directory.EnumerateFiles(directory, "*.json", SearchOption.TopDirectoryOnly))
        {
            var value = JsonSerializer.Deserialize<T>(File.ReadAllText(path), BridgeJson.Options)
                ?? throw new InvalidDataException($"Project operation record is empty: {path}");
            var id = getId(value);
            validate?.Invoke(value);
            if (id == Guid.Empty
                || !string.Equals(
                    Path.GetFileNameWithoutExtension(path),
                    id.ToString("N"),
                    StringComparison.OrdinalIgnoreCase)
                || !loaded.TryAdd(id, value))
            {
                throw new InvalidDataException($"Project operation record is malformed: {path}");
            }
        }
        return loaded;
    }

    private static void Persist<T>(string directory, Guid id, T value)
    {
        Directory.CreateDirectory(directory);
        var path = Path.Combine(directory, $"{id:N}.json");
        var temporary = $"{path}.tmp-{Guid.NewGuid():N}";
        var bytes = JsonSerializer.SerializeToUtf8Bytes(value, BridgeJson.Options);
        using (var stream = new FileStream(
                   temporary,
                   FileMode.CreateNew,
                   FileAccess.Write,
                   FileShare.None,
                   4096,
                   FileOptions.WriteThrough))
        {
            stream.Write(bytes);
            stream.Flush(true);
        }
        File.Move(temporary, path, overwrite: true);
    }

    private void EnsureHealthy()
    {
        if (loadError is not null)
        {
            throw new BridgeUnavailableException(
                $"YMM4 project-operation store could not be loaded; refusing operation: {loadError.Message}");
        }
    }
}
