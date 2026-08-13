using System.Reflection;
using System.Text.Json;
using System.Windows;

namespace TakeGraph.Ymm4Bridge;

internal sealed partial class Ymm4Facade
{
    private const string TimelineEditPlanDomain = "takegraph-timeline-edit-plan-v1";
    private const string TimelineEditRecoveryDriver = "timeline_edit_managed_cue_mixed";

    internal TimelineEditValidationDto ValidateTimelineEdit(
        TimelineEditValidationRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        if (request.Artifacts.Count != 0)
        {
            throw new BridgeValidationException(
                "This bridge profile does not yet support native-extension operations in timeline-edit");
        }
        var parsed = ParseAndValidateTimelineEdit(
            request.PlanDigest,
            request.TimelineEditPlan,
            validateCurrentScope: true,
            validateArtifacts: true);
        EnsureFingerprint(request.ExpectedFingerprint, parsed.Fingerprint);
        return parsed.Validation;
    }

    internal async Task<TimelineEditApplyResponseDto> ApplyTimelineEditAsync(
        TimelineEditApplyRequestDto request)
    {
        ValidateTimelineEditApplyRequestShape(request);
        var binding = ParseTimelineEditBinding(request.PlanDigest, request.TimelineEditPlan);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (receiptStore.TryGet(binding.OperationId, out var stored)
                && stored is not null)
            {
                EnsureTimelineEditReceiptBinding(request, binding, stored);
                if (stored.Status != "applying")
                {
                return WrapTimelineEditResponse(
                    request,
                    binding,
                        stored,
                        replayed: true,
                        stored.Verified ? binding.OperationCount : 0);
                }
            }

            ValidateMutationRuntime();
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            if (receiptStore.TryGet(binding.OperationId, out stored)
                && stored is not null
                && stored.Status != "applying")
            {
                EnsureTimelineEditReceiptBinding(request, binding, stored);
                return WrapTimelineEditResponse(
                    request,
                    binding,
                    stored,
                    replayed: true,
                    stored.Verified ? binding.OperationCount : 0);
            }
            var before = Snapshot();
            EnsureTarget(binding.ProjectId, binding.SceneId, before);
            EnsureFingerprint(request.ExpectedFingerprint, before.Fingerprint);

            // Authorization is rechecked under the single writer gate. The
            // parser verifies the top-level target, capability, and whole-scene
            // scope, while child operations carry only resolved driver facts.
            var parsed = ParseAndValidateTimelineEdit(
                request.PlanDigest,
                request.TimelineEditPlan,
                validateCurrentScope: true,
                validateArtifacts: true);
            EnsureFingerprint(request.ExpectedFingerprint, parsed.Fingerprint);

            using var portableArtifacts = MaterializePortableArtifacts(parsed.PortableUtterances);
            parsed = parsed with { PortableUtterances = portableArtifacts.Utterances };

            receiptStore.Put(new OperationReceiptDto(
                parsed.OperationId,
                request.RequestDigest,
                parsed.ProjectId,
                parsed.SceneId,
                request.ExpectedFingerprint,
                "applying",
                before.Fingerprint,
                before.Fingerprint,
                [],
                false,
                null));

            TimelineEditPreparation? preparation = null;
            var mutationStarted = false;
            var successfulReadbackDurable = false;
            try
            {
                preparation = await Application.Current.Dispatcher.InvokeAsync(() =>
                    PrepareTimelineEdit(parsed));
                var preMutation = Snapshot();
                EnsureTarget(parsed.ProjectId, parsed.SceneId, preMutation);
                EnsureFingerprint(request.ExpectedFingerprint, preMutation.Fingerprint);

                var expectedItems = parsed.PortableUtterances
                    .SelectMany(ToManagedItems)
                    .Concat(parsed.NativeVoiceCues.Select(ToExpectedNativeVoiceItem))
                    .ToArray();
                recoveryStore.Put(CreateRecoveryEntry(
                    parsed.OperationId,
                    request.RequestDigest,
                    parsed.ProjectId,
                    parsed.SceneId,
                    request.ExpectedFingerprint,
                    before.Fingerprint,
                    TimelineEditRecoveryDriver,
                    parsed.PortableUtterances.Select(value => value.EntityId)
                        .Concat(parsed.NativeVoiceCues.Select(value => value.EntityId))
                        .ToArray(),
                    parsed.NativeVoiceCues.Select(value => value.RealizationId).ToArray(),
                    expectedItems,
                    new Dictionary<Guid, string>(),
                    preparation.BeforeItems));
                BridgeFaultInjection.ThrowIf("after_journal_before_mutation");

                mutationStarted = true;
                await ApplyTimelineEditOperationsAsync(preparation, parsed.ProjectId)
                    .ConfigureAwait(false);
                BridgeFaultInjection.ThrowIf("after_mutation_before_readback");
                var after = Snapshot();
                portableArtifacts.Verify();
                var applied = ReadTimelineEditItems(parsed, after);
                if (!TimelineEditItemsMatch(parsed, applied))
                {
                    return await RollBackTimelineEditAsync(
                        request,
                        parsed,
                        before,
                        preparation,
                        "YMM4 timeline-edit read-back did not match every approved operation")
                        .ConfigureAwait(false);
                }

                var receipt = new OperationReceiptDto(
                    parsed.OperationId,
                    request.RequestDigest,
                    parsed.ProjectId,
                    parsed.SceneId,
                    request.ExpectedFingerprint,
                    "verified",
                    before.Fingerprint,
                    after.Fingerprint,
                    applied,
                    true,
                    null);
                recoveryStore.MarkAppliedUnverified(
                    parsed.OperationId,
                    after.Fingerprint,
                    applied);
                successfulReadbackDurable = true;
                receiptStore.Put(receipt);
                BridgeFaultInjection.ThrowIf("after_receipt_before_finalize");
                recoveryStore.Transition(parsed.OperationId, "verified", after.Fingerprint, null);
                lock (historyGate)
                {
                    lastBatch = new AppliedBatch(
                        preparation.Timeline,
                        preparation.BeforeItems,
                        preparation.CurrentItems());
                    lastBatchUndone = false;
                }
                    return WrapTimelineEditResponse(
                        request,
                        binding,
                    receipt,
                    replayed: false,
                    parsed.Operations.Count);
            }
            catch (BridgeSimulatedCrashException)
            {
                throw;
            }
            catch (Exception error)
            {
                if (successfulReadbackDurable)
                {
                    throw;
                }
                if (mutationStarted && preparation is not null)
                {
                    return await RollBackTimelineEditAsync(
                        request,
                        parsed,
                        before,
                        preparation,
                        error.GetBaseException().Message).ConfigureAwait(false);
                }
                var current = TrySnapshot() ?? before;
                var unchanged = current.Fingerprint == before.Fingerprint;
                var failed = new OperationReceiptDto(
                    parsed.OperationId,
                    request.RequestDigest,
                    parsed.ProjectId,
                    parsed.SceneId,
                    request.ExpectedFingerprint,
                    unchanged ? "failed" : "recovery_required",
                    before.Fingerprint,
                    current.Fingerprint,
                    ReadTimelineEditItems(parsed, current),
                    false,
                    error.GetBaseException().Message);
                receiptStore.Put(failed);
                recoveryStore.TryTransition(
                    parsed.OperationId,
                    failed.Status,
                    current.Fingerprint,
                    failed.Error);
                return WrapTimelineEditResponse(
                    request,
                    binding,
                    failed,
                    replayed: false,
                    0);
            }
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal async Task<TimelineEditApplyResponseDto> SealTimelineEditNotStartedAsync(
        TimelineEditApplyRequestDto request)
    {
        ValidateTimelineEditApplyRequestShape(request);
        var binding = ParseTimelineEditBinding(request.PlanDigest, request.TimelineEditPlan);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (receiptStore.TryGet(binding.OperationId, out var existing)
                && existing is not null)
            {
                EnsureTimelineEditReceiptBinding(request, binding, existing);
                return WrapTimelineEditResponse(
                    request,
                    binding,
                    existing,
                    replayed: true,
                    existing.Verified ? binding.OperationCount : 0);
            }
            var tombstone = CreateNotStartedReceipt(
                binding.OperationId,
                request.RequestDigest,
                binding.ProjectId,
                binding.SceneId,
                request.ExpectedFingerprint,
                "Timeline-edit apply did not start; a durable no-mutation tombstone was sealed");
            receiptStore.Put(tombstone);
            return WrapTimelineEditResponse(request, binding, tombstone, false, 0);
        }
        finally
        {
            applyGate.Release();
        }
    }

