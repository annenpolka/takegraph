using System.Security.Cryptography;
using System.Text;
using System.Globalization;

namespace TakeGraph.Ymm4Bridge;

internal static class ApplyRequestDigest
{
    internal static void ValidateCrossRuntimeGolden()
    {
        var portableRequest = new ApplyRequestDto(
            2,
            Guid.Empty,
            string.Empty,
            "project-a",
            "scene-a",
            "fingerprint-a",
            [
                new ManagedUtteranceDto(
                    "utt-01",
                    4,
                    "春日部つむぎ",
                    "ここから第二形態です",
                    @"C:\artifacts\take.wav",
                    "abc",
                    120,
                    60,
                    20,
                    21),
            ]);
        AssertGolden(
            Compute(portableRequest),
            "0330383be4127ab508fc5ebff79d0586fffc8d542edf0d245a92fdb547318c15");
        var portableSeparateTextRequest = portableRequest with
        {
            Utterances =
            [
                portableRequest.Utterances[0] with
                {
                    SpokenText = "ここからだいにけいたいです",
                },
            ],
        };
        AssertGolden(
            Compute(portableSeparateTextRequest),
            "4b6d857b78891e00d753e835eab9304dfdc50de5d82f4cff7dc721fa20ca3895");

        var nativeRequest = new NativeVoiceApplyRequestDto(
            2,
            Guid.Empty,
            string.Empty,
            "project-a",
            "scene-a",
            "fingerprint-a",
            [
                new NativeVoiceCueDto(
                    Guid.Parse("11111111-2222-3333-4444-555555555555"),
                    "utt-01",
                    4,
                    "春日部つむぎ",
                    "ここから第二形態です",
                    "ここから第二形態です",
                    120,
                    20,
                    180),
            ]);
        AssertGolden(
            Compute(nativeRequest),
            "f71d95d8871636271eeacf9fe4ddd858eaf706d1b38a8c63b0ec0dc11a6ad932");

        var nativeMutationRequest = new NativeVoiceMutationApplyRequestDto(
            2,
            Guid.Empty,
            string.Empty,
            "project-a",
            "scene-a",
            "fingerprint-a",
            [
                new NativeVoiceMutationDto(
                    Guid.Parse("11111111-2222-3333-4444-555555555555"),
                    "utt-01",
                    4,
                    "春日部つむぎ",
                    "ここから第二形態です",
                    "ここから第二形態です",
                    120,
                    20,
                    180,
                    "update"),
            ]);
        AssertGolden(
            Compute(nativeMutationRequest),
            "ac5a73a7bc78fa68d8d1835018d7d7da4752ee987d23c25749317657a595f7c4");

        using var targetPlan = System.Text.Json.JsonDocument.Parse(
            """
            {
              "canonicalVersion": 1,
              "operationId": "11111111-1111-4111-8111-111111111111",
              "baseRevision": 7,
              "target": {
                "adapterId": "ymm4-4.55",
                "projectId": "project-日本語",
                "sceneId": "scene-魔理沙",
                "fps": 60,
                "driverVersion": "4.55.1.1/0.2.0"
              },
              "capabilityDigest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
              "expectedScope": {
                "targetIdentityDigest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
                "managedStateDigest": "sha256:3333333333333333333333333333333333333333333333333333333333333333",
                "conflictScopeDigest": "sha256:4444444444444444444444444444444444444444444444444444444444444444"
              },
              "changeBudget": {
                "maxChangedEntities": 1,
                "maxShiftedEntities": 0,
                "maxShiftFrames": 0,
                "allowLockedChanges": false,
                "allowUnmanagedChanges": false
              },
              "cues": [],
              "warnings": []
            }
            """);
        var targetPlanDigest = CanonicalJson.Sha256(
            "takegraph-target-plan",
            targetPlan.RootElement);
        var targetPlanRequest = new TargetPlanApplyRequestDto(
            2,
            string.Empty,
            $"sha256:{new string('5', 64)}",
            targetPlanDigest,
            targetPlan.RootElement);
        AssertGolden(
            Compute(targetPlanRequest),
            "18a5f6cf916dd6250a51ee63dc9a03879f6759c107b2f419b871e84ca31f79fb");

        using var timelineEditPlan = System.Text.Json.JsonDocument.Parse(
            """
            {
              "canonicalVersion": 1,
              "operationId": "00000000-0000-0000-0000-00000000004d",
              "baseRevision": 9,
              "target": {
                "adapterId": "ymm4-4.55",
                "projectId": "project-a",
                "sceneId": "scene-a",
                "fps": 60,
                "driverVersion": "4.55.1.1/0.2.0"
              },
              "capabilityDigest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "expectedScope": {
                "targetIdentityDigest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "managedStateDigest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "conflictScopeDigest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
              },
              "changeBudget": {
                "maxChangedEntities": 1,
                "maxShiftedEntities": 0,
                "maxShiftFrames": 0,
                "allowLockedChanges": false,
                "allowUnmanagedChanges": false
              },
              "operations": [{
                "kind": "managed_cue",
                "cue": {
                  "intent": {
                    "entityId": "one",
                    "entityRevision": 1,
                    "displayText": "caption",
                    "spokenText": "spoken",
                    "speakerRole": "speaker",
                    "voiceProfile": "speaker",
                    "captionStyle": null,
                    "segmentationLocked": false,
                    "placement": {"anchor":{"type":"absolute_frame","frame":12},"ordering":"fixed","trackRole":"dialogue"},
                    "realizationPreference": "require_portable",
                    "fallbackPolicy": "reject",
                    "acceptedPortableTake": null,
                    "template": null,
                    "effects": [],
                    "hardLockPreconditions": []
                  },
                  "realizationId": "00000000-0000-0000-0000-000000000001",
                  "action": "create",
                  "strategy": "portable_pair",
                  "fallback": null,
                  "placement": {"frame":12,"primaryLayer":1,"secondaryLayer":2},
                  "duration": {"kind":"exact","frames":30},
                  "ownership": {"strict":["identity","captionText","audioArtifact","timing"],"derived":[],"preserve":["unknownNativeFields"],"global":["projectSettings"]},
                  "capabilityDependencies": [{"feature":"timelineEdit.apply","minimumVersion":1,"schemaDigest":"sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}],
                  "bindingDependencies": [{"kind":"audio_artifact","id":"artifact","digest":"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"}],
                  "resolvedRealization": {"kind":"portable_pair","audio_path":"audio.wav","artifact_digest":"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"}
                }
              }],
              "warnings": []
            }
            """);
        var timelineEditPlanDigest = CanonicalJson.Sha256(
            "takegraph-timeline-edit-plan-v1",
            timelineEditPlan.RootElement);
        AssertGolden(
            timelineEditPlanDigest,
            "sha256:2ac5a436e8d866383cc04a9d8ea02e7b185d3462be53fea9e7a01f25375b739b");
        var timelineEditRequest = new TimelineEditApplyRequestDto(
            2,
            string.Empty,
            $"sha256:{new string('1', 64)}",
            timelineEditPlanDigest,
            timelineEditPlan.RootElement,
            []);
        AssertGolden(
            Compute(timelineEditRequest),
            "e56f1165f5ff15bf3c1c1377b0aae4c59d31650e7dae7633cb9bc06471582fa0");

        var sceneCaptureRequest = new SceneCaptureRequestDto(
            2,
            Guid.Empty,
            string.Empty,
            "project-a",
            "scene-a",
            "fingerprint-a",
            7,
            "profile-a",
            [10, 20, 30],
            true);
        AssertGolden(
            Compute(sceneCaptureRequest),
            "931b8a38244a07c62e9181df5c739df70f2b8570ae3bdbb25a0817d85255c01c");

        var checkpointRequest = new CheckpointRequestDto(
            2,
            Guid.Empty,
            string.Empty,
            "project-a",
            "scene-a",
            7,
            "target-a",
            "state-a",
            "profile-a");
        AssertGolden(
            Compute(checkpointRequest),
            "7c4bc2d3d568eeee9ed9e018bc26158f9b8ffc7606373144b6a382771b0c32a5");

        var initializationRequest = new ProjectInitializationRequestDto(
            2,
            Guid.Parse("11111111-2222-4333-8444-555555555555"),
            string.Empty,
            $"sha256:{new string('1', 64)}",
            "instance-日本語",
            "project-untitled",
            "scene-a",
            $"sha256:{new string('2', 64)}",
            @"C:\projects\新規.ymmp",
            $"sha256:{new string('3', 64)}",
            "project-saved",
            $"sha256:{new string('4', 64)}",
            false);
        AssertGolden(
            Compute(initializationRequest),
            "32bbaf6a44ed11fdfa2c8de61cd6d6ed309047ebaf0d116bfb1add97d0d4b79a");

        var renderRequest = new RenderRequestDto(
            2,
            Guid.Empty,
            string.Empty,
            "project-a",
            "scene-a",
            7,
            "target-a",
            "state-a",
            Guid.Parse("11111111-1111-4111-8111-111111111111"),
            "checkpoint-request-a",
            @"C:\project\source.ymmp",
            new string('a', 64),
            123,
            "profile-a",
            @"C:\render\final.mp4",
            "deny");
        AssertGolden(
            Compute(renderRequest),
            "1f1bc2a281f3f96bd0e6dc63ba53d1a837b6b2794c7181529fbb4ca5642c53c4");

        var metadataDetachRequest = new MetadataDetachRequestDto(
            2,
            Guid.Parse("11111111-2222-4333-8444-555555555555"),
            string.Empty,
            "project-a",
            "scene-a",
            7,
            "fingerprint-a",
            "utt-01",
            Guid.Parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            "takegraph_remark_v2");
        AssertGolden(
            Compute(metadataDetachRequest),
            "a18c313ad1a046f8d0b25867ab8d988dbece617e55f23c102f70848a5e3e9802");

        using var nativeExtensionPlan = System.Text.Json.JsonDocument.Parse(
            """
            {
              "canonicalVersion": 1,
              "operationId": "00000000-0000-0000-0000-000000000000",
              "baseRevision": 7,
              "target": {
                "adapterId": "ymm4-4.55",
                "projectId": "project-日本語",
                "sceneId": "scene-魔理沙",
                "fps": 60,
                "driverVersion": "4.55.1.1/0.3.0"
              },
              "capabilityDigest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
              "descriptorCatalogDigest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
              "expectedScope": {
                "targetIdentityDigest": "sha256:3333333333333333333333333333333333333333333333333333333333333333",
                "managedStateDigest": "sha256:4444444444444444444444444444444444444444444444444444444444444444",
                "conflictScopeDigest": "sha256:5555555555555555555555555555555555555555555555555555555555555555"
              },
              "changeBudget": {
                "maxChangedEntities": 1,
                "maxShiftedEntities": 0,
                "maxShiftFrames": 0,
                "allowLockedChanges": false,
                "allowUnmanagedChanges": false
              },
              "operations": [{
                "realizationId": "11111111-2222-5333-8444-555555555555",
                "action": "create",
                "intent": {
                  "type": "upsert_asset",
                  "intent": {
                    "entityId": "背景-第一形態",
                    "entityRevision": 2,
                    "asset": {
                      "artifactDigest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                      "mediaType": "image/png",
                      "byteLength": 1234,
                      "kind": "image"
                    },
                    "placement": { "frame": 120, "primaryLayer": 10, "secondaryLayer": null },
                    "durationFrames": 180,
                    "loopPlayback": false,
                    "replacementGuard": { "approvedLossyFields": [] }
                  }
                },
                "capabilityDependencies": [
                  { "feature": "imageItem.upsert", "minimumVersion": 1, "schemaDigest": "sha256:6666666666666666666666666666666666666666666666666666666666666666" },
                  { "feature": "timeline.transaction", "minimumVersion": 1, "schemaDigest": "sha256:7777777777777777777777777777777777777777777777777777777777777777" }
                ],
                "descriptorDependencies": [],
                "preservation": {
                  "mode": "create",
                  "preservedFields": [],
                  "unknownEffects": [],
                  "lossyFields": [],
                  "approvedLossyFields": []
                }
              }],
              "warnings": ["素材を新規作成: 背景"]
            }
            """);
        var nativeExtensionPlanDigest = CanonicalJson.Sha256(
            "takegraph-native-extension-plan-v1",
            nativeExtensionPlan.RootElement);
        AssertGolden(
            nativeExtensionPlanDigest,
            "sha256:9a4d6f6bb603df06c63140caf42f1db1732e9e48d57bcdb82b37c7a85b1665f8");
        var nativeExtensionRequest = new NativeExtensionApplyRequestDto(
            2,
            Guid.Empty,
            string.Empty,
            "project-日本語",
            "scene-魔理沙",
            "fingerprint-魔理沙",
            new string('8', 64),
            new string('9', 64),
            nativeExtensionPlanDigest,
            nativeExtensionPlan.RootElement,
            [new NativeExtensionArtifactDto(
                $"sha256:{new string('b', 64)}",
                "image/png",
                1234,
                "image",
                @"C:\TakeGraph\素材\背景.png",
                $"sha256:{new string('b', 64)}")]);
        AssertGolden(
            Compute(nativeExtensionRequest),
            "e6a0faa5f40a5d37b08616203de9c049cba38cf2d28495bac6222460642ad907");
    }

