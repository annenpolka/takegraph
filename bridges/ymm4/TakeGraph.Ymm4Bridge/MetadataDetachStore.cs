using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

/// Durable WAL and terminal receipt source for Remark-only detach operations.
/// One atomic file per operation avoids coupling this richer receipt to the
/// portable/native-voice receipt schema.
internal sealed class MetadataDetachStore
{
    private readonly object gate = new();
    private readonly string root;
    private readonly Dictionary<Guid, MetadataDetachJournalDto> entries;
    private readonly Exception? loadError;

    internal MetadataDetachStore(string? journalRoot = null)
    {
        root = journalRoot ?? Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "TakeGraph",
            "ymm4-metadata-detach");
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

    internal IReadOnlyList<MetadataDetachJournalDto> ReadAll()
    {
        lock (gate)
        {
            EnsureHealthy();
            return entries.Values
                .OrderBy(value => value.CreatedAt)
                .ThenBy(value => value.Request.OperationId)
                .ToArray();
        }
    }

    internal IReadOnlyList<MetadataDetachJournalDto> ReadPending() =>
        ReadAll().Where(value => value.State is "applying" or "recovery_required").ToArray();

    internal IReadOnlyList<MetadataDetachJournalDto> ReadRecoverable() =>
        ReadAll().Where(value => value.State == "applying").ToArray();

    internal bool TryGet(Guid operationId, out MetadataDetachJournalDto? entry)
    {
        lock (gate)
        {
            EnsureHealthy();
            return entries.TryGetValue(operationId, out entry);
        }
    }

    internal void Put(MetadataDetachJournalDto entry)
    {
        lock (gate)
        {
            EnsureHealthy();
            Validate(entry, $"operation {entry.Request.OperationId}");
            if (entries.TryGetValue(entry.Request.OperationId, out var existing)
                && !SameBinding(existing.Request, entry.Request))
            {
                throw new BridgeConflictException(
                    "Metadata detach operation ID is already bound to another request",
                    existing.AfterFingerprint ?? existing.BeforeFingerprint);
            }
            if (existing is not null && !CanTransition(existing.State, entry.State))
            {
                throw new BridgeConflictException(
                    $"Invalid metadata detach transition: {existing.State} -> {entry.State}",
                    existing.AfterFingerprint ?? existing.BeforeFingerprint);
            }
            Persist(entry);
            entries[entry.Request.OperationId] = entry;
        }
    }

    internal MetadataDetachJournalDto Transition(
        Guid operationId,
        string state,
        string afterFingerprint,
        string remarkDigestAfter,
        string nonRemarkContentDigestAfter,
        bool remarkAbsent,
        string? error)
    {
        lock (gate)
        {
            EnsureHealthy();
            if (!entries.TryGetValue(operationId, out var existing))
            {
                throw new BridgeUnavailableException(
                    $"Metadata detach WAL is missing for operation {operationId}");
            }
            var updated = existing with
            {
                State = state,
                AfterFingerprint = afterFingerprint,
                RemarkDigestAfter = remarkDigestAfter,
                NonRemarkContentDigestAfter = nonRemarkContentDigestAfter,
                RemarkAbsent = remarkAbsent,
                UpdatedAt = DateTimeOffset.UtcNow,
                Error = error,
            };
            Put(updated);
            return updated;
        }
    }

    internal static MetadataDetachReceiptDto ToReceipt(MetadataDetachJournalDto entry)
    {
        var request = entry.Request;
        return new MetadataDetachReceiptDto(
            request.OperationId,
            request.RequestDigest,
            request.ProjectId,
            request.SceneId,
            request.SourceRevision,
            request.ExpectedFingerprint,
            request.EntityId,
            request.RealizationId,
            request.IdentityCarrier,
            entry.State,
            entry.BeforeFingerprint,
            entry.AfterFingerprint ?? entry.BeforeFingerprint,
            checked((uint)entry.Targets.Count),
            entry.BeforeRemarkDigest,
            entry.ExpectedAfterRemarkDigest,
            entry.RemarkDigestAfter ?? entry.BeforeRemarkDigest,
            entry.NonRemarkContentDigestBefore,
            entry.NonRemarkContentDigestAfter ?? entry.NonRemarkContentDigestBefore,
            entry.RemarkAbsent,
            entry.State == "verified",
            entry.Error);
    }