    private static void ValidateTimelineEditApplyRequestShape(
        TimelineEditApplyRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        _ = RequireSha256(request.PlanDigest, "planDigest");
        _ = RequireSha256(request.ExpectedFingerprint, "expectedFingerprint");
        _ = RequireSha256(request.RequestDigest, "requestDigest");
        if (request.TimelineEditPlan.ValueKind != JsonValueKind.Object
            || request.Artifacts.Count != 0)
        {
            throw new BridgeValidationException(
                "Timeline-edit requires an object plan and currently supports no native-extension artifacts");
        }
        var calculated = ApplyRequestDigest.Compute(request);
        if (!ApplyRequestDigest.Matches(request.RequestDigest, calculated))
        {
            throw new BridgeValidationException(
                "Timeline-edit request digest does not match its canonical payload");
        }
    }

    internal static void ValidateTimelineEditHistoricalBindingForTests(
        string declaredDigest,
        JsonElement root) => _ = ParseTimelineEditBinding(declaredDigest, root);

    private static TimelineEditBinding ParseTimelineEditBinding(
        string declaredDigest,
        JsonElement root)
    {
        RequireExactJsonProperties(
            root,
            "canonicalVersion",
            "operationId",
            "baseRevision",
            "target",
            "capabilityDigest",
            "expectedScope",
            "changeBudget",
            "operations",
            "warnings");
        if (RequireJsonInt(root, "canonicalVersion") != 1)
        {
            throw new BridgeValidationException("Unsupported timeline-edit canonicalVersion");
        }
        var operationId = RequireJsonGuid(root, "operationId");
        if (operationId == Guid.Empty)
        {
            throw new BridgeValidationException("Timeline-edit operationId must not be empty");
        }
        var planDigest = RequireSha256(declaredDigest, "planDigest");
        if (!ApplyRequestDigest.Matches(
                planDigest,
                CanonicalJson.Sha256(TimelineEditPlanDomain, root)))
        {
            throw new BridgeValidationException(
                "Timeline-edit plan digest does not match its canonical payload");
        }
        var target = RequireJsonObject(root, "target");
        RequireExactJsonProperties(
            target,
            "adapterId",
            "projectId",
            "sceneId",
            "fps",
            "driverVersion");
        var operationCount = RequireJsonArray(root, "operations").GetArrayLength();
        if (operationCount is < 1 or > 128)
        {
            throw new BridgeValidationException("Timeline-edit operation count is invalid");
        }
        return new TimelineEditBinding(
            operationId,
            RequireJsonString(target, "projectId"),
            RequireJsonString(target, "sceneId"),
            operationCount);
    }

