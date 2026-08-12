using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal sealed class ReceiptStore
{
    private static readonly HashSet<string> CurrentPropertyNames = new(StringComparer.Ordinal)
    {
        "operationId",
        "requestDigest",
        "projectId",
        "sceneId",
        "expectedFingerprint",
        "status",
        "beforeFingerprint",
        "afterFingerprint",
        "appliedItems",
        "verified",
        "error",
    };

    private static readonly HashSet<string> LegacyPropertyNames = new(StringComparer.Ordinal)
    {
        "operationId",
        "status",
        "beforeFingerprint",
        "afterFingerprint",
        "appliedItems",
        "verified",
        "error",
    };

    private readonly object gate = new();
    private readonly string path;
    private readonly Dictionary<Guid, OperationReceiptDto> receipts;
    private readonly Dictionary<Guid, LegacyOperationReceiptDto> legacyReceipts;
    private readonly Exception? loadError;

    internal ReceiptStore(string? journalPath = null)
    {
        path = journalPath ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "TakeGraph",
            "ymm4-receipts.json");
        try
        {
            var loaded = Load(path);
            receipts = loaded.Receipts;
            legacyReceipts = loaded.LegacyReceipts;
        }
        catch (Exception error)
        {
            receipts = [];
            legacyReceipts = [];
            loadError = error;
        }
    }

    internal bool TryGet(Guid operationId, out OperationReceiptDto? receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            RejectLegacyOperationId(operationId);
            return receipts.TryGetValue(operationId, out receipt);
        }
    }

    internal IReadOnlyList<OperationReceiptDto> ReadAll()
    {
        lock (gate)
        {
            EnsureHealthy();
            return receipts.Values.OrderBy(value => value.OperationId).ToArray();
        }
    }

    internal void Put(OperationReceiptDto receipt)
    {
        lock (gate)
        {
            EnsureHealthy();
            RejectLegacyOperationId(receipt.OperationId);
            Validate(receipt.OperationId, receipt, $"operation {receipt.OperationId}");
            if (receipts.TryGetValue(receipt.OperationId, out var existing)
                && (!string.Equals(existing.RequestDigest, receipt.RequestDigest, StringComparison.Ordinal)
                    || !string.Equals(existing.ProjectId, receipt.ProjectId, StringComparison.Ordinal)
                    || !string.Equals(existing.SceneId, receipt.SceneId, StringComparison.Ordinal)
                    || !string.Equals(
                        existing.ExpectedFingerprint,
                        receipt.ExpectedFingerprint,
                        StringComparison.Ordinal)))
            {
                throw new BridgeConflictException(
                    "Receipt operation ID is already bound to another request",
                    existing.BeforeFingerprint);
            }
            if (existing is not null && !CanTransition(existing.Status, receipt.Status))
            {
                throw new BridgeConflictException(
                    $"Invalid receipt state transition: {existing.Status} -> {receipt.Status}",
                    existing.AfterFingerprint);
            }
            var updated = new Dictionary<Guid, OperationReceiptDto>(receipts)
            {
                [receipt.OperationId] = receipt,
            };
            Persist(updated);
            receipts[receipt.OperationId] = receipt;
        }
    }

    private static ReceiptLoadResult Load(string path)
    {
        var legacyPath = LegacyPath(path);
        var legacy = LoadLegacyArchive(legacyPath);
        var loaded = new Dictionary<Guid, OperationReceiptDto>();
        if (!File.Exists(path))
        {
            return new ReceiptLoadResult(loaded, legacy);
        }

        using var document = JsonDocument.Parse(File.ReadAllBytes(path), new JsonDocumentOptions
        {
            AllowTrailingCommas = false,
            CommentHandling = JsonCommentHandling.Disallow,
        });
        if (document.RootElement.ValueKind != JsonValueKind.Object)
        {
            throw new InvalidDataException("YMM4 receipt journal is empty or invalid");
        }

        var migrated = false;
        var storageIds = new HashSet<Guid>();
        foreach (var property in document.RootElement.EnumerateObject())
        {
            if (!Guid.TryParse(property.Name, out var operationId)
                || !storageIds.Add(operationId)
                || property.Value.ValueKind != JsonValueKind.Object)
            {
                throw new InvalidDataException($"YMM4 receipt journal is malformed: {path}");
            }

            var propertyNames = property.Value.EnumerateObject()
                .Select(value => value.Name)
                .ToArray();
            if (propertyNames.Distinct(StringComparer.Ordinal).Count() != propertyNames.Length)
            {
                throw new InvalidDataException($"YMM4 receipt journal has duplicate fields: {path}");
            }

            var hasAnyBinding = propertyNames.Any(IsBindingProperty);
            var hasAllBindings = new[]
            {
                "requestDigest", "projectId", "sceneId", "expectedFingerprint",
            }.All(name => propertyNames.Contains(name, StringComparer.Ordinal));
            if (hasAnyBinding && !hasAllBindings)
            {
                throw new InvalidDataException($"YMM4 receipt journal has a partial request binding: {path}");
            }

            if (hasAllBindings)
            {
                if (propertyNames.Length != CurrentPropertyNames.Count
                    || propertyNames.Any(name => !CurrentPropertyNames.Contains(name)))
                {
                    throw new InvalidDataException($"YMM4 receipt has an unknown shape: {path}");
                }
                var receipt = property.Value.Deserialize<OperationReceiptDto>(BridgeJson.Options)
                    ?? throw new InvalidDataException($"YMM4 receipt journal is malformed: {path}");
                Validate(operationId, receipt, path);
                loaded.Add(operationId, receipt);
                continue;
            }

            if (propertyNames.Length != LegacyPropertyNames.Count
                || propertyNames.Any(name => !LegacyPropertyNames.Contains(name)))
            {
                throw new InvalidDataException($"YMM4 legacy receipt has an unknown shape: {path}");
            }
            var legacyReceipt = property.Value.Deserialize<LegacyOperationReceiptDto>(BridgeJson.Options)
                ?? throw new InvalidDataException($"YMM4 legacy receipt is malformed: {path}");
            ValidateLegacy(operationId, legacyReceipt, path);
            if (legacy.TryGetValue(operationId, out var archived)
                && !JsonSerializer.SerializeToUtf8Bytes(archived, BridgeJson.Options)
                    .SequenceEqual(JsonSerializer.SerializeToUtf8Bytes(legacyReceipt, BridgeJson.Options)))
            {
                throw new InvalidDataException(
                    $"YMM4 legacy receipt archive conflicts for operation {operationId}");
            }
            legacy[operationId] = legacyReceipt;
            migrated = true;
        }

        if (migrated)
        {
            // Archive first. A crash before rewriting the current journal merely
            // repeats this exact, idempotent migration on the next startup.
            PersistFile(legacyPath, legacy);
            PersistFile(path, loaded);
        }
        return new ReceiptLoadResult(loaded, legacy);
    }

    private static bool IsBindingProperty(string name) => name is
        "requestDigest" or "projectId" or "sceneId" or "expectedFingerprint";

    private static string LegacyPath(string receiptPath) => Path.Combine(
        Path.GetDirectoryName(receiptPath)!,
        $"{Path.GetFileNameWithoutExtension(receiptPath)}.legacy-v1.json");

    private static Dictionary<Guid, LegacyOperationReceiptDto> LoadLegacyArchive(string archivePath)
    {
        if (!File.Exists(archivePath))
        {
            return [];
        }
        var loaded = JsonSerializer.Deserialize<Dictionary<Guid, LegacyOperationReceiptDto>>(
            File.ReadAllBytes(archivePath), BridgeJson.Options)
            ?? throw new InvalidDataException("YMM4 legacy receipt archive is empty or invalid");
        foreach (var (operationId, receipt) in loaded)
        {
            ValidateLegacy(operationId, receipt, archivePath);
        }
        return loaded;
    }

    private static void ValidateLegacy(
        Guid storageId,
        LegacyOperationReceiptDto receipt,
        string source)
    {
        if (storageId == Guid.Empty
            || receipt.OperationId != storageId
            || receipt.Status is not ("verified" or "failed")
            || !IsSha256(receipt.BeforeFingerprint)
            || !IsSha256(receipt.AfterFingerprint)
            || receipt.AppliedItems is null
            || receipt.Verified != (receipt.Status == "verified")
            || (receipt.Status == "verified" && receipt.Error is not null))
        {
            throw new InvalidDataException($"YMM4 legacy receipt archive is malformed: {source}");
        }
        foreach (var item in receipt.AppliedItems)
        {
            if (string.IsNullOrWhiteSpace(item.EntityId)
                || item.Kind is not ("audio" or "caption")
                || item.Frame < 0
                || item.Layer < 0
                || item.Length < 0
                || !IsSha256(item.ArtifactHash))
            {
                throw new InvalidDataException($"YMM4 legacy receipt item is malformed: {source}");
            }
        }
    }

    private static void Validate(
        Guid storageId,
        OperationReceiptDto receipt,
        string source)
    {
        if (storageId == Guid.Empty
            || receipt.OperationId != storageId
            || !IsSha256(receipt.RequestDigest)
            || string.IsNullOrWhiteSpace(receipt.ProjectId)
            || string.IsNullOrWhiteSpace(receipt.SceneId)
            || !IsSha256(receipt.ExpectedFingerprint)
            || !IsSha256(receipt.BeforeFingerprint)
            || !IsSha256(receipt.AfterFingerprint)
            || receipt.Status is not (
                "not_started" or "applying" or "verified" or "rolled_back" or
                "failed" or "recovery_required")
            || receipt.AppliedItems is null
            || receipt.Verified != (receipt.Status == "verified")
            || ((receipt.Status is "applying" or "verified") && receipt.Error is not null)
            || (receipt.Status == "not_started"
                && (receipt.AppliedItems.Count != 0
                    || receipt.BeforeFingerprint != receipt.ExpectedFingerprint
                    || receipt.AfterFingerprint != receipt.BeforeFingerprint
                    || string.IsNullOrWhiteSpace(receipt.Error))))
        {
            throw new InvalidDataException($"YMM4 receipt journal is malformed: {source}");
        }
    }

    private static bool IsSha256(string? value)
    {
        return value is { Length: 64 }
            && value.All(character => character is >= '0' and <= '9' or >= 'a' and <= 'f');
    }

    private static bool CanTransition(string current, string next)
    {
        // A terminal-looking receipt can be written just before the recovery
        // journal is finalized. Startup recovery may then need to replace that
        // receipt with another terminal outcome, but no terminal outcome may
        // reopen the operation as applying.
        return next != "applying" || current == "applying";
    }

    private void Persist(IReadOnlyDictionary<Guid, OperationReceiptDto> values)
    {
        PersistFile(path, values, "before_receipt_persist_commit");
    }

    private static void PersistFile<T>(
        string targetPath,
        IReadOnlyDictionary<Guid, T> values,
        string? faultPoint = null)
    {
        var directory = Path.GetDirectoryName(targetPath)!;
        Directory.CreateDirectory(directory);
        var temporary = targetPath + ".tmp";
        var bytes = JsonSerializer.SerializeToUtf8Bytes(values, BridgeJson.Options);
        using (var stream = new FileStream(
                   temporary,
                   FileMode.Create,
                   FileAccess.Write,
                   FileShare.None,
                   4096,
                   FileOptions.WriteThrough))
        {
            stream.Write(bytes);
            stream.Flush(true);
        }
        if (faultPoint is not null)
        {
            BridgeFaultInjection.ThrowIf(faultPoint);
        }
        File.Move(temporary, targetPath, true);
    }

    private void RejectLegacyOperationId(Guid operationId)
    {
        if (legacyReceipts.TryGetValue(operationId, out var legacy))
        {
            throw new BridgeConflictException(
                "Operation ID belongs to an archived protocol-1 receipt without a request binding and cannot be replayed",
                legacy.AfterFingerprint);
        }
    }

    private void EnsureHealthy()
    {
        if (loadError is not null)
        {
            throw new BridgeUnavailableException(
                $"YMM4 receipt journal could not be loaded; refusing mutation until it is recovered: {loadError.Message}");
        }
    }

    private sealed record ReceiptLoadResult(
        Dictionary<Guid, OperationReceiptDto> Receipts,
        Dictionary<Guid, LegacyOperationReceiptDto> LegacyReceipts);

    private sealed record LegacyOperationReceiptDto(
        Guid OperationId,
        string Status,
        string BeforeFingerprint,
        string AfterFingerprint,
        IReadOnlyList<ManagedItemDto> AppliedItems,
        bool Verified,
        string? Error);
}