    private static void AssertGolden(string actual, string expected)
    {
        if (!Matches(actual, expected))
        {
            throw new InvalidOperationException(
                $"YMM4 request digest implementation mismatch: expected {expected}, got {actual}");
        }
    }

    internal static string Compute(ApplyRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-apply-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendString(canonical, "expectedFingerprint", request.ExpectedFingerprint);
        AppendNumber(canonical, "utterances", request.Utterances.Count);
        foreach (var utterance in request.Utterances)
        {
            AppendString(canonical, "entityId", utterance.EntityId);
            AppendNumber(canonical, "revision", utterance.Revision);
            AppendString(canonical, "speaker", utterance.Speaker);
            AppendString(canonical, "caption", utterance.Caption);
            var spokenText = utterance.SpokenText ?? utterance.Caption;
            if (!string.Equals(spokenText, utterance.Caption, StringComparison.Ordinal))
            {
                AppendString(canonical, "spokenText", spokenText);
            }
            AppendString(canonical, "audioPath", utterance.AudioPath);
            AppendString(canonical, "artifactHash", utterance.ArtifactHash);
            AppendNumber(canonical, "frame", utterance.Frame);
            AppendNumber(canonical, "length", utterance.Length);
            AppendNumber(canonical, "audioLayer", utterance.AudioLayer);
            AppendNumber(canonical, "captionLayer", utterance.CaptionLayer);
        }
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(NativeVoiceApplyRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-native-voice-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendString(canonical, "expectedFingerprint", request.ExpectedFingerprint);
        AppendNumber(canonical, "cues", request.Cues.Count);
        foreach (var cue in request.Cues)
        {
            AppendString(canonical, "realizationId", cue.RealizationId.ToString("D"));
            AppendString(canonical, "entityId", cue.EntityId);
            AppendNumber(canonical, "revision", cue.Revision);
            AppendString(canonical, "characterName", cue.CharacterName);
            AppendString(canonical, "displayText", cue.DisplayText);
            if (cue.SpokenText is not null)
            {
                AppendString(canonical, "spokenText", cue.SpokenText);
            }
            AppendNumber(canonical, "frame", cue.Frame);
            AppendNumber(canonical, "layer", cue.Layer);
            AppendNumber(canonical, "maxLength", cue.MaxLength);
        }
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(TargetPlanApplyRequestDto request)
    {
        return CanonicalJson.Sha256(
            "takegraph-ymm4-target-plan-request-v2",
            new
            {
                protocolVersion = request.ProtocolVersion,
                expectedFingerprint = request.ExpectedFingerprint,
                targetPlanDigest = request.TargetPlanDigest,
                targetPlan = request.TargetPlan,
            })["sha256:".Length..];
    }

    internal static string Compute(TimelineEditApplyRequestDto request)
    {
        return CanonicalJson.Sha256(
            "takegraph-ymm4-timeline-edit-request-v1",
            new
            {
                protocolVersion = request.ProtocolVersion,
                expectedFingerprint = request.ExpectedFingerprint,
                planDigest = request.PlanDigest,
                timelineEditPlan = request.TimelineEditPlan,
                artifacts = request.Artifacts,
            })["sha256:".Length..];
    }

    internal static string Compute(SceneCaptureRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-scene-capture-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendString(canonical, "expectedFingerprint", request.ExpectedFingerprint);
        AppendNumber(canonical, "sourceRevision", request.SourceRevision);
        AppendString(canonical, "captureProfileDigest", request.CaptureProfileDigest);
        AppendNumber(canonical, "alpha", request.Alpha ? 1 : 0);
        AppendNumber(canonical, "frames", request.Frames.Count);
        foreach (var frame in request.Frames)
        {
            AppendNumber(canonical, "frame", frame);
        }
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(NativeVoiceMutationApplyRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-native-voice-mutation-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendString(canonical, "expectedFingerprint", request.ExpectedFingerprint);
        AppendNumber(canonical, "mutations", request.Mutations.Count);
        foreach (var mutation in request.Mutations)
        {
            AppendString(canonical, "realizationId", mutation.RealizationId.ToString("D"));
            AppendString(canonical, "entityId", mutation.EntityId);
            AppendNumber(canonical, "revision", mutation.Revision);
            AppendString(canonical, "characterName", mutation.CharacterName);
            AppendString(canonical, "displayText", mutation.DisplayText);
            if (mutation.SpokenText is not null)
            {
                AppendString(canonical, "spokenText", mutation.SpokenText);
            }
            AppendNumber(canonical, "frame", mutation.Frame);
            AppendNumber(canonical, "layer", mutation.Layer);
            AppendNumber(canonical, "maxLength", mutation.MaxLength);
            AppendString(canonical, "action", mutation.Action);
        }
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(CheckpointRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-checkpoint-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendNumber(canonical, "sourceRevision", request.SourceRevision);
        AppendString(canonical, "targetIdentityDigest", request.TargetIdentityDigest);
        AppendString(canonical, "expectedStateDigest", request.ExpectedStateDigest);
        AppendString(canonical, "checkpointProfileDigest", request.CheckpointProfileDigest);
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(ProjectInitializationRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-project-initialization-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "driverProfileDigest", request.DriverProfileDigest);
        AppendString(canonical, "sourceProjectInstanceId", request.SourceProjectInstanceId);
        AppendString(canonical, "sourceProjectId", request.SourceProjectId);
        AppendString(canonical, "sourceSceneId", request.SourceSceneId);
        AppendString(canonical, "expectedSourceFingerprint", request.ExpectedSourceFingerprint);
        AppendString(canonical, "destinationPath", request.DestinationPath);
        AppendString(canonical, "destinationPathDigest", request.DestinationPathDigest);
        AppendString(canonical, "predictedProjectId", request.PredictedProjectId);
        AppendString(canonical, "predictedFingerprint", request.PredictedFingerprint);
        AppendNumber(canonical, "overwrite", request.Overwrite ? 1 : 0);
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(RenderRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-render-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "taskId", request.TaskId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendNumber(canonical, "sourceRevision", request.SourceRevision);
        AppendString(canonical, "targetIdentityDigest", request.TargetIdentityDigest);
        AppendString(canonical, "expectedStateDigest", request.ExpectedStateDigest);
        AppendString(canonical, "checkpointOperationId", request.CheckpointOperationId.ToString("D"));
        AppendString(canonical, "checkpointRequestDigest", request.CheckpointRequestDigest);
        AppendString(canonical, "checkpointProjectPath", request.CheckpointProjectPath);
        AppendString(canonical, "checkpointFileSha256", request.CheckpointFileSha256);
        AppendNumber(canonical, "checkpointFileBytes", request.CheckpointFileBytes);
        AppendString(canonical, "renderProfileDigest", request.RenderProfileDigest);
        AppendString(canonical, "outputPath", request.OutputPath);
        AppendString(canonical, "overwritePolicy", request.OverwritePolicy);
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(MetadataDetachRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-metadata-detach-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendNumber(canonical, "sourceRevision", request.SourceRevision);
        AppendString(canonical, "expectedFingerprint", request.ExpectedFingerprint);
        AppendString(canonical, "entityId", request.EntityId);
        AppendString(canonical, "realizationId", request.RealizationId.ToString("D"));
        AppendString(canonical, "identityCarrier", request.IdentityCarrier);
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static string Compute(NativeExtensionApplyRequestDto request)
    {
        var canonical = new StringBuilder("takegraph-ymm4-native-extension-apply-v2\n");
        AppendNumber(canonical, "protocolVersion", request.ProtocolVersion);
        AppendString(canonical, "operationId", request.OperationId.ToString("D"));
        AppendString(canonical, "projectId", request.ProjectId);
        AppendString(canonical, "sceneId", request.SceneId);
        AppendString(canonical, "expectedFingerprint", request.ExpectedFingerprint);
        AppendString(canonical, "descriptorCatalogDigest", request.DescriptorCatalogDigest);
        AppendString(canonical, "driverProfileDigest", request.DriverProfileDigest);
        AppendString(canonical, "planDigest", request.PlanDigest);
        AppendNumber(canonical, "artifacts", request.Artifacts.Count);
        foreach (var artifact in request.Artifacts)
        {
            AppendString(canonical, "artifactDigest", artifact.ArtifactDigest);
            AppendString(canonical, "mediaType", artifact.MediaType);
            AppendNumber(canonical, "byteLength", artifact.ByteLength);
            AppendString(canonical, "kind", artifact.Kind);
            AppendString(canonical, "path", artifact.Path);
            AppendString(canonical, "sha256", artifact.Sha256);
        }
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    internal static bool Matches(string? provided, string? calculated)
    {
        if (provided is null || calculated is null || provided.Length != calculated.Length)
        {
            return false;
        }
        return CryptographicOperations.FixedTimeEquals(
            Encoding.ASCII.GetBytes(provided),
            Encoding.ASCII.GetBytes(calculated));
    }

    private static void AppendString(StringBuilder canonical, string label, string value)
    {
        canonical.Append(label)
            .Append(':')
            .Append(Encoding.UTF8.GetByteCount(value))
            .Append(':')
            .Append(value)
            .Append('\n');
    }

    private static void AppendNumber<T>(StringBuilder canonical, string label, T value)
        where T : IFormattable
    {
        canonical.Append(label)
            .Append(':')
            .Append(value.ToString(null, CultureInfo.InvariantCulture))
            .Append('\n');
    }
}