    private ParsedTimelineEdit ParseAndValidateTimelineEdit(
        string declaredDigest,
        JsonElement root,
        bool validateCurrentScope,
        bool validateArtifacts)
    {
        RequireExactJsonProperties(
            root,
            "canonicalVersion",
            "operationId",
            "baseRevision",
            "target",
            "capabilityDigest",
            "expectedScope",
            "changeBudget",
            "operations",
            "warnings");
        if (RequireJsonInt(root, "canonicalVersion") != 1)
        {
            throw new BridgeValidationException("Unsupported timeline-edit canonicalVersion");
        }
        var operationId = RequireJsonGuid(root, "operationId");
        if (operationId == Guid.Empty)
        {
            throw new BridgeValidationException("Timeline-edit operationId must not be empty");
        }
        _ = RequireJsonULong(root, "baseRevision");
        var planDigest = RequireSha256(declaredDigest, "planDigest");
        var calculatedDigest = CanonicalJson.Sha256(TimelineEditPlanDomain, root);
        if (!ApplyRequestDigest.Matches(planDigest, calculatedDigest))
        {
            throw new BridgeValidationException(
                "Timeline-edit plan digest does not match its canonical payload");
        }

        var snapshot = Snapshot();
        var target = RequireJsonObject(root, "target");
        RequireExactJsonProperties(
            target,
            "adapterId",
            "projectId",
            "sceneId",
            "fps",
            "driverVersion");
        var projectId = RequireJsonString(target, "projectId");
        var sceneId = RequireJsonString(target, "sceneId");
        var pluginVersion = typeof(Ymm4Facade).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
            ?? typeof(Ymm4Facade).Assembly.GetName().Version?.ToString()
            ?? "unknown";
        var ymm4Version = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "unknown";
        var expectedDriverVersion = $"{ymm4Version}/{pluginVersion}";
        if (RequireJsonString(target, "adapterId") != "ymm4-4.55"
            || projectId != snapshot.ProjectId
            || sceneId != snapshot.SceneId
            || RequireJsonInt(target, "fps") != snapshot.Fps
            || RequireJsonString(target, "driverVersion") != expectedDriverVersion)
        {
            throw new BridgeConflictException(
                "Timeline-edit target identity differs from the active YMM4 target",
                snapshot.Fingerprint);
        }
        var currentCapabilityDigest = ComputeStructuredCapabilityDigest(
            Capabilities().Capabilities.ToHashSet(StringComparer.Ordinal),
            pluginVersion,
            ymm4Version);
        if (!ApplyRequestDigest.Matches(
                RequireSha256(RequireJsonString(root, "capabilityDigest"), "capabilityDigest"),
                currentCapabilityDigest))
        {
            throw new BridgeConflictException(
                "Timeline-edit capability contract changed after approval",
                snapshot.Fingerprint);
        }
        ValidateTargetPlanScope(
            root,
            target,
            snapshot,
            snapshot.Fingerprint,
            validateCurrentScope);

        var budget = RequireJsonObject(root, "changeBudget");
        RequireExactJsonProperties(
            budget,
            "maxChangedEntities",
            "maxShiftedEntities",
            "maxShiftFrames",
            "allowLockedChanges",
            "allowUnmanagedChanges");
        var maxChanged = RequirePositiveJsonInt(budget, "maxChangedEntities");
        if (RequireNonNegativeJsonInt(budget, "maxShiftedEntities") != 0
            || RequireNonNegativeJsonInt(budget, "maxShiftFrames") != 0
            || RequireJsonBoolean(budget, "allowLockedChanges")
            || RequireJsonBoolean(budget, "allowUnmanagedChanges"))
        {
            throw new BridgeValidationException(
                "Timeline-edit supports only fixed, locked-safe, managed-only plans");
        }

        var operationValues = RequireJsonArray(root, "operations").EnumerateArray().ToArray();
        if (operationValues.Length is < 1 or > 128 || operationValues.Length > maxChanged)
        {
            throw new BridgeValidationException(
                "Timeline-edit operations must fit the approved 1-128 entity change budget");
        }
        var portable = new List<ManagedUtteranceDto>();
        var native = new List<NativeVoiceCueDto>();
        var operations = new List<TimelineManagedOperation>();
        var actions = new List<string>();
        var entityIds = new HashSet<string>(StringComparer.Ordinal);
        var realizationIds = new HashSet<Guid>();
        foreach (var operationValue in operationValues)
        {
            RequireExactJsonProperties(operationValue, "kind", "cue");
            if (RequireJsonString(operationValue, "kind") != "managed_cue")
            {
                throw new BridgeValidationException(
                    "This bridge profile supports only managed_cue timeline-edit operations; native_extension fails closed");
            }
            var beforePortable = portable.Count;
            var beforeNative = native.Count;
            ParseTargetPlanCue(
                RequireJsonObject(operationValue, "cue"),
                portable,
                native,
                actions,
                entityIds,
                realizationIds,
                validateCurrentCapability: true,
                timelineEdit: true);
            operations.Add(portable.Count > beforePortable
                ? new TimelineManagedOperation("portable_pair", actions[^1], portable[^1], null)
                : new TimelineManagedOperation("native_voice", actions[^1], null, native[^1]));
            if (native.Count == beforeNative && portable.Count == beforePortable)
            {
                throw new BridgeValidationException("Timeline-edit operation produced no driver payload");
            }
        }

        foreach (var warning in RequireJsonArray(root, "warnings").EnumerateArray())
        {
            RequireExactJsonProperties(warning, "code", "message");
            _ = RequireJsonString(warning, "code");
            _ = RequireJsonString(warning, "message");
        }
        if (validateCurrentScope)
        {
            ValidatePortableTargetPlanActions(
                portable,
                operations.Where(value => value.Kind == "portable_pair")
                    .Select(value => value.Action)
                    .ToArray(),
                snapshot.ManagedItems,
                snapshot.Fingerprint);
            EnsureNativeVoiceEntitiesAreNew(native, snapshot);
            if (portable.Any(value => snapshot.NativeExtensions.Any(extension =>
                    extension.EntityId == value.EntityId)))
            {
                throw new BridgeConflictException(
                    "Timeline-edit managed cue identity collides with a native extension",
                    snapshot.Fingerprint);
            }
            ValidateUtterances(portable, verifyArtifacts: validateArtifacts);
            Application.Current.Dispatcher.Invoke(() =>
            {
                if (portable.Count > 0)
                {
                    _ = PrepareApplyBatch(portable);
                }
                if (native.Count > 0)
                {
                    _ = PrepareNativeVoiceBatch(native, projectId);
                }
            });
        }
        else
        {
            ValidateUtterances(portable, verifyArtifacts: false);
            ValidateNativeVoiceCues(native);
        }
        var strategyCounts = new SortedDictionary<string, int>(StringComparer.Ordinal);
        if (portable.Count > 0) strategyCounts["portable_pair"] = portable.Count;
        if (native.Count > 0) strategyCounts["native_voice"] = native.Count;
        return new ParsedTimelineEdit(
            operationId,
            planDigest,
            projectId,
            sceneId,
            snapshot.Fingerprint,
            operations,
            portable,
            native,
            new TimelineEditValidationDto(
                operationId,
                planDigest,
                snapshot.Fingerprint,
                strategyCounts,
                actions.Count(value => value == "create"),
                actions.Count(value => value == "update"),
                actions.Count(value => value == "delete"),
                portable.Count * 2 + native.Count));
    }

