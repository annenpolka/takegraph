using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TakeGraph.Ymm4Bridge;

var tests = new (string Name, Action Run)[]
{
    ("cross-runtime request digests", ApplyRequestDigest.ValidateCrossRuntimeGolden),
    ("cross-runtime structured capability digest", StructuredCapabilityDigestGolden),
    ("remark identity round-trip", RemarkCodec.ValidateRoundTrip),
    ("portable marker round-trip", PortableMarkerRoundTrip),
    ("native-extension marker round-trip", NativeExtensionMarkerRoundTrip),
    ("marker ownership fields are required", MarkerOwnershipFieldsRequired),
    ("receipt journal persistence", ReceiptJournalPersistence),
    ("protocol-1 receipt migration preserves evidence", LegacyReceiptMigrationPreservesEvidence),
    ("corrupt receipt journal fails closed", CorruptReceiptJournalFailsClosed),
    ("recovery journal persistence and transition", RecoveryJournalPersistence),
    ("applied-unverified recovery is historical", AppliedUnverifiedRecoveryIsHistorical),
    ("corrupt recovery journal fails closed", CorruptRecoveryJournalFailsClosed),
    ("recovery journal invariants fail closed", RecoveryJournalInvariantsFailClosed),
    ("store persistence precedes memory", StorePersistencePrecedesMemory),
    ("foreign unresolved recovery blocks authorization", ForeignRecoveryBlocksAuthorization),
    ("project saved signals fail closed", ProjectSavedSignalsFailClosed),
    ("project operation store persistence", ProjectOperationStorePersistence),
    ("project initialization store is request-bound", ProjectInitializationStoreIsRequestBound),
    ("project initialization path claim never overwrites", ProjectInitializationPathClaimNeverOverwrites),
    ("project initialization path claim is evidence-bound", ProjectInitializationPathClaimIsEvidenceBound),
    ("project initialization recovery evidence is monotonic", ProjectInitializationRecoveryEvidenceIsMonotonic),
    ("project initialization verified replay is historical", ProjectInitializationVerifiedReplayIsHistorical),
    ("corrupt project operation store fails closed", CorruptProjectOperationStoreFailsClosed),
    ("MP4 media probe", Mp4MediaProbeReadsAuthoritativeFields),
    ("MP4 codec evidence fails closed", Mp4CodecEvidenceFailsClosed),
    ("external MP4 evidence when requested", ExternalMp4EvidenceWhenRequested),
    ("queued render cancellation is registered", QueuedRenderCancellationIsRegistered),
    ("render source lease blocks drift", RenderSourceLeaseBlocksDrift),
    ("render binding lease blocks setting and binary drift", RenderBindingLeaseBlocksDrift),
    ("authoritative render requires exhaustive source binding", AuthoritativeRenderRequiresExhaustiveSourceBinding),
    ("render overwrite WAL restores original", RenderOverwriteWalRestoresOriginal),
    ("fault injection is explicit and scoped", FaultInjectionIsScoped),
    ("preservation serialization is canonical and fail closed", PreservationSerializationFailsClosed),
    ("template traversal and identity are stable", TemplateTraversalAndIdentityAreStable),
    ("required reflected timeline fields fail closed", RequiredReflectionFailsClosed),
    ("locked mutation targets map to conflict", LockedMutationTargetsMapToConflict),
    ("request JSON boundary is strict and maps to 400", StrictRequestJsonMapsTo400),
    ("submitted HTTP responses are not rewritten", SubmittedHttpResponsesAreNotRewritten),
    ("preview settle accepts the requested frame", PreviewSettleAcceptsRequestedFrame),
    ("native-extension inner JSON is exact", NativeExtensionInnerJsonIsExact),
    ("scene capture combines capture and restoration failures", SceneCaptureFailureIsCombined),
    ("portable descriptor digests bind exact configuration", PortableDescriptorDigestsBindConfiguration),
    ("native-extension item witness rejects collateral additions", NativeExtensionItemWitnessRejectsCollateralAdditions),
    ("snapshot native-extension DTO is managed-only", SnapshotNativeExtensionDtoIsManagedOnly),
    ("current scene composition wire contract is deterministic", CurrentSceneCompositionWireContractIsDeterministic),
    ("current scene composition never estimates the playhead", CurrentSceneCompositionNeverEstimatesPlayhead),
    ("metadata detach WAL is durable and request-bound", MetadataDetachWalIsDurableAndBound),
    ("metadata detach not-started tombstone is durable", MetadataDetachNotStartedIsDurable),
    ("metadata detach removes only the selected Remark identity", MetadataDetachRemovesSelectedRemarkIdentity),
    ("unified target-plan cue is sealed", UnifiedTargetPlanCueIsSealed),
    ("timeline-edit historical binding is live-state independent", TimelineEditHistoricalBindingIsLiveStateIndependent),
    ("timeline-edit native-only payloads are accepted", TimelineEditNativeOnlyPayloadsAreAccepted),
    ("unified portable actions bind current pair state", UnifiedPortableActionsBindCurrentPairState),
    ("portable audio is leased from immutable CAS", PortableAudioIsLeasedFromImmutableCas),
};

var failures = new List<string>();
foreach (var (name, run) in tests)
{
    try
    {
        run();
        Console.WriteLine($"PASS {name}");
    }
    catch (Exception error)
    {
        failures.Add($"FAIL {name}: {error.GetBaseException().Message}");
    }
}
foreach (var failure in failures)
{
    Console.Error.WriteLine(failure);
}
return failures.Count == 0 ? 0 : 1;

static void PortableAudioIsLeasedFromImmutableCas()
{
    var root = Path.Combine(Path.GetTempPath(), $"takegraph-portable-cas-{Guid.NewGuid():N}");
    var source = Path.Combine(root, "source.wav");
    Directory.CreateDirectory(root);
    var bytes = Encoding.UTF8.GetBytes("approved portable audio bytes");
    File.WriteAllBytes(source, bytes);
    var hash = Convert.ToHexString(SHA256.HashData(bytes)).ToLowerInvariant();
    var utterance = new ManagedUtteranceDto(
        "utt-01",
        1,
        "speaker",
        "display",
        source,
        hash,
        10,
        20,
        1,
        2,
        "spoken");
    string casPath;
    try
    {
        using (var lease = Ymm4Facade.MaterializePortableArtifactsForTests([utterance], Path.Combine(root, "cas")))
        {
            casPath = lease.Utterances.Single().AudioPath;
            Assert(!string.Equals(casPath, source, StringComparison.OrdinalIgnoreCase),
                "portable apply must not retain the caller's mutable staging path");
            Assert(Path.GetFileName(casPath) == $"{hash}.wav",
                "portable CAS path must be named by the approved SHA-256");
            Assert(File.ReadAllBytes(casPath).SequenceEqual(bytes),
                "portable CAS bytes must equal the approved source bytes");
            ExpectWriteRejected(
                () => File.WriteAllText(casPath, "tampered"),
                "the leased CAS file must reject concurrent writes");
            ExpectWriteRejected(
                () => File.Move(casPath, $"{casPath}.moved"),
                "the leased CAS file must reject namespace replacement");
            var casDirectory = Path.GetDirectoryName(casPath)!;
            ExpectWriteRejected(
                () => Directory.Move(casDirectory, $"{casDirectory}-moved"),
                "the leased CAS ancestry must reject namespace replacement");
            lease.Verify();
        }

        File.Delete(source);
        using var replay = Ymm4Facade.MaterializePortableArtifactsForTests(
            [utterance],
            Path.Combine(root, "cas"));
        Assert(string.Equals(replay.Utterances.Single().AudioPath, casPath, StringComparison.OrdinalIgnoreCase),
            "pending replay must reuse CAS without the original staging file");
        replay.Verify();
        Assert(Ymm4Facade.ReadPortableArtifactHash(casPath) == hash,
            "portable read-back must derive the artifact identity from actual CAS bytes");
    }
    finally
    {
        if (Directory.Exists(root))
        {
            foreach (var file in Directory.EnumerateFiles(root, "*", SearchOption.AllDirectories))
            {
                File.SetAttributes(file, FileAttributes.Normal);
            }
            Directory.Delete(root, recursive: true);
        }
    }
}

static void SnapshotNativeExtensionDtoIsManagedOnly()
{
    var extension = new ManagedNativeExtensionDto(
        "portrait:portrait-01",
        Guid.Parse("11111111-1111-4111-8111-111111111111"),
        "portrait",
        "project-a",
        "portrait-01",
        3,
        new SortedDictionary<string, string>(StringComparer.Ordinal)
        {
            ["descriptorId"] = "character.marisa",
            ["frame"] = "120",
        });
    var snapshot = new ProjectSnapshotDto(
        "project-a",
        "test",
        "test.ymmp",
        "scene-a",
        60,
        "sha256:fingerprint",
        [],
        [extension],
        0);
    var json = JsonSerializer.Serialize(snapshot, BridgeJson.Options);
    using var document = JsonDocument.Parse(json);
    var observed = document.RootElement.GetProperty("nativeExtensions")[0];
    Assert(observed.GetProperty("ownedFields").GetProperty("descriptorId").GetString()
        == "character.marisa", "snapshot omitted an owned native-extension field");
    Assert(!observed.TryGetProperty("preservedFields", out _)
        && !observed.TryGetProperty("unknownEffects", out _)
        && !observed.TryGetProperty("stateDigest", out _),
        "snapshot exposed preservation or unknown-effect evidence as managed state");
}

static void CurrentSceneCompositionWireContractIsDeterministic()
{
    const string projectId = "project-a";
    var voiceId = Guid.Parse("11111111-2222-4333-8444-555555555555");
    var voiceRemark = RemarkCodec.Append(
        null,
        new NativeVoiceMarker("takegraph/v2", projectId, "voice-a", voiceId, 3));
    var portableText = MarkerCodec.Append(
        "visible caption",
        new ManagedMarker("caption-a", 2, Sha256("artifact"), "speaker", "audio.wav", 8));
    var items = new Ymm4Facade.RawItem[]
    {
        new(
            new object(),
            10,
            5,
            20,
            0,
            "spoken text",
            string.Empty,
            voiceRemark,
            "speaker",
            "spoken text",
            "YukkuriMovieMaker.Project.Items.VoiceItem",
            true,
            true),
        new(
            new object(),
            0,
            1,
            40,
            0,
            "private unmanaged label",
            string.Empty,
            string.Empty,
            string.Empty,
            string.Empty,
            "YukkuriMovieMaker.Project.Items.ImageItem",
            false,
            false),
        new(
            new object(),
            5,
            3,
            30,
            0,
            portableText,
            string.Empty,
            string.Empty,
            string.Empty,
            string.Empty,
            "YukkuriMovieMaker.Project.Items.TextItem",
            false,
            true),
        new(
            new object(),
            50,
            0,
            10,
            0,
            "not active",
            string.Empty,
            string.Empty,
            string.Empty,
            string.Empty,
            "YukkuriMovieMaker.Project.Items.TextItem",
            false,
            true),
    };
    var snapshot = Ymm4Facade.BuildSceneCompositionSnapshot(
        projectId,
        "scene-a",
        Sha256("source"),
        60,
        15,
        items);

    Assert(BridgeHost.CurrentCompositionPath == "/v2/scene/composition/current",
        "current composition route changed unexpectedly");
    Assert(snapshot.SchemaVersion == 1 && snapshot.Frame == 15 && snapshot.Fps == 60,
        "current frame identity was not preserved");
    Assert(snapshot.Elements.Count == 3
        && snapshot.Elements.Select(value => value.Layer).SequenceEqual([1, 3, 5]),
        "inactive items were not excluded or element ordering is unstable");
    Assert(snapshot.Elements.All(value => value.Active),
        "current-frame snapshot exposed a non-active element");
    Assert(snapshot.Elements.Select(value => value.ElementId).Distinct(StringComparer.Ordinal).Count()
        == snapshot.Elements.Count, "composition element IDs are not unique");
    Assert(snapshot.Elements.All(value => value.ElementId.All(character => character is >= ' ' and <= '~')),
        "composition element IDs must remain printable ASCII for cross-runtime ordering");
    var voice = snapshot.Elements.Single(value => value.Kind == "voice");
    Assert(voice.ElementId == $"realization:{voiceId:N}:part:0"
        && voice.Stability == "realization_identity"
        && voice.Selected == true, "native voice realization or selection was not observed");
    var caption = snapshot.Elements.Single(value => value.Kind == "caption");
    Assert(caption.Text == "visible caption" && caption.Stability == "session_only",
        "portable marker was exposed as text or overstated as realization identity");
    Assert(snapshot.Elements.Single(value => value.Kind == "image").Selected is null,
        "unavailable selection must be null rather than falsely reported as unselected");
    Assert(snapshot.Elements.Single(value => value.Kind == "image").Text is null,
        "unmanaged item text must remain opaque to the model-facing observer");
    Assert(snapshot.Viewport == new SceneCompositionViewportDto("unavailable", null, null)
        && snapshot.Elements.All(value => value.Visual
            == new SceneCompositionVisualDto("unavailable", null, null, null, null)),
        "unreflected geometry must be explicitly unavailable");
    Assert(snapshot.Completeness == "partial"
        && snapshot.UnavailableFields.SequenceEqual(
            [
                "elements[].anchor",
                "elements[].crop",
                "elements[].maskAndParentRelations",
                "elements[].opacity",
                "elements[].paintOrder",
                "elements[].rotation",
                "elements[].scale",
                "elements[].selected",
                "elements[].text",
                "elements[].visibility",
                "elements[].visual",
                "viewport",
            ]),
        "composition completeness does not explain unavailable fields deterministically");

    var selectionChanged = Ymm4Facade.BuildSceneCompositionSnapshot(
        projectId,
        "scene-a",
        Sha256("source"),
        60,
        15,
        items.Select(item => item with { Selected = !item.Selected }).ToArray());
    Assert(snapshot.Elements.Select(value => value.ElementId).SequenceEqual(
            selectionChanged.Elements.Select(value => value.ElementId)),
        "selection-only changes must not replace session element identities");

    var json = JsonSerializer.Serialize(snapshot, BridgeJson.Options);
    using var document = JsonDocument.Parse(json);
    var root = document.RootElement;
    Assert(root.GetProperty("schemaVersion").GetUInt32() == 1
        && root.GetProperty("sourceFingerprint").GetString() == Sha256("source")
        && root.GetProperty("viewport").GetProperty("width").ValueKind == JsonValueKind.Null
        && root.GetProperty("elements")[0].GetProperty("visual").GetProperty("x").ValueKind
            == JsonValueKind.Null,
        "current composition JSON wire names or unavailable placeholders changed");
}

