import assert from "node:assert/strict";
import test from "node:test";
import {
  formatAgentError,
  formatAnnotationsText,
  formatCompositionText,
  formatDescriptorInventoryText,
  formatRenderProfilesText,
  formatStagedTaskReport,
  formatStatusText,
  formatStudioSessionText,
  formatYmm4DescribeText,
  STORE_CANONICAL,
  STORE_STUDIO,
  TAKEGRAPH_AGENT_GUIDE,
} from "./agent-text.js";
import { ProjectSession } from "./project-session.js";

function initialStudio() {
  return new ProjectSession().snapshot();
}

test("studio session report names the demo store and lists take IDs", () => {
  const text = formatStudioSessionText(initialStudio(), "Studio session inventory.");
  assert.match(text, new RegExp(`store: ${STORE_STUDIO}`));
  assert.match(text, /sessionRevision: 0/);
  assert.match(text, /utt-01/);
  assert.match(text, /take-a/);
  assert.match(text, /take-b/);
  assert.match(text, /not the canonical/);
  assert.match(text, /next: voice_stage_take_patch/);
  assert.doesNotMatch(text, /canonicalRevision: 0/);
});

test("studio stage report exposes patchId and digest from the live session", () => {
  const session = new ProjectSession();
  const state = session.stageTake("take-b");
  const text = formatStudioSessionText(state, "staged");
  assert.ok(state.stagedPatch);
  assert.match(text, new RegExp(`patchId: ${state.stagedPatch.id}`));
  assert.match(text, new RegExp(`digest: ${state.stagedPatch.digest}`));
  assert.match(
    text,
    new RegExp(
      `next: studio_patch_commit \\{ patchId: ${state.stagedPatch.id}, digest: ${state.stagedPatch.digest} \\}`,
    ),
  );
});

test("canonical describe report lists path blockers and does not alias studio revision", () => {
  const text = formatYmm4DescribeText({
    health: { status: "running", protocolVersion: 2, ymm4Version: "4.55.1.1" },
    snapshot: {
      projectId: "proj-1",
      projectName: "empty",
      projectPath: "",
      sceneId: "scene-1",
      fps: 60,
      fingerprint: "fp",
      managedItems: [
        {
          entityId: "utt-1",
          kind: "voice",
          speaker: "春日部つむぎ",
          text: "確認",
          frame: 0,
          layer: 0,
          realizationId: "real-1",
        },
      ],
      nativeExtensions: [],
      unmanagedContextCount: 0,
    },
    head: { projectId: "proj-1", revision: 1 },
  });
  assert.match(text, new RegExp(`store: ${STORE_CANONICAL}`));
  assert.match(text, /canonicalRevision: 1/);
  assert.match(text, /projectPath: \(unsaved\)/);
  assert.match(text, /utt-1/);
  assert.match(text, /blocked: unsaved YMM4 project/);
  assert.match(text, /canonical stage\/apply/);
  assert.match(text, /kind=project_initialization/);
  assert.match(text, /mode=save_untitled/);
  assert.doesNotMatch(text, /ymm4_native_voice_mutation_stage or ymm4_export_stage/);
  assert.doesNotMatch(text, /sessionRevision:/);
});

test("descriptor inventory prints bindable IDs not only display names", () => {
  const text = formatDescriptorInventoryText({
    targetCatalog: {
      descriptors: [
        {
          descriptorId: "ymm4-character:abc",
          configDigest: "aa".repeat(32),
          schemaDigest: "bb".repeat(32),
          bindable: true,
        },
      ],
    },
    planningCatalog: {
      characters: {
        "ymm4-character:abc": {
          descriptorId: "ymm4-character:abc",
          displayName: "春日部つむぎ",
          configuration: {
            "takegraph.targetConfigDigest": "aa".repeat(32),
            "takegraph.targetSchemaDigest": "bb".repeat(32),
          },
        },
      },
    },
  });
  assert.match(text, /ymm4-character:abc/);
  assert.match(text, /春日部つむぎ/);
  assert.match(text, /expectedConfigDigest|configDigest=aa/);
  assert.match(text, /ymm4_native_extension_stage/);
});

test("composition report lists observed elements without inventing geometry", () => {
  const text = formatCompositionText({
    observationStatus: "source_bound",
    completeness: "partial",
    evaluatedFrame: 120,
    projectId: "proj-1",
    sceneId: "scene-1",
    fps: 60,
    sourceFingerprint: "fp",
    elements: [
      {
        elementId: "sample-reimu-01",
        kind: "voice",
        speaker: "ゆっくり霊夢",
        text: "こんにちは",
        frame: 60,
        layer: 2,
      },
    ],
    unavailableFields: ["viewport.width"],
  });
  assert.match(text, /sample-reimu-01/);
  assert.match(text, /ゆっくり霊夢/);
  assert.match(text, /evaluatedFrame: 120/);
  assert.match(text, /unavailableFields: viewport.width/);
  assert.match(text, /kind=scene_inspection/);
});

test("render profile report blocks staging when nothing is bindable", () => {
  const text = formatRenderProfilesText({
    profiles: [{ descriptorId: "final-mp4", bindable: false, bindingError: "no closure" }],
  });
  assert.match(text, /final-mp4/);
  assert.match(text, /blocked: no bindable render profile/);
  assert.match(text, /do not call ymm4_render_stage/);
});

