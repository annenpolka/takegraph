import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ProjectSession, type ProjectState } from "./project-session.js";
import { createServer } from "./server.js";
import {
  type StageSceneInspectionInput,
  type StageNativeVoiceInput,
  type StageNativeVoiceMutationsInput,
  Ymm4Workflow,
} from "./ymm4-workflow.js";

const resourceUri = "ui://takegraph/editor/v1.html";

function stateFrom(result: unknown): ProjectState {
  const state = (
    result as { structuredContent?: { state?: ProjectState } }
  ).structuredContent?.state;
  assert.ok(state, "tool result should include project state");
  return state;
}

test("MCP client can open the app and complete a digest-bound take patch", async (t) => {
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph test app</body></html>",
  });
  const client = new Client({ name: "takegraph-test", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();

  await Promise.all([
    server.connect(serverTransport),
    client.connect(clientTransport),
  ]);
  t.after(async () => {
    await client.close();
    await server.close();
  });

  const listed = await client.listTools();
  const opener = listed.tools.find(
    (tool) => tool.name === "studio_project_describe",
  );
  assert.ok(opener);
  assert.equal(
    (opener._meta?.ui as { resourceUri?: string } | undefined)?.resourceUri,
    resourceUri,
  );
  assert.deepEqual(
    (opener._meta?.ui as { visibility?: string[] } | undefined)?.visibility,
    ["model", "app"],
  );
  const appOnlyStateTool = listed.tools.find(
    (tool) => tool.name === "studio_ui_get_state",
  );
  assert.deepEqual(
    (appOnlyStateTool?._meta?.ui as { visibility?: string[] } | undefined)
      ?.visibility,
    ["app"],
  );

  const described = await client.callTool({
    name: "studio_project_describe",
    arguments: {},
  });
  const initial = stateFrom(described);
  assert.equal(initial.revision, 0);
  assert.equal(initial.activeTakeId, "take-a");

  const resource = await client.readResource({ uri: resourceUri });
  assert.equal(resource.contents[0]?.mimeType, "text/html;profile=mcp-app");
  assert.match(
    "text" in resource.contents[0]! ? resource.contents[0].text : "",
    /TakeGraph test app/,
  );

  const generated = await client.callTool({
    name: "voice_generate_variant",
    arguments: { utteranceId: "utt-01", speed: 1.2, intonation: 1.1 },
  });
  const afterGeneration = stateFrom(generated);
  assert.equal(afterGeneration.takes.length, initial.takes.length + 1);
  assert.equal(afterGeneration.takes.at(-1)?.readiness, "query-ready");

  const rejectedUnfinished = await client.callTool({
    name: "voice_stage_take_patch",
    arguments: { takeId: afterGeneration.takes.at(-1)!.id },
  });
  assert.equal(
    (rejectedUnfinished as { isError?: boolean }).isError,
    true,
  );

  const staged = await client.callTool({
    name: "voice_stage_take_patch",
    arguments: { takeId: "take-b" },
  });
  const patch = stateFrom(staged).stagedPatch;
  assert.ok(patch);
  assert.equal(patch.baseRevision, 0);
  assert.equal(patch.digest.length, 64);

  const wrongApproval = await client.callTool({
    name: "studio_patch_commit",
    arguments: { patchId: patch.id, digest: "0".repeat(64) },
  });
  assert.equal((wrongApproval as { isError?: boolean }).isError, true);

  const committed = await client.callTool({
    name: "studio_patch_commit",
    arguments: { patchId: patch.id, digest: patch.digest },
  });
  const finalState = stateFrom(committed);
  assert.equal(finalState.revision, 1);
  assert.equal(finalState.activeTakeId, "take-b");
  assert.equal(finalState.stagedPatch, undefined);
});