static void CurrentSceneCompositionNeverEstimatesPlayhead()
{
    var exact = new ExactPreviewFrameFixture
    {
        CurrentFrame = 120,
        CurrentPositionRate = 0.75,
        StartPosition = TimeSpan.FromSeconds(1),
    };
    Assert(Ymm4Facade.ReadExactPreviewFrame(exact, 60) == 120,
        "directly exposed current frame was not observed");

    var estimatedOnly = new EstimatedPreviewFrameFixture
    {
        CurrentPositionRate = 0.75,
        StartPosition = TimeSpan.FromSeconds(1),
    };
    Assert(Ymm4Facade.ReadExactPreviewFrame(estimatedOnly, 60) is null,
        "composition observer estimated a playhead from progress or start position");
}

static void PortableMarkerRoundTrip()
{
    var marker = new ManagedMarker(
        "utt-01",
        4,
        "artifact-a",
        "speaker-a",
        @"C:\artifacts\a.wav",
        20);
    var encoded = MarkerCodec.Append("caption", marker);
    Assert(MarkerCodec.TryDecode(encoded, out var caption, out var decoded), "marker was not decoded");
    Assert(caption == "caption", "caption changed during marker round-trip");
    Assert(decoded == marker, "portable marker changed during round-trip");
}

static void StructuredCapabilityDigestGolden()
{
    var capabilities = new HashSet<string>(StringComparer.Ordinal)
    {
        "readback_verification",
        "request_bound_receipts",
        "write_ahead_apply",
        "recovery_readback",
        "managed_audio",
        "managed_caption",
        "unified_target_plan",
        "timeline_edit_managed_cue_mixed",
        "idempotent_apply",
        "undo_batch",
        "mutation_profile_ymm4_4_55_1_1",
        "project_checkpoint_verified",
        "project_initialize_save_as_verified",
        "native_portrait_upsert",
        "native_face_upsert",
        "native_image_upsert",
        "native_video_upsert",
        "native_audio_upsert",
        "native_effect_typed_mutation",
        "native_template_instantiate",
        "native_voice_create",
        "native_voice_update_replace_preserving_user_state",
        "native_voice_delete",
        "native_voice_exact_wav_export",
        "native_voice_host_bound_provenance",
        "native_voice_remark_identity",
        "native_voice_bounded_duration",
        "scene_capture_native_png",
        "scene_capture_playhead_restore",
        "scene_capture_content_hash",
        "scene_composition_current",
        "metadata_remark_detach",
    };
    var actual = Ymm4Facade.ComputeStructuredCapabilityDigest(
        capabilities,
        "0.2.0",
        "4.55.1.1");
    Assert(actual == "sha256:8a58fd233893233f43799a1aa0e63b1d1132d4229fa833f622467b382c71e8d5",
        $"structured capability digest differs from Rust: {actual}");
}

static void NativeExtensionMarkerRoundTrip()
{
    var realizationId = Guid.Parse("10000000-0000-0000-0000-000000000001");
    var effectRealizationId = Guid.Parse("20000000-0000-0000-0000-000000000002");
    var marker = NativeExtensionRemarkCodec.Create(
        "project-日本語",
        "背景-第一形態",
        7,
        "image:背景-第一形態",
        realizationId,
        "image",
        0,
        1,
        new Dictionary<string, NativeExtensionEffectMarker>(StringComparer.Ordinal)
        {
            ["invert"] = new(
                "invert",
                effectRealizationId,
                "effect:invert-v1",
                "YukkuriMovieMaker.Project.Effects.VideoEffects.InvertEffect",
                "video",
                0),
        });
    var encoded = NativeExtensionRemarkCodec.Append("user remark", marker);
    Assert(NativeExtensionRemarkCodec.TryDecode(encoded, out var decoded),
        "native-extension marker was not decoded");
    Assert(decoded is not null
        && decoded.ProjectId == marker.ProjectId
        && decoded.EntityId == marker.EntityId
        && decoded.Revision == marker.Revision
        && decoded.LogicalKey == marker.LogicalKey
        && decoded.RealizationId == marker.RealizationId
        && decoded.Kind == marker.Kind
        && decoded.PartIndex == marker.PartIndex
        && decoded.PartCount == marker.PartCount,
        "native-extension marker changed during round-trip");
    Assert(decoded!.Effects.TryGetValue("invert", out var decodedEffect)
        && decodedEffect == marker.Effects["invert"],
        "native-extension effect marker changed during round-trip");
    Assert(encoded.StartsWith("user remark", StringComparison.Ordinal),
        "native-extension marker replaced the user's remark");
}

static void UnifiedTargetPlanCueIsSealed()
{
    const string characterName = "魔理沙";
    var characterDigest = CanonicalJson.Sha256(
        "takegraph-ymm4-character-name-binding",
        characterName);
    object Dependency(string feature) => new
    {
        feature,
        minimumVersion = 1,
        schemaDigest = Ymm4Facade.StructuredFeatureSchemaDigest(
            feature,
            Ymm4Facade.StructuredFeaturePropertyNames(feature)),
    };
    var cue = new
    {
        intent = new
        {
            entityId = "utt-01",
            entityRevision = 4UL,
            displayText = "ここから第二形態だぜ",
            spokenText = "ここから第二形態だぜ",
            speakerRole = characterName,
            voiceProfile = characterName,
            captionStyle = (string?)null,
            segmentationLocked = false,
            placement = new
            {
                anchor = new { type = "absolute_frame", frame = 120 },
                ordering = "fixed",
                trackRole = "dialogue",
            },
            realizationPreference = "require_native",
            fallbackPolicy = "reject",
            acceptedPortableTake = (string?)null,
            template = (object?)null,
            effects = Array.Empty<object>(),
            hardLockPreconditions = Array.Empty<string>(),
        },
        realizationId = "11111111-1111-4111-8111-111111111111",
        action = "create",
        strategy = "native_voice",
        fallback = (object?)null,
        placement = new { frame = 120, primaryLayer = 20, secondaryLayer = (int?)null },
        duration = new { kind = "bounded", maxFrames = 180 },
        ownership = new
        {
            strict = new[] { "identity", "displayText", "spokenText", "characterBinding", "timingIntent" },
            derived = new[] { "length", "voiceCache" },
            preserve = new[] { "unknownNativeFields" },
            global = new[] { "characterDefinitions", "projectSettings" },
        },
        capabilityDependencies = new[]
        {
            Dependency("targetPlan.apply"),
            Dependency("voiceItem.create"),
            Dependency("timeline.transaction"),
            Dependency("readback.semantic"),
        },
        bindingDependencies = new[]
        {
            new { kind = "character_name_legacy", id = characterName, digest = characterDigest },
        },
        resolvedRealization = new
        {
            kind = "native_voice",
            characterName,
            characterBindingDigest = characterDigest,
        },
    };
    var json = JsonSerializer.Serialize(cue, BridgeJson.Options);
    using (var document = JsonDocument.Parse(json))
    {
        Ymm4Facade.ValidateTargetPlanCueContract(document.RootElement);
    }
    var rustNativeCue = System.Text.Json.Nodes.JsonNode.Parse(json)!.AsObject();
    var rustDuration = rustNativeCue["duration"]!.AsObject();
    rustDuration["max_frames"] = rustDuration["maxFrames"]!.GetValue<int>();
    rustDuration.Remove("maxFrames");
    var rustResolved = rustNativeCue["resolvedRealization"]!.AsObject();
    rustResolved["character_name"] = rustResolved["characterName"]!.GetValue<string>();
    rustResolved["character_binding_digest"] =
        rustResolved["characterBindingDigest"]!.GetValue<string>();
    rustResolved.Remove("characterName");
    rustResolved.Remove("characterBindingDigest");
    using (var rustDocument = JsonDocument.Parse(rustNativeCue.ToJsonString()))
    {
        Ymm4Facade.ValidateTargetPlanCueContract(rustDocument.RootElement);
    }
    var timelineNativeCue = System.Text.Json.Nodes.JsonNode.Parse(json)!.AsObject();
    timelineNativeCue["capabilityDependencies"]!.AsArray().Add(
        JsonSerializer.SerializeToNode(Dependency("timelineEdit.apply")));
    var timelineNativeDuration = timelineNativeCue["duration"]!.AsObject();
    timelineNativeDuration["max_frames"] = timelineNativeDuration["maxFrames"]!.GetValue<int>();
    timelineNativeDuration.Remove("maxFrames");
    var timelineNativeResolved = timelineNativeCue["resolvedRealization"]!.AsObject();
    timelineNativeResolved["character_name"] =
        timelineNativeResolved["characterName"]!.GetValue<string>();
    timelineNativeResolved["character_binding_digest"] =
        timelineNativeResolved["characterBindingDigest"]!.GetValue<string>();
    timelineNativeResolved.Remove("characterName");
    timelineNativeResolved.Remove("characterBindingDigest");
    using (var document = JsonDocument.Parse(timelineNativeCue.ToJsonString()))
    {
        Ymm4Facade.ValidateTimelineEditCueContract(document.RootElement);
    }

    var wrongPlacement = json.Replace(
        "\"primaryLayer\": 20",
        "\"primaryLayer\": 20, \"rawLayer\": 99",
        StringComparison.Ordinal);
    using (var document = JsonDocument.Parse(wrongPlacement))
    {
        ExpectBridgeValidation(
            () => Ymm4Facade.ValidateTargetPlanCueContract(document.RootElement),
            "target plan accepted an unsealed raw placement override");
    }
    var wrongStrategy = json.Replace(
        "\"kind\": \"native_voice\"",
        "\"kind\": \"portable_pair\"",
        StringComparison.Ordinal);
    using (var document = JsonDocument.Parse(wrongStrategy))
    {
        ExpectBridgeValidation(
            () => Ymm4Facade.ValidateTargetPlanCueContract(document.RootElement),
            "target plan accepted a strategy/resolved-realization mismatch");
    }

    var artifactHash = new string('a', 64);
    var portableCue = new
    {
        intent = new
        {
            entityId = "utt-portable-01",
            entityRevision = 5UL,
            displayText = "第二形態だぜ",
            spokenText = "だいにけいたいだぜ",
            speakerRole = characterName,
            voiceProfile = characterName,
            captionStyle = (string?)null,
            segmentationLocked = false,
            placement = new
            {
                anchor = new { type = "absolute_frame", frame = 240 },
                ordering = "fixed",
                trackRole = "dialogue",
            },
            realizationPreference = "require_portable",
            fallbackPolicy = "reject",
            acceptedPortableTake = (string?)null,
            template = (object?)null,
            effects = Array.Empty<object>(),
            hardLockPreconditions = Array.Empty<string>(),
        },
        realizationId = "22222222-2222-4222-8222-222222222222",
        action = "create",
        strategy = "portable_pair",
        fallback = (object?)null,
        placement = new { frame = 240, primaryLayer = 20, secondaryLayer = (int?)21 },
        duration = new { kind = "exact", frames = 90 },
        ownership = new
        {
            strict = new[] { "identity", "captionText", "audioArtifact", "timing" },
            derived = Array.Empty<string>(),
            preserve = new[] { "unknownNativeFields" },
            global = new[] { "projectSettings" },
        },
        capabilityDependencies = new[]
        {
            Dependency("targetPlan.apply"),
            Dependency("managedPair.apply"),
            Dependency("timeline.transaction"),
            Dependency("readback.semantic"),
        },
        bindingDependencies = new[]
        {
            new { kind = "audio_artifact", id = artifactHash, digest = $"sha256:{artifactHash}" },
        },
        resolvedRealization = new
        {
            kind = "portable_pair",
            audioPath = @"C:\artifacts\take.wav",
            artifactDigest = $"sha256:{artifactHash}",
        },
    };
    using (var document = JsonDocument.Parse(
               JsonSerializer.Serialize(portableCue, BridgeJson.Options)))
    {
        Ymm4Facade.ValidateTargetPlanCueContract(document.RootElement);
    }
    var timelineCue = System.Text.Json.Nodes.JsonNode.Parse(
        JsonSerializer.Serialize(portableCue, BridgeJson.Options))!.AsObject();
    timelineCue["capabilityDependencies"]!.AsArray().Add(
        JsonSerializer.SerializeToNode(Dependency("timelineEdit.apply")));
    var timelineResolved = timelineCue["resolvedRealization"]!.AsObject();
    timelineResolved["audio_path"] = timelineResolved["audioPath"]!.GetValue<string>();
    timelineResolved["artifact_digest"] = timelineResolved["artifactDigest"]!.GetValue<string>();
    timelineResolved.Remove("audioPath");
    timelineResolved.Remove("artifactDigest");
    using (var document = JsonDocument.Parse(timelineCue.ToJsonString()))
    {
        Ymm4Facade.ValidateTimelineEditCueContract(document.RootElement);
    }
}

