using System.Reflection;
using System.Text.Json;
using System.Windows;

namespace TakeGraph.Ymm4Bridge;

internal sealed partial class Ymm4Facade
{
    private const string TargetPlanDomain = "takegraph-target-plan";

    internal TargetPlanValidationDto ValidateTargetPlan(TargetPlanRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        return ParseAndValidateTargetPlan(
            request.TargetPlanDigest,
            request.TargetPlan,
            validateCurrentScope: true).Validation;
    }

    internal async Task<ApplyResponseDto> ApplyTargetPlanAsync(
        TargetPlanApplyRequestDto request)
    {
        ValidateTargetPlanApplyRequestShape(request);
        var operationId = RequireJsonGuid(request.TargetPlan, "operationId");
        var hasReceipt = receiptStore.TryGet(operationId, out var existingReceipt)
            && existingReceipt is not null;
        if (existingReceipt is not null
            && existingReceipt.Status != "applying")
        {
            var binding = ParseTargetPlanBinding(request);
            EnsureReceiptBinding(
                request.RequestDigest,
                binding.ProjectId,
                binding.SceneId,
                request.ExpectedFingerprint,
                existingReceipt,
                existingReceipt.AfterFingerprint);
            return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
        }
        if (!hasReceipt)
        {
            ValidateMutationRuntime();
        }
        var parsed = ParseAndValidateTargetPlan(
            request.TargetPlanDigest,
            request.TargetPlan,
            validateCurrentScope: !hasReceipt,
            validateCurrentCapability: !hasReceipt,
            historicalFingerprint: existingReceipt?.ExpectedFingerprint);
        if (!hasReceipt)
        {
            EnsureFingerprint(request.ExpectedFingerprint, parsed.Validation.Fingerprint);
        }
        var expectedFingerprint = existingReceipt?.ExpectedFingerprint
            ?? parsed.Validation.Fingerprint;

        if (parsed.PortableUtterances.Count > 0)
        {
            var physical = new ApplyRequestDto(
                BridgeContract.ProtocolVersion,
                operationId,
                request.RequestDigest,
                parsed.ProjectId,
                parsed.SceneId,
                expectedFingerprint,
                parsed.PortableUtterances);
            return await ApplyAsync(physical, sealedTargetPlan: true).ConfigureAwait(false);
        }
        var native = new NativeVoiceApplyRequestDto(
            BridgeContract.ProtocolVersion,
            operationId,
            request.RequestDigest,
            parsed.ProjectId,
            parsed.SceneId,
            expectedFingerprint,
            parsed.NativeVoiceCues);
        return await ApplyNativeVoiceAsync(native, sealedTargetPlan: true).ConfigureAwait(false);
    }

    internal async Task<ApplyResponseDto> SealTargetPlanNotStartedAsync(
        TargetPlanApplyRequestDto request)
    {
        ValidateTargetPlanApplyRequestShape(request);
        var operationId = RequireJsonGuid(request.TargetPlan, "operationId");
        var binding = ParseTargetPlanBinding(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (receiptStore.TryGet(operationId, out var existingReceipt)
                && existingReceipt is not null)
            {
                EnsureReceiptBinding(
                    request.RequestDigest,
                    binding.ProjectId,
                    binding.SceneId,
                    request.ExpectedFingerprint,
                    existingReceipt,
                    existingReceipt.AfterFingerprint);
                return new ApplyResponseDto(existingReceipt.Verified, true, existingReceipt);
            }

            // Sealing is a same-gate journal write, not target authorization.
            // Current project, content, runtime, and capability drift cannot
            // prevent proving that this operation itself never started.
            var tombstone = CreateNotStartedReceipt(
                operationId,
                request.RequestDigest,
                binding.ProjectId,
                binding.SceneId,
                request.ExpectedFingerprint,
                "Target-plan apply did not start; a durable no-mutation tombstone was sealed");
            receiptStore.Put(tombstone);
            return new ApplyResponseDto(false, false, tombstone);
        }
        finally
        {
            applyGate.Release();
        }
    }

