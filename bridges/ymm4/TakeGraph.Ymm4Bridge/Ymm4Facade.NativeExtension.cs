using System.Collections;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Windows;

namespace TakeGraph.Ymm4Bridge;

internal sealed partial class Ymm4Facade
{
    private const string NativeExtensionRecoveryDriver = "native_extension";

    internal NativeExtensionApplyResponseDto GetNativeExtensionOperation(Guid operationId)
    {
        if (!projectOperationStore.TryGetNativeExtension(operationId, out var receipt)
            || receipt is null)
        {
            throw new BridgeNotFoundException(
                $"Native-extension operation was not found: {operationId}");
        }
        return receipt;
    }

    internal NativeExtensionPlanResponseDto PlanNativeExtensions(
        NativeExtensionPlanRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        ValidateMutationRuntime();
        if (request.OperationId == Guid.Empty || request.Intents.Count == 0)
        {
            throw new BridgeValidationException(
                "Native-extension preview requires an operation ID and at least one intent");
        }
        var snapshot = Snapshot();
        EnsureTarget(request.ProjectId, request.SceneId, snapshot);
        EnsureFingerprint(request.ExpectedFingerprint, snapshot.Fingerprint);
        var catalog = Descriptors();
        RequireNativeExtensionCatalog(request.DescriptorCatalogDigest, catalog);
        VerifyNativeExtensionArtifacts(request.Artifacts);
        var intents = request.Intents.Select(ParseNativeExtensionIntent).ToArray();
        RequireDistinctLogicalKeys(intents);
        ValidateNativeExtensionArtifactBindings(intents, request.Artifacts);
        return Application.Current.Dispatcher.Invoke(() =>
        {
            var observation = ObserveNativeExtensions(
                intents,
                operationId: request.OperationId);
            var warnings = intents.Select(intent =>
                    observation.Existing.ContainsKey(intent.LogicalKey)
                        ? $"{intent.LogicalKey}: existing native state will be preserved in place"
                        : $"{intent.LogicalKey}: a new native realization will be created")
                .ToArray();
            return new NativeExtensionPlanResponseDto(
                snapshot.Fingerprint,
                catalog.CatalogDigest,
                catalog.DriverProfileDigest,
                observation,
                warnings);
        });
    }

    internal async Task<NativeExtensionApplyResponseDto> ApplyNativeExtensionsAsync(
        NativeExtensionApplyRequestDto request)
    {
        ValidateNativeExtensionApplyRequest(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (projectOperationStore.TryGetNativeExtension(request.OperationId, out var existing)
                && existing is not null)
            {
                EnsureNativeExtensionReceiptBinding(request, existing);
                if (existing.Status != "applying")
                {
                    return existing;
                }

                // The project-operation WAL precedes the richer recovery
                // preimage. If the latter is absent, this operation provably
                // never reached mutation and becomes a same-ID tombstone.
                if (!recoveryStore.TryGet(request.OperationId, out var recovery)
                    || recovery is null)
                {
                    var notStarted = existing with
                    {
                        Status = "not_started",
                        AfterFingerprint = existing.BeforeFingerprint,
                        Realizations = [],
                        Verified = false,
                        Error = "Native-extension apply did not reach its mutation recovery WAL",
                    };
                    projectOperationStore.PutNativeExtension(notStarted);
                    return notStarted;
                }

                // Historical Applying recovery is driven solely by the exact
                // request/WAL preimage. Current capability, catalog, artifact,
                // or driver gates cannot strand a reserved old operation.
                _ = await RecoverPendingJournalsCoreAsync().ConfigureAwait(false);
                if (projectOperationStore.TryGetNativeExtension(
                        request.OperationId,
                        out var recoveredNative)
                    && recoveredNative is not null
                    && recoveredNative.Status != "applying")
                {
                    EnsureNativeExtensionReceiptBinding(request, recoveredNative);
                    return recoveredNative;
                }
                if (receiptStore.TryGet(request.OperationId, out var recovered)
                    && recovered is not null)
                {
                    if (recovered.Status is not ("rolled_back" or "recovery_required" or "failed"))
                    {
                        var unresolved = existing with
                        {
                            Status = "recovery_required",
                            AfterFingerprint = recovered.AfterFingerprint,
                            Realizations = [],
                            Verified = false,
                            Error = "Native-extension recovery returned an incompatible standard receipt",
                        };
                        projectOperationStore.PutNativeExtension(unresolved);
                        return unresolved;
                    }
                    var recoveredExtension = existing with
                    {
                        Status = recovered.Status,
                        AfterFingerprint = recovered.AfterFingerprint,
                        Realizations = [],
                        Verified = false,
                        Error = recovered.Error ?? (recovered.Status == "rolled_back"
                            ? "Recovered native-extension operation to its exact before state"
                            : "Native-extension recovery did not reach a terminal verified state"),
                    };
                    projectOperationStore.PutNativeExtension(recoveredExtension);
                    return recoveredExtension;
                }
                return existing;
            }

            ValidateMutationRuntime();
            await EnsureRecoveryClearAsync().ConfigureAwait(false);
            var before = Snapshot();
            EnsureTarget(request.ProjectId, request.SceneId, before);
            EnsureFingerprint(request.ExpectedFingerprint, before.Fingerprint);
            var catalog = Descriptors();
            RequireNativeExtensionCatalog(request.DescriptorCatalogDigest, catalog);
            if (!string.Equals(
                    request.DriverProfileDigest,
                    catalog.DriverProfileDigest,
                    StringComparison.Ordinal))
            {
                throw new BridgeConflictException(
                    "Native-extension driver profile changed after approval",
                    before.Fingerprint);
            }
            VerifyNativeExtensionArtifacts(request.Artifacts);
            var operations = ParseNativeExtensionPlan(request);
            ValidateNativeExtensionArtifactBindings(
                operations.Select(value => value.Intent).ToArray(),
                request.Artifacts);
            ValidateNativeExtensionCapabilities(operations);

            var preparation = Application.Current.Dispatcher.Invoke(() =>
                PrepareNativeExtensionApply(operations));
            ValidateNativeExtensionPlanObservation(operations, preparation.Observation);
            var applying = new NativeExtensionApplyResponseDto(
                request.OperationId,
                request.RequestDigest,
                request.ProjectId,
                request.SceneId,
                "applying",
                before.Fingerprint,
                before.Fingerprint,
                request.DescriptorCatalogDigest,
                request.DriverProfileDigest,
                [],
                false,
                null);
            projectOperationStore.PutNativeExtension(applying);
            BridgeFaultInjection.ThrowIf("before_journal_write");
            recoveryStore.Put(CreateRecoveryEntry(
                request.OperationId,
                request.RequestDigest,
                request.ProjectId,
                request.SceneId,
                request.ExpectedFingerprint,
                before.Fingerprint,
                NativeExtensionRecoveryDriver,
                operations.Select(value => value.Intent.EntityId).ToArray(),
                operations.Select(value => value.RealizationId).ToArray(),
                [],
                new Dictionary<Guid, string>(),
                preparation.BeforeItems));
            BridgeFaultInjection.ThrowIf("after_journal_before_mutation");

            var successfulReadbackDurable = false;
            try
            {
                await ApplyNativeExtensionOperationsAsync(
                    operations,
                    request.Artifacts,
                    catalog,
                    preparation).ConfigureAwait(false);
                BridgeFaultInjection.ThrowIf("after_mutation_before_readback");
                var after = Snapshot();
                var realizations = Application.Current.Dispatcher.Invoke(() =>
                    ReadNativeExtensionRealizations(operations, catalog));
                var verified = NativeExtensionRealizationsMatch(
                        operations,
                        realizations,
                        request.ProjectId,
                        catalog)
                    && Application.Current.Dispatcher.Invoke(() =>
                        NativeExtensionPreservationMatches(operations))
                    && Application.Current.Dispatcher.Invoke(() =>
                        UnmanagedWitnessMatches(preparation));
                var receipt = applying with
                {
                    Status = verified ? "verified" : "recovery_required",
                    AfterFingerprint = after.Fingerprint,
                    Realizations = realizations,
                    Verified = verified,
                    Error = verified
                        ? null
                        : "Native-extension semantic read-back or unmanaged-state witness failed",
                };
                if (receipt.Verified)
                {
                    recoveryStore.MarkAppliedUnverified(
                        request.OperationId,
                        after.Fingerprint,
                        [],
                        receipt);
                    successfulReadbackDurable = true;
                }
                else
                {
                    recoveryStore.Transition(
                        request.OperationId,
                        receipt.Status,
                        after.Fingerprint,
                        receipt.Error);
                }
                projectOperationStore.PutNativeExtension(receipt);
                BridgeFaultInjection.ThrowIf("after_receipt_before_finalize");
                if (receipt.Verified)
                {
                    recoveryStore.Transition(
                        request.OperationId,
                        "verified",
                        after.Fingerprint,
                        null);
                }
                return receipt;
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
                return await RollBackNativeExtensionAsync(
                    request,
                    applying,
                    before,
                    error.GetBaseException().Message).ConfigureAwait(false);
            }
        }
        finally
        {
            applyGate.Release();
        }
    }

    internal async Task<NativeExtensionApplyResponseDto> SealNativeExtensionNotStartedAsync(
        NativeExtensionApplyRequestDto request)
    {
        ValidateNativeExtensionApplyRequest(request);
        await applyGate.WaitAsync().ConfigureAwait(false);
        try
        {
            if (projectOperationStore.TryGetNativeExtension(request.OperationId, out var existing)
                && existing is not null)
            {
                EnsureNativeExtensionReceiptBinding(request, existing);
                return existing;
            }
            var tombstone = new NativeExtensionApplyResponseDto(
                request.OperationId,
                request.RequestDigest,
                request.ProjectId,
                request.SceneId,
                "not_started",
                request.ExpectedFingerprint,
                request.ExpectedFingerprint,
                request.DescriptorCatalogDigest,
                request.DriverProfileDigest,
                [],
                false,
                "Native-extension apply did not start; a durable no-mutation tombstone was sealed");
            projectOperationStore.PutNativeExtension(tombstone);
            return tombstone;
        }
        finally
        {
            applyGate.Release();
        }
    }

    private static void ValidateNativeExtensionApplyRequest(
        NativeExtensionApplyRequestDto request)
    {
        ValidateProtocol(request.ProtocolVersion);
        if (request.OperationId == Guid.Empty
            || string.IsNullOrWhiteSpace(request.RequestDigest)
            || string.IsNullOrWhiteSpace(request.ProjectId)
            || string.IsNullOrWhiteSpace(request.SceneId)
            || string.IsNullOrWhiteSpace(request.ExpectedFingerprint)
            || string.IsNullOrWhiteSpace(request.DescriptorCatalogDigest)
            || string.IsNullOrWhiteSpace(request.DriverProfileDigest)
            || string.IsNullOrWhiteSpace(request.PlanDigest)
            || request.Plan.ValueKind != JsonValueKind.Object)
        {
            throw new BridgeValidationException("Native-extension apply binding fields are invalid");
        }
        var calculatedPlanDigest = CanonicalJson.Sha256(
            "takegraph-native-extension-plan-v1",
            request.Plan);
        if (!ApplyRequestDigest.Matches(request.PlanDigest, calculatedPlanDigest))
        {
            throw new BridgeValidationException("Native-extension plan digest does not match its payload");
        }
        if (!ApplyRequestDigest.Matches(
                request.RequestDigest,
                ApplyRequestDigest.Compute(request)))
        {
            throw new BridgeValidationException("Native-extension apply digest does not match its payload");
        }
    }