    private static TimelineEditPreparation PrepareTimelineEdit(ParsedTimelineEdit parsed)
    {
        var portable = parsed.PortableUtterances.ToDictionary(
            value => value.EntityId,
            value => PrepareApplyBatch([value]),
            StringComparer.Ordinal);
        NativeVoicePreparation? native = parsed.NativeVoiceCues.Count == 0
            ? null
            : PrepareNativeVoiceBatch(parsed.NativeVoiceCues, parsed.ProjectId);
        var timeline = portable.Values.FirstOrDefault()?.Timeline ?? native?.Timeline
            ?? throw new BridgeValidationException("Timeline-edit has no supported operation");
        if (portable.Values.Any(value => !ReferenceEquals(value.Timeline, timeline))
            || native is not null && !ReferenceEquals(native.Timeline, timeline))
        {
            throw new BridgeUnavailableException(
                "Timeline changed while the heterogeneous transaction was being prepared");
        }
        var before = portable.Values.SelectMany(value => value.OldItems)
            .Distinct(ReferenceEqualityComparer.Instance)
            .ToArray();
        return new TimelineEditPreparation(parsed.Operations, timeline, before, portable, native);
    }

    private static async Task ApplyTimelineEditOperationsAsync(
        TimelineEditPreparation preparation,
        string projectId)
    {
        foreach (var operation in preparation.Operations)
        {
            if (operation.Kind == "portable_pair")
            {
                var batch = preparation.Portable[operation.Portable!.EntityId];
                await Application.Current.Dispatcher.InvokeAsync(() =>
                    ApplyPreparedBatch(batch));
                preparation.MarkApplied(batch.NewItems);
                if (preparation.Native is not null)
                {
                    foreach (var item in batch.NewItems)
                    {
                        preparation.Native.KnownItems.Add(item);
                    }
                }
                BridgeFaultInjection.ThrowIf("after_partial_mutation");
                continue;
            }
            await ApplyNativeVoiceBatchAsync(
                CreateSingleNativePreparation(preparation.Native!, operation.Native!),
                projectId).ConfigureAwait(false);
        }
    }