static void TimelineEditHistoricalBindingIsLiveStateIndependent()
{
    using var document = JsonDocument.Parse(
        """
        {
          "canonicalVersion":1,
          "operationId":"11111111-1111-4111-8111-111111111111",
          "baseRevision":7,
          "target":{"adapterId":"historical-driver","projectId":"closed-project","sceneId":"old-scene","fps":60,"driverVersion":"old/old"},
          "capabilityDigest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          "expectedScope":{"targetIdentityDigest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","managedStateDigest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","conflictScopeDigest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},
          "changeBudget":{"maxChangedEntities":1,"maxShiftedEntities":0,"maxShiftFrames":0,"allowLockedChanges":false,"allowUnmanagedChanges":false},
          "operations":[{"kind":"native_extension","descriptorCatalogDigest":"sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","operation":{}}],
          "warnings":[]
        }
        """);
    var digest = CanonicalJson.Sha256("takegraph-timeline-edit-plan-v1", document.RootElement);
    // This helper deliberately has no YMM4 snapshot/capability dependency: a
    // verified historical receipt must replay after the active target changes.
    Ymm4Facade.ValidateTimelineEditHistoricalBindingForTests(digest, document.RootElement);
    ExpectBridgeValidation(
        () => Ymm4Facade.ValidateTimelineEditHistoricalBindingForTests(
            $"sha256:{new string('f', 64)}",
            document.RootElement),
        "historical timeline-edit binding accepted a different plan digest");
}

static void TimelineEditNativeOnlyPayloadsAreAccepted()
{
    var native = new NativeVoiceCueDto(
        Guid.Parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
        "live-native-only",
        3,
        "ゆっくり霊夢",
        "ライブ動作確認",
        "ライブ動作確認",
        1000,
        2,
        180);
    var hash = new string('a', 64);
    var portable = new ManagedUtteranceDto(
        "utt-01",
        1,
        "魔理沙",
        "caption",
        @"C:\artifacts\take.wav",
        hash,
        10,
        20,
        1,
        2,
        "spoken");

    Ymm4Facade.ValidateTimelineEditPayloads([], [native], verifyArtifacts: false);
    Ymm4Facade.ValidateTimelineEditPayloads([portable], [], verifyArtifacts: false);
    Ymm4Facade.ValidateTimelineEditPayloads([portable], [native], verifyArtifacts: false);
    Ymm4Facade.ValidateTimelineEditPayloads(
        [],
        [native with { DisplayText = "表示", SpokenText = "発話" }],
        verifyArtifacts: false);
    Ymm4Facade.ValidateTimelineEditPayloads(
        [],
        [native with { SpokenText = null }],
        verifyArtifacts: false);
    ExpectBridgeValidation(
        () => Ymm4Facade.ValidateTimelineEditPayloads([], [], verifyArtifacts: false),
        "empty timeline-edit payloads were accepted");
}

static void UnifiedPortableActionsBindCurrentPairState()
{
    var hash = new string('a', 64);
    var utterance = new ManagedUtteranceDto(
        "utt-01",
        4,
        "魔理沙",
        "ここから第二形態だぜ",
        @"C:\artifacts\take.wav",
        hash,
        120,
        60,
        20,
        21);
    var audio = new ManagedItemDto(
        "utt-01", 3, "audio", 100, 20, 50, null, @"C:\old.wav", hash, "魔理沙");
    var caption = new ManagedItemDto(
        "utt-01", 3, "caption", 100, 21, 50, "旧テキスト", null, hash, "魔理沙");
    ManagedItemDto[] pair = [audio, caption];

    Ymm4Facade.ValidatePortableTargetPlanActions([utterance], ["create"], [], hash);
    Ymm4Facade.ValidatePortableTargetPlanActions([utterance], ["update"], pair, hash);
    ExpectBridgeConflict(
        () => Ymm4Facade.ValidatePortableTargetPlanActions([utterance], ["create"], pair, hash),
        "portable create accepted an existing managed identity");
    ExpectBridgeConflict(
        () => Ymm4Facade.ValidatePortableTargetPlanActions([utterance], ["update"], [], hash),
        "portable update accepted a missing managed identity");
    ExpectBridgeConflict(
        () => Ymm4Facade.ValidatePortableTargetPlanActions(
            [utterance],
            ["update"],
            [audio, caption, caption with { Layer = 22 }],
            hash),
        "portable update accepted duplicate pair members");
    ExpectBridgeConflict(
        () => Ymm4Facade.ValidatePortableTargetPlanActions(
            [utterance],
            ["update"],
            [audio, audio with { Layer = 22 }],
            hash),
        "portable update accepted two audio members");
    ExpectBridgeConflict(
        () => Ymm4Facade.ValidatePortableTargetPlanActions(
            [utterance],
            ["update"],
            [new ManagedItemDto(
                "utt-01",
                3,
                "voice",
                100,
                20,
                50,
                "旧テキスト",
                null,
                null,
                "魔理沙",
                "11111111-1111-4111-8111-111111111111")],
            hash),
        "portable update accepted a heterogeneous managed realization");
}

static void MarkerOwnershipFieldsRequired()
{
    var voice = new NativeVoiceMarker(
        "takegraph/v2",
        string.Empty,
        "utt-a",
        Guid.NewGuid(),
        1);
    Assert(!RemarkCodec.TryDecode(RemarkCodec.Append(null, voice), out _),
        "native voice marker without project ownership was accepted");

    var extension = NativeExtensionRemarkCodec.Create(
        string.Empty,
        "image-a",
        1,
        "image:image-a",
        Guid.NewGuid(),
        "image",
        0,
        1);
    Assert(!NativeExtensionRemarkCodec.TryDecode(
            NativeExtensionRemarkCodec.Append(null, extension),
            out _),
        "native-extension marker without project ownership was accepted");
}

static void MetadataDetachWalIsDurableAndBound()
{
    WithTemporaryDirectory(directory =>
    {
        var request = ValidMetadataDetachRequest();
        var now = DateTimeOffset.UtcNow;
        var target = new MetadataDetachTargetDto(
            "YukkuriMovieMaker.Project.Items.VoiceItem, YukkuriMovieMaker",
            10,
            20,
            30,
            "user remark",
            Sha256("user remark"),
            Sha256("content"));
        var targetWitness = new MetadataDetachSceneRemarkDto(
            MetadataDetachStore.ComputeStableItemWitness(
                target.TypeName,
                target.Frame,
                target.Layer,
                target.Length,
                target.NonRemarkContentDigest),
            target.TypeName,
            target.Frame,
            target.Layer,
            target.Length,
            target.NonRemarkContentDigest,
            target.OriginalRemark,
            target.RemarkSha256,
            "user remark after selected identity removal",
            Sha256("user remark after selected identity removal"));
        var unrelatedWitness = new MetadataDetachSceneRemarkDto(
            MetadataDetachStore.ComputeStableItemWitness(
                "YukkuriMovieMaker.Project.Items.TextItem, YukkuriMovieMaker",
                40,
                21,
                30,
                Sha256("other-content")),
            "YukkuriMovieMaker.Project.Items.TextItem, YukkuriMovieMaker",
            40,
            21,
            30,
            Sha256("other-content"),
            "unrelated user remark",
            Sha256("unrelated user remark"),
            "unrelated user remark",
            Sha256("unrelated user remark"));
        MetadataDetachSceneRemarkDto[] sceneRemarks = [targetWitness, unrelatedWitness];
        var entry = new MetadataDetachJournalDto(
            2,
            request,
            "applying",
            request.ExpectedFingerprint,
            null,
            MetadataDetachStore.ComputeSceneRemarkDigest(sceneRemarks, expected: false),
            MetadataDetachStore.ComputeSceneRemarkDigest(sceneRemarks, expected: true),
            null,
            Sha256("scene"),
            null,
            [target],
            sceneRemarks,
            false,
            now,
            now,
            null);
        var store = new MetadataDetachStore(directory);
        store.Put(entry);
        var aggregateTamperRejected = false;
        try
        {
            store.Put(entry with { BeforeRemarkDigest = Sha256("tampered") });
        }
        catch (InvalidDataException)
        {
            aggregateTamperRejected = true;
        }
        Assert(aggregateTamperRejected,
            "detach WAL accepted a tampered Remark-set digest");
        var unrelatedDrift = unrelatedWitness with
        {
            ExpectedRemark = "unauthorized concurrent edit",
            ExpectedRemarkSha256 = Sha256("unauthorized concurrent edit"),
        };
        var unrelatedDriftScene = new[] { targetWitness, unrelatedDrift };
        var unrelatedDriftRejected = false;
        try
        {
            store.Put(entry with
            {
                SceneRemarks = unrelatedDriftScene,
                ExpectedAfterRemarkDigest = MetadataDetachStore.ComputeSceneRemarkDigest(
                    unrelatedDriftScene,
                    expected: true),
            });
        }
        catch (InvalidDataException)
        {
            unrelatedDriftRejected = true;
        }
        Assert(unrelatedDriftRejected,
            "detach WAL authorized a change to an unrelated scene Remark");
        var duplicateWitnessRejected = false;
        try
        {
            store.Put(entry with { SceneRemarks = [targetWitness, targetWitness] });
        }
        catch (InvalidDataException)
        {
            duplicateWitnessRejected = true;
        }
        Assert(duplicateWitnessRejected,
            "detach WAL accepted ambiguous duplicate stable item witnesses");
        Assert(store.ReadPending().Count == 1, "detach WAL was not pending");
        var reloaded = new MetadataDetachStore(directory);
        Assert(reloaded.TryGet(request.OperationId, out var durable)
            && durable is not null
            && MetadataDetachStore.SameBinding(durable.Request, entry.Request)
            && durable.State == entry.State
            && durable.BeforeFingerprint == entry.BeforeFingerprint
            && durable.Targets.SequenceEqual(entry.Targets)
            && durable.SceneRemarks.SequenceEqual(entry.SceneRemarks),
            "detach WAL did not survive reopen");
        var verified = reloaded.Transition(
            request.OperationId,
            "verified",
            Sha256("after"),
            entry.ExpectedAfterRemarkDigest,
            entry.NonRemarkContentDigestBefore,
            true,
            null);
        var receipt = MetadataDetachStore.ToReceipt(verified);
        Assert(receipt.Verified && receipt.RemarkAbsent,
            "verified detach receipt lost its proof");
        Assert(receipt.NonRemarkContentDigestBefore == receipt.NonRemarkContentDigestAfter,
            "verified detach receipt changed non-Remark content");

        var rebound = request with { EntityId = "another-entity" };
        var reboundDigest = ApplyRequestDigest.Compute(rebound);
        rebound = rebound with { RequestDigest = reboundDigest };
        ExpectBridgeConflict(
            () => reloaded.Put(entry with { Request = rebound }),
            "detach operation ID accepted a rebound identity");
    });
}

static void MetadataDetachNotStartedIsDurable()
{
    WithTemporaryDirectory(directory =>
    {
        var request = ValidMetadataDetachRequest();
        var store = new MetadataDetachStore(directory);
        var tombstone = MetadataDetachStore.CreateNotStarted(
            request,
            request.ExpectedFingerprint,
            "pre-WAL request rejected");
        store.Put(tombstone);
        Assert(store.ReadPending().Count == 0,
            "not-started tombstone entered recovery work");

        var reopened = new MetadataDetachStore(directory);
        Assert(reopened.TryGet(request.OperationId, out var durable)
            && durable is not null
            && durable.State == "not_started"
            && MetadataDetachStore.SameBinding(request, durable.Request),
            "not-started tombstone did not survive reopen");
        var receipt = MetadataDetachStore.ToReceipt(durable!);
        Assert(!receipt.Verified
            && receipt.DetachedItemCount == 0
            && receipt.BeforeFingerprint == request.ExpectedFingerprint
            && receipt.AfterFingerprint == receipt.BeforeFingerprint
            && receipt.BeforeRemarkDigest == receipt.ExpectedAfterRemarkDigest
            && receipt.BeforeRemarkDigest == receipt.RemarkDigestAfter,
            "not-started receipt did not prove no mutation");

        var rebound = request with { EntityId = "rebound" };
        rebound = rebound with { RequestDigest = ApplyRequestDigest.Compute(rebound) };
        ExpectBridgeConflict(
            () => reopened.Put(MetadataDetachStore.CreateNotStarted(
                rebound,
                rebound.ExpectedFingerprint,
                "rebound")),
            "not-started tombstone accepted a rebound request");
    });
}