test("MCP native VoiceItem tools validate the v2 slice and delegate workflow", async (t) => {
  const handle = "33333333-3333-4333-8333-333333333333";
  const digest = "a".repeat(64);
  const stagedInputs: StageNativeVoiceInput[] = [];
  const workflow = {
    async stageNativeVoice(input: StageNativeVoiceInput) {
      stagedInputs.push(input);
      return {
        handle,
        realizationId: "44444444-4444-4444-8444-444444444444",
        digest,
        baseRevision: 7,
        project: { projectId: "project-1" },
        impact: { createCount: 1, durationResolution: "bounded" },
        placement: { frame: input.frame, layer: input.layer, maxLength: input.maxLength },
      };
    },
    async commitNativeVoice(receivedHandle: string, receivedDigest: string) {
      assert.equal(receivedHandle, handle);
      assert.equal(receivedDigest, digest);
      return { revision: 8, verified: true };
    },
    async verifyNativeVoice(receivedHandle: string) {
      assert.equal(receivedHandle, handle);
      return { verified: true, realizationKind: "ymm4_native_voice" };
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph test app</body></html>",
    ymm4Workflow: workflow,
  });
  const client = new Client({ name: "takegraph-native-test", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();

  await Promise.all([
    server.connect(serverTransport),
    client.connect(clientTransport),
  ]);
  t.after(async () => {
    await client.close();
    await server.close();
  });

  const listed = await client.listTools();
  const stageTool = listed.tools.find(
    (tool) => tool.name === "ymm4_native_voice_stage",
  );
  assert.ok(stageTool);
  assert.match(stageTool.description ?? "", /exactly equal/);
  const spokenTextSchema = (
    stageTool.inputSchema.properties as Record<
      string,
      { description?: string }
    >
  ).spokenText;
  assert.match(spokenTextSchema?.description ?? "", /exactly equal displayText/);

  const rejected = await client.callTool({
    name: "ymm4_native_voice_stage",
    arguments: {
      entityId: "utt-native-01",
      displayText: "VOICEVOX",
      spokenText: "ボイスボックス",
      characterName: "春日部つむぎ",
      frame: 0,
      layer: 0,
      maxLength: 240,
    },
  });
  assert.equal((rejected as { isError?: boolean }).isError, true);
  assert.equal(stagedInputs.length, 0);

  const staged = await client.callTool({
    name: "ymm4_native_voice_stage",
    arguments: {
      entityId: "utt-native-01",
      displayText: "ここから第二形態だぜ",
      spokenText: "ここから第二形態だぜ",
      characterName: "春日部つむぎ",
      frame: 3600,
      layer: 0,
      maxLength: 240,
    },
  });
  assert.equal((staged.structuredContent as { handle?: string }).handle, handle);
  assert.equal(stagedInputs.length, 1);

  const committed = await client.callTool({
    name: "ymm4_native_voice_commit",
    arguments: { handle, digest },
  });
  assert.equal(
    (committed.structuredContent as { revision?: number }).revision,
    8,
  );

  const verified = await client.callTool({
    name: "ymm4_native_voice_verify",
    arguments: { handle },
  });
  assert.equal(
    (verified.structuredContent as { realizationKind?: string }).realizationKind,
    "ymm4_native_voice",
  );
});

test("MCP native voice mutation tools expose batch lifecycle and honest artifact provenance", async (t) => {
  const handle = "66666666-6666-4666-8666-666666666666";
  const digest = `sha256:${"d".repeat(64)}`;
  const calls: string[] = [];
  const workflow = {
    async stageNativeVoiceMutations(input: StageNativeVoiceMutationsInput) {
      calls.push("stage");
      assert.deepEqual(
        input.mutations.map((value) => value.action),
        ["create", "update", "delete"],
      );
      return {
        handle,
        realizationIds: [
          "77777777-7777-4777-8777-777777777777",
          "88888888-8888-4888-8888-888888888888",
          "99999999-9999-4999-8999-999999999999",
        ],
        digest,
        baseRevision: 7,
        operationId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        project: { projectId: "project-1" },
        impact: { createCount: 1, updateCount: 1, deleteCount: 1 },
        capabilityDigest: `sha256:${"c".repeat(64)}`,
      };
    },
    async commitNativeVoiceMutations(receivedHandle: string, receivedDigest: string) {
      calls.push("commit");
      assert.equal(receivedHandle, handle);
      assert.equal(receivedDigest, digest);
      return { revision: 8, replayVerified: true };
    },
    async verifyNativeVoiceMutations(receivedHandle: string) {
      calls.push("verify");
      assert.equal(receivedHandle, handle);
      return { verified: true };
    },
    async captureNativeVoiceMutationArtifacts(receivedHandle: string) {
      calls.push("artifacts");
      assert.equal(receivedHandle, handle);
      return {
        artifactSemantics: {
          audio: "exact_wav",
          provenance: "normalized_host_bound_voice_state",
          portableSynthesisQuery: false,
        },
        artifacts: [],
      };
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph test app</body></html>",
    ymm4Workflow: workflow,
  });
  const client = new Client({ name: "voice-mutation-test", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);
  t.after(async () => {
    await client.close();
    await server.close();
  });

  const listed = await client.listTools();
  const artifactTool = listed.tools.find(
    (tool) => tool.name === "ymm4_native_voice_mutation_artifacts",
  );
  assert.match(artifactTool?.description ?? "", /not a portable synthesis query/i);
  const staged = await client.callTool({
    name: "ymm4_native_voice_mutation_stage",
    arguments: {
      mutations: [
        {
          action: "create",
          entityId: "utt-create",
          revision: 1,
          characterName: "魔理沙",
          displayText: "ここから第二形態だぜ",
          spokenText: "ここから第二形態だぜ",
          frame: 100,
          layer: 10,
          maxLength: 180,
        },
        {
          action: "update",
          realizationId: "88888888-8888-4888-8888-888888888888",
          entityId: "utt-update",
          revision: 2,
          characterName: "魔理沙",
          displayText: "更新だぜ",
          spokenText: "更新だぜ",
          frame: 200,
          layer: 11,
          maxLength: 180,
        },
        {
          action: "delete",
          realizationId: "99999999-9999-4999-8999-999999999999",
          entityId: "utt-delete",
          revision: 3,
        },
      ],
    },
  });
  assert.equal((staged.structuredContent as { handle?: string }).handle, handle);
  await client.callTool({
    name: "ymm4_native_voice_mutation_commit",
    arguments: { handle, digest },
  });
  await client.callTool({
    name: "ymm4_native_voice_mutation_verify",
    arguments: { handle },
  });
  const artifacts = await client.callTool({
    name: "ymm4_native_voice_mutation_artifacts",
    arguments: { handle },
  });
  assert.equal(
    (
      artifacts.structuredContent as {
        artifactSemantics: { portableSynthesisQuery: boolean };
      }
    ).artifactSemantics.portableSynthesisQuery,
    false,
  );
  assert.deepEqual(calls, ["stage", "commit", "verify", "artifacts"]);
});

test("MCP scene inspection requires explicit approval, review, and human decision", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-scene-mcp-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    "base64",
  );
  const sha256 = createHash("sha256").update(png).digest("hex");
  const sceneArtifactRoot = path.join(root, "artifacts", "scene-captures");
  const artifactPath = path.join(sceneArtifactRoot, sha256.slice(0, 2), `${sha256}.png`);
  await fs.mkdir(path.dirname(artifactPath), { recursive: true });
  await fs.writeFile(artifactPath, png);
  const capture = {
    artifactPath,
    sha256,
    mediaType: "image/png",
    width: 1,
    height: 1,
  };
  const handle = "55555555-5555-4555-8555-555555555555";
  const digest = "b".repeat(64);
  const calls: string[] = [];
  const workflow = {
    async stageSceneInspection(input: StageSceneInspectionInput) {
      calls.push("stage");
      assert.deepEqual(input.frames, [10, 20]);
      return { handle, digest, receipt: { status: "staged", captures: [] } };
    },
    async approveSceneInspection(receivedHandle: string, receivedDigest: string) {
      calls.push("approve");
      assert.equal(receivedHandle, handle);
      assert.equal(receivedDigest, digest);
      return { receipt: { status: "approved", captures: [] } };
    },
    async captureSceneInspection(receivedHandle: string) {
      calls.push("capture");
      assert.equal(receivedHandle, handle);
      return {
        receipt: {
          status: "captured",
          captures: [capture, capture],
          findings: [{ code: "caption_clipped", severity: "warning" }],
        },
        semanticDiff: { changed: false },
      };
    },
    async reviewSceneInspection(receivedHandle: string, reviewer: string) {
      calls.push("review");
      assert.equal(receivedHandle, handle);
      assert.equal(reviewer, "lance");
      return {
        receipt: {
          status: "reviewed",
          captures: [capture],
        },
        semanticDiff: { changed: false },
      };
    },
    async decideSceneInspection(
      receivedHandle: string,
      decision: "accept" | "reject",
      note: string,
    ) {
      calls.push(`decide:${decision}`);
      assert.equal(receivedHandle, handle);
      assert.equal(note, "images inspected manually");
      return { receipt: { status: "accepted", captures: [] } };
    },
    async sceneInspectionStatus() {
      calls.push("status");
      return { receipt: { status: "accepted", captures: [capture] } };
    },
    async replaySceneInspection() {
      calls.push("replay");
      return { receipt: { status: "captured", captures: [capture] } };
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph test app</body></html>",
    ymm4Workflow: workflow,
    sceneArtifactRoot,
  });
  const client = new Client({ name: "scene-test", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);
  t.after(async () => {
    await client.close();
    await server.close();
  });

  const tools = await client.listTools();
  const decideTool = tools.tools.find(
    (tool) => tool.name === "ymm4_scene_inspection_decide",
  );
  assert.match(decideTool?.description ?? "", /human/i);
  assert.match(decideTool?.description ?? "", /Automated findings cannot/i);

  await client.callTool({
    name: "ymm4_scene_inspection_stage",
    arguments: { frames: [10, 20], expectedWidth: 1920, expectedHeight: 1080 },
  });
  assert.deepEqual(calls, ["stage"]);
  await client.callTool({
    name: "ymm4_scene_inspection_approve",
    arguments: { handle, digest },
  });
  const captured = await client.callTool({
    name: "ymm4_scene_inspection_capture",
    arguments: { handle },
  });
  const capturedImages = (captured.content as Array<{ type: string; data?: string }>).filter(
    (item) => item.type === "image",
  );
  assert.equal(capturedImages.length, 1, "duplicate PNG receipts should attach once");
  assert.equal(capturedImages[0]?.data, png.toString("base64"));
  assert.deepEqual(calls, ["stage", "approve", "capture"]);

  const reviewed = await client.callTool({
    name: "ymm4_scene_inspection_review",
    arguments: { handle, reviewer: "lance" },
  });
  assert.match(
    (reviewed.content as Array<{ text?: string }>)[0]?.text ?? "",
    /Inspection images/,
  );
  assert.equal(
    (reviewed.content as Array<{ type: string }>).filter((item) => item.type === "image")
      .length,
    1,
  );
  assert.ok(!calls.some((call) => call.startsWith("decide:")));

  await client.callTool({
    name: "ymm4_scene_inspection_decide",
    arguments: { handle, decision: "accept", note: "images inspected manually" },
  });
  assert.deepEqual(calls, [
    "stage",
    "approve",
    "capture",
    "review",
    "decide:accept",
  ]);
  const status = await client.callTool({
    name: "ymm4_scene_inspection_status",
    arguments: { handle },
  });
  const replay = await client.callTool({
    name: "ymm4_scene_inspection_replay",
    arguments: { handle },
  });
  assert.equal(
    (status.content as Array<{ type: string }>).filter((item) => item.type === "image").length,
    0,
    "persisted status must not inline images before authenticated bridge replay",
  );
  assert.equal(
    (replay.content as Array<{ type: string }>).filter((item) => item.type === "image").length,
    1,
  );
  assert.deepEqual(calls.slice(-2), ["status", "replay"]);
});

test("MCP scene image attachments reject untrusted identity and oversized bytes", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-scene-mcp-invalid-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    "base64",
  );
  const falseHash = "0".repeat(64);
  const sceneArtifactRoot = path.join(root, "artifacts", "scene-captures");
  const artifactPath = path.join(sceneArtifactRoot, "00", `${falseHash}.png`);
  await fs.mkdir(path.dirname(artifactPath), { recursive: true });
  await fs.writeFile(artifactPath, png);
  let capture: Record<string, unknown> = {
    artifactPath,
    sha256: falseHash,
    mediaType: "image/png",
    width: 1,
    height: 1,
  };
  const workflow = {
    async captureSceneInspection() {
      return { receipt: { status: "captured", captures: [capture] } };
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph test app</body></html>",
    ymm4Workflow: workflow,
    sceneArtifactRoot,
  });
  const client = new Client({ name: "scene-image-fail-test", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);
  t.after(async () => {
    await client.close();
    await server.close();
  });
  const handle = "55555555-5555-4555-8555-555555555555";
  const callCapture = () => client.callTool({
    name: "ymm4_scene_inspection_capture",
    arguments: { handle },
  });

  let result = await callCapture();
  assert.equal(result.isError, true);
  assert.match((result.content as Array<{ text?: string }>)[0]?.text ?? "", /receipt hash/);

  capture = {
    ...capture,
    artifactPath: "relative-user-controlled.png",
    sha256: createHash("sha256").update(png).digest("hex"),
  };
  result = await callCapture();
  assert.equal(result.isError, true);
  assert.match((result.content as Array<{ text?: string }>)[0]?.text ?? "", /invalid PNG artifact identity/);

  const actualHash = createHash("sha256").update(png).digest("hex");
  const outsidePath = path.join(
    root,
    "outside-cas",
    actualHash.slice(0, 2),
    `${actualHash}.png`,
  );
  await fs.mkdir(path.dirname(outsidePath), { recursive: true });
  await fs.writeFile(outsidePath, png);
  capture = {
    ...capture,
    artifactPath: outsidePath,
    sha256: actualHash,
  };
  result = await callCapture();
  assert.equal(result.isError, true);
  assert.match((result.content as Array<{ text?: string }>)[0]?.text ?? "", /artifact boundary/);

  const oversizedPath = path.join(
    sceneArtifactRoot,
    actualHash.slice(0, 2),
    `${actualHash}.png`,
  );
  await fs.mkdir(path.dirname(oversizedPath), { recursive: true });
  await fs.writeFile(oversizedPath, png);
  await fs.truncate(oversizedPath, 8 * 1024 * 1024 + 1);
  capture = {
    ...capture,
    artifactPath: oversizedPath,
    sha256: actualHash,
  };
  result = await callCapture();
  assert.equal(result.isError, true);
  assert.match((result.content as Array<{ text?: string }>)[0]?.text ?? "", /bounded regular file/);

  const overEightKPng = Buffer.from(png);
  overEightKPng.writeUInt32BE(7_681, 16);
  overEightKPng.writeUInt32BE(4_320, 20);
  const overEightKHash = createHash("sha256").update(overEightKPng).digest("hex");
  const overEightKPath = path.join(
    sceneArtifactRoot,
    overEightKHash.slice(0, 2),
    `${overEightKHash}.png`,
  );
  await fs.mkdir(path.dirname(overEightKPath), { recursive: true });
  await fs.writeFile(overEightKPath, overEightKPng);
  capture = {
    artifactPath: overEightKPath,
    sha256: overEightKHash,
    mediaType: "image/png",
    width: 7_681,
    height: 4_320,
  };
  result = await callCapture();
  assert.equal(result.isError, true);
  assert.match((result.content as Array<{ text?: string }>)[0]?.text ?? "", /dimensions/);
});

test("MCP scene status accepts an empty capture set without touching CAS and rejects malformed receipts", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-scene-empty-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const missingArtifactRoot = path.join(root, "never-created");
  let captures: unknown = [];
  const workflow = {
    async sceneInspectionStatus() {
      return { receipt: { status: "staged", captures } };
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph test app</body></html>",
    ymm4Workflow: workflow,
    sceneArtifactRoot: missingArtifactRoot,
  });
  const client = new Client({ name: "scene-empty-test", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);
  t.after(async () => {
    await client.close();
    await server.close();
  });
  const handle = "55555555-5555-4555-8555-555555555555";
  let result = await client.callTool({
    name: "ymm4_scene_inspection_status",
    arguments: { handle },
  });
  assert.equal(result.isError, undefined);
  assert.equal(
    (result.content as Array<{ type: string }>).filter((item) => item.type === "image")
      .length,
    0,
  );
  await assert.rejects(() => fs.access(missingArtifactRoot), /ENOENT/);

  captures = "not-an-array";
  result = await client.callTool({
    name: "ymm4_scene_inspection_status",
    arguments: { handle },
  });
  assert.equal(result.isError, true);
  assert.match(
    (result.content as Array<{ text?: string }>)[0]?.text ?? "",
    /invalid captures collection/,
  );
});