    private static void ValidateTargetPlanApplyRequestShape(TargetPlanApplyRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        if (string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint))
        {
            throw new BridgeValidationException(
                "Target-plan apply requires requestDigest and expectedFingerprint");
        }
        var calculatedRequestDigest = ApplyRequestDigest.Compute(request);
        if (!ApplyRequestDigest.Matches(request.RequestDigest, calculatedRequestDigest))
        {
            throw new BridgeValidationException(
                "Target-plan request digest does not match its canonical payload");
        }
        RequireExactJsonProperties(
            request.TargetPlan,
            "canonicalVersion",
            "operationId",
            "baseRevision",
            "target",
            "capabilityDigest",
            "expectedScope",
            "changeBudget",
            "cues",
            "warnings");
    }

    private ParsedTargetPlan ParseAndValidateTargetPlan(
        string declaredDigest,
        JsonElement root,
        bool validateCurrentScope,
        bool validateCurrentCapability = true,
        string? historicalFingerprint = null)
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
            "cues",
            "warnings");
        if (RequireJsonInt(root, "canonicalVersion") != 1)
        {
            throw new BridgeValidationException("Unsupported target-plan canonicalVersion");
        }
        var operationId = RequireJsonGuid(root, "operationId");
        if (operationId == Guid.Empty)
        {
            throw new BridgeValidationException("Target-plan operationId must not be empty");
        }
        _ = RequireJsonULong(root, "baseRevision");
        var targetPlanDigest = RequireSha256(declaredDigest, "targetPlanDigest");
        var calculatedPlanDigest = CanonicalJson.Sha256(TargetPlanDomain, root);
        if (!ApplyRequestDigest.Matches(targetPlanDigest, calculatedPlanDigest))
        {
            throw new BridgeValidationException(
                "Target-plan digest does not match its canonical payload");
        }

        var snapshot = validateCurrentScope || validateCurrentCapability
            ? Snapshot()
            : null;
        var evidenceFingerprint = snapshot?.Fingerprint
            ?? historicalFingerprint
            ?? throw new BridgeValidationException(
                "Historical target-plan parsing requires its sealed fingerprint");
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
        if (validateCurrentCapability
            && (!string.Equals(RequireJsonString(target, "adapterId"), "ymm4-4.55", StringComparison.Ordinal)
                || !string.Equals(projectId, snapshot!.ProjectId, StringComparison.Ordinal)
                || !string.Equals(sceneId, snapshot.SceneId, StringComparison.Ordinal)
                || RequireJsonInt(target, "fps") != snapshot.Fps
                || !string.Equals(
                    RequireJsonString(target, "driverVersion"),
                    expectedDriverVersion,
                    StringComparison.Ordinal)))
        {
            throw new BridgeConflictException(
                "Target-plan target identity differs from the active YMM4 target",
                evidenceFingerprint);
        }

        _ = RequireSha256(RequireJsonString(root, "capabilityDigest"), "capabilityDigest");
        if (validateCurrentCapability)
        {
            var rawCapabilities = Capabilities().Capabilities.ToHashSet(StringComparer.Ordinal);
            var currentCapabilityDigest = ComputeStructuredCapabilityDigest(
                rawCapabilities,
                pluginVersion,
                ymm4Version);
            if (!ApplyRequestDigest.Matches(
                    RequireJsonString(root, "capabilityDigest"),
                    currentCapabilityDigest))
            {
                throw new BridgeConflictException(
                    "Target-plan capability contract changed after approval",
                    evidenceFingerprint);
            }
        }
        ValidateTargetPlanScope(
            root,
            target,
            snapshot,
            evidenceFingerprint,
            validateCurrentScope);

        var budget = RequireJsonObject(root, "changeBudget");
        RequireExactJsonProperties(
            budget,
            "maxChangedEntities",
            "maxShiftedEntities",
            "maxShiftFrames",
            "allowLockedChanges",
            "allowUnmanagedChanges");
        var maxChangedEntities = RequirePositiveJsonInt(budget, "maxChangedEntities");
        if (RequireNonNegativeJsonInt(budget, "maxShiftedEntities") != 0
            || RequireNonNegativeJsonInt(budget, "maxShiftFrames") != 0
            || RequireJsonBoolean(budget, "allowLockedChanges")
            || RequireJsonBoolean(budget, "allowUnmanagedChanges"))
        {
            throw new BridgeValidationException(
                "The current target-plan driver accepts only fixed, locked-safe, managed-only plans");
        }

        var cueValues = RequireJsonArray(root, "cues").EnumerateArray().ToArray();
        if (cueValues.Length is < 1 or > 128 || cueValues.Length > maxChangedEntities)
        {
            throw new BridgeValidationException(
                "Target-plan cues must fit the approved 1-128 entity change budget");
        }
        var portable = new List<ManagedUtteranceDto>();
        var native = new List<NativeVoiceCueDto>();
        var actions = new List<string>();
        var entityIds = new HashSet<string>(StringComparer.Ordinal);
        var realizationIds = new HashSet<Guid>();
        foreach (var cue in cueValues)
        {
            ParseTargetPlanCue(
                cue,
                portable,
                native,
                actions,
                entityIds,
                realizationIds,
                validateCurrentCapability);
        }
        if (portable.Count > 0 && native.Count > 0)
        {
            throw new BridgeValidationException(
                "One target plan cannot mix portable_pair and native_voice transactions");
        }
        foreach (var warning in RequireJsonArray(root, "warnings").EnumerateArray())
        {
            RequireExactJsonProperties(warning, "code", "message");
            _ = RequireJsonString(warning, "code");
            _ = RequireJsonString(warning, "message");
        }

        if (portable.Count > 0)
        {
            if (validateCurrentScope)
            {
                var portableEntityIds = portable
                    .Select(value => value.EntityId)
                    .ToHashSet(StringComparer.Ordinal);
                if (snapshot!.NativeExtensions.Any(value => portableEntityIds.Contains(value.EntityId)))
                {
                    throw new BridgeConflictException(
                        "Portable target-plan identity collides with a managed native extension",
                        evidenceFingerprint);
                }
                ValidatePortableTargetPlanActions(
                    portable,
                    actions,
                    snapshot.ManagedItems,
                    evidenceFingerprint);
            }
            // A receipt-bound replay must reach ApplyAsync before consulting the
            // caller's original staging path. New operations still verify and
            // materialize the source while current scope is being authorized;
            // pending operations reuse the bridge-owned content-addressed copy.
            ValidateUtterances(portable, verifyArtifacts: validateCurrentScope);
            if (validateCurrentScope)
            {
                Application.Current.Dispatcher.Invoke(() => PrepareApplyBatch(portable));
            }
        }
        else
        {
            ValidateNativeVoiceCues(native);
            if (validateCurrentScope)
            {
                EnsureNativeVoiceEntitiesAreNew(native, snapshot!);
                Application.Current.Dispatcher.Invoke(() =>
                    PrepareNativeVoiceBatch(native, projectId));
            }
        }
        var strategyCounts = new SortedDictionary<string, int>(StringComparer.Ordinal);
        if (portable.Count > 0)
        {
            strategyCounts["portable_pair"] = portable.Count;
        }
        if (native.Count > 0)
        {
            strategyCounts["native_voice"] = native.Count;
        }
        var validation = new TargetPlanValidationDto(
            operationId,
            targetPlanDigest,
            evidenceFingerprint,
            strategyCounts,
            actions.Count(value => value == "create"),
            actions.Count(value => value == "update"),
            actions.Count(value => value == "delete"),
            portable.Count * 2 + native.Count);
        return new ParsedTargetPlan(projectId, sceneId, portable, native, validation);
    }

    private static TargetPlanBinding ParseTargetPlanBinding(TargetPlanApplyRequestDto request)
    {
        var target = RequireJsonObject(request.TargetPlan, "target");
        return new TargetPlanBinding(
            RequireJsonString(target, "projectId"),
            RequireJsonString(target, "sceneId"));
    }

    private static void ValidateTargetPlanScope(
        JsonElement root,
        JsonElement target,
        ProjectSnapshotDto? snapshot,
        string evidenceFingerprint,
        bool validateCurrentScope)
    {
        var scope = RequireJsonObject(root, "expectedScope");
        RequireExactJsonProperties(
            scope,
            "targetIdentityDigest",
            "managedStateDigest",
            "conflictScopeDigest");
        var targetDigest = CanonicalJson.Sha256("takegraph-ymm4-target-identity", target);
        if (!ApplyRequestDigest.Matches(
                RequireSha256(
                    RequireJsonString(scope, "targetIdentityDigest"),
                    "targetIdentityDigest"),
                targetDigest))
        {
            throw new BridgeConflictException(
                "Target-plan target identity scope changed after approval",
                evidenceFingerprint);
        }
        if (!validateCurrentScope)
        {
            return;
        }
        if (snapshot is null)
        {
            throw new BridgeValidationException(
                "Current target-plan scope validation requires a live snapshot");
        }
        var managedDigest = CanonicalJson.Sha256(
            "takegraph-ymm4-managed-state",
            snapshot.ManagedItems);
        var conflictDigest = CanonicalJson.Sha256(
            "takegraph-ymm4-conflict-scope-v1-whole-scene",
            new
            {
                fingerprint = snapshot.Fingerprint,
                unmanagedContextCount = snapshot.UnmanagedContextCount,
            });
        if (!ApplyRequestDigest.Matches(
                RequireSha256(RequireJsonString(scope, "managedStateDigest"), "managedStateDigest"),
                managedDigest)
            || !ApplyRequestDigest.Matches(
                RequireSha256(
                    RequireJsonString(scope, "conflictScopeDigest"),
                    "conflictScopeDigest"),
                conflictDigest))
        {
            throw new BridgeConflictException(
                "Target-plan managed or conflict scope changed after approval",
                evidenceFingerprint);
        }
    }

    private static void ParseTargetPlanCue(
        JsonElement cue,
        ICollection<ManagedUtteranceDto> portable,
        ICollection<NativeVoiceCueDto> native,
        ICollection<string> actions,
        ISet<string> entityIds,
        ISet<Guid> realizationIds,
        bool validateCurrentCapability = true,
        bool timelineEdit = false)
    {
        RequireExactJsonProperties(
            cue,
            "intent",
            "realizationId",
            "action",
            "strategy",
            "fallback",
            "placement",
            "duration",
            "ownership",
            "capabilityDependencies",
            "bindingDependencies",
            "resolvedRealization");
        var intent = RequireJsonObject(cue, "intent");
        RequireExactJsonProperties(
            intent,
            "entityId",
            "entityRevision",
            "displayText",
            "spokenText",
            "speakerRole",
            "voiceProfile",
            "captionStyle",
            "segmentationLocked",
            "placement",
            "realizationPreference",
            "fallbackPolicy",
            "acceptedPortableTake",
            "template",
            "effects",
            "hardLockPreconditions");
        var entityId = RequireJsonString(intent, "entityId");
        var entityRevision = RequireJsonULong(intent, "entityRevision");
        var displayText = RequireJsonString(intent, "displayText");
        var spokenText = RequireOptionalJsonString(intent, "spokenText");
        var speakerRole = RequireJsonString(intent, "speakerRole");
        var voiceProfile = RequireOptionalJsonString(intent, "voiceProfile")
            ?? throw new BridgeValidationException("Target-plan voiceProfile must be resolved");
        RequireJsonNull(intent, "captionStyle");
        if (RequireJsonBoolean(intent, "segmentationLocked"))
        {
            throw new BridgeValidationException(
                "The current target-plan driver cannot mutate segmentation-locked cues");
        }
        ValidatePortableIntentPlacement(RequireJsonObject(intent, "placement"));
        RequireJsonNull(intent, "acceptedPortableTake");
        RequireJsonNull(intent, "template");
        RequireEmptyJsonArray(intent, "effects");
        RequireEmptyJsonArray(intent, "hardLockPreconditions");

        var realizationId = RequireJsonGuid(cue, "realizationId");
        if (realizationId == Guid.Empty
            || !entityIds.Add(entityId)
            || !realizationIds.Add(realizationId))
        {
            throw new BridgeValidationException(
                "Target-plan entity and realization identities must be non-empty and unique");
        }
        var action = RequireJsonString(cue, "action");
        if (action is not ("create" or "update"))
        {
            throw new BridgeValidationException(
                "The current unified cue route supports create/update only");
        }
        actions.Add(action);
        var strategy = RequireJsonString(cue, "strategy");
        RequireJsonNull(cue, "fallback");
        var placement = RequireJsonObject(cue, "placement");
        RequireExactJsonProperties(placement, "frame", "primaryLayer", "secondaryLayer");
        var frame = RequireNonNegativeJsonInt(placement, "frame");
        var primaryLayer = RequireNonNegativeJsonInt(placement, "primaryLayer");
        var secondaryLayer = RequireOptionalNonNegativeJsonInt(placement, "secondaryLayer");
        ValidateAbsoluteIntentMatches(intent, frame);

        var duration = RequireJsonObject(cue, "duration");
        var ownership = RequireJsonObject(cue, "ownership");
        var resolved = RequireJsonObject(cue, "resolvedRealization");
        var bindings = RequireJsonArray(cue, "bindingDependencies").EnumerateArray().ToArray();
        ValidateCueCapabilities(cue, strategy, validateCurrentCapability, timelineEdit);
        if (strategy == "portable_pair")
        {
            if (RequireJsonString(intent, "realizationPreference") != "require_portable"
                || RequireJsonString(intent, "fallbackPolicy") != "reject"
                || voiceProfile != speakerRole
                || secondaryLayer is null
                || secondaryLayer == primaryLayer
                || action is not ("create" or "update"))
            {
                throw new BridgeValidationException(
                    "Portable-pair strategy, text, binding, action, or placement is inconsistent");
            }
            RequireExactJsonProperties(duration, "kind", "frames");
            if (RequireJsonString(duration, "kind") != "exact")
            {
                throw new BridgeValidationException("Portable-pair duration must be exact");
            }
            var length = RequirePositiveJsonInt(duration, "frames");
            if (spokenText is null)
            {
                throw new BridgeValidationException(
                    "Portable pair spokenText must be a non-empty approved synthesis input");
            }
            ValidateOwnership(ownership, portableStrategy: true, spokenBound: true);
            if (bindings.Length != 1)
            {
                throw new BridgeValidationException(
                    "Portable-pair plan requires exactly one audio artifact binding");
            }
            var binding = bindings[0];
            RequireExactJsonProperties(binding, "kind", "id", "digest");
            var artifactHash = RequireJsonString(binding, "id");
            var artifactDigest = RequireSha256(
                RequireJsonString(binding, "digest"),
                "audio artifact binding digest");
            if (RequireJsonString(binding, "kind") != "audio_artifact"
                || !ApplyRequestDigest.Matches(artifactDigest, $"sha256:{artifactHash}"))
            {
                throw new BridgeValidationException("Portable audio artifact binding is invalid");
            }
            RequireAllowedJsonProperties(
                resolved,
                "kind",
                "audio_path",
                "audioPath",
                "artifact_digest",
                "artifactDigest");
            var portableAudioPathField = RequireOneJsonAlias(
                resolved,
                "portable audio path",
                "audio_path",
                "audioPath");
            var portableArtifactField = RequireOneJsonAlias(
                resolved,
                "portable artifact digest",
                "artifact_digest",
                "artifactDigest");
            var audioPath = RequireJsonString(resolved, portableAudioPathField);
            if (RequireJsonString(resolved, "kind") != "portable_pair"
                || !ApplyRequestDigest.Matches(
                    RequireSha256(
                        RequireJsonString(resolved, portableArtifactField),
                        "resolved portable artifactDigest"),
                    artifactDigest))
            {
                throw new BridgeValidationException(
                    "Portable resolved realization differs from its artifact binding");
            }
            portable.Add(new ManagedUtteranceDto(
                entityId,
                entityRevision,
                speakerRole,
                displayText,
                audioPath,
                artifactHash,
                frame,
                length,
                primaryLayer,
                secondaryLayer.Value,
                spokenText));
            return;
        }
        if (strategy != "native_voice"
            || action != "create"
            || RequireJsonString(intent, "realizationPreference") != "require_native"
            || RequireJsonString(intent, "fallbackPolicy") != "reject"
            || secondaryLayer is not null)
        {
            throw new BridgeValidationException(
                "Native-voice strategy, text, action, or placement is inconsistent");
        }
        RequireAllowedJsonProperties(duration, "kind", "max_frames", "maxFrames");
        if (RequireJsonString(duration, "kind") != "bounded")
        {
            throw new BridgeValidationException("Native-voice duration must be bounded");
        }
        var nativeMaxFramesField = RequireOneJsonAlias(
            duration,
            "native duration bound",
            "max_frames",
            "maxFrames");
        var maxLength = RequirePositiveJsonInt(duration, nativeMaxFramesField);
        ValidateOwnership(ownership, portableStrategy: false, spokenBound: spokenText is not null);
        if (bindings.Length != 1)
        {
            throw new BridgeValidationException(
                "Native-voice plan requires exactly one character binding");
        }
        var nativeBinding = bindings[0];
        RequireExactJsonProperties(nativeBinding, "kind", "id", "digest");
        var characterName = RequireJsonString(nativeBinding, "id");
        var characterDigest = RequireSha256(
            RequireJsonString(nativeBinding, "digest"),
            "native character binding digest");
        var calculatedCharacterDigest = CanonicalJson.Sha256(
            "takegraph-ymm4-character-name-binding",
            characterName);
        if (RequireJsonString(nativeBinding, "kind") != "character_name_legacy"
            || !ApplyRequestDigest.Matches(characterDigest, calculatedCharacterDigest)
            || voiceProfile != characterName
            || speakerRole != characterName)
        {
            throw new BridgeValidationException("Native character binding is invalid");
        }
        RequireAllowedJsonProperties(
            resolved,
            "kind",
            "character_name",
            "characterName",
            "character_binding_digest",
            "characterBindingDigest");
        var nativeCharacterField = RequireOneJsonAlias(
            resolved,
            "native character field",
            "character_name",
            "characterName");
        var nativeBindingField = RequireOneJsonAlias(
            resolved,
            "native binding digest field",
            "character_binding_digest",
            "characterBindingDigest");
        if (RequireJsonString(resolved, "kind") != "native_voice"
            || RequireJsonString(resolved, nativeCharacterField) != characterName
            || !ApplyRequestDigest.Matches(
                RequireSha256(
                    RequireJsonString(resolved, nativeBindingField),
                    "resolved character binding digest"),
                characterDigest))
        {
            throw new BridgeValidationException(
                "Native resolved realization differs from its character binding");
        }
        native.Add(new NativeVoiceCueDto(
            realizationId,
            entityId,
            entityRevision,
            characterName,
            displayText,
            spokenText,
            frame,
            primaryLayer,
            maxLength));
    }

    internal static void ValidateTargetPlanCueContract(JsonElement cue)
    {
        ParseTargetPlanCue(
            cue,
            new List<ManagedUtteranceDto>(),
            new List<NativeVoiceCueDto>(),
            new List<string>(),
            new HashSet<string>(StringComparer.Ordinal),
            new HashSet<Guid>());
    }

    internal static void ValidateTimelineEditCueContract(JsonElement cue)
    {
        ParseTargetPlanCue(
            cue,
            new List<ManagedUtteranceDto>(),
            new List<NativeVoiceCueDto>(),
            new List<string>(),
            new HashSet<string>(StringComparer.Ordinal),
            new HashSet<Guid>(),
            validateCurrentCapability: false,
            timelineEdit: true);
    }

    internal static void ValidatePortableTargetPlanActions(
        IReadOnlyList<ManagedUtteranceDto> utterances,
        IReadOnlyList<string> actions,
        IReadOnlyList<ManagedItemDto> managedItems,
        string currentFingerprint)
    {
        if (utterances.Count != actions.Count)
        {
            throw new BridgeValidationException(
                "Portable target-plan actions do not match their sealed cues");
        }
        var existingByEntity = managedItems
            .GroupBy(item => item.EntityId, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.ToArray(), StringComparer.Ordinal);
        for (var index = 0; index < utterances.Count; index++)
        {
            var utterance = utterances[index];
            var exists = existingByEntity.TryGetValue(utterance.EntityId, out var existing);
            if (actions[index] == "create")
            {
                if (exists)
                {
                    throw new BridgeConflictException(
                        $"Portable create target already exists: {utterance.EntityId}",
                        currentFingerprint);
                }
                continue;
            }
            if (actions[index] != "update" || !exists || !IsCanonicalPortablePair(existing!))
            {
                throw new BridgeConflictException(
                    $"Portable update requires one canonical audio/caption pair: {utterance.EntityId}",
                    currentFingerprint);
            }
        }
    }

    private static bool IsCanonicalPortablePair(IReadOnlyList<ManagedItemDto> items)
    {
        if (items.Count != 2)
        {
            return false;
        }
        if (items.Count(item => item.Kind == "audio") != 1
            || items.Count(item => item.Kind == "caption") != 1)
        {
            return false;
        }
        var audio = items.First(item => item.Kind == "audio");
        var caption = items.First(item => item.Kind == "caption");
        return audio is not null
            && caption is not null
            && audio.Revision == caption.Revision
            && audio.Frame == caption.Frame
            && audio.Length > 0
            && audio.Length == caption.Length
            && audio.Layer != caption.Layer
            && audio.Text is null
            && !string.IsNullOrWhiteSpace(audio.AudioPath)
            && caption.AudioPath is null
            && !string.IsNullOrWhiteSpace(caption.Text)
            && !string.IsNullOrWhiteSpace(audio.Speaker)
            && string.Equals(audio.Speaker, caption.Speaker, StringComparison.Ordinal)
            && audio.ArtifactHash is { Length: 64 }
            && audio.ArtifactHash.All(Uri.IsHexDigit)
            && string.Equals(audio.ArtifactHash, caption.ArtifactHash, StringComparison.OrdinalIgnoreCase);
    }

    private static void ValidateCueCapabilities(
        JsonElement cue,
        string strategy,
        bool validateCurrentCapability,
        bool timelineEdit = false)
    {
        var expected = (strategy switch
        {
            "portable_pair" => new[]
            {
                "targetPlan.apply",
                "managedPair.apply",
                "timeline.transaction",
                "readback.semantic",
            },
            "native_voice" => new[]
            {
                "targetPlan.apply",
                "voiceItem.create",
                "timeline.transaction",
                "readback.semantic",
            },
            _ => throw new BridgeValidationException($"Unknown target-plan strategy: {strategy}"),
        }).ToList();
        if (timelineEdit)
        {
            expected.Add("timelineEdit.apply");
        }
        var dependencies = RequireJsonArray(cue, "capabilityDependencies")
            .EnumerateArray()
            .ToArray();
        if (dependencies.Length != expected.Count)
        {
            throw new BridgeValidationException(
                "Target-plan capability dependencies are incomplete");
        }
        var actual = new HashSet<string>(StringComparer.Ordinal);
        foreach (var dependency in dependencies)
        {
            RequireExactJsonProperties(
                dependency,
                "feature",
                "minimumVersion",
                "schemaDigest");
            var feature = RequireJsonString(dependency, "feature");
            if (!actual.Add(feature) || RequirePositiveJsonInt(dependency, "minimumVersion") != 1)
            {
                throw new BridgeValidationException(
                    "Target-plan capability dependency is duplicated or has an unsupported version");
            }
            var actualSchema = RequireSha256(
                RequireJsonString(dependency, "schemaDigest"),
                "target-plan capability schemaDigest");
            if (validateCurrentCapability)
            {
                var expectedSchema = StructuredFeatureSchemaDigest(
                    feature,
                    StructuredFeaturePropertyNames(feature));
                if (!ApplyRequestDigest.Matches(actualSchema, expectedSchema))
                {
                    throw new BridgeConflictException(
                        $"Target-plan capability schema changed: {feature}",
                        SnapshotCore().Fingerprint);
                }
            }
        }
        if (!actual.SetEquals(expected))
        {
            throw new BridgeValidationException(
                "Target-plan capability dependencies do not match its strategy");
        }
    }

    private static void ValidateOwnership(
        JsonElement ownership,
        bool portableStrategy,
        bool spokenBound)
    {
        RequireExactJsonProperties(ownership, "strict", "derived", "preserve", "global");
        var strict = portableStrategy
            ? new[] { "identity", "captionText", "audioArtifact", "timing" }
            : spokenBound
                ? new[] { "identity", "displayText", "spokenText", "characterBinding", "timingIntent" }
                : new[] { "identity", "displayText", "characterBinding", "timingIntent" };
        var derived = portableStrategy
            ? Array.Empty<string>()
            : spokenBound
                ? new[] { "length", "voiceCache" }
                : new[] { "length", "pronunciation", "voiceCache" };
        var global = portableStrategy
            ? new[] { "projectSettings" }
            : new[] { "characterDefinitions", "projectSettings" };
        RequireExactStringArray(ownership, "strict", strict);
        RequireExactStringArray(ownership, "derived", derived);
        RequireExactStringArray(ownership, "preserve", ["unknownNativeFields"]);
        RequireExactStringArray(ownership, "global", global);
    }

    private static void ValidatePortableIntentPlacement(JsonElement placement)
    {
        RequireExactJsonProperties(placement, "anchor", "ordering", "trackRole");
        if (RequireJsonString(placement, "ordering") != "fixed"
            || RequireJsonString(placement, "trackRole") != "dialogue")
        {
            throw new BridgeValidationException(
                "The current target-plan route requires fixed dialogue placement");
        }
        var anchor = RequireJsonObject(placement, "anchor");
        RequireExactJsonProperties(anchor, "type", "frame");
        if (RequireJsonString(anchor, "type") != "absolute_frame")
        {
            throw new BridgeValidationException(
                "The current target-plan route requires an already resolved absolute anchor");
        }
        _ = RequireNonNegativeJsonInt(anchor, "frame");
    }

    private static void ValidateAbsoluteIntentMatches(JsonElement intent, int frame)
    {
        var anchor = RequireJsonObject(RequireJsonObject(intent, "placement"), "anchor");
        if (RequireNonNegativeJsonInt(anchor, "frame") != frame)
        {
            throw new BridgeValidationException(
                "Resolved placement differs from the sealed absolute intent");
        }
    }

    private static string? RequireOptionalJsonString(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property))
        {
            throw new BridgeValidationException($"Target-plan JSON property is missing: {name}");
        }
        if (property.ValueKind == JsonValueKind.Null)
        {
            return null;
        }
        if (property.ValueKind != JsonValueKind.String
            || string.IsNullOrWhiteSpace(property.GetString()))
        {
            throw new BridgeValidationException($"Target-plan JSON string is invalid: {name}");
        }
        return property.GetString();
    }

    private static int? RequireOptionalNonNegativeJsonInt(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property))
        {
            throw new BridgeValidationException($"Target-plan JSON property is missing: {name}");
        }
        if (property.ValueKind == JsonValueKind.Null)
        {
            return null;
        }
        if (!property.TryGetInt32(out var result) || result < 0)
        {
            throw new BridgeValidationException($"Target-plan JSON integer is invalid: {name}");
        }
        return result;
    }

    private static void RequireJsonNull(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property) || property.ValueKind != JsonValueKind.Null)
        {
            throw new BridgeValidationException($"Target-plan JSON null is required: {name}");
        }
    }

    private static void RequireEmptyJsonArray(JsonElement value, string name)
    {
        if (RequireJsonArray(value, name).GetArrayLength() != 0)
        {
            throw new BridgeValidationException($"Target-plan JSON array must be empty: {name}");
        }
    }

    private static void RequireExactStringArray(
        JsonElement value,
        string name,
        IReadOnlyList<string> expected)
    {
        var actual = RequireJsonArray(value, name).EnumerateArray().Select(item =>
        {
            if (item.ValueKind != JsonValueKind.String || item.GetString() is not { } text)
            {
                throw new BridgeValidationException(
                    $"Target-plan JSON array contains a non-string: {name}");
            }
            return text;
        }).ToArray();
        if (!actual.SequenceEqual(expected, StringComparer.Ordinal))
        {
            throw new BridgeValidationException(
                $"Target-plan ownership mask differs from the supported profile: {name}");
        }
    }

    private sealed record ParsedTargetPlan(
        string ProjectId,
        string SceneId,
        IReadOnlyList<ManagedUtteranceDto> PortableUtterances,
        IReadOnlyList<NativeVoiceCueDto> NativeVoiceCues,
        TargetPlanValidationDto Validation);

    private sealed record TargetPlanBinding(string ProjectId, string SceneId);
}