static void MetadataDetachRemovesSelectedRemarkIdentity()
{
    var request = ValidMetadataDetachRequest();
    var voice = new NativeVoiceMarker(
        "takegraph/v2",
        request.ProjectId,
        request.EntityId,
        request.RealizationId,
        4);
    var voiceRemark = RemarkCodec.Append("user note", voice);
    var detached = Ymm4Facade.RemoveMetadataIdentity(voiceRemark, request, out var matched);
    Assert(matched, "native voice identity was not matched");
    Assert(detached == "user note", "native voice detach changed the user Remark");

    var otherEffect = Guid.NewGuid();
    var baseRealization = Guid.NewGuid();
    var extension = NativeExtensionRemarkCodec.Create(
        request.ProjectId,
        request.EntityId,
        4,
        "image:utt-01",
        baseRealization,
        "image",
        0,
        1,
        new Dictionary<string, NativeExtensionEffectMarker>(StringComparer.Ordinal)
        {
            ["selected"] = new(
                "selected", request.RealizationId, "descriptor:selected", "Effect.Selected", "video", 0),
            ["other"] = new(
                "other", otherEffect, "descriptor:other", "Effect.Other", "video", 1),
        });
    var extensionRemark = NativeExtensionRemarkCodec.Append("user note", extension);
    detached = Ymm4Facade.RemoveMetadataIdentity(extensionRemark, request, out matched);
    Assert(matched, "native extension effect identity was not matched");
    Assert(NativeExtensionRemarkCodec.TryDecode(detached, out var decoded)
        && decoded is not null
        && !decoded.Effects.Values.Any(value => value.RealizationId == request.RealizationId)
        && decoded.Effects.Values.Any(value => value.RealizationId == otherEffect),
        "effect detach removed the wrong native-extension identity");
    Assert(detached.StartsWith("user note", StringComparison.Ordinal),
        "effect detach changed the user Remark");

    var baseRequest = request with { RealizationId = baseRealization };
    baseRequest = baseRequest with
    {
        RequestDigest = ApplyRequestDigest.Compute(baseRequest),
    };
    ExpectBridgeConflict(
        () => Ymm4Facade.RemoveMetadataIdentity(extensionRemark, baseRequest, out _),
        "base native-extension detach erased independently managed effect identities");
}

static MetadataDetachRequestDto ValidMetadataDetachRequest()
{
    var request = new MetadataDetachRequestDto(
        BridgeContract.ProtocolVersion,
        Guid.NewGuid(),
        string.Empty,
        "project-a",
        "scene-a",
        7,
        Sha256("before"),
        "utt-01",
        Guid.NewGuid(),
        "takegraph_remark_v2");
    return request with { RequestDigest = ApplyRequestDigest.Compute(request) };
}

static void ReceiptJournalPersistence()
{
    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "receipts.json");
        var operationId = Guid.NewGuid();
        var receipt = ValidReceipt(operationId, "verified") with
        {
            AfterFingerprint = Sha256("receipt-after"),
        };
        new ReceiptStore(path).Put(receipt);
        Assert(new ReceiptStore(path).TryGet(operationId, out var loaded), "receipt was not reloaded");
        if (loaded is null)
        {
            throw new InvalidOperationException("reloaded receipt is null");
        }
        Assert(loaded.OperationId == receipt.OperationId, "operation ID changed");
        Assert(loaded.RequestDigest == receipt.RequestDigest, "request digest changed");
        Assert(loaded.ProjectId == receipt.ProjectId, "project ID changed");
        Assert(loaded.SceneId == receipt.SceneId, "scene ID changed");
        Assert(loaded.Status == receipt.Status && loaded.Verified, "receipt status changed");
        Assert(loaded.AppliedItems.Count == 0, "receipt items changed");
        Assert(!Directory.EnumerateFiles(directory, "*.tmp").Any(), "temporary journal remained");
    });
}

static void CorruptReceiptJournalFailsClosed()
{
    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "receipts.json");
        File.WriteAllText(path, "{ definitely not json");
        var store = new ReceiptStore(path);
        try
        {
            _ = store.TryGet(Guid.NewGuid(), out _);
            throw new InvalidOperationException("corrupt journal was accepted");
        }
        catch (BridgeUnavailableException)
        {
            // Expected: a corrupt durable journal must disable mutation.
        }
    });

    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "receipts.json");
        var receipt = ValidReceipt(Guid.NewGuid(), "verified");
        var storedUnderAnotherId = new Dictionary<Guid, OperationReceiptDto>
        {
            [Guid.NewGuid()] = receipt,
        };
        File.WriteAllText(path, JsonSerializer.Serialize(storedUnderAnotherId, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new ReceiptStore(path).ReadAll(),
            "receipt storage ID mismatch was accepted");
    });

    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "receipts.json");
        var operationId = Guid.NewGuid();
        var json = JsonSerializer.SerializeToNode(
            new Dictionary<Guid, OperationReceiptDto>
            {
                [operationId] = ValidReceipt(operationId, "verified"),
            },
            BridgeJson.Options)!;
        json[operationId.ToString()]!["unexpected"] = true;
        File.WriteAllText(path, json.ToJsonString(BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new ReceiptStore(path).ReadAll(),
            "unknown receipt field was accepted");
    });
}

static void LegacyReceiptMigrationPreservesEvidence()
{
    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "receipts.json");
        var operationId = Guid.NewGuid();
        var item = new ManagedItemDto(
            "utt-legacy",
            1,
            "caption",
            10,
            20,
            30,
            "legacy",
            null,
            Sha256("legacy-artifact"));
        var legacy = new Dictionary<Guid, object>
        {
            [operationId] = new
            {
                operationId,
                status = "verified",
                beforeFingerprint = Sha256("legacy-before"),
                afterFingerprint = Sha256("legacy-after"),
                appliedItems = new[] { item },
                verified = true,
                error = (string?)null,
            },
        };
        File.WriteAllText(path, JsonSerializer.Serialize(legacy, BridgeJson.Options));

        var store = new ReceiptStore(path);
        Assert(store.ReadAll().Count == 0, "unbound legacy receipt remained replayable");
        var archivePath = Path.Combine(directory, "receipts.legacy-v1.json");
        Assert(File.Exists(archivePath), "legacy receipt evidence was not archived");
        Assert(File.ReadAllText(archivePath).Contains(operationId.ToString(), StringComparison.OrdinalIgnoreCase),
            "legacy archive lost the operation identity");
        var current = JsonSerializer.Deserialize<Dictionary<Guid, OperationReceiptDto>>(
            File.ReadAllText(path), BridgeJson.Options);
        Assert(current is { Count: 0 }, "current receipt journal retained an unbound receipt");

        try
        {
            _ = store.TryGet(operationId, out _);
            throw new InvalidOperationException("legacy operation ID was replayable");
        }
        catch (BridgeConflictException)
        {
            // Archived protocol-1 IDs are durable tombstones.
        }

        var currentOperation = Guid.NewGuid();
        store.Put(ValidReceipt(currentOperation, "verified"));
        var reloaded = new ReceiptStore(path);
        Assert(reloaded.TryGet(currentOperation, out _), "current receipt did not survive migration");
        try
        {
            reloaded.Put(ValidReceipt(operationId, "verified"));
            throw new InvalidOperationException("legacy operation ID was rebound");
        }
        catch (BridgeConflictException)
        {
            // Expected.
        }
    });

    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "receipts.json");
        var operationId = Guid.NewGuid();
        var malformed = new Dictionary<Guid, object>
        {
            [operationId] = new
            {
                operationId,
                requestDigest = Sha256("partial-binding"),
                status = "verified",
                beforeFingerprint = Sha256("legacy-before"),
                afterFingerprint = Sha256("legacy-after"),
                appliedItems = Array.Empty<ManagedItemDto>(),
                verified = true,
                error = (string?)null,
            },
        };
        File.WriteAllText(path, JsonSerializer.Serialize(malformed, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new ReceiptStore(path).ReadAll(),
            "partially bound receipt was migrated as legacy");
    });
}

static void RecoveryJournalPersistence()
{
    WithTemporaryDirectory(directory =>
    {
        var operationId = Guid.NewGuid();
        var entry = ValidRecoveryEntry(operationId);
        var store = new RecoveryJournalStore(directory);
        store.Put(entry);
        Assert(store.ReadPending().Count == 1, "applying recovery entry was not pending");
        var reloaded = new RecoveryJournalStore(directory);
        Assert(reloaded.ReadPending().Single().RequestDigest == entry.RequestDigest,
            "recovery binding changed");
        var after = Sha256("after-a");
        reloaded.Transition(operationId, "verified", after, null);
        var terminal = new RecoveryJournalStore(directory).ReadAll().Single();
        Assert(terminal.State == "verified", "recovery transition was not durable");
        Assert(terminal.AfterFingerprint == after, "recovery fingerprint was not durable");

        var unresolved = ValidRecoveryEntry(Guid.NewGuid()) with
        {
            State = "recovery_required",
            AfterFingerprint = Sha256("unresolved-after"),
            Error = "manual recovery is required",
        };
        reloaded.Put(unresolved);
        Assert(reloaded.ReadPending().Single().OperationId == unresolved.OperationId,
            "recovery_required entry was not authorization-pending");
        Assert(reloaded.ReadRecoverable().Count == 0,
            "recovery_required entry was incorrectly treated as automatically recoverable");
        Assert(!Directory.EnumerateFiles(directory, "*.tmp-*").Any(), "temporary recovery journal remained");

        reloaded.Transition(unresolved.OperationId, "failed", unresolved.AfterFingerprint, unresolved.Error);
        var acknowledged = new RecoveryJournalStore(directory).ReadAll()
            .Single(value => value.OperationId == unresolved.OperationId);
        Assert(acknowledged.State == "failed", "operator ack did not close recovery_required");
        Assert(new RecoveryJournalStore(directory).ReadPending().Count == 0,
            "acknowledged recovery_required remained pending");
    });
}

static void AppliedUnverifiedRecoveryIsHistorical()
{
    foreach (var driver in new[]
             {
                 "portable_pair", "native_voice_create", "native_voice_mutation",
             })
    {
        foreach (var receiptWasAlreadyPublished in new[] { false, true })
        {
            WithTemporaryDirectory(directory =>
            {
                var operationId = Guid.NewGuid();
                var entry = ValidRecoveryEntryForDriver(operationId, driver);
                var recoveryRoot = Path.Combine(directory, "recovery");
                var receiptPath = Path.Combine(directory, "receipts.json");
                var recoveryStore = new RecoveryJournalStore(recoveryRoot);
                var receiptStore = new ReceiptStore(receiptPath);
                recoveryStore.Put(entry);
                var verifiedItems = entry.ExpectedItems.ToArray();
                var after = Sha256($"historical-after-{driver}");
                recoveryStore.MarkAppliedUnverified(operationId, after, verifiedItems);

                var reloaded = new RecoveryJournalStore(recoveryRoot).ReadRecoverable().Single();
                Assert(reloaded.State == "applied_unverified"
                    && reloaded.AfterFingerprint == after
                    && reloaded.VerifiedItems?.Count == verifiedItems.Length,
                    $"{driver} successful read-back evidence was not durable");
                try
                {
                    _ = recoveryStore.Transition(
                        operationId,
                        "rolled_back",
                        entry.BeforeFingerprint,
                        "must not roll back verified read-back");
                    throw new InvalidOperationException(
                        "applied-unverified evidence was allowed to roll back");
                }
                catch (BridgeConflictException)
                {
                    // Successful read-back may finalize or fail closed, never undo.
                }

                if (receiptWasAlreadyPublished)
                {
                    receiptStore.Put(VerifiedReceipt(entry, after, verifiedItems));
                }
                var facade = new Ymm4Facade(
                    new ReceiptStore(receiptPath),
                    new RecoveryJournalStore(recoveryRoot),
                    new MetadataDetachStore(Path.Combine(directory, "detach")),
                    new ProjectOperationStore(Path.Combine(directory, "operations")));
                Assert(facade.RecoverPendingOperationsForTestsAsync()
                    .GetAwaiter().GetResult(), $"{driver} historical recovery did not complete");
                var durableReceipt = new ReceiptStore(receiptPath).ReadAll().Single();
                Assert(durableReceipt.AfterFingerprint == after && durableReceipt.Verified,
                    $"{driver} verified receipt was not reconstructed after restart");
                Assert(new RecoveryJournalStore(recoveryRoot).ReadAll().Single().State == "verified",
                    $"{driver} recovery WAL did not finalize historically");
            });
        }
    }

    foreach (var receiptWasAlreadyPublished in new[] { false, true })
    {
        WithTemporaryDirectory(directory =>
        {
            var operationId = Guid.NewGuid();
            var entry = ValidRecoveryEntryForDriver(operationId, "native_extension");
            var receipt = new NativeExtensionApplyResponseDto(
                operationId,
                entry.RequestDigest,
                entry.ProjectId,
                entry.SceneId,
                "verified",
                entry.BeforeFingerprint,
                Sha256("extension-after"),
                Sha256("catalog"),
                Sha256("driver"),
                [],
                true,
                null);
            var recoveryRoot = Path.Combine(directory, "recovery");
            var operationRoot = Path.Combine(directory, "operations");
            var recoveryStore = new RecoveryJournalStore(recoveryRoot);
            var operationStore = new ProjectOperationStore(operationRoot);
            recoveryStore.Put(entry);
            operationStore.PutNativeExtension(receipt with
            {
                Status = "applying",
                AfterFingerprint = receipt.BeforeFingerprint,
                Verified = false,
            });
            recoveryStore.MarkAppliedUnverified(
                operationId,
                receipt.AfterFingerprint,
                [],
                receipt);
            if (receiptWasAlreadyPublished)
            {
                operationStore.PutNativeExtension(receipt);
            }

            var facade = new Ymm4Facade(
                new ReceiptStore(Path.Combine(directory, "receipts.json")),
                new RecoveryJournalStore(recoveryRoot),
                new MetadataDetachStore(Path.Combine(directory, "detach")),
                new ProjectOperationStore(operationRoot));
            Assert(facade.RecoverPendingOperationsForTestsAsync()
                .GetAwaiter().GetResult(), "native-extension historical recovery did not complete");
            Assert(new ProjectOperationStore(operationRoot)
                    .TryGetNativeExtension(operationId, out var recovered)
                && recovered is { Status: "verified", Verified: true }
                && recovered.AfterFingerprint == receipt.AfterFingerprint,
                "native-extension rich receipt was not reconstructed after restart");
            Assert(new RecoveryJournalStore(recoveryRoot).ReadAll().Single().State == "verified",
                "native-extension recovery WAL did not finalize historically");
        });
    }
}