test("staged task report is sufficient to assemble the next commit call", () => {
  const text = formatStagedTaskReport({
    kind: "YMM4 native voice mutation",
    store: STORE_CANONICAL,
    identityKey: "handle",
    identity: "86671fbc-39ea-45da-bbe5-429dc271e417",
    digest: `sha256:${"f".repeat(64)}`,
    next: "ymm4_native_voice_mutation_commit { handle, digest }",
  });
  assert.match(text, /handle: 86671fbc-39ea-45da-bbe5-429dc271e417/);
  assert.match(text, new RegExp(`digest: sha256:${"f".repeat(64)}`));
  assert.match(text, /next: ymm4_native_voice_mutation_commit/);
});

test("status report keeps both heads and the static guide", () => {
  const text = formatStatusText({
    studio: initialStudio(),
    canonical: {
      snapshot: { projectId: "proj-1", projectPath: "", managedItems: [] },
      head: { projectId: "proj-1", revision: 1 },
    },
  });
  assert.match(text, /TakeGraph MCP guide/);
  assert.match(text, new RegExp(STORE_STUDIO));
  assert.match(text, new RegExp(STORE_CANONICAL));
  assert.match(text, /sessionRevision: 0/);
  assert.match(text, /canonicalRevision: 1/);
  assert.equal(text.includes(TAKEGRAPH_AGENT_GUIDE), true);
});

test("agent guide presents the consolidated task workflow and approval tokens", () => {
  for (const tool of [
    "takegraph_inspect",
    "takegraph_task_stage",
    "takegraph_task_approve",
    "takegraph_task_execute",
    "takegraph_task_decide",
  ]) {
    assert.match(TAKEGRAPH_AGENT_GUIDE, new RegExp(`- ${tool}:`));
  }
  assert.match(TAKEGRAPH_AGENT_GUIDE, /taskId is the opaque public identity/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /approve the exact planDigest/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /evidenceDigest binds accept\/reject/);
  assert.match(
    TAKEGRAPH_AGENT_GUIDE,
    /kind=timeline_edit.*operations.*1-128.*portable_voice_create.*native_voice_create.*one managed-cue task.*one exact planDigest/s,
  );
  assert.match(TAKEGRAPH_AGENT_GUIDE, /sourceEvidence/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /kind=annotation_derive/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /timeline_edit restart.*intent=revalidate.*same taskId/s);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /update\/delete remain on kind=native_voice_mutation/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /Native extensions are not part of timeline_edit/);
  assert.match(
    TAKEGRAPH_AGENT_GUIDE,
    /kind=project_initialization.*mode=adopt_active/s,
  );
  assert.match(
    TAKEGRAPH_AGENT_GUIDE,
    /mode=save_untitled.*explicit path.*staging only/s,
  );
  assert.match(TAKEGRAPH_AGENT_GUIDE, /view=annotations is read-only/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /audioSha256, transcriptDigest, interpretationDigest, and derivePhase/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /ASR executable\/model paths/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, /Legacy route-specific tools are opt-in/);
  assert.match(TAKEGRAPH_AGENT_GUIDE, new RegExp(STORE_STUDIO));
  assert.match(TAKEGRAPH_AGENT_GUIDE, new RegExp(STORE_CANONICAL));
});

test("annotation inventory text names IDs and refuses capture from MCP", () => {
  const text = formatAnnotationsText({
    projectId: "project-a",
    sourceFingerprint: "fp-1",
    annotations: [
      {
        annotationId: "ann-1",
        startFrame: 10,
        endFrame: 20,
        stability: "stable",
        lifecycle: "active",
        stale: false,
        intents: [{ kind: "note" }],
        temporal: { relation: "at", referenceFrame: 10, startOffsetFrames: 0 },
        transcriptSummary: "残す",
        audioSha256: `sha256:${"a".repeat(64)}`,
        transcriptDigest: `sha256:${"b".repeat(64)}`,
        interpretationDigest: `sha256:${"c".repeat(64)}`,
        derivePhase: "succeeded",
        promotionStatus: "staged",
        promotionTaskId: "55555555-5555-4555-8555-555555555555",
        promotionPlanDigest: `sha256:${"2".repeat(64)}`,
        pinEntityId: "ann-aaaaaaaa-pin",
        pinRealizationId: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
      },
    ],
  });
  assert.match(text, /ann-1/);
  assert.match(text, /frames=10-20/);
  assert.match(text, /temporal=at@10:0/);
  assert.match(text, /derivePhase=succeeded/);
  assert.match(text, /audioSha256=sha256:a{64}/);
  assert.match(text, /transcriptDigest=sha256:b{64}/);
  assert.match(text, /promotion=staged/);
  assert.match(text, /promotionPlanDigest=sha256:2{64}/);
  assert.match(text, /pinEntityId=ann-aaaaaaaa-pin/);
  assert.match(text, /do not restage/);
  assert.match(text, /recording is local-only/);
  assert.match(text, /annotation_derive/);
});

test("agent errors keep the original message and add a next action", () => {
  const target = formatAgentError(
    new Error('ProjectStore(TargetLinkNotFound("abc/def"))'),
  );
  assert.match(target, /TargetLinkNotFound/);
  assert.match(target, /takegraph_task_stage with kind=native_voice_mutation/);
  assert.match(target, /cannot: takegraph_task_stage with kind=reconciliation/);

  const queryReady = formatAgentError(
    new Error(
      "Take take-x has no completed audio artifact yet; only its synthesis query is ready. readyTakes: take-a",
    ),
  );
  assert.match(queryReady, /take-x/);
  assert.match(queryReady, /takegraph_task_stage with kind=studio_take/);

  const unsaved = formatAgentError(
    new Error("project has no existing path; Save As is not authorized"),
  );
  assert.match(unsaved, /untitled YMM4 cannot stage/);
  assert.match(unsaved, /kind=project_initialization/);
  assert.match(unsaved, /mode=save_untitled/);
});
