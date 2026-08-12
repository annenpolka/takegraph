using System.Security.Cryptography;
using System.Text;
using System.Windows;

namespace TakeGraph.Ymm4Bridge;

internal sealed partial class Ymm4Facade
{
    internal async Task<MetadataDetachResponseDto> DetachManagedMetadataAsync(
        MetadataDetachRequestDto request)
    {
        ValidateMetadataDetachRequestShape(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (metadataDetachStore.TryGet(request.OperationId, out var existing)
                && existing is not null)
            {
                EnsureMetadataDetachBinding(request, existing);
                if (existing.State != "applying")
                {
                    return MetadataDetachResponse(existing, replayed: true);
                }
                // An existing WAL is historical recovery evidence. Recover it
                // before consulting today's mutation runtime/capability gate.
                await EnsureRecoveryClearAsync().ConfigureAwait(false);
                if (!metadataDetachStore.TryGet(request.OperationId, out var recovered)
                    || recovered is null)
                {
                    throw new BridgeUnavailableException(
                        "Metadata detach WAL disappeared during recovery");
                }
                EnsureMetadataDetachBinding(request, recovered);
                return MetadataDetachResponse(recovered, replayed: true);
            }

            ValidateMutationRuntime();
            await EnsureRecoveryClearAsync().ConfigureAwait(false);

            var before = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, before);
            EnsureFingerprint(request.ExpectedFingerprint, before.Fingerprint);
            var preparation = await Application.Current.Dispatcher.InvokeAsync(() =>
                PrepareMetadataDetach(request));
            var now = DateTimeOffset.UtcNow;
            var journal = new MetadataDetachJournalDto(
                2,
                request,
                "applying",
                before.Fingerprint,
                null,
                preparation.BeforeRemarkDigest,
                preparation.ExpectedAfterRemarkDigest,
                null,
                preparation.SceneNonRemarkDigest,
                null,
                preparation.TargetDtos,
                preparation.SceneRemarks,
                false,
                now,
                now,
                null);
            // WAL persistence must complete before the first YMM object changes.
            metadataDetachStore.Put(journal);

            try
            {
                await Application.Current.Dispatcher.InvokeAsync(() =>
                {
                    var sealedBefore = ReadMetadataDetachReadback(
                        request,
                        preparation.SceneRemarks);
                    if (!sealedBefore.SceneWitnessesExact
                        || sealedBefore.SceneRemarkDigest != preparation.BeforeRemarkDigest
                        || sealedBefore.SceneNonRemarkDigest != preparation.SceneNonRemarkDigest)
                    {
                        throw new BridgeConflictException(
                            "The YMM4 scene changed after the metadata detach WAL was sealed",
                            SnapshotCore().Fingerprint);
                    }
                    foreach (var target in preparation.Targets)
                    {
                        EnsureUnlockedForMutation(
                            target.Item,
                            $"metadata detach {request.EntityId}/{request.RealizationId:D}",
                            before.Fingerprint);
                        var currentRemark = GetString(target.Item, "Remark");
                        if (!string.Equals(
                                currentRemark,
                                target.Remark,
                                StringComparison.Ordinal))
                        {
                            throw new BridgeConflictException(
                                "A target Remark changed after the detach WAL was sealed",
                                SnapshotCore().Fingerprint);
                        }
                        var detached = RemoveMetadataIdentity(
                            currentRemark,
                            request,
                            out var matched);
                        if (!matched)
                        {
                            throw new BridgeConflictException(
                                "The approved TakeGraph Remark identity changed before mutation",
                                SnapshotCore().Fingerprint);
                        }
                        SetRequired(target.Item, detached, "Remark");
                    }
                });
                BridgeFaultInjection.ThrowIf("after_metadata_detach_mutation");

                var after = Snapshot();
                var readback = await Application.Current.Dispatcher.InvokeAsync(() =>
                    ReadMetadataDetachReadback(request, preparation.SceneRemarks));
                if (!readback.RemarkAbsent
                    || !readback.SceneWitnessesExact
                    || readback.SceneRemarkDigest != preparation.ExpectedAfterRemarkDigest
                    || readback.SceneNonRemarkDigest != preparation.SceneNonRemarkDigest)
                {
                    throw new BridgeUnavailableException(
                        "Metadata detach read-back did not prove the exact post-Remark scene and unchanged non-Remark content");
                }
                var verified = metadataDetachStore.Transition(
                    request.OperationId,
                    "verified",
                    after.Fingerprint,
                    readback.SceneRemarkDigest,
                    readback.SceneNonRemarkDigest,
                    true,
                    null);
                return MetadataDetachResponse(verified, replayed: false);
            }
            catch (Exception error)
            {
                return await RollBackMetadataDetachAsync(
                    request,
                    before,
                    preparation,
                    error.GetBaseException().Message).ConfigureAwait(false);
            }
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal MetadataDetachReceiptDto GetMetadataDetach(Guid operationId)
    {
        if (!metadataDetachStore.TryGet(operationId, out var entry) || entry is null)
        {
            throw new BridgeNotFoundException(
                $"Metadata detach operation was not found: {operationId}");
        }
        return MetadataDetachStore.ToReceipt(entry);
    }

    internal async Task<MetadataDetachResponseDto> SealMetadataDetachNotStartedAsync(
        MetadataDetachRequestDto request)
    {
        ValidateMetadataDetachRequestShape(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (metadataDetachStore.TryGet(request.OperationId, out var existing)
                && existing is not null)
            {
                EnsureMetadataDetachBinding(request, existing);
                return MetadataDetachResponse(existing, replayed: true);
            }

            // This durable tombstone is created while holding the same gate as
            // detach apply. It both proves that no WAL/mutation preceded it and
            // prevents a delayed copy of the exact POST from mutating later.
            var notStarted = MetadataDetachStore.CreateNotStarted(
                request,
                request.ExpectedFingerprint,
                "Metadata detach did not start; a durable no-mutation tombstone was sealed");
            metadataDetachStore.Put(notStarted);
            return MetadataDetachResponse(notStarted, replayed: false);
        }
        finally
        {
            applyGate.Release();
        }
    }

    private async Task<MetadataDetachResponseDto> RollBackMetadataDetachAsync(
        MetadataDetachRequestDto request,
        ProjectSnapshotDto before,
        MetadataDetachPreparation preparation,
        string failure)
    {
        string? restoreError = null;
        try
        {
            BridgeFaultInjection.ThrowIf("during_metadata_detach_rollback");
            await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                foreach (var target in preparation.Targets.Zip(preparation.TargetDtos))
                {
                    SetRequired(target.First.Item, target.Second.OriginalRemark, "Remark");
                }
            });
        }
        catch (Exception error)
        {
            restoreError = error.GetBaseException().Message;
        }