static void RecoveryJournalInvariantsFailClosed()
{
    WithTemporaryDirectory(directory =>
    {
        var entry = ValidRecoveryEntry(Guid.NewGuid()) with
        {
            Driver = "untrusted_driver",
        };
        var path = Path.Combine(directory, $"{entry.OperationId:N}.json");
        File.WriteAllText(path, JsonSerializer.Serialize(entry, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new RecoveryJournalStore(directory).ReadAll(),
            "unknown recovery driver was accepted");
    });

    WithTemporaryDirectory(directory =>
    {
        var entry = ValidRecoveryEntry(Guid.NewGuid());
        var invalidPreimage = entry.BeforeItems.Single() with { Sha256 = Sha256("other") };
        entry = entry with { BeforeItems = [invalidPreimage] };
        var path = Path.Combine(directory, $"{entry.OperationId:N}.json");
        File.WriteAllText(path, JsonSerializer.Serialize(entry, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new RecoveryJournalStore(directory).ReadAll(),
            "recovery preimage hash mismatch was accepted");
    });

    WithTemporaryDirectory(directory =>
    {
        var entry = ValidRecoveryEntry(Guid.NewGuid()) with { State = "unknown" };
        var path = Path.Combine(directory, $"{entry.OperationId:N}.json");
        File.WriteAllText(path, JsonSerializer.Serialize(entry, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new RecoveryJournalStore(directory).ReadAll(),
            "unknown recovery state was accepted");
    });

    WithTemporaryDirectory(directory =>
    {
        var entry = ValidRecoveryEntry(Guid.NewGuid());
        var path = Path.Combine(directory, $"{Guid.NewGuid():N}.json");
        File.WriteAllText(path, JsonSerializer.Serialize(entry, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new RecoveryJournalStore(directory).ReadAll(),
            "recovery storage ID mismatch was accepted");
    });

    WithTemporaryDirectory(directory =>
    {
        var entry = ValidRecoveryEntry(Guid.NewGuid()) with { RequestDigest = "not-a-hash" };
        var path = Path.Combine(directory, $"{entry.OperationId:N}.json");
        File.WriteAllText(path, JsonSerializer.Serialize(entry, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new RecoveryJournalStore(directory).ReadAll(),
            "invalid recovery request hash was accepted");
    });

    WithTemporaryDirectory(directory =>
    {
        var entry = ValidRecoveryEntryForDriver(
            Guid.NewGuid(),
            "timeline_edit_managed_cue_mixed") with
        {
            EntityIds = ["utt-a", "portable-a", "unapproved-extra"],
        };
        var path = Path.Combine(directory, $"{entry.OperationId:N}.json");
        File.WriteAllText(path, JsonSerializer.Serialize(entry, BridgeJson.Options));
        ExpectBridgeUnavailable(
            () => _ = new RecoveryJournalStore(directory).ReadAll(),
            "timeline-edit WAL accepted an identity outside its expected state");
    });
}

static void StorePersistencePrecedesMemory()
{
    WithTemporaryDirectory(directory =>
    {
        var operationId = Guid.NewGuid();
        var path = Path.Combine(directory, "receipts.json");
        var original = ValidReceipt(operationId, "applying");
        var store = new ReceiptStore(path);
        store.Put(original);
        WithFaultPoint("before_receipt_persist_commit", () =>
        {
            try
            {
                store.Put(original with
                {
                    Status = "verified",
                    Verified = true,
                    AfterFingerprint = Sha256("receipt-after"),
                });
                throw new InvalidOperationException("receipt persist fault did not throw");
            }
            catch (BridgeSimulatedCrashException)
            {
                // Expected.
            }
        });
        Assert(store.TryGet(operationId, out var inMemory) && inMemory?.Status == "applying",
            "failed receipt persistence advanced in-memory state");
        Assert(new ReceiptStore(path).TryGet(operationId, out var durable) && durable?.Status == "applying",
            "failed receipt persistence advanced durable state");
    });

    WithTemporaryDirectory(directory =>
    {
        var operationId = Guid.NewGuid();
        var store = new RecoveryJournalStore(directory);
        store.Put(ValidRecoveryEntry(operationId));
        WithFaultPoint("before_recovery_persist_commit", () =>
        {
            try
            {
                store.Transition(operationId, "verified", Sha256("recovery-after"), null);
                throw new InvalidOperationException("recovery persist fault did not throw");
            }
            catch (BridgeSimulatedCrashException)
            {
                // Expected.
            }
        });
        Assert(store.ReadAll().Single().State == "applying",
            "failed recovery persistence advanced in-memory state");
        Assert(new RecoveryJournalStore(directory).ReadAll().Single().State == "applying",
            "failed recovery persistence advanced durable state");
    });
}

static void ForeignRecoveryBlocksAuthorization()
{
    var foreign = ValidRecoveryEntry(Guid.NewGuid()) with
    {
        ProjectId = "foreign-project",
        State = "recovery_required",
        AfterFingerprint = Sha256("foreign-after"),
        Error = "unresolved foreign operation",
    };
    ExpectBridgeUnavailable(
        () => Ymm4Facade.EnsureRecoveryAuthorizationClear([foreign], []),
        "unresolved foreign recovery did not block writes");
    Ymm4Facade.EnsureRecoveryAuthorizationClear([], []);
}

static void ProjectSavedSignalsFailClosed()
{
    Assert(!Ymm4Facade.SavedSignalsIndicateDirty([true, true]),
        "unanimous saved signals were treated as dirty");
    Assert(Ymm4Facade.SavedSignalsIndicateDirty([true, false]),
        "a dirty signal was masked by a saved signal");
    ExpectBridgeUnavailable(
        () => _ = Ymm4Facade.SavedSignalsIndicateDirty([]),
        "missing dirty-state evidence was treated as saved");
}

static void CorruptRecoveryJournalFailsClosed()
{
    WithTemporaryDirectory(directory =>
    {
        File.WriteAllText(Path.Combine(directory, $"{Guid.NewGuid():N}.json"), "{ not valid json");
        var store = new RecoveryJournalStore(directory);
        try
        {
            _ = store.ReadPending();
            throw new InvalidOperationException("corrupt recovery journal was accepted");
        }
        catch (BridgeUnavailableException)
        {
            // Expected: durable preimages are trusted only when the complete journal loads.
        }
    });
}

static void ProjectOperationStorePersistence()
{
    WithTemporaryDirectory(directory =>
    {
        var checkpointId = Guid.NewGuid();
        var renderId = Guid.NewGuid();
        var nativeExtensionId = Guid.NewGuid();
        var store = new ProjectOperationStore(directory);
        store.PutCheckpoint(new CheckpointReceiptDto(
            checkpointId,
            "checkpoint-request",
            "project-a",
            "scene-a",
            3,
            "target-a",
            "state-a",
            "checkpoint-profile",
            "verified",
            Path.Combine(directory, "project.ymmp"),
            null,
            "saved-hash",
            12,
            "state-a",
            "state-a",
            "driver-a",
            null));
        store.PutRender(new RenderTaskDto(
            renderId,
            "render-request",
            "project-a",
            "scene-a",
            3,
            "target-a",
            "state-a",
            checkpointId,
            "checkpoint-request",
            Path.Combine(directory, "project.ymmp"),
            "saved-hash",
            12,
            "render-profile",
            Path.Combine(directory, "output.mp4"),
            "deny",
            Path.Combine(directory, "render-input.ymmp"),
            new RenderOverwriteJournalDto(
                "prepared",
                false,
                null,
                null,
                $"{Path.Combine(directory, "output.mp4")}.takegraph-backup-{renderId:N}",
                $"{Path.Combine(directory, "output.mp4")}.takegraph-stale-{renderId:N}",
                Path.Combine(directory, $".takegraph-render-{renderId:N}"),
                Path.Combine(directory, $".takegraph-render-{renderId:N}", "candidate.mp4"),
                null,
                null),
            "queued",
            0,
            "queued",
            true,
            "state-a",
            null,
            null,
            null));
        store.PutNativeExtension(new NativeExtensionApplyResponseDto(
            nativeExtensionId,
            "native-extension-request",
            "project-a",
            "scene-a",
            "verified",
            "before-a",
            "after-a",
            "catalog-a",
            "driver-a",
            [new NativeExtensionRealizationDto(
                "image:background",
                Guid.Parse("30000000-0000-0000-0000-000000000003"),
                "image",
                "project-a",
                "background",
                1,
                0,
                0,
                30,
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                new Dictionary<string, string>(),
                [],
                "state-a",
                [])],
            true,
            null));

        var loaded = new ProjectOperationStore(directory);
        Assert(loaded.TryGetCheckpoint(checkpointId, out var checkpoint), "checkpoint was not reloaded");
        Assert(checkpoint?.RequestDigest == "checkpoint-request", "checkpoint binding changed");
        Assert(loaded.TryGetRender(renderId, out var render), "render task was not reloaded");
        Assert(render?.Status == "queued" && render.Cancellable, "render lifecycle changed");
        Assert(loaded.TryGetNativeExtension(nativeExtensionId, out var nativeExtension),
            "native-extension receipt was not reloaded");
        Assert(nativeExtension?.RequestDigest == "native-extension-request"
            && nativeExtension.Verified
            && nativeExtension.Realizations.Count == 1,
            "native-extension receipt binding changed");
        Assert(!Directory.EnumerateFiles(directory, "*.tmp-*", SearchOption.AllDirectories).Any(),
            "temporary project-operation record remained");
    });
}

static void ProjectInitializationStoreIsRequestBound()
{
    WithTemporaryDirectory(directory =>
    {
        var operationId = Guid.NewGuid();
        var receipt = ProjectInitializationReceiptForTests(operationId, directory, "request-a");
        var store = new ProjectOperationStore(directory);
        store.PutProjectInitialization(receipt);
        var loaded = new ProjectOperationStore(directory);
        Assert(loaded.TryGetProjectInitialization(operationId, out var persisted),
            "project initialization receipt was not reloaded");
        Assert(persisted?.RequestDigest == "request-a",
            "project initialization binding changed");
        try
        {
            store.PutProjectInitialization(receipt with { RequestDigest = "request-b" });
            throw new InvalidOperationException(
                "project initialization operation ID was rebound");
        }
        catch (BridgeConflictException)
        {
            // Expected: an operation ID has one immutable request binding.
        }
    });
}

static void ProjectInitializationPathClaimNeverOverwrites()
{
    WithTemporaryDirectory(directory =>
    {
        var temporary = Path.Combine(
            directory,
            $".takegraph-project-{Guid.NewGuid():N}.ymmp");
        var destination = Path.Combine(directory, "destination.ymmp");
        File.WriteAllText(temporary, "new project");
        File.WriteAllText(destination, "racing owner");
        var sha256 = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(temporary)));
        var bytes = checked((ulong)new FileInfo(temporary).Length);
        try
        {
            Ymm4Facade.ClaimPreparedProjectInitializationPath(
                temporary,
                destination,
                sha256,
                bytes);
            throw new InvalidOperationException(
                "project initialization overwrote a racing destination owner");
        }
        catch (IOException)
        {
            Assert(File.ReadAllText(destination) == "racing owner",
                "project initialization changed a racing destination file");
            Assert(File.ReadAllText(temporary) == "new project",
                "failed namespace claim lost the temporary project file");
        }
    });
}

static void ProjectInitializationPathClaimIsEvidenceBound()
{
    WithTemporaryDirectory(directory =>
    {
        var destination = Path.Combine(directory, "destination.ymmp");
        var mismatched = Path.Combine(
            directory,
            $".takegraph-project-{Guid.NewGuid():N}.ymmp");
        File.WriteAllText(mismatched, "new project");
        try
        {
            Ymm4Facade.ClaimPreparedProjectInitializationPath(
                mismatched,
                destination,
                new string('0', 64),
                checked((ulong)new FileInfo(mismatched).Length));
            throw new InvalidOperationException("mismatched prepared evidence was accepted");
        }
        catch (InvalidDataException)
        {
            Assert(!File.Exists(destination), "mismatched evidence created the destination");
            Assert(File.Exists(mismatched), "mismatched evidence consumed the temporary file");
        }

        var matching = Path.Combine(
            directory,
            $".takegraph-project-{Guid.NewGuid():N}.ymmp");
        File.WriteAllText(matching, "exact project");
        var sha256 = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(matching)));
        var bytes = checked((ulong)new FileInfo(matching).Length);
        Ymm4Facade.ClaimPreparedProjectInitializationPath(
            matching,
            destination,
            sha256,
            bytes);
        Assert(!File.Exists(matching), "successful claim retained the temporary file");
        Assert(Ymm4Facade.ProjectInitializationFileMatches(destination, sha256, bytes),
            "claimed destination did not preserve the prepared evidence");
    });
}

static void ProjectInitializationRecoveryEvidenceIsMonotonic()
{
    WithTemporaryDirectory(directory =>
    {
        var operationId = Guid.NewGuid();
        var destination = Path.Combine(directory, "project.ymmp");
        var temporary = Path.Combine(
            directory,
            $".takegraph-project-{Guid.NewGuid():N}.ymmp");
        var applying = ProjectInitializationReceiptForTests(operationId, directory, "request-a");
        var store = new ProjectOperationStore(directory);
        store.PutProjectInitialization(applying);
        Assert(store.ReadPendingProjectInitializations().Count == 1,
            "applying initialization was absent from the global write gate");
        try
        {
            store.PutProjectInitialization(applying with
            {
                PreparedTemporaryPath = temporary,
                PreparedFileSha256 = new string('a', 64),
                PreparedFileBytes = 42,
            });
            throw new InvalidOperationException("prepared evidence skipped its durable path-only stage");
        }
        catch (BridgeConflictException)
        {
            // Temp ownership must be durable before any file can be created.
        }
        var pathOnly = applying with { PreparedTemporaryPath = temporary };
        store.PutProjectInitialization(pathOnly);
        var complete = pathOnly with
        {
            PreparedFileSha256 = new string('a', 64),
            PreparedFileBytes = 42,
        };
        store.PutProjectInitialization(complete);
        var recovery = complete with
        {
            Status = "recovery_required",
            Error = "simulated interruption",
        };
        store.PutProjectInitialization(recovery);
        store.PutProjectInitialization(recovery with
        {
            Status = "applying",
            Error = null,
        });
        var after = SourceSnapshotForProjectInitializationTests(
            complete.PredictedProjectId,
            complete.SourceSceneId,
            destination,
            complete.PredictedFingerprint);
        var verified = complete with
        {
            Status = "verified",
            AfterSnapshot = after,
            FileSha256 = complete.PreparedFileSha256,
            FileBytes = complete.PreparedFileBytes,
        };
        store.PutProjectInitialization(verified);
        Assert(store.ReadPendingProjectInitializations().Count == 0,
            "verified initialization remained in the write gate");
        try
        {
            store.PutProjectInitialization(verified with { Status = "recovery_required", Error = "regress" });
            throw new InvalidOperationException("verified initialization was mutated");
        }
        catch (BridgeConflictException)
        {
            // Verified receipts are immutable historical certificates.
        }
    });
}

static void ProjectInitializationVerifiedReplayIsHistorical()
{
    WithTemporaryDirectory(directory =>
    {
        var receipt = ProjectInitializationReceiptForTests(
            Guid.NewGuid(),
            directory,
            "request-a");
        var temporary = Path.Combine(
            directory,
            $".takegraph-project-{Guid.NewGuid():N}.ymmp");
        var verified = receipt with
        {
            Status = "verified",
            PreparedTemporaryPath = temporary,
            PreparedFileSha256 = new string('a', 64),
            PreparedFileBytes = 42,
            AfterSnapshot = SourceSnapshotForProjectInitializationTests(
                receipt.PredictedProjectId,
                receipt.SourceSceneId,
                receipt.DestinationPath,
                receipt.PredictedFingerprint),
            FileSha256 = new string('a', 64),
            FileBytes = 42,
        };
        Assert(!File.Exists(receipt.DestinationPath), "test destination unexpectedly exists");
        var replayed = Ymm4Facade.ReplayProjectInitialization(verified);
        Assert(replayed.Status == "replayed", "historical receipt was not replayed");
        Assert(!File.Exists(receipt.DestinationPath), "receipt replay mutated the filesystem");
    });
}

static ProjectSnapshotDto SourceSnapshotForProjectInitializationTests(
    string projectId,
    string sceneId,
    string projectPath,
    string fingerprint) =>
    new(projectId, "project", projectPath, sceneId, 60, fingerprint, [], [], 0);

static ProjectInitializationReceiptDto ProjectInitializationReceiptForTests(
    Guid operationId,
    string directory,
    string requestDigest)
{
    var destination = Path.Combine(directory, "project.ymmp");
    return new ProjectInitializationReceiptDto(
        operationId,
        requestDigest,
        "applying",
        "driver-profile",
        "instance-a",
        "source-project",
        "scene-a",
        "before-fingerprint",
        destination,
        "path-digest",
        "predicted-project",
        "predicted-fingerprint",
        null,
        null,
        null,
        null,
        null,
        null,
        null);
}

static void CorruptProjectOperationStoreFailsClosed()
{
    WithTemporaryDirectory(directory =>
    {
        var checkpoints = Path.Combine(directory, "checkpoints");
        Directory.CreateDirectory(checkpoints);
        File.WriteAllText(Path.Combine(checkpoints, $"{Guid.NewGuid():N}.json"), "{ bad json");
        var store = new ProjectOperationStore(directory);
        try
        {
            _ = store.TryGetCheckpoint(Guid.NewGuid(), out _);
            throw new InvalidOperationException("corrupt project-operation record was accepted");
        }
        catch (BridgeUnavailableException)
        {
            // Expected: save/render state is fail-closed after a corrupt durable record.
        }
    });
}

static void Mp4MediaProbeReadsAuthoritativeFields()
{
    WithTemporaryDirectory(directory =>
    {
        var movieHeader = new byte[20];
        BinaryPrimitives.WriteUInt32BigEndian(movieHeader.AsSpan(12, 4), 1_000);
        BinaryPrimitives.WriteUInt32BigEndian(movieHeader.AsSpan(16, 4), 2_500);

        var videoTrack = Mp4TestTrack("vide", 1_920, 1_080, versionOne: false, "avc1");
        var audioTrack = Mp4TestTrack("soun", 0, 0, versionOne: false, "mp4a");
        var bytes = Concat(
            Mp4Box("ftyp", Encoding.ASCII.GetBytes("isom")),
            Mp4Box("moov", Concat(Mp4Box("mvhd", movieHeader), videoTrack, audioTrack)));
        var path = Path.Combine(directory, "sample.mp4");
        File.WriteAllBytes(path, bytes);

        var receipt = Mp4MediaProbe.Probe(path);
        Assert(receipt.ByteLength == (ulong)bytes.Length, "MP4 byte length was wrong");
        Assert(receipt.DurationMillis == 2_500, "MP4 duration was wrong");
        Assert(receipt.Width == 1_920 && receipt.Height == 1_080, "MP4 dimensions were wrong");
        Assert(receipt.VideoStreams == 1 && receipt.AudioStreams == 1, "MP4 streams were wrong");
        Assert(receipt.VideoCodec == "h264" && receipt.AudioCodec == "aac_lc",
            "MP4 codecs were wrong");
        Assert(receipt.AudioSampleRate == 48_000, "MP4 audio sample rate was wrong");
        Assert(receipt.PixelFormat == "yuv420p", "MP4 pixel format was wrong");
        Assert(receipt.FpsNumerator == 60 && receipt.FpsDenominator == 1,
            "MP4 frame rate was wrong");
        Assert(receipt.Sha256.Length == 64 && receipt.ProbeDigest.Length == 64,
            "MP4 evidence digest was missing");

        var extraAudioBytes = Concat(
            Mp4Box("ftyp", Encoding.ASCII.GetBytes("isom")),
            Mp4Box(
                "moov",
                Concat(Mp4Box("mvhd", movieHeader), videoTrack, audioTrack, audioTrack)));
        var extraAudioPath = Path.Combine(directory, "extra-audio.mp4");
        File.WriteAllBytes(extraAudioPath, extraAudioBytes);
        ExpectInvalidData(
            () => Mp4MediaProbe.Probe(extraAudioPath),
            "MP4 probe accepted an extra audio track");

        var misleadingAudioDimensions = Mp4TestTrack(
            "soun", 1_920, 1_080, versionOne: false, "mp4a");
        var wrongVideoDimensions = Mp4TestTrack(
            "vide", 640, 360, versionOne: false, "avc1");
        var dimensionPath = Path.Combine(directory, "track-dimensions.mp4");
        File.WriteAllBytes(
            dimensionPath,
            Concat(
                Mp4Box("ftyp", Encoding.ASCII.GetBytes("isom")),
                Mp4Box(
                    "moov",
                    Concat(
                        Mp4Box("mvhd", movieHeader),
                        wrongVideoDimensions,
                        misleadingAudioDimensions))));
        var dimensionReceipt = Mp4MediaProbe.Probe(dimensionPath);
        Assert(dimensionReceipt.Width == 640 && dimensionReceipt.Height == 360,
            "MP4 dimensions were not bound to the video track");

        var versionOneVideoTrack = Mp4TestTrack("vide", 1_920, 1_080, versionOne: true, "avc3");
        var versionOneAudioTrack = Mp4TestTrack("soun", 0, 0, versionOne: true, "mp4a");
        var versionOneBytes = Concat(
            Mp4Box("ftyp", Encoding.ASCII.GetBytes("isom")),
            Mp4Box(
                "moov",
                Concat(
                    Mp4Box("mvhd", movieHeader),
                    versionOneVideoTrack,
                    versionOneAudioTrack)));
        var versionOnePath = Path.Combine(directory, "sample-v1.mp4");
        File.WriteAllBytes(versionOnePath, versionOneBytes);

        var versionOneReceipt = Mp4MediaProbe.Probe(versionOnePath);
        Assert(versionOneReceipt.Width == 1_920 && versionOneReceipt.Height == 1_080,
            "version 1 tkhd dimensions were read from the wrong offset");
        Assert(versionOneReceipt.Sha256 ==
            "086499a4a586bd4c16007bef212f546bcbf1637156e42cad213db6bec4540db5",
            $"version 1 MP4 cross-runtime SHA-256 changed: {versionOneReceipt.Sha256}");
        Assert(versionOneReceipt.ProbeDigest ==
            "daf321f48e76f20b97da73a8f176d657afc08380f81ca80b94c250c0cb6eb0d4",
            $"version 1 MP4 cross-runtime probe digest changed: {versionOneReceipt.ProbeDigest}");
    });
}

static void Mp4CodecEvidenceFailsClosed()
{
    WithTemporaryDirectory(directory =>
    {
        void Reject(string name, string[] videoEntries, string[] audioEntries)
        {
            var movieHeader = new byte[20];
            BinaryPrimitives.WriteUInt32BigEndian(movieHeader.AsSpan(12, 4), 1_000);
            BinaryPrimitives.WriteUInt32BigEndian(movieHeader.AsSpan(16, 4), 2_500);
            var bytes = Concat(
                Mp4Box("ftyp", Encoding.ASCII.GetBytes("isom")),
                Mp4Box(
                    "moov",
                    Concat(
                        Mp4Box("mvhd", movieHeader),
                        Mp4TestTrack("vide", 1_920, 1_080, false, videoEntries),
                        Mp4TestTrack("soun", 0, 0, false, audioEntries))));
            var path = Path.Combine(directory, $"{name}.mp4");
            File.WriteAllBytes(path, bytes);
            ExpectInvalidData(
                () => Mp4MediaProbe.Probe(path),
                $"MP4 probe accepted unsupported evidence: {name}");
        }

        Reject("hevc", ["hev1"], ["mp4a"]);
        Reject("opus", ["avc1"], ["Opus"]);
        Reject("missing-stsd-entry", [], ["mp4a"]);
        Reject("multiple-video-codecs", ["avc1", "hev1"], ["mp4a"]);
        Reject("aac-he", ["avc1"], ["mp4a-he"]);
        Reject("aac-rate-mismatch", ["avc1"], ["mp4a-44-entry"]);
    });
}

static void RenderBindingLeaseBlocksDrift()
{
    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "writer-settings.json");
        File.WriteAllText(path, "bound-settings", Encoding.UTF8);
        var bytes = File.ReadAllBytes(path);
        var binding = new RenderRuntimeBinding(
            "writer",
            "writer",
            "video",
            "resolved-video",
            "audio",
            "h264",
            "aac_lc",
            "yuv420p",
            "mp4",
            1920,
            1080,
            60,
            48000,
            "manifest",
            "driver",
            [new RenderBindingFile(
                "writer-settings",
                path,
                Convert.ToHexStringLower(SHA256.HashData(bytes)),
                checked((ulong)bytes.Length))]);
        using (var lease = binding.OpenVerifiedReadLease())
        {
            lease.Verify();
            try
            {
                using var write = new FileStream(path, FileMode.Open, FileAccess.Write, FileShare.Read);
                throw new InvalidOperationException("render binding lease allowed a concurrent writer");
            }
            catch (IOException)
            {
                // Expected: child-visible setting/binary bytes are immutable during encoding.
            }
        }
        File.WriteAllText(path, "drifted-settings", Encoding.UTF8);
        ExpectRenderDrift(
            () => binding.OpenVerifiedReadLease().Dispose(),
            "render binding accepted changed bytes after staging");
    });
}

