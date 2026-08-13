using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal sealed class ProjectOperationStore
{
    private readonly object gate = new();
    private readonly string root;
    private readonly Dictionary<Guid, CheckpointReceiptDto> checkpoints;
    private readonly Dictionary<Guid, RenderTaskDto> renders;
    private readonly Dictionary<Guid, NativeExtensionApplyResponseDto> nativeExtensions;
    private readonly Dictionary<Guid, ProjectInitializationReceiptDto> projectInitializations;
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
            projectInitializations = Load<ProjectInitializationReceiptDto>(
                Path.Combine(root, "project-initializations"),
                value => value.OperationId,
                ValidateProjectInitializationReceipt);
        }
        catch (Exception error)
        {
            checkpoints = [];
            renders = [];
            nativeExtensions = [];
            projectInitializations = [];
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

    internal bool TryGetProjectInitialization(
        Guid operationId,
        out ProjectInitializationReceiptDto? receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            return projectInitializations.TryGetValue(operationId, out receipt);
        }
    }

    internal IReadOnlyList<ProjectInitializationReceiptDto> ReadPendingProjectInitializations()
    {
        lock (gate)
        {
            EnsureHealthy();
            return projectInitializations.Values
                .Where(value => value.Status is "applying" or "recovery_required")
                .OrderBy(value => value.OperationId)
                .ToArray();
        }
    }

    internal void PutProjectInitialization(ProjectInitializationReceiptDto receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            ValidateProjectInitializationReceipt(receipt);
            var hasExisting = projectInitializations.TryGetValue(receipt.OperationId, out var existing);
            if (!hasExisting
                && !((receipt.Status == "applying"
                        && PreparedProjectInitializationEvidenceStage(receipt) == 0)
                    || receipt.Status == "failed"))
            {
                throw new BridgeConflictException(
                    $"Invalid initial project initialization receipt state: {receipt.Status}",
                    receipt.BeforeFingerprint);
            }
            if (hasExisting
                && existing is not null
                && (!string.Equals(existing.RequestDigest, receipt.RequestDigest, StringComparison.Ordinal)
                    || !string.Equals(existing.DriverProfileDigest, receipt.DriverProfileDigest, StringComparison.Ordinal)
                    || !string.Equals(existing.SourceProjectInstanceId, receipt.SourceProjectInstanceId, StringComparison.Ordinal)
                    || !string.Equals(existing.SourceProjectId, receipt.SourceProjectId, StringComparison.Ordinal)
                    || !string.Equals(existing.SourceSceneId, receipt.SourceSceneId, StringComparison.Ordinal)
                    || !string.Equals(existing.BeforeFingerprint, receipt.BeforeFingerprint, StringComparison.Ordinal)
                    || !string.Equals(existing.DestinationPath, receipt.DestinationPath, StringComparison.Ordinal)
                    || !string.Equals(existing.DestinationPathDigest, receipt.DestinationPathDigest, StringComparison.Ordinal)
                    || !string.Equals(existing.PredictedProjectId, receipt.PredictedProjectId, StringComparison.Ordinal)
                    || !string.Equals(existing.PredictedFingerprint, receipt.PredictedFingerprint, StringComparison.Ordinal)))
            {
                throw new BridgeConflictException(
                    "Project initialization operation ID is already bound to another request",
                    existing!.BeforeFingerprint);
            }
            if (existing is not null
                && !JsonSerializer.SerializeToUtf8Bytes(existing, BridgeJson.Options)
                    .SequenceEqual(JsonSerializer.SerializeToUtf8Bytes(receipt, BridgeJson.Options))
                && (!IsProjectInitializationTransitionAllowed(existing.Status, receipt.Status)
                    || !PreparedProjectInitializationEvidenceAdvances(existing, receipt)))
            {
                throw new BridgeConflictException(
                    $"Invalid project initialization receipt transition: {existing.Status} -> {receipt.Status}",
                    existing.BeforeFingerprint);
            }
            Persist(Path.Combine(root, "project-initializations"), receipt.OperationId, receipt);
            projectInitializations[receipt.OperationId] = receipt;
        }
    }

    private static bool IsProjectInitializationTransitionAllowed(string before, string after) =>
        before switch
        {
            "applying" => after is "applying" or "verified" or "recovery_required" or "failed",
            "recovery_required" => after is "applying" or "verified" or "recovery_required" or "failed",
            _ => false,
        };

    private static bool PreparedProjectInitializationEvidenceAdvances(
        ProjectInitializationReceiptDto before,
        ProjectInitializationReceiptDto after)
    {
        var beforeStage = PreparedProjectInitializationEvidenceStage(before);
        var afterStage = PreparedProjectInitializationEvidenceStage(after);
        if (before.PreparedTemporaryPath is not null
            && after.PreparedTemporaryPath is not null
            && !string.Equals(
                before.PreparedTemporaryPath,
                after.PreparedTemporaryPath,
                StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }
        if (beforeStage == 2
            && (!ApplyRequestDigest.Matches(
                    before.PreparedFileSha256,
                    after.PreparedFileSha256)
                || before.PreparedFileBytes != after.PreparedFileBytes))
        {
            return false;
        }
        if (afterStage >= beforeStage)
        {
            return afterStage - beforeStage <= 1;
        }
        // A path-only temporary name can be cleared only after the bridge
        // deleted that operation-owned file and sealed an authenticated
        // no-target-write terminal receipt.
        return beforeStage == 1 && afterStage == 0 && after.Status == "failed";
    }

    private static int PreparedProjectInitializationEvidenceStage(
        ProjectInitializationReceiptDto receipt)
    {
        if (receipt.PreparedTemporaryPath is null)
        {
            return 0;
        }
        return receipt.PreparedFileSha256 is null ? 1 : 2;
    }

    private static void ValidateProjectInitializationReceipt(ProjectInitializationReceiptDto receipt)
    {
        var preparedAllMissing = receipt.PreparedTemporaryPath is null
            && receipt.PreparedFileSha256 is null
            && receipt.PreparedFileBytes is null;
        var preparedPathOnly = receipt.PreparedTemporaryPath is not null
            && receipt.PreparedFileSha256 is null
            && receipt.PreparedFileBytes is null;
        var preparedComplete = receipt.PreparedTemporaryPath is not null
            && IsSha256(receipt.PreparedFileSha256)
            && receipt.PreparedFileBytes is > 0;
        var finalAllMissing = receipt.FileSha256 is null && receipt.FileBytes is null;
        var finalComplete = IsSha256(receipt.FileSha256) && receipt.FileBytes is > 0;
        if (receipt.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(receipt.RequestDigest)
            || string.IsNullOrWhiteSpace(receipt.DriverProfileDigest)
            || string.IsNullOrWhiteSpace(receipt.SourceProjectInstanceId)
            || string.IsNullOrWhiteSpace(receipt.SourceProjectId)
            || string.IsNullOrWhiteSpace(receipt.SourceSceneId)
            || string.IsNullOrWhiteSpace(receipt.BeforeFingerprint)
            || string.IsNullOrWhiteSpace(receipt.DestinationPath)
            || string.IsNullOrWhiteSpace(receipt.DestinationPathDigest)
            || string.IsNullOrWhiteSpace(receipt.PredictedProjectId)
            || string.IsNullOrWhiteSpace(receipt.PredictedFingerprint)
            || receipt.Status is not ("applying" or "verified" or "recovery_required" or "failed")
            || (!(preparedAllMissing || preparedPathOnly || preparedComplete))
            || (receipt.PreparedTemporaryPath is not null
                && !Ymm4Facade.IsOwnedProjectInitializationTemporaryPath(
                    receipt.DestinationPath,
                    receipt.PreparedTemporaryPath))
            || (!(finalAllMissing || finalComplete))
            || (receipt.Status == "applying"
                && (receipt.AfterSnapshot is not null
                    || !finalAllMissing
                    || receipt.Error is not null))
            || (receipt.Status == "verified"
                && (receipt.AfterSnapshot is null
                    || !preparedComplete
                    || !finalComplete
                    || !ApplyRequestDigest.Matches(
                        receipt.PreparedFileSha256,
                        receipt.FileSha256)
                    || receipt.PreparedFileBytes != receipt.FileBytes
                    || receipt.Error is not null
                    || receipt.AfterSnapshot.ProjectId != receipt.PredictedProjectId
                    || receipt.AfterSnapshot.SceneId != receipt.SourceSceneId
                    || receipt.AfterSnapshot.Fingerprint != receipt.PredictedFingerprint
                    || receipt.AfterSnapshot.ProjectPath != receipt.DestinationPath))
            || (receipt.Status == "recovery_required"
                && string.IsNullOrWhiteSpace(receipt.Error))
            || (receipt.Status == "failed"
                && (!preparedAllMissing
                    || !finalAllMissing
                    || receipt.AfterSnapshot is null
                    || receipt.AfterSnapshot.ProjectId != receipt.SourceProjectId
                    || receipt.AfterSnapshot.SceneId != receipt.SourceSceneId
                    || receipt.AfterSnapshot.Fingerprint != receipt.BeforeFingerprint
                    || receipt.AfterSnapshot.ProjectPath.Length != 0
                    || string.IsNullOrWhiteSpace(receipt.Error))))
        {
            throw new InvalidDataException(
                $"Project initialization operation record is malformed: {receipt.OperationId}");
        }
    }

    private static bool IsSha256(string? value) =>
        value is { Length: 64 }
        && value.All(character => Uri.IsHexDigit(character));

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