    private ParsedNativeExtensionOperation[] ParseNativeExtensionPlan(
        NativeExtensionApplyRequestDto request,
        bool validateCurrentScope = true)
    {
        var root = request.Plan;
        RequireExactJsonProperties(
            root,
            "canonicalVersion",
            "operationId",
            "baseRevision",
            "target",
            "capabilityDigest",
            "descriptorCatalogDigest",
            "expectedScope",
            "changeBudget",
            "operations",
            "warnings");
        RequireJsonNumber(root, "canonicalVersion", 1);
        var operationId = RequireJsonGuid(root, "operationId");
        if (operationId != request.OperationId)
        {
            throw new BridgeValidationException("Native-extension plan operation ID differs from request");
        }
        var target = RequireJsonObject(root, "target");
        RequireExactJsonProperties(
            target,
            "adapterId",
            "projectId",
            "sceneId",
            "fps",
            "driverVersion");
        if (!string.Equals(RequireJsonString(target, "projectId"), request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(RequireJsonString(target, "sceneId"), request.SceneId, StringComparison.Ordinal))
        {
            throw new BridgeValidationException("Native-extension plan target differs from request");
        }
        var pluginVersion = typeof(Ymm4Facade).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
            ?? typeof(Ymm4Facade).Assembly.GetName().Version?.ToString()
            ?? "unknown";
        var expectedDriverVersion = $"{SupportedMutationYmm4Version}/{pluginVersion}";
        if (RequireJsonULong(root, "baseRevision") > long.MaxValue
            || !string.Equals(RequireJsonString(target, "adapterId"), "ymm4-4.55", StringComparison.Ordinal)
            || !string.Equals(
                RequireJsonString(target, "driverVersion"),
                expectedDriverVersion,
                StringComparison.Ordinal))
        {
            throw new BridgeValidationException(
                "Native-extension canonical revision or target driver binding is invalid");
        }
        _ = RequireSha256(RequireJsonString(root, "capabilityDigest"), "capabilityDigest");
        _ = RequireSha256(
            RequireJsonString(root, "descriptorCatalogDigest"),
            "descriptorCatalogDigest");
        var expectedScope = RequireJsonObject(root, "expectedScope");
        RequireExactJsonProperties(
            expectedScope,
            "targetIdentityDigest",
            "managedStateDigest",
            "conflictScopeDigest");
        _ = RequireSha256(
            RequireJsonString(expectedScope, "targetIdentityDigest"),
            "targetIdentityDigest");
        _ = RequireSha256(
            RequireJsonString(expectedScope, "managedStateDigest"),
            "managedStateDigest");
        _ = RequireSha256(
            RequireJsonString(expectedScope, "conflictScopeDigest"),
            "conflictScopeDigest");
        var budget = RequireJsonObject(root, "changeBudget");
        RequireExactJsonProperties(
            budget,
            "maxChangedEntities",
            "maxShiftedEntities",
            "maxShiftFrames",
            "allowLockedChanges",
            "allowUnmanagedChanges");
        if (RequireJsonBoolean(budget, "allowUnmanagedChanges")
            || RequireJsonBoolean(budget, "allowLockedChanges"))
        {
            throw new BridgeValidationException("Native-extension plan cannot authorize unmanaged or locked changes");
        }
        if (RequireNonNegativeJsonInt(budget, "maxShiftedEntities") != 0
            || RequireNonNegativeJsonInt(budget, "maxShiftFrames") != 0)
        {
            throw new BridgeValidationException(
                "The current native-extension driver never shifts unrelated timeline entities");
        }
        var operationsElement = RequireJsonArray(root, "operations");
        var operations = operationsElement.EnumerateArray().Select(value =>
        {
            RequireExactJsonProperties(
                value,
                "realizationId",
                "action",
                "intent",
                "capabilityDependencies",
                "descriptorDependencies",
                "preservation");
            var intent = ParseNativeExtensionIntent(RequireJsonObject(value, "intent"));
            var preservation = ParseNativeExtensionPreservation(
                RequireJsonObject(value, "preservation"));
            return new ParsedNativeExtensionOperation(
                RequireJsonGuid(value, "realizationId"),
                RequireJsonString(value, "action"),
                intent,
                preservation);
        }).ToArray();
        if (operations.Length == 0
            || operations.Length > RequireJsonInt(budget, "maxChangedEntities"))
        {
            throw new BridgeValidationException("Native-extension plan exceeds its change budget");
        }
        RequireDistinctLogicalKeys(operations.Select(value => value.Intent).ToArray());
        if (operations.Select(value => value.RealizationId).Distinct().Count() != operations.Length
            || operations.Any(value => value.RealizationId == Guid.Empty))
        {
            throw new BridgeValidationException("Native-extension realization identities are invalid");
        }
        ValidateNativeExtensionPlanDependencies(root, operations);
        foreach (var warning in RequireJsonArray(root, "warnings").EnumerateArray())
        {
            if (warning.ValueKind != JsonValueKind.String)
            {
                throw new BridgeValidationException(
                    "Native-extension plan warnings must be strings");
            }
        }
        var snapshot = SnapshotCore();
        if (RequireJsonInt(target, "fps") != snapshot.Fps)
        {
            throw new BridgeConflictException(
                "Native-extension plan FPS differs from the active target",
                snapshot.Fingerprint);
        }
        ValidateNativeExtensionScope(
            target,
            expectedScope,
            snapshot,
            expectedDriverVersion,
            validateCurrentScope);
        return operations;
    }

    private static void ValidateNativeExtensionScope(
        JsonElement target,
        JsonElement expectedScope,
        ProjectSnapshotDto snapshot,
        string expectedDriverVersion,
        bool validateCurrentScope)
    {
        var targetIdentityDigest = CanonicalJson.Sha256(
            "takegraph-ymm4-target-identity",
            new
            {
                adapterId = "ymm4-4.55",
                projectId = snapshot.ProjectId,
                sceneId = snapshot.SceneId,
                fps = snapshot.Fps,
                driverVersion = expectedDriverVersion,
            });
        if (!ApplyRequestDigest.Matches(
                RequireJsonString(expectedScope, "targetIdentityDigest"),
                targetIdentityDigest)
            || RequireJsonInt(target, "fps") != snapshot.Fps)
        {
            throw new BridgeConflictException(
                "Native-extension target identity scope changed after approval",
                snapshot.Fingerprint);
        }
        if (!validateCurrentScope)
        {
            return;
        }
        var managedStateDigest = CanonicalJson.Sha256(
            "takegraph-ymm4-managed-state",
            snapshot.ManagedItems);
        var conflictScopeDigest = CanonicalJson.Sha256(
            "takegraph-ymm4-conflict-scope",
            new
            {
                fingerprint = snapshot.Fingerprint,
                unmanagedContextCount = snapshot.UnmanagedContextCount,
            });
        if (!ApplyRequestDigest.Matches(
                RequireJsonString(expectedScope, "managedStateDigest"),
                managedStateDigest)
            || !ApplyRequestDigest.Matches(
                RequireJsonString(expectedScope, "conflictScopeDigest"),
                conflictScopeDigest))
        {
            throw new BridgeConflictException(
                "Native-extension managed/conflict scope changed after approval",
                snapshot.Fingerprint);
        }
    }

    private void ValidateNativeExtensionPlanDependencies(
        JsonElement root,
        IReadOnlyList<ParsedNativeExtensionOperation> operations)
    {
        var catalog = Descriptors();
        var capabilities = Capabilities().Capabilities.ToHashSet(StringComparer.Ordinal);
        var pluginVersion = typeof(Ymm4Facade).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
            ?? typeof(Ymm4Facade).Assembly.GetName().Version?.ToString()
            ?? "unknown";
        var ymm4Version = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "unknown";
        var currentCapabilityDigest = ComputeStructuredCapabilityDigest(
            capabilities,
            pluginVersion,
            ymm4Version);
        if (!ApplyRequestDigest.Matches(
                RequireJsonString(root, "capabilityDigest"),
                currentCapabilityDigest))
        {
            throw new BridgeConflictException(
                "Native-extension structured capability contract changed after approval",
                SnapshotCore().Fingerprint);
        }
        var currentPortableCatalogDigest = ComputePortableDescriptorCatalogDigest(catalog);
        if (!ApplyRequestDigest.Matches(
                RequireJsonString(root, "descriptorCatalogDigest"),
                currentPortableCatalogDigest))
        {
            throw new BridgeConflictException(
                "Native-extension portable descriptor catalog changed after approval",
                SnapshotCore().Fingerprint);
        }
        foreach (var pair in RequireJsonArray(root, "operations")
                     .EnumerateArray().Zip(operations))
        {
            var operationJson = pair.First;
            var operation = pair.Second;
            var requiredCapability = operation.Intent.Kind switch
            {
                "portrait" => "native_portrait_upsert",
                "face" => "native_face_upsert",
                "image" => "native_image_upsert",
                "video" => "native_video_upsert",
                "audio" or "bgm" => "native_audio_upsert",
                "managed_effect" => "native_effect_typed_mutation",
                "template" => "native_template_instantiate",
                _ => string.Empty,
            };
            var capabilityNames = RequireJsonArray(operationJson, "capabilityDependencies")
                .EnumerateArray()
                .Select(dependency =>
                {
                    RequireExactJsonProperties(
                        dependency,
                        "feature",
                        "minimumVersion",
                        "schemaDigest");
                    if (RequirePositiveJsonInt(dependency, "minimumVersion") != 1)
                    {
                        throw new BridgeValidationException(
                            "Native-extension capability dependency version is unsupported");
                    }
                    var schemaDigest = RequireSha256(
                        RequireJsonString(dependency, "schemaDigest"),
                        "capability schemaDigest");
                    var feature = RequireJsonString(dependency, "feature");
                    if (!ApplyRequestDigest.Matches(
                            schemaDigest,
                            StructuredFeatureSchemaDigest(
                                feature,
                                StructuredFeaturePropertyNames(feature))))
                    {
                        throw new BridgeConflictException(
                            $"Native-extension capability schema drifted: {feature}",
                            SnapshotCore().Fingerprint);
                    }
                    return feature;
                })
                .ToArray();
            if (capabilityNames.Length != 2
                || !capabilityNames.Contains(requiredCapability, StringComparer.Ordinal)
                || !capabilityNames.Contains("timeline.transaction", StringComparer.Ordinal))
            {
                throw new BridgeValidationException(
                    $"Native-extension capability dependencies are incomplete: {operation.Intent.LogicalKey}");
            }

            var descriptorDependencies = RequireJsonArray(
                    operationJson,
                    "descriptorDependencies")
                .EnumerateArray()
                .ToArray();
            var needsDescriptor = operation.Intent.DescriptorId is not null;
            if (descriptorDependencies.Length != (needsDescriptor ? 1 : 0))
            {
                throw new BridgeValidationException(
                    $"Native-extension descriptor dependencies are incomplete: {operation.Intent.LogicalKey}");
            }
            if (needsDescriptor)
            {
                var dependency = descriptorDependencies[0];
                RequireExactJsonProperties(dependency, "kind", "descriptorId", "digest");
                var descriptorId = RequireJsonString(dependency, "descriptorId");
                var expectedKind = operation.Intent.Kind switch
                {
                    "portrait" or "face" => "character",
                    "managed_effect" => "effect",
                    "template" => "template",
                    _ => string.Empty,
                };
                var descriptorDigest = RequireSha256(
                    RequireJsonString(dependency, "digest"),
                    "descriptor digest");
                var currentDescriptors = catalog.Descriptors.Where(value =>
                        value.DescriptorId == descriptorId
                        && value.Bindable
                        && (expectedKind != "effect"
                            ? value.Kind == expectedKind
                            : value.Kind is "video-effect" or "audio-effect"
                                && value.MutationAllowed))
                    .ToArray();
                if (!string.Equals(
                        RequireJsonString(dependency, "kind"),
                        expectedKind,
                        StringComparison.Ordinal)
                    || !string.Equals(
                        descriptorId,
                        operation.Intent.DescriptorId,
                        StringComparison.Ordinal)
                    || !ApplyRequestDigest.Matches(
                        descriptorDigest,
                        operation.Intent.DescriptorExpectedDigest ?? string.Empty)
                    || currentDescriptors.Length != 1
                    || !ApplyRequestDigest.Matches(
                        descriptorDigest,
                        ComputePortableDescriptorDigest(currentDescriptors[0])))
                {
                    throw new BridgeConflictException(
                        $"Native-extension descriptor dependency drifted: {descriptorId}",
                        SnapshotCore().Fingerprint);
                }
            }
        }
    }

    internal static string ComputePortableDescriptorCatalogDigest(DescriptorCatalogDto catalog)
    {
        var characters = new SortedDictionary<string, object>(StringComparer.Ordinal);
        var templates = new SortedDictionary<string, object>(StringComparer.Ordinal);
        var effects = new SortedDictionary<string, object>(StringComparer.Ordinal);
        foreach (var descriptor in catalog.Descriptors.Where(value => value.Bindable))
        {
            switch (descriptor.Kind)
            {
                case "character":
                    characters.Add(
                        descriptor.DescriptorId,
                        PortableCharacterDescriptor(descriptor));
                    break;
                case "template":
                    templates.Add(
                        descriptor.DescriptorId,
                        PortableTemplateDescriptor(descriptor));
                    break;
                case "video-effect" or "audio-effect" when descriptor.MutationAllowed:
                    effects.Add(
                        descriptor.DescriptorId,
                        PortableEffectDescriptor(descriptor));
                    break;
            }
        }
        return CanonicalJson.Sha256(
            "takegraph-native-descriptor-catalog-v1",
            new { characters, templates, effects });
    }

    internal static string ComputePortableDescriptorDigest(TargetDescriptorDto descriptor)
    {
        return descriptor.Kind switch
        {
            "character" => CanonicalJson.Sha256(
                "takegraph-character-descriptor-v1",
                PortableCharacterDescriptor(descriptor)),
            "template" => CanonicalJson.Sha256(
                "takegraph-template-descriptor-v1",
                PortableTemplateDescriptor(descriptor)),
            "video-effect" or "audio-effect" when descriptor.MutationAllowed =>
                CanonicalJson.Sha256(
                    "takegraph-effect-descriptor-v1",
                    PortableEffectDescriptor(descriptor)),
            _ => throw new BridgeValidationException(
                $"Descriptor cannot participate in native-extension planning: {descriptor.DescriptorId}"),
        };
    }

    private static object PortableCharacterDescriptor(TargetDescriptorDto descriptor)
    {
        var configuration = new SortedDictionary<string, string>(
            descriptor.Metadata
                .Where(pair => !string.IsNullOrWhiteSpace(pair.Key)
                    && !string.IsNullOrWhiteSpace(pair.Value))
                .ToDictionary(pair => pair.Key, pair => pair.Value),
            StringComparer.Ordinal)
        {
            ["takegraph.targetConfigDigest"] = RequireSha256(
                descriptor.ConfigDigest,
                "target descriptor configDigest"),
            ["takegraph.targetSchemaDigest"] = RequireSha256(
                descriptor.SchemaDigest,
                "target descriptor schemaDigest"),
        };
        return new
        {
            descriptorId = descriptor.DescriptorId,
            displayName = descriptor.Name,
            supportedPresentations = new[] { "portrait", "face" },
            configuration,
        };
    }

    private static object PortableTemplateDescriptor(TargetDescriptorDto descriptor)
    {
        var producedItemKinds = descriptor.Metadata.GetValueOrDefault("itemTypes")
            ?.Split('\n')
            .Where(value => !string.IsNullOrWhiteSpace(value))
            .Distinct(StringComparer.Ordinal)
            .Order(StringComparer.Ordinal)
            .ToArray() ?? [];
        if (producedItemKinds.Length == 0)
        {
            producedItemKinds = ["opaque-native-item"];
        }
        return new
        {
            descriptorId = descriptor.DescriptorId,
            displayName = descriptor.Name,
            contentDigest = ComputeTargetDescriptorDigest(descriptor),
            producedItemKinds,
        };
    }

    private static object PortableEffectDescriptor(TargetDescriptorDto descriptor)
    {
        return new
        {
            descriptorId = descriptor.DescriptorId,
            stableTypeId = $"{descriptor.Metadata.GetValueOrDefault("type") ?? "unknown"}#"
                + ComputeTargetDescriptorDigest(descriptor),
            displayName = descriptor.Name,
            schemaVersion = 1,
            parameters = new SortedDictionary<string, object>(StringComparer.Ordinal),
        };
    }

    private static string ComputeTargetDescriptorDigest(TargetDescriptorDto descriptor)
    {
        return CanonicalJson.Sha256(
            "takegraph-ymm4-target-descriptor-v1",
            new
            {
                descriptorId = descriptor.DescriptorId,
                kind = descriptor.Kind,
                configDigest = RequireSha256(
                    descriptor.ConfigDigest,
                    "target descriptor configDigest"),
                schemaDigest = RequireSha256(
                    descriptor.SchemaDigest,
                    "target descriptor schemaDigest"),
                bindable = descriptor.Bindable,
                mutationAllowed = descriptor.MutationAllowed,
            });
    }

    internal static string ComputeStructuredCapabilityDigest(
        IReadOnlySet<string> capabilities,
        string pluginVersion,
        string ymm4Version)
    {
        var has = (string value) => capabilities.Contains(value);
        var features = new SortedDictionary<string, object>(StringComparer.Ordinal);
        AddStructuredFeature(
            features,
            "managedPair.apply",
            has("managed_audio") && has("managed_caption"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["identityCarrier"] = "caption_marker",
                ["semanticItemCount"] = 2L,
            });
        AddStructuredFeature(
            features,
            "targetPlan.apply",
            has("unified_target_plan"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["canonicalVersion"] = 1L,
                ["mixedStrategies"] = false,
                ["resolvedBindings"] = true,
                ["resolvedPlacement"] = true,
            });
        AddStructuredFeature(
            features,
            "managedIdentity.detach",
            has("metadata_remark_detach")
                && has("request_bound_receipts")
                && has("write_ahead_apply")
                && has("recovery_readback"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["freshReadback"] = true,
                ["identityCarrier"] = "remark",
                ["mutationScope"] = "metadata_only",
                ["nonRemarkDigestPreserved"] = true,
                ["notStartedTombstone"] = true,
                ["projectScopedIdentity"] = true,
            });
        AddStructuredFeature(
            features,
            "voiceItem.create",
            has("native_voice_create")
                && has("native_voice_remark_identity")
                && has("native_voice_bounded_duration"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["artifactCapture"] = false,
                ["artifactExportAvailable"] = has("native_voice_exact_wav_export")
                    && has("native_voice_host_bound_provenance"),
                ["durationResolution"] = "bounded",
                ["identityCarrier"] = "remark",
                ["prepare"] = false,
                ["separateDisplayAndSpokenText"] = false,
            });
        AddStructuredFeature(
            features,
            "voiceItem.update",
            has("native_voice_update_replace_preserving_user_state")
                && has("native_voice_remark_identity")
                && has("native_voice_bounded_duration"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["durationResolution"] = "bounded",
                ["identityCarrier"] = "remark",
                ["mutationMode"] = "replace_preserving_user_state",
                ["preservedStateVerified"] = true,
                ["separateDisplayAndSpokenText"] = false,
            });
        AddStructuredFeature(
            features,
            "voiceItem.delete",
            has("native_voice_delete") && has("native_voice_remark_identity"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["deleteReadback"] = "realization_absent",
                ["identityCarrier"] = "remark",
            });
        AddStructuredFeature(
            features,
            "voiceItem.artifactExport",
            has("native_voice_exact_wav_export")
                && has("native_voice_host_bound_provenance"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["audio"] = "exact_wav",
                ["contentHash"] = "sha256",
                ["portableSynthesisQuery"] = false,
                ["provenance"] = "normalized_host_bound_voice_state",
            });
        AddStructuredFeature(
            features,
            "timeline.transaction",
            has("idempotent_apply")
                && has("request_bound_receipts")
                && has("write_ahead_apply"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["durableRollback"] = false,
                ["recoveryReadback"] = has("recovery_readback"),
                ["undoBatch"] = has("undo_batch"),
            });
        AddStructuredFeature(
            features,
            "scene.capture",
            has("scene_capture_native_png")
                && has("scene_capture_playhead_restore")
                && has("scene_capture_content_hash"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["contentHash"] = "sha256",
                ["maxFrames"] = 64L,
                ["mediaType"] = "image/png",
                ["transientStateRestore"] = true,
            });
        foreach (var definition in new[]
        {
            ("portraitItem.upsert", "native_portrait_upsert", "portrait"),
            ("faceItem.upsert", "native_face_upsert", "face"),
            ("imageItem.upsert", "native_image_upsert", "image"),
            ("videoItem.upsert", "native_video_upsert", "video"),
            ("audioItem.upsert", "native_audio_upsert", "audio_or_bgm"),
            ("effect.typedMutation", "native_effect_typed_mutation", "typed_effect"),
            ("template.instantiate", "native_template_instantiate", "native_template"),
        })
        {
            AddStructuredFeature(
                features,
                definition.Item1,
                has(definition.Item2),
                new SortedDictionary<string, object>(StringComparer.Ordinal)
                {
                    ["driverOperation"] = definition.Item3,
                    ["exactLossAllowlist"] = true,
                    ["identityCarrier"] = "remark",
                    ["unknownEffectsPreserved"] = true,
                });
        }
        AddStructuredFeature(
            features,
            "readback.semantic",
            has("readback_verification"),
            new SortedDictionary<string, object>(StringComparer.Ordinal));
        AddStructuredFeature(
            features,
            "project.checkpoint",
            has("project_checkpoint_verified"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["canonicalRevisionAdvances"] = false,
                ["existingPathOnly"] = true,
                ["fileHash"] = "sha256",
            });
        AddStructuredFeature(
            features,
            "project.render",
            has("project_render")
                && has("project_render_cancel")
                && has("project_render_media_receipt"),
            new SortedDictionary<string, object>(StringComparer.Ordinal)
            {
                ["cancellation"] = true,
                ["canonicalRevisionAdvances"] = false,
                ["exactEncoderProfileBinding"] = true,
                ["explicitOutputPath"] = true,
                ["explicitOverwritePolicy"] = true,
                ["finalMediaReceipt"] = "sha256_and_probe",
                ["immutableCheckpointSnapshot"] = true,
                ["replaceExistingWriteAheadLog"] = true,
                ["verifiedCheckpointRequired"] = true,
            });
        var mutationTested = has("mutation_profile_ymm4_4_55_1_1");
        return CanonicalJson.Sha256(
            "takegraph-ymm4-capabilities",
            new
            {
                protocol = new { major = BridgeContract.ProtocolVersion, minMinor = 0, maxMinor = 0 },
                driver = new
                {
                    id = mutationTested ? "ymm4-4.55" : "ymm4-observation-only",
                    pluginVersion,
                    ymm4Version,
                    mutationStatus = mutationTested ? "tested" : "observation_only",
                },
                features,
            });
    }

    private static void AddStructuredFeature(
        IDictionary<string, object> features,
        string name,
        bool available,
        SortedDictionary<string, object> properties)
    {
        features[name] = new
        {
            version = 1,
            available,
            schemaDigest = StructuredFeatureSchemaDigest(name, properties.Keys),
            properties,
        };
    }

    internal static string StructuredFeatureSchemaDigest(
        string name,
        IEnumerable<string> propertyNames)
    {
        return CanonicalJson.Sha256(
            "takegraph-ymm4-feature-schema",
            new
            {
                name,
                version = 1,
                propertyNames = propertyNames.Order(StringComparer.Ordinal).ToArray(),
            });
    }

    internal static IReadOnlyList<string> StructuredFeaturePropertyNames(string feature)
    {
        return feature switch
        {
            "managedPair.apply" => ["identityCarrier", "semanticItemCount"],
            "targetPlan.apply" =>
                ["canonicalVersion", "mixedStrategies", "resolvedBindings", "resolvedPlacement"],
            "managedIdentity.detach" =>
                ["freshReadback", "identityCarrier", "mutationScope", "nonRemarkDigestPreserved", "notStartedTombstone", "projectScopedIdentity"],
            "voiceItem.create" =>
                [
                    "artifactCapture",
                    "artifactExportAvailable",
                    "durationResolution",
                    "identityCarrier",
                    "prepare",
                    "separateDisplayAndSpokenText",
                ],
            "portraitItem.upsert" or "faceItem.upsert" or "imageItem.upsert"
                or "videoItem.upsert" or "audioItem.upsert" or "effect.typedMutation"
                or "template.instantiate" =>
                ["driverOperation", "exactLossAllowlist", "identityCarrier", "unknownEffectsPreserved"],
            "timeline.transaction" => ["durableRollback", "recoveryReadback", "undoBatch"],
            "readback.semantic" => [],
            _ => throw new BridgeValidationException(
                $"Unknown native-extension capability dependency: {feature}"),
        };
    }

    private static ParsedNativeExtensionIntent ParseNativeExtensionIntent(JsonElement value)
    {
        RequireExactJsonProperties(value, "type", "intent");
        var type = RequireJsonString(value, "type");
        var intent = RequireJsonObject(value, "intent");
        return type switch
        {
            "upsert_portrait" => ParsePortraitIntent(intent),
            "upsert_asset" => ParseAssetIntent(intent),
            "mutate_effect" => ParseEffectIntent(intent),
            "instantiate_template" => ParseTemplateIntent(intent),
            _ => throw new BridgeValidationException(
                $"Unsupported native-extension intent: {type}"),
        };
    }

    private static ParsedNativeExtensionIntent ParsePortraitIntent(JsonElement value)
    {
        RequireExactJsonProperties(
            value,
            "entityId",
            "entityRevision",
            "presentation",
            "characterBinding",
            "placement",
            "durationFrames",
            "replacementGuard");
        var entityId = RequireJsonString(value, "entityId");
        var presentation = RequireJsonString(value, "presentation");
        if (presentation is not ("portrait" or "face"))
        {
            throw new BridgeValidationException("Unsupported portrait presentation");
        }
        var placement = RequireJsonObject(value, "placement");
        var descriptor = RequireJsonObject(value, "characterBinding");
        ValidatePlacement(placement);
        ValidateDescriptorReference(descriptor);
        ValidateLosslessReplacementGuard(RequireJsonObject(value, "replacementGuard"));
        return new ParsedNativeExtensionIntent(
            $"portrait:{entityId}",
            presentation,
            entityId,
            RequireJsonULong(value, "entityRevision"),
            RequireNonNegativeJsonInt(placement, "frame"),
            RequireNonNegativeJsonInt(placement, "primaryLayer"),
            RequirePositiveJsonInt(value, "durationFrames"),
            RequireJsonString(descriptor, "descriptorId"),
            RequireSha256(RequireJsonString(descriptor, "expectedDigest"), "character descriptor"),
            null,
            null,
            null,
            false,
            null,
            null,
            null,
            null);
    }

    private static ParsedNativeExtensionIntent ParseAssetIntent(JsonElement value)
    {
        RequireExactJsonProperties(
            value,
            "entityId",
            "entityRevision",
            "asset",
            "placement",
            "durationFrames",
            "loopPlayback",
            "replacementGuard");
        var entityId = RequireJsonString(value, "entityId");
        var asset = RequireJsonObject(value, "asset");
        RequireExactJsonProperties(asset, "artifactDigest", "mediaType", "byteLength", "kind");
        var kind = RequireJsonString(asset, "kind");
        if (kind is not ("image" or "video" or "audio" or "bgm"))
        {
            throw new BridgeValidationException("Unsupported native asset kind");
        }
        var placement = RequireJsonObject(value, "placement");
        ValidatePlacement(placement);
        ValidateLosslessReplacementGuard(RequireJsonObject(value, "replacementGuard"));
        return new ParsedNativeExtensionIntent(
            $"asset:{entityId}",
            kind,
            entityId,
            RequireJsonULong(value, "entityRevision"),
            RequireNonNegativeJsonInt(placement, "frame"),
            RequireNonNegativeJsonInt(placement, "primaryLayer"),
            RequirePositiveJsonInt(value, "durationFrames"),
            null,
            null,
            RequireJsonString(asset, "artifactDigest"),
            RequireJsonString(asset, "mediaType"),
            RequirePositiveJsonULong(asset, "byteLength"),
            RequireJsonBoolean(value, "loopPlayback"),
            null,
            null,
            null,
            null);
    }

    private static ParsedNativeExtensionIntent ParseEffectIntent(JsonElement value)
    {
        RequireExactJsonProperties(
            value,
            "targetEntityId",
            "targetEntityRevision",
            "effectInstanceId",
            "descriptor",
            "operation");
        var targetEntityId = RequireJsonString(value, "targetEntityId");
        var effectInstanceId = RequireJsonString(value, "effectInstanceId");
        var descriptor = RequireJsonObject(value, "descriptor");
        var operation = RequireJsonObject(value, "operation");
        var action = RequireJsonString(operation, "type");
        if (action is not ("upsert" or "remove"))
        {
            throw new BridgeValidationException("Unsupported typed effect action");
        }
        RequireExactJsonProperties(
            operation,
            action == "upsert" ? ["type", "parameters"] : ["type"]);
        ValidateDescriptorReference(descriptor);
        var parameters = action == "upsert"
            ? RequireJsonObject(operation, "parameters")
            : default;
        if (action == "upsert" && parameters.EnumerateObject().Any())
        {
            throw new BridgeValidationException(
                "The current typed effect allowlist is parameterless");
        }
        return new ParsedNativeExtensionIntent(
            $"effect:{targetEntityId}:{effectInstanceId}",
            "managed_effect",
            targetEntityId,
            RequireJsonULong(value, "targetEntityRevision"),
            0,
            0,
            0,
            RequireJsonString(descriptor, "descriptorId"),
            RequireSha256(RequireJsonString(descriptor, "expectedDigest"), "effect descriptor"),
            null,
            null,
            null,
            false,
            targetEntityId,
            effectInstanceId,
            action,
            parameters);
    }

    private static ParsedNativeExtensionIntent ParseTemplateIntent(JsonElement value)
    {
        RequireExactJsonProperties(
            value,
            "entityId",
            "entityRevision",
            "template",
            "placement");
        var entityId = RequireJsonString(value, "entityId");
        var descriptor = RequireJsonObject(value, "template");
        var placement = RequireJsonObject(value, "placement");
        ValidateDescriptorReference(descriptor);
        ValidatePlacement(placement);
        return new ParsedNativeExtensionIntent(
            $"template:{entityId}",
            "template",
            entityId,
            RequireJsonULong(value, "entityRevision"),
            RequireNonNegativeJsonInt(placement, "frame"),
            RequireNonNegativeJsonInt(placement, "primaryLayer"),
            0,
            RequireJsonString(descriptor, "descriptorId"),
            RequireSha256(RequireJsonString(descriptor, "expectedDigest"), "template descriptor"),
            null,
            null,
            null,
            false,
            null,
            null,
            null,
            null);
    }

    private static ParsedNativeExtensionPreservation ParseNativeExtensionPreservation(
        JsonElement value)
    {
        RequireExactJsonProperties(
            value,
            "mode",
            "preservedFields",
            "unknownEffects",
            "lossyFields",
            "approvedLossyFields");
        var mode = RequireJsonString(value, "mode");
        if (mode is not ("create" or "in_place" or "replace"))
        {
            throw new BridgeValidationException("Unsupported native-extension preservation mode");
        }
        var preserved = RequireJsonArray(value, "preservedFields").EnumerateArray()
            .Select(item =>
            {
                RequireExactJsonProperties(item, "field", "stateDigest");
                return new NativeExtensionPreservedFieldDto(
                    RequireJsonString(item, "field"),
                    RequireSha256(RequireJsonString(item, "stateDigest"), "preserved state"));
            })
            .OrderBy(item => item.Field, StringComparer.Ordinal)
            .ToArray();
        var effects = RequireJsonArray(value, "unknownEffects").EnumerateArray()
            .Select(item =>
            {
                RequireExactJsonProperties(item, "stableTypeId", "instanceKey", "stateDigest");
                return new NativeExtensionOpaqueEffectDto(
                    RequireJsonString(item, "stableTypeId"),
                    RequireJsonString(item, "instanceKey"),
                    RequireSha256(RequireJsonString(item, "stateDigest"), "unknown effect"));
            })
            .OrderBy(item => item.StableTypeId, StringComparer.Ordinal)
            .ThenBy(item => item.InstanceKey, StringComparer.Ordinal)
            .ToArray();
        var lossy = RequireJsonArray(value, "lossyFields").EnumerateArray()
            .Select(item => item.GetString() ?? string.Empty)
            .Order(StringComparer.Ordinal)
            .ToArray();
        var approved = RequireJsonArray(value, "approvedLossyFields").EnumerateArray()
            .Select(item => item.GetString() ?? string.Empty)
            .Order(StringComparer.Ordinal)
            .ToArray();
        if (!lossy.SequenceEqual(approved, StringComparer.Ordinal) || lossy.Length > 0)
        {
            throw new BridgeValidationException(
                "This native driver currently supports only lossless in-place updates");
        }
        return new ParsedNativeExtensionPreservation(mode, preserved, effects);
    }

    private static void ValidateDescriptorReference(JsonElement value)
    {
        RequireExactJsonProperties(value, "descriptorId", "expectedDigest");
        _ = RequireJsonString(value, "descriptorId");
        _ = RequireSha256(
            RequireJsonString(value, "expectedDigest"),
            "descriptor expectedDigest");
    }

    private static void ValidatePlacement(JsonElement value)
    {
        RequireExactJsonProperties(value, "frame", "primaryLayer", "secondaryLayer");
        _ = RequireNonNegativeJsonInt(value, "frame");
        _ = RequireNonNegativeJsonInt(value, "primaryLayer");
        if (!value.TryGetProperty("secondaryLayer", out var secondary)
            || secondary.ValueKind != JsonValueKind.Null)
        {
            throw new BridgeValidationException(
                "The YMM4 native-extension driver requires placement.secondaryLayer to be null");
        }
    }

    private static void ValidateLosslessReplacementGuard(JsonElement value)
    {
        RequireExactJsonProperties(value, "approvedLossyFields");
        if (RequireJsonArray(value, "approvedLossyFields").GetArrayLength() != 0)
        {
            throw new BridgeValidationException(
                "The current native-extension driver does not approve lossy replacement fields");
        }
    }

    internal static void RequireExactJsonProperties(
        JsonElement value,
        params string[] expectedNames)
    {
        if (value.ValueKind != JsonValueKind.Object)
        {
            throw new BridgeValidationException(
                "Native-extension JSON value must be an object");
        }
        var expected = expectedNames.ToHashSet(StringComparer.Ordinal);
        foreach (var property in value.EnumerateObject())
        {
            if (!expected.Remove(property.Name))
            {
                throw new BridgeValidationException(
                    $"Unknown native-extension JSON property: {property.Name}");
            }
        }
        if (expected.Count != 0)
        {
            throw new BridgeValidationException(
                $"Missing native-extension JSON properties: "
                + string.Join(", ", expected.Order(StringComparer.Ordinal)));
        }
    }

    private static void RequireDistinctLogicalKeys(
        IReadOnlyList<ParsedNativeExtensionIntent> intents)
    {
        if (intents.Select(value => value.LogicalKey).Distinct(StringComparer.Ordinal).Count()
            != intents.Count)
        {
            throw new BridgeValidationException("Native-extension logical keys must be unique");
        }
    }

    private static JsonElement RequireJsonObject(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property)
            || property.ValueKind != JsonValueKind.Object)
        {
            throw new BridgeValidationException($"Native-extension JSON object is missing: {name}");
        }
        return property;
    }

    private static JsonElement RequireJsonArray(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property)
            || property.ValueKind != JsonValueKind.Array)
        {
            throw new BridgeValidationException($"Native-extension JSON array is missing: {name}");
        }
        return property;
    }

    private static string RequireJsonString(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property)
            || property.ValueKind != JsonValueKind.String
            || string.IsNullOrWhiteSpace(property.GetString()))
        {
            throw new BridgeValidationException($"Native-extension JSON string is missing: {name}");
        }
        return property.GetString()!;
    }

    private static int RequireJsonInt(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property) || !property.TryGetInt32(out var result))
        {
            throw new BridgeValidationException($"Native-extension JSON integer is missing: {name}");
        }
        return result;
    }

    private static int RequireNonNegativeJsonInt(JsonElement value, string name)
    {
        var result = RequireJsonInt(value, name);
        if (result < 0)
        {
            throw new BridgeValidationException($"Native-extension JSON integer is negative: {name}");
        }
        return result;
    }

    private static int RequirePositiveJsonInt(JsonElement value, string name)
    {
        var result = RequireJsonInt(value, name);
        if (result <= 0)
        {
            throw new BridgeValidationException($"Native-extension JSON integer is not positive: {name}");
        }
        return result;
    }

    private static ulong RequireJsonULong(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property) || !property.TryGetUInt64(out var result))
        {
            throw new BridgeValidationException($"Native-extension JSON unsigned integer is missing: {name}");
        }
        return result;
    }

    private static ulong RequirePositiveJsonULong(JsonElement value, string name)
    {
        var result = RequireJsonULong(value, name);
        if (result == 0)
        {
            throw new BridgeValidationException(
                $"Native-extension JSON unsigned integer is not positive: {name}");
        }
        return result;
    }

    private static bool RequireJsonBoolean(JsonElement value, string name)
    {
        if (!value.TryGetProperty(name, out var property)
            || property.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
        {
            throw new BridgeValidationException($"Native-extension JSON boolean is missing: {name}");
        }
        return property.GetBoolean();
    }

    private static Guid RequireJsonGuid(JsonElement value, string name)
    {
        var text = RequireJsonString(value, name);
        if (!Guid.TryParse(text, out var result))
        {
            throw new BridgeValidationException($"Native-extension JSON UUID is invalid: {name}");
        }
        return result;
    }

    private static void RequireJsonNumber(JsonElement value, string name, int expected)
    {
        if (RequireJsonInt(value, name) != expected)
        {
            throw new BridgeValidationException(
                $"Unsupported native-extension {name}; expected {expected}");
        }
    }

    private static string RequireSha256(string value, string field)
    {
        var hex = value.StartsWith("sha256:", StringComparison.Ordinal)
            ? value["sha256:".Length..]
            : value;
        if (hex.Length != 64 || !hex.All(Uri.IsHexDigit))
        {
            throw new BridgeValidationException($"Invalid SHA-256 for {field}");
        }
        return $"sha256:{hex.ToLowerInvariant()}";
    }

    private static void RequireNativeExtensionCatalog(
        string expectedDigest,
        DescriptorCatalogDto catalog)
    {
        if (!string.Equals(expectedDigest, catalog.CatalogDigest, StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Native-extension descriptor catalog changed after approval",
                SnapshotCore().Fingerprint);
        }
    }

    private static void VerifyNativeExtensionArtifacts(
        IReadOnlyList<NativeExtensionArtifactDto> artifacts)
    {
        var root = Path.GetFullPath(Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "TakeGraph",
            "native-extension-artifacts"));
        var rootPrefix = root.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar)
            + Path.DirectorySeparatorChar;
        var identities = new HashSet<string>(StringComparer.Ordinal);
        foreach (var artifact in artifacts)
        {
            var expected = RequireSha256(artifact.ArtifactDigest, "artifactDigest");
            var claimed = RequireSha256(artifact.Sha256, "artifact sha256");
            if (!string.Equals(expected, claimed, StringComparison.Ordinal)
                || !identities.Add(expected)
                || artifact.ByteLength == 0
                || artifact.Kind is not ("image" or "video" or "audio" or "bgm")
                || !MediaTypeMatchesKind(artifact.MediaType, artifact.Kind))
            {
                throw new BridgeValidationException(
                    "Native-extension artifact declaration is invalid or duplicated");
            }
            var path = Path.GetFullPath(artifact.Path);
            if (!path.StartsWith(rootPrefix, StringComparison.OrdinalIgnoreCase)
                || !File.Exists(path)
                || Directory.Exists(path))
            {
                throw new BridgeValidationException(
                    "Native-extension artifact is outside the authorized content-addressed root");
            }
            var info = new FileInfo(path);
            if (checked((ulong)info.Length) != artifact.ByteLength)
            {
                throw new BridgeValidationException("Native-extension artifact length changed");
            }
            using var stream = File.OpenRead(path);
            var actual = $"sha256:{Convert.ToHexStringLower(SHA256.HashData(stream))}";
            if (!ApplyRequestDigest.Matches(actual, expected))
            {
                throw new BridgeValidationException("Native-extension artifact bytes changed");
            }
        }
    }

    private static bool MediaTypeMatchesKind(string mediaType, string kind)
    {
        return kind switch
        {
            "image" => mediaType.StartsWith("image/", StringComparison.Ordinal),
            "video" => mediaType.StartsWith("video/", StringComparison.Ordinal),
            "audio" or "bgm" => mediaType.StartsWith("audio/", StringComparison.Ordinal),
            _ => false,
        };
    }

    private static void ValidateNativeExtensionArtifactBindings(
        IReadOnlyList<ParsedNativeExtensionIntent> intents,
        IReadOnlyList<NativeExtensionArtifactDto> artifacts)
    {
        var required = intents.Where(value => value.ArtifactDigest is not null)
            .Select(value => RequireSha256(value.ArtifactDigest!, "intent artifactDigest"))
            .Distinct(StringComparer.Ordinal)
            .Order(StringComparer.Ordinal)
            .ToArray();
        var supplied = artifacts.Select(value => RequireSha256(
                value.ArtifactDigest,
                "artifactDigest"))
            .Distinct(StringComparer.Ordinal)
            .Order(StringComparer.Ordinal)
            .ToArray();
        if (!required.SequenceEqual(supplied, StringComparer.Ordinal))
        {
            throw new BridgeValidationException(
                "Native-extension artifacts must exactly match all asset intents");
        }
    }

    private static NativeExtensionObservationDto ObserveNativeExtensions(
        IReadOnlyList<ParsedNativeExtensionIntent> intents,
        Guid? operationId = null,
        IReadOnlyDictionary<string, Guid>? plannedRealizations = null)
    {
        var timelineViewModel = GetMember(RequireMainViewModel(), "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var rawItems = ReadItems(timelineViewModel);
        var groups = ReadNativeExtensionGroups(rawItems);
        var existing = new SortedDictionary<string, NativeExtensionExistingDto>(StringComparer.Ordinal);
        foreach (var intent in intents)
        {
            if (intent.Kind == "managed_effect")
            {
                var parent = FindNativeExtensionParent(intent.TargetEntityId!, rawItems);
                if (parent is not null
                    && TryReadManagedEffect(parent, intent.EffectInstanceId!, out var effect)
                    && effect is not null)
                {
                    existing.Add(intent.LogicalKey, new NativeExtensionExistingDto(
                        intent.LogicalKey,
                        effect.Marker.RealizationId,
                        "managed_effect",
                        new NativeExtensionUpdateModeDto("in_place"),
                        ReadPreservedFields([parent], excludeEffects: true),
                        ReadUnknownEffects(parent)));
                }
                else if (parent is not null && intent.EffectAction == "upsert")
                {
                    var realizationId = plannedRealizations is not null
                        && plannedRealizations.TryGetValue(intent.LogicalKey, out var planned)
                            ? planned
                            : operationId is not null
                                ? DeterministicNativeExtensionRealization(
                                    operationId.Value,
                                    intent.LogicalKey)
                                : throw new BridgeUnavailableException(
                                    "Native-extension effect slot has no deterministic operation identity");
                    existing.Add(intent.LogicalKey, new NativeExtensionExistingDto(
                        intent.LogicalKey,
                        realizationId,
                        "managed_effect",
                        new NativeExtensionUpdateModeDto("in_place"),
                        ReadPreservedFields([parent], excludeEffects: true),
                        ReadUnknownEffects(parent)));
                }
                continue;
            }
            if (!groups.TryGetValue(intent.LogicalKey, out var group))
            {
                continue;
            }
            if (!string.Equals(group.Kind, intent.Kind, StringComparison.Ordinal))
            {
                throw new BridgeConflictException(
                    $"Native-extension logical key {intent.LogicalKey} has kind {group.Kind}, expected {intent.Kind}",
                    SnapshotCore().Fingerprint);
            }
            existing.Add(intent.LogicalKey, new NativeExtensionExistingDto(
                intent.LogicalKey,
                group.RealizationId,
                group.Kind,
                new NativeExtensionUpdateModeDto("in_place"),
                ReadPreservedFields(group.Items, excludeEffects: false),
                ReadGroupUnknownEffects(group.Items)));
        }
        return new NativeExtensionObservationDto(existing);
    }

    private static IReadOnlyDictionary<string, NativeExtensionGroup> ReadNativeExtensionGroups(
        IReadOnlyList<RawItem> rawItems,
        string? expectedProjectId = null,
        string? conflictFingerprint = null)
    {
        var projectId = expectedProjectId ?? CurrentProjectId();
        var markedItems = rawItems
            .Select(item => (Item: item, Parsed: NativeExtensionRemarkCodec.TryDecode(
                item.Remark,
                out var marker) ? marker : null))
            .Where(value => value.Parsed is not null)
            .ToArray();
        if (markedItems.Any(value => !string.Equals(
                value.Parsed!.ProjectId,
                projectId,
                StringComparison.Ordinal)))
        {
            throw new BridgeConflictException(
                "The active timeline contains a native-extension identity owned by a foreign project",
                conflictFingerprint ?? SnapshotCore().Fingerprint);
        }
        var result = new SortedDictionary<string, NativeExtensionGroup>(StringComparer.Ordinal);
        foreach (var grouping in markedItems
                     .Where(value => value.Parsed!.Kind != "managed_effect_host")
                     .GroupBy(value => value.Parsed!.LogicalKey, StringComparer.Ordinal))
        {
            var marked = grouping.ToArray();
            var marker = marked[0].Parsed!;
            if (marked.Any(value =>
                    value.Parsed!.RealizationId != marker.RealizationId
                    || value.Parsed.Kind != marker.Kind
                    || value.Parsed.EntityId != marker.EntityId
                    || value.Parsed.Revision != marker.Revision
                    || value.Parsed.DescriptorId != marker.DescriptorId
                    || value.Parsed.PartCount != marker.PartCount)
                || marked.Length != marker.PartCount
                || marked.Select(value => value.Parsed!.PartIndex).Distinct().Count() != marked.Length
                || marked.Any(value => value.Parsed!.PartIndex < 0
                    || value.Parsed.PartIndex >= marked.Length))
            {
                throw new BridgeConflictException(
                    $"Native-extension realization is ambiguous: {grouping.Key}",
                    conflictFingerprint ?? SnapshotCore().Fingerprint);
            }
            result.Add(grouping.Key, new NativeExtensionGroup(
                grouping.Key,
                marker.RealizationId,
                marker.Kind,
                marker.ProjectId,
                marker.EntityId,
                marker.Revision,
                marker.DescriptorId,
                marked.OrderBy(value => value.Parsed!.PartIndex)
                    .Select(value => value.Item)
                    .ToArray()));
        }
        return result;
    }

    /// <summary>
    /// Builds the reconciliation projection directly from the current YMM4
    /// objects. The allowlist mirrors the Rust projection profile. It must not
    /// contain preservation digests, template footprint/state digests, or
    /// unknown effects.
    /// </summary>
    private static IReadOnlyList<ManagedNativeExtensionDto> ReadManagedNativeExtensions(
        IReadOnlyList<RawItem> rawItems,
        string projectId,
        string conflictFingerprint)
    {
        var result = new List<ManagedNativeExtensionDto>();
        var groups = ReadNativeExtensionGroups(rawItems, projectId, conflictFingerprint);
        foreach (var group in groups.Values)
        {
            var first = group.Items[0];
            var fields = NativeExtensionSnapshotBaseFields(
                group.LogicalKey,
                group.ProjectId,
                group.EntityId,
                group.Revision,
                group.Kind);
            fields["frame"] = Invariant(first.Frame);
            fields["layer"] = Invariant(first.Layer);
            switch (group.Kind)
            {
                case "portrait":
                case "face":
                    fields["length"] = Invariant(first.Length);
                    fields["descriptorId"] = ResolveLiveCharacterDescriptorId(
                        GetMember(first.Item, "Character"),
                        conflictFingerprint);
                    break;
                case "image":
                case "video":
                case "audio":
                case "bgm":
                {
                    fields["length"] = Invariant(first.Length);
                    var path = GetString(first.Item, "FilePath");
                    if (string.IsNullOrWhiteSpace(path) || !File.Exists(path))
                    {
                        throw new BridgeConflictException(
                            $"Native-extension artifact is missing during snapshot: {group.LogicalKey}",
                            conflictFingerprint);
                    }
                    using var stream = File.OpenRead(path);
                    fields["artifactDigest"] =
                        $"sha256:{Convert.ToHexStringLower(SHA256.HashData(stream))}";
                    fields["byteLength"] = new FileInfo(path).Length.ToString(
                        System.Globalization.CultureInfo.InvariantCulture);
                    fields["loopPlayback"] = ReadOptionalLoop(first.Item)
                        .ToString().ToLowerInvariant();
                    break;
                }
                case "template":
                    fields["descriptorId"] = group.DescriptorId
                        ?? throw new BridgeConflictException(
                            $"Native template marker has no descriptor: {group.LogicalKey}",
                            conflictFingerprint);
                    fields["partCount"] = Invariant(group.Items.Count);
                    break;
                default:
                    throw new BridgeConflictException(
                        $"Unsupported native-extension marker kind: {group.Kind}",
                        conflictFingerprint);
            }
            result.Add(new ManagedNativeExtensionDto(
                group.LogicalKey,
                group.RealizationId,
                group.Kind,
                group.ProjectId,
                group.EntityId,
                group.Revision,
                fields));
        }

        foreach (var parent in rawItems)
        {
            if (!NativeExtensionRemarkCodec.TryDecode(parent.Remark, out var marker)
                || marker is null
                || !string.Equals(marker.ProjectId, projectId, StringComparison.Ordinal))
            {
                continue;
            }
            foreach (var pair in marker.Effects.OrderBy(value => value.Key, StringComparer.Ordinal))
            {
                var effectMarker = pair.Value;
                var effect = ResolveMarkedEffect(parent.Item, effectMarker);
                if (effect is null)
                {
                    // A stale marker is represented as a missing managed
                    // realization, allowing reconciliation to report drift.
                    continue;
                }
                var logicalKey = $"effect:{marker.EntityId}:{effectMarker.EffectInstanceId}";
                var fields = NativeExtensionSnapshotBaseFields(
                    logicalKey,
                    marker.ProjectId,
                    marker.EntityId,
                    marker.Revision,
                    "managed_effect");
                fields["descriptorId"] = effectMarker.DescriptorId;
                fields["stableTypeId"] = effect.GetType().FullName ?? effect.GetType().Name;
                fields["collection"] = effectMarker.Collection;
                fields["parametersDigest"] = CanonicalJson.Sha256(
                    "takegraph-ymm4-native-extension-effect-parameters-v1",
                    JsonSerializer.Deserialize<JsonElement>("{}"));
                result.Add(new ManagedNativeExtensionDto(
                    logicalKey,
                    effectMarker.RealizationId,
                    "managed_effect",
                    marker.ProjectId,
                    marker.EntityId,
                    marker.Revision,
                    fields));
            }
        }
        return result.OrderBy(value => value.LogicalKey, StringComparer.Ordinal)
            .ThenBy(value => value.RealizationId)
            .ToArray();
    }

    private static SortedDictionary<string, string> NativeExtensionSnapshotBaseFields(
        string logicalKey,
        string projectId,
        string entityId,
        ulong revision,
        string kind)
    {
        return new SortedDictionary<string, string>(StringComparer.Ordinal)
        {
            ["logicalKey"] = logicalKey,
            ["projectId"] = projectId,
            ["entityId"] = entityId,
            ["entityRevision"] = revision.ToString(
                System.Globalization.CultureInfo.InvariantCulture),
            ["kind"] = kind,
        };
    }

    private static string ResolveLiveCharacterDescriptorId(
        object? character,
        string conflictFingerprint)
    {
        if (character is null)
        {
            throw new BridgeConflictException(
                "Native portrait character binding is missing",
                conflictFingerprint);
        }
        var timeline = GetMember(RequireMainViewModel(), "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        if (GetMember(timeline, "Characters") is not IEnumerable characterValues)
        {
            throw new BridgeUnavailableException("YMM4 character catalog is unavailable");
        }
        var descriptors = new List<(object Character, string DescriptorId, bool Bindable)>();
        foreach (var group in characterValues.Cast<object>()
                     .Where(value => value is not null)
                     .GroupBy(value => GetString(value, "Name"), StringComparer.Ordinal))
        {
            var values = group.ToArray();
            for (var index = 0; index < values.Length; index++)
            {
                var value = values[index];
                var metadata = new SortedDictionary<string, string>(StringComparer.Ordinal)
                {
                    ["groupName"] = GetString(value, "GroupName"),
                    ["voiceType"] = GetMember(value, "Voice")?.GetType().FullName ?? string.Empty,
                    ["voiceParameterType"] = GetMember(value, "VoiceParameter")?.GetType().FullName
                        ?? string.Empty,
                    ["tachieType"] = GetMember(value, "TachieType")?.ToString() ?? string.Empty,
                    ["tachieCharacterParameterType"] = GetMember(value, "TachieCharacterParameter")
                        ?.GetType().FullName ?? string.Empty,
                };
                var configDigest = HashDescriptor("character-config", metadata);
                descriptors.Add((
                    value,
                    $"ymm4-character:{Hash($"character|{group.Key}|{configDigest}|{(values.Length == 1 ? 0 : index)}")}",
                    values.Length == 1 && !string.IsNullOrWhiteSpace(group.Key)));
            }
        }
        var matches = descriptors.Where(value =>
                ReferenceEquals(value.Character, character) && value.Bindable)
            .ToArray();
        return matches.Length == 1
            ? matches[0].DescriptorId
            : throw new BridgeConflictException(
                "Native portrait character binding is no longer exactly bindable",
                conflictFingerprint);
    }

    private static string Invariant(int value) =>
        value.ToString(System.Globalization.CultureInfo.InvariantCulture);

    private static RawItem? FindNativeExtensionParent(
        string entityId,
        IReadOnlyList<RawItem> rawItems)
    {
        var projectId = CurrentProjectId();
        var markedCandidates = rawItems.Select(item =>
        {
            if (NativeExtensionRemarkCodec.TryDecode(item.Remark, out var extension)
                && extension?.EntityId == entityId
                && extension.PartIndex == 0)
            {
                return (Item: item, ProjectId: extension.ProjectId, Matches: true);
            }
            if (item.TypeName.EndsWith(".VoiceItem", StringComparison.Ordinal)
                && RemarkCodec.TryDecode(item.Remark, out var voice)
                && voice?.EntityId == entityId)
            {
                return (Item: item, ProjectId: voice.ProjectId, Matches: true);
            }
            return (Item: item, ProjectId: string.Empty, Matches: false);
        }).Where(value => value.Matches).ToArray();
        if (markedCandidates.Any(value => !string.Equals(
                value.ProjectId,
                projectId,
                StringComparison.Ordinal)))
        {
            throw new BridgeConflictException(
                $"Native-extension effect target is owned by a foreign project: {entityId}",
                SnapshotCore().Fingerprint);
        }
        var candidates = markedCandidates.Select(value => value.Item).ToArray();
        return candidates.Length switch
        {
            0 => null,
            1 => candidates[0],
            _ => throw new BridgeConflictException(
                $"Native-extension effect target is ambiguous: {entityId}",
                SnapshotCore().Fingerprint),
        };
    }

    private static IReadOnlyList<NativeExtensionPreservedFieldDto> ReadPreservedFields(
        IReadOnlyList<RawItem> items,
        bool excludeEffects)
    {
        return items.Select((item, index) => new NativeExtensionPreservedFieldDto(
                $"part:{index}:targetLocalState",
                ComputeExtensionPreservedStateDigest(item.Item, excludeEffects)))
            .ToArray();
    }

    internal static string ComputeExtensionPreservedStateDigest(object item, bool excludeEffects)
    {
        var excluded = new HashSet<string>(
            ["Frame", "Layer", "Length", "FilePath", "Character", "Remark"],
            StringComparer.Ordinal);
        if (excludeEffects)
        {
            excluded.Add("VideoEffects");
            excluded.Add("AudioEffects");
            excluded.Add("Effects");
        }
        var canonical = new StringBuilder("takegraph-ymm4-native-extension-preserved-v2\n");
        foreach (var property in item.GetType()
                     .GetProperties(BindingFlags.Public | BindingFlags.Instance)
                     .Where(value => value.CanRead
                         && (value.CanWrite || value.Name == "KeyFrames")
                         && value.GetIndexParameters().Length == 0
                         && !excluded.Contains(value.Name))
                     .OrderBy(value => value.Name, StringComparer.Ordinal))
        {
            try
            {
                AppendDigestValue(canonical, property.Name);
                AppendDigestValue(canonical, SerializeYmmValue(property.GetValue(item)));
            }
            catch (BridgeUnavailableException)
            {
                throw;
            }
            catch (Exception error)
            {
                throw new BridgeUnavailableException(
                    $"YMM4 preservation member read failed: "
                    + $"{item.GetType().FullName}.{property.Name}: "
                    + error.GetBaseException().Message);
            }
        }
        return $"sha256:{Hash(canonical.ToString())}";
    }

    private static string NativeExtensionStateDigest(IEnumerable<object> items)
    {
        var canonical = new StringBuilder("takegraph-ymm4-native-extension-state-v2\n");
        foreach (var item in items)
        {
            AppendDigestValue(canonical, item.GetType().AssemblyQualifiedName ?? item.GetType().FullName ?? string.Empty);
            AppendDigestValue(canonical, SerializeYmmValue(item));
        }
        return $"sha256:{Hash(canonical.ToString())}";
    }

    private static IReadOnlyList<NativeExtensionOpaqueEffectDto> ReadUnknownEffects(RawItem raw)
    {
        NativeExtensionRemarkCodec.TryDecode(raw.Remark, out var marker);
        var managed = new HashSet<object>(ReferenceEqualityComparer.Instance);
        if (marker is not null)
        {
            EnsureNativeExtensionMarkerOwnership(marker.ProjectId);
            foreach (var managedMarker in marker.Effects.Values)
            {
                var resolved = ResolveMarkedEffect(raw.Item, managedMarker);
                if (resolved is not null)
                {
                    managed.Add(resolved);
                }
            }
        }
        var result = new List<NativeExtensionOpaqueEffectDto>();
        foreach (var collection in ReadEffectCollections(raw.Item))
        {
            var ordinals = new Dictionary<string, int>(StringComparer.Ordinal);
            foreach (var effect in collection.Items)
            {
                var type = effect.GetType().FullName ?? effect.GetType().Name;
                var ordinal = ordinals.GetValueOrDefault(type);
                ordinals[type] = ordinal + 1;
                if (managed.Contains(effect))
                {
                    continue;
                }
                result.Add(new NativeExtensionOpaqueEffectDto(
                    type,
                    $"{collection.Name}:{type}:{ordinal}",
                    NativeExtensionStateDigest([effect])));
            }
        }
        return result.OrderBy(value => value.StableTypeId, StringComparer.Ordinal)
            .ThenBy(value => value.InstanceKey, StringComparer.Ordinal)
            .ToArray();
    }

    private static IReadOnlyList<NativeExtensionOpaqueEffectDto> ReadGroupUnknownEffects(
        IReadOnlyList<RawItem> items)
    {
        return items.SelectMany((item, partIndex) => ReadUnknownEffects(item)
                .Select(effect => effect with
                {
                    InstanceKey = $"part:{partIndex}:{effect.InstanceKey}",
                }))
            .OrderBy(value => value.StableTypeId, StringComparer.Ordinal)
            .ThenBy(value => value.InstanceKey, StringComparer.Ordinal)
            .ToArray();
    }

    private static IReadOnlyList<EffectCollectionView> ReadEffectCollections(object item)
    {
        var result = new List<EffectCollectionView>();
        foreach (var name in new[] { "VideoEffects", "AudioEffects", "Effects" })
        {
            var property = item.GetType().GetProperty(
                name,
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance);
            if (property?.GetValue(item) is not IEnumerable values)
            {
                continue;
            }
            result.Add(new EffectCollectionView(
                name,
                property,
                values.Cast<object?>().Where(value => value is not null).Cast<object>().ToArray()));
        }
        return result;
    }

    private static bool TryReadManagedEffect(
        RawItem parent,
        string effectInstanceId,
        out ManagedEffectView? result)
    {
        result = null;
        if (!NativeExtensionRemarkCodec.TryDecode(parent.Remark, out var marker)
            || marker is null
            || !marker.Effects.TryGetValue(effectInstanceId, out var effectMarker))
        {
            return false;
        }
        EnsureNativeExtensionMarkerOwnership(marker.ProjectId);
        var effect = ResolveMarkedEffect(parent.Item, effectMarker)
            ?? throw new BridgeConflictException(
                $"Managed native effect marker is stale: {effectInstanceId}",
                SnapshotCore().Fingerprint);
        result = new ManagedEffectView(marker, effectMarker, effect);
        return true;
    }

    private static void EnsureNativeExtensionMarkerOwnership(string markerProjectId)
    {
        if (!string.Equals(markerProjectId, CurrentProjectId(), StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Native-extension marker is owned by a foreign project",
                SnapshotCore().Fingerprint);
        }
    }

    private static object? ResolveMarkedEffect(
        object parent,
        NativeExtensionEffectMarker marker)
    {
        var collection = ReadEffectCollections(parent)
            .SingleOrDefault(value => value.Name == marker.Collection);
        if (collection is null)
        {
            return null;
        }
        if (marker.Index >= 0
            && marker.Index < collection.Items.Count
            && string.Equals(
                collection.Items[marker.Index].GetType().FullName,
                marker.StableTypeId,
                StringComparison.Ordinal))
        {
            return collection.Items[marker.Index];
        }
        var candidates = collection.Items.Where(value => string.Equals(
                value.GetType().FullName,
                marker.StableTypeId,
                StringComparison.Ordinal))
            .ToArray();
        return candidates.Length == 1 ? candidates[0] : null;
    }

    private static NativeExtensionPreparation PrepareNativeExtensionApply(
        IReadOnlyList<ParsedNativeExtensionOperation> operations)
    {
        var main = RequireMainViewModel();
        var timelineViewModel = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var timeline = GetField(timelineViewModel, "timeline")
            ?? GetMember(timelineViewModel, "Timeline")
            ?? throw new BridgeUnavailableException("YMM4 timeline mutation API is unavailable");
        var rawItems = ReadItems(timelineViewModel);
        var groups = ReadNativeExtensionGroups(rawItems);
        var touched = new HashSet<object>(ReferenceEqualityComparer.Instance);
        foreach (var operation in operations)
        {
            if (operation.Intent.Kind == "managed_effect")
            {
                var parent = FindNativeExtensionParent(operation.Intent.TargetEntityId!, rawItems);
                if (parent is not null)
                {
                    touched.Add(parent.Item);
                }
            }
            else if (groups.TryGetValue(operation.Intent.LogicalKey, out var group))
            {
                foreach (var item in group.Items)
                {
                    touched.Add(item.Item);
                }
            }
        }
        var fingerprint = SnapshotCore().Fingerprint;
        foreach (var item in touched)
        {
            EnsureUnlockedForMutation(item, "native extension target", fingerprint);
        }
        var witness = rawItems.Where(value => !touched.Contains(value.Item))
            .ToDictionary(
                value => value.Item,
                value => NativeExtensionStateDigest([value.Item]),
                ReferenceEqualityComparer.Instance);
        return new NativeExtensionPreparation(
            RequireMainModel(main),
            timelineViewModel,
            timeline,
            touched.ToArray(),
            rawItems.Select(value => value.Item).ToArray(),
            witness,
            ObserveNativeExtensions(
                operations.Select(value => value.Intent).ToArray(),
                plannedRealizations: operations.ToDictionary(
                    value => value.Intent.LogicalKey,
                    value => value.RealizationId,
                    StringComparer.Ordinal)));
    }

    private static void ValidateNativeExtensionPlanObservation(
        IReadOnlyList<ParsedNativeExtensionOperation> operations,
        NativeExtensionObservationDto observation)
    {
        foreach (var operation in operations)
        {
            observation.Existing.TryGetValue(operation.Intent.LogicalKey, out var existing);
            var expectsExisting = operation.Action is "update" or "delete";
            var expectsAbsent = operation.Action is "create" or "instantiate";
            if ((expectsExisting && existing is null) || (expectsAbsent && existing is not null))
            {
                throw new BridgeConflictException(
                    $"Native-extension action no longer matches target existence: {operation.Intent.LogicalKey}",
                    SnapshotCore().Fingerprint);
            }
            if (existing is not null
                && (existing.RealizationId != operation.RealizationId
                    || !string.Equals(existing.Kind, operation.Intent.Kind, StringComparison.Ordinal)
                    || existing.UpdateMode.Mode != "in_place"))
            {
                throw new BridgeConflictException(
                    $"Native-extension identity/kind changed: {operation.Intent.LogicalKey}",
                    SnapshotCore().Fingerprint);
            }
            if (operation.Preservation.Mode == "create" != expectsAbsent)
            {
                throw new BridgeValidationException(
                    $"Native-extension preservation mode differs from action: {operation.Intent.LogicalKey}");
            }
            if (existing is not null
                && (!SequenceEqual(existing.PreservedFields, operation.Preservation.PreservedFields)
                    || !SequenceEqual(existing.UnknownEffects, operation.Preservation.UnknownEffects)))
            {
                throw new BridgeConflictException(
                    $"Preserved native state drifted: {operation.Intent.LogicalKey}",
                    SnapshotCore().Fingerprint);
            }
            var validAction = operation.Intent.Kind switch
            {
                "template" => operation.Action == "instantiate",
                "managed_effect" when operation.Intent.EffectAction == "remove" => operation.Action == "delete",
                "managed_effect" => operation.Action is "create" or "update",
                _ => operation.Action is "create" or "update",
            };
            if (!validAction)
            {
                throw new BridgeValidationException(
                    $"Native-extension action is invalid for {operation.Intent.LogicalKey}");
            }
        }
    }

    private static bool SequenceEqual<T>(IReadOnlyList<T> left, IReadOnlyList<T> right)
        where T : notnull
    {
        return left.Count == right.Count && left.SequenceEqual(right);
    }

    private void ValidateNativeExtensionCapabilities(
        IReadOnlyList<ParsedNativeExtensionOperation> operations)
    {
        var capabilities = Capabilities().Capabilities.ToHashSet(StringComparer.Ordinal);
        foreach (var operation in operations)
        {
            var required = operation.Intent.Kind switch
            {
                "portrait" => "native_portrait_upsert",
                "face" => "native_face_upsert",
                "image" => "native_image_upsert",
                "video" => "native_video_upsert",
                "audio" or "bgm" => "native_audio_upsert",
                "managed_effect" => "native_effect_typed_mutation",
                "template" => "native_template_instantiate",
                _ => string.Empty,
            };
            if (!capabilities.Contains(required))
            {
                throw new BridgeUnavailableException(
                    $"Required native-extension capability is unavailable: {required}");
            }
        }
    }

    private static void EnsureNativeExtensionReceiptBinding(
        NativeExtensionApplyRequestDto request,
        NativeExtensionApplyResponseDto receipt)
    {
        if (receipt.OperationId != request.OperationId
            || !ApplyRequestDigest.Matches(receipt.RequestDigest, request.RequestDigest)
            || !string.Equals(receipt.ProjectId, request.ProjectId, StringComparison.Ordinal)
            || !string.Equals(receipt.SceneId, request.SceneId, StringComparison.Ordinal)
            || !string.Equals(
                receipt.DescriptorCatalogDigest,
                request.DescriptorCatalogDigest,
                StringComparison.Ordinal)
            || !string.Equals(
                receipt.DriverProfileDigest,
                request.DriverProfileDigest,
                StringComparison.Ordinal)
            || !string.Equals(
                receipt.BeforeFingerprint,
                request.ExpectedFingerprint,
                StringComparison.Ordinal))
        {
            throw new BridgeConflictException(
                "Native-extension operation ID is already bound to another request",
                receipt.BeforeFingerprint);
        }
    }

    private static bool UnmanagedWitnessMatches(NativeExtensionPreparation preparation)
    {
        var currentItems = ReadItems(preparation.TimelineViewModel)
            .Select(value => value.Item)
            .ToArray();
        var current = currentItems.ToHashSet(ReferenceEqualityComparer.Instance);
        if (!ExactNativeExtensionItemSetMatches(
                preparation.BeforeItemIdentities,
                preparation.AddedItems,
                currentItems))
        {
            return false;
        }
        return preparation.UnmanagedWitness.All(pair =>
            current.TryGetValue(pair.Key, out var raw)
            && string.Equals(
                NativeExtensionStateDigest([raw]),
                pair.Value,
                StringComparison.Ordinal));
    }

    internal static bool ExactNativeExtensionItemSetMatches(
        IReadOnlyList<object> before,
        IReadOnlyList<object> approvedAdded,
        IReadOnlyList<object> after)
    {
        var expected = before.ToHashSet(ReferenceEqualityComparer.Instance);
        expected.UnionWith(approvedAdded);
        var actual = after.ToHashSet(ReferenceEqualityComparer.Instance);
        return expected.Count == actual.Count && expected.SetEquals(actual);
    }

    private static async Task ApplyNativeExtensionOperationsAsync(
        IReadOnlyList<ParsedNativeExtensionOperation> operations,
        IReadOnlyList<NativeExtensionArtifactDto> artifacts,
        DescriptorCatalogDto catalog,
        NativeExtensionPreparation preparation)
    {
        var artifactMap = artifacts.ToDictionary(
            value => RequireSha256(value.ArtifactDigest, "artifactDigest"),
            StringComparer.Ordinal);
        foreach (var operation in operations)
        {
            if (operation.Intent.Kind == "managed_effect")
            {
                await Application.Current.Dispatcher.InvokeAsync(() =>
                    ApplyManagedEffect(operation, catalog));
            }
            else if (operation.Action == "create")
            {
                await CreateNativeExtensionItemAsync(
                    operation,
                    artifactMap,
                    catalog,
                    preparation).ConfigureAwait(false);
            }
            else if (operation.Action == "update")
            {
                await Application.Current.Dispatcher.InvokeAsync(() =>
                    UpdateNativeExtensionItem(operation, artifactMap, catalog));
            }
            else if (operation.Action == "instantiate")
            {
                await InstantiateNativeTemplateAsync(operation, catalog, preparation)
                    .ConfigureAwait(false);
            }
            else
            {
                throw new BridgeValidationException(
                    $"Unsupported native-extension action: {operation.Action}");
            }
            if (preparation.AddedItems.Count > 0 || operation.Intent.Kind == "managed_effect")
            {
                BridgeFaultInjection.ThrowIf("after_partial_mutation");
            }
        }
    }

    private static async Task CreateNativeExtensionItemAsync(
        ParsedNativeExtensionOperation operation,
        IReadOnlyDictionary<string, NativeExtensionArtifactDto> artifacts,
        DescriptorCatalogDto catalog,
        NativeExtensionPreparation preparation)
    {
        var intent = operation.Intent;
        var (methodName, argument) = intent.Kind switch
        {
            "portrait" => ("AddTachieItem", ResolveCharacterDescriptor(intent.DescriptorId!, catalog)),
            "face" => ("AddFaceItem", ResolveCharacterDescriptor(intent.DescriptorId!, catalog)),
            "image" => ("AddImageItem", ResolveArtifactPath(intent, artifacts)),
            "video" => ("AddVideoItem", ResolveArtifactPath(intent, artifacts)),
            "audio" or "bgm" => ("AddAudioItem", ResolveArtifactPath(intent, artifacts)),
            _ => throw new BridgeValidationException(
                $"Unsupported native-extension create kind: {intent.Kind}"),
        };
        var created = await InvokeNativeAddAsync(
            preparation,
            methodName,
            intent.Frame,
            intent.Layer,
            argument).ConfigureAwait(false);
        var expectedSuffix = intent.Kind switch
        {
            "portrait" => ".TachieItem",
            "face" => ".TachieFaceItem",
            "image" => ".ImageItem",
            "video" => ".VideoItem",
            "audio" or "bgm" => ".AudioItem",
            _ => string.Empty,
        };
        var matches = created.Where(value => value.TypeName.EndsWith(
                expectedSuffix,
                StringComparison.Ordinal))
            .ToArray();
        if (created.Length != 1 || matches.Length != 1)
        {
            throw new BridgeUnavailableException(
                $"YMM4 {methodName} produced an ambiguous item set");
        }
        await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            var item = matches[0].Item;
            SetRequired(item, intent.Frame, "Frame");
            SetRequired(item, intent.Layer, "Layer");
            SetRequired(item, intent.Length, "Length");
            if (intent.Kind is "audio" or "bgm" or "image" or "video")
            {
                SetRequired(item, ResolveArtifactPath(intent, artifacts), "FilePath");
                SetOptionalLoop(item, intent.LoopPlayback);
            }
            SetRequired(
                item,
                NativeExtensionRemarkCodec.Append(
                    matches[0].Remark,
                    NativeExtensionRemarkCodec.Create(
                        SnapshotCore().ProjectId,
                        intent.EntityId,
                        intent.EntityRevision,
                        intent.LogicalKey,
                        operation.RealizationId,
                        intent.Kind,
                        0,
                        1,
                        descriptorId: intent.DescriptorId)),
                "Remark");
            preparation.AddedItems.Add(item);
        });
    }

    private static void UpdateNativeExtensionItem(
        ParsedNativeExtensionOperation operation,
        IReadOnlyDictionary<string, NativeExtensionArtifactDto> artifacts,
        DescriptorCatalogDto catalog)
    {
        var timelineViewModel = GetMember(RequireMainViewModel(), "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var groups = ReadNativeExtensionGroups(ReadItems(timelineViewModel));
        if (!groups.TryGetValue(operation.Intent.LogicalKey, out var group)
            || group.Items.Count != 1
            || group.RealizationId != operation.RealizationId)
        {
            throw new BridgeConflictException(
                $"Native-extension update target is missing or ambiguous: {operation.Intent.LogicalKey}",
                SnapshotCore().Fingerprint);
        }
        var raw = group.Items[0];
        var beforePreserved = ComputeExtensionPreservedStateDigest(raw.Item, excludeEffects: false);
        SetRequired(raw.Item, operation.Intent.Frame, "Frame");
        SetRequired(raw.Item, operation.Intent.Layer, "Layer");
        SetRequired(raw.Item, operation.Intent.Length, "Length");
        if (operation.Intent.Kind is "portrait" or "face")
        {
            SetRequired(
                raw.Item,
                ResolveCharacterDescriptor(operation.Intent.DescriptorId!, catalog),
                "Character");
        }
        else
        {
            SetRequired(raw.Item, ResolveArtifactPath(operation.Intent, artifacts), "FilePath");
            SetOptionalLoop(raw.Item, operation.Intent.LoopPlayback);
        }
        NativeExtensionRemarkCodec.TryDecode(raw.Remark, out var existingMarker);
        var marker = existingMarker is null
            ? NativeExtensionRemarkCodec.Create(
                SnapshotCore().ProjectId,
                operation.Intent.EntityId,
                operation.Intent.EntityRevision,
                operation.Intent.LogicalKey,
                operation.RealizationId,
                operation.Intent.Kind,
                0,
                1,
                descriptorId: operation.Intent.DescriptorId)
            : existingMarker with
            {
                ProjectId = SnapshotCore().ProjectId,
                EntityId = operation.Intent.EntityId,
                Revision = operation.Intent.EntityRevision,
                LogicalKey = operation.Intent.LogicalKey,
                RealizationId = operation.RealizationId,
                Kind = operation.Intent.Kind,
                PartIndex = 0,
                PartCount = 1,
                DescriptorId = operation.Intent.DescriptorId,
            };
        SetRequired(raw.Item, NativeExtensionRemarkCodec.Append(raw.Remark, marker), "Remark");
        var afterPreserved = ComputeExtensionPreservedStateDigest(raw.Item, excludeEffects: false);
        if (!string.Equals(beforePreserved, afterPreserved, StringComparison.Ordinal))
        {
            throw new BridgeUnavailableException(
                $"YMM4 native update changed target-local state: {operation.Intent.LogicalKey}");
        }
    }

    private static async Task InstantiateNativeTemplateAsync(
        ParsedNativeExtensionOperation operation,
        DescriptorCatalogDto catalog,
        NativeExtensionPreparation preparation)
    {
        var template = ResolveTemplateDescriptor(operation.Intent.DescriptorId!, catalog);
        var created = await InvokeNativeAddAsync(
            preparation,
            "AddTemplateItemAsync",
            operation.Intent.Frame,
            operation.Intent.Layer,
            template).ConfigureAwait(false);
        if (created.Length == 0)
        {
            throw new BridgeUnavailableException("YMM4 native template produced no items");
        }
        await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            for (var index = 0; index < created.Length; index++)
            {
                var raw = created[index];
                var marker = NativeExtensionRemarkCodec.Create(
                    SnapshotCore().ProjectId,
                    operation.Intent.EntityId,
                    operation.Intent.EntityRevision,
                    operation.Intent.LogicalKey,
                    operation.RealizationId,
                    "template",
                    index,
                    created.Length,
                    descriptorId: operation.Intent.DescriptorId);
                SetRequired(
                    raw.Item,
                    NativeExtensionRemarkCodec.Append(raw.Remark, marker),
                    "Remark");
                preparation.AddedItems.Add(raw.Item);
            }
        });
    }

    private static async Task<RawItem[]> InvokeNativeAddAsync(
        NativeExtensionPreparation preparation,
        string methodName,
        int frame,
        int layer,
        object argument)
    {
        var before = await Application.Current.Dispatcher.InvokeAsync(() => ReadItems(
                preparation.TimelineViewModel)
            .Select(value => value.Item)
            .ToHashSet(ReferenceEqualityComparer.Instance));
        var invocation = await Application.Current.Dispatcher.InvokeAsync(() =>
        {
            var method = preparation.MainModel.GetType()
                .GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                .SingleOrDefault(value =>
                    value.Name == methodName
                    && value.GetParameters() is
                    [
                        { ParameterType: var frameType },
                        { ParameterType: var layerType },
                        { ParameterType: var argumentType },
                    ]
                    && frameType == typeof(int)
                    && layerType == typeof(int)
                    && argumentType.IsInstanceOfType(argument))
                ?? throw new BridgeUnavailableException($"YMM4 {methodName} is unavailable");
            try
            {
                return method.Invoke(preparation.MainModel, [frame, layer, argument]);
            }
            catch (TargetInvocationException error)
            {
                throw error.InnerException ?? error;
            }
        });
        if (invocation is Task task)
        {
            await task.ConfigureAwait(false);
        }
        RawItem[] created = [];
        for (var attempt = 0; attempt < 30; attempt++)
        {
            created = await Application.Current.Dispatcher.InvokeAsync(() => ReadItems(
                    preparation.TimelineViewModel)
                .Where(value => !before.Contains(value.Item))
                .ToArray());
            if (created.Length > 0)
            {
                return created;
            }
            await Task.Delay(100).ConfigureAwait(false);
        }
        throw new BridgeUnavailableException($"YMM4 {methodName} produced no readable item");
    }

    private static object ResolveCharacterDescriptor(string descriptorId, DescriptorCatalogDto catalog)
    {
        var descriptor = RequireTargetDescriptor(descriptorId, "character", catalog, mutationAllowed: false);
        var timeline = GetMember(RequireMainViewModel(), "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        return ResolveCharacter(timeline, descriptor.Name);
    }

    private static string ResolveCharacterDescriptorId(object? character, DescriptorCatalogDto catalog)
    {
        if (character is null)
        {
            throw new BridgeUnavailableException("YMM4 character binding is missing during read-back");
        }
        var name = GetString(character, "Name");
        var matches = catalog.Descriptors.Where(value =>
                value.Kind == "character"
                && value.Bindable
                && string.Equals(value.Name, name, StringComparison.Ordinal))
            .ToArray();
        return matches.Length == 1
            ? matches[0].DescriptorId
            : throw new BridgeConflictException(
                $"YMM4 character binding is missing or ambiguous during read-back: {name}",
                SnapshotCore().Fingerprint);
    }

    private static object ResolveTemplateDescriptor(string descriptorId, DescriptorCatalogDto catalog)
    {
        var descriptor = RequireTargetDescriptor(descriptorId, "template", catalog, mutationAllowed: false);
        var main = RequireMainViewModel();
        var timeline = GetMember(main, "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var templates = new HashSet<object>(ReferenceEqualityComparer.Instance);
        var visited = new HashSet<object>(ReferenceEqualityComparer.Instance);
        foreach (var propertyName in TemplateMenuPropertyNames)
        {
            CollectTemplates(GetMember(timeline, propertyName), templates, visited);
        }
        CollectTemplates(
            GetMember(main, "AddTemplateContextMenuViewModel"),
            templates,
            visited);
        var matches = templates.Where(value =>
        {
            var actual = DescribeTemplate(value);
            return string.Equals(
                    actual.DescriptorId,
                    descriptor.DescriptorId,
                    StringComparison.Ordinal)
                && string.Equals(
                    actual.ConfigDigest,
                    descriptor.ConfigDigest,
                    StringComparison.Ordinal)
                && string.Equals(
                    actual.SchemaDigest,
                    descriptor.SchemaDigest,
                    StringComparison.Ordinal);
        }).ToArray();
        return matches.Length == 1
            ? matches[0]
            : throw new BridgeConflictException(
                $"Native YMM4 template descriptor is missing or ambiguous: {descriptorId}",
                SnapshotCore().Fingerprint);
    }

    private static TargetDescriptorDto RequireTargetDescriptor(
        string descriptorId,
        string kind,
        DescriptorCatalogDto catalog,
        bool mutationAllowed)
    {
        var matches = catalog.Descriptors.Where(value =>
                value.DescriptorId == descriptorId
                && value.Kind == kind
                && value.Bindable
                && (!mutationAllowed || value.MutationAllowed))
            .ToArray();
        return matches.Length == 1
            ? matches[0]
            : throw new BridgeConflictException(
                $"Native descriptor is unavailable or ambiguous: {descriptorId}",
                SnapshotCore().Fingerprint);
    }

    private static string ResolveArtifactPath(
        ParsedNativeExtensionIntent intent,
        IReadOnlyDictionary<string, NativeExtensionArtifactDto> artifacts)
    {
        var digest = RequireSha256(intent.ArtifactDigest ?? string.Empty, "asset artifactDigest");
        if (!artifacts.TryGetValue(digest, out var artifact)
            || artifact.Kind != intent.Kind
            || artifact.MediaType != intent.MediaType
            || artifact.ByteLength != intent.ArtifactByteLength)
        {
            throw new BridgeValidationException(
                $"Native-extension artifact is not bound to intent {intent.LogicalKey}");
        }
        return Path.GetFullPath(artifact.Path);
    }

    private static void SetOptionalLoop(object item, bool loopPlayback)
    {
        var property = new[] { "IsLooped", "IsLoop", "LoopPlayback", "Loop" }
            .Select(name => item.GetType().GetProperty(
                name,
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance))
            .FirstOrDefault(value => value?.CanWrite == true);
        if (property is null)
        {
            if (loopPlayback)
            {
                throw new BridgeUnavailableException(
                    $"YMM4 {item.GetType().Name} does not expose loop playback");
            }
            return;
        }
        property.SetValue(item, ConvertValue(loopPlayback, property.PropertyType));
    }

    private static void ApplyManagedEffect(
        ParsedNativeExtensionOperation operation,
        DescriptorCatalogDto catalog)
    {
        var timeline = GetMember(RequireMainViewModel(), "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var rawItems = ReadItems(timeline);
        var parent = FindNativeExtensionParent(operation.Intent.TargetEntityId!, rawItems)
            ?? throw new BridgeConflictException(
                $"Managed effect target no longer exists: {operation.Intent.TargetEntityId}",
                SnapshotCore().Fingerprint);
        NativeExtensionRemarkCodec.TryDecode(parent.Remark, out var parentMarker);
        parentMarker ??= NativeExtensionRemarkCodec.Create(
            SnapshotCore().ProjectId,
            operation.Intent.TargetEntityId!,
            operation.Intent.EntityRevision,
            $"host:{operation.Intent.TargetEntityId}",
            operation.RealizationId,
            "managed_effect_host",
            0,
            1);
        var effectMarkers = new SortedDictionary<string, NativeExtensionEffectMarker>(
            parentMarker.Effects.ToDictionary(pair => pair.Key, pair => pair.Value, StringComparer.Ordinal),
            StringComparer.Ordinal);
        var descriptor = catalog.Descriptors.SingleOrDefault(value =>
            value.DescriptorId == operation.Intent.DescriptorId
            && value.Kind is "video-effect" or "audio-effect"
            && value.Bindable
            && value.MutationAllowed)
            ?? throw new BridgeConflictException(
                $"Typed effect descriptor is unavailable: {operation.Intent.DescriptorId}",
                SnapshotCore().Fingerprint);
        var stableType = descriptor.Metadata.GetValueOrDefault("type")
            ?? throw new BridgeUnavailableException("Typed effect descriptor has no stable type");
        var collectionName = descriptor.Kind == "audio-effect" ? "AudioEffects" : "VideoEffects";
        TryReadManagedEffect(parent, operation.Intent.EffectInstanceId!, out var existing);

        if (operation.Intent.EffectAction == "remove")
        {
            if (existing is null)
            {
                throw new BridgeConflictException(
                    $"Managed effect no longer exists: {operation.Intent.EffectInstanceId}",
                    SnapshotCore().Fingerprint);
            }
            RemoveEffect(parent.Item, existing.Marker.Collection, existing.Effect);
            effectMarkers.Remove(operation.Intent.EffectInstanceId!);
        }
        else
        {
            if (operation.Intent.EffectParameters is { } parameters
                && parameters.EnumerateObject().Any())
            {
                throw new BridgeValidationException(
                    "The current typed effect allowlist is parameterless");
            }
            object effect;
            if (existing is not null)
            {
                effect = existing.Effect;
            }
            else
            {
                var effectType = FindLoadedType(stableType)
                    ?? throw new BridgeUnavailableException(
                        $"Typed effect implementation is unavailable: {stableType}");
                effect = Activator.CreateInstance(effectType)
                    ?? throw new BridgeUnavailableException(
                        $"Typed effect could not be created: {stableType}");
                AddEffect(parent.Item, collectionName, effect);
            }
            var collection = ReadEffectCollections(parent.Item)
                .Single(value => value.Name == collectionName);
            var index = collection.Items.Select((value, index) => (value, index))
                .Single(value => ReferenceEquals(value.value, effect)).index;
            effectMarkers[operation.Intent.EffectInstanceId!] = new NativeExtensionEffectMarker(
                operation.Intent.EffectInstanceId!,
                operation.RealizationId,
                operation.Intent.DescriptorId!,
                stableType,
                collectionName,
                index);
        }
        var updatedMarker = parentMarker with { Effects = effectMarkers };
        SetRequired(
            parent.Item,
            NativeExtensionRemarkCodec.Append(parent.Remark, updatedMarker),
            "Remark");
    }

    private static void AddEffect(object parent, string collectionName, object effect)
    {
        var collection = ReadEffectCollections(parent)
            .SingleOrDefault(value => value.Name == collectionName)
            ?? throw new BridgeUnavailableException(
                $"YMM4 item has no {collectionName} collection");
        var current = collection.Property.GetValue(parent)
            ?? throw new BridgeUnavailableException($"YMM4 {collectionName} is null");
        var add = current.GetType().GetMethods(BindingFlags.Public | BindingFlags.Instance)
            .SingleOrDefault(value =>
                value.Name == "Add"
                && value.GetParameters() is [{ ParameterType: var parameter }]
                && parameter.IsInstanceOfType(effect))
            ?? throw new BridgeUnavailableException($"YMM4 {collectionName}.Add is unavailable");
        var updated = add.Invoke(current, [effect])
            ?? throw new BridgeUnavailableException($"YMM4 {collectionName}.Add returned null");
        collection.Property.SetValue(parent, updated);
    }

    private static void RemoveEffect(object parent, string collectionName, object effect)
    {
        var collection = ReadEffectCollections(parent)
            .SingleOrDefault(value => value.Name == collectionName)
            ?? throw new BridgeUnavailableException(
                $"YMM4 item has no {collectionName} collection");
        var matchingIndexes = collection.Items.Select((value, index) => (value, index))
            .Where(value => ReferenceEquals(value.value, effect))
            .Select(value => value.index)
            .ToArray();
        var index = matchingIndexes.Length == 1 ? matchingIndexes[0] : -1;
        if (index < 0 || index >= collection.Items.Count)
        {
            throw new BridgeConflictException("Managed effect collection changed", SnapshotCore().Fingerprint);
        }
        var current = collection.Property.GetValue(parent)
            ?? throw new BridgeUnavailableException($"YMM4 {collectionName} is null");
        var removeAt = current.GetType().GetMethod("RemoveAt", [typeof(int)])
            ?? throw new BridgeUnavailableException($"YMM4 {collectionName}.RemoveAt is unavailable");
        var updated = removeAt.Invoke(current, [index])
            ?? throw new BridgeUnavailableException($"YMM4 {collectionName}.RemoveAt returned null");
        collection.Property.SetValue(parent, updated);
    }

    private static IReadOnlyList<NativeExtensionRealizationDto> ReadNativeExtensionRealizations(
        IReadOnlyList<ParsedNativeExtensionOperation> operations,
        DescriptorCatalogDto catalog)
    {
        var timeline = GetMember(RequireMainViewModel(), "ActiveTimelineViewModel")
            ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
        var rawItems = ReadItems(timeline);
        var groups = ReadNativeExtensionGroups(rawItems);
        var result = new List<NativeExtensionRealizationDto>();
        foreach (var operation in operations.Where(value => value.Action != "delete"))
        {
            if (operation.Intent.Kind == "managed_effect")
            {
                var parent = FindNativeExtensionParent(operation.Intent.TargetEntityId!, rawItems);
                if (parent is null
                    || !TryReadManagedEffect(parent, operation.Intent.EffectInstanceId!, out var effect)
                    || effect is null)
                {
                    continue;
                }
                var ownedFields = ReadOwnedNativeExtensionFields(
                    operation,
                    parent,
                    effect,
                    catalog);
                result.Add(new NativeExtensionRealizationDto(
                    operation.Intent.LogicalKey,
                    effect.Marker.RealizationId,
                    "managed_effect",
                    parentMarkerProjectId(parent),
                    operation.Intent.EntityId,
                    operation.Intent.EntityRevision,
                    parent.Frame,
                    parent.Layer,
                    parent.Length,
                    CanonicalJson.Sha256(
                        "takegraph-ymm4-native-extension-owned-state-v1",
                        ownedFields),
                    ownedFields,
                    ReadPreservedFields([parent], excludeEffects: true),
                    NativeExtensionStateDigest([effect.Effect]),
                    ReadUnknownEffects(parent)));
                continue;
            }
            if (!groups.TryGetValue(operation.Intent.LogicalKey, out var group))
            {
                continue;
            }
            var first = group.Items[0];
            var length = group.Kind == "template"
                ? group.Items.Max(value => value.Frame + Math.Max(value.Length, 0)) - first.Frame
                : first.Length;
            var groupOwnedFields = ReadOwnedNativeExtensionFields(operation, group, catalog);
            result.Add(new NativeExtensionRealizationDto(
                operation.Intent.LogicalKey,
                group.RealizationId,
                group.Kind,
                group.ProjectId,
                group.EntityId,
                group.Revision,
                first.Frame,
                first.Layer,
                length,
                CanonicalJson.Sha256(
                    "takegraph-ymm4-native-extension-owned-state-v1",
                    groupOwnedFields),
                groupOwnedFields,
                ReadPreservedFields(group.Items, excludeEffects: false),
                NativeExtensionStateDigest(group.Items.Select(value => value.Item)),
                ReadGroupUnknownEffects(group.Items)));
        }
        return result.OrderBy(value => value.LogicalKey, StringComparer.Ordinal).ToArray();
    }

    private static string parentMarkerProjectId(RawItem parent)
    {
        if (NativeExtensionRemarkCodec.TryDecode(parent.Remark, out var extension))
        {
            return extension!.ProjectId;
        }
        if (RemarkCodec.TryDecode(parent.Remark, out var voice))
        {
            return voice!.ProjectId;
        }
        throw new BridgeConflictException(
            "Managed effect host has no project-scoped ownership marker",
            SnapshotCore().Fingerprint);
    }

    private static IReadOnlyDictionary<string, string> ReadOwnedNativeExtensionFields(
        ParsedNativeExtensionOperation operation,
        NativeExtensionGroup group,
        DescriptorCatalogDto catalog)
    {
        var first = group.Items[0].Item;
        var values = BaseOwnedNativeExtensionFields(operation);
        var actualLength = group.Kind == "template"
            ? group.Items.Max(value => value.Frame + Math.Max(value.Length, 0)) - group.Items[0].Frame
            : group.Items[0].Length;
        values["projectId"] = group.ProjectId;
        values["entityId"] = group.EntityId;
        values["entityRevision"] = group.Revision.ToString(
            System.Globalization.CultureInfo.InvariantCulture);
        values["kind"] = group.Kind;
        values["frame"] = group.Items[0].Frame.ToString(
            System.Globalization.CultureInfo.InvariantCulture);
        values["layer"] = group.Items[0].Layer.ToString(
            System.Globalization.CultureInfo.InvariantCulture);
        values["length"] = actualLength.ToString(
            System.Globalization.CultureInfo.InvariantCulture);
        switch (operation.Intent.Kind)
        {
            case "portrait":
            case "face":
                values["descriptorId"] = ResolveCharacterDescriptorId(
                    GetMember(first, "Character"), catalog);
                break;
            case "image":
            case "video":
            case "audio":
            case "bgm":
                values["artifactDigest"] = HashFileOwnedArtifact(GetString(first, "FilePath"));
                values["mediaType"] = operation.Intent.MediaType ?? string.Empty;
                values["byteLength"] = FileLengthOwnedArtifact(GetString(first, "FilePath"));
                values["loopPlayback"] = ReadOptionalLoop(first).ToString().ToLowerInvariant();
                break;
            case "template":
                values["descriptorId"] = group.DescriptorId ?? string.Empty;
                values["partCount"] = group.Items.Count.ToString(
                    System.Globalization.CultureInfo.InvariantCulture);
                values["footprintDigest"] = NativeExtensionStateDigest(
                    group.Items.Select(value => value.Item));
                break;
        }
        return values;
    }

    private static IReadOnlyDictionary<string, string> ReadOwnedNativeExtensionFields(
        ParsedNativeExtensionOperation operation,
        RawItem parent,
        ManagedEffectView effect,
        DescriptorCatalogDto catalog)
    {
        var values = BaseOwnedNativeExtensionFields(operation);
        var ownership = ReadParentOwnership(parent);
        values["projectId"] = ownership.ProjectId;
        values["entityId"] = ownership.EntityId;
        values["entityRevision"] = ownership.Revision.ToString(
            System.Globalization.CultureInfo.InvariantCulture);
        values["frame"] = parent.Frame.ToString(System.Globalization.CultureInfo.InvariantCulture);
        values["layer"] = parent.Layer.ToString(System.Globalization.CultureInfo.InvariantCulture);
        values["length"] = parent.Length.ToString(System.Globalization.CultureInfo.InvariantCulture);
        values["descriptorId"] = effect.Marker.DescriptorId;
        values["stableTypeId"] = effect.Effect.GetType().FullName ?? effect.Effect.GetType().Name;
        values["collection"] = effect.Marker.Collection;
        values["parametersDigest"] = CanonicalJson.Sha256(
            "takegraph-ymm4-native-extension-effect-parameters-v1",
            operation.Intent.EffectParameters ?? JsonDocument.Parse("{}").RootElement);
        _ = RequireTargetDescriptor(
            effect.Marker.DescriptorId,
            effect.Marker.Collection == "AudioEffects" ? "audio-effect" : "video-effect",
            catalog,
            mutationAllowed: true);
        values["hostStateDigest"] = ComputeExtensionPreservedStateDigest(
            parent.Item,
            excludeEffects: true);
        return values;
    }

    private static (string ProjectId, string EntityId, ulong Revision) ReadParentOwnership(
        RawItem parent)
    {
        if (NativeExtensionRemarkCodec.TryDecode(parent.Remark, out var extension))
        {
            return (extension!.ProjectId, extension.EntityId, extension.Revision);
        }
        if (RemarkCodec.TryDecode(parent.Remark, out var voice))
        {
            return (voice!.ProjectId, voice.EntityId, voice.Revision);
        }
        throw new BridgeConflictException(
            "Managed effect host has no project-scoped ownership marker",
            SnapshotCore().Fingerprint);
    }

    private static SortedDictionary<string, string> BaseOwnedNativeExtensionFields(
        ParsedNativeExtensionOperation operation)
    {
        return new SortedDictionary<string, string>(StringComparer.Ordinal)
        {
            ["logicalKey"] = operation.Intent.LogicalKey,
            ["projectId"] = SnapshotCore().ProjectId,
            ["entityId"] = operation.Intent.EntityId,
            ["entityRevision"] = operation.Intent.EntityRevision.ToString(
                System.Globalization.CultureInfo.InvariantCulture),
            ["kind"] = operation.Intent.Kind,
            ["frame"] = operation.Intent.Frame.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["layer"] = operation.Intent.Layer.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["length"] = operation.Intent.Length.ToString(System.Globalization.CultureInfo.InvariantCulture),
        };
    }

    private static string HashFileOwnedArtifact(string path)
    {
        if (string.IsNullOrWhiteSpace(path) || !File.Exists(path))
        {
            throw new BridgeConflictException(
                "Native-extension artifact is missing during semantic read-back",
                SnapshotCore().Fingerprint);
        }
        using var stream = File.OpenRead(path);
        return $"sha256:{Convert.ToHexStringLower(SHA256.HashData(stream))}";
    }

    private static string FileLengthOwnedArtifact(string path)
    {
        return new FileInfo(path).Length.ToString(System.Globalization.CultureInfo.InvariantCulture);
    }

    private static bool ReadOptionalLoop(object item)
    {
        foreach (var name in new[] { "IsLooped", "IsLoop", "LoopPlayback", "Loop" })
        {
            var property = item.GetType().GetProperty(
                name,
                BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance);
            if (property?.CanRead == true && property.GetValue(item) is bool value)
            {
                return value;
            }
        }
        return false;
    }

    private static bool NativeExtensionRealizationsMatch(
        IReadOnlyList<ParsedNativeExtensionOperation> operations,
        IReadOnlyList<NativeExtensionRealizationDto> realizations,
        string expectedProjectId,
        DescriptorCatalogDto catalog)
    {
        if (realizations.Count != operations.Count(value => value.Action != "delete"))
        {
            return false;
        }
        return operations.All(operation =>
        {
            var matches = realizations.Where(value => value.LogicalKey == operation.Intent.LogicalKey)
                .ToArray();
            if (operation.Action == "delete")
            {
                return matches.Length == 0;
            }
            if (matches.Length != 1)
            {
                return false;
            }
            var actual = matches[0];
            if (actual.RealizationId != operation.RealizationId
                || actual.Kind != operation.Intent.Kind
                || !IsSha256(actual.StateDigest)
                || !IsSha256(actual.OwnedStateDigest)
                || !string.Equals(
                    actual.OwnedStateDigest,
                    CanonicalJson.Sha256(
                        "takegraph-ymm4-native-extension-owned-state-v1",
                        actual.OwnedFields),
                    StringComparison.Ordinal)
                || !NativeExtensionOwnedFieldsMatch(
                    operation,
                    actual,
                    expectedProjectId,
                    catalog)
                || !SequenceEqual(
                    actual.UnknownEffects.OrderBy(value => value.StableTypeId, StringComparer.Ordinal)
                        .ThenBy(value => value.InstanceKey, StringComparer.Ordinal).ToArray(),
                    operation.Preservation.UnknownEffects))
            {
                return false;
            }
            return operation.Intent.Kind == "managed_effect"
                || actual.Frame == operation.Intent.Frame
                && actual.Layer == operation.Intent.Layer
                && (operation.Intent.Kind == "template" || actual.Length == operation.Intent.Length);
        });
    }

    private static bool NativeExtensionOwnedFieldsMatch(
        ParsedNativeExtensionOperation operation,
        NativeExtensionRealizationDto actual,
        string expectedProjectId,
        DescriptorCatalogDto catalog)
    {
        var expected = BaseOwnedNativeExtensionFields(operation);
        expected["projectId"] = expectedProjectId;
        if (actual.ProjectId != expectedProjectId
            || actual.EntityId != operation.Intent.EntityId
            || actual.EntityRevision != operation.Intent.EntityRevision
            || actual.OwnedFields.GetValueOrDefault("projectId") != expectedProjectId
            || actual.OwnedFields.GetValueOrDefault("entityId") != operation.Intent.EntityId
            || actual.OwnedFields.GetValueOrDefault("entityRevision")
                != operation.Intent.EntityRevision.ToString(
                    System.Globalization.CultureInfo.InvariantCulture))
        {
            return false;
        }
        switch (operation.Intent.Kind)
        {
            case "portrait":
            case "face":
                expected["descriptorId"] = operation.Intent.DescriptorId ?? string.Empty;
                break;
            case "image":
            case "video":
            case "audio":
            case "bgm":
                expected["artifactDigest"] = RequireSha256(
                    operation.Intent.ArtifactDigest ?? string.Empty,
                    "artifactDigest");
                expected["mediaType"] = operation.Intent.MediaType ?? string.Empty;
                expected["byteLength"] = operation.Intent.ArtifactByteLength?.ToString(
                    System.Globalization.CultureInfo.InvariantCulture) ?? string.Empty;
                expected["loopPlayback"] = operation.Intent.LoopPlayback.ToString().ToLowerInvariant();
                break;
            case "managed_effect":
            {
                expected["frame"] = actual.Frame.ToString(
                    System.Globalization.CultureInfo.InvariantCulture);
                expected["layer"] = actual.Layer.ToString(
                    System.Globalization.CultureInfo.InvariantCulture);
                expected["length"] = actual.Length.ToString(
                    System.Globalization.CultureInfo.InvariantCulture);
                var descriptor = catalog.Descriptors.SingleOrDefault(value =>
                    value.DescriptorId == operation.Intent.DescriptorId
                    && value.Kind is "video-effect" or "audio-effect"
                    && value.Bindable
                    && value.MutationAllowed);
                if (descriptor is null)
                {
                    return false;
                }
                expected["descriptorId"] = operation.Intent.DescriptorId ?? string.Empty;
                expected["stableTypeId"] = descriptor.Metadata.GetValueOrDefault("type")
                    ?? string.Empty;
                expected["collection"] = descriptor.Kind == "audio-effect"
                    ? "AudioEffects"
                    : "VideoEffects";
                expected["parametersDigest"] = CanonicalJson.Sha256(
                    "takegraph-ymm4-native-extension-effect-parameters-v1",
                    operation.Intent.EffectParameters ?? JsonDocument.Parse("{}").RootElement);
                expected["hostStateDigest"] = operation.Preservation.PreservedFields
                    .SingleOrDefault(value => value.Field == "part:0:targetLocalState")?.StateDigest
                    ?? string.Empty;
                break;
            }
            case "template":
                expected["length"] = actual.Length.ToString(
                    System.Globalization.CultureInfo.InvariantCulture);
                expected["descriptorId"] = operation.Intent.DescriptorId ?? string.Empty;
                var partCount = actual.OwnedFields.GetValueOrDefault("partCount");
                var footprint = actual.OwnedFields.GetValueOrDefault("footprintDigest");
                if (!int.TryParse(
                        partCount,
                        System.Globalization.NumberStyles.None,
                        System.Globalization.CultureInfo.InvariantCulture,
                        out var parsedPartCount)
                    || parsedPartCount <= 0
                    || footprint is null
                    || !IsSha256(footprint))
                {
                    return false;
                }
                expected["partCount"] = partCount;
                expected["footprintDigest"] = footprint;
                break;
            default:
                return false;
        }
        return expected.Count == actual.OwnedFields.Count
            && expected.All(pair => actual.OwnedFields.TryGetValue(pair.Key, out var value)
                && string.Equals(value, pair.Value, StringComparison.Ordinal));
    }

    private static bool NativeExtensionPreservationMatches(
        IReadOnlyList<ParsedNativeExtensionOperation> operations)
    {
        var observation = ObserveNativeExtensions(
            operations.Select(value => value.Intent).ToArray(),
            plannedRealizations: operations.ToDictionary(
                value => value.Intent.LogicalKey,
                value => value.RealizationId,
                StringComparer.Ordinal));
        foreach (var operation in operations.Where(value => value.Action == "update"))
        {
            if (!observation.Existing.TryGetValue(operation.Intent.LogicalKey, out var existing)
                || !SequenceEqual(existing.PreservedFields, operation.Preservation.PreservedFields)
                || !SequenceEqual(existing.UnknownEffects, operation.Preservation.UnknownEffects))
            {
                return false;
            }
        }
        foreach (var operation in operations.Where(value => value.Action == "delete"))
        {
            if (observation.Existing.ContainsKey(operation.Intent.LogicalKey))
            {
                return false;
            }
            if (operation.Intent.Kind == "managed_effect")
            {
                var timeline = GetMember(RequireMainViewModel(), "ActiveTimelineViewModel");
                if (timeline is null)
                {
                    return false;
                }
                var parent = FindNativeExtensionParent(
                    operation.Intent.TargetEntityId!,
                    ReadItems(timeline));
                if (parent is null
                    || !SequenceEqual(
                        ReadPreservedFields([parent], excludeEffects: true),
                        operation.Preservation.PreservedFields)
                    || !SequenceEqual(
                        ReadUnknownEffects(parent),
                        operation.Preservation.UnknownEffects))
                {
                    return false;
                }
            }
        }
        return true;
    }

    private static bool IsSha256(string value)
    {
        var hex = value.StartsWith("sha256:", StringComparison.Ordinal)
            ? value["sha256:".Length..]
            : value;
        return hex.Length == 64 && hex.All(Uri.IsHexDigit);
    }

    private static Guid DeterministicNativeExtensionRealization(
        Guid operationId,
        string logicalKey)
    {
        using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        hasher.AppendData("takegraph-native-extension-realization-v1\0"u8);
        Span<byte> operationBytes = stackalloc byte[16];
        operationId.TryWriteBytes(operationBytes, bigEndian: true, out _);
        hasher.AppendData(operationBytes);
        hasher.AppendData(Encoding.UTF8.GetBytes(logicalKey));
        var bytes = hasher.GetHashAndReset()[..16];
        bytes[6] = (byte)((bytes[6] & 0x0f) | 0x50);
        bytes[8] = (byte)((bytes[8] & 0x3f) | 0x80);
        return new Guid(bytes, bigEndian: true);
    }

    private async Task<NativeExtensionApplyResponseDto> RollBackNativeExtensionAsync(
        NativeExtensionApplyRequestDto request,
        NativeExtensionApplyResponseDto applying,
        ProjectSnapshotDto before,
        string failure)
    {
        string? rollbackError = null;
        try
        {
            BridgeFaultInjection.ThrowIf("during_rollback");
            if (!recoveryStore.TryGet(request.OperationId, out var entry) || entry is null)
            {
                throw new BridgeUnavailableException("Native-extension recovery preimage is missing");
            }
            var restoredItems = entry.BeforeItems.Select(DeserializeYmmObject).ToArray();
            await Application.Current.Dispatcher.InvokeAsync(() =>
            {
                var main = RequireMainViewModel();
                var timeline = GetMember(main, "ActiveTimelineViewModel")
                    ?? throw new BridgeUnavailableException("No active YMM4 timeline is open");
                var model = GetField(timeline, "timeline")
                    ?? GetMember(timeline, "Timeline")
                    ?? throw new BridgeUnavailableException("YMM4 timeline mutation API is unavailable");
                var touched = FindRecoveryRawItems(entry, ReadItems(timeline));
                ReplaceBatchItems(model, touched, restoredItems);
            });
        }
        catch (Exception error)
        {
            rollbackError = error.GetBaseException().Message;
        }
        var current = TrySnapshot() ?? before;
        var restored = rollbackError is null
            && string.Equals(current.Fingerprint, before.Fingerprint, StringComparison.Ordinal);
        var receipt = applying with
        {
            Status = restored ? "rolled_back" : "recovery_required",
            AfterFingerprint = current.Fingerprint,
            Realizations = [],
            Verified = false,
            Error = rollbackError is null
                ? failure
                : $"{failure}; rollback failed: {rollbackError}",
        };
        projectOperationStore.PutNativeExtension(receipt);
        recoveryStore.TryTransition(
            request.OperationId,
            receipt.Status,
            current.Fingerprint,
            receipt.Error);
        return receipt;
    }

    private sealed record ParsedNativeExtensionIntent(
        string LogicalKey,
        string Kind,
        string EntityId,
        ulong EntityRevision,
        int Frame,
        int Layer,
        int Length,
        string? DescriptorId,
        string? DescriptorExpectedDigest,
        string? ArtifactDigest,
        string? MediaType,
        ulong? ArtifactByteLength,
        bool LoopPlayback,
        string? TargetEntityId,
        string? EffectInstanceId,
        string? EffectAction,
        JsonElement? EffectParameters);

    private sealed record ParsedNativeExtensionPreservation(
        string Mode,
        IReadOnlyList<NativeExtensionPreservedFieldDto> PreservedFields,
        IReadOnlyList<NativeExtensionOpaqueEffectDto> UnknownEffects);

    private sealed record ParsedNativeExtensionOperation(
        Guid RealizationId,
        string Action,
        ParsedNativeExtensionIntent Intent,
        ParsedNativeExtensionPreservation Preservation);

    private sealed record NativeExtensionGroup(
        string LogicalKey,
        Guid RealizationId,
        string Kind,
        string ProjectId,
        string EntityId,
        ulong Revision,
        string? DescriptorId,
        IReadOnlyList<RawItem> Items);

    private sealed record EffectCollectionView(
        string Name,
        PropertyInfo Property,
        IReadOnlyList<object> Items);

    private sealed record ManagedEffectView(
        NativeExtensionMarker ParentMarker,
        NativeExtensionEffectMarker Marker,
        object Effect);

    private sealed class NativeExtensionPreparation(
        object mainModel,
        object timelineViewModel,
        object timeline,
        object[] beforeItems,
        object[] beforeItemIdentities,
        Dictionary<object, string> unmanagedWitness,
        NativeExtensionObservationDto observation)
    {
        internal object MainModel { get; } = mainModel;
        internal object TimelineViewModel { get; } = timelineViewModel;
        internal object Timeline { get; } = timeline;
        internal object[] BeforeItems { get; } = beforeItems;
        internal object[] BeforeItemIdentities { get; } = beforeItemIdentities;
        internal Dictionary<object, string> UnmanagedWitness { get; } = unmanagedWitness;
        internal NativeExtensionObservationDto Observation { get; } = observation;
        internal List<object> AddedItems { get; } = [];
    }
}