static void AuthoritativeRenderRequiresExhaustiveSourceBinding()
{
    try
    {
        Ymm4Facade.RequireAuthoritativeRenderSourceBinding();
        throw new InvalidOperationException(
            "Authoritative render unexpectedly accepted an incomplete dependency inventory");
    }
    catch (BridgeUnavailableException error)
    {
        Assert(
            error.Message == Ymm4Facade.AuthoritativeRenderSourceBindingError,
            "Authoritative render did not report the exact fail-closed dependency boundary");
        Assert(
            error.Message.Contains("exhaustive render dependency manifest", StringComparison.Ordinal),
            "Authoritative render did not identify the missing dependency proof");
        Assert(
            error.Message.Contains("SHA-256-bound and read-leased", StringComparison.Ordinal),
            "Authoritative render did not identify the required source evidence");
    }
}

static void ExternalMp4EvidenceWhenRequested()
{
    var path = Environment.GetEnvironmentVariable("TAKEGRAPH_TEST_MP4_PATH");
    if (string.IsNullOrWhiteSpace(path))
    {
        return;
    }
    var receipt = Mp4MediaProbe.Probe(path);
    Assert(receipt.VideoCodec == "h264", "external MP4 video codec differs");
    Assert(receipt.AudioCodec == "aac_lc", "external MP4 audio codec differs");
    Assert(receipt.AudioSampleRate == 48_000, "external MP4 audio sample rate differs");
    Assert(receipt.PixelFormat == "yuv420p", "external MP4 pixel format differs");
    Assert(receipt.FpsNumerator == 60 && receipt.FpsDenominator == 1,
        "external MP4 frame rate differs");
}