    private static NativeVoicePreparation CreateSingleNativePreparation(
        NativeVoicePreparation shared,
        NativeVoiceCueDto cue) => new(
            shared.MainModel,
            shared.TimelineViewModel,
            shared.Timeline,
            shared.AddMethod,
            shared.Decorations,
            [cue],
            new Dictionary<Guid, object> { [cue.RealizationId] = shared.Characters[cue.RealizationId] },
            shared.KnownItems,
            shared.AddedItems);

    private async Task<TimelineEditApplyResponseDto> RollBackTimelineEditAsync(
        TimelineEditApplyRequestDto request,
        ParsedTimelineEdit parsed,
        ProjectSnapshotDto before,
        TimelineEditPreparation preparation,
        string failure)
    {
        string? rollbackError = null;
        try
        {
            BridgeFaultInjection.ThrowIf("during_rollback");
            await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                var current = preparation.CurrentItems();
                ReplaceBatchItems(preparation.Timeline, current, preparation.BeforeItems);
            });
        }
        catch (Exception error)
        {
            rollbackError = error.GetBaseException().Message;
        }
        var currentSnapshot = TrySnapshot() ?? before;
        var restored = rollbackError is null
            && currentSnapshot.Fingerprint == before.Fingerprint;
        var receipt = new OperationReceiptDto(
            parsed.OperationId,
            request.RequestDigest,
            parsed.ProjectId,
            parsed.SceneId,
            request.ExpectedFingerprint,
            restored ? "rolled_back" : "recovery_required",
            before.Fingerprint,
            currentSnapshot.Fingerprint,
            ReadTimelineEditItems(parsed, currentSnapshot),
            false,
            rollbackError is null ? failure : $"{failure}; rollback failed: {rollbackError}");
        receiptStore.Put(receipt);
        recoveryStore.TryTransition(
            parsed.OperationId,
            receipt.Status,
            currentSnapshot.Fingerprint,
            receipt.Error);
        return WrapTimelineEditResponse(
            request,
            new TimelineEditBinding(
                parsed.OperationId,
                parsed.ProjectId,
                parsed.SceneId,
                parsed.Operations.Count),
            receipt,
            false,
            0);
    }

    private static ManagedItemDto[] ReadTimelineEditItems(
        ParsedTimelineEdit parsed,
        ProjectSnapshotDto snapshot)
    {
        var entities = parsed.PortableUtterances.Select(value => value.EntityId)
            .ToHashSet(StringComparer.Ordinal);
        var realizations = parsed.NativeVoiceCues
            .Select(value => value.RealizationId.ToString("D"))
            .ToHashSet(StringComparer.OrdinalIgnoreCase);
        return snapshot.ManagedItems.Where(item =>
                entities.Contains(item.EntityId)
                || item.RealizationId is not null && realizations.Contains(item.RealizationId))
            .ToArray();
    }

    private static bool TimelineEditItemsMatch(
        ParsedTimelineEdit parsed,
        IReadOnlyList<ManagedItemDto> actual)
    {
        var portableActual = actual.Where(item => parsed.PortableUtterances.Any(value =>
            value.EntityId == item.EntityId)).ToArray();
        var portableExpected = parsed.PortableUtterances.SelectMany(ToManagedItems).ToArray();
        var nativeActual = actual.Where(item => item.RealizationId is not null).ToArray();
        return Equivalent(portableActual, portableExpected)
            && NativeVoiceItemsMatch(parsed.NativeVoiceCues, nativeActual);
    }

    private static ManagedItemDto ToExpectedNativeVoiceItem(NativeVoiceCueDto cue) => new(
        cue.EntityId,
        cue.Revision,
        "voice",
        cue.Frame,
        cue.Layer,
        cue.MaxLength,
        cue.DisplayText,
        null,
        null,
        cue.CharacterName,
        cue.RealizationId.ToString("D"));

    private static void EnsureTimelineEditReceiptBinding(
        TimelineEditApplyRequestDto request,
        TimelineEditBinding binding,
        OperationReceiptDto receipt)
    {
        EnsureReceiptBinding(
            request.RequestDigest,
            binding.ProjectId,
            binding.SceneId,
            request.ExpectedFingerprint,
            receipt,
            receipt.AfterFingerprint);
    }

    private static TimelineEditApplyResponseDto WrapTimelineEditResponse(
        TimelineEditApplyRequestDto request,
        TimelineEditBinding binding,
        OperationReceiptDto receipt,
        bool replayed,
        int appliedOperationCount) => new(
            receipt.Verified,
            replayed,
            new TimelineEditReceiptDto(
                receipt.OperationId,
                receipt.RequestDigest,
                receipt.ProjectId,
                receipt.SceneId,
                receipt.ExpectedFingerprint,
                request.PlanDigest,
                receipt.Status,
                receipt.BeforeFingerprint,
                receipt.AfterFingerprint,
                receipt.AppliedItems,
                [],
                appliedOperationCount,
                receipt.Verified,
                receipt.Error));

    private sealed record TimelineManagedOperation(
        string Kind,
        string Action,
        ManagedUtteranceDto? Portable,
        NativeVoiceCueDto? Native);

    private sealed record TimelineEditBinding(
        Guid OperationId,
        string ProjectId,
        string SceneId,
        int OperationCount);

    private sealed record ParsedTimelineEdit(
        Guid OperationId,
        string PlanDigest,
        string ProjectId,
        string SceneId,
        string Fingerprint,
        IReadOnlyList<TimelineManagedOperation> Operations,
        IReadOnlyList<ManagedUtteranceDto> PortableUtterances,
        IReadOnlyList<NativeVoiceCueDto> NativeVoiceCues,
        TimelineEditValidationDto Validation);

    private sealed class TimelineEditPreparation(
        IReadOnlyList<TimelineManagedOperation> operations,
        object timeline,
        object[] beforeItems,
        IReadOnlyDictionary<string, AppliedBatch> portable,
        NativeVoicePreparation? native)
    {
        internal IReadOnlyList<TimelineManagedOperation> Operations { get; } = operations;
        internal object Timeline { get; } = timeline;
        internal object[] BeforeItems { get; } = beforeItems;
        internal IReadOnlyDictionary<string, AppliedBatch> Portable { get; } = portable;
        internal NativeVoicePreparation? Native { get; } = native;
        private List<object> AppliedItems { get; } = [];
        internal void MarkApplied(IEnumerable<object> items) => AppliedItems.AddRange(items);
        internal object[] CurrentItems() =>
            AppliedItems.Concat(Native?.AddedItems ?? [])
                .ToArray();
    }
}
