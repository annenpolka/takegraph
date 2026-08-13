using System.Text;
using System.Text.Json;
using System.Security.Cryptography;

namespace TakeGraph.Ymm4Bridge;

internal sealed class RecoveryJournalStore
{
    private readonly object gate = new();
    private readonly string root;
    private readonly Dictionary<Guid, RecoveryJournalEntryDto> entries;
    private readonly Exception? loadError;

    internal RecoveryJournalStore(string? journalRoot = null)
    {
        root = journalRoot ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "TakeGraph",
            "ymm4-recovery");
        try
        {
            entries = Load(root);
        }
        catch (Exception error)
        {
            entries = [];
            loadError = error;
        }
    }

    internal IReadOnlyList<RecoveryJournalEntryDto> ReadAll()
    {
        lock (gate)
        {
            EnsureHealthy();
            return entries.Values
                .OrderBy(value => value.CreatedAt)
                .ThenBy(value => value.OperationId)
                .ToArray();
        }
    }

    internal IReadOnlyList<RecoveryJournalEntryDto> ReadPending()
    {
        return ReadAll().Where(value => value.State is
                "applying" or "applied_unverified" or "recovery_required")
            .ToArray();
    }

    internal IReadOnlyList<RecoveryJournalEntryDto> ReadRecoverable()
    {
        return ReadAll().Where(value => value.State is "applying" or "applied_unverified")
            .ToArray();
    }

    internal bool TryGet(Guid operationId, out RecoveryJournalEntryDto? entry)
    {
        lock (gate)
        {
            EnsureHealthy();
            return entries.TryGetValue(operationId, out entry);
        }
    }

    internal void Put(RecoveryJournalEntryDto entry)
    {
        lock (gate)
        {
            EnsureHealthy();
            Validate(entry, $"operation {entry.OperationId}");
            if (entries.TryGetValue(entry.OperationId, out var existing)
                && (!string.Equals(existing.RequestDigest, entry.RequestDigest, StringComparison.Ordinal)
                    || !string.Equals(existing.ProjectId, entry.ProjectId, StringComparison.Ordinal)
                    || !string.Equals(existing.SceneId, entry.SceneId, StringComparison.Ordinal)
                    || !string.Equals(
                        existing.ExpectedFingerprint,
                        entry.ExpectedFingerprint,
                        StringComparison.Ordinal)
                    || !string.Equals(existing.Driver, entry.Driver, StringComparison.Ordinal)))
            {
                throw new BridgeConflictException(
                    "Recovery operation ID is already bound to another request",
                    entry.BeforeFingerprint);
            }
            Persist(entry);
            entries[entry.OperationId] = entry;
        }
    }

    internal RecoveryJournalEntryDto Transition(
        Guid operationId,
        string state,
        string? afterFingerprint,
        string? error)
    {
        lock (gate)
        {
            EnsureHealthy();
            if (!entries.TryGetValue(operationId, out var existing))
            {
                throw new BridgeUnavailableException(
                    $"Recovery journal entry is missing for operation {operationId}");
            }
            if (!CanTransition(existing.State, state))
            {
                throw new BridgeConflictException(
                    $"Invalid recovery state transition: {existing.State} -> {state}",
                    existing.AfterFingerprint ?? existing.BeforeFingerprint);
            }
            var updated = existing with
            {
                State = state,
                AfterFingerprint = afterFingerprint,
                Error = error,
                UpdatedAt = DateTimeOffset.UtcNow,
            };
            Validate(updated, $"operation {operationId}");
            Persist(updated);
            entries[operationId] = updated;
            return updated;
        }
    }

    internal RecoveryJournalEntryDto MarkAppliedUnverified(
        Guid operationId,
        string afterFingerprint,
        IReadOnlyList<ManagedItemDto> verifiedItems,
        NativeExtensionApplyResponseDto? nativeExtensionReceipt = null)
    {
        lock (gate)
        {
            EnsureHealthy();
            if (!entries.TryGetValue(operationId, out var existing))
            {
                throw new BridgeUnavailableException(
                    $"Recovery journal entry is missing for operation {operationId}");
            }
            if (existing.State != "applying")
            {
                throw new BridgeConflictException(
                    $"Invalid recovery state transition: {existing.State} -> applied_unverified",
                    existing.AfterFingerprint ?? existing.BeforeFingerprint);
            }
            var updated = existing with
            {
                State = "applied_unverified",
                AfterFingerprint = afterFingerprint,
                VerifiedItems = verifiedItems,
                NativeExtensionReceipt = nativeExtensionReceipt,
                Error = null,
                UpdatedAt = DateTimeOffset.UtcNow,
            };
            Validate(updated, $"operation {operationId}");
            Persist(updated);
            entries[operationId] = updated;
            return updated;
        }
    }

    internal bool TryTransition(
        Guid operationId,
        string state,
        string? afterFingerprint,
        string? error)
    {
        lock (gate)
        {
            EnsureHealthy();
            if (!entries.ContainsKey(operationId))
            {
                return false;
            }
            Transition(operationId, state, afterFingerprint, error);
            return true;
        }
    }

    private static Dictionary<Guid, RecoveryJournalEntryDto> Load(string root)
    {
        if (!Directory.Exists(root))
        {
            return [];
        }
        var loaded = new Dictionary<Guid, RecoveryJournalEntryDto>();
        foreach (var path in Directory.EnumerateFiles(root, "*.json", SearchOption.TopDirectoryOnly))
        {
            var entry = JsonSerializer.Deserialize<RecoveryJournalEntryDto>(
                    File.ReadAllText(path),
                    BridgeJson.Options)
                ?? throw new InvalidDataException($"Recovery journal is empty: {path}");
            Validate(entry, path);
            if (!string.Equals(
                    Path.GetFileNameWithoutExtension(path),
                    entry.OperationId.ToString("N"),
                    StringComparison.OrdinalIgnoreCase))
            {
                throw new InvalidDataException($"Recovery journal is malformed: {path}");
            }
            if (!loaded.TryAdd(entry.OperationId, entry))
            {
                throw new InvalidDataException(
                    $"Duplicate recovery operation ID: {entry.OperationId}");
            }
        }
        return loaded;
    }

    private void Persist(RecoveryJournalEntryDto entry)
    {
        Directory.CreateDirectory(root);
        var path = Path.Combine(root, $"{entry.OperationId:N}.json");
        var temporary = $"{path}.tmp-{Guid.NewGuid():N}";
        var bytes = JsonSerializer.SerializeToUtf8Bytes(entry, BridgeJson.Options);
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
        BridgeFaultInjection.ThrowIf("before_recovery_persist_commit");
        File.Move(temporary, path, overwrite: true);
    }

    private static void Validate(RecoveryJournalEntryDto entry, string source)
    {
        if (entry.SchemaVersion != 1
            || entry.OperationId == Guid.Empty
            || !IsSha256(entry.RequestDigest)
            || string.IsNullOrWhiteSpace(entry.ProjectId)
            || string.IsNullOrWhiteSpace(entry.SceneId)
            || !IsSha256(entry.ExpectedFingerprint)
            || !IsSha256(entry.BeforeFingerprint)
            || (entry.AfterFingerprint is not null && !IsSha256(entry.AfterFingerprint))
            || entry.State is not (
                "applying" or "applied_unverified" or "verified" or "rolled_back" or
                "failed" or "recovery_required")
            || entry.Driver is not (
                "portable_pair" or "native_voice_create" or "native_voice_mutation" or
                "native_extension" or "timeline_edit_managed_cue_mixed")
            || entry.EntityIds is null
            || entry.RealizationIds is null
            || entry.ExpectedItems is null
            || entry.PreservedStateDigests is null
            || entry.BeforeItems is null
            || entry.CreatedAt == default
            || entry.UpdatedAt < entry.CreatedAt
            || (entry.State != "applying" && entry.AfterFingerprint is null)
            || ((entry.State is "failed" or "recovery_required")
                && string.IsNullOrWhiteSpace(entry.Error))
            || ((entry.State is "verified" or "applied_unverified") && entry.Error is not null))
        {
            throw new InvalidDataException($"Recovery journal is malformed: {source}");
        }


        if (entry.State == "applied_unverified"
            && (entry.VerifiedItems is not null || entry.NativeExtensionReceipt is not null))
        {
            if (entry.Driver == "native_extension")
            {
                var receipt = entry.NativeExtensionReceipt;
                if (receipt is null
                    || receipt.OperationId != entry.OperationId
                    || receipt.RequestDigest != entry.RequestDigest
                    || receipt.ProjectId != entry.ProjectId
                    || receipt.SceneId != entry.SceneId
                    || receipt.BeforeFingerprint != entry.BeforeFingerprint
                    || receipt.AfterFingerprint != entry.AfterFingerprint
                    || receipt.Status != "verified"
                    || !receipt.Verified
                    || receipt.Error is not null
                    || entry.VerifiedItems is not { Count: 0 })
                {
                    throw new InvalidDataException(
                        $"Native-extension verified recovery evidence is malformed: {source}");
                }
            }
            else if (entry.VerifiedItems is null || entry.NativeExtensionReceipt is not null)
            {
                throw new InvalidDataException(
                    $"Verified recovery evidence is malformed: {source}");
            }
        }

        if (entry.EntityIds.Any(string.IsNullOrWhiteSpace)
            || entry.EntityIds.Distinct(StringComparer.Ordinal).Count() != entry.EntityIds.Count
            || entry.RealizationIds.Any(value => value == Guid.Empty)
            || entry.RealizationIds.Distinct().Count() != entry.RealizationIds.Count)
        {
            throw new InvalidDataException($"Recovery journal identities are malformed: {source}");
        }

        var entityIds = entry.EntityIds.ToHashSet(StringComparer.Ordinal);
        var realizationIds = entry.RealizationIds.ToHashSet();
        foreach (var expected in entry.ExpectedItems)
        {
            if (string.IsNullOrWhiteSpace(expected.EntityId)
                || !entityIds.Contains(expected.EntityId)
                || string.IsNullOrWhiteSpace(expected.Kind)
                || expected.Frame < 0
                || expected.Layer < 0
                || expected.Length <= 0
                || (expected.ArtifactHash is not null && !IsHexSha256(expected.ArtifactHash))
                || (expected.RealizationId is not null
                    && (!Guid.TryParse(expected.RealizationId, out var realizationId)
                        || !realizationIds.Contains(realizationId))))
            {
                throw new InvalidDataException($"Recovery journal expected state is malformed: {source}");
            }
        }

        if ((entry.Driver == "native_extension" && entry.ExpectedItems.Count != 0)
            || (entry.Driver == "portable_pair" && entry.RealizationIds.Count != 0)
            || ((entry.Driver is "native_voice_create" or "native_voice_mutation")
                && entry.ExpectedItems.Any(value => value.RealizationId is null)))
        {
            throw new InvalidDataException($"Recovery journal driver binding is malformed: {source}");
        }
        if ((entry.Driver == "native_extension" && entry.PreservedStateDigests.Count != 0)
            || (entry.Driver == "portable_pair" && entry.PreservedStateDigests.Count != 0)
            || (entry.Driver == "native_voice_create"
                && (entry.PreservedStateDigests.Count != 0
                    || entry.BeforeItems.Count != 0
                    || entry.ExpectedItems.Count != entry.RealizationIds.Count))
            || (entry.Driver == "portable_pair"
                && (entry.ExpectedItems.Count != checked(entry.EntityIds.Count * 2)
                    || entry.ExpectedItems.Any(value => value.Kind is not ("audio" or "caption"))))
            || (entry.Driver == "timeline_edit_managed_cue_mixed"
                && (entry.PreservedStateDigests.Count != 0
                    || !entry.EntityIds.ToHashSet(StringComparer.Ordinal).SetEquals(
                        entry.ExpectedItems.Select(value => value.EntityId))
                    || !entry.RealizationIds.ToHashSet().SetEquals(
                        entry.ExpectedItems.Where(value => value.RealizationId is not null)
                            .Select(value => Guid.Parse(value.RealizationId!)))
                    || entry.ExpectedItems.Where(value => value.RealizationId is null)
                        .GroupBy(value => value.EntityId, StringComparer.Ordinal)
                        .Any(group => group.Count() != 2
                            || group.Any(value => value.Kind is not ("audio" or "caption")))
                    || entry.ExpectedItems.Where(value => value.RealizationId is not null)
                        .GroupBy(value => value.RealizationId, StringComparer.OrdinalIgnoreCase)
                        .Any(group => group.Count() != 1 || group.Single().Kind != "voice"))))
        {
            throw new InvalidDataException($"Recovery journal driver payload is malformed: {source}");
        }

        foreach (var (realizationId, digest) in entry.PreservedStateDigests)
        {
            if (realizationId == Guid.Empty
                || !realizationIds.Contains(realizationId)
                || !IsSha256(digest))
            {
                throw new InvalidDataException($"Recovery journal preservation binding is malformed: {source}");
            }
        }

        foreach (var beforeItem in entry.BeforeItems)
        {
            if (string.IsNullOrWhiteSpace(beforeItem.TypeName)
                || !beforeItem.TypeName.StartsWith(
                    "YukkuriMovieMaker.Project.Items.",
                    StringComparison.Ordinal)
                || string.IsNullOrWhiteSpace(beforeItem.Json)
                || !IsSha256(beforeItem.Sha256)
                || !string.Equals(
                    beforeItem.Sha256,
                    Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(beforeItem.Json))),
                    StringComparison.Ordinal))
            {
                throw new InvalidDataException($"Recovery journal preimage is malformed: {source}");
            }
            try
            {
                using var _ = JsonDocument.Parse(beforeItem.Json);
            }
            catch (JsonException error)
            {
                throw new InvalidDataException($"Recovery journal preimage JSON is malformed: {source}", error);
            }
        }
    }

    private static bool IsSha256(string? value)
    {
        return value is { Length: 64 }
            && value.All(character => character is >= '0' and <= '9' or >= 'a' and <= 'f');
    }

    private static bool IsHexSha256(string? value)
    {
        return value is { Length: 64 }
            && value.All(character => character is >= '0' and <= '9'
                or >= 'a' and <= 'f'
                or >= 'A' and <= 'F');
    }

    private static bool CanTransition(string current, string next)
    {
        if (string.Equals(current, next, StringComparison.Ordinal))
        {
            return true;
        }
        return current switch
        {
            "applying" => next is
                "applied_unverified" or "verified" or "rolled_back" or "failed" or
                "recovery_required",
            // Successful semantic read-back is historical commit evidence.
            // Later target drift may require reconciliation but can never
            // authorize restoring the pre-mutation image.
            "applied_unverified" => next is "verified" or "recovery_required",
            "recovery_required" => next == "failed",
            _ => false,
        };
    }

    private void EnsureHealthy()
    {
        if (loadError is not null)
        {
            throw new BridgeUnavailableException(
                $"YMM4 recovery journal could not be loaded; refusing mutation until it is recovered: {loadError.Message}");
        }
    }
}