static byte[] Mp4Box(string kind, byte[] payload)
{
    var bytes = new byte[checked(payload.Length + 8)];
    BinaryPrimitives.WriteUInt32BigEndian(bytes.AsSpan(0, 4), checked((uint)bytes.Length));
    Encoding.ASCII.GetBytes(kind).CopyTo(bytes, 4);
    payload.CopyTo(bytes, 8);
    return bytes;
}

static byte[] Mp4SampleDescription(params string[] sampleEntries)
{
    var entries = sampleEntries.Select(Mp4SampleEntry).ToArray();
    var header = new byte[8];
    BinaryPrimitives.WriteUInt32BigEndian(header.AsSpan(4, 4), checked((uint)entries.Length));
    return Mp4Box("stsd", Concat([header, .. entries]));
}

static byte[] Mp4SampleEntry(string sampleEntry)
{
    byte[] entryPayload;
    if (sampleEntry is "avc1" or "avc3")
    {
        var avcConfig = new byte[]
        {
            1, 66, 0, 30, 0xff, 0xe1,
            0, 5, 0x67, 0x42, 0, 0x1e, 0x80,
            1, 0, 1, 0x68,
        };
        entryPayload = Concat(new byte[78], Mp4Box("avcC", avcConfig));
    }
    else if (sampleEntry is "mp4a" or "mp4a-he" or "mp4a-44-entry")
    {
        var audioSpecificConfig = Mp4Descriptor(
            0x05,
            sampleEntry == "mp4a-he" ? [0x29, 0x90] : [0x11, 0x90]);
        var decoderConfig = Mp4Descriptor(
            0x04,
            Concat(
                new byte[]
                {
                    0x40, 0x15,
                    0, 0, 0,
                    0, 0, 0, 0,
                    0, 0, 0, 0,
                },
                audioSpecificConfig));
        var esDescriptor = Mp4Descriptor(
            0x03,
            Concat([0, 1, 0], decoderConfig, Mp4Descriptor(0x06, [2])));
        var audioEntry = new byte[28];
        BinaryPrimitives.WriteUInt32BigEndian(
            audioEntry.AsSpan(24, 4),
            (sampleEntry == "mp4a-44-entry" ? 44_100u : 48_000u) << 16);
        entryPayload = Concat(audioEntry, Mp4Box("esds", Concat(new byte[4], esDescriptor)));
    }
    else
    {
        entryPayload = [];
    }
    var type = sampleEntry.StartsWith("mp4a-", StringComparison.Ordinal) ? "mp4a" : sampleEntry;
    return Mp4Box(type, entryPayload);
}

static byte[] Mp4TestTrack(
    string handler,
    uint width,
    uint height,
    bool versionOne,
    params string[] sampleEntries)
{
    var trackHeader = new byte[versionOne ? 96 : 84];
    trackHeader[0] = versionOne ? (byte)1 : (byte)0;
    var dimensionOffset = versionOne ? 88 : 76;
    BinaryPrimitives.WriteUInt32BigEndian(trackHeader.AsSpan(dimensionOffset, 4), width << 16);
    BinaryPrimitives.WriteUInt32BigEndian(trackHeader.AsSpan(dimensionOffset + 4, 4), height << 16);
    var handlerData = new byte[12];
    Encoding.ASCII.GetBytes(handler).CopyTo(handlerData, 8);
    var mediaHeader = new byte[24];
    BinaryPrimitives.WriteUInt32BigEndian(mediaHeader.AsSpan(12, 4), handler == "vide" ? 60_000u : 48_000u);
    var timeToSample = new byte[16];
    BinaryPrimitives.WriteUInt32BigEndian(timeToSample.AsSpan(4, 4), 1);
    BinaryPrimitives.WriteUInt32BigEndian(timeToSample.AsSpan(8, 4), handler == "vide" ? 150u : 12_000u);
    BinaryPrimitives.WriteUInt32BigEndian(timeToSample.AsSpan(12, 4), handler == "vide" ? 1_000u : 1u);
    var sampleTable = Concat(Mp4Box("stts", timeToSample), Mp4SampleDescription(sampleEntries));
    var media = Concat(
        Mp4Box("mdhd", mediaHeader),
        Mp4Box("hdlr", handlerData),
        Mp4Box("minf", Mp4Box("stbl", sampleTable)));
    return Mp4Box("trak", Concat(Mp4Box("tkhd", trackHeader), Mp4Box("mdia", media)));
}

static byte[] Mp4Descriptor(byte tag, byte[] payload)
{
    Assert(payload.Length < 128, "test descriptor is unexpectedly large");
    return Concat([tag, checked((byte)payload.Length)], payload);
}

static byte[] Concat(params byte[][] values)
{
    var result = new byte[values.Sum(value => value.Length)];
    var offset = 0;
    foreach (var value in values)
    {
        value.CopyTo(result, offset);
        offset += value.Length;
    }
    return result;
}

static void QueuedRenderCancellationIsRegistered()
{
    using var control = new RenderExecutionControl();
    var cancellationPersisted = false;
    Assert(control.TryCancel(() =>
    {
        cancellationPersisted = true;
        return true;
    }), "queued cancellation was not accepted");
    Assert(cancellationPersisted, "queued cancellation was not persisted under the phase lock");
    Assert(control.Token.IsCancellationRequested, "queued cancellation did not signal the worker");
    Assert(!control.TryAdvance(() => throw new InvalidOperationException("cancelled task advanced")),
        "cancelled queued task advanced to running");
}

static void RenderOverwriteWalRestoresOriginal()
{
    WithTemporaryDirectory(directory =>
    {
        var output = Path.Combine(directory, "final.mp4");
        var backup = Path.Combine(directory, "final.mp4.backup");
        var quarantine = Path.Combine(directory, "final.mp4.stale");
        var original = Encoding.UTF8.GetBytes("approved original");
        var stale = Encoding.UTF8.GetBytes("partial replacement");
        File.WriteAllBytes(backup, original);
        File.WriteAllBytes(output, stale);
        var journal = new RenderOverwriteJournalDto(
            "candidate_published",
            true,
            Convert.ToHexStringLower(SHA256.HashData(original)),
            (ulong)original.Length,
            backup,
            quarantine,
            directory,
            Path.Combine(directory, "candidate.mp4"),
            Convert.ToHexStringLower(SHA256.HashData(stale)),
            (ulong)stale.Length);

        var recovered = Ymm4Facade.RecoverRenderOutput(journal, output);

        Assert(File.ReadAllBytes(output).SequenceEqual(original),
            "overwrite WAL did not restore the exact original bytes");
        Assert(File.ReadAllBytes(quarantine).SequenceEqual(stale),
            "overwrite WAL did not quarantine the stale replacement");
        Assert(!File.Exists(backup), "overwrite WAL left a second movable original");
        Assert(recovered.State == "quarantined_and_restored",
            "overwrite WAL did not persist the recovery outcome");
    });
}

static void RenderSourceLeaseBlocksDrift()
{
    WithTemporaryDirectory(directory =>
    {
        var path = Path.Combine(directory, "checkpoint.ymmp");
        var bytes = Encoding.UTF8.GetBytes("verified checkpoint");
        File.WriteAllBytes(path, bytes);
        var sha256 = Convert.ToHexStringLower(SHA256.HashData(bytes));
        using var lease = Ymm4Facade.OpenVerifiedReadLease(
            path,
            sha256,
            (ulong)bytes.Length,
            "checkpoint drifted");
        try
        {
            using var writer = new FileStream(
                path,
                FileMode.Open,
                FileAccess.Write,
                FileShare.ReadWrite);
            throw new InvalidOperationException("render source lease allowed concurrent mutation");
        }
        catch (IOException)
        {
            // Expected on Windows: the verified read lease denies writes/deletes.
        }
    });
}

static void FaultInjectionIsScoped()
{
    const string variable = "TAKEGRAPH_YMM4_FAULT_POINT";
    var original = Environment.GetEnvironmentVariable(variable);
    try
    {
        Environment.SetEnvironmentVariable(variable, "after_journal_before_mutation,during_save");
        BridgeFaultInjection.ThrowIf("unconfigured_point");
        try
        {
            BridgeFaultInjection.ThrowIf("after_journal_before_mutation");
            throw new InvalidOperationException("configured fault point did not throw");
        }
        catch (BridgeSimulatedCrashException)
        {
            // Expected: integration tests can stop between durable states without killing the test host.
        }
    }
    finally
    {
        Environment.SetEnvironmentVariable(variable, original);
    }
}

static void PreservationSerializationFailsClosed()
{
    var first = Ymm4Facade.CanonicalizeSerializedYmmValue(
        typeof(RequiredItemFixture),
        "{\"z\":2,\"a\":1}");
    var second = Ymm4Facade.CanonicalizeSerializedYmmValue(
        typeof(RequiredItemFixture),
        "{\"a\":1,\"z\":2}");
    Assert(first == second, "preservation JSON property order changed its canonical value");
    Assert(first.StartsWith("takegraph-ymm4-value-v2|", StringComparison.Ordinal),
        "preservation value version/domain was missing");
    ExpectBridgeUnavailable(
        () => Ymm4Facade.CanonicalizeSerializedYmmValue(
            typeof(RequiredItemFixture),
            "not-json"),
        "invalid preservation JSON did not fail closed");
    ExpectBridgeUnavailable(
        () => Ymm4Facade.ComputeExtensionPreservedStateDigest(
            new ThrowingPreservationItemFixture(),
            excludeEffects: false),
        "throwing preservation getter was silently omitted");
    ExpectBridgeUnavailable(
        () => Ymm4Facade.ComputePreservedNativeVoiceStateDigest(
            new ThrowingPreservationItemFixture()),
        "throwing native-voice preservation getter was silently omitted");
}

static void TemplateTraversalAndIdentityAreStable()
{
    var firstTemplate = new YukkuriMovieMaker.Settings.ItemTemplate
    {
        Name = "Opening",
        Path = ["User", "Intro"],
        Group = "group-a",
        SceneId = "scene-a",
        Width = 1920,
        Height = 1080,
        FPS = 30,
        Hz = 48000,
    };
    var sameVisibleBindingButDifferentConfig = new YukkuriMovieMaker.Settings.ItemTemplate
    {
        Name = "Opening",
        Path = ["User", "Intro"],
        Group = "group-b",
        SceneId = "scene-a",
        Width = 1280,
        Height = 720,
        FPS = 30,
        Hz = 48000,
    };
    var root = new TemplateMenuNode();
    var nested = new TemplateMenuNode { Template = firstTemplate };
    root.Items.Add(nested);
    nested.Items.Add(root);
    var templates = new HashSet<object>(ReferenceEqualityComparer.Instance);
    Ymm4Facade.CollectTemplates(root, templates);
    Assert(templates.Count == 1 && templates.Contains(firstTemplate),
        "cyclic template menu traversal was not bounded/exact");

    var descriptor = Ymm4Facade.DescribeTemplate(firstTemplate);
    var repeated = Ymm4Facade.DescribeTemplate(firstTemplate);
    var other = Ymm4Facade.DescribeTemplate(sameVisibleBindingButDifferentConfig);
    Assert(descriptor.DescriptorId == repeated.DescriptorId
        && descriptor.ConfigDigest == repeated.ConfigDigest
        && descriptor.SchemaDigest == repeated.SchemaDigest,
        "template descriptor identity was not stable");
    Assert(descriptor.DescriptorId != other.DescriptorId,
        "template group was not included in stable descriptor identity");
    Assert(descriptor.ConfigDigest != other.ConfigDigest,
        "template configuration drift did not change the exact binding digest");
}

static void RequiredReflectionFailsClosed()
{
    var fallback = new RequiredItemFixture { Frame = 12, Layer = 3, Length = 30 };
    Assert(Ymm4Facade.GetRequiredInt(new object(), fallback, "Frame") == 12,
        "required reflection did not use an explicitly available wrapper field");
    ExpectBridgeUnavailable(
        () => Ymm4Facade.GetRequiredInt(new ThrowingRequiredItemFixture(), fallback, "Frame"),
        "throwing required getter silently fell back to a different value");
    ExpectBridgeUnavailable(
        () => Ymm4Facade.GetRequiredInt(new InvalidRequiredItemFixture(), fallback, "Frame"),
        "non-integral required timeline value defaulted or coerced");
    ExpectBridgeUnavailable(
        () => Ymm4Facade.GetRequiredInt(new object(), new object(), "Frame"),
        "missing required timeline value defaulted to zero");
}