        var current = TrySnapshot() ?? before;
        var currentDigest = TryReadSceneNonRemarkDigest() ?? preparation.SceneNonRemarkDigest;
        var currentRemarks = TryReadMetadataDetachReadback(
            request,
            preparation.SceneRemarks);
        var restored = restoreError is null
            && string.Equals(current.Fingerprint, before.Fingerprint, StringComparison.Ordinal)
            && string.Equals(
                currentDigest,
                preparation.SceneNonRemarkDigest,
                StringComparison.Ordinal)
            && currentRemarks is { SceneWitnessesExact: true }
            && string.Equals(
                currentRemarks.SceneRemarkDigest,
                preparation.BeforeRemarkDigest,
                StringComparison.Ordinal);
        var errorText = restoreError is null
            ? failure
            : $"{failure}; metadata Remark restoration failed: {restoreError}";
        var terminal = metadataDetachStore.Transition(
            request.OperationId,
            restored ? "rolled_back" : "recovery_required",
            current.Fingerprint,
            currentRemarks?.SceneRemarkDigest ?? preparation.ExpectedAfterRemarkDigest,
            currentDigest,
            false,
            errorText);
        return MetadataDetachResponse(terminal, replayed: false);
    }

    private async Task<bool> RecoverPendingMetadataDetachJournalsCoreAsync()
    {
        var pending = metadataDetachStore.ReadRecoverable();
        if (pending.Count == 0)
        {
            return true;
        }
        var allResolved = true;
        foreach (var entry in pending)
        {
            var current = Snapshot();
            if (!string.Equals(entry.Request.ProjectId, current.ProjectId, StringComparison.Ordinal)
                || !string.Equals(entry.Request.SceneId, current.SceneId, StringComparison.Ordinal))
            {
                allResolved = false;
                continue;
            }
            var currentDigest = TryReadSceneNonRemarkDigest() ?? string.Empty;
            var remarkReadback = await Application.Current.Dispatcher.InvokeAsync(() =>
                ReadMetadataDetachReadback(entry.Request, entry.SceneRemarks));
            if (string.Equals(current.Fingerprint, entry.BeforeFingerprint, StringComparison.Ordinal)
                && string.Equals(
                    currentDigest,
                    entry.NonRemarkContentDigestBefore,
                    StringComparison.Ordinal)
                && remarkReadback.SceneWitnessesExact
                && string.Equals(
                    remarkReadback.SceneRemarkDigest,
                    entry.BeforeRemarkDigest,
                    StringComparison.Ordinal))
            {
                metadataDetachStore.Transition(
                    entry.Request.OperationId,
                    "rolled_back",
                    current.Fingerprint,
                    remarkReadback.SceneRemarkDigest,
                    currentDigest,
                    false,
                    "Recovered metadata detach before any mutation was committed");
                continue;
            }
            if (remarkReadback.RemarkAbsent
                && remarkReadback.SceneWitnessesExact
                && string.Equals(
                    remarkReadback.SceneRemarkDigest,
                    entry.ExpectedAfterRemarkDigest,
                    StringComparison.Ordinal)
                && string.Equals(
                    remarkReadback.SceneNonRemarkDigest,
                    entry.NonRemarkContentDigestBefore,
                    StringComparison.Ordinal))
            {
                metadataDetachStore.Transition(
                    entry.Request.OperationId,
                    "verified",
                    current.Fingerprint,
                    remarkReadback.SceneRemarkDigest,
                    remarkReadback.SceneNonRemarkDigest,
                    true,
                    null);
                continue;
            }

            try
            {
                await Application.Current.Dispatcher.InvokeAsync(() =>
                    RestoreMetadataDetachTargets(entry));
                current = Snapshot();
                currentDigest = TryReadSceneNonRemarkDigest() ?? string.Empty;
                remarkReadback = await Application.Current.Dispatcher.InvokeAsync(() =>
                    ReadMetadataDetachReadback(entry.Request, entry.SceneRemarks));
                if (string.Equals(current.Fingerprint, entry.BeforeFingerprint, StringComparison.Ordinal)
                    && string.Equals(
                        currentDigest,
                        entry.NonRemarkContentDigestBefore,
                        StringComparison.Ordinal)
                    && remarkReadback.SceneWitnessesExact
                    && string.Equals(
                        remarkReadback.SceneRemarkDigest,
                        entry.BeforeRemarkDigest,
                        StringComparison.Ordinal))
                {
                    metadataDetachStore.Transition(
                        entry.Request.OperationId,
                        "rolled_back",
                        current.Fingerprint,
                        remarkReadback.SceneRemarkDigest,
                        currentDigest,
                        false,
                        "Recovered metadata detach by restoring the durable Remark preimage");
                }
                else
                {
                    allResolved = false;
                    metadataDetachStore.Transition(
                        entry.Request.OperationId,
                        "recovery_required",
                        current.Fingerprint,
                        remarkReadback.SceneRemarkDigest,
                        currentDigest.PadRight(64, '0')[..64],
                        false,
                        "Metadata Remark preimage was restored, but the exact before-state could not be proven");
                }
            }
            catch (Exception error)
            {
                allResolved = false;
                current = TrySnapshot() ?? current;
                currentDigest = TryReadSceneNonRemarkDigest()
                    ?? entry.NonRemarkContentDigestBefore;
                var currentRemarkDigest = TryReadMetadataDetachReadback(
                        entry.Request,
                        entry.SceneRemarks)
                    ?.SceneRemarkDigest
                    ?? entry.ExpectedAfterRemarkDigest;
                metadataDetachStore.Transition(
                    entry.Request.OperationId,
                    "recovery_required",
                    current.Fingerprint,
                    currentRemarkDigest,
                    currentDigest,
                    false,
                    $"Metadata detach recovery failed: {error.GetBaseException().Message}");
            }
        }
        return allResolved;
    }

    private static MetadataDetachPreparation PrepareMetadataDetach(
        MetadataDetachRequestDto request)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var rawItems = ReadItems(timelineViewModel);
        var targets = FindMetadataDetachTargets(rawItems, request);
        if (targets.Count == 0)
        {
            throw new BridgeNotFoundException(
                $"Managed Remark identity was not found: {request.EntityId}/{request.RealizationId:D}");
        }
        if (targets.Count != 1)
        {
            throw new BridgeConflictException(
                "Managed Remark identity is duplicated or ambiguous in the active scene",
                request.ExpectedFingerprint);
        }
        var targetDtos = targets.Select(target => new MetadataDetachTargetDto(
                target.TypeName,
                target.Frame,
                target.Layer,
                target.Length,
                target.Remark,
                HashUtf8(target.Remark),
                ComputeNonRemarkContentDigest(target.Item)))
            .ToArray();
        var sceneRemarks = rawItems.Select(raw =>
            {
                var expectedRemark = raw.Remark;
                if (RemarkContainsIdentity(raw.Remark, request))
                {
                    expectedRemark = RemoveMetadataIdentity(
                        raw.Remark,
                        request,
                        out var matched);
                    if (!matched)
                    {
                        throw new BridgeConflictException(
                            "The approved TakeGraph Remark identity could not be projected exactly",
                            request.ExpectedFingerprint);
                    }
                }
                return CreateSceneRemarkWitness(raw, expectedRemark);
            })
            .ToArray();
        if (sceneRemarks.Select(value => value.StableItemWitness)
                .Distinct(StringComparer.Ordinal)
                .Count() != sceneRemarks.Length)
        {
            throw new BridgeConflictException(
                "Metadata detach scene contains ambiguous stable item witnesses",
                request.ExpectedFingerprint);
        }
        return new MetadataDetachPreparation(
            targets,
            targetDtos,
            sceneRemarks,
            MetadataDetachStore.ComputeSceneRemarkDigest(sceneRemarks, expected: false),
            MetadataDetachStore.ComputeSceneRemarkDigest(sceneRemarks, expected: true),
            ComputeSceneNonRemarkContentDigest(rawItems));
    }

    private static MetadataDetachReadback ReadMetadataDetachReadback(
        MetadataDetachRequestDto request,
        IReadOnlyList<MetadataDetachSceneRemarkDto> expectedScene)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var rawItems = ReadItems(timelineViewModel);
        var observed = rawItems
            .Select(raw => CreateSceneRemarkWitness(raw, raw.Remark))
            .ToArray();
        var observedWitnesses = observed
            .Select(value => value.StableItemWitness)
            .ToArray();
        var sceneWitnessesExact = observed.Length == expectedScene.Count
            && observedWitnesses.Distinct(StringComparer.Ordinal).Count() == observed.Length
            && observedWitnesses.ToHashSet(StringComparer.Ordinal).SetEquals(
                expectedScene.Select(value => value.StableItemWitness));
        return new MetadataDetachReadback(
            !rawItems.Any(item => RemarkContainsIdentity(item.Remark, request)),
            sceneWitnessesExact,
            MetadataDetachStore.ComputeSceneRemarkDigest(observed, expected: false),
            ComputeSceneNonRemarkContentDigest(rawItems));
    }

    private static MetadataDetachReadback? TryReadMetadataDetachReadback(
        MetadataDetachRequestDto request,
        IReadOnlyList<MetadataDetachSceneRemarkDto> expectedScene)
    {
        try
        {
            return Application.Current.Dispatcher.Invoke(() =>
                ReadMetadataDetachReadback(request, expectedScene));
        }
        catch
        {
            return null;
        }
    }

    private static MetadataDetachSceneRemarkDto CreateSceneRemarkWitness(
        RawItem raw,
        string expectedRemark)
    {
        var nonRemarkDigest = ComputeNonRemarkContentDigest(raw.Item);
        return new MetadataDetachSceneRemarkDto(
            MetadataDetachStore.ComputeStableItemWitness(
                raw.TypeName,
                raw.Frame,
                raw.Layer,
                raw.Length,
                nonRemarkDigest),
            raw.TypeName,
            raw.Frame,
            raw.Layer,
            raw.Length,
            nonRemarkDigest,
            raw.Remark,
            HashUtf8(raw.Remark),
            expectedRemark,
            HashUtf8(expectedRemark));
    }

    private static IReadOnlyList<RawItem> FindMetadataDetachTargets(
        IReadOnlyList<RawItem> rawItems,
        MetadataDetachRequestDto request)
    {
        var targets = new List<RawItem>();
        foreach (var raw in rawItems)
        {
            if (RemarkCodec.TryDecode(raw.Remark, out var voice) && voice is not null
                && voice.RealizationId == request.RealizationId)
            {
                EnsureMetadataMarkerBinding(request, voice.ProjectId, voice.EntityId);
                targets.Add(raw);
                continue;
            }
            if (NativeExtensionRemarkCodec.TryDecode(raw.Remark, out var extension)
                && extension is not null
                && (extension.RealizationId == request.RealizationId
                    || extension.Effects.Values.Any(effect =>
                        effect.RealizationId == request.RealizationId)))
            {
                EnsureMetadataMarkerBinding(request, extension.ProjectId, extension.EntityId);
                targets.Add(raw);
            }
        }
        return targets;
    }

    private static void EnsureMetadataMarkerBinding(
        MetadataDetachRequestDto request,
        string markerProjectId,
        string markerEntityId)
    {
        if (!string.Equals(markerProjectId, request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(markerEntityId, request.EntityId, StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "The requested realization identity belongs to another project or entity",
                SnapshotCore().Fingerprint);
        }
    }

    internal static string RemoveMetadataIdentity(
        string remark,
        MetadataDetachRequestDto request,
        out bool matched)
    {
        matched = false;
        if (RemarkCodec.TryDecode(remark, out var voice) && voice is not null
            && voice.RealizationId == request.RealizationId)
        {
            EnsureMetadataMarkerBinding(request, voice.ProjectId, voice.EntityId);
            matched = true;
            return RemarkCodec.Remove(remark, out _);
        }
        if (!NativeExtensionRemarkCodec.TryDecode(remark, out var extension)
            || extension is null)
        {
            return remark;
        }
        EnsureMetadataMarkerBinding(request, extension.ProjectId, extension.EntityId);
        var userRemark = NativeExtensionRemarkCodec.Remove(remark, out _);
        if (extension.RealizationId == request.RealizationId)
        {
            if (extension.Effects.Count != 0)
            {
                throw new BridgeConflictException(
                    "Detaching a base native-extension Remark that also carries effect identities requires explicit batch approval",
                    request.ExpectedFingerprint);
            }
            matched = true;
            return userRemark;
        }
        var remainingEffects = extension.Effects
            .Where(pair => pair.Value.RealizationId != request.RealizationId)
            .ToDictionary(pair => pair.Key, pair => pair.Value, StringComparer.Ordinal);
        if (remainingEffects.Count == extension.Effects.Count)
        {
            return remark;
        }
        matched = true;
        return NativeExtensionRemarkCodec.Append(
            userRemark,
            extension with { Effects = remainingEffects });
    }

    private static bool RemarkContainsIdentity(
        string remark,
        MetadataDetachRequestDto request)
    {
        if (RemarkCodec.TryDecode(remark, out var voice) && voice is not null
            && voice.RealizationId == request.RealizationId)
        {
            EnsureMetadataMarkerBinding(request, voice.ProjectId, voice.EntityId);
            return true;
        }
        if (NativeExtensionRemarkCodec.TryDecode(remark, out var extension)
            && extension is not null
            && (extension.RealizationId == request.RealizationId
                || extension.Effects.Values.Any(effect =>
                    effect.RealizationId == request.RealizationId)))
        {
            EnsureMetadataMarkerBinding(request, extension.ProjectId, extension.EntityId);
            return true;
        }
        return false;
    }

    private static string ComputeNonRemarkContentDigest(object item)
    {
        // Serialize/clone before clearing Remark so observing the digest never
        // transiently changes the live YMM object.
        var clone = DeserializeYmmObject(SerializeRecoveryItem(item));
        SetRequired(clone, string.Empty, "Remark");
        return HashUtf8(SerializeYmmObject(clone));
    }

    private static string ComputeSceneNonRemarkContentDigest(IReadOnlyList<RawItem> rawItems)
    {
        var canonical = new StringBuilder("takegraph-ymm4-scene-non-remark-v1\n");
        foreach (var witness in rawItems.Select(item => new
                 {
                     item.TypeName,
                     item.Frame,
                     item.Layer,
                     item.Length,
                     Digest = ComputeNonRemarkContentDigest(item.Item),
                 })
                 .OrderBy(value => value.TypeName, StringComparer.Ordinal)
                 .ThenBy(value => value.Frame)
                 .ThenBy(value => value.Layer)
                 .ThenBy(value => value.Length)
                 .ThenBy(value => value.Digest, StringComparer.Ordinal))
        {
            canonical.Append(witness.TypeName.Length).Append(':').Append(witness.TypeName)
                .Append('|').Append(witness.Frame)
                .Append('|').Append(witness.Layer)
                .Append('|').Append(witness.Length)
                .Append('|').Append(witness.Digest).Append('\n');
        }
        return HashUtf8(canonical.ToString());
    }

    private static void RestoreMetadataDetachTargets(MetadataDetachJournalDto entry)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var rawItems = ReadItems(timelineViewModel);
        var used = new HashSet<object>(ReferenceEqualityComparer.Instance);
        foreach (var target in entry.Targets)
        {
            var matches = rawItems.Where(raw =>
                    !used.Contains(raw.Item)
                    && raw.TypeName == target.TypeName
                    && raw.Frame == target.Frame
                    && raw.Layer == target.Layer
                    && raw.Length == target.Length
                    && ComputeNonRemarkContentDigest(raw.Item) == target.NonRemarkContentDigest)
                .ToArray();
            if (matches.Length != 1)
            {
                throw new BridgeUnavailableException(
                    "Metadata detach recovery target is missing or ambiguous");
            }
            used.Add(matches[0].Item);
            SetRequired(matches[0].Item, target.OriginalRemark, "Remark");
        }
    }

    private static string? TryReadSceneNonRemarkDigest()
    {
        try
        {
            return Application.Current.Dispatcher.Invoke(() =>
            {
                var main = RequireMainViewModel();
                var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
                    ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
                return ComputeSceneNonRemarkContentDigest(ReadItems(timelineViewModel));
            });
        }
        catch
        {
            return null;
        }
    }

    private static MetadataDetachResponseDto MetadataDetachResponse(
        MetadataDetachJournalDto entry,
        bool replayed)
    {
        var receipt = MetadataDetachStore.ToReceipt(entry);
        return new MetadataDetachResponseDto(
            receipt.Status == "verified",
            replayed,
            receipt);
    }

    private static void EnsureMetadataDetachBinding(
        MetadataDetachRequestDto request,
        MetadataDetachJournalDto entry)
    {
        if (!MetadataDetachStore.SameBinding(request, entry.Request))
        {
            throw new BridgeConflictException(
                "Metadata detach operation ID was already used for another approved request",
                entry.AfterFingerprint ?? entry.BeforeFingerprint);
        }
    }

    private static void ValidateMetadataDetachRequestShape(MetadataDetachRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        if (request.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint)
            || string.IsNullOrWhiteSpace(request.EntityId)
            || request.RealizationId == Guid.Empty
            || request.IdentityCarrier != "takegraph_remark_v2")
        {
            throw new BridgeValidationException(
                "Metadata detach requires complete project, source, identity, and Remark-carrier bindings");
        }
        var digest = ApplyRequestDigest.Compute(request);
        if (!ApplyRequestDigest.Matches(request.RequestDigest, digest))
        {
            throw new BridgeValidationException(
                "Metadata detach request digest does not match its payload");
        }
    }

    private static string HashUtf8(string value) =>
        Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(value)));

    private sealed record MetadataDetachPreparation(
        IReadOnlyList<RawItem> Targets,
        IReadOnlyList<MetadataDetachTargetDto> TargetDtos,
        IReadOnlyList<MetadataDetachSceneRemarkDto> SceneRemarks,
        string BeforeRemarkDigest,
        string ExpectedAfterRemarkDigest,
        string SceneNonRemarkDigest);

    private sealed record MetadataDetachReadback(
        bool RemarkAbsent,
        bool SceneWitnessesExact,
        string SceneRemarkDigest,
        string SceneNonRemarkDigest);
}
