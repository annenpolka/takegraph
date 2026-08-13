using System.Collections;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Windows;
using System.Windows.Input;

namespace TakeGraph.Ymm4Bridge;

internal sealed partial class Ymm4Facade
{
    private const string SupportedMutationYmm4Version = "4.55.1.1";
    private const string SceneCaptureDriver = "ymm4-preview-save-image/4.55.1.1";
    private const string CheckpointDriver = "ymm4-existing-path-save-checkpoint/4.55.1.1";
    private const string ProjectInitializationDriver =
        "ymm4-project-json-atomic-claim-rebind/4.55.1.1";
    private const string RenderDriver = "ymm4-command-line-encoder/4.55.1.1";
    private const string FinalMediaProbeProfile = "takegraph-final-media-probe/mp4-v3";
    private static readonly string[] TemplateMenuPropertyNames =
    [
        "AddTemplateContextMenuViewModel",
        "AddVoiceItemTemplateContextMenuViewModel",
        "AddTextItemTemplateContextMenuViewModel",
        "AddVideoItemTemplateContextMenuViewModel",
        "AddAudioItemTemplateContextMenuViewModel",
        "AddImageItemTemplateContextMenuViewModel",
        "AddTachieItemTemplateContextMenuViewModel",
        "AddFaceItemTemplateContextMenuViewModel",
        "AddEffectItemTemplateContextMenuViewModel",
        "AddSceneTimelineTemplateContextMenuViewModel",
    ];
    private readonly SemaphoreSlim applyGate = new(1, 1);
    private readonly ReceiptStore receiptStore;
    private readonly RecoveryJournalStore recoveryStore;
    private readonly MetadataDetachStore metadataDetachStore;
    private readonly ProjectOperationStore projectOperationStore;
    private readonly ConcurrentDictionary<Guid, RenderExecutionControl> renderExecutions = new();
    private readonly object historyGate = new();
    private AppliedBatch? lastBatch;
    private bool lastBatchUndone;
    private readonly object projectInstanceGate = new();
    private object? observedProjectInstance;
    private string? observedProjectInstanceId;

    internal Ymm4Facade()
        : this(
            new ReceiptStore(),
            new RecoveryJournalStore(),
            new MetadataDetachStore(),
            new ProjectOperationStore())
    {
    }

    internal Ymm4Facade(
        ReceiptStore receiptStore,
        RecoveryJournalStore recoveryStore,
        MetadataDetachStore metadataDetachStore,
        ProjectOperationStore projectOperationStore)
    {
        this.receiptStore = receiptStore;
        this.recoveryStore = recoveryStore;
        this.metadataDetachStore = metadataDetachStore;
        this.projectOperationStore = projectOperationStore;
    }

    internal Task<bool> RecoverPendingOperationsForTestsAsync() =>
        RecoverPendingJournalsCoreAsync();

    internal ProjectSnapshotDto Snapshot()
    {
        return Application.Current.Dispatcher.Invoke(SnapshotCore);
    }