    internal static MetadataDetachJournalDto CreateNotStarted(
        MetadataDetachRequestDto request,
        string beforeFingerprint,
        string error)
    {
        var now = DateTimeOffset.UtcNow;
        var noMutationDigest = ComputeNoMutationDigest(request);
        return new MetadataDetachJournalDto(
            2,
            request,
            "not_started",
            beforeFingerprint,
            beforeFingerprint,
            noMutationDigest,
            noMutationDigest,
            noMutationDigest,
            noMutationDigest,
            noMutationDigest,
            [],
            [],
            false,
            now,
            now,
            error);
    }

    internal static bool SameBinding(
        MetadataDetachRequestDto left,
        MetadataDetachRequestDto right) =>
        left.ProtocolVersion == right.ProtocolVersion
        && left.OperationId == right.OperationId
        && string.Equals(left.RequestDigest, right.RequestDigest, StringComparison.Ordinal)
        && string.Equals(left.ProjectId, right.ProjectId, StringComparison.Ordinal)
        && string.Equals(left.SceneId, right.SceneId, StringComparison.Ordinal)
        && left.SourceRevision == right.SourceRevision
        && string.Equals(left.ExpectedFingerprint, right.ExpectedFingerprint, StringComparison.Ordinal)
        && string.Equals(left.EntityId, right.EntityId, StringComparison.Ordinal)
        && left.RealizationId == right.RealizationId
        && string.Equals(left.IdentityCarrier, right.IdentityCarrier, StringComparison.Ordinal);

    private static Dictionary<Guid, MetadataDetachJournalDto> Load(string root)
    {
        if (!Directory.Exists(root))
        {
            return [];
        }
        var loaded = new Dictionary<Guid, MetadataDetachJournalDto>();
        foreach (var path in Directory.EnumerateFiles(root, "*.json", SearchOption.TopDirectoryOnly))
        {
            var entry = JsonSerializer.Deserialize<MetadataDetachJournalDto>(
                    File.ReadAllText(path), BridgeJson.Options)
                ?? throw new InvalidDataException($"Metadata detach WAL is empty: {path}");
            Validate(entry, path);
            if (!string.Equals(
                    Path.GetFileNameWithoutExtension(path),
                    entry.Request.OperationId.ToString("N"),
                    StringComparison.OrdinalIgnoreCase)
                || !loaded.TryAdd(entry.Request.OperationId, entry))
            {
                throw new InvalidDataException($"Metadata detach WAL identity is invalid: {path}");
            }
        }
        return loaded;
    }

    private void Persist(MetadataDetachJournalDto entry)
    {
        Directory.CreateDirectory(root);
        var path = Path.Combine(root, $"{entry.Request.OperationId:N}.json");
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
        BridgeFaultInjection.ThrowIf("before_metadata_detach_persist_commit");
        File.Move(temporary, path, overwrite: true);
    }