static void LockedMutationTargetsMapToConflict()
{
    Ymm4Facade.EnsureUnlockedForMutation(
        new RequiredItemFixture { IsLocked = false },
        "unlocked",
        "fingerprint-a");
    try
    {
        Ymm4Facade.EnsureUnlockedForMutation(
            new RequiredItemFixture { IsLocked = true },
            "locked",
            "fingerprint-a");
        throw new InvalidOperationException("locked mutation target was accepted");
    }
    catch (BridgeConflictException error)
    {
        var mapped = BridgeHost.MapError(error);
        Assert(mapped.StatusCode == 409, "locked mutation conflict did not map to HTTP 409");
        Assert(mapped.Error.ActualFingerprint == "fingerprint-a",
            "locked mutation conflict lost the current fingerprint");
    }
    ExpectBridgeUnavailable(
        () => Ymm4Facade.EnsureUnlockedForMutation(
            new object(),
            "unknown-lock-state",
            "fingerprint-a"),
        "missing required IsLocked state was treated as unlocked");
}

static void StrictRequestJsonMapsTo400()
{
    var valid = BridgeHost.DeserializeRequest<StrictRequestFixture>(
        "{\"protocolVersion\":1,\"projectId\":\"project-a\"}");
    Assert(valid.ProtocolVersion == 1 && valid.ProjectId == "project-a",
        "valid exact-case request JSON was rejected");

    foreach (var invalid in new[]
             {
                 "{\"ProtocolVersion\":1,\"projectId\":\"project-a\"}",
                 "{\"protocolVersion\":1,\"projectId\":\"project-a\",\"unknown\":true}",
                 "{\"protocolVersion\":1,\"protocolVersion\":2,\"projectId\":\"project-a\"}",
                 "{\"protocolVersion\":1}",
                 "{\"protocolVersion\":1,",
             })
    {
        var error = CaptureBridgeValidation(
            () => BridgeHost.DeserializeRequest<StrictRequestFixture>(invalid));
        Assert(BridgeHost.MapError(error).StatusCode == 400,
            $"invalid request JSON did not map to HTTP 400: {invalid}");
    }
    var nestedDuplicate = CaptureBridgeValidation(
        () => BridgeHost.DeserializeRequest<StrictNestedRequestFixture>(
            "{\"protocolVersion\":1,\"payload\":{\"key\":1,\"key\":2}}"));
    Assert(BridgeHost.MapError(nestedDuplicate).StatusCode == 400,
        "nested duplicate request property did not map to HTTP 400");
}

static BridgeValidationException CaptureBridgeValidation(Action action)
{
    try
    {
        action();
        throw new InvalidOperationException("expected request validation failure was not raised");
    }
    catch (BridgeValidationException error)
    {
        return error;
    }
}

static void NativeExtensionInnerJsonIsExact()
{
    using var exact = JsonDocument.Parse("{\"type\":\"remove\"}");
    Ymm4Facade.RequireExactJsonProperties(exact.RootElement, "type");

    foreach (var json in new[]
             {
                 "{\"type\":\"remove\",\"ignored\":true}",
                 "{\"type\":\"remove\",\"type\":\"upsert\"}",
                 "{}",
             })
    {
        using var document = JsonDocument.Parse(json);
        _ = CaptureBridgeValidation(
            () => Ymm4Facade.RequireExactJsonProperties(document.RootElement, "type"));
    }
}

static void SubmittedHttpResponsesAreNotRewritten()
{
    var submitted = new InvalidOperationException(
        "This operation cannot be performed after the response has been submitted.");
    Assert(BridgeHost.IsResponseAlreadySubmitted(submitted),
        "HttpListener already-submitted error was treated as a writable 500");
    Assert(
        BridgeHost.IsResponseAlreadySubmitted(
            new InvalidOperationException("outer", submitted)),
        "wrapped already-submitted error was treated as a writable 500");
    Assert(BridgeHost.IsResponseAlreadySubmitted(new ObjectDisposedException("response")),
        "disposed listener response was treated as a writable 500");
    Assert(BridgeHost.IsResponseAlreadySubmitted(new IOException("connection reset")),
        "dropped client write was treated as a writable 500");
    Assert(
        !BridgeHost.IsResponseAlreadySubmitted(
            new InvalidOperationException("YMM4 preview seek did not settle at frame 1080")),
        "unrelated invalid-operation errors were swallowed as already-submitted");
}

static void PreviewSettleAcceptsRequestedFrame()
{
    Assert(Ymm4Facade.PreviewFrameSettled(1080, 1080),
        "exact preview frame was rejected as unsettled");
    Assert(Ymm4Facade.PreviewFrameSettled(1079, 1080),
        "one-frame preview lag was rejected as unsettled");
    Assert(!Ymm4Facade.PreviewFrameSettled(60, 1080),
        "stale playhead was treated as settled");
    Assert(!Ymm4Facade.PreviewFrameSettled(null, 1080),
        "unreadable playhead was treated as settled");
}

static void SceneCaptureFailureIsCombined()
{
    var message = Ymm4Facade.DescribeSceneCaptureFailure(
        new InvalidOperationException("encoder failed"),
        new InvalidOperationException("seek failed"),
        new InvalidOperationException("read-back failed"),
        transientStateRestored: false,
        dirtyStateRestored: false);
    Assert(message.Contains("capture failed: encoder failed", StringComparison.Ordinal)
        && message.Contains("playhead restore failed: seek failed", StringComparison.Ordinal)
        && message.Contains("restoration read-back failed: read-back failed", StringComparison.Ordinal)
        && message.Contains("preview position or selection restoration was not verified", StringComparison.Ordinal)
        && message.Contains("project dirty-state restoration was not verified", StringComparison.Ordinal),
        "scene-capture failure did not retain all recovery diagnostics");
}

static void PortableDescriptorDigestsBindConfiguration()
{
    var first = new TargetDescriptorDto(
        "template-a",
        "template",
        "Opening",
        new string('1', 64),
        new string('2', 64),
        true,
        false,
        new SortedDictionary<string, string>(StringComparer.Ordinal)
        {
            ["group"] = "group-a",
            ["itemTypes"] = "YukkuriMovieMaker.Project.Items.TextItem",
        });
    var changed = first with { ConfigDigest = new string('3', 64) };
    var firstDigest = Ymm4Facade.ComputePortableDescriptorDigest(first);
    Assert(firstDigest.StartsWith("sha256:", StringComparison.Ordinal)
        && firstDigest.Length == 71,
        "portable descriptor digest was malformed");
    Assert(firstDigest != Ymm4Facade.ComputePortableDescriptorDigest(changed),
        "portable descriptor digest ignored target configuration drift");

    var catalog = new DescriptorCatalogDto(
        2,
        "project-a",
        "scene-a",
        new string('4', 64),
        new string('5', 64),
        [first]);
    var changedCatalog = catalog with { Descriptors = [changed] };
    Assert(Ymm4Facade.ComputePortableDescriptorCatalogDigest(catalog)
        != Ymm4Facade.ComputePortableDescriptorCatalogDigest(changedCatalog),
        "portable descriptor catalog digest ignored descriptor configuration drift");
}

static void NativeExtensionItemWitnessRejectsCollateralAdditions()
{
    var unchanged = new object();
    var approved = new object();
    var collateral = new object();
    Assert(Ymm4Facade.ExactNativeExtensionItemSetMatches(
            [unchanged],
            [approved],
            [unchanged, approved]),
        "exact native-extension item witness rejected the approved set");
    Assert(!Ymm4Facade.ExactNativeExtensionItemSetMatches(
            [unchanged],
            [approved],
            [unchanged, approved, collateral]),
        "native-extension witness accepted a collateral unmanaged addition");
    Assert(!Ymm4Facade.ExactNativeExtensionItemSetMatches(
            [unchanged],
            [approved],
            [approved]),
        "native-extension witness accepted removal of a pre-existing item");
}

static RecoveryJournalEntryDto ValidRecoveryEntry(Guid operationId)
{
    const string preimageJson = "{}";
    var realizationId = Guid.Parse("10000000-0000-0000-0000-000000000001");
    var now = DateTimeOffset.UtcNow;
    return new RecoveryJournalEntryDto(
        1,
        operationId,
        Sha256("request-a"),
        "project-a",
        "scene-a",
        Sha256("expected-a"),
        Sha256("before-a"),
        "applying",
        "native_voice_mutation",
        ["utt-a"],
        [realizationId],
        [new ManagedItemDto(
            "utt-a",
            1,
            "voice",
            0,
            1,
            30,
            "text",
            null,
            null,
            "character",
            realizationId.ToString("D"))],
        new Dictionary<Guid, string>(),
        [new RecoveryItemDto(
            "YukkuriMovieMaker.Project.Items.VoiceItem, YukkuriMovieMaker",
            preimageJson,
            Sha256(preimageJson))],
        now,
        now,
        null,
        null);
}

static RecoveryJournalEntryDto ValidRecoveryEntryForDriver(
    Guid operationId,
    string driver)
{
    var entry = ValidRecoveryEntry(operationId);
    return driver switch
    {
        "native_voice_mutation" => entry,
        "native_voice_create" => entry with
        {
            Driver = driver,
            BeforeItems = [],
        },
        "portable_pair" => entry with
        {
            Driver = driver,
            RealizationIds = [],
            ExpectedItems =
            [
                entry.ExpectedItems[0] with
                {
                    Kind = "audio",
                    ArtifactHash = Sha256("portable-audio"),
                    RealizationId = null,
                    Speaker = null,
                },
                entry.ExpectedItems[0] with
                {
                    Kind = "caption",
                    ArtifactHash = null,
                    RealizationId = null,
                    Speaker = null,
                },
            ],
        },
        "native_extension" => entry with
        {
            Driver = driver,
            ExpectedItems = [],
            PreservedStateDigests = new Dictionary<Guid, string>(),
        },
        "timeline_edit_managed_cue_mixed" => entry with
        {
            Driver = driver,
            EntityIds = ["utt-a", "portable-a"],
            BeforeItems = [],
            ExpectedItems =
            [
                entry.ExpectedItems[0],
                entry.ExpectedItems[0] with
                {
                    EntityId = "portable-a",
                    Kind = "audio",
                    ArtifactHash = Sha256("portable-audio"),
                    RealizationId = null,
                },
                entry.ExpectedItems[0] with
                {
                    EntityId = "portable-a",
                    Kind = "caption",
                    ArtifactHash = Sha256("portable-audio"),
                    RealizationId = null,
                },
            ],
        },
        _ => throw new ArgumentOutOfRangeException(nameof(driver), driver, null),
    };
}

static OperationReceiptDto VerifiedReceipt(
    RecoveryJournalEntryDto entry,
    string afterFingerprint,
    IReadOnlyList<ManagedItemDto> verifiedItems) =>
    new(
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

static OperationReceiptDto ValidReceipt(Guid operationId, string status)
{
    var fingerprint = Sha256("receipt-before");
    return new OperationReceiptDto(
        operationId,
        Sha256("receipt-request"),
        "project-a",
        "scene-a",
        fingerprint,
        status,
        fingerprint,
        fingerprint,
        [],
        status == "verified",
        null);
}

static string Sha256(string value)
{
    return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(value)));
}

static void ExpectBridgeUnavailable(Action action, string failure)
{
    try
    {
        action();
        throw new InvalidOperationException(failure);
    }
    catch (BridgeUnavailableException)
    {
        // Expected.
    }
}

static void ExpectBridgeConflict(Action action, string failure)
{
    try
    {
        action();
        throw new InvalidOperationException(failure);
    }
    catch (BridgeConflictException)
    {
        // Expected.
    }
}

static void ExpectBridgeValidation(Action action, string failure)
{
    try
    {
        action();
        throw new InvalidOperationException(failure);
    }
    catch (BridgeValidationException)
    {
        // Expected.
    }
}

static void ExpectInvalidData(Action action, string failure)
{
    try
    {
        action();
        throw new InvalidOperationException(failure);
    }
    catch (InvalidDataException)
    {
        // Expected.
    }
}

static void ExpectRenderDrift(Action action, string failure)
{
    try
    {
        action();
        throw new InvalidOperationException(failure);
    }
    catch (RenderSourceDriftException)
    {
        // Expected.
    }
}

static void ExpectWriteRejected(Action action, string failure)
{
    try
    {
        action();
        throw new InvalidOperationException(failure);
    }
    catch (Exception error) when (error is IOException or UnauthorizedAccessException)
    {
        // Expected from either the read-only CAS attribute or the active lease.
    }
}

static void WithFaultPoint(string point, Action action)
{
    const string variable = "TAKEGRAPH_YMM4_FAULT_POINT";
    var original = Environment.GetEnvironmentVariable(variable);
    try
    {
        Environment.SetEnvironmentVariable(variable, point);
        action();
    }
    finally
    {
        Environment.SetEnvironmentVariable(variable, original);
    }
}

static void WithTemporaryDirectory(Action<string> action)
{
    var directory = Path.Combine(Path.GetTempPath(), $"takegraph-bridge-test-{Guid.NewGuid():N}");
    Directory.CreateDirectory(directory);
    try
    {
        action(directory);
    }
    finally
    {
        Directory.Delete(directory, recursive: true);
    }
}

static void Assert(bool condition, string message)
{
    if (!condition)
    {
        throw new InvalidOperationException(message);
    }
}