    /// Observes the current preview/timeline state in one dispatcher turn. A
    /// read barrier waits for any in-flight mutation batch before dispatching;
    /// the observer itself performs no seek, save, selection, or dirty-state
    /// operation.
    internal async Task<SceneCompositionSnapshotDto> CurrentSceneCompositionAsync()
    {
        ValidateObservationRuntime();
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            return await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                var main = RequireMainViewModel();
                var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
                    ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
                var rawItems = ReadItems(timelineViewModel);
                var projectPath = GetString(main, "ProjectFilePath", "ProjectPath");
                var projectId = Hash($"project|{projectPath}");
                var sceneId = GetString(timelineViewModel, "ID", "Id", "SceneId");
                if (string.IsNullOrWhiteSpace(sceneId))
                {
                    throw new BridgeUnavailableException(
                        "YMM4 active scene identity is unavailable for source-bound composition");
                }
                var fps = FindExactFps(main, timelineViewModel)
                    ?? throw new BridgeUnavailableException("YMM4 scene FPS is unavailable");
                var preview = RequirePreviewViewModel();
                var frame = ReadExactPreviewFrame(preview, fps)
                    ?? throw new BridgeUnavailableException("YMM4 current preview frame is unavailable");
                var fingerprint = Fingerprint(rawItems, projectPath, sceneId, fps);
                return BuildSceneCompositionSnapshot(
                    projectId,
                    sceneId,
                    fingerprint,
                    fps,
                    frame,
                    rawItems);
            });
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal static SceneCompositionSnapshotDto BuildSceneCompositionSnapshot(
        string projectId,
        string sceneId,
        string sourceFingerprint,
        uint fps,
        int frame,
        IReadOnlyList<RawItem> rawItems)
    {
        if (string.IsNullOrWhiteSpace(projectId)
            || string.IsNullOrWhiteSpace(sceneId)
            || string.IsNullOrWhiteSpace(sourceFingerprint)
            || fps == 0
            || frame < 0)
        {
            throw new BridgeUnavailableException(
                "Current scene composition identity or preview frame is unavailable");
        }
        if (rawItems.Any(item => item.Frame < 0 || item.Layer < 0 || item.Length <= 0))
        {
            throw new BridgeUnavailableException(
                "YMM4 timeline item placement is invalid for composition observation");
        }

        var candidates = rawItems
            .Select(item => CreateSceneCompositionCandidate(item, projectId))
            .ToArray();
        var preferredCounts = candidates
            .Where(candidate => candidate.PreferredId is not null)
            .GroupBy(candidate => candidate.PreferredId!, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.Count(), StringComparer.Ordinal);
        var active = candidates
            .Where(candidate => (long)candidate.Item.Frame <= frame
                && frame < (long)candidate.Item.Frame + candidate.Item.Length)
            .ToArray();
        var fallbackOrdinals = new Dictionary<string, int>(StringComparer.Ordinal);
        var usedIds = new HashSet<string>(StringComparer.Ordinal);
        var elements = new List<SceneCompositionElementDto>(active.Length);
        foreach (var candidate in active)
        {
            var item = candidate.Item;
            var hasUniqueRealizationIdentity = candidate.PreferredId is not null
                && preferredCounts[candidate.PreferredId] == 1;
            var stability = hasUniqueRealizationIdentity
                ? "realization_identity"
                : "session_only";
            var elementId = hasUniqueRealizationIdentity
                ? candidate.PreferredId!
                : AllocateSessionElementId(item, fallbackOrdinals, usedIds);
            if (!usedIds.Add(elementId))
            {
                // A duplicate/corrupt marker must never produce an ambiguous wire
                // identity. Downgrade it to a session-only content identity.
                stability = "session_only";
                elementId = AllocateSessionElementId(item, fallbackOrdinals, usedIds);
                _ = usedIds.Add(elementId);
            }
            elements.Add(new SceneCompositionElementDto(
                elementId,
                stability,
                candidate.Kind,
                item.Frame,
                item.Layer,
                item.Length,
                true,
                item.SelectionAvailable ? item.Selected : null,
                candidate.Text,
                new SceneCompositionVisualDto("unavailable", null, null, null, null)));
        }

        var unavailableFields = new SortedSet<string>(StringComparer.Ordinal)
        {
            "elements[].anchor",
            "elements[].crop",
            "elements[].maskAndParentRelations",
            "elements[].opacity",
            "elements[].paintOrder",
            "elements[].rotation",
            "elements[].scale",
            "elements[].visual",
            "elements[].visibility",
            "viewport",
        };
        if (active.Any(candidate => !candidate.Item.SelectionAvailable))
        {
            unavailableFields.Add("elements[].selected");
        }
        if (active.Any(candidate => candidate.TextWithheld))
        {
            unavailableFields.Add("elements[].text");
        }
        return new SceneCompositionSnapshotDto(
            1,
            projectId,
            sceneId,
            sourceFingerprint,
            fps,
            frame,
            new SceneCompositionViewportDto("unavailable", null, null),
            elements
                .OrderBy(element => element.Layer)
                .ThenBy(element => element.ElementId, StringComparer.Ordinal)
                .ToArray(),
            "partial",
            unavailableFields.ToArray());
    }

    private static SceneCompositionCandidate CreateSceneCompositionCandidate(
        RawItem item,
        string projectId)
    {
        string? preferredId = null;
        string? explicitKind = null;
        if (NativeExtensionRemarkCodec.TryDecode(item.Remark, out var extension)
            && extension is not null
            && string.Equals(extension.ProjectId, projectId, StringComparison.Ordinal))
        {
            preferredId = $"realization:{extension.RealizationId:N}:part:{extension.PartIndex}";
            explicitKind = extension.Kind;
        }
        else if (RemarkCodec.TryDecode(item.Remark, out var voice)
            && voice is not null
            && string.Equals(voice.ProjectId, projectId, StringComparison.Ordinal))
        {
            preferredId = $"realization:{voice.RealizationId:N}:part:0";
            explicitKind = "voice";
        }

        string? text = null;
        if (MarkerCodec.TryDecode(item.Text, out var caption, out var portable)
            && portable is not null)
        {
            text = caption;
            explicitKind ??= "caption";
        }
        else if (preferredId is not null
            && string.Equals(explicitKind, "voice", StringComparison.Ordinal))
        {
            text = item.Text;
        }
        else if (preferredId is not null
            && string.Equals(explicitKind, "caption", StringComparison.Ordinal))
        {
            text = item.Text;
        }
        var normalizedText = string.IsNullOrWhiteSpace(text) ? null : text;
        return new SceneCompositionCandidate(
            item,
            preferredId,
            string.IsNullOrWhiteSpace(explicitKind)
                ? SceneCompositionKind(item.TypeName)
                : explicitKind,
            normalizedText,
            normalizedText is null && !string.IsNullOrWhiteSpace(item.Text));
    }

    private static string SceneCompositionKind(string typeName)
    {
        var separator = typeName.LastIndexOf('.');
        var kind = separator >= 0 ? typeName[(separator + 1)..] : typeName;
        if (kind.EndsWith("Item", StringComparison.Ordinal) && kind.Length > "Item".Length)
        {
            kind = kind[..^"Item".Length];
        }
        return string.IsNullOrWhiteSpace(kind) ? "unknown" : kind.ToLowerInvariant();
    }

    private static string AllocateSessionElementId(
        RawItem item,
        IDictionary<string, int> fallbackOrdinals,
        IReadOnlySet<string> usedIds)
    {
        var contentId = Hash(string.Join('|',
            item.TypeName,
            item.Frame,
            item.Layer,
            item.Length,
            item.GroupId,
            item.Text,
            item.AudioPath,
            item.Remark,
            item.CharacterName,
            item.SpokenText));
        _ = fallbackOrdinals.TryGetValue(contentId, out var next);
        string candidate;
        do
        {
            candidate = $"session:{contentId}:{next:D4}";
            next++;
        }
        while (usedIds.Contains(candidate));
        fallbackOrdinals[contentId] = next;
        return candidate;
    }

    private sealed record SceneCompositionCandidate(
        RawItem Item,
        string? PreferredId,
        string Kind,
        string? Text,
        bool TextWithheld);

    internal void BeginStartupRecovery()
    {
        _ = Task.Run(async () =>
        {
            RecoverInterruptedRenderTasks();
            for (var attempt = 0; attempt < 60; attempt++)
            {
                try
                {
                    if (recoveryStore.ReadPending().Count == 0
                        && metadataDetachStore.ReadPending().Count == 0
                        && !receiptStore.ReadAll().Any(value => value.Status == "applying"))
                    {
                        return;
                    }
                    if (await RecoverPendingJournalsAsync().ConfigureAwait(false))
                    {
                        return;
                    }
                }
                catch (BridgeUnavailableException)
                {
                    // The YMM main window/project may not exist yet. Retry during startup.
                }
                await Task.Delay(TimeSpan.FromSeconds(1)).ConfigureAwait(false);
            }
        });
    }

    internal RecoveryStatusDto RecoveryStatus()
    {
        var entries = recoveryStore.ReadAll();
        return new RecoveryStatusDto(
            entries.Count(value => value.State is "applying" or "applied_unverified"),
            entries.Count(value => value.State == "verified"),
            entries.Count(value => value.State == "rolled_back"),
            entries.Count(value => value.State == "recovery_required"),
            entries);
    }

    internal RecoveryJournalEntryDto AcknowledgeRecoveryRequired(Guid operationId)
    {
        if (!recoveryStore.TryGet(operationId, out var entry) || entry is null)
        {
            throw new BridgeNotFoundException(
                $"Recovery journal entry was not found: {operationId}");
        }
        if (entry.State == "failed"
            && (entry.Error?.Contains("operator acknowledged recovery_required", StringComparison.Ordinal)
                ?? false))
        {
            return entry;
        }
        if (entry.State != "recovery_required")
        {
            throw new BridgeConflictException(
                $"Recovery journal is not operator-pending: {entry.State}",
                entry.AfterFingerprint ?? entry.BeforeFingerprint);
        }
        var error = string.IsNullOrWhiteSpace(entry.Error)
            ? "operator acknowledged recovery_required and released the write gate"
            : $"{entry.Error}; operator acknowledged recovery_required and released the write gate";
        var acknowledged = recoveryStore.Transition(
            operationId,
            "failed",
            entry.AfterFingerprint ?? entry.BeforeFingerprint,
            error);
        if (receiptStore.TryGet(operationId, out var receipt)
            && receipt is not null
            && receipt.Status == "recovery_required")
        {
            receiptStore.Put(receipt with
            {
                Status = "failed",
                Verified = false,
                Error = error,
            });
        }
        return acknowledged;
    }

    internal PlanResponseDto Plan(PlanRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        ValidateUtterances(request.Utterances, verifyArtifacts: true);
        var snapshot = Snapshot();
        EnsureFingerprint(request.ExpectedFingerprint, snapshot.Fingerprint);

        var existingByEntity = snapshot.ManagedItems
            .GroupBy(item => item.EntityId, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.ToArray(), StringComparer.Ordinal);
        var desired = request.Utterances.SelectMany(ToManagedItems).ToArray();
        var desiredByEntity = desired
            .GroupBy(item => item.EntityId, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.ToArray(), StringComparer.Ordinal);

        var creates = 0;
        var replacements = 0;
        var unchanged = 0;
        foreach (var utterance in request.Utterances)
        {
            if (!existingByEntity.TryGetValue(utterance.EntityId, out var existing))
            {
                creates++;
            }
            else if (Equivalent(existing, desiredByEntity[utterance.EntityId]))
            {
                unchanged++;
            }
            else
            {
                replacements++;
            }
        }

        var requestedIds = request.Utterances.Select(value => value.EntityId).ToHashSet(StringComparer.Ordinal);
        var after = snapshot.ManagedItems.Where(item => !requestedIds.Contains(item.EntityId))
            .Concat(desired)
            .OrderBy(item => item.Frame)
            .ThenBy(item => item.Layer)
            .ThenBy(item => item.EntityId, StringComparer.Ordinal)
            .ToArray();
        return new PlanResponseDto(
            snapshot.Fingerprint,
            creates + replacements,
            creates,
            replacements,
            unchanged,
            after);
    }

    internal NativeVoicePlanResponseDto PlanNativeVoice(NativeVoicePlanRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        ValidateNativeVoiceCues(request.Cues);
        var snapshot = Snapshot();
        EnsureFingerprint(request.ExpectedFingerprint, snapshot.Fingerprint);
        EnsureNativeVoiceEntitiesAreNew(request.Cues, snapshot);
        Application.Current.Dispatcher.Invoke(() =>
            PrepareNativeVoiceBatch(request.Cues, snapshot.ProjectId));
        return new NativeVoicePlanResponseDto(
            snapshot.Fingerprint,
            request.Cues.Count,
            "bounded");
    }

    internal async Task<ApplyResponseDto> ApplyNativeVoiceAsync(
        NativeVoiceApplyRequestDto request,
        bool sealedTargetPlan = false)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateNativeVoiceCues(request.Cues);
        if (!sealedTargetPlan)
        {
            ValidateNativeVoiceApplyRequest(request);
        }
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            var hasReceipt = receiptStore.TryGet(request.OperationId, out var existingReceipt)
                && existingReceipt is not null;
            if (existingReceipt is not null)
            {
                EnsureReceiptBinding(
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    existingReceipt,
                    existingReceipt.AfterFingerprint);
                if (existingReceipt.Status != "applying")
                {
                    return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
                }
            }
            if (!hasReceipt)
            {
                ValidateMutationRuntime();
            }
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            if (hasReceipt)
            {
                if (!receiptStore.TryGet(request.OperationId, out existingReceipt)
                    || existingReceipt is null)
                {
                    throw new BridgeUnavailableException(
                        "Native-voice receipt disappeared during recovery");
                }
                EnsureReceiptBinding(
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    existingReceipt,
                    existingReceipt.AfterFingerprint);
                if (existingReceipt.Status != "applying")
                {
                    return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
                }
            }
            var before = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, before);
            var replayingPending = false;
            if (existingReceipt is not null)
            {
                EnsureReceiptBinding(
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    existingReceipt,
                    before.Fingerprint);
                if (!string.Equals(existingReceipt.Status, "applying", StringComparison.Ordinal))
                {
                    return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
                }
                if (string.Equals(before.Fingerprint, existingReceipt.BeforeFingerprint, StringComparison.Ordinal))
                {
                    replayingPending = true;
                }
                else
                {
                    var recoveredItems = ReadAppliedItems(request.Cues, before);
                    if (NativeVoiceItemsMatch(request.Cues, recoveredItems))
                    {
                        var verifiedRecovery = CreateReceipt(
                            request,
                            "verified",
                            existingReceipt.BeforeFingerprint,
                            before.Fingerprint,
                            recoveredItems,
                            true,
                            null);
                        receiptStore.Put(verifiedRecovery);
                        return new ApplyResponseDto(true, true, verifiedRecovery);
                    }
                    var recoveryRequired = CreateReceipt(
                        request,
                        "recovery_required",
                        existingReceipt.BeforeFingerprint,
                        before.Fingerprint,
                        recoveredItems,
                        false,
                        "YMM4 changed after the native voice write-ahead receipt but is not an approved result");
                    receiptStore.Put(recoveryRequired);
                    return new ApplyResponseDto(false, true, recoveryRequired);
                }
            }

            EnsureFingerprint(request.ExpectedFingerprint, before.Fingerprint);
            EnsureNativeVoiceEntitiesAreNew(request.Cues, before);
            BridgeFaultInjection.ThrowIf("before_journal_write");
            receiptStore.Put(CreateReceipt(
                request,
                "applying",
                before.Fingerprint,
                before.Fingerprint,
                [],
                false,
                null));

            NativeVoicePreparation? preparation = null;
            var successfulReadbackDurable = false;
            try
            {
                preparation = await Application.Current.Dispatcher.InvokeAsync(
                    () => PrepareNativeVoiceBatch(request.Cues, request.ProjectId));
                var preMutation = Snapshot();
                EnsureTarget(request.ProjectId, request.SceneId, preMutation);
                EnsureFingerprint(request.ExpectedFingerprint, preMutation.Fingerprint);
                recoveryStore.Put(CreateRecoveryEntry(
                    request.OperationId,
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    before.Fingerprint,
                    "native_voice_create",
                    request.Cues.Select(value => value.EntityId).ToArray(),
                    request.Cues.Select(value => value.RealizationId).ToArray(),
                    request.Cues.Select(value => new ManagedItemDto(
                        value.EntityId,
                        value.Revision,
                        "voice",
                        value.Frame,
                        value.Layer,
                        value.MaxLength,
                        value.DisplayText,
                        null,
                        null,
                        value.CharacterName,
                        value.RealizationId.ToString("D"))).ToArray(),
                    new Dictionary<Guid, string>(),
                    []));
                BridgeFaultInjection.ThrowIf("after_journal_before_mutation");
                await ApplyNativeVoiceBatchAsync(preparation, request.ProjectId).ConfigureAwait(false);
                BridgeFaultInjection.ThrowIf("after_mutation_before_readback");
                var after = Snapshot();
                var applied = ReadAppliedItems(request.Cues, after);
                if (!NativeVoiceItemsMatch(request.Cues, applied))
                {
                    return await RollBackNativeVoiceAsync(
                        request,
                        before,
                        preparation,
                        "YMM4 native voice read-back did not match the approved cues",
                        replayingPending);
                }

                var verified = CreateReceipt(
                    request,
                    "verified",
                    before.Fingerprint,
                    after.Fingerprint,
                    applied,
                    true,
                    null);
                recoveryStore.MarkAppliedUnverified(
                    request.OperationId,
                    after.Fingerprint,
                    applied);
                successfulReadbackDurable = true;
                receiptStore.Put(verified);
                BridgeFaultInjection.ThrowIf("after_receipt_before_finalize");
                recoveryStore.Transition(request.OperationId, "verified", after.Fingerprint, null);
                lock (historyGate)
                {
                    lastBatch = new AppliedBatch(preparation.Timeline, [], preparation.AddedItems.ToArray());
                    lastBatchUndone = false;
                }
                return new ApplyResponseDto(true, replayingPending, verified);
            }
            catch (BridgeSimulatedCrashException)
            {
                throw;
            }
            catch (Exception error)
            {
                if (successfulReadbackDurable)
                {
                    // The target already passed semantic read-back and the WAL
                    // sealed that fact. Receipt/finalize persistence failure is
                    // recovered historically; restoring the preimage here
                    // would contradict the durable certificate.
                    throw;
                }
                if (preparation is not null && preparation.AddedItems.Count > 0)
                {
                    return await RollBackNativeVoiceAsync(
                        request,
                        before,
                        preparation,
                        error.GetBaseException().Message,
                        replayingPending);
                }

                var current = TrySnapshot() ?? before;
                var unchanged = string.Equals(
                    current.Fingerprint,
                    before.Fingerprint,
                    StringComparison.Ordinal);
                var failed = CreateReceipt(
                    request,
                    unchanged ? "failed" : "recovery_required",
                    before.Fingerprint,
                    current.Fingerprint,
                    ReadAppliedItems(request.Cues, current),
                    false,
                    error.GetBaseException().Message);
                receiptStore.Put(failed);
                recoveryStore.TryTransition(
                    request.OperationId,
                    failed.Status,
                    current.Fingerprint,
                    failed.Error);
                return new ApplyResponseDto(false, replayingPending, failed);
            }
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal NativeVoiceMutationPlanResponseDto PlanNativeVoiceMutations(
        NativeVoiceMutationPlanRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        ValidateNativeVoiceMutations(request.Mutations);
        var snapshot = Snapshot();
        EnsureFingerprint(request.ExpectedFingerprint, snapshot.Fingerprint);
        Application.Current.Dispatcher.Invoke(() =>
            PrepareNativeVoiceMutationBatch(request.Mutations, snapshot.ProjectId));
        return new NativeVoiceMutationPlanResponseDto(
            snapshot.Fingerprint,
            request.Mutations.Count(value => value.Action == "create"),
            request.Mutations.Count(value => value.Action == "update"),
            request.Mutations.Count(value => value.Action == "delete"),
            "bounded",
            ["audioEffects", "videoEffects", "keyframes", "flags", "voiceParameters", "captionStyle"]);
    }

    internal async Task<ApplyResponseDto> ApplyNativeVoiceMutationsAsync(
        NativeVoiceMutationApplyRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateNativeVoiceMutations(request.Mutations);
        ValidateNativeVoiceMutationApplyRequest(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            var hasReceipt = receiptStore.TryGet(request.OperationId, out var existingReceipt)
                && existingReceipt is not null;
            if (existingReceipt is not null)
            {
                EnsureReceiptBinding(
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    existingReceipt,
                    existingReceipt.AfterFingerprint);
                if (existingReceipt.Status != "applying")
                {
                    return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
                }
            }
            if (!hasReceipt)
            {
                ValidateMutationRuntime();
            }
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            if (hasReceipt)
            {
                if (!receiptStore.TryGet(request.OperationId, out existingReceipt)
                    || existingReceipt is null)
                {
                    throw new BridgeUnavailableException(
                        "Native-voice mutation receipt disappeared during recovery");
                }
                EnsureReceiptBinding(
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    existingReceipt,
                    existingReceipt.AfterFingerprint);
                return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
            }
            var before = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, before);
            EnsureFingerprint(request.ExpectedFingerprint, before.Fingerprint);
            BridgeFaultInjection.ThrowIf("before_journal_write");
            receiptStore.Put(CreateReceipt(
                request,
                "applying",
                before.Fingerprint,
                before.Fingerprint,
                [],
                false,
                null));

            NativeVoiceMutationPreparation? preparation = null;
            var successfulReadbackDurable = false;
            try
            {
                preparation = await Application.Current.Dispatcher.InvokeAsync(
                    () => PrepareNativeVoiceMutationBatch(request.Mutations, request.ProjectId));
                var preMutation = Snapshot();
                EnsureTarget(request.ProjectId, request.SceneId, preMutation);
                EnsureFingerprint(request.ExpectedFingerprint, preMutation.Fingerprint);
                recoveryStore.Put(CreateRecoveryEntry(
                    request.OperationId,
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    before.Fingerprint,
                    "native_voice_mutation",
                    request.Mutations.Select(value => value.EntityId).ToArray(),
                    request.Mutations.Select(value => value.RealizationId).ToArray(),
                    request.Mutations.Where(value => value.Action != "delete")
                        .Select(value => new ManagedItemDto(
                            value.EntityId,
                            value.Revision,
                            "voice",
                            value.Frame,
                            value.Layer,
                            value.MaxLength,
                            value.DisplayText,
                            null,
                            null,
                            value.CharacterName,
                            value.RealizationId.ToString("D")))
                        .ToArray(),
                    preparation.PreservedStateDigests,
                    preparation.Originals.Values.Select(value => value.Item)));
                BridgeFaultInjection.ThrowIf("after_journal_before_mutation");
                await ApplyNativeVoiceMutationBatchAsync(preparation, request.ProjectId)
                    .ConfigureAwait(false);
                BridgeFaultInjection.ThrowIf("after_mutation_before_readback");
                var after = Snapshot();
                var affected = ReadMutationItems(request.Mutations, after);
                if (!NativeVoiceMutationsMatch(request.Mutations, affected))
                {
                    return await RollBackNativeVoiceMutationsAsync(
                        request,
                        before,
                        preparation,
                        "YMM4 native voice mutation read-back did not match the approved result");
                }
                var receipt = CreateReceipt(
                    request,
                    "verified",
                    before.Fingerprint,
                    after.Fingerprint,
                    affected,
                    true,
                    null);
                recoveryStore.MarkAppliedUnverified(
                    request.OperationId,
                    after.Fingerprint,
                    affected);
                successfulReadbackDurable = true;
                receiptStore.Put(receipt);
                BridgeFaultInjection.ThrowIf("after_receipt_before_finalize");
                recoveryStore.Transition(request.OperationId, "verified", after.Fingerprint, null);
                lock (historyGate)
                {
                    lastBatch = new AppliedBatch(
                        preparation.Timeline,
                        preparation.DeletedItems.ToArray(),
                        preparation.AddedItems.ToArray());
                    lastBatchUndone = false;
                }
                return new ApplyResponseDto(true, false, receipt);
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
                if (preparation is not null)
                {
                    return await RollBackNativeVoiceMutationsAsync(
                        request,
                        before,
                        preparation,
                        error.GetBaseException().Message);
                }
                var failed = CreateReceipt(
                    request,
                    "failed",
                    before.Fingerprint,
                    before.Fingerprint,
                    [],
                    false,
                    error.GetBaseException().Message);
                receiptStore.Put(failed);
                recoveryStore.TryTransition(
                    request.OperationId,
                    failed.Status,
                    before.Fingerprint,
                    failed.Error);
                return new ApplyResponseDto(false, false, failed);
            }
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal async Task<ApplyResponseDto> SealNativeVoiceMutationNotStartedAsync(
        NativeVoiceMutationApplyRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateNativeVoiceMutations(request.Mutations);
        ValidateNativeVoiceMutationApplyRequest(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (receiptStore.TryGet(request.OperationId, out var existingReceipt)
                && existingReceipt is not null)
            {
                EnsureReceiptBinding(
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    existingReceipt,
                    existingReceipt.AfterFingerprint);
                return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
            }
            var tombstone = CreateNotStartedReceipt(
                request.OperationId,
                request.RequestDigest,
                request.ProjectId,
                request.SceneId,
                request.ExpectedFingerprint,
                "Native-voice mutation did not start; a durable no-mutation tombstone was sealed");
            receiptStore.Put(tombstone);
            return new ApplyResponseDto(false, false, tombstone);
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal async Task<NativeVoiceArtifactDto> ExportNativeVoiceArtifactsAsync(
        NativeVoiceArtifactRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        if (request.RealizationId == Guid.Empty)
        {
            throw new BridgeValidationException("A native voice realization ID is required");
        }
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            var snapshot = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, snapshot);
            EnsureFingerprint(request.ExpectedFingerprint, snapshot.Fingerprint);
            var stagingRoot = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "TakeGraph",
                "native-voice-artifacts",
                ".staging");
            Directory.CreateDirectory(stagingRoot);
            var temporaryAudioPath = Path.Combine(stagingRoot, $"{Guid.NewGuid():N}.wav");
            var export = await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                var raw = FindNativeVoiceRawItem(request.RealizationId, request.ProjectId);
                var main = RequireMainViewModel();
                var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
                    ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
                var timeline = GetField(timelineViewModel, "timeline")
                    ?? GetMember(timelineViewModel, "Timeline")
                    ?? throw new BridgeUnavailableException("YMM4 timeline is unavailable");
                var videoInfo = GetMember(timeline, "VideoInfo")
                    ?? throw new BridgeUnavailableException("YMM4 timeline video settings are unavailable");
                var createAudio = raw.Item.GetType()
                    .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                    .SingleOrDefault(method =>
                        method.Name == "CreateAudioFileAsync"
                        && method.GetParameters() is
                        [
                            { ParameterType: var pathType },
                            { ParameterType: var videoInfoType },
                        ]
                        && pathType == typeof(string)
                        && videoInfoType.IsInstanceOfType(videoInfo))
                    ?? throw new BridgeUnavailableException(
                        "YMM4 VoiceItem.CreateAudioFileAsync is unavailable");
                Task audioTask;
                try
                {
                    audioTask = createAudio.Invoke(raw.Item, [temporaryAudioPath, videoInfo]) as Task
                        ?? throw new BridgeUnavailableException(
                            "YMM4 VoiceItem.CreateAudioFileAsync did not return a Task");
                }
                catch (TargetInvocationException error)
                {
                    throw error.InnerException ?? error;
                }
                var query = CreateVoiceProvenanceJson(raw.Item, request.RealizationId);
                return (AudioTask: audioTask, Query: Encoding.UTF8.GetBytes(query));
            });
            byte[] audio;
            try
            {
                await export.AudioTask.ConfigureAwait(false);
                audio = await File.ReadAllBytesAsync(temporaryAudioPath).ConfigureAwait(false);
            }
            finally
            {
                if (File.Exists(temporaryAudioPath))
                {
                    File.Delete(temporaryAudioPath);
                }
            }
            if (audio.Length == 0)
            {
                throw new BridgeUnavailableException("YMM4 produced an empty native voice WAV");
            }
            var root = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "TakeGraph",
                "native-voice-artifacts");
            var audioHash = Convert.ToHexStringLower(SHA256.HashData(audio));
            var queryHash = Convert.ToHexStringLower(SHA256.HashData(export.Query));
            var audioPath = Path.Combine(root, "audio", $"{audioHash}.wav");
            var queryPath = Path.Combine(root, "queries", $"{queryHash}.json");
            WriteImmutableArtifact(audioPath, audio, audioHash);
            WriteImmutableArtifact(queryPath, export.Query, queryHash);
            return new NativeVoiceArtifactDto(
                request.RealizationId,
                audioPath,
                audioHash,
                audio.LongLength,
                queryPath,
                queryHash,
                export.Query.LongLength,
                "ymm4-native-create-audio+normalized-host-bound-voice-state/4.55.1.1");
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal async Task<ApplyResponseDto> ApplyAsync(
        ApplyRequestDto request,
        bool sealedTargetPlan = false)
    {
        ValidateProtocol(request.ProtocolVersion);
        // Artifact bytes are materialized and leased only after replay lookup.
        // This preserves exact idempotent replay even when the caller's original
        // staging path has since been removed.
        ValidateUtterances(request.Utterances, verifyArtifacts: false);
        if (!sealedTargetPlan)
        {
            ValidateApplyRequest(request);
        }
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            var hasReceipt = receiptStore.TryGet(request.OperationId, out var existingReceipt)
                && existingReceipt is not null;
            if (existingReceipt is not null)
            {
                EnsureReceiptBinding(request, existingReceipt, existingReceipt.AfterFingerprint);
                if (existingReceipt.Status != "applying")
                {
                    return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
                }
            }
            if (!hasReceipt)
            {
                ValidateMutationRuntime();
            }
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            if (hasReceipt)
            {
                if (!receiptStore.TryGet(request.OperationId, out existingReceipt)
                    || existingReceipt is null)
                {
                    throw new BridgeUnavailableException(
                        "Portable operation receipt disappeared during recovery");
                }
                EnsureReceiptBinding(request, existingReceipt, existingReceipt.AfterFingerprint);
                if (existingReceipt.Status != "applying")
                {
                    return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
                }
            }
            var before = Snapshot();
            EnsureTarget(request, before);
            var replayingPending = false;
            OperationReceiptDto? pendingReceipt = null;
            if (existingReceipt is not null)
            {
                EnsureReceiptBinding(request, existingReceipt, before.Fingerprint);
                if (!string.Equals(existingReceipt.Status, "applying", StringComparison.Ordinal))
                {
                    return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
                }

                pendingReceipt = existingReceipt;
                replayingPending = true;
            }

            using var portableArtifacts = MaterializePortableArtifacts(request.Utterances);
            var effectiveRequest = request with { Utterances = portableArtifacts.Utterances };
            if (pendingReceipt is not null)
            {
                var recovered = RecoverPending(request, effectiveRequest.Utterances, pendingReceipt, before);
                if (recovered is not null)
                {
                    return recovered;
                }
            }

            EnsureFingerprint(request.ExpectedFingerprint, before.Fingerprint);
            BridgeFaultInjection.ThrowIf("before_journal_write");
            receiptStore.Put(CreateReceipt(
                request,
                "applying",
                before.Fingerprint,
                before.Fingerprint,
                [],
                false,
                null));

            AppliedBatch? batch = null;
            var mutationStarted = false;
            var successfulReadbackDurable = false;
            try
            {
                batch = await Application.Current.Dispatcher.InvokeAsync(
                    () => PrepareApplyBatch(effectiveRequest.Utterances));
                var preMutation = Snapshot();
                EnsureTarget(request, preMutation);
                EnsureFingerprint(request.ExpectedFingerprint, preMutation.Fingerprint);
                recoveryStore.Put(CreateRecoveryEntry(
                    request.OperationId,
                    request.RequestDigest,
                    request.ProjectId,
                    request.SceneId,
                    request.ExpectedFingerprint,
                    before.Fingerprint,
                    "portable_pair",
                    effectiveRequest.Utterances.Select(value => value.EntityId).ToArray(),
                    [],
                    effectiveRequest.Utterances.SelectMany(ToManagedItems).ToArray(),
                    new Dictionary<Guid, string>(),
                    batch.OldItems));
                BridgeFaultInjection.ThrowIf("after_journal_before_mutation");
                mutationStarted = true;
                await Application.Current.Dispatcher.InvokeAsync(() => ApplyPreparedBatch(batch));
                BridgeFaultInjection.ThrowIf("after_mutation_before_readback");
                var after = Snapshot();
                portableArtifacts.Verify();
                var desired = effectiveRequest.Utterances.SelectMany(ToManagedItems).ToArray();
                var applied = ReadAppliedItems(request, after);
                if (!Equivalent(applied, desired))
                {
                    return await RollBackAsync(
                        request,
                        before,
                        batch,
                        "YMM4 read-back did not match the approved managed items",
                        replayingPending);
                }

                var verified = CreateReceipt(
                    request,
                    "verified",
                    before.Fingerprint,
                    after.Fingerprint,
                    applied,
                    true,
                    null);
                recoveryStore.MarkAppliedUnverified(
                    request.OperationId,
                    after.Fingerprint,
                    applied);
                successfulReadbackDurable = true;
                receiptStore.Put(verified);
                BridgeFaultInjection.ThrowIf("after_receipt_before_finalize");
                recoveryStore.Transition(request.OperationId, "verified", after.Fingerprint, null);
                lock (historyGate)
                {
                    lastBatch = batch;
                    lastBatchUndone = false;
                }
                return new ApplyResponseDto(true, replayingPending, verified);
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
                if (mutationStarted && batch is not null)
                {
                    return await RollBackAsync(
                        request,
                        before,
                        batch,
                        error.GetBaseException().Message,
                        replayingPending);
                }

                var current = TrySnapshot() ?? before;
                var unchanged = string.Equals(
                    current.Fingerprint,
                    before.Fingerprint,
                    StringComparison.Ordinal);
                var failed = CreateReceipt(
                    request,
                    unchanged ? "failed" : "recovery_required",
                    before.Fingerprint,
                    current.Fingerprint,
                    [],
                    false,
                    unchanged
                        ? error.GetBaseException().Message
                        : $"{error.GetBaseException().Message}; target changed before mutation could be proven absent");
                receiptStore.Put(failed);
                recoveryStore.TryTransition(
                    request.OperationId,
                    failed.Status,
                    current.Fingerprint,
                    failed.Error);
                return new ApplyResponseDto(false, replayingPending, failed);
            }
        }
        finally
        {
            applyGate.Release();
        }
    }

    private ApplyResponseDto? RecoverPending(
        ApplyRequestDto request,
        IReadOnlyList<ManagedUtteranceDto> effectiveUtterances,
        OperationReceiptDto pending,
        ProjectSnapshotDto current)
    {
        if (string.Equals(current.Fingerprint, pending.BeforeFingerprint, StringComparison.Ordinal))
        {
            return null;
        }

        var applied = ReadAppliedItems(request, current);
        var desired = effectiveUtterances.SelectMany(ToManagedItems).ToArray();
        if (Equivalent(applied, desired))
        {
            var verified = CreateReceipt(
                request,
                "verified",
                pending.BeforeFingerprint,
                current.Fingerprint,
                applied,
                true,
                null);
            receiptStore.Put(verified);
            return new ApplyResponseDto(true, true, verified);
        }

        var recoveryRequired = CreateReceipt(
            request,
            "recovery_required",
            pending.BeforeFingerprint,
            current.Fingerprint,
            applied,
            false,
            "YMM4 changed after the write-ahead receipt but does not match the approved result");
        receiptStore.Put(recoveryRequired);
        return new ApplyResponseDto(false, true, recoveryRequired);
    }

    private async Task<bool> RecoverPendingJournalsAsync()
    {
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            EnsureNoPendingProjectInitialization();
            var standard = await RecoverPendingJournalsCoreAsync().ConfigureAwait(false);
            var detach = await RecoverPendingMetadataDetachJournalsCoreAsync().ConfigureAwait(false);
            return standard && detach;
        }
        finally
        {
            applyGate.Release();
        }
    }

    private async Task EnsureRecoveryClearAsync(Guid? resumedProjectInitialization = null)
    {
        EnsureNoPendingProjectInitialization(resumedProjectInitialization);
        if (resumedProjectInitialization is not null)
        {
            // The exact initialization POST is the sole permitted writer while
            // its WAL is pending. Do not auto-recover an unrelated operation
            // under this exception; require the operator to resolve the other
            // recovery state first.
            var pendingJournals = recoveryStore.ReadPending();
            var pendingMetadataDetach = metadataDetachStore.ReadPending();
            var pendingReceipts = receiptStore.ReadAll()
                .Where(value => value.Status is "applying" or "recovery_required")
                .ToArray();
            EnsureRecoveryAuthorizationClear(pendingJournals, pendingReceipts);
            if (pendingMetadataDetach.Count > 0)
            {
                throw new BridgeUnavailableException(
                    "YMM4 has unresolved metadata detach recovery state; refusing project initialization resume");
            }
            return;
        }
        _ = await RecoverPendingJournalsCoreAsync().ConfigureAwait(false);
        _ = await RecoverPendingMetadataDetachJournalsCoreAsync().ConfigureAwait(false);
        var unresolvedJournals = recoveryStore.ReadPending();
        var unresolvedMetadataDetach = metadataDetachStore.ReadPending();
        var unresolvedReceipts = receiptStore.ReadAll()
            .Where(value => value.Status is "applying" or "recovery_required")
            .ToArray();
        EnsureRecoveryAuthorizationClear(unresolvedJournals, unresolvedReceipts);
        if (unresolvedMetadataDetach.Count > 0)
        {
            var targets = unresolvedMetadataDetach
                .Select(value => $"{value.Request.ProjectId}/{value.Request.SceneId}")
                .Distinct(StringComparer.Ordinal)
                .Order(StringComparer.Ordinal)
                .Take(4);
            throw new BridgeUnavailableException(
                $"YMM4 has unresolved metadata detach recovery state; refusing every write: {string.Join(", ", targets)}");
        }
    }

    private void EnsureNoPendingProjectInitialization(Guid? allowedOperationId = null)
    {
        var pending = projectOperationStore.ReadPendingProjectInitializations()
            .Where(value => value.OperationId != allowedOperationId)
            .ToArray();
        if (pending.Length == 0)
        {
            return;
        }
        var operations = pending
            .Select(value => value.OperationId.ToString("D"))
            .Order(StringComparer.Ordinal)
            .Take(4);
        throw new BridgeUnavailableException(
            "YMM4 has unresolved project initialization state; refusing every other write: "
            + string.Join(", ", operations));
    }

    internal static void EnsureRecoveryAuthorizationClear(
        IReadOnlyList<RecoveryJournalEntryDto> unresolvedJournals,
        IReadOnlyList<OperationReceiptDto> unresolvedReceipts)
    {
        if (unresolvedJournals.Count == 0 && unresolvedReceipts.Count == 0)
        {
            return;
        }

        var targets = unresolvedJournals
            .Select(value => $"{value.ProjectId}/{value.SceneId}")
            .Concat(unresolvedReceipts.Select(value => $"{value.ProjectId}/{value.SceneId}"))
            .Distinct(StringComparer.Ordinal)
            .Order(StringComparer.Ordinal)
            .Take(4);
        throw new BridgeUnavailableException(
            $"YMM4 has unresolved TakeGraph recovery state; refusing every write, including writes to other targets: {string.Join(", ", targets)}");
    }

    private async Task<bool> RecoverPendingJournalsCoreAsync()
    {
        var pending = recoveryStore.ReadRecoverable();
        var allResolved = true;

        // Compatibility with the earlier native-extension finalize order:
        // the rich verified receipt itself was written only after successful
        // semantic read-back, even if the generic WAL still says applying.
        foreach (var entry in pending.Where(value =>
                     value.State == "applying" && value.Driver == NativeExtensionRecoveryDriver))
        {
            if (projectOperationStore.TryGetNativeExtension(entry.OperationId, out var receipt)
                && receipt is not null
                && NativeExtensionLegacyApplyingReceiptBindingMatches(entry, receipt))
            {
                recoveryStore.Transition(
                    entry.OperationId,
                    "verified",
                    receipt.AfterFingerprint,
                    null);
            }
        }
        pending = recoveryStore.ReadRecoverable();

        // applied_unverified is a durable successful-readback certificate.
        // It must be completed from its sealed evidence before any current
        // project snapshot is consulted; later user edits are reconciliation
        // drift and must never cause startup to roll the successful write back.
        foreach (var entry in pending.Where(value => value.State == "applied_unverified"))
        {
            if (!CompleteAppliedUnverified(entry))
            {
                allResolved = false;
            }
        }
        pending = recoveryStore.ReadRecoverable();
        var pendingIds = pending.Select(value => value.OperationId).ToHashSet();
        var orphanReceipts = receiptStore.ReadAll()
            .Where(value => value.Status == "applying" && !pendingIds.Contains(value.OperationId))
            .ToArray();
        if (pending.Count == 0 && orphanReceipts.Length == 0)
        {
            return allResolved;
        }
        var current = Snapshot();
        foreach (var receipt in orphanReceipts)
        {
            if (!string.Equals(receipt.ProjectId, current.ProjectId, StringComparison.Ordinal)
                || !string.Equals(receipt.SceneId, current.SceneId, StringComparison.Ordinal))
            {
                allResolved = false;
                continue;
            }
            var restored = string.Equals(
                current.Fingerprint,
                receipt.BeforeFingerprint,
                StringComparison.Ordinal);
            receiptStore.Put(receipt with
            {
                Status = restored ? "rolled_back" : "recovery_required",
                AfterFingerprint = current.Fingerprint,
                Verified = false,
                Error = restored
                    ? "Recovered a write-ahead receipt that had no mutation preimage; no mutation occurred"
                    : "Write-ahead receipt has no durable preimage and the exact before-state cannot be proven",
            });
        }
        foreach (var entry in pending)
        {
            if (!string.Equals(entry.ProjectId, current.ProjectId, StringComparison.Ordinal)
                || !string.Equals(entry.SceneId, current.SceneId, StringComparison.Ordinal))
            {
                allResolved = false;
                continue;
            }

            var actual = ReadRecoveryItems(entry, current);
            if (string.Equals(current.Fingerprint, entry.BeforeFingerprint, StringComparison.Ordinal))
            {
                CompleteRecoveredOperation(entry, "rolled_back", current, actual, null);
                continue;
            }
            if (RecoveryExpectedStateMatches(entry, current))
            {
                CompleteRecoveredOperation(entry, "verified", current, actual, null);
                continue;
            }

            try
            {
                var restoredItems = entry.BeforeItems.Select(DeserializeYmmObject).ToArray();
                await Application.Current.Dispatcher.InvokeAsync(() =>
                {
                    var main = RequireMainViewModel();
                    var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
                        ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
                    var timeline = GetField(timelineViewModel, "timeline")
                        ?? GetMember(timelineViewModel, "Timeline")
                        ?? throw new BridgeUnavailableException("YMM4 timeline mutation API is unavailable");
                    var touched = FindRecoveryRawItems(entry, ReadItems(timelineViewModel));
                    ReplaceBatchItems(timeline, touched, restoredItems);
                });
                current = Snapshot();
                actual = ReadRecoveryItems(entry, current);
                if (string.Equals(current.Fingerprint, entry.BeforeFingerprint, StringComparison.Ordinal))
                {
                    CompleteRecoveredOperation(entry, "rolled_back", current, actual, null);
                }
                else
                {
                    allResolved = false;
                    CompleteRecoveredOperation(
                        entry,
                        "recovery_required",
                        current,
                        actual,
                        "Durable preimage was restored, but the exact before-state could not be proven");
                }
            }
            catch (Exception error)
            {
                allResolved = false;
                current = TrySnapshot() ?? current;
                actual = ReadRecoveryItems(entry, current);
                CompleteRecoveredOperation(
                    entry,
                    "recovery_required",
                    current,
                    actual,
                    $"Durable recovery failed: {error.GetBaseException().Message}");
            }
        }
        return allResolved;
    }

    private bool CompleteAppliedUnverified(RecoveryJournalEntryDto entry)
    {
        var afterFingerprint = entry.AfterFingerprint
            ?? throw new BridgeUnavailableException(
                $"Applied recovery evidence has no after fingerprint: {entry.OperationId}");
        if (entry.Driver == NativeExtensionRecoveryDriver)
        {
            var receipt = entry.NativeExtensionReceipt;
            if (receipt is null
                && projectOperationStore.TryGetNativeExtension(entry.OperationId, out var stored)
                && stored is not null
                && stored.Status == "verified"
                && stored.Verified)
            {
                receipt = stored;
            }
            if (receipt is not null
                && NativeExtensionRecoveryBindingMatches(entry, receipt))
            {
                projectOperationStore.PutNativeExtension(receipt);
                recoveryStore.Transition(entry.OperationId, "verified", afterFingerprint, null);
                return true;
            }

            if (projectOperationStore.TryGetNativeExtension(entry.OperationId, out var applying)
                && applying is not null)
            {
                projectOperationStore.PutNativeExtension(applying with
                {
                    Status = "recovery_required",
                    AfterFingerprint = afterFingerprint,
                    Realizations = [],
                    Verified = false,
                    Error = "Successful native-extension read-back evidence is missing or inconsistent",
                });
            }
            recoveryStore.Transition(
                entry.OperationId,
                "recovery_required",
                afterFingerprint,
                "Successful native-extension read-back evidence is missing or inconsistent");
            return false;
        }

        var verifiedItems = entry.VerifiedItems;
        if (verifiedItems is null)
        {
            var error = "Successful read-back evidence is missing from the durable recovery journal";
            receiptStore.Put(new OperationReceiptDto(
                entry.OperationId,
                entry.RequestDigest,
                entry.ProjectId,
                entry.SceneId,
                entry.ExpectedFingerprint,
                "recovery_required",
                entry.BeforeFingerprint,
                afterFingerprint,
                [],
                false,
                error));
            recoveryStore.Transition(
                entry.OperationId,
                "recovery_required",
                afterFingerprint,
                error);
            return false;
        }

        var verified = new OperationReceiptDto(
            entry.OperationId,
            entry.RequestDigest,
            entry.ProjectId,
            entry.SceneId,
            entry.ExpectedFingerprint,
            "verified",
            entry.BeforeFingerprint,
            afterFingerprint,
            verifiedItems,
            true,
            null);
        receiptStore.Put(verified);
        recoveryStore.Transition(entry.OperationId, "verified", afterFingerprint, null);
        return true;
    }

    private static bool NativeExtensionRecoveryBindingMatches(
        RecoveryJournalEntryDto entry,
        NativeExtensionApplyResponseDto receipt) =>
        receipt.OperationId == entry.OperationId
        && receipt.RequestDigest == entry.RequestDigest
        && receipt.ProjectId == entry.ProjectId
        && receipt.SceneId == entry.SceneId
        && receipt.BeforeFingerprint == entry.BeforeFingerprint
        && receipt.AfterFingerprint == entry.AfterFingerprint
        && receipt.Status == "verified"
        && receipt.Verified
        && receipt.Error is null;

    private static bool NativeExtensionLegacyApplyingReceiptBindingMatches(
        RecoveryJournalEntryDto entry,
        NativeExtensionApplyResponseDto receipt) =>
        entry.State == "applying"
        && entry.AfterFingerprint is null
        && receipt.OperationId == entry.OperationId
        && receipt.RequestDigest == entry.RequestDigest
        && receipt.ProjectId == entry.ProjectId
        && receipt.SceneId == entry.SceneId
        && receipt.BeforeFingerprint == entry.BeforeFingerprint
        && receipt.Status == "verified"
        && receipt.Verified
        && receipt.Error is null;

    private void CompleteRecoveredOperation(
        RecoveryJournalEntryDto entry,
        string status,
        ProjectSnapshotDto snapshot,
        IReadOnlyList<ManagedItemDto> appliedItems,
        string? error)
    {
        if (entry.Driver == NativeExtensionRecoveryDriver)
        {
            if (projectOperationStore.TryGetNativeExtension(entry.OperationId, out var existing)
                && existing is not null)
            {
                var nativeError = error ?? (status == "rolled_back"
                    ? "Recovered native-extension operation to its exact before state"
                    : "Native-extension recovery requires manual intervention");
                projectOperationStore.PutNativeExtension(existing with
                {
                    Status = status,
                    AfterFingerprint = snapshot.Fingerprint,
                    Realizations = [],
                    Verified = false,
                    Error = nativeError,
                });
                recoveryStore.Transition(
                    entry.OperationId,
                    status,
                    snapshot.Fingerprint,
                    nativeError);
            }
            else
            {
                recoveryStore.Transition(
                    entry.OperationId,
                    "recovery_required",
                    snapshot.Fingerprint,
                    "Native-extension request binding is missing during recovery");
            }
            return;
        }
        var verified = status == "verified";
        receiptStore.Put(new OperationReceiptDto(
            entry.OperationId,
            entry.RequestDigest,
            entry.ProjectId,
            entry.SceneId,
            entry.ExpectedFingerprint,
            status,
            entry.BeforeFingerprint,
            snapshot.Fingerprint,
            appliedItems,
            verified,
            error));
        recoveryStore.Transition(entry.OperationId, status, snapshot.Fingerprint, error);
    }

    private static ManagedItemDto[] ReadRecoveryItems(
        RecoveryJournalEntryDto entry,
        ProjectSnapshotDto snapshot)
    {
        if (entry.Driver == TimelineEditRecoveryDriver)
        {
            var realizationIds = entry.RealizationIds.Select(value => value.ToString("D"))
                .ToHashSet(StringComparer.OrdinalIgnoreCase);
            var portableEntityIds = entry.ExpectedItems
                .Where(value => value.RealizationId is null)
                .Select(value => value.EntityId)
                .ToHashSet(StringComparer.Ordinal);
            return snapshot.ManagedItems.Where(item =>
                    portableEntityIds.Contains(item.EntityId)
                    || item.RealizationId is not null && realizationIds.Contains(item.RealizationId))
                .ToArray();
        }
        if (entry.RealizationIds.Count > 0)
        {
            var realizationIds = entry.RealizationIds.Select(value => value.ToString("D"))
                .ToHashSet(StringComparer.OrdinalIgnoreCase);
            return snapshot.ManagedItems.Where(item =>
                    item.RealizationId is not null && realizationIds.Contains(item.RealizationId))
                .ToArray();
        }
        var entityIds = entry.EntityIds.ToHashSet(StringComparer.Ordinal);
        return snapshot.ManagedItems.Where(item => entityIds.Contains(item.EntityId)).ToArray();
    }

    private static bool RecoveryExpectedStateMatches(
        RecoveryJournalEntryDto entry,
        ProjectSnapshotDto snapshot)
    {
        if (entry.Driver == NativeExtensionRecoveryDriver)
        {
            // Phase-4 receipts have their own richer realization contract. On
            // startup, prefer the durable YMM preimage over guessing from the
            // managed-only snapshot projection.
            return false;
        }
        var actual = ReadRecoveryItems(entry, snapshot);
        if (entry.Driver == "portable_pair")
        {
            return Equivalent(actual, entry.ExpectedItems);
        }
        if (entry.Driver == TimelineEditRecoveryDriver)
        {
            var portableExpected = entry.ExpectedItems
                .Where(value => value.RealizationId is null)
                .ToArray();
            var portableIds = portableExpected.Select(value => value.EntityId)
                .ToHashSet(StringComparer.Ordinal);
            var portableActual = actual.Where(value => portableIds.Contains(value.EntityId))
                .ToArray();
            if (!Equivalent(portableActual, portableExpected))
            {
                return false;
            }
            var nativeExpected = entry.ExpectedItems
                .Where(value => value.RealizationId is not null)
                .ToArray();
            var nativeActual = actual.Where(value => value.RealizationId is not null)
                .ToArray();
            if (nativeActual.Length != nativeExpected.Length)
            {
                return false;
            }
            return nativeExpected.All(expected => nativeActual.Any(item =>
                item.Kind == "voice"
                && item.EntityId == expected.EntityId
                && item.Revision == expected.Revision
                && string.Equals(
                    item.RealizationId,
                    expected.RealizationId,
                    StringComparison.OrdinalIgnoreCase)
                && item.Frame == expected.Frame
                && item.Layer == expected.Layer
                && item.Length is > 0
                && item.Length <= expected.Length
                && item.Text == expected.Text
                && item.Speaker == expected.Speaker));
        }
        if (actual.Length != entry.ExpectedItems.Count)
        {
            return false;
        }
        foreach (var expected in entry.ExpectedItems)
        {
            var matching = actual.Where(item =>
                    string.Equals(item.RealizationId, expected.RealizationId, StringComparison.OrdinalIgnoreCase))
                .ToArray();
            if (matching.Length != 1)
            {
                return false;
            }
            var item = matching[0];
            if (item.Kind != expected.Kind
                || item.EntityId != expected.EntityId
                || item.Revision != expected.Revision
                || item.Speaker != expected.Speaker
                || item.Text != expected.Text
                || item.Frame != expected.Frame
                || item.Layer != expected.Layer
                || item.Length is <= 0
                || item.Length > expected.Length)
            {
                return false;
            }
        }
        foreach (var pair in entry.PreservedStateDigests)
        {
            var actualDigest = Application.Current.Dispatcher.Invoke(() =>
            {
                var raw = FindNativeVoiceRawItem(pair.Key, entry.ProjectId);
                return ComputePreservedNativeVoiceStateDigest(raw.Item);
            });
            if (!string.Equals(
                    actualDigest,
                    pair.Value,
                    StringComparison.Ordinal))
            {
                return false;
            }
        }
        return true;
    }

    private static object[] FindRecoveryRawItems(
        RecoveryJournalEntryDto entry,
        IReadOnlyList<RawItem> rawItems)
    {
        var result = new HashSet<object>(ReferenceEqualityComparer.Instance);
        var realizationIds = entry.RealizationIds.ToHashSet();
        var entityIds = entry.EntityIds.ToHashSet(StringComparer.Ordinal);
        foreach (var raw in rawItems)
        {
            if (NativeExtensionRemarkCodec.TryDecode(raw.Remark, out var extensionMarker)
                && extensionMarker is not null
                && (realizationIds.Contains(extensionMarker.RealizationId)
                    || entityIds.Contains(extensionMarker.EntityId)
                    || extensionMarker.Effects.Values.Any(value =>
                        realizationIds.Contains(value.RealizationId))))
            {
                EnsureRecoveryMarkerOwnership(entry, extensionMarker.ProjectId);
                result.Add(raw.Item);
            }
            if (raw.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                && RemarkCodec.TryDecode(raw.Remark, out var voiceMarker)
                && voiceMarker is not null
                && (realizationIds.Contains(voiceMarker.RealizationId)
                    || entityIds.Contains(voiceMarker.EntityId)))
            {
                EnsureRecoveryMarkerOwnership(entry, voiceMarker.ProjectId);
                result.Add(raw.Item);
            }
            if (!MarkerCodec.TryDecode(raw.Text, out _, out var marker)
                || marker is null
                || !entityIds.Contains(marker.EntityId))
            {
                continue;
            }
            result.Add(raw.Item);
            foreach (var audio in rawItems.Where(item =>
                         item.Layer == marker.AudioLayer
                         && PathsEqual(item.AudioPath, marker.AudioPath)
                         && !string.IsNullOrWhiteSpace(item.AudioPath)))
            {
                result.Add(audio.Item);
            }
        }
        foreach (var expectedAudio in entry.ExpectedItems.Where(item => item.Kind == "audio"))
        {
            if (string.IsNullOrWhiteSpace(expectedAudio.AudioPath))
            {
                continue;
            }
            foreach (var audio in rawItems.Where(item =>
                         item.Layer == expectedAudio.Layer
                         && item.Frame == expectedAudio.Frame
                         && PathsEqual(item.AudioPath, expectedAudio.AudioPath)
                         && !string.IsNullOrWhiteSpace(item.AudioPath)))
            {
                result.Add(audio.Item);
            }
        }
        return result.ToArray();
    }

    private static void EnsureRecoveryMarkerOwnership(
        RecoveryJournalEntryDto entry,
        string markerProjectId)
    {
        if (!string.Equals(entry.ProjectId, markerProjectId, StringComparison.Ordinal))
        {
            throw new BridgeUnavailableException(
                $"Recovery identity is also marked for foreign project {markerProjectId}; refusing ambiguous restoration");
        }
    }

    private static RecoveryJournalEntryDto CreateRecoveryEntry(
        Guid operationId,
        string requestDigest,
        string projectId,
        string sceneId,
        string expectedFingerprint,
        string beforeFingerprint,
        string driver,
        IReadOnlyList<string> entityIds,
        IReadOnlyList<Guid> realizationIds,
        IReadOnlyList<ManagedItemDto> expectedItems,
        IReadOnlyDictionary<Guid, string> preservedStateDigests,
        IEnumerable<object> beforeItems)
    {
        var serializedItems = Application.Current.Dispatcher.Invoke(() => beforeItems
            .Distinct(ReferenceEqualityComparer.Instance)
            .Select(SerializeRecoveryItem)
            .ToArray());
        var now = DateTimeOffset.UtcNow;
        return new RecoveryJournalEntryDto(
            1,
            operationId,
            requestDigest,
            projectId,
            sceneId,
            expectedFingerprint,
            beforeFingerprint,
            "applying",
            driver,
            entityIds.Distinct(StringComparer.Ordinal).Order(StringComparer.Ordinal).ToArray(),
            realizationIds.Distinct().Order().ToArray(),
            expectedItems,
            preservedStateDigests,
            serializedItems,
            now,
            now,
            null,
            null);
    }

    private static RecoveryItemDto SerializeRecoveryItem(object item)
    {
        var json = SerializeYmmObject(item);
        return new RecoveryItemDto(
            item.GetType().AssemblyQualifiedName
                ?? throw new BridgeUnavailableException("YMM4 recovery item has no type identity"),
            json,
            Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(json))));
    }

    private async Task<ApplyResponseDto> RollBackAsync(
        ApplyRequestDto request,
        ProjectSnapshotDto before,
        AppliedBatch batch,
        string failure,
        bool replayed)
    {
        string? rollbackError = null;
        try
        {
            BridgeFaultInjection.ThrowIf("during_rollback");
            await Application.Current.Dispatcher.InvokeAsync(
                () => ReplaceBatchItems(batch.Timeline, batch.NewItems, batch.OldItems));
        }
        catch (Exception error)
        {
            rollbackError = error.GetBaseException().Message;
        }

        var current = TrySnapshot() ?? before;
        var restored = rollbackError is null
            && string.Equals(current.Fingerprint, before.Fingerprint, StringComparison.Ordinal);
        var receipt = CreateReceipt(
            request,
            restored ? "rolled_back" : "recovery_required",
            before.Fingerprint,
            current.Fingerprint,
            ReadAppliedItems(request, current),
            false,
            rollbackError is null ? failure : $"{failure}; rollback failed: {rollbackError}");
        receiptStore.Put(receipt);
        recoveryStore.TryTransition(
            request.OperationId,
            receipt.Status,
            current.Fingerprint,
            receipt.Error);
        return new ApplyResponseDto(false, replayed, receipt);
    }

    private static ManagedItemDto[] ReadAppliedItems(
        ApplyRequestDto request,
        ProjectSnapshotDto snapshot)
    {
        var entityIds = request.Utterances
            .Select(value => value.EntityId)
            .ToHashSet(StringComparer.Ordinal);
        return snapshot.ManagedItems
            .Where(item => entityIds.Contains(item.EntityId))
            .ToArray();
    }

    private static OperationReceiptDto CreateReceipt(
        ApplyRequestDto request,
        string status,
        string beforeFingerprint,
        string afterFingerprint,
        IReadOnlyList<ManagedItemDto> appliedItems,
        bool verified,
        string? error)
    {
        return new OperationReceiptDto(
            request.OperationId,
            request.RequestDigest,
            request.ProjectId,
            request.SceneId,
            request.ExpectedFingerprint,
            status,
            beforeFingerprint,
            afterFingerprint,
            appliedItems,
            verified,
            error);
    }

    private static OperationReceiptDto CreateNotStartedReceipt(
        Guid operationId,
        string requestDigest,
        string projectId,
        string sceneId,
        string expectedFingerprint,
        string error)
    {
        return new OperationReceiptDto(
            operationId,
            requestDigest,
            projectId,
            sceneId,
            expectedFingerprint,
            "not_started",
            expectedFingerprint,
            expectedFingerprint,
            [],
            false,
            error);
    }

    private async Task<ApplyResponseDto> RollBackNativeVoiceAsync(
        NativeVoiceApplyRequestDto request,
        ProjectSnapshotDto before,
        NativeVoicePreparation preparation,
        string failure,
        bool replayed)
    {
        string? rollbackError = null;
        try
        {
            BridgeFaultInjection.ThrowIf("during_rollback");
            var added = preparation.AddedItems.ToArray();
            if (added.Length > 0)
            {
                await Application.Current.Dispatcher.InvokeAsync(
                    () => ReplaceBatchItems(preparation.Timeline, added, []));
            }
        }
        catch (Exception error)
        {
            rollbackError = error.GetBaseException().Message;
        }

        var current = TrySnapshot() ?? before;
        var restored = rollbackError is null
            && string.Equals(current.Fingerprint, before.Fingerprint, StringComparison.Ordinal);
        var receipt = CreateReceipt(
            request,
            restored ? "rolled_back" : "recovery_required",
            before.Fingerprint,
            current.Fingerprint,
            ReadAppliedItems(request.Cues, current),
            false,
            rollbackError is null ? failure : $"{failure}; rollback failed: {rollbackError}");
        receiptStore.Put(receipt);
        recoveryStore.TryTransition(
            request.OperationId,
            receipt.Status,
            current.Fingerprint,
            receipt.Error);
        return new ApplyResponseDto(false, replayed, receipt);
    }

    private static ManagedItemDto[] ReadAppliedItems(
        IReadOnlyList<NativeVoiceCueDto> cues,
        ProjectSnapshotDto snapshot)
    {
        var realizationIds = cues
            .Select(value => value.RealizationId.ToString("D"))
            .ToHashSet(StringComparer.OrdinalIgnoreCase);
        return snapshot.ManagedItems
            .Where(item => item.RealizationId is not null && realizationIds.Contains(item.RealizationId))
            .ToArray();
    }

    private static bool NativeVoiceItemsMatch(
        IReadOnlyList<NativeVoiceCueDto> cues,
        IReadOnlyList<ManagedItemDto> actual)
    {
        if (actual.Count != cues.Count)
        {
            return false;
        }
        return cues.All(cue => actual.Any(item =>
            item.Kind == "voice"
            && item.EntityId == cue.EntityId
            && item.Revision == cue.Revision
            && string.Equals(
                item.RealizationId,
                cue.RealizationId.ToString("D"),
                StringComparison.OrdinalIgnoreCase)
            && item.Frame == cue.Frame
            && item.Layer == cue.Layer
            && item.Length is > 0
            && item.Length <= cue.MaxLength
            && item.Text == cue.DisplayText
            && item.Speaker == cue.CharacterName));
    }

    private static OperationReceiptDto CreateReceipt(
        NativeVoiceApplyRequestDto request,
        string status,
        string beforeFingerprint,
        string afterFingerprint,
        IReadOnlyList<ManagedItemDto> appliedItems,
        bool verified,
        string? error)
    {
        return new OperationReceiptDto(
            request.OperationId,
            request.RequestDigest,
            request.ProjectId,
            request.SceneId,
            request.ExpectedFingerprint,
            status,
            beforeFingerprint,
            afterFingerprint,
            appliedItems,
            verified,
            error);
    }

    private static OperationReceiptDto CreateReceipt(
        NativeVoiceMutationApplyRequestDto request,
        string status,
        string beforeFingerprint,
        string afterFingerprint,
        IReadOnlyList<ManagedItemDto> appliedItems,
        bool verified,
        string? error)
    {
        return new OperationReceiptDto(
            request.OperationId,
            request.RequestDigest,
            request.ProjectId,
            request.SceneId,
            request.ExpectedFingerprint,
            status,
            beforeFingerprint,
            afterFingerprint,
            appliedItems,
            verified,
            error);
    }

    private static NativeVoiceMutationPreparation PrepareNativeVoiceMutationBatch(
        IReadOnlyList<NativeVoiceMutationDto> mutations,
        string projectId)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var timeline = GetField(timelineViewModel, "timeline")
            ?? GetMember(timelineViewModel, "Timeline")
            ?? throw new BridgeUnavailableException("YMM4 timeline mutation API is unavailable");
        var mainModel = RequireMainModel(main);
        var addMethod = mainModel.GetType()
            .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .SingleOrDefault(method =>
                method.Name == "AddVoiceItemAsync"
                && method.GetParameters() is
                [
                    { ParameterType: var frameType },
                    { ParameterType: var layerType },
                    _,
                    { ParameterType: var serifType },
                    _,
                ]
                && frameType == typeof(int)
                && layerType == typeof(int)
                && serifType == typeof(string))
            ?? throw new BridgeUnavailableException(
                "YMM4 MainModel.AddVoiceItemAsync(int,int,Character,string,decorations) is unavailable");
        var rawItems = ReadItems(timelineViewModel);
        var requestedEntities = mutations.Select(value => value.EntityId)
            .ToHashSet(StringComparer.Ordinal);
        var requestedRealizations = mutations.Select(value => value.RealizationId).ToHashSet();
        var relevantMarkers = ReadNativeVoiceMarkers(rawItems)
            .Where(value => requestedEntities.Contains(value.Marker.EntityId)
                || requestedRealizations.Contains(value.Marker.RealizationId))
            .ToArray();
        if (relevantMarkers.Any(value => !string.Equals(
                value.Marker.ProjectId,
                projectId,
                StringComparison.Ordinal)))
        {
            throw new BridgeConflictException(
                "A requested native voice identity is owned by a foreign project",
                SnapshotCore().Fingerprint);
        }
        var originals = new Dictionary<Guid, RawItem>();
        foreach (var mutation in mutations.Where(value => value.Action is "update" or "delete"))
        {
            var matches = rawItems.Where(item =>
                    item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                    && RemarkCodec.TryDecode(item.Remark, out var marker)
                    && string.Equals(marker?.ProjectId, projectId, StringComparison.Ordinal)
                    && marker?.RealizationId == mutation.RealizationId)
                .ToArray();
            if (matches.Length != 1)
            {
                throw new BridgeConflictException(
                    $"Native voice mutation expected one realization {mutation.RealizationId}, found {matches.Length}",
                    SnapshotCore().Fingerprint);
            }
            EnsureUnlockedForMutation(
                matches[0].Item,
                $"native voice {mutation.EntityId}",
                SnapshotCore().Fingerprint);
            originals.Add(mutation.RealizationId, matches[0]);
        }
        foreach (var mutation in mutations.Where(value => value.Action == "create"))
        {
            if (rawItems.Any(item =>
                    item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                    && RemarkCodec.TryDecode(item.Remark, out var marker)
                    && string.Equals(marker?.ProjectId, projectId, StringComparison.Ordinal)
                    && (marker?.RealizationId == mutation.RealizationId
                        || string.Equals(marker?.EntityId, mutation.EntityId, StringComparison.Ordinal))))
            {
                throw new BridgeConflictException(
                    $"Native voice realization already exists: {mutation.RealizationId}",
                    SnapshotCore().Fingerprint);
            }
        }
        var characters = mutations
            .Where(value => value.Action != "delete")
            .ToDictionary(
                mutation => mutation.RealizationId,
                mutation => ResolveCharacter(timelineViewModel, mutation.CharacterName));
        var preservedStateDigests = originals.ToDictionary(
            pair => pair.Key,
            pair => ComputePreservedNativeVoiceStateDigest(pair.Value.Item));
        return new NativeVoiceMutationPreparation(
            mainModel,
            timelineViewModel,
            timeline,
            addMethod,
            CreateEmptyDecorations(addMethod.GetParameters()[4].ParameterType),
            mutations,
            characters,
            originals,
            preservedStateDigests,
            rawItems.Select(item => item.Item).ToHashSet(ReferenceEqualityComparer.Instance));
    }

    private static async Task ApplyNativeVoiceMutationBatchAsync(
        NativeVoiceMutationPreparation preparation,
        string projectId)
    {
        foreach (var mutation in preparation.Mutations)
        {
            if (mutation.Action == "delete")
            {
                var original = preparation.Originals[mutation.RealizationId];
                await Application.Current.Dispatcher.InvokeAsync(() =>
                    ReplaceBatchItems(preparation.Timeline, [original.Item], []));
                preparation.DeletedItems.Add(original.Item);
                if (preparation.DeletedItems.Count + preparation.AddedItems.Count == 1)
                {
                    BridgeFaultInjection.ThrowIf("after_partial_mutation");
                }
                continue;
            }

            var task = await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                try
                {
                    return preparation.AddMethod.Invoke(
                            preparation.MainModel,
                            [
                                mutation.Frame,
                                mutation.Layer,
                                preparation.Characters[mutation.RealizationId],
                                mutation.SpokenText,
                                preparation.Decorations,
                            ]) as Task
                        ?? throw new BridgeUnavailableException(
                            "YMM4 AddVoiceItemAsync did not return a Task");
                }
                catch (TargetInvocationException error)
                {
                    throw error.InnerException ?? error;
                }
            });
            await task.ConfigureAwait(false);

            RawItem[] newItems = [];
            for (var attempt = 0; attempt < 30; attempt++)
            {
                newItems = await Application.Current.Dispatcher.InvokeAsync(() => ReadItems(
                        preparation.TimelineViewModel)
                    .Where(item => !preparation.KnownItems.Contains(item.Item))
                    .ToArray());
                if (newItems.Any(item =>
                        item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                        && item.Frame == mutation.Frame
                        && item.Layer == mutation.Layer
                        && item.Text == mutation.SpokenText
                        && item.CharacterName == mutation.CharacterName
                        && item.Length > 0))
                {
                    break;
                }
                await Task.Delay(100).ConfigureAwait(false);
            }
            await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                foreach (var item in newItems)
                {
                    preparation.KnownItems.Add(item.Item);
                }
                var voices = newItems.Where(item =>
                        item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                        && item.Frame == mutation.Frame
                        && item.Layer == mutation.Layer
                        && item.Text == mutation.SpokenText
                        && item.CharacterName == mutation.CharacterName)
                    .ToArray();
                if (voices.Length != 1 || newItems.Length != 1)
                {
                    throw new BridgeUnavailableException(
                        $"Native voice mutation produced an ambiguous result for {mutation.EntityId}");
                }
                var voice = voices[0];
                preparation.AddedItems.Add(voice.Item);
                SetRequired(
                    voice.Item,
                    RemarkCodec.Append(
                        voice.Remark,
                        new NativeVoiceMarker(
                            "takegraph/v2",
                            projectId,
                            mutation.EntityId,
                            mutation.RealizationId,
                            mutation.Revision)),
                    "Remark");
                if (mutation.Action == "update")
                {
                    var original = preparation.Originals[mutation.RealizationId];
                    CopyPreservedNativeVoiceState(original.Item, voice.Item);
                    var preservedDigest = ComputePreservedNativeVoiceStateDigest(voice.Item);
                    if (!string.Equals(
                            preservedDigest,
                            preparation.PreservedStateDigests[mutation.RealizationId],
                            StringComparison.Ordinal))
                    {
                        throw new BridgeUnavailableException(
                            $"Native voice update could not preserve target-local state for {mutation.EntityId}");
                    }
                    ReplaceBatchItems(preparation.Timeline, [original.Item], []);
                    preparation.DeletedItems.Add(original.Item);
                }
                if (voice.Length <= 0 || voice.Length > mutation.MaxLength)
                {
                    throw new BridgeConflictException(
                        $"Native voice mutation exceeded its duration budget for {mutation.EntityId}",
                        SnapshotCore().Fingerprint);
                }
            });
            if (preparation.DeletedItems.Count + preparation.AddedItems.Count == 1)
            {
                BridgeFaultInjection.ThrowIf("after_partial_mutation");
            }
        }
    }

    private static void CopyPreservedNativeVoiceState(object source, object destination)
    {
        foreach (var propertyName in PreservedNativeVoicePropertyNames(source.GetType()))
        {
            CopyWritableProperty(source, destination, propertyName);
        }
        var keyFrames = GetMember(source, "KeyFrames");
        var setKeyFrames = destination.GetType().GetMethod(
            "SetKeyFrames",
            BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance);
        if (keyFrames is not null && setKeyFrames is not null)
        {
            setKeyFrames.Invoke(destination, [keyFrames]);
        }
    }

    private static readonly HashSet<string> OwnedOrDerivedNativeVoiceProperties = new(
        [
            "Serif",
            "Hatsuon",
            "Character",
            "CharacterName",
            "Pronounce",
            "VoiceCache",
            "VoiceLength",
            "FilePath",
            "Frame",
            "Layer",
            "Length",
            "Remark",
            "LipSyncFrames",
            "ContentLength",
            "OriginalContentLength",
            "ContentSeparations",
            "HasErrors",
            "Description",
            "Label",
            "LicenseOverviewDummy",
            "SetToDefaultDummy",
        ],
        StringComparer.Ordinal);

    private static IReadOnlyList<string> PreservedNativeVoicePropertyNames(Type type)
    {
        return type.GetProperties(BindingFlags.Public | BindingFlags.Instance)
            .Where(property =>
                property.CanRead
                && property.CanWrite
                && property.GetIndexParameters().Length == 0
                && !OwnedOrDerivedNativeVoiceProperties.Contains(property.Name))
            .Select(property => property.Name)
            .Append("KeyFrames")
            .Distinct(StringComparer.Ordinal)
            .Order(StringComparer.Ordinal)
            .ToArray();
    }

    internal static string ComputePreservedNativeVoiceStateDigest(object item)
    {
        var canonical = new StringBuilder("takegraph-ymm4-preserved-voice-state-v2\n");
        foreach (var propertyName in PreservedNativeVoicePropertyNames(item.GetType()))
        {
            AppendDigestValue(canonical, propertyName);
            var value = ReadPreservedMember(item, propertyName);
            AppendDigestValue(canonical, SerializeYmmValue(value));
        }
        return Hash(canonical.ToString());
    }

    internal static string SerializeYmmValue(object? value)
    {
        if (value is null)
        {
            return "takegraph-ymm4-value-v2|null";
        }
        try
        {
            var json = SerializeYmmObject(value);
            return CanonicalizeSerializedYmmValue(value.GetType(), json);
        }
        catch (BridgeUnavailableException)
        {
            throw;
        }
        catch (Exception error)
        {
            throw new BridgeUnavailableException(
                $"YMM4 preservation serialization failed for {value.GetType().FullName}: "
                + error.GetBaseException().Message);
        }
    }

    internal static string CanonicalizeSerializedYmmValue(Type type, string json)
    {
        try
        {
            using var document = JsonDocument.Parse(json);
            BridgeHost.RejectDuplicateProperties(
                document.RootElement,
                "$",
                new HashSet<string>(StringComparer.Ordinal));
            var typeIdentity = type.AssemblyQualifiedName
                ?? type.FullName
                ?? throw new BridgeUnavailableException(
                    "YMM4 preservation value has no stable type identity");
            return $"takegraph-ymm4-value-v2|{typeIdentity}|"
                + CanonicalJson.Canonicalize(document.RootElement);
        }
        catch (BridgeUnavailableException)
        {
            throw;
        }
        catch (Exception error) when (error is JsonException or BridgeValidationException)
        {
            throw new BridgeUnavailableException(
                $"YMM4 preservation JSON is invalid: {error.GetBaseException().Message}");
        }
    }

    private static object? ReadPreservedMember(object value, string name)
    {
        var flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance;
        var property = value.GetType().GetProperty(name, flags);
        var field = value.GetType().GetField(name, flags);
        if (property is null && field is null)
        {
            throw new BridgeUnavailableException(
                $"YMM4 preservation member is unavailable: {value.GetType().FullName}.{name}");
        }
        try
        {
            var found = property is not null
                ? property.GetValue(value)
                : field!.GetValue(value);
            if (found is not null
                && found.GetType().Name.Contains("ReactiveProperty", StringComparison.Ordinal))
            {
                var reactiveValue = found.GetType().GetProperty("Value", flags)
                    ?? throw new BridgeUnavailableException(
                        $"YMM4 reactive preservation member has no Value: {name}");
                found = reactiveValue.GetValue(found);
            }
            return found;
        }
        catch (BridgeUnavailableException)
        {
            throw;
        }
        catch (Exception error)
        {
            throw new BridgeUnavailableException(
                $"YMM4 preservation member read failed: {value.GetType().FullName}.{name}: "
                + error.GetBaseException().Message);
        }
    }

    private static void CopyWritableProperty(object source, object destination, string name)
    {
        var flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance;
        var sourceProperty = source.GetType().GetProperty(name, flags);
        var destinationProperty = destination.GetType().GetProperty(name, flags);
        if (sourceProperty?.CanRead == true
            && destinationProperty?.CanWrite == true
            && destinationProperty.PropertyType.IsAssignableFrom(sourceProperty.PropertyType))
        {
            destinationProperty.SetValue(destination, sourceProperty.GetValue(source));
        }
    }

    private async Task<ApplyResponseDto> RollBackNativeVoiceMutationsAsync(
        NativeVoiceMutationApplyRequestDto request,
        ProjectSnapshotDto before,
        NativeVoiceMutationPreparation preparation,
        string failure)
    {
        string? rollbackError = null;
        try
        {
            BridgeFaultInjection.ThrowIf("during_rollback");
            await Application.Current.Dispatcher.InvokeAsync(() => ReplaceBatchItems(
                preparation.Timeline,
                preparation.AddedItems.ToArray(),
                preparation.DeletedItems.ToArray()));
        }
        catch (Exception error)
        {
            rollbackError = error.GetBaseException().Message;
        }
        var current = TrySnapshot() ?? before;
        var restored = rollbackError is null
            && string.Equals(current.Fingerprint, before.Fingerprint, StringComparison.Ordinal);
        var receipt = CreateReceipt(
            request,
            restored ? "rolled_back" : "recovery_required",
            before.Fingerprint,
            current.Fingerprint,
            ReadMutationItems(request.Mutations, current),
            false,
            rollbackError is null ? failure : $"{failure}; rollback failed: {rollbackError}");
        receiptStore.Put(receipt);
        recoveryStore.TryTransition(
            request.OperationId,
            receipt.Status,
            current.Fingerprint,
            receipt.Error);
        return new ApplyResponseDto(false, false, receipt);
    }

    private static ManagedItemDto[] ReadMutationItems(
        IReadOnlyList<NativeVoiceMutationDto> mutations,
        ProjectSnapshotDto snapshot)
    {
        var ids = mutations.Select(value => value.RealizationId.ToString("D"))
            .ToHashSet(StringComparer.OrdinalIgnoreCase);
        return snapshot.ManagedItems
            .Where(item => item.RealizationId is not null && ids.Contains(item.RealizationId))
            .ToArray();
    }

    private static bool NativeVoiceMutationsMatch(
        IReadOnlyList<NativeVoiceMutationDto> mutations,
        IReadOnlyList<ManagedItemDto> actual)
    {
        foreach (var mutation in mutations)
        {
            var matches = actual.Where(item => string.Equals(
                    item.RealizationId,
                    mutation.RealizationId.ToString("D"),
                    StringComparison.OrdinalIgnoreCase))
                .ToArray();
            if (mutation.Action == "delete")
            {
                if (matches.Length != 0)
                {
                    return false;
                }
                continue;
            }
            if (matches.Length != 1
                || matches[0].Kind != "voice"
                || matches[0].EntityId != mutation.EntityId
                || matches[0].Revision != mutation.Revision
                || matches[0].Speaker != mutation.CharacterName
                || matches[0].Text != mutation.DisplayText
                || matches[0].Frame != mutation.Frame
                || matches[0].Layer != mutation.Layer
                || matches[0].Length is <= 0
                || matches[0].Length > mutation.MaxLength)
            {
                return false;
            }
        }
        return true;
    }

    private static RawItem FindNativeVoiceRawItem(Guid realizationId, string projectId)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var marked = ReadItems(timelineViewModel).Where(item =>
                item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                && RemarkCodec.TryDecode(item.Remark, out var marker)
                && marker?.RealizationId == realizationId)
            .Select(item => (Item: item, Marker: DecodeNativeVoiceMarker(item.Remark)))
            .ToArray();
        if (marked.Any(value => !string.Equals(
                value.Marker.ProjectId,
                projectId,
                StringComparison.Ordinal)))
        {
            throw new BridgeConflictException(
                $"Native voice realization is also owned by a foreign project: {realizationId}",
                SnapshotCore().Fingerprint);
        }
        var matches = marked.Select(value => value.Item).ToArray();
        return matches.Length switch
        {
            1 => matches[0],
            0 => throw new BridgeNotFoundException($"Native voice realization not found: {realizationId}"),
            _ => throw new BridgeConflictException(
                $"Native voice realization is ambiguous: {realizationId}",
                SnapshotCore().Fingerprint),
        };
    }

    private static NativeVoiceMarker DecodeNativeVoiceMarker(string? remark)
    {
        return RemarkCodec.TryDecode(remark, out var marker) && marker is not null
            ? marker
            : throw new BridgeUnavailableException("Native voice identity marker became invalid");
    }

    private static string CreateVoiceProvenanceJson(object item, Guid realizationId)
    {
        var document = new SortedDictionary<string, object?>(StringComparer.Ordinal)
        {
            ["schema"] = "takegraph/ymm4-native-voice-provenance/v1",
            ["realizationId"] = realizationId.ToString("D"),
            ["characterName"] = GetString(item, "CharacterName"),
            ["displayText"] = GetString(item, "Serif"),
            ["spokenText"] = GetString(item, "Hatsuon", "Serif"),
            ["pronounceType"] = GetMember(item, "Pronounce")?.GetType().FullName,
            ["voiceParameterType"] = GetMember(item, "VoiceParameter")?.GetType().FullName,
            ["voiceLength"] = GetMember(item, "VoiceLength")?.ToString(),
            ["audioEffectTypes"] = ReadTypeNames(GetMember(item, "AudioEffects")),
        };
        return System.Text.Json.JsonSerializer.Serialize(document, BridgeJson.Options);
    }

    private static string[] ReadTypeNames(object? value)
    {
        return value is IEnumerable enumerable
            ? enumerable.Cast<object?>()
                .Where(item => item is not null)
                .Select(item => item!.GetType().FullName ?? item.GetType().Name)
                .Order(StringComparer.Ordinal)
                .ToArray()
            : [];
    }

    private static void WriteImmutableArtifact(string path, byte[] bytes, string expectedHash)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        if (File.Exists(path))
        {
            var existingHash = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(path)));
            if (!ApplyRequestDigest.Matches(existingHash, expectedHash))
            {
                throw new BridgeUnavailableException($"Artifact hash collision at {path}");
            }
            return;
        }
        var temporary = $"{path}.tmp-{Guid.NewGuid():N}";
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
        try
        {
            File.Move(temporary, path);
        }
        catch (IOException) when (File.Exists(path))
        {
            File.Delete(temporary);
            var existingHash = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(path)));
            if (!ApplyRequestDigest.Matches(existingHash, expectedHash))
            {
                throw new BridgeUnavailableException($"Artifact hash collision at {path}");
            }
        }
    }

    internal OperationReceiptDto GetOperation(Guid operationId)
    {
        if (!receiptStore.TryGet(operationId, out var receipt) || receipt is null)
        {
            throw new BridgeNotFoundException($"Operation receipt not found: {operationId}");
        }
        return receipt;
    }

    internal async Task<SceneCaptureReceiptDto> CaptureSceneAsync(SceneCaptureRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        ValidateSceneCaptureRequest(request);
        if (!ProbeSceneCaptureRuntime())
        {
            throw new BridgeUnavailableException(
                "YMM4 native preview frame capture is unavailable for the active driver");
        }

        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            var before = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, before);
            EnsureFingerprint(request.ExpectedFingerprint, before.Fingerprint);

            var operationDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "TakeGraph",
                "scene-captures",
                request.OperationId.ToString("N"));
            var receiptPath = Path.Combine(operationDirectory, "receipt.json");
            if (File.Exists(receiptPath))
            {
                return ReadAndValidateSceneCaptureReceipt(receiptPath, request);
            }

            Directory.CreateDirectory(operationDirectory);
            var preview = await Application.Current.Dispatcher.InvokeAsync(RequirePreviewViewModel);
            var originalFrame = await Application.Current.Dispatcher.InvokeAsync(
                () => ReadPreviewFrame(preview, before.Fps));
            if (originalFrame is null)
            {
                throw new BridgeUnavailableException(
                    "YMM4 preview position cannot be read safely, so capture cannot restore it");
            }
            var dirtyBefore = await Application.Current.Dispatcher.InvokeAsync(ReadProjectDirty);
            var selectionBefore = await Application.Current.Dispatcher.InvokeAsync(ReadSelectionDigest);

            var captures = new List<SceneCaptureFrameDto>(request.Frames.Count);
            Exception? captureError = null;
            try
            {
                for (var index = 0; index < request.Frames.Count; index++)
                {
                    var requestedFrame = request.Frames[index];
                    await SeekPreviewToFrameAsync(preview, requestedFrame, before.Fps).ConfigureAwait(false);
                    await Task.Delay(120).ConfigureAwait(false);
                    var actualFrame = await Application.Current.Dispatcher.InvokeAsync(
                        () => ReadPreviewFrame(preview, before.Fps));
                    if (actualFrame is null || Math.Abs(actualFrame.Value - requestedFrame) > 1)
                    {
                        throw new BridgeUnavailableException(
                            $"YMM4 preview seek did not settle at frame {requestedFrame}");
                    }

                    var temporaryPath = Path.Combine(
                        operationDirectory,
                        $"frame-{index:D3}-{requestedFrame}.tmp-{Guid.NewGuid():N}.png");
                    var finalPath = Path.Combine(
                        operationDirectory,
                        $"frame-{index:D3}-{requestedFrame}.png");
                    await SavePreviewImageAsync(preview, temporaryPath, request.Alpha).ConfigureAwait(false);
                    var bytes = await ReadCompletedCaptureAsync(temporaryPath).ConfigureAwait(false);
                    var (width, height) = ReadPngDimensions(bytes);
                    var hash = Convert.ToHexStringLower(SHA256.HashData(bytes));
                    File.Move(temporaryPath, finalPath, overwrite: true);
                    captures.Add(new SceneCaptureFrameDto(
                        requestedFrame,
                        actualFrame.Value,
                        finalPath,
                        hash,
                        width,
                        height,
                        "image/png"));
                }
            }
            catch (Exception error)
            {
                captureError = error;
            }
            Exception? restoreError = null;
            try
            {
                await SeekPreviewToFrameAsync(preview, originalFrame.Value, before.Fps)
                    .ConfigureAwait(false);
            }
            catch (Exception error)
            {
                restoreError = error;
            }

            int? restoredFrame = null;
            var dirtyAfter = dirtyBefore;
            string? selectionAfter = null;
            Exception? restorationReadbackError = null;
            try
            {
                restoredFrame = await Application.Current.Dispatcher.InvokeAsync(
                    () => ReadPreviewFrame(preview, before.Fps));
                dirtyAfter = await Application.Current.Dispatcher.InvokeAsync(ReadProjectDirty);
                selectionAfter = await Application.Current.Dispatcher.InvokeAsync(ReadSelectionDigest);
            }
            catch (Exception error)
            {
                restorationReadbackError = error;
            }
            var transientStateRestored = restoreError is null
                && restorationReadbackError is null
                && restoredFrame is not null
                && Math.Abs(restoredFrame.Value - originalFrame.Value) <= 1
                && string.Equals(selectionBefore, selectionAfter, StringComparison.Ordinal);
            var dirtyStateRestored = restorationReadbackError is null
                && dirtyAfter == dirtyBefore;
            if (captureError is not null || !transientStateRestored || !dirtyStateRestored)
            {
                var afterFailure = TrySnapshot() ?? before;
                var status = transientStateRestored && dirtyStateRestored
                    ? "failed"
                    : "recovery_required";
                var failure = DescribeSceneCaptureFailure(
                    captureError,
                    restoreError,
                    restorationReadbackError,
                    transientStateRestored,
                    dirtyStateRestored);
                WriteSceneCaptureReceipt(
                    receiptPath,
                    new SceneCaptureReceiptDto(
                        request.OperationId,
                        request.RequestDigest,
                        request.ProjectId,
                        request.SceneId,
                        request.SourceRevision,
                        request.ExpectedFingerprint,
                        request.CaptureProfileDigest,
                        status,
                        before.Fingerprint,
                        afterFailure.Fingerprint,
                        captures,
                        SceneCaptureDriver,
                        Hash(SceneCaptureDriver),
                        transientStateRestored,
                        dirtyBefore,
                        dirtyAfter,
                        failure));
                throw new BridgeUnavailableException(
                    $"Scene capture {status}: {failure}");
            }

            var after = Snapshot();
            var unchanged = string.Equals(
                before.Fingerprint,
                after.Fingerprint,
                StringComparison.Ordinal);
            var receipt = new SceneCaptureReceiptDto(
                request.OperationId,
                request.RequestDigest,
                request.ProjectId,
                request.SceneId,
                request.SourceRevision,
                request.ExpectedFingerprint,
                request.CaptureProfileDigest,
                unchanged ? "captured" : "stale",
                before.Fingerprint,
                after.Fingerprint,
                captures,
                SceneCaptureDriver,
                Hash(SceneCaptureDriver),
                transientStateRestored,
                dirtyBefore,
                dirtyAfter,
                unchanged ? null : "YMM4 scene changed while frames were being captured");
            WriteSceneCaptureReceipt(receiptPath, receipt);
            return receipt;
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal CapabilitiesDto Capabilities()
    {
        var observationRuntime = IsSupportedObservationRuntime();
        var capabilities = new List<string>
        {
            "readback_verification",
            "request_bound_receipts",
            "write_ahead_apply",
            "recovery_readback",
        };
        if (observationRuntime)
        {
            capabilities.Add("scene_composition_current");
        }
        if (!IsSupportedMutationRuntime())
        {
            return new CapabilitiesDto(BridgeContract.ProtocolVersion, capabilities);
        }

        capabilities.AddRange(
        [
            "managed_audio",
            "managed_caption",
            "unified_target_plan",
            "idempotent_apply",
            "undo_batch",
            "mutation_profile_ymm4_4_55_1_1",
            "project_checkpoint_verified",
            "native_portrait_upsert",
            "native_face_upsert",
            "native_image_upsert",
            "native_video_upsert",
            "native_audio_upsert",
            "native_effect_typed_mutation",
            "native_template_instantiate",
            "metadata_remark_detach",
        ]);
        if (ProbeProjectInitializationRuntime())
        {
            capabilities.Add("project_initialize_save_as_verified");
        }
        if (ProbeBindableRenderRuntime())
        {
            capabilities.AddRange(
            [
                "project_render",
                "project_render_cancel",
                "project_render_media_receipt",
            ]);
        }
        var nativeVoiceRuntime = ProbeNativeVoiceRuntime();
        if (nativeVoiceRuntime)
        {
            capabilities.AddRange(
            [
                "timeline_edit_managed_cue_mixed",
                "native_voice_create",
                "native_voice_update_replace_preserving_user_state",
                "native_voice_delete",
                "native_voice_exact_wav_export",
                "native_voice_host_bound_provenance",
                "native_voice_remark_identity",
                "native_voice_bounded_duration",
            ]);
        }
        if (ProbeSceneCaptureRuntime())
        {
            capabilities.AddRange(
            [
                "scene_capture_native_png",
                "scene_capture_playhead_restore",
                "scene_capture_content_hash",
            ]);
        }
        return new CapabilitiesDto(BridgeContract.ProtocolVersion, capabilities);
    }

    private static bool ProbeBindableRenderRuntime()
    {
        return TryCaptureRenderRuntimeBinding(out _, out _);
    }

    private static bool ProbeProjectInitializationRuntime()
    {
        try
        {
            return Application.Current.Dispatcher.Invoke(() =>
            {
                EnsureSaveAsRuntime();
                return true;
            });
        }
        catch
        {
            return false;
        }
    }

    internal DescriptorCatalogDto Descriptors()
    {
        return Application.Current.Dispatcher.Invoke(() =>
        {
            var snapshot = SnapshotCore();
            var main = RequireMainViewModel();
            var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
                ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
            var descriptors = new List<TargetDescriptorDto>();

            if (GetMember(timelineViewModel, "Characters") is IEnumerable characterValues)
            {
                var characters = characterValues.Cast<object>()
                    .Where(value => value is not null)
                    .ToArray();
                foreach (var group in characters.GroupBy(
                             value => GetString(value, "Name"),
                             StringComparer.Ordinal))
                {
                    var values = group.ToArray();
                    for (var characterIndex = 0; characterIndex < values.Length; characterIndex++)
                    {
                        var character = values[characterIndex];
                        var metadata = new SortedDictionary<string, string>(StringComparer.Ordinal)
                        {
                            ["groupName"] = GetString(character, "GroupName"),
                            ["voiceType"] = GetMember(character, "Voice")?.GetType().FullName ?? string.Empty,
                            ["voiceParameterType"] = GetMember(character, "VoiceParameter")?.GetType().FullName
                                ?? string.Empty,
                            ["tachieType"] = GetMember(character, "TachieType")?.ToString() ?? string.Empty,
                            ["tachieCharacterParameterType"] = GetMember(character, "TachieCharacterParameter")
                                ?.GetType().FullName ?? string.Empty,
                        };
                        var configDigest = HashDescriptor("character-config", metadata);
                        descriptors.Add(new TargetDescriptorDto(
                            $"ymm4-character:{Hash($"character|{group.Key}|{configDigest}|{(values.Length == 1 ? 0 : characterIndex)}")}",
                            "character",
                            group.Key,
                            configDigest,
                            HashDescriptor("character-schema", new SortedDictionary<string, string>(
                                metadata.ToDictionary(pair => pair.Key, _ => "string"),
                                StringComparer.Ordinal)),
                            values.Length == 1 && !string.IsNullOrWhiteSpace(group.Key),
                            false,
                            metadata));
                    }
                }
            }

            var templateObjects = new HashSet<object>(ReferenceEqualityComparer.Instance);
            var visitedTemplateNodes = new HashSet<object>(ReferenceEqualityComparer.Instance);
            foreach (var propertyName in TemplateMenuPropertyNames)
            {
                CollectTemplates(
                    GetMember(timelineViewModel, propertyName),
                    templateObjects,
                    visitedTemplateNodes);
            }
            CollectTemplates(
                GetMember(main, "AddTemplateContextMenuViewModel"),
                templateObjects,
                visitedTemplateNodes);
            foreach (var template in templateObjects)
            {
                descriptors.Add(DescribeTemplate(template));
            }

            foreach (var effectType in ReadEffectTypes())
            {
                var effectKind = EffectKind(effectType);
                var schema = new SortedDictionary<string, string>(StringComparer.Ordinal);
                foreach (var property in effectType.GetProperties(BindingFlags.Public | BindingFlags.Instance)
                             .Where(property => property.GetIndexParameters().Length == 0))
                {
                    schema[property.Name] = $"{property.PropertyType.FullName}|write={property.CanWrite}";
                }
                var assemblyName = effectType.Assembly.GetName();
                var metadata = new SortedDictionary<string, string>(StringComparer.Ordinal)
                {
                    ["type"] = effectType.FullName ?? effectType.Name,
                    ["assembly"] = assemblyName.Name ?? string.Empty,
                    ["assemblyVersion"] = assemblyName.Version?.ToString() ?? string.Empty,
                };
                var mutationAllowed = string.Equals(
                    effectType.FullName,
                    "YukkuriMovieMaker.Project.Effects.InvertEffect",
                    StringComparison.Ordinal);
                object? effect = null;
                try
                {
                    effect = Activator.CreateInstance(effectType);
                }
                catch
                {
                    // Descriptor discovery remains read-only even when a third-party
                    // effect's nominal parameterless constructor needs live services.
                }
                descriptors.Add(new TargetDescriptorDto(
                    $"ymm4-effect:{Hash($"{assemblyName.Name}|{effectType.FullName}")}",
                    effectKind,
                    effect is not null && GetString(effect, "Label") is { Length: > 0 } label
                        ? label
                        : effectType.Name,
                    HashDescriptor("effect-config", metadata),
                    HashDescriptor("effect-schema", schema),
                    true,
                    mutationAllowed,
                    metadata));
            }

            var uniqueDescriptors = descriptors
                .GroupBy(value => value.DescriptorId, StringComparer.Ordinal)
                .SelectMany(group => group.Count() == 1
                    ? group
                    : group.OrderBy(value => value.ConfigDigest, StringComparer.Ordinal)
                        .ThenBy(value => value.SchemaDigest, StringComparer.Ordinal)
                        .Select((value, index) => value with
                        {
                            DescriptorId = $"{value.DescriptorId}:ambiguous-{index}",
                            Bindable = false,
                        }))
                .ToArray();
            var ordered = uniqueDescriptors
                .OrderBy(value => value.Kind, StringComparer.Ordinal)
                .ThenBy(value => value.Name, StringComparer.Ordinal)
                .ThenBy(value => value.DescriptorId, StringComparer.Ordinal)
                .ToArray();
            var catalogFields = new SortedDictionary<string, string>(StringComparer.Ordinal);
            for (var index = 0; index < ordered.Length; index++)
            {
                var descriptor = ordered[index];
                catalogFields[index.ToString("D6")] = string.Join(
                    "\n",
                    descriptor.DescriptorId,
                    descriptor.Kind,
                    descriptor.ConfigDigest,
                    descriptor.SchemaDigest,
                    descriptor.Bindable,
                    descriptor.MutationAllowed);
            }
            return new DescriptorCatalogDto(
                BridgeContract.ProtocolVersion,
                snapshot.ProjectId,
                snapshot.SceneId,
                Hash($"ymm4-driver|{SupportedMutationYmm4Version}"),
                HashDescriptor("descriptor-catalog", catalogFields),
                ordered);
        });
    }

    private static string HashDescriptor(
        string domain,
        IReadOnlyDictionary<string, string> values)
    {
        var canonical = new StringBuilder(domain).Append('\n');
        foreach (var pair in values.OrderBy(pair => pair.Key, StringComparer.Ordinal))
        {
            AppendDigestValue(canonical, pair.Key);
            AppendDigestValue(canonical, pair.Value);
        }
        return Hash(canonical.ToString());
    }

    private static void AppendDigestValue(StringBuilder canonical, string value)
    {
        canonical.Append(Encoding.UTF8.GetByteCount(value))
            .Append(':')
            .Append(value)
            .Append('\n');
    }

    internal static TargetDescriptorDto DescribeTemplate(object template)
    {
        var name = GetString(template, "Name");
        var pathSegments = GetMember(template, "Path") is IEnumerable pathValues
            ? pathValues.Cast<object?>().Select(value => value?.ToString() ?? string.Empty).ToArray()
            : [];
        var itemTypes = GetMember(template, "Items") is IEnumerable itemValues
            ? itemValues.Cast<object?>()
                .Where(value => value is not null)
                .Select(value => value!.GetType().FullName ?? value.GetType().Name)
                .Order(StringComparer.Ordinal)
                .ToArray()
            : [];
        var metadata = new SortedDictionary<string, string>(StringComparer.Ordinal)
        {
            ["path"] = string.Join("/", pathSegments),
            ["group"] = GetMember(template, "Group")?.ToString() ?? string.Empty,
            ["sceneId"] = GetMember(template, "SceneId")?.ToString() ?? string.Empty,
            ["itemTypes"] = string.Join("\n", itemTypes),
            ["width"] = GetMember(template, "Width")?.ToString() ?? string.Empty,
            ["height"] = GetMember(template, "Height")?.ToString() ?? string.Empty,
            ["fps"] = GetMember(template, "FPS")?.ToString() ?? string.Empty,
            ["hz"] = GetMember(template, "Hz")?.ToString() ?? string.Empty,
        };
        return new TargetDescriptorDto(
            $"ymm4-template:{Hash($"template|{metadata["path"]}|{name}|{metadata["group"]}")}",
            "template",
            name,
            HashDescriptor("template-config", metadata),
            HashDescriptor("template-schema", new SortedDictionary<string, string>(
                metadata.ToDictionary(pair => pair.Key, _ => "string"),
                StringComparer.Ordinal)),
            !string.IsNullOrWhiteSpace(name),
            false,
            metadata);
    }

    internal static void CollectTemplates(
        object? value,
        ISet<object> result,
        ISet<object>? visited = null)
    {
        if (value is null)
        {
            return;
        }
        visited ??= new HashSet<object>(ReferenceEqualityComparer.Instance);
        if (!visited.Add(value))
        {
            return;
        }
        var typeName = value.GetType().FullName ?? value.GetType().Name;
        if (typeName == "YukkuriMovieMaker.Settings.ItemTemplate")
        {
            result.Add(value);
            return;
        }
        var embeddedTemplate = GetMember(value, "Template");
        if (embeddedTemplate is not null
            && (embeddedTemplate.GetType().FullName ?? embeddedTemplate.GetType().Name)
                == "YukkuriMovieMaker.Settings.ItemTemplate")
        {
            result.Add(embeddedTemplate);
        }
        if (GetMember(value, "Items") is not IEnumerable children)
        {
            return;
        }
        foreach (var child in children.Cast<object?>().Where(child => child is not null))
        {
            CollectTemplates(child, result, visited);
        }
    }

    private static IReadOnlyList<Type> ReadEffectTypes()
    {
        var audioInterface = FindLoadedType("YukkuriMovieMaker.Plugin.Effects.IAudioEffect");
        var videoInterface = FindLoadedType("YukkuriMovieMaker.Plugin.Effects.IVideoEffect");
        if (audioInterface is null || videoInterface is null)
        {
            return [];
        }
        return AppDomain.CurrentDomain.GetAssemblies()
            .SelectMany(SafeGetTypes)
            .Where(type =>
                !type.IsAbstract
                && !type.IsInterface
                && (audioInterface.IsAssignableFrom(type) || videoInterface.IsAssignableFrom(type))
                && type.GetConstructor(Type.EmptyTypes) is not null)
            .Distinct()
            .OrderBy(type => type.FullName, StringComparer.Ordinal)
            .ToArray();
    }

    private static string EffectKind(Type type)
    {
        var audioInterface = FindLoadedType("YukkuriMovieMaker.Plugin.Effects.IAudioEffect");
        return audioInterface?.IsAssignableFrom(type) is true ? "audio-effect" : "video-effect";
    }

    private static Type? FindLoadedType(string fullName)
    {
        return AppDomain.CurrentDomain.GetAssemblies()
            .Select(assembly => assembly.GetType(fullName, throwOnError: false))
            .FirstOrDefault(type => type is not null);
    }

    private static IEnumerable<Type> SafeGetTypes(Assembly assembly)
    {
        try
        {
            return assembly.GetTypes();
        }
        catch (ReflectionTypeLoadException error)
        {
            return error.Types.Where(type => type is not null).Cast<Type>();
        }
        catch
        {
            return [];
        }
    }

    private static string SerializeYmmObject(object value)
    {
        var jsonType = FindLoadedType("YukkuriMovieMaker.Json.Json")
            ?? throw new BridgeUnavailableException("YMM4 canonical JSON serializer is unavailable");
        var method = jsonType.GetMethods(BindingFlags.Public | BindingFlags.Static)
            .SingleOrDefault(candidate =>
                candidate.Name == "GetJsonText"
                && candidate.IsGenericMethodDefinition
                && candidate.GetParameters().Length == 2)
            ?? throw new BridgeUnavailableException("YMM4 Json.GetJsonText<T> is unavailable");
        try
        {
            return method.MakeGenericMethod(value.GetType()).Invoke(null, [value, Type.Missing]) as string
                ?? throw new BridgeUnavailableException("YMM4 JSON serialization returned no text");
        }
        catch (TargetInvocationException error)
        {
            throw new BridgeUnavailableException(
                $"YMM4 JSON serialization failed: {(error.InnerException ?? error).Message}");
        }
    }

    private static object DeserializeYmmObject(RecoveryItemDto value)
    {
        var actualHash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(value.Json)));
        if (!ApplyRequestDigest.Matches(value.Sha256, actualHash))
        {
            throw new BridgeUnavailableException("Recovery item JSON hash mismatch");
        }
        var itemType = Type.GetType(value.TypeName, throwOnError: false)
            ?? AppDomain.CurrentDomain.GetAssemblies()
                .Select(assembly => assembly.GetType(
                    value.TypeName.Split(',')[0].Trim(),
                    throwOnError: false))
                .FirstOrDefault(type => type is not null)
            ?? throw new BridgeUnavailableException(
                $"Recovery item type is unavailable: {value.TypeName}");
        if (itemType.Assembly.GetName().Name != "YukkuriMovieMaker"
            || itemType.FullName?.StartsWith(
                "YukkuriMovieMaker.Project.Items.",
                StringComparison.Ordinal) is not true)
        {
            throw new BridgeUnavailableException(
                $"Recovery item type is outside the supported YMM4 item boundary: {value.TypeName}");
        }
        var jsonType = FindLoadedType("YukkuriMovieMaker.Json.Json")
            ?? throw new BridgeUnavailableException("YMM4 canonical JSON serializer is unavailable");
        var method = jsonType.GetMethods(BindingFlags.Public | BindingFlags.Static)
            .SingleOrDefault(candidate =>
                candidate.Name == "LoadFromText"
                && candidate.IsGenericMethodDefinition
                && candidate.GetParameters() is [{ ParameterType: var parameterType }]
                && parameterType == typeof(string))
            ?? throw new BridgeUnavailableException("YMM4 Json.LoadFromText<T> is unavailable");
        try
        {
            return method.MakeGenericMethod(itemType).Invoke(null, [value.Json])
                ?? throw new BridgeUnavailableException("YMM4 recovery deserialization returned no item");
        }
        catch (TargetInvocationException error)
        {
            throw new BridgeUnavailableException(
                $"YMM4 recovery deserialization failed: {(error.InnerException ?? error).Message}");
        }
    }

    internal ProjectControlsDto Controls()
    {
        return Application.Current.Dispatcher.Invoke(() =>
        {
            var commands = ControlScopes()
                .SelectMany(scope => scope.Value.GetType()
                    .GetProperties(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                    .Where(property => typeof(ICommand).IsAssignableFrom(property.PropertyType))
                    .Where(property => IsProjectControlName(property.Name))
                    .Select(property => ReadControl(scope.Key, scope.Value, property)))
                .Where(value => value is not null)
                .Cast<ProjectControlDto>()
                .Concat(ControlScopes()
                    .Where(scope => scope.Key.EndsWith("_undo_redo", StringComparison.Ordinal))
                    .SelectMany(scope => new[]
                    {
                        new ProjectControlDto(
                            scope.Key,
                            "Undo",
                            GetMember(scope.Value, "IsUndoable") is true),
                        new ProjectControlDto(
                            scope.Key,
                            "Redo",
                            GetMember(scope.Value, "IsRedoable") is true),
                    }))
                .Concat(new[]
                {
                    new ProjectControlDto(
                        "takegraph_batch",
                        "Undo",
                        lastBatch is not null && !lastBatchUndone),
                    new ProjectControlDto(
                        "takegraph_batch",
                        "Redo",
                        lastBatch is not null && lastBatchUndone),
                })
                .Concat(ControlScopes().SelectMany(scope => scope.Value.GetType()
                    .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                    .Where(method => method.Name is "SaveProject" or "SaveProjectAs")
                    .Select(method => new ProjectControlDto(
                        $"{scope.Key}_fixed_method",
                        $"{method.Name}({string.Join(',', method.GetParameters().Select(parameter => parameter.ParameterType.Name))})",
                        false))))
                .OrderBy(value => value.Scope, StringComparer.Ordinal)
                .ThenBy(value => value.Name, StringComparer.Ordinal)
                .ToArray();
            return new ProjectControlsDto(commands);
        });
    }

    internal async Task<ProjectControlResultDto> SaveProjectAsync()
    {
        ValidateMutationRuntime();
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            BridgeFaultInjection.ThrowIf("before_save");
            var invocation = await InvokeSaveProjectAsync().ConfigureAwait(false);
            BridgeFaultInjection.ThrowIf("during_save");
            return new ProjectControlResultDto("save", invocation.Scope, "SaveProject(string)", true);
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal ProjectInitializationPreparationDto PrepareProjectInitialization(
        ProjectInitializationPrepareRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        return Application.Current.Dispatcher.Invoke(() =>
        {
            EnsureSaveAsRuntime();
            var snapshot = SnapshotCore();
            if (!string.IsNullOrWhiteSpace(snapshot.ProjectPath))
            {
                throw new BridgeConflictException(
                    "Project initialization requires an active untitled YMM4 project",
                    snapshot.Fingerprint);
            }
            if (snapshot.ManagedItems.Count != 0 || snapshot.NativeExtensions.Count != 0)
            {
                throw new BridgeConflictException(
                    "Untitled project initialization cannot rebind existing TakeGraph-managed identities",
                    snapshot.Fingerprint);
            }
            var destinationPath = ValidateNewProjectPath(request.DestinationPath);
            var instanceId = CaptureProjectInstanceId(RequireActiveProjectInstance());
            return new ProjectInitializationPreparationDto(
                BridgeContract.ProtocolVersion,
                ProjectInitializationProfileDigest(),
                instanceId,
                snapshot,
                destinationPath,
                ProjectPathDigest(destinationPath),
                ProjectIdForPath(destinationPath),
                FingerprintForPath(snapshot, destinationPath),
                false);
        });
    }

    internal ProjectInstanceBindingDto ProjectInstanceBinding()
    {
        ValidateMutationRuntime();
        return Application.Current.Dispatcher.Invoke(() =>
        {
            var snapshot = SnapshotCore();
            return new ProjectInstanceBindingDto(
                BridgeContract.ProtocolVersion,
                ProjectInitializationProfileDigest(),
                CaptureProjectInstanceId(RequireActiveProjectInstance()),
                snapshot);
        });
    }

    internal async Task<ProjectInitializationReceiptDto> InitializeProjectAsync(
        ProjectInitializationRequestDto request)
    {
        ValidateProjectInitializationRequest(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (projectOperationStore.TryGetProjectInitialization(request.OperationId, out var existing)
                && existing is not null)
            {
                EnsureProjectInitializationBinding(request, existing);
                if (existing.Status == "verified")
                {
                    return ReplayProjectInitialization(existing);
                }
                if (existing.Status is "applying" or "recovery_required")
                {
                    await EnsureRecoveryClearAsync(request.OperationId).ConfigureAwait(false);
                    return await ResumeProjectInitializationAsync(request, existing)
                        .ConfigureAwait(false);
                }
                return existing;
            }

            await EnsureRecoveryClearAsync().ConfigureAwait(false);

            var destinationPath = ValidateProjectPath(request.DestinationPath);
            var before = await ReadExactProjectInitializationSourceAsync(request, destinationPath)
                .ConfigureAwait(false);

            var applying = new ProjectInitializationReceiptDto(
                request.OperationId,
                request.RequestDigest,
                "applying",
                request.DriverProfileDigest,
                request.SourceProjectInstanceId,
                request.SourceProjectId,
                request.SourceSceneId,
                before.Fingerprint,
                destinationPath,
                request.DestinationPathDigest,
                request.PredictedProjectId,
                request.PredictedFingerprint,
                null,
                null,
                null,
                null,
                null,
                null,
                null);
            if (File.Exists(destinationPath))
            {
                var failed = applying with
                {
                    Status = "failed",
                    AfterSnapshot = before,
                    Error = "Project initialization destination is already owned by another file",
                };
                projectOperationStore.PutProjectInitialization(failed);
                return failed;
            }
            projectOperationStore.PutProjectInitialization(applying);
            return await ResumeProjectInitializationAsync(request, applying).ConfigureAwait(false);
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal ProjectInitializationReceiptDto GetProjectInitialization(Guid operationId)
    {
        if (!projectOperationStore.TryGetProjectInitialization(operationId, out var receipt)
            || receipt is null)
        {
            throw new BridgeNotFoundException(
                $"Project initialization receipt not found: {operationId}");
        }
        return receipt.Status == "verified" ? ReplayProjectInitialization(receipt) : receipt;
    }

    internal CheckpointProfileDto CheckpointProfile()
    {
        ValidateMutationRuntime();
        return new CheckpointProfileDto(
            CheckpointProfileDigest(),
            Hash(CheckpointDriver),
            true);
    }

    internal async Task<CheckpointReceiptDto> CreateCheckpointAsync(CheckpointRequestDto request)
    {
        ValidateCheckpointRequest(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            if (projectOperationStore.TryGetCheckpoint(request.OperationId, out var existing)
                && existing is not null)
            {
                EnsureCheckpointBinding(request, existing);
                if (existing.Status != "applying")
                {
                    return ValidateCheckpointArtifact(existing);
                }

                var recoverySnapshot = Snapshot();
                EnsureTarget(request.ProjectId, request.SceneId, recoverySnapshot);
                existing = RecoverCheckpoint(request, existing, recoverySnapshot);
                projectOperationStore.PutCheckpoint(existing);
                return ValidateCheckpointArtifact(existing);
            }


            var before = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, before);
            EnsureFingerprint(request.ExpectedStateDigest, before.Fingerprint);

            if (string.IsNullOrWhiteSpace(before.ProjectPath))
            {
                throw new BridgeUnavailableException(
                    "The active YMM4 project has no existing path; use Save As in YMM4 first");
            }
            var projectPath = Path.GetFullPath(before.ProjectPath);
            var preHash = File.Exists(projectPath) ? HashFile(projectPath) : null;
            BridgeFaultInjection.ThrowIf("before_checkpoint_journal_write");
            var applying = new CheckpointReceiptDto(
                request.OperationId,
                request.RequestDigest,
                request.ProjectId,
                request.SceneId,
                request.SourceRevision,
                request.TargetIdentityDigest,
                request.ExpectedStateDigest,
                request.CheckpointProfileDigest,
                "applying",
                projectPath,
                preHash,
                null,
                null,
                before.Fingerprint,
                before.Fingerprint,
                Hash(CheckpointDriver),
                null);
            projectOperationStore.PutCheckpoint(applying);
            BridgeFaultInjection.ThrowIf("after_checkpoint_journal_before_save");

            try
            {
                var saved = await InvokeSaveProjectAsync().ConfigureAwait(false);
                if (!PathsEqual(saved.ProjectPath, projectPath))
                {
                    throw new BridgeUnavailableException("YMM4 saved a different project path");
                }
                BridgeFaultInjection.ThrowIf("during_save");
                var after = Snapshot();
                var postHash = File.Exists(projectPath) ? HashFile(projectPath) : null;
                ulong? postBytes = File.Exists(projectPath)
                    ? checked((ulong)new FileInfo(projectPath).Length)
                    : null;
                var verified = string.Equals(
                        after.Fingerprint,
                        request.ExpectedStateDigest,
                        StringComparison.Ordinal)
                    && postHash is not null
                    && postBytes > 0
                    && IsProjectSaved();
                var receipt = applying with
                {
                    Status = verified ? "verified" : "stale",
                    PostFileSha256 = postHash,
                    PostFileBytes = postBytes,
                    AfterStateDigest = after.Fingerprint,
                    Error = verified
                        ? null
                        : "Project state changed during save, the file is missing, or YMM4 still reports unsaved state",
                };
                projectOperationStore.PutCheckpoint(receipt);
                BridgeFaultInjection.ThrowIf("after_checkpoint_receipt");
                return receipt;
            }
            catch (BridgeSimulatedCrashException)
            {
                throw;
            }
            catch (Exception error)
            {
                var after = TrySnapshot() ?? before;
                var failed = applying with
                {
                    Status = "recovery_required",
                    PostFileSha256 = File.Exists(projectPath) ? HashFile(projectPath) : null,
                    PostFileBytes = File.Exists(projectPath)
                        ? checked((ulong)new FileInfo(projectPath).Length)
                        : null,
                    AfterStateDigest = after.Fingerprint,
                    Error = error.GetBaseException().Message,
                };
                projectOperationStore.PutCheckpoint(failed);
                return failed;
            }
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal CheckpointReceiptDto GetCheckpoint(Guid operationId)
    {
        if (!projectOperationStore.TryGetCheckpoint(operationId, out var receipt) || receipt is null)
        {
            throw new BridgeNotFoundException($"Checkpoint receipt not found: {operationId}");
        }
        return ValidateCheckpointArtifact(receipt);
    }

    private static string CheckpointProfileDigest()
    {
        return HashDescriptor(
            "checkpoint-profile",
            new SortedDictionary<string, string>(StringComparer.Ordinal)
            {
                ["driver"] = CheckpointDriver,
                ["existingPathOnly"] = "true",
            });
    }

    private static string ProjectInitializationProfileDigest()
    {
        return HashDescriptor(
            "project-initialization-profile",
            new SortedDictionary<string, string>(StringComparer.Ordinal)
            {
                ["driver"] = ProjectInitializationDriver,
                ["newPathOnly"] = "true",
                ["overwrite"] = "false",
                ["extension"] = ".ymmp",
                ["sourceInstanceBound"] = "true",
                ["managedIdentityRebind"] = "rejected",
                ["temporaryPath"] = "same_directory_unique_project_dto",
                ["namespaceClaim"] = "atomic_move_no_overwrite",
                ["pathRebind"] = "MainModel.ChangeProjectPath(string)",
                ["savedStateCommit"] = "MainModel.IsProjectFileSaved=true",
                ["serializer"] = "YukkuriMovieMaker.Json.Json.Save<Project>",
            });
    }

    private static string ValidateProjectPath(string path)
    {
        if (string.IsNullOrWhiteSpace(path) || !Path.IsPathFullyQualified(path))
        {
            throw new BridgeValidationException(
                "Project initialization destination must be an absolute path");
        }
        string fullPath;
        try
        {
            fullPath = Path.GetFullPath(path);
        }
        catch (Exception error) when (error is ArgumentException or NotSupportedException or PathTooLongException)
        {
            throw new BridgeValidationException(
                $"Project initialization destination is invalid: {error.Message}");
        }
        if (!string.Equals(Path.GetExtension(fullPath), ".ymmp", StringComparison.OrdinalIgnoreCase))
        {
            throw new BridgeValidationException(
                "Project initialization destination must use the .ymmp extension");
        }
        if (Directory.Exists(fullPath))
        {
            throw new BridgeValidationException(
                "Project initialization destination is a directory");
        }
        var parent = Path.GetDirectoryName(fullPath);
        if (string.IsNullOrWhiteSpace(parent) || !Directory.Exists(parent))
        {
            throw new BridgeValidationException(
                "Project initialization destination directory must already exist");
        }
        return fullPath;
    }

    private static string ValidateNewProjectPath(string path)
    {
        var fullPath = ValidateProjectPath(path);
        if (File.Exists(fullPath))
        {
            throw new BridgeConflictException(
                "Project initialization never overwrites an existing file",
                HashFile(fullPath));
        }
        return fullPath;
    }

    private static string ProjectPathDigest(string path) =>
        HashDescriptor(
            "project-initialization-path",
            new SortedDictionary<string, string>(StringComparer.Ordinal)
            {
                ["path"] = Path.GetFullPath(path),
            });

    private static string ProjectIdForPath(string path) => Hash($"project|{Path.GetFullPath(path)}");

    private static string FingerprintForPath(ProjectSnapshotDto source, string destinationPath)
    {
        // Snapshot fingerprints bind the project path plus the exact scene/item
        // state. An untitled Save As changes only that path.
        var main = RequireMainViewModel();
        var timeline = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        return Fingerprint(
            ReadItems(timeline),
            Path.GetFullPath(destinationPath),
            source.SceneId,
            source.Fps);
    }

    private object RequireActiveProjectInstance()
    {
        var main = RequireMainViewModel();
        // MainModel survives New/Open operations. Bind to the active domain
        // Timeline object beneath its view model: it is replaced on New/Open
        // (and on a scene switch), which distinguishes otherwise identical
        // untitled projects even if a view-model shell were reused.
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException(
                "YMM4 active project instance is unavailable");
        var timeline = GetField(timelineViewModel, "timeline")
            ?? throw new BridgeUnavailableException(
                "YMM4 active timeline instance is unavailable");
        if (timeline.GetType().FullName != "YukkuriMovieMaker.Project.Timeline"
            || timeline.GetType().Assembly.GetName().Name != "YukkuriMovieMaker")
        {
            throw new BridgeUnavailableException(
                "YMM4 active timeline type does not match the verified instance-binding profile");
        }
        return timeline;
    }

    private string CaptureProjectInstanceId(object instance)
    {
        lock (projectInstanceGate)
        {
            if (!ReferenceEquals(observedProjectInstance, instance))
            {
                observedProjectInstance = instance;
                observedProjectInstanceId = Guid.NewGuid().ToString("D");
            }
            return observedProjectInstanceId!;
        }
    }

    private void EnsureProjectInstanceId(string expected, object instance, string actualFingerprint)
    {
        lock (projectInstanceGate)
        {
            if (!ReferenceEquals(observedProjectInstance, instance)
                || !string.Equals(observedProjectInstanceId, expected, StringComparison.Ordinal))
            {
                throw new BridgeConflictException(
                    "The active YMM4 project instance changed after initialization preparation",
                    actualFingerprint);
            }
        }
    }

    private static void EnsureSaveAsRuntime()
    {
        var main = RequireMainViewModel();
        if (main.GetType().FullName != "YukkuriMovieMaker.ViewModels.MainViewModel"
            || main.GetType().Assembly.GetName().Name != "YukkuriMovieMaker")
        {
            throw new BridgeUnavailableException(
                "YMM4 MainViewModel type does not match the verified Save As profile");
        }
        _ = GetField(main, "model")
            ?? throw new BridgeUnavailableException(
                "YMM4 MainViewModel.model is unavailable for instance binding");
        var model = GetField(main, "model")!;
        if (model.GetType().FullName != "YukkuriMovieMaker.Project.MainModel"
            || model.GetType().Assembly.GetName().Name != "YukkuriMovieMaker")
        {
            throw new BridgeUnavailableException(
                "YMM4 MainModel type does not match the verified Save As profile");
        }
        var projectType = main.GetType().Assembly.GetType("YukkuriMovieMaker.Project.Project")
            ?? throw new BridgeUnavailableException("YMM4 Project save DTO is unavailable");
        if (projectType.GetConstructors(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .Count(constructor =>
            {
                var parameters = constructor.GetParameters();
                return parameters.Length == 5
                    && parameters[0].ParameterType == typeof(int)
                    && parameters[1].ParameterType.FullName == "YukkuriMovieMaker.Project.Scenes"
                    && parameters[2].ParameterType == typeof(string)
                    && parameters[3].ParameterType == typeof(string)
                    && parameters[4].ParameterType.IsGenericType
                    && parameters[4].ParameterType.GetGenericTypeDefinition() == typeof(Dictionary<,>)
                    && parameters[4].ParameterType.GetGenericArguments()[0] == typeof(string)
                    && parameters[4].ParameterType.GetGenericArguments()[1].FullName
                        == "YukkuriMovieMaker.Plugin.SerializableToolState";
            }) != 1)
        {
            throw new BridgeUnavailableException(
                "YMM4 Project save DTO constructor is unavailable or ambiguous");
        }
        var layoutService = GetField(model, "layoutService")
            ?? throw new BridgeUnavailableException("YMM4 layout service is unavailable");
        if (layoutService.GetType().FullName != "YukkuriMovieMaker.Views.Dock.LayoutService"
            || layoutService.GetType().GetMethods(BindingFlags.Public | BindingFlags.Instance)
                .Count(method =>
                    method.Name == "TryGetLayoutXml"
                    && method.ReturnType == typeof(bool)
                    && method.GetParameters() is
                    [{ ParameterType: var parameterType, IsOut: true }]
                    && parameterType == typeof(string).MakeByRefType()) != 1
            || layoutService.GetType().GetMethods(BindingFlags.Public | BindingFlags.Instance)
                .Count(method => method.Name == "GetToolStates"
                    && method.GetParameters().Length == 0
                    && method.ReturnType.IsGenericType
                    && method.ReturnType.GetGenericTypeDefinition() == typeof(Dictionary<,>)) != 1)
        {
            throw new BridgeUnavailableException(
                "YMM4 layout serialization contract is unavailable or ambiguous");
        }
        var jsonType = FindLoadedType("YukkuriMovieMaker.Json.Json");
        if (jsonType?.Assembly.GetName().Name != "YukkuriMovieMaker.Plugin"
            || jsonType.GetMethods(BindingFlags.Public | BindingFlags.Static)
                .Count(method =>
                {
                    if (method.Name != "Save" || !method.IsGenericMethodDefinition)
                    {
                        return false;
                    }
                    var parameters = method.GetParameters();
                    return method.ReturnType == typeof(void)
                        && parameters.Length == 3
                        && parameters[1].ParameterType == typeof(string)
                        && parameters[2].ParameterType.FullName
                            == "Newtonsoft.Json.JsonSerializerSettings";
                }) != 1)
        {
            throw new BridgeUnavailableException(
                "YMM4 Json.Save<Project> is unavailable or ambiguous");
        }
        if (model.GetType().GetMethods(
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .Count(method =>
                method.Name == "ChangeProjectPath"
                && method.IsPublic
                && !method.IsStatic
                && method.ReturnType == typeof(void)
                && method.GetParameters() is [{ ParameterType: var parameterType }]
                && parameterType == typeof(string)) != 1)
        {
            throw new BridgeUnavailableException(
                "YMM4 MainModel.ChangeProjectPath(string) is unavailable or ambiguous");
        }
        if (model.GetType().GetMethods(
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .Count(method =>
                method.Name == "set_IsProjectFileSaved"
                && !method.IsStatic
                && method.ReturnType == typeof(void)
                && method.GetParameters() is [{ ParameterType: var parameterType }]
                && parameterType == typeof(bool)) != 1)
        {
            throw new BridgeUnavailableException(
                "YMM4 MainModel.IsProjectFileSaved setter is unavailable or ambiguous");
        }
    }

    private static void ValidateProjectInitializationRequest(ProjectInitializationRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        if (request.OperationId == Guid.Empty
            || request.Overwrite
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.DriverProfileDigest)
            || string.IsNullOrWhiteSpace(request.SourceProjectInstanceId)
            || string.IsNullOrWhiteSpace(request.SourceProjectId)
            || string.IsNullOrWhiteSpace(request.SourceSceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedSourceFingerprint)
            || string.IsNullOrWhiteSpace(request.DestinationPath)
            || string.IsNullOrWhiteSpace(request.DestinationPathDigest)
            || string.IsNullOrWhiteSpace(request.PredictedProjectId)
            || string.IsNullOrWhiteSpace(request.PredictedFingerprint))
        {
            throw new BridgeValidationException(
                "Project initialization request has invalid binding fields");
        }
        if (!string.Equals(
                request.DriverProfileDigest,
                ProjectInitializationProfileDigest(),
                StringComparison.Ordinal)
            || !ApplyRequestDigest.Matches(
                request.RequestDigest,
                ApplyRequestDigest.Compute(request)))
        {
            throw new BridgeValidationException(
                "Project initialization profile or request digest is stale");
        }
    }

    private static void ValidateProjectInitializationPredictions(
        ProjectInitializationRequestDto request,
        ProjectSnapshotDto before,
        string destinationPath)
    {
        if (!PathsEqual(request.DestinationPath, destinationPath)
            || !string.Equals(request.DestinationPathDigest, ProjectPathDigest(destinationPath), StringComparison.Ordinal)
            || !string.Equals(request.PredictedProjectId, ProjectIdForPath(destinationPath), StringComparison.Ordinal)
            || !string.Equals(request.PredictedFingerprint, FingerprintForPath(before, destinationPath), StringComparison.Ordinal))
        {
            throw new BridgeValidationException(
                "Project initialization destination predictions do not match the active project");
        }
    }

    private static void EnsureProjectInitializationBinding(
        ProjectInitializationRequestDto request,
        ProjectInitializationReceiptDto receipt)
    {
        if (receipt.OperationId != request.OperationId
            || !string.Equals(receipt.RequestDigest, request.RequestDigest, StringComparison.Ordinal)
            || !string.Equals(receipt.DriverProfileDigest, request.DriverProfileDigest, StringComparison.Ordinal)
            || !string.Equals(receipt.SourceProjectInstanceId, request.SourceProjectInstanceId, StringComparison.Ordinal)
            || !string.Equals(receipt.SourceProjectId, request.SourceProjectId, StringComparison.Ordinal)
            || !string.Equals(receipt.SourceSceneId, request.SourceSceneId, StringComparison.Ordinal)
            || !string.Equals(receipt.BeforeFingerprint, request.ExpectedSourceFingerprint, StringComparison.Ordinal)
            || !string.Equals(receipt.DestinationPath, request.DestinationPath, StringComparison.Ordinal)
            || !string.Equals(receipt.DestinationPathDigest, request.DestinationPathDigest, StringComparison.Ordinal)
            || !string.Equals(receipt.PredictedProjectId, request.PredictedProjectId, StringComparison.Ordinal)
            || !string.Equals(receipt.PredictedFingerprint, request.PredictedFingerprint, StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Project initialization operation ID is already bound to another request",
                receipt.BeforeFingerprint);
        }
    }

    private static bool VerifyInitializedProject(
        ProjectInitializationRequestDto request,
        ProjectSnapshotDto after,
        string destinationPath)
    {
        return PathsEqual(after.ProjectPath, destinationPath)
            && string.Equals(after.ProjectId, request.PredictedProjectId, StringComparison.Ordinal)
            && string.Equals(after.SceneId, request.SourceSceneId, StringComparison.Ordinal)
            && string.Equals(after.Fingerprint, request.PredictedFingerprint, StringComparison.Ordinal)
            && File.Exists(destinationPath)
            && new FileInfo(destinationPath).Length > 0
            && IsProjectSaved();
    }

    internal static ProjectInitializationReceiptDto ReplayProjectInitialization(
        ProjectInitializationReceiptDto receipt)
    {
        // A verified record is a durable historical certificate. Re-reading
        // the live file here would make a lost HTTP response unreplayable if
        // the user subsequently edited, saved, moved, or deleted the file.
        return receipt.Status == "verified" ? receipt with { Status = "replayed" } : receipt;
    }

    private async Task<ProjectInitializationReceiptDto> ResumeProjectInitializationAsync(
        ProjectInitializationRequestDto request,
        ProjectInitializationReceiptDto receipt)
    {
        var working = receipt.Status == "applying"
            ? receipt
            : receipt with
            {
                Status = "applying",
                AfterSnapshot = null,
                FileSha256 = null,
                FileBytes = null,
                Error = null,
            };
        if (!ReferenceEquals(working, receipt))
        {
            projectOperationStore.PutProjectInitialization(working);
        }

        try
        {
            var destinationPath = ValidateProjectPath(working.DestinationPath);
            var current = Snapshot();
            if (HasCompletePreparedProjectInitializationEvidence(working)
                && IsInitializedProjectSnapshot(request, current, destinationPath)
                && ProjectInitializationFileMatches(
                    destinationPath,
                    working.PreparedFileSha256,
                    working.PreparedFileBytes)
                && IsProjectSaved())
            {
                return SealVerifiedProjectInitialization(working, current);
            }

            if (File.Exists(destinationPath))
            {
                if (!HasCompletePreparedProjectInitializationEvidence(working))
                {
                    return await SealNoWriteProjectInitializationFailureAsync(
                            request,
                            working,
                            destinationPath,
                            "Project initialization destination was claimed by another file")
                        .ConfigureAwait(false);
                }
                if (!ProjectInitializationFileMatches(
                        destinationPath,
                        working.PreparedFileSha256,
                        working.PreparedFileBytes))
                {
                    throw new BridgeUnavailableException(
                        "Existing project initialization destination does not match the durable prepared-file evidence");
                }
                await CommitProjectInitializationPathAsync(
                        request,
                        destinationPath,
                        working.PreparedFileSha256!,
                        working.PreparedFileBytes!.Value)
                    .ConfigureAwait(false);
            }
            else
            {
                _ = await ReadExactProjectInitializationSourceAsync(request, destinationPath)
                    .ConfigureAwait(false);
                if (working.PreparedTemporaryPath is null)
                {
                    working = working with
                    {
                        PreparedTemporaryPath = AllocateProjectInitializationTemporaryPath(
                            destinationPath),
                    };
                    // The operation owns this unpredictable name only after
                    // the path-only record is durable. No file is created first.
                    projectOperationStore.PutProjectInitialization(working);
                }

                if (!HasCompletePreparedProjectInitializationEvidence(working))
                {
                    await SerializeProjectInitializationTemporaryAsync(
                            request,
                            destinationPath,
                            working.PreparedTemporaryPath,
                            recreatePathOnly: true)
                        .ConfigureAwait(false);
                    BridgeFaultInjection.ThrowIf("after_project_initialization_temp_save");
                    var prepared = ReadProjectInitializationFileEvidence(
                        working.PreparedTemporaryPath);
                    working = working with
                    {
                        PreparedFileSha256 = prepared.Sha256,
                        PreparedFileBytes = prepared.Bytes,
                    };
                    // This receipt must reach stable storage before the
                    // no-overwrite namespace claim. It is the only authority
                    // for recognizing a claimed destination after a crash.
                    projectOperationStore.PutProjectInitialization(working);
                    BridgeFaultInjection.ThrowIf("after_project_initialization_evidence");
                }
                else if (!File.Exists(working.PreparedTemporaryPath))
                {
                    // A complete receipt with neither final nor temporary file
                    // can only be retried from the still-exact source. The
                    // recreated bytes must match the immutable receipt.
                    await SerializeProjectInitializationTemporaryAsync(
                            request,
                            destinationPath,
                            working.PreparedTemporaryPath,
                            recreatePathOnly: false)
                        .ConfigureAwait(false);
                }

                if (!ProjectInitializationFileMatches(
                        working.PreparedTemporaryPath,
                        working.PreparedFileSha256,
                        working.PreparedFileBytes))
                {
                    throw new BridgeUnavailableException(
                        "Prepared project file does not match its durable hash and byte count");
                }
                ClaimPreparedProjectInitializationPath(
                    working.PreparedTemporaryPath,
                    destinationPath,
                    working.PreparedFileSha256!,
                    working.PreparedFileBytes!.Value);
                BridgeFaultInjection.ThrowIf("after_project_initialization_claim");
                await CommitProjectInitializationPathAsync(
                        request,
                        destinationPath,
                        working.PreparedFileSha256!,
                        working.PreparedFileBytes!.Value)
                    .ConfigureAwait(false);
            }

            BridgeFaultInjection.ThrowIf("after_project_initialization_rebind");
            var after = Snapshot();
            if (!VerifyInitializedProject(request, after, destinationPath)
                || !ProjectInitializationFileMatches(
                    destinationPath,
                    working.PreparedFileSha256,
                    working.PreparedFileBytes))
            {
                throw new BridgeUnavailableException(
                    "YMM4 project initialization read-back did not match the exact prepared destination");
            }
            return SealVerifiedProjectInitialization(working, after);
        }
        catch (BridgeSimulatedCrashException)
        {
            throw;
        }
        catch (Exception error)
        {
            var after = TrySnapshot();
            var finalEvidence = TryReadProjectInitializationFileEvidence(working.DestinationPath);
            var recoveryRequired = working with
            {
                Status = "recovery_required",
                AfterSnapshot = after,
                FileSha256 = finalEvidence?.Sha256,
                FileBytes = finalEvidence?.Bytes,
                Error = error.GetBaseException().Message,
            };
            projectOperationStore.PutProjectInitialization(recoveryRequired);
            return recoveryRequired;
        }
    }

    private async Task<ProjectSnapshotDto> ReadExactProjectInitializationSourceAsync(
        ProjectInitializationRequestDto request,
        string destinationPath)
    {
        return await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            var source = SnapshotCore();
            EnsureExactProjectInitializationSource(request, source, destinationPath);
            return source;
        });
    }

    private void EnsureExactProjectInitializationSource(
        ProjectInitializationRequestDto request,
        ProjectSnapshotDto source,
        string destinationPath)
    {
        EnsureTarget(request.SourceProjectId, request.SourceSceneId, source);
        EnsureFingerprint(request.ExpectedSourceFingerprint, source.Fingerprint);
        if (!string.IsNullOrWhiteSpace(source.ProjectPath))
        {
            throw new BridgeConflictException(
                "The active YMM4 project was already assigned a path",
                source.Fingerprint);
        }
        if (source.ManagedItems.Count != 0 || source.NativeExtensions.Count != 0)
        {
            throw new BridgeConflictException(
                "Untitled project initialization cannot rebind existing TakeGraph-managed identities",
                source.Fingerprint);
        }
        ValidateProjectInitializationPredictions(request, source, destinationPath);
        EnsureProjectInstanceId(
            request.SourceProjectInstanceId,
            RequireActiveProjectInstance(),
            source.Fingerprint);
    }

    private async Task<ProjectInitializationReceiptDto> SealNoWriteProjectInitializationFailureAsync(
        ProjectInitializationRequestDto request,
        ProjectInitializationReceiptDto receipt,
        string destinationPath,
        string error)
    {
        if (HasCompletePreparedProjectInitializationEvidence(receipt))
        {
            throw new BridgeUnavailableException(
                "A complete prepared-file receipt cannot be downgraded to a no-write result");
        }
        var source = await ReadExactProjectInitializationSourceAsync(request, destinationPath)
            .ConfigureAwait(false);
        if (receipt.PreparedTemporaryPath is not null)
        {
            if (!IsOwnedProjectInitializationTemporaryPath(
                    destinationPath,
                    receipt.PreparedTemporaryPath))
            {
                throw new BridgeUnavailableException(
                    "Project initialization temporary path is not operation-owned");
            }
            if (Directory.Exists(receipt.PreparedTemporaryPath))
            {
                throw new BridgeUnavailableException(
                    "Project initialization temporary path was replaced by a directory");
            }
            if (File.Exists(receipt.PreparedTemporaryPath))
            {
                File.Delete(receipt.PreparedTemporaryPath);
            }
            if (File.Exists(receipt.PreparedTemporaryPath))
            {
                throw new BridgeUnavailableException(
                    "Project initialization temporary file could not be cleaned");
            }
        }
        var failed = receipt with
        {
            Status = "failed",
            PreparedTemporaryPath = null,
            PreparedFileSha256 = null,
            PreparedFileBytes = null,
            AfterSnapshot = source,
            FileSha256 = null,
            FileBytes = null,
            Error = error,
        };
        projectOperationStore.PutProjectInitialization(failed);
        return failed;
    }

    private async Task SerializeProjectInitializationTemporaryAsync(
        ProjectInitializationRequestDto request,
        string destinationPath,
        string temporaryPath,
        bool recreatePathOnly)
    {
        if (!IsOwnedProjectInitializationTemporaryPath(destinationPath, temporaryPath))
        {
            throw new BridgeUnavailableException(
                "Project initialization temporary path is not operation-owned");
        }
        if (Directory.Exists(temporaryPath))
        {
            throw new BridgeUnavailableException(
                "Project initialization temporary path was replaced by a directory");
        }
        if (recreatePathOnly && File.Exists(temporaryPath))
        {
            File.Delete(temporaryPath);
        }
        await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            var source = SnapshotCore();
            EnsureExactProjectInitializationSource(request, source, destinationPath);
            EnsureSaveAsRuntime();
            SaveProjectDtoToTemporary(destinationPath, temporaryPath);
        });
        using var stream = new FileStream(
            temporaryPath,
            FileMode.Open,
            FileAccess.ReadWrite,
            FileShare.Read,
            4096,
            FileOptions.WriteThrough);
        stream.Flush(flushToDisk: true);
    }

    private async Task CommitProjectInitializationPathAsync(
        ProjectInitializationRequestDto request,
        string destinationPath,
        string expectedSha256,
        ulong expectedBytes)
    {
        await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            // Keep the exact destination open without write/delete sharing
            // through the YMM path mutation. This closes the evidence-check
            // versus rebind race for ordinary filesystem participants.
            using var destination = new FileStream(
                destinationPath,
                FileMode.Open,
                FileAccess.Read,
                FileShare.Read,
                4096,
                FileOptions.SequentialScan);
            var actualBytes = checked((ulong)destination.Length);
            var actualSha256 = Convert.ToHexStringLower(SHA256.HashData(destination));
            if (actualBytes != expectedBytes
                || !ApplyRequestDigest.Matches(actualSha256, expectedSha256))
            {
                throw new BridgeUnavailableException(
                    "Project initialization destination changed before YMM path rebind");
            }
            var source = SnapshotCore();
            EnsureExactProjectInitializationSource(request, source, destinationPath);
            ChangeActiveProjectPathCore(destinationPath, saved: true);
        });
    }

    private ProjectInitializationReceiptDto SealVerifiedProjectInitialization(
        ProjectInitializationReceiptDto receipt,
        ProjectSnapshotDto after)
    {
        if (!HasCompletePreparedProjectInitializationEvidence(receipt))
        {
            throw new BridgeUnavailableException(
                "Project initialization cannot verify without durable prepared-file evidence");
        }
        using var destination = new FileStream(
            receipt.DestinationPath,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read,
            4096,
            FileOptions.SequentialScan);
        var actualBytes = checked((ulong)destination.Length);
        var actualSha256 = Convert.ToHexStringLower(SHA256.HashData(destination));
        if (actualBytes != receipt.PreparedFileBytes
            || !ApplyRequestDigest.Matches(actualSha256, receipt.PreparedFileSha256))
        {
            throw new BridgeUnavailableException(
                "Project initialization destination changed before receipt verification");
        }
        var verified = receipt with
        {
            Status = "verified",
            AfterSnapshot = after,
            FileSha256 = receipt.PreparedFileSha256,
            FileBytes = receipt.PreparedFileBytes,
            Error = null,
        };
        projectOperationStore.PutProjectInitialization(verified);
        BridgeFaultInjection.ThrowIf("after_project_initialization_receipt");
        return verified;
    }

    private static bool IsInitializedProjectSnapshot(
        ProjectInitializationRequestDto request,
        ProjectSnapshotDto snapshot,
        string destinationPath) =>
        PathsEqual(snapshot.ProjectPath, destinationPath)
        && string.Equals(snapshot.ProjectId, request.PredictedProjectId, StringComparison.Ordinal)
        && string.Equals(snapshot.SceneId, request.SourceSceneId, StringComparison.Ordinal)
        && string.Equals(snapshot.Fingerprint, request.PredictedFingerprint, StringComparison.Ordinal);

    private static bool HasCompletePreparedProjectInitializationEvidence(
        ProjectInitializationReceiptDto receipt) =>
        receipt.PreparedTemporaryPath is not null
        && receipt.PreparedFileSha256 is not null
        && receipt.PreparedFileBytes is > 0;

    private sealed record ProjectInitializationFileEvidence(string Sha256, ulong Bytes);

    private static ProjectInitializationFileEvidence ReadProjectInitializationFileEvidence(
        string path)
    {
        using var stream = new FileStream(
            path,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read,
            4096,
            FileOptions.SequentialScan);
        var bytes = checked((ulong)stream.Length);
        var sha256 = Convert.ToHexStringLower(SHA256.HashData(stream));
        if (bytes == 0)
        {
            throw new BridgeUnavailableException(
                "Project initialization file is empty");
        }
        return new ProjectInitializationFileEvidence(sha256, bytes);
    }

    private static ProjectInitializationFileEvidence? TryReadProjectInitializationFileEvidence(
        string path)
    {
        try
        {
            return ReadProjectInitializationFileEvidence(path);
        }
        catch
        {
            return null;
        }
    }

    internal static bool ProjectInitializationFileMatches(
        string path,
        string? expectedSha256,
        ulong? expectedBytes)
    {
        if (expectedSha256 is null || expectedBytes is not > 0)
        {
            return false;
        }
        var evidence = TryReadProjectInitializationFileEvidence(path);
        return evidence is not null
            && evidence.Bytes == expectedBytes
            && ApplyRequestDigest.Matches(evidence.Sha256, expectedSha256);
    }

    private static string AllocateProjectInitializationTemporaryPath(string destinationPath) =>
        Path.Combine(
            Path.GetDirectoryName(destinationPath)!,
            $".takegraph-project-{Guid.NewGuid():N}.ymmp");

    private static void ValidateCheckpointRequest(CheckpointRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        if (request.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.TargetIdentityDigest)
            || string.IsNullOrWhiteSpace(request.ExpectedStateDigest)
            || !string.Equals(
                request.CheckpointProfileDigest,
                CheckpointProfileDigest(),
                StringComparison.Ordinal))
        {
            throw new BridgeValidationException("Checkpoint request has invalid binding or profile fields");
        }
        if (!ApplyRequestDigest.Matches(request.RequestDigest, ApplyRequestDigest.Compute(request)))
        {
            throw new BridgeValidationException("Checkpoint request digest does not match its payload");
        }
    }

    private static void EnsureCheckpointBinding(
        CheckpointRequestDto request,
        CheckpointReceiptDto receipt)
    {
        if (receipt.OperationId != request.OperationId
            || !string.Equals(receipt.RequestDigest, request.RequestDigest, StringComparison.Ordinal)
            || !string.Equals(receipt.ProjectId, request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(receipt.SceneId, request.SceneId, StringComparison.Ordinal)
            || receipt.SourceRevision != request.SourceRevision
            || !string.Equals(
                receipt.TargetIdentityDigest,
                request.TargetIdentityDigest,
                StringComparison.Ordinal)
            || !string.Equals(
                receipt.ExpectedStateDigest,
                request.ExpectedStateDigest,
                StringComparison.Ordinal)
            || !string.Equals(
                receipt.CheckpointProfileDigest,
                request.CheckpointProfileDigest,
                StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Checkpoint operation ID is already bound to another request",
                receipt.BeforeStateDigest);
        }
    }

    private static CheckpointReceiptDto RecoverCheckpoint(
        CheckpointRequestDto request,
        CheckpointReceiptDto receipt,
        ProjectSnapshotDto snapshot)
    {
        var projectPath = Path.GetFullPath(receipt.ProjectPath);
        var verified = string.Equals(snapshot.Fingerprint, request.ExpectedStateDigest, StringComparison.Ordinal)
            && File.Exists(projectPath)
            && new FileInfo(projectPath).Length > 0
            && IsProjectSaved();
        return receipt with
        {
            Status = verified ? "verified" : "recovery_required",
            PostFileSha256 = File.Exists(projectPath) ? HashFile(projectPath) : null,
            PostFileBytes = File.Exists(projectPath)
                ? checked((ulong)new FileInfo(projectPath).Length)
                : null,
            AfterStateDigest = snapshot.Fingerprint,
            Error = verified
                ? null
                : "Interrupted checkpoint could not prove a complete save of the expected state",
        };
    }

    private static CheckpointReceiptDto ValidateCheckpointArtifact(CheckpointReceiptDto receipt)
    {
        if (receipt.Status != "verified")
        {
            return receipt;
        }
        if (!File.Exists(receipt.ProjectPath)
            || receipt.PostFileSha256 is null
            || receipt.PostFileBytes is null
            || checked((ulong)new FileInfo(receipt.ProjectPath).Length) != receipt.PostFileBytes
            || !ApplyRequestDigest.Matches(HashFile(receipt.ProjectPath), receipt.PostFileSha256))
        {
            throw new BridgeUnavailableException(
                "Verified checkpoint file no longer matches its durable receipt");
        }
        return receipt;
    }

    private static bool IsProjectSaved()
    {
        return Application.Current.Dispatcher.Invoke(() => !ReadProjectDirty());
    }

    private static string HashFile(string path)
    {
        using var stream = File.OpenRead(path);
        return Convert.ToHexStringLower(SHA256.HashData(stream));
    }

    private static async Task<(string Scope, string ProjectPath)> InvokeSaveProjectAsync()
    {
        var invocation = await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            var main = RequireMainViewModel();
            var projectPath = GetString(main, "ProjectFilePath", "ProjectPath");
            if (string.IsNullOrWhiteSpace(projectPath))
            {
                throw new BridgeUnavailableException(
                    "The active YMM4 project has no existing path; use Save As in YMM4 first");
            }
            var fullPath = Path.GetFullPath(projectPath);
            foreach (var scope in ControlScopes())
            {
                var method = scope.Value.GetType().GetMethods(
                        BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                    .FirstOrDefault(value =>
                        value.Name == "SaveProject"
                        && value.GetParameters() is [{ ParameterType: var parameterType }]
                        && parameterType == typeof(string));
                if (method is null)
                {
                    continue;
                }
                try
                {
                    return (
                        Result: method.Invoke(scope.Value, [fullPath]),
                        Scope: scope.Key,
                        ProjectPath: fullPath);
                }
                catch (TargetInvocationException error)
                {
                    throw error.InnerException ?? error;
                }
            }
            throw new BridgeUnavailableException("YMM4 SaveProject(string) is unavailable");
        });
        if (invocation.Result is Task task)
        {
            await task.ConfigureAwait(false);
        }
        return (invocation.Scope, invocation.ProjectPath);
    }

    private static void SaveProjectDtoToTemporary(string destinationPath, string temporaryPath)
    {
        var main = RequireMainViewModel();
        var model = GetField(main, "model")
            ?? throw new BridgeUnavailableException("YMM4 MainViewModel.model is unavailable");
        var scenes = GetMember(model, "Scenes")
            ?? throw new BridgeUnavailableException("YMM4 project scenes are unavailable");
        var timelines = GetMember(scenes, "Timelines") as IEnumerable
            ?? throw new BridgeUnavailableException("YMM4 project timelines are unavailable");
        var currentTimeline = GetMember(model, "Timeline")
            ?? throw new BridgeUnavailableException("YMM4 current project timeline is unavailable");
        var timelineValues = timelines.Cast<object>().ToArray();
        var selectedIndex = Array.FindIndex(
            timelineValues,
            candidate => ReferenceEquals(candidate, currentTimeline) || Equals(candidate, currentTimeline));
        if (selectedIndex < 0)
        {
            throw new BridgeUnavailableException(
                "YMM4 current timeline is absent from the project scene list");
        }

        var layoutService = GetField(model, "layoutService")
            ?? throw new BridgeUnavailableException("YMM4 layout service is unavailable");
        var tryGetLayout = layoutService.GetType().GetMethods(BindingFlags.Public | BindingFlags.Instance)
            .Single(method =>
                method.Name == "TryGetLayoutXml"
                && method.ReturnType == typeof(bool)
                && method.GetParameters() is
                [{ ParameterType: var parameterType, IsOut: true }]
                && parameterType == typeof(string).MakeByRefType());
        var layoutArguments = new object?[] { null };
        var hasLayout = (bool)(tryGetLayout.Invoke(layoutService, layoutArguments) ?? false);
        var layoutXml = hasLayout ? layoutArguments[0] as string : null;
        var getToolStates = layoutService.GetType()
            .GetMethods(BindingFlags.Public | BindingFlags.Instance)
            .Single(method => method.Name == "GetToolStates"
                && method.GetParameters().Length == 0
                && method.ReturnType.IsGenericType
                && method.ReturnType.GetGenericTypeDefinition() == typeof(Dictionary<,>));
        var toolStates = getToolStates.Invoke(layoutService, null)
            ?? throw new BridgeUnavailableException("YMM4 tool-state snapshot is unavailable");

        var projectType = main.GetType().Assembly.GetType("YukkuriMovieMaker.Project.Project")!;
        var constructor = projectType.GetConstructors(
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .Single(candidate =>
            {
                var parameters = candidate.GetParameters();
                return parameters.Length == 5
                    && parameters[0].ParameterType == typeof(int)
                    && parameters[1].ParameterType == scenes.GetType()
                    && parameters[2].ParameterType == typeof(string)
                    && parameters[3].ParameterType == typeof(string)
                    && parameters[4].ParameterType.IsInstanceOfType(toolStates);
            });
        object project;
        try
        {
            project = constructor.Invoke(
                [selectedIndex, scenes, destinationPath, layoutXml, toolStates]);
        }
        catch (TargetInvocationException error)
        {
            throw new BridgeUnavailableException(
                $"YMM4 project save DTO construction failed: {(error.InnerException ?? error).Message}");
        }

        var jsonType = FindLoadedType("YukkuriMovieMaker.Json.Json")!;
        var save = jsonType.GetMethods(BindingFlags.Public | BindingFlags.Static)
            .Single(method =>
            {
                var parameters = method.GetParameters();
                return method.Name == "Save"
                    && method.IsGenericMethodDefinition
                    && method.ReturnType == typeof(void)
                    && parameters.Length == 3
                    && parameters[1].ParameterType == typeof(string)
                    && parameters[2].ParameterType.FullName
                        == "Newtonsoft.Json.JsonSerializerSettings";
            });
        try
        {
            save.MakeGenericMethod(projectType).Invoke(null, [project, temporaryPath, null]);
        }
        catch (TargetInvocationException error)
        {
            throw new BridgeUnavailableException(
                $"YMM4 project serialization failed: {(error.InnerException ?? error).Message}");
        }
    }

    private static async Task ChangeActiveProjectPathAsync(string path, bool saved)
    {
        await Application.Current.Dispatcher.InvokeAsync(() =>
            ChangeActiveProjectPathCore(path, saved));
    }

    private static void ChangeActiveProjectPathCore(string path, bool saved)
    {
        var main = RequireMainViewModel();
        var model = GetField(main, "model")
            ?? throw new BridgeUnavailableException("YMM4 MainViewModel.model is unavailable");
        var changePath = model.GetType().GetMethods(
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .Single(candidate =>
                candidate.Name == "ChangeProjectPath"
                && candidate.ReturnType == typeof(void)
                && candidate.GetParameters() is [{ ParameterType: var parameterType }]
                && parameterType == typeof(string));
        try
        {
            changePath.Invoke(model, [path]);
            if (saved)
            {
                var setSaved = model.GetType().GetMethods(
                        BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                    .Single(candidate =>
                        candidate.Name == "set_IsProjectFileSaved"
                        && !candidate.IsStatic
                        && candidate.ReturnType == typeof(void)
                        && candidate.GetParameters() is [{ ParameterType: var parameterType }]
                        && parameterType == typeof(bool));
                setSaved.Invoke(model, [true]);
            }
        }
        catch (TargetInvocationException error)
        {
            throw error.InnerException ?? error;
        }
    }

    internal static bool IsOwnedProjectInitializationTemporaryPath(
        string destination,
        string temporary)
    {
        try
        {
            var destinationFull = Path.GetFullPath(destination);
            var temporaryFull = Path.GetFullPath(temporary);
            if (!Path.IsPathFullyQualified(destination)
                || !Path.IsPathFullyQualified(temporary)
                || !string.Equals(temporary, temporaryFull, StringComparison.OrdinalIgnoreCase)
                || PathsEqual(destinationFull, temporaryFull)
                || !string.Equals(
                    Path.GetDirectoryName(destinationFull),
                    Path.GetDirectoryName(temporaryFull),
                    StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }
            var name = Path.GetFileName(temporaryFull);
            const string prefix = ".takegraph-project-";
            const string suffix = ".ymmp";
            if (!name.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)
                || !name.EndsWith(suffix, StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }
            var token = name[prefix.Length..^suffix.Length];
            return token.Length == 32 && token.All(Uri.IsHexDigit);
        }
        catch
        {
            return false;
        }
    }

    internal static void ClaimPreparedProjectInitializationPath(
        string temporary,
        string destination,
        string expectedSha256,
        ulong expectedBytes)
    {
        if (!IsOwnedProjectInitializationTemporaryPath(destination, temporary))
        {
            throw new InvalidDataException(
                "Prepared project initialization file does not match its durable evidence");
        }
        // Deny writers while checking and atomically moving the exact file.
        // Delete sharing is required for the rename while the handle remains
        // open; the subsequent path commit independently locks the final path.
        using var prepared = new FileStream(
            temporary,
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read | FileShare.Delete,
            4096,
            FileOptions.SequentialScan);
        var actualBytes = checked((ulong)prepared.Length);
        var actualSha256 = Convert.ToHexStringLower(SHA256.HashData(prepared));
        if (actualBytes != expectedBytes
            || !ApplyRequestDigest.Matches(actualSha256, expectedSha256))
        {
            throw new InvalidDataException(
                "Prepared project initialization file does not match its durable evidence");
        }
        File.Move(temporary, destination, overwrite: false);
        if (!ProjectInitializationFileMatches(destination, expectedSha256, expectedBytes))
        {
            throw new BridgeUnavailableException(
                "Claimed project initialization file does not match its durable evidence");
        }
    }

    internal RenderProfilesDto RenderProfiles()
    {
        ValidateMutationRuntime();
        return Application.Current.Dispatcher.Invoke(ReadRenderProfilesCore);
    }

    internal async Task<RenderTaskDto> StartRenderAsync(RenderRequestDto request)
    {
        ValidateRenderRequestShape(request);
        RequireAuthoritativeRenderSourceBinding();
        await applyGate.WaitAsync().ConfigureAwait(false);
        var releaseGate = true;
        try
        {
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            RecoverInterruptedRenderTasks();
            if (projectOperationStore.TryGetRender(request.TaskId, out var existing)
                && existing is not null)
            {
                EnsureRenderBinding(request, existing);
                return ValidateRenderedArtifact(existing);
            }

            var before = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, before);
            EnsureFingerprint(request.ExpectedStateDigest, before.Fingerprint);
            var boundProfile = Application.Current.Dispatcher.Invoke(ReadBoundRenderProfileCore);
            var profile = boundProfile.Profile;
            if (!string.Equals(
                    profile.ProfileDigest,
                    request.RenderProfileDigest,
                    StringComparison.Ordinal))
            {
                throw new BridgeValidationException(
                    "The requested render profile is unavailable or stale");
            }
            var checkpoint = RequireVerifiedRenderCheckpoint(request, before);

            var outputPath = Path.GetFullPath(request.OutputPath);
            if (!PathsEqual(outputPath, request.OutputPath))
            {
                throw new BridgeValidationException("Render outputPath must already be a canonical absolute path");
            }
            if (Directory.Exists(outputPath))
            {
                throw new BridgeValidationException("Render outputPath points to a directory");
            }
            var outputDirectory = Path.GetDirectoryName(outputPath)
                ?? throw new BridgeValidationException("Render outputPath has no parent directory");
            Directory.CreateDirectory(outputDirectory);
            RenderPathNamespaceLease.RequireDirectoryPath(outputDirectory);
            RenderPathNamespaceLease.RequireRegularFilePath(outputPath, mustExist: false);
            if (File.Exists(outputPath) && request.OverwritePolicy == "deny")
            {
                throw new BridgeConflictException("Render output already exists", before.Fingerprint);
            }

            var backupPath = $"{outputPath}.takegraph-backup-{request.TaskId:N}";
            var quarantinePath = $"{outputPath}.takegraph-stale-{request.TaskId:N}";
            var candidateDirectory = Path.Combine(
                outputDirectory,
                $".takegraph-render-{request.TaskId:N}");
            if (Directory.Exists(candidateDirectory) || File.Exists(candidateDirectory))
            {
                throw new BridgeConflictException(
                    "Task-private render candidate directory already exists",
                    before.Fingerprint);
            }
            Directory.CreateDirectory(candidateDirectory);
            RenderPathNamespaceLease.RequireDirectoryPath(candidateDirectory);
            var candidatePath = Path.Combine(candidateDirectory, "candidate.mp4");
            foreach (var ownedPath in new[] { backupPath, quarantinePath, candidatePath })
            {
                RenderPathNamespaceLease.RequireRegularFilePath(ownedPath, mustExist: false);
            }
            if (File.Exists(backupPath) || File.Exists(quarantinePath) || File.Exists(candidatePath))
            {
                throw new BridgeConflictException(
                    "Render recovery paths already exist; refusing to overwrite recovery evidence",
                    before.Fingerprint);
            }
            using var stageNamespaceLease = RenderPathNamespaceLease.Open(
                File.Exists(outputPath) ? [outputPath] : [],
                [outputDirectory]);
            var originalExisted = File.Exists(outputPath);
            var originalEvidence = originalExisted ? ReadFileEvidence(outputPath) : default;
            var overwriteJournal = new RenderOverwriteJournalDto(
                "prepared",
                originalExisted,
                originalExisted ? originalEvidence.Sha256 : null,
                originalExisted ? originalEvidence.ByteLength : null,
                backupPath,
                quarantinePath,
                candidateDirectory,
                candidatePath,
                null,
                null);
            var control = new RenderExecutionControl();
            var candidateNamespaceLease = RenderPathNamespaceLease.Open(
                [],
                [outputDirectory, candidateDirectory]);
            if (!control.TryAttachCandidateNamespaceLease(candidateNamespaceLease))
            {
                candidateNamespaceLease.Dispose();
                control.Dispose();
                throw new BridgeConflictException(
                    "Could not lease the task-private render namespace",
                    before.Fingerprint);
            }
            if (!renderExecutions.TryAdd(request.TaskId, control))
            {
                control.Dispose();
                throw new BridgeConflictException(
                    "A render execution is already bound to this task",
                    before.Fingerprint);
            }
            string encodeSourcePath;
            try
            {
                encodeSourcePath = CreateImmutableCheckpointSnapshot(request, checkpoint);
            }
            catch
            {
                renderExecutions.TryRemove(request.TaskId, out _);
                control.Dispose();
                throw;
            }

            var queued = new RenderTaskDto(
                request.TaskId,
                request.RequestDigest,
                request.ProjectId,
                request.SceneId,
                request.SourceRevision,
                request.TargetIdentityDigest,
                request.ExpectedStateDigest,
                request.CheckpointOperationId,
                request.CheckpointRequestDigest,
                request.CheckpointProjectPath,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                request.RenderProfileDigest,
                request.OutputPath,
                request.OverwritePolicy,
                encodeSourcePath,
                overwriteJournal,
                "queued",
                0,
                "queued",
                true,
                before.Fingerprint,
                null,
                null,
                null);
            try
            {
                projectOperationStore.PutRender(queued);
                _ = Task.Run(() => RunRenderAsync(
                    request,
                    queued,
                    profile,
                    boundProfile.Binding,
                    control));
            }
            catch
            {
                renderExecutions.TryRemove(request.TaskId, out _);
                control.Dispose();
                throw;
            }
            releaseGate = false;
            return queued;
        }
        finally
        {
            if (releaseGate)
            {
                applyGate.Release();
            }
        }
    }

    internal RenderTaskDto GetRender(Guid taskId)
    {
        RecoverInterruptedRenderTasks();
        if (!projectOperationStore.TryGetRender(taskId, out var task) || task is null)
        {
            throw new BridgeNotFoundException($"Render task not found: {taskId}");
        }
        return ValidateRenderedArtifact(task);
    }

    internal RenderTaskDto CancelRender(RenderCancelRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        if (request.TaskId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedStateDigest)
            || request.CheckpointOperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.CheckpointRequestDigest)
            || string.IsNullOrWhiteSpace(request.CheckpointFileSha256)
            || string.IsNullOrWhiteSpace(request.RenderProfileDigest))
        {
            throw new BridgeValidationException("Render cancellation has invalid binding fields");
        }
        if (!projectOperationStore.TryGetRender(request.TaskId, out var task) || task is null)
        {
            throw new BridgeNotFoundException($"Render task not found: {request.TaskId}");
        }
        EnsureRenderCancelBinding(request, task);
        if (task.Status is "cancelled" or "succeeded" or "failed" or "stale" or "recovery_required")
        {
            return ValidateRenderedArtifact(task);
        }

        if (!renderExecutions.TryGetValue(request.TaskId, out var control))
        {
            var recovery = task with
            {
                Status = "recovery_required",
                Phase = "recovery_required",
                Error = "The render process is no longer attached to this bridge instance",
            };
            projectOperationStore.PutRender(recovery);
            return recovery;
        }
        control.TryCancel(() =>
        {
            if (!projectOperationStore.TryGetRender(request.TaskId, out var current)
                || current is null
                || current.Status is "cancelled" or "succeeded" or "failed" or "stale" or "recovery_required")
            {
                return false;
            }
            projectOperationStore.PutRender(current with
            {
                Status = "cancelling",
                Phase = "cancelling",
                Cancellable = false,
            });
            return true;
        });
        return projectOperationStore.TryGetRender(request.TaskId, out var latest)
            && latest is not null
            ? ValidateRenderedArtifact(latest)
            : throw new BridgeNotFoundException($"Render task not found: {request.TaskId}");
    }

    private async Task RunRenderAsync(
        RenderRequestDto request,
        RenderTaskDto queued,
        RenderProfileDescriptorDto profile,
        RenderRuntimeBinding approvedBinding,
        RenderExecutionControl control)
    {
        var outputPath = Path.GetFullPath(request.OutputPath);
        Process? process = null;
        try
        {
            control.Token.ThrowIfCancellationRequested();
            var reboundProfile = Application.Current.Dispatcher.Invoke(ReadBoundRenderProfileCore);
            EnsureRenderRuntimeBinding(
                request.RenderProfileDigest,
                profile,
                approvedBinding,
                reboundProfile);
            using var renderBindingLease = reboundProfile.Binding.OpenVerifiedReadLease();
            var leasedProfile = Application.Current.Dispatcher.Invoke(ReadBoundRenderProfileCore);
            EnsureRenderRuntimeBinding(
                request.RenderProfileDigest,
                profile,
                approvedBinding,
                leasedProfile);
            renderBindingLease.Verify();
            using var checkpointLease = OpenVerifiedReadLease(
                request.CheckpointProjectPath,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                "The verified checkpoint changed before rendering started");
            using var encodeSourceLease = OpenVerifiedReadLease(
                queued.EncodeSourcePath,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                "The immutable checkpoint snapshot changed before rendering started");
            var directory = Path.GetDirectoryName(outputPath)
                ?? throw new BridgeValidationException("Render output has no parent directory");
            var candidatePath = queued.OverwriteJournal.CandidatePath;
            using var renderNamespaceLease = RenderPathNamespaceLease.Open(
                [request.CheckpointProjectPath, queued.EncodeSourcePath],
                [directory]);
            control.VerifyCandidateNamespaceLease();
            renderNamespaceLease.Verify();
            control.Token.ThrowIfCancellationRequested();
            if (File.Exists(candidatePath))
            {
                throw new IOException("The task-owned render candidate appeared after staging");
            }

            var running = queued with
            {
                Status = "running",
                ProgressBasisPoints = 100,
                Phase = "encoding",
                Cancellable = true,
                OverwriteJournal = queued.OverwriteJournal with { State = "encoding_candidate" },
            };
            if (!control.TryAdvance(() => projectOperationStore.PutRender(running)))
            {
                throw new OperationCanceledException(control.Token);
            }
            var executable = Environment.ProcessPath;
            if (string.IsNullOrWhiteSpace(executable) || !File.Exists(executable))
            {
                throw new BridgeUnavailableException("The YMM4 executable path is unavailable");
            }
            var startInfo = new ProcessStartInfo
            {
                FileName = executable,
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
            };
            startInfo.ArgumentList.Add("--encode");
            startInfo.ArgumentList.Add(queued.EncodeSourcePath);
            startInfo.ArgumentList.Add("--output");
            startInfo.ArgumentList.Add(candidatePath);
            process = new Process { StartInfo = startInfo, EnableRaisingEvents = true };
            if (!control.TryAttach(process))
            {
                throw new OperationCanceledException(control.Token);
            }
            if (!process.Start())
            {
                throw new BridgeUnavailableException("YMM4 encoder process did not start");
            }
            if (control.Token.IsCancellationRequested)
            {
                process.Kill(entireProcessTree: true);
            }
            var stdoutTask = process.StandardOutput.ReadToEndAsync();
            var stderrTask = process.StandardError.ReadToEndAsync();
            await process.WaitForExitAsync().ConfigureAwait(false);
            var stdout = await stdoutTask.ConfigureAwait(false);
            var stderr = await stderrTask.ConfigureAwait(false);

            if (control.Token.IsCancellationRequested)
            {
                throw new OperationCanceledException(control.Token);
            }
            if (process.ExitCode != 0)
            {
                throw new InvalidOperationException(
                    $"YMM4 encoder exited with code {process.ExitCode}: {TrimProcessOutput(stderr, stdout)}");
            }
            if (!File.Exists(candidatePath))
            {
                throw new InvalidDataException("YMM4 encoder reported success but produced no output file");
            }

            renderBindingLease.Verify();
            renderNamespaceLease.Verify();
            control.VerifyCandidateNamespaceLease();

            RequireExactFileEvidence(
                checkpointLease,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                "The verified checkpoint changed while rendering");
            RequireExactFileEvidence(
                encodeSourceLease,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                "The immutable checkpoint snapshot changed while rendering");

            var media = Mp4MediaProbe.Probe(candidatePath) with { OutputPath = request.OutputPath };
            if (media.Width != profile.Width
                || media.Height != profile.Height
                || media.VideoStreams != 1
                || media.FpsNumerator != profile.FpsNumerator
                || media.FpsDenominator != profile.FpsDenominator
                || !string.Equals(media.VideoCodec, profile.VideoCodec, StringComparison.Ordinal)
                || !string.Equals(media.PixelFormat, profile.PixelFormat, StringComparison.Ordinal)
                || (profile.HasAudio
                    && (media.AudioStreams != 1
                        || !string.Equals(media.AudioCodec, profile.AudioCodec, StringComparison.Ordinal)
                        || media.AudioSampleRate != profile.AudioSampleRate)))
            {
                throw new InvalidDataException(
                    "Rendered media does not match the approved profile dimensions or audio contract");
            }
            var after = Snapshot();
            var stable = string.Equals(
                after.Fingerprint,
                request.ExpectedStateDigest,
                StringComparison.Ordinal);
            if (!stable)
            {
                throw new RenderSourceDriftException("YMM4 project state changed while rendering");
            }
            var verifiedCandidateJournal = running.OverwriteJournal with
            {
                State = "candidate_verified",
                CandidateSha256 = media.Sha256,
                CandidateByteLength = media.ByteLength,
            };
            running = running with { OverwriteJournal = verifiedCandidateJournal };
            if (!control.TryAdvance(() => projectOperationStore.PutRender(running)))
            {
                throw new OperationCanceledException(control.Token);
            }
            renderNamespaceLease.Verify();
            if (running.OverwriteJournal.OriginalExisted)
            {
                RequireExactFileEvidence(
                    outputPath,
                    running.OverwriteJournal.OriginalSha256!,
                    running.OverwriteJournal.OriginalByteLength!.Value,
                    "The pre-existing render output changed before publication");
                File.Replace(
                    candidatePath,
                    outputPath,
                    running.OverwriteJournal.BackupPath,
                    ignoreMetadataErrors: false);
                RequireExactFileEvidence(
                    running.OverwriteJournal.BackupPath,
                    running.OverwriteJournal.OriginalSha256!,
                    running.OverwriteJournal.OriginalByteLength!.Value,
                    "The render backup does not match the approved original output");
            }
            else if (File.Exists(outputPath))
            {
                throw new RenderSourceDriftException(
                    "A destination file appeared after deny-mode staging");
            }
            else
            {
                File.Move(candidatePath, outputPath, overwrite: false);
            }
            RequireExactFileEvidence(
                outputPath,
                media.Sha256,
                media.ByteLength,
                "The published render output differs from the verified candidate");
            running = running with
            {
                OverwriteJournal = running.OverwriteJournal with { State = "candidate_published" },
            };
            if (!control.TryAdvance(() => projectOperationStore.PutRender(running)))
            {
                throw new OperationCanceledException(control.Token);
            }
            var completed = running with
            {
                Status = "succeeded",
                ProgressBasisPoints = 10_000,
                Phase = "completed",
                Cancellable = false,
                AfterStateDigest = after.Fingerprint,
                Media = media,
                Error = null,
                OverwriteJournal = running.OverwriteJournal with { State = "committed" },
            };
            if (!control.TryAdvance(() => projectOperationStore.PutRender(completed)))
            {
                throw new OperationCanceledException(control.Token);
            }
            if (File.Exists(completed.OverwriteJournal.BackupPath))
            {
                try
                {
                    File.Delete(completed.OverwriteJournal.BackupPath);
                }
                catch (IOException)
                {
                    // The committed WAL proves the output is authoritative; startup retries cleanup.
                }
            }
        }
        catch (Exception error)
        {
            projectOperationStore.TryGetRender(request.TaskId, out var latest);
            var basis = latest ?? queued;
            RenderOverwriteJournalDto journal;
            try
            {
                journal = RecoverRenderOutput(basis.OverwriteJournal, outputPath);
            }
            catch (Exception restoreError)
            {
                error = new AggregateException(error, restoreError);
                var recoveryError = error.GetBaseException().Message;
                control.TryComplete(() =>
                {
                    projectOperationStore.TryGetRender(request.TaskId, out var current);
                    projectOperationStore.PutRender((current ?? basis) with
                    {
                        Status = "recovery_required",
                        Phase = "recovery_required",
                        Cancellable = false,
                        AfterStateDigest = TrySnapshot()?.Fingerprint,
                        Media = null,
                        Error = recoveryError,
                    });
                });
                return;
            }
            var terminalError = error;
            control.TryComplete(() =>
            {
                projectOperationStore.TryGetRender(request.TaskId, out var current);
                var cancelled = terminalError is OperationCanceledException
                    || control.Token.IsCancellationRequested
                    || current?.Status == "cancelling";
                var stale = terminalError is RenderSourceDriftException;
                projectOperationStore.PutRender((current ?? basis) with
                {
                    Status = cancelled ? "cancelled" : stale ? "stale" : "failed",
                    Phase = cancelled ? "cancelled" : stale ? "stale" : "failed",
                    Cancellable = false,
                    AfterStateDigest = TrySnapshot()?.Fingerprint,
                    Media = null,
                    Error = cancelled ? null : terminalError.GetBaseException().Message,
                    OverwriteJournal = journal,
                });
            });
        }
        finally
        {
            if (process is not null)
            {
                control.Detach(process);
                process.Dispose();
            }
            renderExecutions.TryRemove(request.TaskId, out _);
            control.Dispose();
            try
            {
                if (Directory.Exists(queued.OverwriteJournal.CandidateDirectory)
                    && !Directory.EnumerateFileSystemEntries(
                        queued.OverwriteJournal.CandidateDirectory).Any())
                {
                    Directory.Delete(queued.OverwriteJournal.CandidateDirectory);
                }
            }
            catch (IOException)
            {
                // The terminal WAL owns cleanup; an empty directory is harmless evidence.
            }
            applyGate.Release();
        }
    }

    private static RenderProfilesDto ReadRenderProfilesCore()
    {
        var (width, height, fps) = ReadActiveRenderDimensionsCore();
        RenderProfileDescriptorDto profile;
        string error;
        if (TryCaptureRenderRuntimeBinding(out var binding, out error)
            && binding is not null)
        {
            try
            {
                profile = CreateRenderProfile(width, height, fps, binding);
                return RenderProfileSet(profile);
            }
            catch (Exception exception)
            {
                error = exception.GetBaseException().Message;
            }
        }
        {
            var driverDigest = Hash(RenderDriver);
            var bindingError = string.IsNullOrWhiteSpace(error)
                ? "encoder-binding-unavailable"
                : error;
            var fields = new SortedDictionary<string, string>(StringComparer.Ordinal)
            {
                ["bindable"] = "false",
                ["bindingError"] = bindingError,
                ["container"] = "mp4",
                ["driverProfileDigest"] = driverDigest,
                ["fpsDenominator"] = "1",
                ["fpsNumerator"] = fps.ToString(System.Globalization.CultureInfo.InvariantCulture),
                ["hasAudio"] = "true",
                ["height"] = height.ToString(System.Globalization.CultureInfo.InvariantCulture),
                ["width"] = width.ToString(System.Globalization.CultureInfo.InvariantCulture),
            };
            profile = new RenderProfileDescriptorDto(
                "active-project-mp4",
                $"YMM4 active project {width}x{height} {fps}fps MP4 (unavailable)",
                HashDescriptor("render-profile", fields),
                "mp4",
                width,
                height,
                fps,
                1,
                true,
                string.Empty,
                string.Empty,
                0,
                string.Empty,
                string.Empty,
                string.Empty,
                driverDigest,
                false,
                bindingError);
        }
        return RenderProfileSet(profile);
    }

    private static BoundRenderProfile ReadBoundRenderProfileCore()
    {
        var (width, height, fps) = ReadActiveRenderDimensionsCore();
        var binding = CaptureRenderRuntimeBinding();
        RequireAuthoritativeRenderSourceBinding();
        return new BoundRenderProfile(
            CreateRenderProfile(width, height, fps, binding),
            binding);
    }

    private static (uint Width, uint Height, uint Fps) ReadActiveRenderDimensionsCore()
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var timeline = GetField(timelineViewModel, "timeline")
            ?? GetMember(timelineViewModel, "Timeline")
            ?? throw new BridgeUnavailableException("YMM4 timeline is unavailable");
        var videoInfo = GetMember(timeline, "VideoInfo")
            ?? GetMember(main, "VideoInfo")
            ?? throw new BridgeUnavailableException("YMM4 video settings are unavailable");
        var width = ReadRequiredUInt(videoInfo, "Width", "VideoWidth");
        var height = ReadRequiredUInt(videoInfo, "Height", "VideoHeight");
        var fps = FindFps(main, timelineViewModel);
        return (width, height, fps);
    }

    private static RenderProfileDescriptorDto CreateRenderProfile(
        uint width,
        uint height,
        uint fps,
        RenderRuntimeBinding binding)
    {
        if (binding.WriterWidth != width
            || binding.WriterHeight != height
            || binding.WriterFps != fps
            || binding.WriterAudioHz == 0)
        {
            throw new BridgeUnavailableException(
                "The persisted child-writer dimensions/rate do not match the active timeline");
        }
        var fields = new SortedDictionary<string, string>(StringComparer.Ordinal)
        {
            ["bindable"] = "true",
            ["bindingManifestDigest"] = binding.ManifestDigest,
            ["container"] = binding.Container,
            ["driverProfileDigest"] = binding.DriverProfileDigest,
            ["fpsDenominator"] = "1",
            ["fpsNumerator"] = fps.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["hasAudio"] = "true",
            ["height"] = height.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["audioCodec"] = binding.AudioCodec,
            ["audioSampleRate"] = binding.WriterAudioHz.ToString(
                System.Globalization.CultureInfo.InvariantCulture),
            ["pixelFormat"] = binding.PixelFormat,
            ["videoCodec"] = binding.VideoCodec,
            ["width"] = width.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["writerPlugin"] = binding.WriterPlugin,
        };
        return new RenderProfileDescriptorDto(
            "active-project-mp4",
            $"YMM4 active project {width}x{height} {fps}fps H.264/AAC MP4",
            HashDescriptor("render-profile", fields),
            binding.Container,
            width,
            height,
            fps,
            1,
            true,
            binding.VideoCodec,
            binding.AudioCodec,
            binding.WriterAudioHz,
            binding.PixelFormat,
            binding.WriterPlugin,
            binding.ManifestDigest,
            binding.DriverProfileDigest,
            true,
            null);
    }

    private static RenderProfilesDto RenderProfileSet(RenderProfileDescriptorDto profile) =>
        new(
            [profile],
            HashDescriptor(
                "render-profile-set",
                new SortedDictionary<string, string>(StringComparer.Ordinal)
                {
                    [profile.DescriptorId] = profile.ProfileDigest,
                }));

    private static void EnsureRenderRuntimeBinding(
        string expectedProfileDigest,
        RenderProfileDescriptorDto approvedProfile,
        RenderRuntimeBinding approvedBinding,
        BoundRenderProfile current)
    {
        if (!current.Profile.Bindable
            || current.Profile != approvedProfile
            || !string.Equals(current.Profile.ProfileDigest, expectedProfileDigest, StringComparison.Ordinal)
            || !string.Equals(
                current.Binding.ManifestDigest,
                approvedBinding.ManifestDigest,
                StringComparison.Ordinal)
            || !string.Equals(
                current.Binding.DriverProfileDigest,
                approvedBinding.DriverProfileDigest,
                StringComparison.Ordinal))
        {
            throw new RenderSourceDriftException(
                "The approved writer settings, resolved arguments, or encoder binaries changed");
        }
    }

    private static uint ReadRequiredUInt(object value, params string[] names)
    {
        var member = GetMember(value, names);
        if (member is null
            || !uint.TryParse(
                member.ToString(),
                System.Globalization.NumberStyles.Integer,
                System.Globalization.CultureInfo.InvariantCulture,
                out var parsed)
            || parsed == 0)
        {
            throw new BridgeUnavailableException(
                $"YMM4 video setting is unavailable: {string.Join('/', names)}");
        }
        return parsed;
    }

    private static void ValidateRenderRequestShape(RenderRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        if (request.TaskId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.TargetIdentityDigest)
            || string.IsNullOrWhiteSpace(request.ExpectedStateDigest)
            || request.CheckpointOperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.CheckpointRequestDigest)
            || string.IsNullOrWhiteSpace(request.CheckpointProjectPath)
            || !Path.IsPathFullyQualified(request.CheckpointProjectPath)
            || string.IsNullOrWhiteSpace(request.CheckpointFileSha256)
            || request.CheckpointFileSha256.Length != 64
            || string.IsNullOrWhiteSpace(request.RenderProfileDigest)
            || string.IsNullOrWhiteSpace(request.OutputPath)
            || !Path.IsPathFullyQualified(request.OutputPath)
            || !string.Equals(Path.GetExtension(request.OutputPath), ".mp4", StringComparison.OrdinalIgnoreCase)
            || request.OverwritePolicy is not ("deny" or "replace_existing"))
        {
            throw new BridgeValidationException("Render request has invalid binding, output, or policy fields");
        }
        if (!ApplyRequestDigest.Matches(request.RequestDigest, ApplyRequestDigest.Compute(request)))
        {
            throw new BridgeValidationException("Render request digest does not match its payload");
        }
    }

    private static void EnsureRenderBinding(RenderRequestDto request, RenderTaskDto task)
    {
        if (task.TaskId != request.TaskId
            || !string.Equals(task.RequestDigest, request.RequestDigest, StringComparison.Ordinal)
            || !string.Equals(task.ProjectId, request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(task.SceneId, request.SceneId, StringComparison.Ordinal)
            || task.SourceRevision != request.SourceRevision
            || !string.Equals(task.TargetIdentityDigest, request.TargetIdentityDigest, StringComparison.Ordinal)
            || !string.Equals(task.ExpectedStateDigest, request.ExpectedStateDigest, StringComparison.Ordinal)
            || task.CheckpointOperationId != request.CheckpointOperationId
            || !string.Equals(task.CheckpointRequestDigest, request.CheckpointRequestDigest, StringComparison.Ordinal)
            || !string.Equals(task.CheckpointProjectPath, request.CheckpointProjectPath, StringComparison.Ordinal)
            || !string.Equals(task.CheckpointFileSha256, request.CheckpointFileSha256, StringComparison.Ordinal)
            || task.CheckpointFileBytes != request.CheckpointFileBytes
            || !string.Equals(task.RenderProfileDigest, request.RenderProfileDigest, StringComparison.Ordinal)
            || !string.Equals(task.OutputPath, request.OutputPath, StringComparison.Ordinal)
            || !string.Equals(task.OverwritePolicy, request.OverwritePolicy, StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Render task ID is already bound to another request",
                task.BeforeStateDigest);
        }
    }

    private static void EnsureRenderCancelBinding(RenderCancelRequestDto request, RenderTaskDto task)
    {
        if (task.TaskId != request.TaskId
            || !string.Equals(task.RequestDigest, request.RequestDigest, StringComparison.Ordinal)
            || !string.Equals(task.ProjectId, request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(task.SceneId, request.SceneId, StringComparison.Ordinal)
            || task.SourceRevision != request.SourceRevision
            || !string.Equals(task.ExpectedStateDigest, request.ExpectedStateDigest, StringComparison.Ordinal)
            || task.CheckpointOperationId != request.CheckpointOperationId
            || !string.Equals(task.CheckpointRequestDigest, request.CheckpointRequestDigest, StringComparison.Ordinal)
            || !string.Equals(task.CheckpointFileSha256, request.CheckpointFileSha256, StringComparison.Ordinal)
            || task.CheckpointFileBytes != request.CheckpointFileBytes
            || !string.Equals(task.RenderProfileDigest, request.RenderProfileDigest, StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Render cancellation is not bound to the original task",
                task.BeforeStateDigest);
        }
    }

    private static RenderTaskDto ValidateRenderedArtifact(RenderTaskDto task)
    {
        if (task.Status != "succeeded")
        {
            return task;
        }
        if (task.Media is null)
        {
            throw new BridgeUnavailableException("Succeeded render has no media receipt");
        }
        RenderedMediaReceiptDto measured;
        try
        {
            measured = Mp4MediaProbe.Probe(task.OutputPath) with { OutputPath = task.OutputPath };
        }
        catch (Exception error)
        {
            throw new BridgeUnavailableException(
                $"Succeeded render output can no longer be verified: {error.Message}");
        }
        if (measured != task.Media)
        {
            throw new BridgeUnavailableException(
                "Succeeded render output no longer matches its durable media receipt");
        }
        return task;
    }

    private void RecoverInterruptedRenderTasks()
    {
        foreach (var task in projectOperationStore.ReadRenders()
                     .Where(value => value.Status is "queued" or "running" or "cancelling"))
        {
            if (renderExecutions.ContainsKey(task.TaskId))
            {
                continue;
            }
            try
            {
                ValidateStoredRenderTask(task);
                var journal = RecoverRenderOutput(task.OverwriteJournal, task.OutputPath);
                projectOperationStore.PutRender(task with
                {
                    Status = "recovery_required",
                    Phase = "recovery_required",
                    Cancellable = false,
                    Media = null,
                    OverwriteJournal = journal,
                    Error = "YMM4 bridge restarted while the render lifecycle was incomplete; original output was restored and stale output was quarantined",
                });
            }
            catch (Exception error)
            {
                projectOperationStore.PutRender(task with
                {
                    Status = "recovery_required",
                    Phase = "recovery_required",
                    Cancellable = false,
                    Media = null,
                    Error = $"Interrupted render recovery could not prove restoration: {error.Message}",
                });
            }
        }

        foreach (var task in projectOperationStore.ReadRenders()
                     .Where(value => value.Status == "succeeded"))
        {
            if (File.Exists(task.OverwriteJournal.BackupPath))
            {
                try
                {
                    File.Delete(task.OverwriteJournal.BackupPath);
                }
                catch (IOException)
                {
                    // Retain a committed backup rather than disrupting unrelated recovery.
                }
            }
        }
    }

    internal static RenderOverwriteJournalDto RecoverRenderOutput(
        RenderOverwriteJournalDto journal,
        string outputPath)
    {
        outputPath = Path.GetFullPath(outputPath);
        var outputDirectory = Path.GetDirectoryName(outputPath)
            ?? throw new InvalidDataException("Render output has no parent directory");
        using var namespaceLease = RenderPathNamespaceLease.Open(
            [],
            [outputDirectory]);
        foreach (var path in new[]
                 {
                     outputPath,
                     journal.BackupPath,
                     journal.QuarantinePath,
                     journal.CandidatePath,
                 })
        {
            RenderPathNamespaceLease.RequireRegularFilePath(path, mustExist: false);
        }
        RenderPathNamespaceLease.RequireDirectoryPath(journal.CandidateDirectory);
        var candidateBound = journal.CandidateSha256 is not null
            && journal.CandidateByteLength is not null;
        if ((journal.CandidateSha256 is null) != (journal.CandidateByteLength is null))
        {
            throw new InvalidDataException("Render WAL has incomplete candidate evidence");
        }
        var outputIsCandidate = candidateBound
            && FileEvidenceMatches(
                outputPath,
                journal.CandidateSha256!,
                journal.CandidateByteLength!.Value);
        var candidatePathIsOwned = File.Exists(journal.CandidatePath)
            && (!candidateBound
                || FileEvidenceMatches(
                    journal.CandidatePath,
                    journal.CandidateSha256!,
                    journal.CandidateByteLength!.Value));
        if (outputIsCandidate && File.Exists(journal.CandidatePath))
        {
            throw new InvalidDataException(
                "Render WAL has the same candidate evidence at two paths");
        }
        var quarantined = false;
        if (journal.OriginalExisted)
        {
            var originalSha256 = journal.OriginalSha256
                ?? throw new InvalidDataException("Render WAL has no original hash");
            var originalByteLength = journal.OriginalByteLength
                ?? throw new InvalidDataException("Render WAL has no original byte length");
            var outputIsOriginal = FileEvidenceMatches(
                outputPath,
                originalSha256,
                originalByteLength);
            if (outputIsCandidate && File.Exists(journal.BackupPath))
            {
                if (File.Exists(journal.QuarantinePath))
                {
                    throw new IOException("Render quarantine path already exists");
                }
                File.Replace(
                    journal.BackupPath,
                    outputPath,
                    journal.QuarantinePath,
                    ignoreMetadataErrors: false);
                quarantined = true;
                outputIsOriginal = FileEvidenceMatches(
                    outputPath,
                    originalSha256,
                    originalByteLength);
            }
            else if (!outputIsOriginal)
            {
                throw new InvalidDataException(
                    "Recovery refused to move a destination without task-owned candidate evidence");
            }
            RequireExactFileEvidence(
                outputPath,
                originalSha256,
                originalByteLength,
                "The original render output could not be restored exactly");
        }
        else if (outputIsCandidate)
        {
            QuarantineRenderOutput(outputPath, journal.QuarantinePath);
            quarantined = true;
        }
        // The candidate path is task-owned, but a verified journal must still match
        // its exact bytes before recovery moves it. Unknown destination bytes are never moved.
        if (File.Exists(journal.CandidatePath))
        {
            if (!candidatePathIsOwned)
            {
                throw new InvalidDataException(
                    "The task-owned candidate path no longer matches its durable evidence");
            }
            if (File.Exists(journal.QuarantinePath))
            {
                throw new IOException("Render quarantine path already exists");
            }
            QuarantineRenderOutput(journal.CandidatePath, journal.QuarantinePath);
            quarantined = true;
        }
        namespaceLease.Verify();
        return journal with { State = quarantined ? "quarantined_and_restored" : "restored" };
    }

    internal static void ValidateStoredRenderTask(RenderTaskDto task)
    {
        if (task.TaskId == Guid.Empty
            || !Path.IsPathFullyQualified(task.OutputPath)
            || !Path.IsPathFullyQualified(task.EncodeSourcePath)
            || !PathsEqual(task.OutputPath, Path.GetFullPath(task.OutputPath))
            || !PathsEqual(task.EncodeSourcePath, Path.GetFullPath(task.EncodeSourcePath))
            || task.Status is not ("queued" or "running" or "cancelling" or "cancelled"
                or "succeeded" or "failed" or "stale" or "recovery_required")
            || task.OverwriteJournal.State is not ("prepared" or "encoding_candidate"
                or "candidate_verified" or "candidate_published" or "committed"
                or "quarantined_and_restored" or "restored"))
        {
            throw new InvalidDataException("Stored render task has an invalid lifecycle shape");
        }
        var outputPath = Path.GetFullPath(task.OutputPath);
        var outputDirectory = Path.GetDirectoryName(outputPath)
            ?? throw new InvalidDataException("Stored render output has no parent directory");
        var expectedBackup = $"{outputPath}.takegraph-backup-{task.TaskId:N}";
        var expectedQuarantine = $"{outputPath}.takegraph-stale-{task.TaskId:N}";
        var expectedCandidateDirectory = Path.Combine(
            outputDirectory,
            $".takegraph-render-{task.TaskId:N}");
        var expectedCandidatePath = Path.Combine(expectedCandidateDirectory, "candidate.mp4");
        var journal = task.OverwriteJournal;
        if (!PathsEqual(journal.BackupPath, expectedBackup)
            || !PathsEqual(journal.QuarantinePath, expectedQuarantine)
            || !PathsEqual(journal.CandidateDirectory, expectedCandidateDirectory)
            || !PathsEqual(journal.CandidatePath, expectedCandidatePath)
            || journal.OriginalExisted
                != (journal.OriginalSha256 is not null && journal.OriginalByteLength is not null)
            || (journal.CandidateSha256 is null) != (journal.CandidateByteLength is null)
            || (journal.CandidateSha256 is not null
                && (journal.CandidateSha256.Length != 64
                    || journal.CandidateSha256.Any(character => !Uri.IsHexDigit(character))))
            || (journal.OriginalSha256 is not null
                && (journal.OriginalSha256.Length != 64
                    || journal.OriginalSha256.Any(character => !Uri.IsHexDigit(character))))
            || (journal.State is "candidate_verified" or "candidate_published" or "committed"
                && journal.CandidateSha256 is null))
        {
            throw new InvalidDataException(
                "Stored render WAL is not deterministically bound to task/output identity");
        }
    }

    private static void QuarantineRenderOutput(string outputPath, string quarantinePath)
    {
        if (File.Exists(quarantinePath))
        {
            throw new IOException("Render quarantine path already exists");
        }
        File.Move(outputPath, quarantinePath);
    }

    private static (string Sha256, ulong ByteLength) ReadFileEvidence(string path)
    {
        using var stream = new FileStream(
            Path.GetFullPath(path),
            FileMode.Open,
            FileAccess.Read,
            FileShare.Read);
        return (
            Convert.ToHexStringLower(SHA256.HashData(stream)),
            checked((ulong)stream.Length));
    }

    private static void RequireExactFileEvidence(
        string path,
        string sha256,
        ulong byteLength,
        string message)
    {
        if (!File.Exists(path))
        {
            throw new RenderSourceDriftException(message);
        }
        var actual = ReadFileEvidence(path);
        if (actual.ByteLength != byteLength
            || !string.Equals(actual.Sha256, sha256, StringComparison.Ordinal))
        {
            throw new RenderSourceDriftException(message);
        }
    }

    internal static FileStream OpenVerifiedReadLease(
        string path,
        string sha256,
        ulong byteLength,
        string message)
    {
        FileStream? stream = null;
        try
        {
            stream = new FileStream(
                Path.GetFullPath(path),
                FileMode.Open,
                FileAccess.Read,
                FileShare.Read);
            RequireExactFileEvidence(stream, sha256, byteLength, message);
            return stream;
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            stream?.Dispose();
            throw new RenderSourceDriftException($"{message}: {error.Message}");
        }
        catch
        {
            stream?.Dispose();
            throw;
        }
    }

    private static void RequireExactFileEvidence(
        FileStream stream,
        string sha256,
        ulong byteLength,
        string message)
    {
        stream.Position = 0;
        var actualLength = checked((ulong)stream.Length);
        var actualSha256 = Convert.ToHexStringLower(SHA256.HashData(stream));
        stream.Position = 0;
        if (actualLength != byteLength
            || !string.Equals(actualSha256, sha256, StringComparison.Ordinal))
        {
            throw new RenderSourceDriftException(message);
        }
    }

    private static bool FileEvidenceMatches(string path, string sha256, ulong byteLength)
    {
        if (!File.Exists(path))
        {
            return false;
        }
        var actual = ReadFileEvidence(path);
        return actual.ByteLength == byteLength
            && string.Equals(actual.Sha256, sha256, StringComparison.Ordinal);
    }

    private CheckpointReceiptDto RequireVerifiedRenderCheckpoint(
        RenderRequestDto request,
        ProjectSnapshotDto snapshot)
    {
        if (!projectOperationStore.TryGetCheckpoint(request.CheckpointOperationId, out var checkpoint)
            || checkpoint is null
            || checkpoint.Status != "verified"
            || !string.Equals(checkpoint.RequestDigest, request.CheckpointRequestDigest, StringComparison.Ordinal)
            || !string.Equals(checkpoint.ProjectId, request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(checkpoint.SceneId, request.SceneId, StringComparison.Ordinal)
            || checkpoint.SourceRevision != request.SourceRevision
            || !string.Equals(checkpoint.TargetIdentityDigest, request.TargetIdentityDigest, StringComparison.Ordinal)
            || !string.Equals(checkpoint.ExpectedStateDigest, request.ExpectedStateDigest, StringComparison.Ordinal)
            || !string.Equals(checkpoint.BeforeStateDigest, request.ExpectedStateDigest, StringComparison.Ordinal)
            || !string.Equals(checkpoint.AfterStateDigest, request.ExpectedStateDigest, StringComparison.Ordinal)
            || !PathsEqual(checkpoint.ProjectPath, request.CheckpointProjectPath)
            || !PathsEqual(snapshot.ProjectPath, request.CheckpointProjectPath)
            || !string.Equals(checkpoint.PostFileSha256, request.CheckpointFileSha256, StringComparison.Ordinal)
            || checkpoint.PostFileBytes != request.CheckpointFileBytes)
        {
            throw new BridgeConflictException(
                "Render request is not bound to the exact verified checkpoint receipt",
                snapshot.Fingerprint);
        }
        RequireExactFileEvidence(
            request.CheckpointProjectPath,
            request.CheckpointFileSha256,
            request.CheckpointFileBytes,
            "The verified checkpoint file no longer matches its receipt");
        return checkpoint;
    }

    private string CreateImmutableCheckpointSnapshot(
        RenderRequestDto request,
        CheckpointReceiptDto checkpoint)
    {
        var extension = Path.GetExtension(checkpoint.ProjectPath);
        if (string.IsNullOrWhiteSpace(extension))
        {
            extension = ".ymmp";
        }
        var sourceDirectory = Path.GetDirectoryName(checkpoint.ProjectPath)
            ?? throw new BridgeValidationException("Checkpoint project path has no parent directory");
        var sourceName = Path.GetFileNameWithoutExtension(checkpoint.ProjectPath);
        var destination = Path.Combine(
            sourceDirectory,
            $".{sourceName}.takegraph-render-{request.TaskId:N}{extension}");
        if (File.Exists(destination))
        {
            throw new BridgeConflictException(
                "Immutable render checkpoint snapshot already exists",
                request.ExpectedStateDigest);
        }
        var temporary = $"{destination}.tmp-{Guid.NewGuid():N}";
        try
        {
            RequireExactFileEvidence(
                checkpoint.ProjectPath,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                "The checkpoint changed before immutable snapshot creation");
            File.Copy(checkpoint.ProjectPath, temporary, overwrite: false);
            RequireExactFileEvidence(
                temporary,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                "The immutable checkpoint copy does not match its receipt");
            RequireExactFileEvidence(
                checkpoint.ProjectPath,
                request.CheckpointFileSha256,
                request.CheckpointFileBytes,
                "The checkpoint changed during immutable snapshot creation");
            File.Move(temporary, destination);
            return destination;
        }
        finally
        {
            if (File.Exists(temporary))
            {
                File.Delete(temporary);
            }
        }
    }

    private static string TrimProcessOutput(string stderr, string stdout)
    {
        var value = string.IsNullOrWhiteSpace(stderr) ? stdout : stderr;
        value = value.Trim();
        return value.Length <= 2_000 ? value : value[^2_000..];
    }

    internal ProjectControlResultDto Undo()
    {
        applyGate.Wait();
        try
        {
            EnsureRecoveryClearAsync().GetAwaiter().GetResult();
            return ExecuteTakeGraphHistory(undo: true);
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal ProjectControlResultDto Redo()
    {
        applyGate.Wait();
        try
        {
            EnsureRecoveryClearAsync().GetAwaiter().GetResult();
            return ExecuteTakeGraphHistory(undo: false);
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal ProjectControlResultDto ScheduleApplicationClose()
    {
        applyGate.Wait();
        try
        {
            EnsureRecoveryClearAsync().GetAwaiter().GetResult();
            var dispatcher = Application.Current.Dispatcher;
            return dispatcher.Invoke(() =>
            {
                var main = RequireMainViewModel();
                var window = Application.Current.Windows.OfType<Window>()
                    .FirstOrDefault(candidate => ReferenceEquals(candidate.DataContext, main))
                    ?? throw new BridgeUnavailableException("YMM4 main window is unavailable");
                _ = Task.Run(async () =>
                {
                    await Task.Delay(500).ConfigureAwait(false);
                    await dispatcher.InvokeAsync(window.Close);
                });
                return new ProjectControlResultDto("close", "application", "Window.Close", true);
            });
        }
        finally
        {
            applyGate.Release();
        }
    }

    private ProjectSnapshotDto? TrySnapshot()
    {
        try
        {
            return Snapshot();
        }
        catch
        {
            return null;
        }
    }

    private static IReadOnlyDictionary<string, object> ControlScopes()
    {
        var main = RequireMainViewModel();
        var scopes = new Dictionary<string, object>(StringComparer.Ordinal)
        {
            ["main"] = main,
        };
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel");
        if (timelineViewModel is not null)
        {
            scopes["timeline_view"] = timelineViewModel;
            var timeline = GetField(timelineViewModel, "timeline") ?? GetMember(timelineViewModel, "Timeline");
            if (timeline is not null)
            {
                scopes["timeline"] = timeline;
            }
        }
        var project = GetMember(main, "Project", "project", "_project");
        if (project is not null)
        {
            scopes["project"] = project;
        }

        for (var depth = 0; depth < 3; depth++)
        {
            var added = false;
            foreach (var scope in scopes.ToArray())
            {
                foreach (var name in new[] { "Scene", "scene", "Model", "model", "Timeline", "timeline" })
                {
                    var nested = GetMember(scope.Value, name);
                    if (nested is null
                        || nested is string
                        || nested.GetType().IsValueType
                        || scopes.Values.Any(value => ReferenceEquals(value, nested)))
                    {
                        continue;
                    }
                    scopes[$"{scope.Key}_{name.ToLowerInvariant()}"] = nested;
                    added = true;
                }
            }
            if (!added)
            {
                break;
            }
        }

        foreach (var scope in scopes.ToArray())
        {
            var manager = GetMember(scope.Value, "UndoRedoManager");
            if (manager is not null
                && !scopes.Values.Any(value => ReferenceEquals(value, manager)))
            {
                scopes[$"{scope.Key}_undo_redo"] = manager;
            }
        }
        return scopes;
    }

    private static ProjectControlDto? ReadControl(string scope, object target, PropertyInfo property)
    {
        try
        {
            return property.GetValue(target) is ICommand command
                ? new ProjectControlDto(scope, property.Name, command.CanExecute(null))
                : null;
        }
        catch
        {
            return null;
        }
    }

    private static bool IsProjectControlName(string name)
    {
        return name.Contains("Save", StringComparison.OrdinalIgnoreCase)
            || name.Contains("Undo", StringComparison.OrdinalIgnoreCase)
            || name.Contains("Redo", StringComparison.OrdinalIgnoreCase);
    }

    private ProjectControlResultDto ExecuteTakeGraphHistory(bool undo)
    {
        ValidateMutationRuntime();
        return Application.Current.Dispatcher.Invoke(() =>
        {
            lock (historyGate)
            {
                if (lastBatch is null || lastBatchUndone == undo)
                {
                    throw new BridgeUnavailableException(
                        $"TakeGraph batch {(undo ? "undo" : "redo")} is unavailable now");
                }

                if (undo)
                {
                    ReplaceBatchItems(lastBatch.Timeline, lastBatch.NewItems, lastBatch.OldItems);
                }
                else
                {
                    ReplaceBatchItems(lastBatch.Timeline, lastBatch.OldItems, lastBatch.NewItems);
                }
                lastBatchUndone = undo;
                var action = undo ? "undo" : "redo";
                return new ProjectControlResultDto(action, "takegraph_batch", action, true);
            }
        });
    }

    private static ProjectSnapshotDto SnapshotCore()
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var rawItems = ReadItems(timelineViewModel);
        var projectPath = GetString(main, "ProjectFilePath", "ProjectPath");
        var projectId = Hash($"project|{projectPath}");
        var managed = ReadManagedItems(rawItems, projectId);
        var managedObjects = FindManagedRawItems(rawItems, projectId)
            .ToHashSet(ReferenceEqualityComparer.Instance);
        var projectName = GetString(main, "ProjectName", "Title");
        if (string.IsNullOrWhiteSpace(projectName) && !string.IsNullOrWhiteSpace(projectPath))
        {
            projectName = Path.GetFileNameWithoutExtension(projectPath);
        }
        var sceneId = GetString(timelineViewModel, "ID", "Id", "SceneId");
        if (string.IsNullOrWhiteSpace(sceneId))
        {
            sceneId = Hash($"scene|{projectPath}|{timelineViewModel.GetType().FullName}");
        }
        var fps = FindFps(main, timelineViewModel);
        var fingerprint = Fingerprint(rawItems, projectPath, sceneId, fps);
        var nativeExtensions = ReadManagedNativeExtensions(rawItems, projectId, fingerprint);
        foreach (var nativeItem in rawItems.Where(item =>
                     NativeExtensionRemarkCodec.TryDecode(item.Remark, out var marker)
                     && marker is not null
                     && string.Equals(marker.ProjectId, projectId, StringComparison.Ordinal)))
        {
            managedObjects.Add(nativeItem.Item);
        }
        return new ProjectSnapshotDto(
            projectId,
            projectName,
            projectPath,
            sceneId,
            fps,
            fingerprint,
            managed,
            nativeExtensions,
            rawItems.Count(item => !managedObjects.Contains(item.Item)));
    }

    private static string CurrentProjectId()
    {
        var main = RequireMainViewModel();
        return Hash($"project|{GetString(main, "ProjectFilePath", "ProjectPath")}");
    }

    private static IReadOnlyList<ManagedItemDto> ReadManagedItems(
        IReadOnlyList<RawItem> rawItems,
        string projectId)
    {
        var result = new List<ManagedItemDto>();
        foreach (var voiceItem in rawItems)
        {
            if (!voiceItem.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                || !RemarkCodec.TryDecode(voiceItem.Remark, out var marker)
                || marker is null
                || !string.Equals(marker.ProjectId, projectId, StringComparison.Ordinal))
            {
                continue;
            }
            result.Add(new ManagedItemDto(
                marker.EntityId,
                marker.Revision,
                "voice",
                voiceItem.Frame,
                voiceItem.Layer,
                voiceItem.Length,
                voiceItem.Text,
                null,
                null,
                voiceItem.CharacterName,
                marker.RealizationId.ToString("D")));
        }
        foreach (var captionItem in rawItems)
        {
            if (!MarkerCodec.TryDecode(captionItem.Text, out var caption, out var marker) || marker is null)
            {
                continue;
            }

            result.Add(new ManagedItemDto(
                marker.EntityId,
                marker.Revision,
                "caption",
                captionItem.Frame,
                captionItem.Layer,
                captionItem.Length,
                caption,
                null,
                marker.ArtifactHash,
                marker.Speaker));
            var audio = rawItems.FirstOrDefault(item =>
                !ReferenceEquals(item.Item, captionItem.Item)
                && item.Layer == marker.AudioLayer
                && PathsEqual(item.AudioPath, marker.AudioPath)
                && !string.IsNullOrWhiteSpace(item.AudioPath));
            if (audio is not null)
            {
                result.Add(new ManagedItemDto(
                    marker.EntityId,
                    marker.Revision,
                    "audio",
                    audio.Frame,
                    audio.Layer,
                    audio.Length,
                    null,
                    audio.AudioPath,
                    ReadPortableArtifactHash(audio.AudioPath),
                    marker.Speaker));
            }
        }

        return result.OrderBy(item => item.Frame)
            .ThenBy(item => item.Layer)
            .ThenBy(item => item.EntityId, StringComparer.Ordinal)
            .ToArray();
    }

    private static IEnumerable<object> FindManagedRawItems(
        IReadOnlyList<RawItem> rawItems,
        string projectId)
    {
        foreach (var voice in rawItems)
        {
            if (voice.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                && RemarkCodec.TryDecode(voice.Remark, out var marker)
                && marker is not null
                && string.Equals(marker.ProjectId, projectId, StringComparison.Ordinal))
            {
                yield return voice.Item;
            }
        }
        foreach (var caption in rawItems)
        {
            if (!MarkerCodec.TryDecode(caption.Text, out _, out var marker) || marker is null)
            {
                continue;
            }
            yield return caption.Item;
            foreach (var audio in rawItems.Where(item =>
                         !ReferenceEquals(item.Item, caption.Item)
                         && item.Layer == marker.AudioLayer
                         && PathsEqual(item.AudioPath, marker.AudioPath)
                         && !string.IsNullOrWhiteSpace(item.AudioPath)))
            {
                yield return audio.Item;
            }
        }
    }

    private static NativeVoicePreparation PrepareNativeVoiceBatch(
        IReadOnlyList<NativeVoiceCueDto> cues,
        string projectId)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var timeline = GetField(timelineViewModel, "timeline")
            ?? GetMember(timelineViewModel, "Timeline")
            ?? throw new BridgeUnavailableException("YMM4 timeline mutation API is unavailable");
        var mainModel = RequireMainModel(main);
        var addMethod = mainModel.GetType()
            .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .SingleOrDefault(method =>
                method.Name == "AddVoiceItemAsync"
                && method.GetParameters() is
                [
                    { ParameterType: var frameType },
                    { ParameterType: var layerType },
                    _,
                    { ParameterType: var serifType },
                    _,
                ]
                && frameType == typeof(int)
                && layerType == typeof(int)
                && serifType == typeof(string))
            ?? throw new BridgeUnavailableException(
                "YMM4 MainModel.AddVoiceItemAsync(int,int,Character,string,decorations) is unavailable");
        var rawItems = ReadItems(timelineViewModel);
        var entityIds = cues.Select(value => value.EntityId).ToHashSet(StringComparer.Ordinal);
        var realizationIds = cues.Select(value => value.RealizationId).ToHashSet();
        var conflicts = ReadNativeVoiceMarkers(rawItems)
            .Where(value => entityIds.Contains(value.Marker.EntityId)
                || realizationIds.Contains(value.Marker.RealizationId))
            .ToArray();
        if (conflicts.Length > 0)
        {
            var foreign = conflicts.Any(value => !string.Equals(
                value.Marker.ProjectId,
                projectId,
                StringComparison.Ordinal));
            throw new BridgeConflictException(
                foreign
                    ? "A requested native voice identity is owned by a foreign project"
                    : "A requested native voice identity already exists",
                SnapshotCore().Fingerprint);
        }
        var characters = cues.ToDictionary(
            cue => cue.RealizationId,
            cue => ResolveCharacter(timelineViewModel, cue.CharacterName));
        var decorations = CreateEmptyDecorations(addMethod.GetParameters()[4].ParameterType);
        var knownItems = rawItems
            .Select(item => item.Item)
            .ToHashSet(ReferenceEqualityComparer.Instance);
        return new NativeVoicePreparation(
            mainModel,
            timelineViewModel,
            timeline,
            addMethod,
            decorations,
            cues,
            characters,
            knownItems);
    }

    private static IReadOnlyList<(RawItem Item, NativeVoiceMarker Marker)> ReadNativeVoiceMarkers(
        IReadOnlyList<RawItem> rawItems)
    {
        return rawItems.Where(item =>
                item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                && RemarkCodec.TryDecode(item.Remark, out _))
            .Select(item => (item, DecodeNativeVoiceMarker(item.Remark)))
            .ToArray();
    }

    private static async Task ApplyNativeVoiceBatchAsync(
        NativeVoicePreparation preparation,
        string projectId)
    {
        foreach (var cue in preparation.Cues)
        {
            var task = await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                try
                {
                    return preparation.AddMethod.Invoke(
                            preparation.MainModel,
                            [
                                cue.Frame,
                                cue.Layer,
                                preparation.Characters[cue.RealizationId],
                                cue.SpokenText,
                                preparation.Decorations,
                            ]) as Task
                        ?? throw new BridgeUnavailableException(
                            "YMM4 AddVoiceItemAsync did not return a Task");
                }
                catch (TargetInvocationException error)
                {
                    throw error.InnerException ?? error;
                }
            });
            await task.ConfigureAwait(false);

            RawItem[] newItems = [];
            for (var attempt = 0; attempt < 30; attempt++)
            {
                newItems = await Application.Current.Dispatcher.InvokeAsync(() => ReadItems(
                        preparation.TimelineViewModel)
                    .Where(item => !preparation.KnownItems.Contains(item.Item))
                    .ToArray());
                if (newItems.Any(item => IsExpectedNativeVoice(item, cue) && item.Length > 0))
                {
                    break;
                }
                await Task.Delay(100).ConfigureAwait(false);
            }

            await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                foreach (var item in newItems)
                {
                    preparation.KnownItems.Add(item.Item);
                }
                var voices = newItems
                    .Where(item => IsExpectedNativeVoice(item, cue))
                    .ToArray();
                if (voices.Length != 1)
                {
                    throw new BridgeUnavailableException(
                        $"Native voice creation produced {voices.Length} matching VoiceItem(s); exactly one is required");
                }

                var voice = voices[0];
                preparation.AddedItems.Add(voice.Item);
                var marker = new NativeVoiceMarker(
                    "takegraph/v2",
                    projectId,
                    cue.EntityId,
                    cue.RealizationId,
                    cue.Revision);
                SetRequired(
                    voice.Item,
                    RemarkCodec.Append(voice.Remark, marker),
                    "Remark");
                if (newItems.Length != 1)
                {
                    throw new BridgeUnavailableException(
                        $"Native voice creation also observed {newItems.Length - 1} unowned item(s); they were preserved and manual recovery is required");
                }
                if (voice.Frame != cue.Frame
                    || voice.Layer != cue.Layer
                    || voice.Length <= 0
                    || voice.Length > cue.MaxLength)
                {
                    throw new BridgeConflictException(
                        $"Native voice result exceeded its approved placement/duration budget for {cue.EntityId}",
                        SnapshotCore().Fingerprint);
                }
            });
            if (preparation.AddedItems.Count == 1)
            {
                BridgeFaultInjection.ThrowIf("after_partial_mutation");
            }
        }
    }

    private static bool IsExpectedNativeVoice(RawItem item, NativeVoiceCueDto cue)
    {
        return item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
            && item.Frame == cue.Frame
            && item.Layer == cue.Layer
            && item.Text == cue.SpokenText
            && item.CharacterName == cue.CharacterName;
    }

    private static object RequireMainModel(object mainViewModel)
    {
        return GetMember(mainViewModel, "Model", "model", "_model")
            ?? mainViewModel.GetType()
                .GetFields(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                .FirstOrDefault(field => field.FieldType.Name.Contains("MainModel", StringComparison.Ordinal))
                ?.GetValue(mainViewModel)
            ?? throw new BridgeUnavailableException("YMM4 MainModel is unavailable");
    }

    private static object ResolveCharacter(object timelineViewModel, string characterName)
    {
        var characters = GetMember(timelineViewModel, "Characters") as IEnumerable
            ?? throw new BridgeUnavailableException("YMM4 character list is unavailable");
        var matches = characters.Cast<object>()
            .Where(character => string.Equals(
                GetString(character, "Name"),
                characterName,
                StringComparison.Ordinal))
            .ToArray();
        return matches.Length switch
        {
            1 => matches[0],
            0 => throw new BridgeValidationException(
                $"YMM4 character was not found by exact name: {characterName}"),
            _ => throw new BridgeValidationException(
                $"YMM4 character name is ambiguous: {characterName}"),
        };
    }

    private static object CreateEmptyDecorations(Type parameterType)
    {
        var enumerable = parameterType.IsGenericType
            && parameterType.GetGenericTypeDefinition() == typeof(IEnumerable<>)
                ? parameterType
                : parameterType.GetInterfaces()
                    .FirstOrDefault(type =>
                        type.IsGenericType
                        && type.GetGenericTypeDefinition() == typeof(IEnumerable<>));
        var elementType = enumerable?.GetGenericArguments()[0]
            ?? throw new BridgeUnavailableException(
                "YMM4 AddVoiceItemAsync decorations parameter is unsupported");
        return Array.CreateInstance(elementType, 0);
    }

    private static AppliedBatch PrepareApplyBatch(IReadOnlyList<ManagedUtteranceDto> utterances)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var timeline = GetField(timelineViewModel, "timeline")
            ?? GetMember(timelineViewModel, "Timeline")
            ?? throw new BridgeUnavailableException("YMM4 timeline mutation API is unavailable");
        var rawItems = ReadItems(timelineViewModel);
        var entityIds = utterances.Select(value => value.EntityId).ToHashSet(StringComparer.Ordinal);
        var oldItems = new List<object>();
        foreach (var caption in rawItems)
        {
            if (!MarkerCodec.TryDecode(caption.Text, out _, out var marker)
                || marker is null
                || !entityIds.Contains(marker.EntityId))
            {
                continue;
            }
            oldItems.Add(caption.Item);
            oldItems.AddRange(rawItems.Where(item =>
                !ReferenceEquals(item.Item, caption.Item)
                && item.Layer == marker.AudioLayer
                && PathsEqual(item.AudioPath, marker.AudioPath)
                && !string.IsNullOrWhiteSpace(item.AudioPath))
                .Select(item => item.Item));
        }
        oldItems = oldItems.Distinct(ReferenceEqualityComparer.Instance).ToList();

        var newItems = utterances.SelectMany(CreateItems).ToArray();
        return new AppliedBatch(timeline, oldItems.ToArray(), newItems);
    }

    private static void ApplyPreparedBatch(AppliedBatch batch)
    {
        ReplaceBatchItems(batch.Timeline, batch.OldItems, batch.NewItems);
    }

    private static void ReplaceBatchItems(object timeline, object[] removeItems, object[] addItems)
    {
        if (removeItems.Length == 0 && addItems.Length == 0)
        {
            return;
        }
        if (removeItems.Length == 0)
        {
            InvokeItemsMethod(timeline, "AddItems", addItems);
            return;
        }
        if (addItems.Length == 0)
        {
            InvokeItemsMethod(timeline, "DeleteItems", removeItems);
            return;
        }
        if (removeItems.Length == addItems.Length)
        {
            InvokeItemsMethod(timeline, "ReplaceItems", removeItems, addItems);
            return;
        }
        InvokeItemsMethod(timeline, "DeleteItems", removeItems);
        InvokeItemsMethod(timeline, "AddItems", addItems);
    }

    private static IEnumerable<object> CreateItems(ManagedUtteranceDto utterance)
    {
        var audioType = FindItemType("AudioItem");
        var textType = FindItemType("TextItem");
        var audio = Activator.CreateInstance(audioType)
            ?? throw new InvalidOperationException("Unable to create a YMM4 AudioItem");
        SetRequired(audio, utterance.AudioPath, "FilePath");
        SetRequired(audio, utterance.Frame, "Frame");
        SetRequired(audio, utterance.AudioLayer, "Layer");
        SetRequired(audio, utterance.Length, "Length");

        var caption = Activator.CreateInstance(textType)
            ?? throw new InvalidOperationException("Unable to create a YMM4 TextItem");
        var markedCaption = MarkerCodec.Append(
            utterance.Caption,
            new ManagedMarker(
                utterance.EntityId,
                utterance.Revision,
                utterance.ArtifactHash,
                utterance.Speaker,
                Path.GetFullPath(utterance.AudioPath),
                utterance.AudioLayer));
        SetRequired(caption, markedCaption, "Text", "Serif");
        SetRequired(caption, utterance.Frame, "Frame");
        SetRequired(caption, utterance.CaptionLayer, "Layer");
        SetRequired(caption, utterance.Length, "Length");
        return [audio, caption];
    }

    private static void InvokeItemsMethod(object timeline, string name, params object[][] itemGroups)
    {
        var method = timeline.GetType().GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .FirstOrDefault(candidate => candidate.Name == name && candidate.GetParameters().Length == itemGroups.Length)
            ?? throw new MissingMethodException(timeline.GetType().FullName, name);
        var parameters = method.GetParameters();
        var arguments = new object[itemGroups.Length];
        for (var index = 0; index < itemGroups.Length; index++)
        {
            arguments[index] = CreateTypedItemArray(itemGroups[index], parameters[index].ParameterType);
        }
        try
        {
            method.Invoke(timeline, arguments);
        }
        catch (TargetInvocationException error)
        {
            throw error.InnerException ?? error;
        }
    }

    private static object CreateTypedItemArray(object[] items, Type parameterType)
    {
        var elementType = parameterType.IsArray
            ? parameterType.GetElementType()
            : parameterType.IsGenericType
                ? parameterType.GetGenericArguments().FirstOrDefault()
                : null;
        elementType ??= items.FirstOrDefault()?.GetType().GetInterfaces().FirstOrDefault(value => value.Name == "IItem");
        elementType ??= typeof(object);
        var array = Array.CreateInstance(elementType, items.Length);
        for (var index = 0; index < items.Length; index++)
        {
            array.SetValue(items[index], index);
        }
        return array;
    }

    private static Type FindItemType(string name)
    {
        var candidates = AppDomain.CurrentDomain.GetAssemblies().SelectMany(assembly =>
        {
            try
            {
                return assembly.GetTypes();
            }
            catch (ReflectionTypeLoadException error)
            {
                return error.Types.Where(type => type is not null)!;
            }
            catch
            {
                return [];
            }
        });
        return candidates.FirstOrDefault(type =>
                   type is { IsAbstract: false }
                   && type.Name == name
                   && type.Namespace?.Contains("YukkuriMovieMaker", StringComparison.Ordinal) == true)
               ?? throw new TypeLoadException($"YMM4 item type not found: {name}");
    }

    private static IReadOnlyList<RawItem> ReadItems(object timelineViewModel)
    {
        var enumerable = GetMember(timelineViewModel, "Items") as IEnumerable
            ?? throw new BridgeUnavailableException("YMM4 timeline items are unavailable");
        var result = new List<RawItem>();
        foreach (var wrapper in enumerable)
        {
            if (wrapper is null)
            {
                continue;
            }
            var item = GetMember(wrapper, "Item") ?? wrapper;
            var selectedValue = GetMember(wrapper, "IsSelected");
            result.Add(new RawItem(
                item,
                GetRequiredInt(item, wrapper, "Frame"),
                GetRequiredInt(item, wrapper, "Layer"),
                GetRequiredInt(item, wrapper, "Length"),
                GetInt(item, wrapper, "GroupID", "GroupId"),
                GetString(item, "Serif", "Text", "Name"),
                GetString(item, "FilePath"),
                GetString(item, "Remark"),
                GetString(item, "CharacterName"),
                GetString(item, "Hatsuon", "Pronounce"),
                item.GetType().FullName ?? item.GetType().Name,
                selectedValue is bool selected && selected,
                selectedValue is bool));
        }
        return result;
    }

    private static object RequireMainViewModel()
    {
        var candidates = Application.Current.Windows.OfType<Window>()
            .Select(window => window.DataContext)
            .Where(value => value is not null)
            .Cast<object>()
            .ToArray();
        return candidates.FirstOrDefault(value => GetMember(value, "ActiveTimelineViewModel") is not null)
            ?? throw new BridgeUnavailableException("YMM4 MainViewModel is unavailable");
    }

    private static object? GetMember(object? value, params string[] names)
    {
        if (value is null)
        {
            return null;
        }
        var flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance;
        foreach (var name in names)
        {
            try
            {
                var found = value.GetType().GetProperty(name, flags)?.GetValue(value)
                    ?? value.GetType().GetField(name, flags)?.GetValue(value);
                if (found is not null)
                {
                    var valueProperty = found.GetType().Name.Contains("ReactiveProperty", StringComparison.Ordinal)
                        ? found.GetType().GetProperty("Value")
                        : null;
                    return valueProperty?.GetValue(found) ?? found;
                }
            }
            catch
            {
                // Try the next compatible member name.
            }
        }
        return null;
    }

    private static object? GetField(object value, string name)
    {
        return value.GetType().GetField(name, BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            ?.GetValue(value);
    }

    private static string GetString(object value, params string[] names)
    {
        var result = GetMember(value, names)?.ToString() ?? string.Empty;
        return result.Equals("null", StringComparison.OrdinalIgnoreCase) ? string.Empty : result;
    }

    private static int GetInt(object item, object fallback, params string[] names)
    {
        var value = GetMember(item, names) ?? GetMember(fallback, names);
        return value is null || !int.TryParse(value.ToString(), out var parsed) ? 0 : parsed;
    }

    internal static int GetRequiredInt(object item, object fallback, params string[] names)
    {
        var value = GetRequiredMember(item, fallback, names);
        try
        {
            return value switch
            {
                sbyte number => number,
                byte number => number,
                short number => number,
                ushort number => number,
                int number => number,
                uint number => checked((int)number),
                long number => checked((int)number),
                ulong number => checked((int)number),
                string text when int.TryParse(
                    text,
                    System.Globalization.NumberStyles.Integer,
                    System.Globalization.CultureInfo.InvariantCulture,
                    out var parsed) => parsed,
                _ => throw new BridgeUnavailableException(
                    $"Required YMM4 integer member has an unsupported value: {string.Join("/", names)}"),
            };
        }
        catch (OverflowException)
        {
            throw new BridgeUnavailableException(
                $"Required YMM4 integer member is out of range: {string.Join("/", names)}");
        }
    }

    internal static bool GetRequiredBool(object item, params string[] names)
    {
        var value = GetRequiredMember(item, null, names);
        return value is bool result
            ? result
            : throw new BridgeUnavailableException(
                $"Required YMM4 boolean member has an unsupported value: {string.Join("/", names)}");
    }

    internal static void EnsureUnlockedForMutation(
        object item,
        string subject,
        string actualFingerprint)
    {
        if (GetRequiredBool(item, "IsLocked"))
        {
            throw new BridgeConflictException(
                $"YMM4 item is locked and cannot be mutated: {subject}",
                actualFingerprint);
        }
    }

    private static object GetRequiredMember(
        object item,
        object? fallback,
        IReadOnlyList<string> names)
    {
        foreach (var source in fallback is null || ReferenceEquals(item, fallback)
                     ? new[] { item }
                     : new[] { item, fallback })
        {
            foreach (var name in names)
            {
                var (found, value) = ReadMemberStrict(source, name);
                if (!found)
                {
                    continue;
                }
                return value ?? throw new BridgeUnavailableException(
                    $"Required YMM4 member is null: {source.GetType().FullName}.{name}");
            }
        }
        throw new BridgeUnavailableException(
            $"Required YMM4 member is unavailable: {item.GetType().FullName}."
            + string.Join("/", names));
    }

    private static (bool Found, object? Value) ReadMemberStrict(object source, string name)
    {
        var flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance;
        var property = source.GetType().GetProperty(name, flags);
        var field = source.GetType().GetField(name, flags);
        if (property is null && field is null)
        {
            return (false, null);
        }
        try
        {
            var value = property is not null
                ? property.GetValue(source)
                : field!.GetValue(source);
            if (value is not null
                && value.GetType().Name.Contains("ReactiveProperty", StringComparison.Ordinal))
            {
                var valueProperty = value.GetType().GetProperty("Value", flags)
                    ?? throw new BridgeUnavailableException(
                        $"Required YMM4 reactive member has no Value: {source.GetType().FullName}.{name}");
                value = valueProperty.GetValue(value);
            }
            return (true, value);
        }
        catch (BridgeUnavailableException)
        {
            throw;
        }
        catch (Exception error)
        {
            throw new BridgeUnavailableException(
                $"Required YMM4 member read failed: {source.GetType().FullName}.{name}: "
                + error.GetBaseException().Message);
        }
    }

    private static uint FindFps(object main, object timeline)
    {
        return FindExactFps(main, timeline) ?? 30;
    }

    private static uint? FindExactFps(object main, object timeline)
    {
        foreach (var source in new[]
                 {
                     GetMember(main, "Project", "project", "_project"),
                     timeline,
                     GetField(timeline, "scene"),
                     GetField(timeline, "timeline"),
                 })
        {
            if (source is null)
            {
                continue;
            }
            foreach (var name in new[] { "FPS", "Fps", "FrameRate", "VideoFPS" })
            {
                var value = GetMember(source, name);
                if (value is not null && uint.TryParse(value.ToString(), out var fps) && fps is > 0 and <= 240)
                {
                    return fps;
                }
            }
        }
        return null;
    }

    private static string Fingerprint(
        IReadOnlyList<RawItem> items,
        string projectPath,
        string sceneId,
        uint fps)
    {
        var canonical = new StringBuilder();
        canonical.Append(projectPath).Append('|').Append(sceneId).Append('|').Append(fps).AppendLine();
        foreach (var item in items.OrderBy(value => value.Frame)
                     .ThenBy(value => value.Layer)
                     .ThenBy(value => value.TypeName, StringComparer.Ordinal)
                     .ThenBy(value => value.Text, StringComparer.Ordinal)
                     .ThenBy(value => value.AudioPath, StringComparer.Ordinal))
        {
            canonical.Append(item.TypeName).Append('|')
                .Append(item.Frame).Append('|')
                .Append(item.Layer).Append('|')
                .Append(item.Length).Append('|')
                .Append(item.GroupId).Append('|')
                .Append(item.Text).Append('|')
                .Append(item.AudioPath).Append('|')
                .Append(item.Remark).Append('|')
                .Append(item.CharacterName).Append('|')
                .Append(item.SpokenText).Append('|')
                .Append(NativeExtensionStateDigest([item.Item])).Append('|')
                .Append(ExternalFileWitness(item.AudioPath)).AppendLine();
        }
        return Hash(canonical.ToString());
    }

    private static string ExternalFileWitness(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return string.Empty;
        }
        try
        {
            var fullPath = Path.GetFullPath(path);
            var info = new FileInfo(fullPath);
            return info.Exists
                ? $"{fullPath}|{info.Length}|{info.LastWriteTimeUtc.Ticks}"
                : $"{fullPath}|missing";
        }
        catch
        {
            return $"{path}|unreadable";
        }
    }

    private static string Hash(string value)
    {
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(value))).ToLowerInvariant();
    }

    private static bool PathsEqual(string left, string right)
    {
        if (string.IsNullOrWhiteSpace(left) || string.IsNullOrWhiteSpace(right))
        {
            return false;
        }
        try
        {
            return Path.GetFullPath(left).Equals(Path.GetFullPath(right), StringComparison.OrdinalIgnoreCase);
        }
        catch
        {
            return left.Equals(right, StringComparison.OrdinalIgnoreCase);
        }
    }

    private static IReadOnlyList<ManagedItemDto> ToManagedItems(ManagedUtteranceDto utterance)
    {
        return
        [
            new ManagedItemDto(
                utterance.EntityId,
                utterance.Revision,
                "audio",
                utterance.Frame,
                utterance.AudioLayer,
                utterance.Length,
                null,
                utterance.AudioPath,
                utterance.ArtifactHash,
                utterance.Speaker),
            new ManagedItemDto(
                utterance.EntityId,
                utterance.Revision,
                "caption",
                utterance.Frame,
                utterance.CaptionLayer,
                utterance.Length,
                utterance.Caption,
                null,
                utterance.ArtifactHash,
                utterance.Speaker),
        ];
    }

    private static bool Equivalent(
        IEnumerable<ManagedItemDto> left,
        IEnumerable<ManagedItemDto> right)
    {
        static string Normalize(ManagedItemDto item)
        {
            return string.Join('|',
                item.EntityId,
                item.Revision,
                item.Kind,
                item.Frame,
                item.Layer,
                item.Length,
                item.Text ?? string.Empty,
                item.AudioPath ?? string.Empty,
                item.ArtifactHash ?? string.Empty,
                item.Speaker ?? string.Empty,
                item.RealizationId ?? string.Empty);
        }
        return left.Select(Normalize).Order(StringComparer.Ordinal)
            .SequenceEqual(right.Select(Normalize).Order(StringComparer.Ordinal), StringComparer.Ordinal);
    }

    private static void ValidateProtocol(int protocolVersion)
    {
        if (protocolVersion != BridgeContract.ProtocolVersion)
        {
            throw new BridgeValidationException(
                $"Protocol mismatch: expected {BridgeContract.ProtocolVersion}, got {protocolVersion}");
        }
    }

    private static bool IsSupportedMutationRuntime()
    {
        var version = Assembly.GetEntryAssembly()?.GetName().Version?.ToString();
        return string.Equals(version, SupportedMutationYmm4Version, StringComparison.Ordinal);
    }

    private static bool IsSupportedObservationRuntime()
    {
        var entryAssembly = Assembly.GetEntryAssembly();
        if (entryAssembly?.GetName().Name == "TakeGraph.Ymm4Bridge.Tests")
        {
            return true;
        }
        var version = entryAssembly?.GetName().Version?.ToString();
        return string.Equals(version, SupportedMutationYmm4Version, StringComparison.Ordinal);
    }

    private static void ValidateObservationRuntime()
    {
        if (!IsSupportedObservationRuntime())
        {
            var actual = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "unknown";
            throw new BridgeUnavailableException(
                $"YMM4 {actual} current-frame composition is unsupported; verified only for {SupportedMutationYmm4Version}");
        }
    }

    private static void ValidateMutationRuntime()
    {
        if (!IsSupportedMutationRuntime())
        {
            var actual = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "unknown";
            throw new BridgeUnavailableException(
                $"YMM4 {actual} is observation-only; mutation is verified only for {SupportedMutationYmm4Version}");
        }
    }

    private static bool ProbeNativeVoiceRuntime()
    {
        try
        {
            return Application.Current.Dispatcher.Invoke(() =>
            {
                var main = RequireMainViewModel();
                var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
                    ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
                _ = GetMember(timelineViewModel, "Characters") as IEnumerable
                    ?? throw new BridgeUnavailableException("YMM4 character list is unavailable");
                var mainModel = RequireMainModel(main);
                var addMethod = mainModel.GetType()
                    .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                    .SingleOrDefault(method =>
                        method.Name == "AddVoiceItemAsync"
                        && method.GetParameters() is
                        [
                            { ParameterType: var frameType },
                            { ParameterType: var layerType },
                            _,
                            { ParameterType: var serifType },
                            _,
                        ]
                        && frameType == typeof(int)
                        && layerType == typeof(int)
                        && serifType == typeof(string));
                if (addMethod is null)
                {
                    return false;
                }
                _ = CreateEmptyDecorations(addMethod.GetParameters()[4].ParameterType);
                var voiceType = FindItemType("VoiceItem");
                return voiceType.GetProperty(
                    "Remark",
                    BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance) is not null;
            });
        }
        catch
        {
            return false;
        }
    }

    private static bool ProbeSceneCaptureRuntime()
    {
        try
        {
            return Application.Current.Dispatcher.Invoke(() =>
            {
                var preview = RequirePreviewViewModel();
                var methods = preview.GetType().GetMethods(
                    BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance);
                var canSeek = methods.Any(method =>
                    method.Name == "SeekAsync"
                    && method.GetParameters() is [{ ParameterType: var parameterType }]
                    && (parameterType == typeof(int) || parameterType == typeof(TimeSpan)));
                var canSave = methods.Any(method =>
                    method.Name == "SaveImage"
                    && method.GetParameters() is
                    [
                        { ParameterType: var pathType },
                        { ParameterType: var alphaType },
                    ]
                    && pathType == typeof(string)
                    && alphaType == typeof(bool));
                _ = ReadProjectDirty();
                _ = ReadSelectionDigest();
                return canSeek && canSave;
            });
        }
        catch
        {
            return false;
        }
    }

    private static object RequirePreviewViewModel()
    {
        var main = RequireMainViewModel();
        if (GetMember(main, "AnchorableAreaViewModels") is IEnumerable areas)
        {
            foreach (var area in areas)
            {
                if (area is null)
                {
                    continue;
                }
                var viewModel = GetMember(area, "ViewModel") ?? area;
                if (viewModel.GetType().Name == "PreviewViewModel")
                {
                    return viewModel;
                }
            }
        }
        foreach (var name in new[] { "PreviewViewModel", "PlayerViewModel", "Preview", "Player" })
        {
            var candidate = GetMember(main, name);
            if (candidate?.GetType().Name == "PreviewViewModel")
            {
                return candidate;
            }
        }
        throw new BridgeUnavailableException("YMM4 PreviewViewModel is unavailable");
    }

    private static int? ReadPreviewFrame(object preview, uint fps)
    {
        foreach (var name in new[] { "CurrentFrame", "Frame", "Position" })
        {
            var value = GetMember(preview, name);
            if (value is TimeSpan time)
            {
                return checked((int)Math.Round(time.TotalSeconds * fps));
            }
            if (value is not null && int.TryParse(value.ToString(), out var frame) && frame >= 0)
            {
                return frame;
            }
        }

        var rateValue = GetMember(preview, "CurrentPositionRate");
        if (rateValue is not null
            && double.TryParse(
                rateValue.ToString(),
                System.Globalization.NumberStyles.Float,
                System.Globalization.CultureInfo.InvariantCulture,
                out var rate))
        {
            var main = RequireMainViewModel();
            var timeline = GetMember(main, "ActiveTimelineViewModel");
            if (timeline is not null)
            {
                var totalFrames = ReadItems(timeline)
                    .Select(item => item.Frame + item.Length)
                    .DefaultIfEmpty(0)
                    .Max();
                if (totalFrames > 0)
                {
                    return checked((int)Math.Round(Math.Clamp(rate, 0, 1) * totalFrames));
                }
            }
        }

        var start = GetMember(preview, "StartPosition");
        return start is TimeSpan startTime
            ? checked((int)Math.Round(startTime.TotalSeconds * fps))
            : null;
    }

    /// Reads only a directly exposed playhead/frame value. Composition
    /// observation must not estimate a frame from progress or timeline length,
    /// because that could select the wrong active elements while still matching
    /// the scene fingerprint.
    internal static int? ReadExactPreviewFrame(object preview, uint fps)
    {
        foreach (var name in new[] { "CurrentFrame", "Frame", "Position" })
        {
            var value = GetMember(preview, name);
            if (value is TimeSpan time)
            {
                return checked((int)Math.Round(time.TotalSeconds * fps));
            }
            if (value is not null && int.TryParse(value.ToString(), out var frame) && frame >= 0)
            {
                return frame;
            }
        }
        return null;
    }

    private static bool ReadProjectDirty()
    {
        var main = RequireMainViewModel();
        var mainModel = RequireMainModel(main);
        var savedSignals = new List<bool>();
        foreach (var (source, name) in new[]
                 {
                     (main, "IsSaved"),
                     (main, "IsProjectFileSaved"),
                     (mainModel, "IsSaved"),
                     (mainModel, "IsProjectFileSaved"),
                 })
        {
            var value = GetMember(source, name);
            if (value is bool saved)
            {
                savedSignals.Add(saved);
            }
            else if (value is not null && bool.TryParse(value.ToString(), out saved))
            {
                savedSignals.Add(saved);
            }
        }
        return SavedSignalsIndicateDirty(savedSignals);
    }

    internal static bool SavedSignalsIndicateDirty(IReadOnlyList<bool> savedSignals)
    {
        if (savedSignals.Count == 0)
        {
            throw new BridgeUnavailableException("YMM4 project dirty state is unavailable");
        }
        return savedSignals.Any(saved => !saved);
    }

    private static string ReadSelectionDigest()
    {
        var main = RequireMainViewModel();
        var timeline = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var items = GetMember(timeline, "Items") as IEnumerable;
        if (items is null)
        {
            throw new BridgeUnavailableException("YMM4 timeline selection state is unavailable");
        }
        var canonical = new StringBuilder();
        foreach (var selectedValue in items.Cast<object?>().Where(value => value is not null))
        {
            var wrapper = selectedValue!;
            var selected = GetMember(wrapper, "IsSelected");
            if (selected is not bool isSelected || !isSelected)
            {
                continue;
            }
            var item = GetMember(wrapper, "Item") ?? wrapper;
            canonical.Append(item.GetType().FullName)
                .Append('|').Append(GetRequiredInt(item, wrapper, "Frame"))
                .Append('|').Append(GetRequiredInt(item, wrapper, "Layer"))
                .Append('|').Append(GetRequiredInt(item, wrapper, "Length"))
                .Append('|').Append(GetString(item, "Remark"))
                .Append('\n');
        }
        return Hash(canonical.ToString());
    }

    private static async Task SeekPreviewToFrameAsync(object preview, int frame, uint fps)
    {
        var task = await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            var methods = preview.GetType()
                .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                .Where(method => method.Name == "SeekAsync" && method.GetParameters().Length == 1)
                .ToArray();
            var method = methods.FirstOrDefault(candidate =>
                    candidate.GetParameters()[0].ParameterType == typeof(int))
                ?? methods.FirstOrDefault(candidate =>
                    candidate.GetParameters()[0].ParameterType == typeof(TimeSpan))
                ?? throw new BridgeUnavailableException("YMM4 PreviewViewModel.SeekAsync is unavailable");
            var argument = method.GetParameters()[0].ParameterType == typeof(int)
                ? (object)frame
                : TimeSpan.FromSeconds(frame / (double)fps);
            try
            {
                return method.Invoke(preview, [argument]) as Task;
            }
            catch (TargetInvocationException error)
            {
                throw error.InnerException ?? error;
            }
        });
        if (task is not null)
        {
            await task.ConfigureAwait(false);
        }
    }

    private static async Task SavePreviewImageAsync(object preview, string path, bool alpha)
    {
        var task = await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            var method = preview.GetType()
                .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                .SingleOrDefault(candidate =>
                    candidate.Name == "SaveImage"
                    && candidate.GetParameters() is
                    [
                        { ParameterType: var pathType },
                        { ParameterType: var alphaType },
                    ]
                    && pathType == typeof(string)
                    && alphaType == typeof(bool))
                ?? throw new BridgeUnavailableException("YMM4 PreviewViewModel.SaveImage is unavailable");
            try
            {
                return method.Invoke(preview, [path, alpha]) as Task;
            }
            catch (TargetInvocationException error)
            {
                throw error.InnerException ?? error;
            }
        });
        if (task is not null)
        {
            await task.ConfigureAwait(false);
        }
    }

    private static async Task<byte[]> ReadCompletedCaptureAsync(string path)
    {
        for (var attempt = 0; attempt < 40; attempt++)
        {
            try
            {
                if (File.Exists(path))
                {
                    var bytes = await File.ReadAllBytesAsync(path).ConfigureAwait(false);
                    if (bytes.Length >= 24)
                    {
                        return bytes;
                    }
                }
            }
            catch (IOException) when (attempt < 39)
            {
                // The native writer may still hold the file for a short time.
            }
            await Task.Delay(25).ConfigureAwait(false);
        }
        throw new BridgeUnavailableException("YMM4 did not finish writing the captured PNG");
    }

    private static (int Width, int Height) ReadPngDimensions(byte[] bytes)
    {
        ReadOnlySpan<byte> signature = [137, 80, 78, 71, 13, 10, 26, 10];
        if (bytes.Length < 24 || !bytes.AsSpan(0, 8).SequenceEqual(signature))
        {
            throw new BridgeUnavailableException("YMM4 scene capture is not a valid PNG");
        }
        var width = ReadBigEndianInt32(bytes.AsSpan(16, 4));
        var height = ReadBigEndianInt32(bytes.AsSpan(20, 4));
        if (width <= 0 || height <= 0)
        {
            throw new BridgeUnavailableException("YMM4 scene capture has invalid dimensions");
        }
        return (width, height);
    }

    private static int ReadBigEndianInt32(ReadOnlySpan<byte> value)
    {
        return (value[0] << 24) | (value[1] << 16) | (value[2] << 8) | value[3];
    }

    private SceneCaptureReceiptDto ReadAndValidateSceneCaptureReceipt(
        string path,
        SceneCaptureRequestDto request)
    {
        SceneCaptureReceiptDto receipt;
        try
        {
            receipt = System.Text.Json.JsonSerializer.Deserialize<SceneCaptureReceiptDto>(
                File.ReadAllText(path),
                BridgeJson.Options)
                ?? throw new InvalidDataException("Scene capture receipt is empty");
        }
        catch (Exception error) when (error is IOException or System.Text.Json.JsonException)
        {
            throw new BridgeUnavailableException(
                $"Scene capture receipt cannot be trusted: {error.Message}");
        }
        if (receipt.OperationId != request.OperationId
            || !string.Equals(receipt.RequestDigest, request.RequestDigest, StringComparison.Ordinal)
            || !string.Equals(receipt.ProjectId, request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(receipt.SceneId, request.SceneId, StringComparison.Ordinal)
            || receipt.SourceRevision != request.SourceRevision
            || !string.Equals(
                receipt.ExpectedFingerprint,
                request.ExpectedFingerprint,
                StringComparison.Ordinal)
            || !string.Equals(
                receipt.CaptureProfileDigest,
                request.CaptureProfileDigest,
                StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Scene capture operation ID is already bound to another request",
                Snapshot().Fingerprint);
        }
        if (receipt.Status is "failed" or "recovery_required")
        {
            throw new BridgeUnavailableException(
                $"Scene capture {receipt.Status}: {receipt.Error ?? "no failure detail was persisted"}");
        }
        if (receipt.Frames.Count != request.Frames.Count)
        {
            throw new BridgeUnavailableException("Scene capture receipt has an incomplete frame set");
        }
        foreach (var (frame, requestedFrame) in receipt.Frames.Zip(request.Frames))
        {
            if (frame.RequestedFrame != requestedFrame || !File.Exists(frame.Path))
            {
                throw new BridgeUnavailableException("Scene capture artifact is missing or mismatched");
            }
            var actualHash = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(frame.Path)));
            if (!ApplyRequestDigest.Matches(frame.Sha256, actualHash))
            {
                throw new BridgeUnavailableException("Scene capture artifact hash does not match its receipt");
            }
        }
        return receipt;
    }

    internal static string DescribeSceneCaptureFailure(
        Exception? captureError,
        Exception? restoreError,
        Exception? restorationReadbackError,
        bool transientStateRestored,
        bool dirtyStateRestored)
    {
        var failures = new List<string>();
        if (captureError is not null)
        {
            failures.Add($"capture failed: {captureError.GetBaseException().Message}");
        }
        if (restoreError is not null)
        {
            failures.Add($"playhead restore failed: {restoreError.GetBaseException().Message}");
        }
        if (restorationReadbackError is not null)
        {
            failures.Add(
                $"restoration read-back failed: {restorationReadbackError.GetBaseException().Message}");
        }
        if (!transientStateRestored)
        {
            failures.Add("preview position or selection restoration was not verified");
        }
        if (!dirtyStateRestored)
        {
            failures.Add("project dirty-state restoration was not verified");
        }
        return failures.Count > 0
            ? string.Join("; ", failures)
            : "scene capture failed without a diagnostic";
    }

    private static void WriteSceneCaptureReceipt(string path, SceneCaptureReceiptDto receipt)
    {
        var temporary = $"{path}.tmp-{Guid.NewGuid():N}";
        File.WriteAllText(
            temporary,
            System.Text.Json.JsonSerializer.Serialize(receipt, BridgeJson.Options),
            new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
        File.Move(temporary, path, overwrite: true);
    }

    private static void ValidateApplyRequest(ApplyRequestDto request)
    {
        if (request.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint))
        {
            throw new BridgeValidationException(
                "Apply requires operationId, requestDigest, projectId, sceneId, and expectedFingerprint");
        }

        var calculated = ApplyRequestDigest.Compute(request);
        if (!ApplyRequestDigest.Matches(request.RequestDigest, calculated))
        {
            throw new BridgeValidationException("Apply request digest does not match its payload");
        }
    }

    private static void ValidateSceneCaptureRequest(SceneCaptureRequestDto request)
    {
        if (request.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint)
            || string.IsNullOrWhiteSpace(request.CaptureProfileDigest))
        {
            throw new BridgeValidationException(
                "Scene capture requires operationId, requestDigest, projectId, sceneId, expectedFingerprint, and captureProfileDigest");
        }
        if (request.Frames.Count is < 1 or > 64
            || request.Frames.Any(frame => frame < 0)
            || request.Frames.Distinct().Count() != request.Frames.Count
            || !request.Frames.SequenceEqual(request.Frames.Order()))
        {
            throw new BridgeValidationException(
                "Scene capture frames must contain 1-64 distinct non-negative values in ascending order");
        }
        var canonical = ApplyRequestDigest.Compute(request);
        if (!ApplyRequestDigest.Matches(request.RequestDigest, canonical))
        {
            throw new BridgeValidationException(
                "Scene capture request digest does not match the canonical request");
        }
    }

    private static void ValidateNativeVoiceMutationApplyRequest(
        NativeVoiceMutationApplyRequestDto request)
    {
        if (request.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint))
        {
            throw new BridgeValidationException(
                "Native voice mutation apply requires operationId, requestDigest, projectId, sceneId, and expectedFingerprint");
        }
        var canonical = ApplyRequestDigest.Compute(request);
        if (!ApplyRequestDigest.Matches(request.RequestDigest, canonical))
        {
            throw new BridgeValidationException(
                "Native voice mutation request digest does not match its payload");
        }
    }

    private static void ValidateNativeVoiceMutations(
        IReadOnlyList<NativeVoiceMutationDto> mutations)
    {
        if (mutations.Count is < 1 or > 128
            || mutations.Select(value => value.RealizationId).Distinct().Count() != mutations.Count
            || mutations.Select(value => value.EntityId).Distinct(StringComparer.Ordinal).Count()
                != mutations.Count)
        {
            throw new BridgeValidationException(
                "Native voice mutations require 1-128 unique entity and realization IDs");
        }
        foreach (var mutation in mutations)
        {
            if (mutation.RealizationId == Guid.Empty
                || string.IsNullOrWhiteSpace(mutation.EntityId)
                || mutation.Action is not ("create" or "update" or "delete")
                || mutation.Frame < 0
                || mutation.Layer < 0
                || mutation.MaxLength <= 0)
            {
                throw new BridgeValidationException("Native voice mutation fields are invalid");
            }
            if (mutation.Action != "delete"
                && (string.IsNullOrWhiteSpace(mutation.CharacterName)
                    || string.IsNullOrWhiteSpace(mutation.DisplayText)
                    || string.IsNullOrWhiteSpace(mutation.SpokenText)
                    || !string.Equals(
                        mutation.DisplayText,
                        mutation.SpokenText,
                        StringComparison.Ordinal)))
            {
                throw new BridgeValidationException(
                    "Native voice create/update requires exact character, non-empty text, and equal display/spoken text");
            }
        }
    }

    private static void ValidateNativeVoiceApplyRequest(NativeVoiceApplyRequestDto request)
    {
        if (request.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint))
        {
            throw new BridgeValidationException(
                "Native voice apply requires operationId, requestDigest, projectId, sceneId, and expectedFingerprint");
        }
        var calculated = ApplyRequestDigest.Compute(request);
        if (!ApplyRequestDigest.Matches(request.RequestDigest, calculated))
        {
            throw new BridgeValidationException(
                "Native voice apply request digest does not match its payload");
        }
    }

    private static void ValidateNativeVoiceCues(IReadOnlyList<NativeVoiceCueDto> cues)
    {
        if (cues.Count == 0)
        {
            throw new BridgeValidationException("At least one native voice cue is required");
        }
        if (cues.Select(value => value.EntityId).Distinct(StringComparer.Ordinal).Count() != cues.Count
            || cues.Select(value => value.RealizationId).Distinct().Count() != cues.Count)
        {
            throw new BridgeValidationException(
                "Native voice entity IDs and realization IDs must be unique");
        }
        foreach (var cue in cues)
        {
            if (cue.RealizationId == Guid.Empty
                || string.IsNullOrWhiteSpace(cue.EntityId)
                || string.IsNullOrWhiteSpace(cue.CharacterName)
                || string.IsNullOrWhiteSpace(cue.DisplayText)
                || string.IsNullOrWhiteSpace(cue.SpokenText))
            {
                throw new BridgeValidationException("Native voice cue fields must not be empty");
            }
            if (!string.Equals(cue.DisplayText, cue.SpokenText, StringComparison.Ordinal))
            {
                throw new BridgeValidationException(
                    "This YMM4 driver cannot yet separate native voice display text from spoken text; use portable audio/caption fallback");
            }
            if (cue.Frame < 0 || cue.Layer < 0 || cue.MaxLength <= 0)
            {
                throw new BridgeValidationException(
                    "Native voice frame/layer must be non-negative and maxLength must be positive");
            }
        }
    }

    private static void EnsureNativeVoiceEntitiesAreNew(
        IReadOnlyList<NativeVoiceCueDto> cues,
        ProjectSnapshotDto snapshot)
    {
        var entityIds = cues.Select(value => value.EntityId).ToHashSet(StringComparer.Ordinal);
        var realizationIds = cues
            .Select(value => value.RealizationId.ToString("D"))
            .ToHashSet(StringComparer.OrdinalIgnoreCase);
        if (snapshot.ManagedItems.Any(item =>
                entityIds.Contains(item.EntityId)
                || item.RealizationId is not null && realizationIds.Contains(item.RealizationId))
            || snapshot.NativeExtensions.Any(item =>
                entityIds.Contains(item.EntityId)
                || realizationIds.Contains(item.RealizationId.ToString("D"))))
        {
            throw new BridgeConflictException(
                "Native voice v2 currently supports create-only cues; an approved identity already exists",
                snapshot.Fingerprint);
        }
    }

    private static void EnsureTarget(ApplyRequestDto request, ProjectSnapshotDto snapshot)
    {
        EnsureTarget(request.ProjectId, request.SceneId, snapshot);
    }

    private static void EnsureTarget(
        string projectId,
        string sceneId,
        ProjectSnapshotDto snapshot)
    {
        if (!string.Equals(projectId, snapshot.ProjectId, StringComparison.Ordinal)
            || !string.Equals(sceneId, snapshot.SceneId, StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "The active YMM4 project or scene is not the approved target",
                snapshot.Fingerprint);
        }
    }

    private static void EnsureReceiptBinding(
        ApplyRequestDto request,
        OperationReceiptDto receipt,
        string actualFingerprint)
    {
        EnsureReceiptBinding(
            request.RequestDigest,
            request.ProjectId,
            request.SceneId,
            request.ExpectedFingerprint,
            receipt,
            actualFingerprint);
    }

    private static void EnsureReceiptBinding(
        string requestDigest,
        string projectId,
        string sceneId,
        string expectedFingerprint,
        OperationReceiptDto receipt,
        string actualFingerprint)
    {
        if (!ApplyRequestDigest.Matches(requestDigest, receipt.RequestDigest)
            || !string.Equals(projectId, receipt.ProjectId, StringComparison.Ordinal)
            || !string.Equals(sceneId, receipt.SceneId, StringComparison.Ordinal)
            || !string.Equals(
                expectedFingerprint,
                receipt.ExpectedFingerprint,
                StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Operation ID was already used for a different approved request",
                actualFingerprint);
        }
    }

    private static void ValidateUtterances(
        IReadOnlyList<ManagedUtteranceDto> utterances,
        bool verifyArtifacts)
    {
        if (utterances.Count == 0)
        {
            throw new BridgeValidationException("At least one managed utterance is required");
        }
        if (utterances.Select(value => value.EntityId).Distinct(StringComparer.Ordinal).Count() != utterances.Count)
        {
            throw new BridgeValidationException("Managed utterance entity IDs must be unique");
        }
        foreach (var utterance in utterances)
        {
            if (string.IsNullOrWhiteSpace(utterance.EntityId)
                || string.IsNullOrWhiteSpace(utterance.Caption)
                || string.IsNullOrWhiteSpace(utterance.SpokenText ?? utterance.Caption)
                || string.IsNullOrWhiteSpace(utterance.AudioPath)
                || string.IsNullOrWhiteSpace(utterance.ArtifactHash))
            {
                throw new BridgeValidationException("Managed utterance fields must not be empty");
            }
            if (utterance.Frame < 0 || utterance.Length <= 0 || utterance.AudioLayer < 0 || utterance.CaptionLayer < 0)
            {
                throw new BridgeValidationException("Frame/layer must be non-negative and length must be positive");
            }
            if (!verifyArtifacts)
            {
                continue;
            }
            var fullPath = Path.GetFullPath(utterance.AudioPath);
            if (!File.Exists(fullPath))
            {
                throw new BridgeValidationException($"Audio artifact does not exist: {fullPath}");
            }
            using var stream = File.OpenRead(fullPath);
            var actualHash = Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant();
            if (!actualHash.Equals(utterance.ArtifactHash, StringComparison.OrdinalIgnoreCase))
            {
                throw new BridgeValidationException(
                    $"Audio artifact hash mismatch: expected {utterance.ArtifactHash}, got {actualHash}");
            }
        }
    }

    private static void EnsureFingerprint(string expected, string actual)
    {
        if (!expected.Equals(actual, StringComparison.Ordinal))
        {
            throw new BridgeConflictException("YMM4 changed after preview", actual);
        }
    }

    private static void SetRequired(object target, object value, params string[] names)
    {
        var flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance;
        foreach (var name in names)
        {
            var property = target.GetType().GetProperty(name, flags);
            if (property is null)
            {
                continue;
            }
            try
            {
                var current = property.GetValue(target);
                var valueProperty = current?.GetType().Name.Contains("ReactiveProperty", StringComparison.Ordinal) == true
                    ? current.GetType().GetProperty("Value")
                    : null;
                if (valueProperty?.CanWrite == true)
                {
                    valueProperty.SetValue(current, ConvertValue(value, valueProperty.PropertyType));
                    return;
                }
                if (property.CanWrite)
                {
                    property.SetValue(target, ConvertValue(value, property.PropertyType));
                    return;
                }
            }
            catch (Exception error)
            {
                throw new BridgeUnavailableException(
                    $"Unable to set {target.GetType().Name}.{name}: {error.GetBaseException().Message}");
            }
        }
        throw new BridgeUnavailableException(
            $"YMM4 item property is unavailable: {target.GetType().Name}.({string.Join('/', names)})");
    }

    private static object? ConvertValue(object value, Type targetType)
    {
        var effectiveType = Nullable.GetUnderlyingType(targetType) ?? targetType;
        if (effectiveType.IsInstanceOfType(value))
        {
            return value;
        }
        if (effectiveType.IsEnum)
        {
            return Enum.Parse(effectiveType, value.ToString()!, true);
        }
        return Convert.ChangeType(value, effectiveType);
    }

    internal sealed record RawItem(
        object Item,
        int Frame,
        int Layer,
        int Length,
        int GroupId,
        string Text,
        string AudioPath,
        string Remark,
        string CharacterName,
        string SpokenText,
        string TypeName,
        bool Selected,
        bool SelectionAvailable);

    private sealed record AppliedBatch(
        object Timeline,
        object[] OldItems,
        object[] NewItems);

    private sealed class NativeVoicePreparation(
        object mainModel,
        object timelineViewModel,
        object timeline,
        MethodInfo addMethod,
        object decorations,
        IReadOnlyList<NativeVoiceCueDto> cues,
        IReadOnlyDictionary<Guid, object> characters,
        HashSet<object> knownItems,
        List<object>? addedItems = null)
    {
        internal object MainModel { get; } = mainModel;
        internal object TimelineViewModel { get; } = timelineViewModel;
        internal object Timeline { get; } = timeline;
        internal MethodInfo AddMethod { get; } = addMethod;
        internal object Decorations { get; } = decorations;
        internal IReadOnlyList<NativeVoiceCueDto> Cues { get; } = cues;
        internal IReadOnlyDictionary<Guid, object> Characters { get; } = characters;
        internal HashSet<object> KnownItems { get; } = knownItems;
        internal List<object> AddedItems { get; } = addedItems ?? [];
    }

    private sealed class NativeVoiceMutationPreparation(
        object mainModel,
        object timelineViewModel,
        object timeline,
        MethodInfo addMethod,
        object decorations,
        IReadOnlyList<NativeVoiceMutationDto> mutations,
        IReadOnlyDictionary<Guid, object> characters,
        IReadOnlyDictionary<Guid, RawItem> originals,
        IReadOnlyDictionary<Guid, string> preservedStateDigests,
        HashSet<object> knownItems)
    {
        internal object MainModel { get; } = mainModel;
        internal object TimelineViewModel { get; } = timelineViewModel;
        internal object Timeline { get; } = timeline;
        internal MethodInfo AddMethod { get; } = addMethod;
        internal object Decorations { get; } = decorations;
        internal IReadOnlyList<NativeVoiceMutationDto> Mutations { get; } = mutations;
        internal IReadOnlyDictionary<Guid, object> Characters { get; } = characters;
        internal IReadOnlyDictionary<Guid, RawItem> Originals { get; } = originals;
        internal IReadOnlyDictionary<Guid, string> PreservedStateDigests { get; } = preservedStateDigests;
        internal HashSet<object> KnownItems { get; } = knownItems;
        internal List<object> AddedItems { get; } = [];
        internal List<object> DeletedItems { get; } = [];
    }
}

internal sealed class BridgeValidationException(string message) : Exception(message);
internal sealed class BridgeUnavailableException(string message) : Exception(message);
internal sealed class BridgeNotFoundException(string message) : Exception(message);
internal sealed class RenderSourceDriftException(string message) : Exception(message);
internal sealed class BridgeConflictException(string message, string actualFingerprint) : Exception(message)
{
    internal string ActualFingerprint { get; } = actualFingerprint;
}