    private static void Validate(MetadataDetachJournalDto entry, string source)
    {
        var request = entry.Request;
        if (entry.SchemaVersion != 2
            || request.ProtocolVersion != BridgeContract.ProtocolVersion
            || request.OperationId == Guid.Empty
            || !IsSha256(request.RequestDigest)
            || !ApplyRequestDigest.Matches(
                request.RequestDigest,
                ApplyRequestDigest.Compute(request))
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || !IsSha256(request.ExpectedFingerprint)
            || string.IsNullOrWhiteSpace(request.EntityId)
            || request.RealizationId == Guid.Empty
            || request.IdentityCarrier != "takegraph_remark_v2"
            || entry.State is not ("not_started" or "applying" or "verified" or "rolled_back" or "failed" or "recovery_required")
            || !IsSha256(entry.BeforeFingerprint)
            || (entry.AfterFingerprint is not null && !IsSha256(entry.AfterFingerprint))
            || !IsSha256(entry.BeforeRemarkDigest)
            || !IsSha256(entry.ExpectedAfterRemarkDigest)
            || (entry.RemarkDigestAfter is not null && !IsSha256(entry.RemarkDigestAfter))
            || !IsSha256(entry.NonRemarkContentDigestBefore)
            || (entry.NonRemarkContentDigestAfter is not null
                && !IsSha256(entry.NonRemarkContentDigestAfter))
            || entry.Targets is null
            || entry.SceneRemarks is null
            || entry.CreatedAt == default
            || entry.UpdatedAt < entry.CreatedAt
            || (entry.State == "not_started"
                && (entry.BeforeFingerprint != request.ExpectedFingerprint
                    || entry.AfterFingerprint != entry.BeforeFingerprint
                    || entry.Targets.Count != 0
                    || entry.SceneRemarks.Count != 0
                    || entry.BeforeRemarkDigest != ComputeNoMutationDigest(request)
                    || entry.ExpectedAfterRemarkDigest != entry.BeforeRemarkDigest
                    || entry.RemarkDigestAfter != entry.BeforeRemarkDigest
                    || entry.NonRemarkContentDigestBefore != entry.BeforeRemarkDigest
                    || entry.NonRemarkContentDigestAfter != entry.BeforeRemarkDigest
                    || entry.RemarkAbsent
                    || string.IsNullOrWhiteSpace(entry.Error)))
            || (entry.State != "not_started" && entry.Targets.Count != 1)
            || (entry.State != "not_started" && entry.SceneRemarks.Count == 0)
            || (entry.State == "applying"
                && (entry.AfterFingerprint is not null
                    || entry.RemarkDigestAfter is not null
                    || entry.NonRemarkContentDigestAfter is not null
                    || entry.RemarkAbsent))
            || (entry.State is not ("applying" or "not_started")
                && (entry.AfterFingerprint is null || entry.RemarkDigestAfter is null))
            || (entry.State == "verified"
                && (!entry.RemarkAbsent
                    || entry.RemarkDigestAfter != entry.ExpectedAfterRemarkDigest
                    || entry.NonRemarkContentDigestAfter != entry.NonRemarkContentDigestBefore
                    || entry.Error is not null))
            || (entry.State == "rolled_back"
                && (entry.RemarkAbsent
                    || entry.RemarkDigestAfter != entry.BeforeRemarkDigest
                    || entry.NonRemarkContentDigestAfter != entry.NonRemarkContentDigestBefore))
            || (entry.State is "failed" or "recovery_required" && string.IsNullOrWhiteSpace(entry.Error)))
        {
            throw new InvalidDataException($"Metadata detach WAL is malformed: {source}");
        }
        if (entry.State == "not_started")
        {
            return;
        }
        foreach (var target in entry.Targets)
        {
            if (string.IsNullOrWhiteSpace(target.TypeName)
                || target.Frame < 0
                || target.Layer < 0
                || target.Length <= 0
                || target.OriginalRemark is null
                || !IsSha256(target.RemarkSha256)
                || !IsSha256(target.NonRemarkContentDigest)
                || !string.Equals(
                    target.RemarkSha256,
                    Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(target.OriginalRemark))),
                    StringComparison.Ordinal))
            {
                throw new InvalidDataException($"Metadata detach target is malformed: {source}");
            }
        }
        var stableWitnesses = new HashSet<string>(StringComparer.Ordinal);
        foreach (var witness in entry.SceneRemarks)
        {
            if (string.IsNullOrWhiteSpace(witness.TypeName)
                || witness.Frame < 0
                || witness.Layer < 0
                || witness.Length <= 0
                || witness.OriginalRemark is null
                || witness.ExpectedRemark is null
                || !IsSha256(witness.NonRemarkContentDigest)
                || !IsSha256(witness.StableItemWitness)
                || !IsSha256(witness.OriginalRemarkSha256)
                || !IsSha256(witness.ExpectedRemarkSha256)
                || !string.Equals(
                    witness.StableItemWitness,
                    ComputeStableItemWitness(
                        witness.TypeName,
                        witness.Frame,
                        witness.Layer,
                        witness.Length,
                        witness.NonRemarkContentDigest),
                    StringComparison.Ordinal)
                || !string.Equals(
                    witness.OriginalRemarkSha256,
                    HashUtf8(witness.OriginalRemark),
                    StringComparison.Ordinal)
                || !string.Equals(
                    witness.ExpectedRemarkSha256,
                    HashUtf8(witness.ExpectedRemark),
                    StringComparison.Ordinal)
                || !stableWitnesses.Add(witness.StableItemWitness))
            {
                throw new InvalidDataException(
                    $"Metadata detach scene Remark witness is malformed or ambiguous: {source}");
            }
        }
        var targetWitnesses = entry.Targets.Select(target => ComputeStableItemWitness(
                target.TypeName,
                target.Frame,
                target.Layer,
                target.Length,
                target.NonRemarkContentDigest))
            .ToHashSet(StringComparer.Ordinal);
        var targetPreimagesMatch = entry.Targets.All(target =>
        {
            var stableWitness = ComputeStableItemWitness(
                target.TypeName,
                target.Frame,
                target.Layer,
                target.Length,
                target.NonRemarkContentDigest);
            return entry.SceneRemarks.Count(witness =>
                witness.StableItemWitness == stableWitness
                && witness.OriginalRemarkSha256 == target.RemarkSha256
                && witness.OriginalRemark == target.OriginalRemark) == 1;
        });
        var changedWitnesses = entry.SceneRemarks
            .Where(witness => !string.Equals(
                witness.OriginalRemarkSha256,
                witness.ExpectedRemarkSha256,
                StringComparison.Ordinal))
            .Select(witness => witness.StableItemWitness)
            .ToHashSet(StringComparer.Ordinal);
        if (!targetPreimagesMatch
            || targetWitnesses.Count != entry.Targets.Count
            || changedWitnesses.Count != entry.Targets.Count
            || !targetWitnesses.SetEquals(changedWitnesses))
        {
            throw new InvalidDataException(
                $"Metadata detach expected Remark changes are not target-exact: {source}");
        }
        if (!string.Equals(
                entry.BeforeRemarkDigest,
                ComputeSceneRemarkDigest(entry.SceneRemarks, expected: false),
                StringComparison.Ordinal))
        {
            throw new InvalidDataException(
                $"Metadata detach Remark-set digest is malformed: {source}");
        }
        if (!string.Equals(
                entry.ExpectedAfterRemarkDigest,
                ComputeSceneRemarkDigest(entry.SceneRemarks, expected: true),
                StringComparison.Ordinal))
        {
            throw new InvalidDataException(
                $"Metadata detach expected post-Remark digest is malformed: {source}");
        }
    }

    internal static string ComputeSceneRemarkDigest(
        IReadOnlyList<MetadataDetachSceneRemarkDto> witnesses,
        bool expected)
    {
        var canonical = new StringBuilder("takegraph-ymm4-detach-scene-remarks-v2\n");
        foreach (var witness in witnesses.OrderBy(
                     value => value.StableItemWitness,
                     StringComparer.Ordinal))
        {
            canonical.Append(witness.StableItemWitness)
                .Append('|')
                .Append(expected
                    ? witness.ExpectedRemarkSha256
                    : witness.OriginalRemarkSha256)
                .Append('\n');
        }
        return HashUtf8(canonical.ToString());
    }

    internal static string ComputeStableItemWitness(
        string typeName,
        int frame,
        int layer,
        int length,
        string nonRemarkContentDigest)
    {
        var canonical = new StringBuilder("takegraph-ymm4-detach-item-witness-v1\n")
            .Append(typeName.Length).Append(':').Append(typeName)
            .Append('|').Append(frame)
            .Append('|').Append(layer)
            .Append('|').Append(length)
            .Append('|').Append(nonRemarkContentDigest)
            .Append('\n');
        return HashUtf8(canonical.ToString());
    }

    private static string HashUtf8(string value) =>
        Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(value)));

    private static string ComputeNoMutationDigest(MetadataDetachRequestDto request) =>
        HashUtf8($"takegraph-ymm4-detach-not-started-v1\n{request.RequestDigest}\n");

    private static bool IsSha256(string? value) => value is { Length: 64 }
        && value.All(character => character is >= '0' and <= '9' or >= 'a' and <= 'f');

    private static bool CanTransition(string current, string next) =>
        current == next
        || current == "applying" && next is "verified" or "rolled_back" or "failed" or "recovery_required";

    private void EnsureHealthy()
    {
        if (loadError is not null)
        {
            throw new BridgeUnavailableException(
                $"Metadata detach WAL could not be loaded; refusing mutation: {loadError.Message}");
        }
    }
}
