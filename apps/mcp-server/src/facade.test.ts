import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { type ProjectState, ProjectSession } from "./project-session.js";
import { TaskFacadeRegistry } from "./facade.js";
import { createServer, type CreateServerOptions } from "./server.js";
import {
  type ReconciliationDecisionInput,
  type StageNativeExtensionInput,
  type StageNativeVoiceRequest,
  type StageRenderInput,
  type StageSceneInspectionInput,
  type StageTimelineEditInput,
  type StageYmm4Input,
  type StageYmm4Request,
  Ymm4Workflow,
} from "./ymm4-workflow.js";

const facadeNames = [
  "takegraph_inspect",
  "takegraph_task_stage",
  "takegraph_task_approve",
  "takegraph_task_execute",
  "takegraph_task_decide",
] as const;

const studioToolNames = [
  "studio_project_describe",
  "studio_ui_get_state",
  "voice_generate_variant",
  "voice_stage_take_patch",
  "studio_patch_commit",
] as const;

const taskEnvelopeFields = [
  "taskId",
  "kind",
  "store",
  "phase",
  "source",
  "planDigest",
  "approvedPlanDigest",
  "evidenceDigest",
  "receiptDigest",
  "availableActions",
  "staleReasons",
] as const;

type ToolResult = Awaited<ReturnType<Client["callTool"]>>;
type FacadeServerOptions = CreateServerOptions & { legacyTools?: boolean };

async function connect(options: FacadeServerOptions = {}) {
  const server = createServer({
    viewHtml: "<!doctype html><html><body>TakeGraph facade test</body></html>",
    ...options,
  } as CreateServerOptions);
  const client = new Client({ name: "takegraph-facade-test", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);
  return {
    client,
    async close() {
      await client.close();
      await server.close();
    },
  };
}

function structuredFrom(result: ToolResult): Record<string, unknown> {
  const structured = result.structuredContent;
  assert.ok(structured, "tool result should include structuredContent");
  return structured as Record<string, unknown>;
}

function textFrom(result: ToolResult): string {
  const content = result.content as Array<{ type?: string; text?: string }>;
  const text = content.find((item) => item.type === "text")?.text;
  assert.ok(text, "tool result should include agent-visible text");
  return text;
}

function taskFrom(
  result: ToolResult,
  expected: { taskId: string; kind: string; store: string; phase: string },
): Record<string, unknown> {
  assert.notEqual(result.isError, true, textFrom(result));
  const task = structuredFrom(result);
  for (const field of taskEnvelopeFields) {
    assert.ok(
      Object.hasOwn(task, field),
      `TaskEnvelope should always include ${field}`,
    );
  }
  assert.equal(task.taskId, expected.taskId);
  assert.equal(task.kind, expected.kind);
  assert.equal(task.store, expected.store);
  assert.equal(task.phase, expected.phase);
  assert.ok(
    task.source !== null && typeof task.source === "object",
    "TaskEnvelope.source should describe the bound source",
  );
  assert.ok(Array.isArray(task.availableActions));
  assert.ok(Array.isArray(task.staleReasons));
  return task;
}

function studioState(overrides: Partial<ProjectState> = {}): ProjectState {
  return {
    projectName: "Facade Test",
    revision: 4,
    durationMs: 2_030,
    voiceEngine: "unavailable",
    utterances: [
      {
        id: "utt-01",
        speaker: "Marisa",
        caption: "second form",
        spokenText: "second form",
        startMs: 0,
      },
    ],
    takes: [
      {
        id: "take-a",
        utteranceId: "utt-01",
        label: "A",
        durationMs: 2_030,
        speed: 1,
        intonation: 1,
        status: "active",
        readiness: "ready",
      },
      {
        id: "take-b",
        utteranceId: "utt-01",
        label: "B",
        durationMs: 1_840,
        speed: 1.1,
        intonation: 1,
        status: "candidate",
        readiness: "ready",
      },
    ],
    activeTakeId: "take-a",
    ...overrides,
  };
}

test("default tool inventory exposes five model facades and app-only Studio tools", async (t) => {
  const current = await connect();
  t.after(current.close);

  const listed = await current.client.listTools();
  const names = listed.tools.map((tool) => tool.name);
  for (const name of facadeNames) {
    assert.ok(names.includes(name), `${name} missing from default tool inventory`);
  }
  const inspect = listed.tools.find((tool) => tool.name === "takegraph_inspect");
  assert.equal(
    (inspect?._meta?.ui as { resourceUri?: string } | undefined)?.resourceUri,
    "ui://takegraph/editor/v1.html",
  );
  assert.deepEqual(
    (inspect?._meta?.ui as { visibility?: string[] } | undefined)?.visibility,
    ["model", "app"],
  );
  assert.equal(inspect?.annotations?.readOnlyHint, true);
  for (const name of ["takegraph_task_stage", "takegraph_task_approve"] as const) {
    const tool = listed.tools.find((candidate) => candidate.name === name);
    assert.notEqual(tool?.annotations?.destructiveHint, true, `${name} must plan only`);
  }
  for (const name of ["takegraph_task_execute", "takegraph_task_decide"] as const) {
    const tool = listed.tools.find((candidate) => candidate.name === name);
    assert.equal(tool?.annotations?.destructiveHint, true, `${name} is a write boundary`);
  }
  assert.equal(
    names.some((name) => name.startsWith("ymm4_")),
    false,
    "legacy YMM4 routes should not be model-visible by default",
  );
  for (const name of studioToolNames) {
    const tool = listed.tools.find((candidate) => candidate.name === name);
    assert.ok(tool, `${name} missing from default tool inventory`);
    assert.deepEqual(
      (tool._meta?.ui as { visibility?: string[] } | undefined)?.visibility,
      ["app"],
      `${name} must remain app-only`,
    );
  }

  const legacy = await connect({ legacyTools: true });
  t.after(legacy.close);
  const legacyNames = (await legacy.client.listTools()).tools.map((tool) => tool.name);
  for (const name of [
    "ymm4_link_describe",
    "ymm4_native_extension_stage",
    "ymm4_scene_inspection_stage",
  ]) {
    assert.ok(legacyNames.includes(name), `${name} missing with legacyTools enabled`);
  }
});

test("facade registry is bounded and preserves the newest task envelopes", () => {
  const registry = new TaskFacadeRegistry(2);
  for (const nativeId of ["one", "two", "three"]) {
    registry.remember({
      kind: "render",
      nativeId,
      store: "canonical-project",
      phase: "staged",
      source: { revision: 1 },
      revisionEffect: "none",
      availableActions: ["inspect", "execute"],
      staleReasons: [],
      details: {},
    });
  }
  assert.equal(registry.get("render:one"), undefined);
  assert.deepEqual(
    registry.list().map((task) => task.taskId),
    ["render:three", "render:two"],
  );
});

test("overview inspection reports both stores through the single read facade", async (t) => {
  const studio = studioState();
  const session = {
    snapshot() {
      return structuredClone(studio);
    },
  } as unknown as ProjectSession;
  const workflow = {
    async describe() {
      return {
        health: { status: "running", protocolVersion: 2 },
        capabilities: { capabilities: ["native_extension"] },
        snapshot: {
          projectId: "project-1",
          projectName: "YMM4 Project",
          projectPath: "C:\\projects\\facade.ymmp",
          sceneId: "scene-1",
          fps: 60,
          fingerprint: "fingerprint-1",
          managedItems: [],
          nativeExtensions: [],
          unmanagedContextCount: 0,
        },
      };
    },
    async canonicalHead() {
      return { projectId: "project-1", revision: 9 };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ session, ymm4Workflow: workflow });
  t.after(current.close);

  const inspected = await current.client.callTool({
    name: "takegraph_inspect",
    arguments: { view: "overview" },
  });
  assert.notEqual(inspected.isError, true, textFrom(inspected));
  const overview = structuredFrom(inspected);
  assert.equal(overview.view, "overview");
  assert.deepEqual(overview.studio, studio);
  assert.equal(
    (overview.canonical as { head?: { projectId?: string; revision?: number } }).head
      ?.projectId,
    "project-1",
  );
  assert.equal(
    (overview.canonical as { head?: { projectId?: string; revision?: number } }).head
      ?.revision,
    9,
  );
  assert.doesNotMatch(JSON.stringify(overview.canonical), /C:\\\\projects/i);
  assert.equal(
    Object.hasOwn(
      (overview.canonical as { snapshot?: Record<string, unknown> }).snapshot ?? {},
      "projectPath",
    ),
    false,
  );
});

test("studio_take stages and executes only with its exact plan digest", async (t) => {
  const planDigest = "a".repeat(64);
  const taskId = "patch-11111111-1111-4111-8111-111111111111";
  const facadeTaskId = `studio_take:${taskId}`;
  const calls: string[] = [];
  let state = studioState();
  const session = {
    snapshot() {
      return structuredClone(state);
    },
    stageTake(takeId: string) {
      calls.push(`stage:${takeId}`);
      assert.equal(takeId, "take-b");
      state = studioState({
        stagedPatch: {
          id: taskId,
          digest: planDigest,
          baseRevision: 4,
          takeId,
          durationDeltaMs: -190,
          movedItemCount: 5,
          status: "previewable",
        },
      });
      return structuredClone(state);
    },
    async commitPatch(input: { patchId: string; digest: string }) {
      calls.push("commit");
      assert.deepEqual(input, { patchId: taskId, digest: planDigest });
      state = studioState({
        revision: 5,
        durationMs: 1_840,
        activeTakeId: "take-b",
        takes: studioState().takes.map((take) => ({
          ...take,
          status: take.id === "take-b" ? "active" : "candidate",
        })),
      });
      return structuredClone(state);
    },
  } as unknown as ProjectSession;
  const current = await connect({ session });
  t.after(current.close);

  const staged = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: { kind: "studio_take", takeId: "take-b" },
    }),
    { taskId: facadeTaskId, kind: "studio_take", store: "studio-session", phase: "staged" },
  );
  assert.equal(staged.planDigest, planDigest);
  assert.equal(staged.approvedPlanDigest, null);
  assert.deepEqual(calls, ["stage:take-b"]);

  const wrong = await current.client.callTool({
    name: "takegraph_task_execute",
    arguments: { taskId: facadeTaskId, intent: "run", planDigest: "0".repeat(64) },
  });
  assert.equal(wrong.isError, true);
  assert.match(textFrom(wrong), /digest/i);
  assert.deepEqual(calls, ["stage:take-b"], "digest mismatch must not reach commitPatch");

  const completed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "run", planDigest },
    }),
    { taskId: facadeTaskId, kind: "studio_take", store: "studio-session", phase: "completed" },
  );
  assert.equal(completed.planDigest, planDigest);
  assert.equal(completed.approvedPlanDigest, planDigest);
  assert.deepEqual(completed.source, {
    store: "studio-session",
    projectName: "Facade Test",
    revision: 4,
  });
  assert.deepEqual(calls, ["stage:take-b", "commit"]);
});

test("native_extension keeps approval separate from execution and rejects cross-kind decisions", async (t) => {
  const taskId = "22222222-2222-4222-8222-222222222222";
  const facadeTaskId = `native_extension:${taskId}`;
  const planDigest = "b".repeat(64);
  const calls: string[] = [];
  const workflow = {
    async stageNativeExtension(input: StageNativeExtensionInput) {
      calls.push("stage");
      assert.equal(input.operations[0]?.type, "template");
      return {
        handle: taskId,
        operationId: taskId,
        taskFile: "C:\\private\\task.json",
        outputPath: "C:\\private\\output.json",
        digest: planDigest,
        plan: { warnings: [] },
      };
    },
    async approveNativeExtension(handle: string, digest: string) {
      calls.push("approve");
      assert.equal(handle, taskId);
      assert.equal(digest, planDigest);
      return { status: "approved" };
    },
    async applyNativeExtension(handle: string) {
      calls.push("apply");
      assert.equal(handle, taskId);
      return {
        status: "committed",
        revision: 10,
        canonicalReplay: false,
        verified: true,
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const staged = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "native_extension",
        operations: [
          {
            type: "template",
            entityId: "template-01",
            entityRevision: 2,
            descriptorId: "template.title",
            expectedConfigDigest: "c".repeat(64),
            expectedSchemaDigest: "d".repeat(64),
            frame: 120,
            layer: 4,
          },
        ],
      },
    }),
    { taskId: facadeTaskId, kind: "native_extension", store: "canonical-project", phase: "staged" },
  );
  assert.equal(staged.planDigest, planDigest);
  assert.deepEqual(staged.details, {
    digest: planDigest,
    plan: { warnings: [] },
  });

  const wrongKind = await current.client.callTool({
    name: "takegraph_task_decide",
    arguments: {
      taskId: facadeTaskId,
      reviewer: "human@example.test",
      decision: "accept",
      note: "not a scene-inspection task",
    },
  });
  assert.equal(wrongKind.isError, true);
  assert.match(textFrom(wrongKind), /scene_inspection|native_extension|task kind/i);
  assert.deepEqual(calls, ["stage"]);

  const wrongDigest = await current.client.callTool({
    name: "takegraph_task_approve",
    arguments: { taskId: facadeTaskId, planDigest: "0".repeat(64) },
  });
  assert.equal(wrongDigest.isError, true);
  assert.match(textFrom(wrongDigest), /digest/i);
  assert.deepEqual(calls, ["stage"], "digest mismatch must not reach approval");

  const approved = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_approve",
      arguments: { taskId: facadeTaskId, planDigest },
    }),
    { taskId: facadeTaskId, kind: "native_extension", store: "canonical-project", phase: "approved" },
  );
  assert.equal(approved.planDigest, planDigest);
  assert.equal(approved.approvedPlanDigest, planDigest);

  const applied = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "run" },
    }),
    { taskId: facadeTaskId, kind: "native_extension", store: "canonical-project", phase: "committed" },
  );
  assert.deepEqual(applied.availableActions, ["inspect"]);
  assert.deepEqual(calls, ["stage", "approve", "apply"]);
});

test("project initialization stages an explicit Save As path once, then approves and executes by opaque taskId", async (t) => {
  const nativeTaskId = "77777777-7777-4777-8777-777777777777";
  const facadeTaskId = `project_initialization:${nativeTaskId}`;
  const planDigest = "7".repeat(64);
  const pathDigest = "8".repeat(64);
  const destinationPath = "C:\\private\\new-project.ymmp";
  const calls: string[] = [];
  const source = {
    projectId: "untitled-project",
    sceneId: "scene-1",
    fingerprint: "fingerprint-before",
    projectPathPresent: false,
  };
  const workflow = {
    async stageProjectInitialization(input: {
      mode: "adopt_active" | "save_untitled";
      path?: string;
    }) {
      calls.push("stage");
      assert.deepEqual(input, { mode: "save_untitled", path: destinationPath });
      return {
        generation: 1,
        payload: {
          plan: {
            operationId: nativeTaskId,
            status: "not-the-task-status",
            planDigest,
            mode: input.mode,
            source: {
              projectId: source.projectId,
              sceneId: source.sceneId,
              fingerprint: source.fingerprint,
              projectInstanceId: "project-instance-secret",
              projectPathDigest: null,
            },
            destination: { fileName: "new-project.ymmp", pathDigest },
            capabilityDigest: "capability-internal",
          },
          destinationPath,
          bridgeRequest: {
            sourceProjectInstanceId: "project-instance-secret",
            destinationPath,
            requestDigest: "request-internal",
            bearerToken: "also-secret",
          },
          authorizationToken: "do-not-expose",
          status: "staged",
        },
        taskFile: "C:\\private\\task.json",
      };
    },
    async approveProjectInitialization(taskId: string, digest: string) {
      calls.push("approve");
      assert.equal(taskId, nativeTaskId);
      assert.equal(digest, planDigest);
      return {
        taskId,
        status: "approved",
        planDigest,
        approvedPlanDigest: digest,
        source,
        mode: "save_untitled",
        destination: { fileName: "new-project.ymmp", pathDigest },
      };
    },
    async executeProjectInitialization(taskId: string) {
      calls.push("execute");
      assert.equal(taskId, nativeTaskId);
      return {
        taskId,
        status: "initialized",
        planDigest,
        approvedPlanDigest: planDigest,
        source,
        mode: "save_untitled",
        destination: { fileName: "new-project.ymmp", pathDigest },
        result: {
          projectId: "initialized-project",
          sceneId: "scene-1",
          canonicalRevision: 0,
          outcome: "initialized",
        },
        warnings: [],
      };
    },
    async projectInitializationStatus(taskId: string) {
      calls.push("status");
      assert.equal(taskId, nativeTaskId);
      return {
        taskId,
        status: "initialized",
        planDigest,
        approvedPlanDigest: planDigest,
        source,
        mode: "save_untitled",
        destination: { fileName: "new-project.ymmp", pathDigest },
        result: {
          projectId: "initialized-project",
          sceneId: "scene-1",
          canonicalRevision: 0,
          outcome: "initialized",
        },
        executionReplayed: true,
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const staged = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "project_initialization",
        mode: "save_untitled",
        path: destinationPath,
      },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.equal(staged.planDigest, planDigest);
  assert.deepEqual(staged.availableActions, ["inspect", "approve"]);
  assert.deepEqual(staged.source, { store: "canonical-project", ...source });
  assert.deepEqual((staged.details as Record<string, unknown>).destination, {
    fileName: "new-project.ymmp",
    pathDigest,
  });
  const stagedDetails = JSON.stringify(staged.details);
  assert.doesNotMatch(stagedDetails, /C:\\\\private/i);
  assert.doesNotMatch(
    stagedDetails,
    /do-not-expose|also-secret|project-instance-secret|request-internal|capability-internal/,
  );
  assert.equal(Object.hasOwn(staged.details as object, "taskId"), false);

  const wrongDigest = await current.client.callTool({
    name: "takegraph_task_approve",
    arguments: { taskId: facadeTaskId, planDigest: "0".repeat(64) },
  });
  assert.equal(wrongDigest.isError, true);
  assert.match(textFrom(wrongDigest), /digest/i);
  assert.deepEqual(calls, ["stage"]);

  const approved = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_approve",
      arguments: { taskId: facadeTaskId, planDigest },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "approved",
    },
  );
  assert.equal(approved.approvedPlanDigest, planDigest);
  assert.deepEqual(approved.availableActions, ["inspect", "execute"]);

  const initialized = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "run" },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "initialized",
    },
  );
  assert.equal(initialized.approvedPlanDigest, planDigest);
  assert.equal(initialized.revisionEffect, "canonical-project-initialized");
  assert.deepEqual(initialized.availableActions, ["inspect"]);
  assert.deepEqual(calls, ["stage", "approve", "execute"]);

  const replayed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "revalidate" },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "initialized",
    },
  );
  assert.equal(
    replayed.revisionEffect,
    "canonical-project-replay-no-revision-change",
  );
  assert.deepEqual(calls, ["stage", "approve", "execute", "status"]);
});

test("project initialization exposes a sanitized recovery blocker", async (t) => {
  const nativeTaskId = "67676767-6767-4676-8676-676767676767";
  const facadeTaskId = `project_initialization:${nativeTaskId}`;
  const planDigest = "6".repeat(64);
  const workflow = {
    async projectInitializationStatus() {
      return {
        operationId: nativeTaskId,
        status: "recovery_required",
        planDigest,
        approvedPlanDigest: planDigest,
        mode: "save_untitled",
        source: {
          projectId: "untitled-project",
          sceneId: "scene-1",
          fingerprint: "5".repeat(64),
          projectPathPresent: false,
        },
        failureKind: "exact-retry-required",
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const inspected = taskFrom(
    await current.client.callTool({
      name: "takegraph_inspect",
      arguments: { view: "task", taskId: facadeTaskId },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "recovery_required",
    },
  );
  assert.deepEqual(inspected.availableActions, ["inspect", "execute"]);
  assert.equal(
    (inspected.details as Record<string, unknown>).failureKind,
    "exact-retry-required",
  );
});

test("project initialization adopts a saved active project without a path and has durable task inspection", async (t) => {
  const nativeTaskId = "88888888-8888-4888-8888-888888888888";
  const facadeTaskId = `project_initialization:${nativeTaskId}`;
  const planDigest = "9".repeat(64);
  const calls: string[] = [];
  const result = {
    taskId: nativeTaskId,
    status: "already_initialized",
    planDigest,
    approvedPlanDigest: planDigest,
    source: {
      projectId: "saved-project",
      sceneId: "scene-2",
      fingerprint: "fingerprint-saved",
      projectPathPresent: true,
    },
    mode: "adopt_active",
    destination: { fileName: "saved.ymmp", pathDigest: "a".repeat(64) },
    result: {
      projectId: "saved-project",
      sceneId: "scene-2",
      canonicalRevision: 0,
      outcome: "already_initialized",
    },
    warnings: [],
  };
  const workflow = {
    async stageProjectInitialization(input: unknown) {
      calls.push("stage");
      assert.deepEqual(input, { mode: "adopt_active" });
      return { ...result, status: "staged", approvedPlanDigest: undefined };
    },
    async projectInitializationStatus(taskId: string) {
      calls.push("status");
      assert.equal(taskId, nativeTaskId);
      return result;
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const staged = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: { kind: "project_initialization", mode: "adopt_active" },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.deepEqual(staged.availableActions, ["inspect", "approve"]);

  const refreshed = taskFrom(
    await current.client.callTool({
      name: "takegraph_inspect",
      arguments: { view: "task", taskId: facadeTaskId },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "already_initialized",
    },
  );
  assert.deepEqual(refreshed.availableActions, ["inspect"]);

  const restarted = await connect({ ymm4Workflow: workflow });
  t.after(restarted.close);
  const inspected = taskFrom(
    await restarted.client.callTool({
      name: "takegraph_inspect",
      arguments: { view: "task", taskId: facadeTaskId },
    }),
    {
      taskId: facadeTaskId,
      kind: "project_initialization",
      store: "canonical-project",
      phase: "already_initialized",
    },
  );
  assert.deepEqual(inspected.availableActions, ["inspect"]);
  assert.equal(inspected.revisionEffect, "none");
  assert.deepEqual(calls, ["stage", "status", "status"]);
});

test("project initialization accepts a path only for save_untitled staging", async (t) => {
  let stageCalls = 0;
  const workflow = {
    async stageProjectInitialization() {
      stageCalls += 1;
      throw new Error("invalid initialization input reached the workflow");
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  for (const arguments_ of [
    {
      kind: "project_initialization",
      mode: "adopt_active",
      path: "C:\\private\\must-not-be-consumed.ymmp",
    },
    { kind: "project_initialization", mode: "save_untitled" },
  ]) {
    const rejected = await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: arguments_,
    });
    assert.equal(rejected.isError, true);
    assert.match(textFrom(rejected), /invalid/i);
  }
  assert.equal(stageCalls, 0);
});

test("project initialization presents every durable Rust phase with fail-closed actions", async () => {
  const nativeTaskId = "99999999-9999-4999-8999-999999999999";
  const facadeTaskId = `project_initialization:${nativeTaskId}`;
  const planDigest = "b".repeat(64);
  const expectedActions = new Map<string, string[]>([
    ["staged", ["inspect", "approve"]],
    ["approved", ["inspect", "execute"]],
    ["executing", ["inspect", "execute"]],
    ["initialized", ["inspect"]],
    ["already_initialized", ["inspect"]],
    ["conflicted", ["inspect"]],
    ["stale", ["inspect"]],
    ["failed", ["inspect"]],
    ["recovery_required", ["inspect", "execute"]],
  ]);

  for (const [status, availableActions] of expectedActions) {
    let statusReads = 0;
    const workflow = {
      async projectInitializationStatus(taskId: string) {
        statusReads += 1;
        assert.equal(taskId, nativeTaskId);
        return {
          payload: {
            plan: {
              operationId: nativeTaskId,
              mode: "adopt_active",
              planDigest,
              source: {
                projectId: "saved-project",
                sceneId: "scene-1",
                fingerprint: "c".repeat(64),
                projectInstanceId: "must-not-be-presented",
                projectPathDigest: "d".repeat(64),
              },
            },
            approvedPlanDigest:
              status === "staged" ? null : planDigest,
            status,
          },
        };
      },
    } as unknown as Ymm4Workflow;
    const current = await connect({ ymm4Workflow: workflow });
    const inspected = taskFrom(
      await current.client.callTool({
        name: "takegraph_inspect",
        arguments: { view: "task", taskId: facadeTaskId },
      }),
      {
        taskId: facadeTaskId,
        kind: "project_initialization",
        store: "canonical-project",
        phase: status,
      },
    );
    assert.deepEqual(inspected.availableActions, availableActions, status);
    assert.equal(statusReads, 1, `${status} inspection must perform one status read`);
    assert.doesNotMatch(JSON.stringify(inspected.details), /must-not-be-presented/);
    await current.close();
  }
});

test("native_extension cache miss keeps inspect read-only and recovers through execute", async (t) => {
  const taskId = "23232323-2323-4232-8232-232323232323";
  const facadeTaskId = `native_extension:${taskId}`;
  const workflow = {
    async nativeExtensionStatus(handle: string) {
      assert.equal(handle, taskId);
      return {
        operationId: taskId,
        patchStatus: "approved",
        currentVerified: false,
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const inspected = await current.client.callTool({
    name: "takegraph_inspect",
    arguments: { view: "task", taskId: facadeTaskId },
  });
  assert.equal(inspected.isError, true);
  assert.match(textFrom(inspected), /takegraph_task_execute.*revalidate/i);

  const recovered = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "revalidate" },
    }),
    {
      taskId: facadeTaskId,
      kind: "native_extension",
      store: "canonical-project",
      phase: "approved",
    },
  );
  assert.equal(recovered.planDigest, null);
  assert.deepEqual(recovered.availableActions, ["inspect", "execute"]);
});

test("scene_inspection exposes capture, review, and digest-bound human decision phases", async (t) => {
  const taskId = "33333333-3333-4333-8333-333333333333";
  const facadeTaskId = `scene_inspection:${taskId}`;
  const planDigest = "e".repeat(64);
  const evidenceDigest = "f".repeat(64);
  const receiptDigest = "1".repeat(64);
  const calls: string[] = [];
  const workflow = {
    async stageSceneInspection(input: StageSceneInspectionInput) {
      calls.push("stage");
      assert.deepEqual(input.frames, [10, 20]);
      return {
        handle: taskId,
        digest: planDigest,
        receipt: { status: "staged", captures: [] },
      };
    },
    async approveSceneInspection(handle: string, digest: string) {
      calls.push("approve");
      assert.equal(handle, taskId);
      assert.equal(digest, planDigest);
      return { receipt: { status: "approved", captures: [] } };
    },
    async captureSceneInspection(handle: string) {
      calls.push("capture");
      assert.equal(handle, taskId);
      return {
        receipt: {
          status: "captured",
          captures: [],
          evidenceDigest,
          receiptDigest,
        },
        semanticDiff: { changed: false },
      };
    },
    async reviewSceneInspection(handle: string, reviewer: string) {
      calls.push("review");
      assert.equal(handle, taskId);
      assert.equal(reviewer, "reviewer-a");
      return {
        receipt: {
          status: "reviewed",
          captures: [],
          evidenceDigest,
          receiptDigest,
          reviewer,
        },
        semanticDiff: { changed: false },
      };
    },
    async decideSceneInspection(
      handle: string,
      decision: "accept" | "reject",
      note: string,
    ) {
      calls.push(`decide:${decision}`);
      assert.equal(handle, taskId);
      assert.equal(note, "images inspected manually");
      return {
        receipt: {
          status: "accepted",
          captures: [],
          evidenceDigest,
          receiptDigest,
        },
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({
    ymm4Workflow: workflow,
    sceneArtifactRoot: process.cwd(),
  });
  t.after(current.close);

  taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "scene_inspection",
        frames: [10, 20],
        expectedWidth: 1920,
        expectedHeight: 1080,
      },
    }),
    { taskId: facadeTaskId, kind: "scene_inspection", store: "canonical-project", phase: "staged" },
  );
  taskFrom(
    await current.client.callTool({
      name: "takegraph_task_approve",
      arguments: { taskId: facadeTaskId, planDigest },
    }),
    { taskId: facadeTaskId, kind: "scene_inspection", store: "canonical-project", phase: "approved" },
  );

  const captured = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "run" },
    }),
    { taskId: facadeTaskId, kind: "scene_inspection", store: "canonical-project", phase: "captured" },
  );
  assert.equal(captured.evidenceDigest, evidenceDigest);
  assert.equal(captured.receiptDigest, receiptDigest);

  const reviewed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "review", reviewer: "reviewer-a" },
    }),
    { taskId: facadeTaskId, kind: "scene_inspection", store: "canonical-project", phase: "reviewed" },
  );
  assert.equal(reviewed.evidenceDigest, evidenceDigest);

  const accepted = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_decide",
      arguments: {
        taskId: facadeTaskId,
        evidenceDigest,
        reviewer: "reviewer-a",
        decision: "accept",
        note: "images inspected manually",
      },
    }),
    { taskId: facadeTaskId, kind: "scene_inspection", store: "canonical-project", phase: "accepted" },
  );
  assert.equal(accepted.evidenceDigest, evidenceDigest);
  assert.deepEqual(calls, ["stage", "approve", "capture", "review", "decide:accept"]);
});

test("scene decision remains available with durable replay when no evidence digest is exported", async (t) => {
  const nativeTaskId = "34343434-3434-4434-8434-343434343434";
  const taskId = `scene_inspection:${nativeTaskId}`;
  const planDigest = "a".repeat(64);
  const workflow = {
    async stageSceneInspection() {
      return { handle: nativeTaskId, digest: planDigest, receipt: { status: "staged", captures: [] } };
    },
    async approveSceneInspection() {
      return { receipt: { status: "approved", captures: [] } };
    },
    async captureSceneInspection() {
      return { receipt: { status: "captured", captures: [] } };
    },
    async reviewSceneInspection(_handle: string, reviewer: string) {
      return { receipt: { status: "reviewed", captures: [], reviewer } };
    },
    async decideSceneInspection() {
      return { receipt: { status: "accepted", captures: [] } };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  await current.client.callTool({
    name: "takegraph_task_stage",
    arguments: {
      kind: "scene_inspection",
      frames: [10],
      expectedWidth: 1920,
      expectedHeight: 1080,
    },
  });
  await current.client.callTool({
    name: "takegraph_task_approve",
    arguments: { taskId, planDigest },
  });
  await current.client.callTool({
    name: "takegraph_task_execute",
    arguments: { taskId, intent: "run" },
  });
  const reviewed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "review", reviewer: "reviewer-a" },
    }),
    { taskId, kind: "scene_inspection", store: "canonical-project", phase: "reviewed" },
  );
  assert.equal(reviewed.evidenceDigest, null);
  assert.deepEqual(reviewed.availableActions, ["inspect", "decide"]);

  taskFrom(
    await current.client.callTool({
      name: "takegraph_task_decide",
      arguments: {
        taskId,
        reviewer: "reviewer-a",
        decision: "accept",
        note: "durable evidence replayed",
      },
    }),
    { taskId, kind: "scene_inspection", store: "canonical-project", phase: "accepted" },
  );
});

test("scene attachment failure does not turn a completed capture into a facade error", async (t) => {
  const nativeTaskId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  const facadeTaskId = `scene_inspection:${nativeTaskId}`;
  const planDigest = "d".repeat(64);
  const missingPng = path.join(process.cwd(), ".takegraph-missing-scene.png");
  const workflow = {
    async stageSceneInspection() {
      return {
        handle: nativeTaskId,
        digest: planDigest,
        receipt: { status: "staged", captures: [] },
      };
    },
    async approveSceneInspection() {
      return { receipt: { status: "approved", captures: [] } };
    },
    async captureSceneInspection() {
      return {
        receipt: {
          status: "captured",
          captures: [
            {
              artifactPath: missingPng,
              sha256: "0".repeat(64),
              mediaType: "image/png",
              width: 1,
              height: 1,
            },
          ],
        },
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({
    ymm4Workflow: workflow,
    sceneArtifactRoot: process.cwd(),
  });
  t.after(current.close);

  await current.client.callTool({
    name: "takegraph_task_stage",
    arguments: {
      kind: "scene_inspection",
      frames: [1],
      expectedWidth: 1920,
      expectedHeight: 1080,
    },
  });
  await current.client.callTool({
    name: "takegraph_task_approve",
    arguments: { taskId: facadeTaskId, planDigest },
  });
  const result = await current.client.callTool({
    name: "takegraph_task_execute",
    arguments: { taskId: facadeTaskId, intent: "run" },
  });
  const captured = taskFrom(result, {
    taskId: facadeTaskId,
    kind: "scene_inspection",
    store: "canonical-project",
    phase: "captured",
  });
  assert.equal(captured.phase, "captured");
  assert.match(textFrom(result), /attachments were omitted/i);
  assert.equal(
    (result.content as Array<{ type?: string }>).some((item) => item.type === "image"),
    false,
  );
});

test("scene inspection exposes timeline composition while marking visual geometry unavailable", async (t) => {
  const workflow = {
    async describe() {
      return {
        health: { status: "running", protocolVersion: 2 },
        capabilities: { capabilities: [] },
        snapshot: {
          projectId: "project-composition",
          projectName: "Composition Test",
          projectPath: "C:\\projects\\composition.ymmp",
          sceneId: "scene-composition",
          fps: 60,
          fingerprint: "fingerprint-composition",
          managedItems: [
            {
              entityId: "voice-01",
              realizationId: "voice-realization-01",
              kind: "voice",
              frame: 120,
              length: 180,
              layer: 4,
              text: "visible timeline text",
              // A bridge-specific field must not be mistaken for evaluated geometry.
              bounds: { x: 10, y: 20, width: 300, height: 80 },
            },
          ],
          nativeExtensions: [],
          unmanagedContextCount: 0,
        },
      };
    },
    async canonicalHead() {
      return { projectId: "project-composition", revision: 12 };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const inspected = await current.client.callTool({
    name: "takegraph_inspect",
    arguments: { view: "scene", include: ["composition"] },
  });
  assert.notEqual(inspected.isError, true, textFrom(inspected));
  assert.match(textFrom(inspected), /geometry unavailable|not inferred/i);

  const payload = structuredFrom(inspected);
  const composition = payload.composition as {
    schemaVersion?: string;
    authority?: string;
    availability?: string;
    evaluatedFrame?: unknown;
    viewport?: { availability?: string };
    source?: Record<string, unknown>;
    elements?: Array<Record<string, unknown>>;
    unavailableFields?: string[];
    note?: string;
  };
  assert.equal(composition.schemaVersion, "takegraph.composition-observation.v0");
  assert.equal(composition.authority, "derived-read-only-observation");
  assert.equal(composition.availability, "timeline_only");
  assert.equal(composition.evaluatedFrame, null);
  assert.equal(composition.viewport?.availability, "unavailable");
  assert.deepEqual(composition.source, {
    projectId: "project-composition",
    sceneId: "scene-composition",
    fingerprint: "fingerprint-composition",
    fps: 60,
    canonicalRevision: 12,
  });
  assert.deepEqual(composition.elements, [
    {
      elementId: "voice-realization-01",
      stability: "realization_identity",
      observation: "timeline_placement",
      entityId: "voice-01",
      realizationId: "voice-realization-01",
      kind: "voice",
      frame: 120,
      length: 180,
      layer: 4,
      text: "visible timeline text",
    },
  ]);
  assert.ok(composition.unavailableFields?.includes("bounds"));
  assert.ok(composition.unavailableFields?.includes("position"));
  assert.match(composition.note ?? "", /not inferred/i);
  assert.equal(
    Object.hasOwn(composition.elements?.[0] ?? {}, "bounds"),
    false,
    "opaque bridge data must not be presented as evaluated geometry",
  );
});

test("scene inspection returns a source-bound current-frame composition without adding a tool", async (t) => {
  const calls: string[] = [];
  const workflow = {
    async composition() {
      calls.push("composition");
      return {
        schemaVersion: 1,
        projectId: "project-current-frame",
        sceneId: "scene-current-frame",
        sourceFingerprint: "fingerprint-current-frame",
        fps: 60,
        frame: 150,
        viewport: {
          availability: "unavailable",
          width: null,
          height: null,
        },
        elements: [
          {
            elementId: "voice-current-frame",
            stability: "realization_identity",
            kind: "voice",
            frame: 120,
            layer: 4,
            length: 90,
            active: true,
            selected: false,
            text: "current caption",
            visual: {
              availability: "unavailable",
              x: null,
              y: null,
              width: null,
              height: null,
            },
          },
        ],
        completeness: "partial",
        unavailableFields: ["elements[].selected", "elements[].visual", "viewport"],
      };
    },
    async describe() {
      calls.push("describe");
      return {
        health: { status: "running", protocolVersion: 2 },
        capabilities: { capabilities: ["scene_composition_current"] },
        snapshot: {
          projectId: "project-current-frame",
          projectName: "Current Frame",
          projectPath: "C:\\projects\\current-frame.ymmp",
          sceneId: "scene-current-frame",
          fps: 60,
          fingerprint: "fingerprint-current-frame",
          managedItems: [],
          nativeExtensions: [],
          unmanagedContextCount: 0,
        },
      };
    },
    async canonicalHead() {
      calls.push("head");
      return { projectId: "project-current-frame", revision: 21 };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const inspected = await current.client.callTool({
    name: "takegraph_inspect",
    arguments: { view: "scene" },
  });
  assert.notEqual(inspected.isError, true, textFrom(inspected));
  assert.match(textFrom(inspected), /source-bound current-frame/i);
  assert.deepEqual(calls, ["composition", "describe", "head"]);

  const composition = structuredFrom(inspected).composition as Record<string, unknown>;
  assert.equal(composition.schemaVersion, 1);
  assert.equal(composition.authority, "target-derived-read-only-observation");
  assert.equal(composition.availability, "current_frame_partial");
  assert.equal(composition.evaluatedFrame, 150);
  assert.equal(composition.observationStatus, "source_bound");
  assert.deepEqual(composition.source, {
    projectId: "project-current-frame",
    sceneId: "scene-current-frame",
    fingerprint: "fingerprint-current-frame",
    fps: 60,
    canonicalRevision: 21,
  });
  assert.equal(
    ((composition.elements as Array<Record<string, unknown>>)[0]?.visual as Record<string, unknown>)
      .availability,
    "unavailable",
  );
});

test("scene inspection rejects a stale current-frame composition binding", async (t) => {
  const workflow = {
    async composition() {
      return {
        schemaVersion: 1,
        projectId: "project-binding",
        sceneId: "scene-binding",
        sourceFingerprint: "stale-fingerprint",
        fps: 60,
        frame: 20,
        viewport: { availability: "available", width: 1920, height: 1080 },
        elements: [
          {
            elementId: "stale-element",
            visual: { availability: "available", x: 10, y: 10, width: 100, height: 100 },
          },
        ],
        completeness: "complete",
        unavailableFields: [],
      };
    },
    async describe() {
      return {
        health: { status: "running", protocolVersion: 2 },
        capabilities: { capabilities: ["scene_composition_current"] },
        snapshot: {
          projectId: "project-binding",
          projectName: "Binding",
          projectPath: "C:\\projects\\binding.ymmp",
          sceneId: "scene-binding",
          fps: 60,
          fingerprint: "fresh-fingerprint",
          managedItems: [],
          nativeExtensions: [],
          unmanagedContextCount: 0,
        },
      };
    },
    async canonicalHead() {
      return { projectId: "project-binding", revision: 2 };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const inspected = await current.client.callTool({
    name: "takegraph_inspect",
    arguments: { view: "scene" },
  });
  assert.notEqual(inspected.isError, true, textFrom(inspected));
  const composition = structuredFrom(inspected).composition as {
    availability?: string;
    observationStatus?: string;
    observationError?: string;
    elements?: Array<Record<string, unknown>>;
  };
  assert.equal(composition.availability, "timeline_only");
  assert.equal(composition.observationStatus, "current_frame_unavailable");
  assert.match(composition.observationError ?? "", /sourceFingerprint/);
  assert.deepEqual(composition.elements, []);
});

test("portable_voice and native_voice stage ordered item batches through one task each", async (t) => {
  const portableInputs: StageYmm4Request[] = [];
  const nativeInputs: StageNativeVoiceRequest[] = [];
  const workflow = {
    async stage(input: StageYmm4Request) {
      portableInputs.push(input);
      return {
        handle: "portable-batch-01",
        digest: "4".repeat(64),
        baseRevision: 20,
        itemCount: 2,
        placements: [
          { entityId: "portable-01", frame: 0, length: 60, audioLayer: 20, captionLayer: 21 },
          { entityId: "portable-02", frame: 60, length: 60, audioLayer: 20, captionLayer: 21 },
        ],
        voices: [
          {
            entityId: "portable-01",
            artifact: {
              audio_path: "C:\\private\\portable-01.wav",
              query_path: "C:\\private\\portable-01.json",
            },
          },
        ],
      };
    },
    async stageNativeVoice(input: StageNativeVoiceRequest) {
      nativeInputs.push(input);
      return {
        handle: "native-batch-01",
        digest: "5".repeat(64),
        baseRevision: 20,
        itemCount: 2,
        realizationIds: [
          "11111111-1111-4111-8111-111111111111",
          "22222222-2222-4222-8222-222222222222",
        ],
        placements: [
          { entityId: "native-01", frame: 120, layer: 2, maxLength: 180 },
          { entityId: "native-02", frame: 300, layer: 3, maxLength: 240 },
        ],
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const portable = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "portable_voice",
        items: [
          {
            entityId: "portable-01",
            caption: "first",
            spokenText: "first reading",
            frame: 0,
          },
          {
            entityId: "portable-02",
            caption: "second",
            spokenText: "second reading",
            speaker: "speaker-two",
            style: "style-two",
            frame: 60,
          },
        ],
      },
    }),
    {
      taskId: "portable_voice:portable-batch-01",
      kind: "portable_voice",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.equal((portable.details as { itemCount?: number }).itemCount, 2);
  assert.doesNotMatch(JSON.stringify(portable.details), /C:\\\\private|audio_path|query_path/);
  assert.deepEqual(portableInputs, [
    {
      items: [
        {
          entityId: "portable-01",
          caption: "first",
          spokenText: "first reading",
          speaker: "春日部つむぎ",
          style: "ノーマル",
          frame: 0,
          audioLayer: 20,
          captionLayer: 21,
        },
        {
          entityId: "portable-02",
          caption: "second",
          spokenText: "second reading",
          speaker: "speaker-two",
          style: "style-two",
          frame: 60,
          audioLayer: 20,
          captionLayer: 21,
        },
      ],
    },
  ]);

  const native = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "native_voice",
        items: [
          {
            entityId: "native-01",
            displayText: "first",
            spokenText: "first",
            characterName: "character-one",
            frame: 120,
            layer: 2,
            maxLength: 180,
          },
          {
            entityId: "native-02",
            displayText: "second",
            spokenText: "second",
            characterName: "character-two",
            frame: 300,
            layer: 3,
            maxLength: 240,
          },
        ],
      },
    }),
    {
      taskId: "native_voice:native-batch-01",
      kind: "native_voice",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.equal((native.details as { itemCount?: number }).itemCount, 2);
  assert.equal(nativeInputs.length, 1);
  assert.equal("items" in nativeInputs[0]!, true);

});

test("timeline_edit stages ordered heterogeneous creates as one exact-digest task", async (t) => {
  const handle = "timeline-edit-01";
  const planDigest = "6".repeat(64);
  const calls: string[] = [];
  const stagedInputs: StageTimelineEditInput[] = [];
  const workflow = {
    async stageTimelineEdit(input: StageTimelineEditInput) {
      calls.push("stage");
      stagedInputs.push(input);
      return {
        handle,
        digest: planDigest,
        baseRevision: 31,
        operationCount: input.operations.length,
        operationKinds: input.operations.map((operation) => operation.op),
        manifestPath: "C:\\private\\timeline-edit.manifest.json",
      };
    },
    async commitTimelineEdit(nativeHandle: string, digest: string) {
      calls.push("commit");
      assert.equal(nativeHandle, handle);
      assert.equal(digest, planDigest);
      return {
        status: "completed",
        baseRevision: 31,
        revision: 32,
        canonicalReplay: false,
      };
    },
    async timelineEditStatus(nativeHandle: string) {
      calls.push("status");
      assert.equal(nativeHandle, handle);
      return {
        patchStatus: "committed",
        baseRevision: 31,
        digest: planDigest,
        approvedDigest: planDigest,
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const operations = [
    {
      op: "portable_voice_create",
      entityId: "voice-portable-01",
      caption: "caption",
      spokenText: "spoken",
      frame: 0,
    },
    {
      op: "native_voice_create",
      entityId: "voice-native-01",
      displayText: "native",
      spokenText: "native",
      characterName: "character",
      frame: 120,
      layer: 3,
      maxLength: 240,
    },
  ];
  const staged = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: { kind: "timeline_edit", operations },
    }),
    {
      taskId: `timeline_edit:${handle}`,
      kind: "timeline_edit",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.equal(staged.planDigest, planDigest);
  assert.deepEqual(staged.availableActions, ["inspect", "execute"]);
  assert.doesNotMatch(JSON.stringify(staged.details), /C:\\\\private|manifestPath/);
  assert.deepEqual(stagedInputs, [
    {
      operations: [
        {
          op: "portable_voice_create",
          entityId: "voice-portable-01",
          caption: "caption",
          spokenText: "spoken",
          speaker: "春日部つむぎ",
          style: "ノーマル",
          frame: 0,
          audioLayer: 20,
          captionLayer: 21,
        },
        {
          op: "native_voice_create",
          entityId: "voice-native-01",
          displayText: "native",
          spokenText: "native",
          characterName: "character",
          frame: 120,
          layer: 3,
          maxLength: 240,
        },
      ],
      maxChangedEntities: undefined,
    },
  ]);

  const separateApproval = await current.client.callTool({
    name: "takegraph_task_approve",
    arguments: {
      taskId: `timeline_edit:${handle}`,
      planDigest,
    },
  });
  assert.equal(separateApproval.isError, true);
  assert.match(textFrom(separateApproval), /no separate approve|execute/i);
  assert.deepEqual(calls, ["stage"]);

  const wrongDigest = await current.client.callTool({
    name: "takegraph_task_execute",
    arguments: {
      taskId: `timeline_edit:${handle}`,
      planDigest: "7".repeat(64),
    },
  });
  assert.equal(wrongDigest.isError, true);
  assert.deepEqual(calls, ["stage"]);

  const committed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: {
        taskId: `timeline_edit:${handle}`,
        planDigest,
      },
    }),
    {
      taskId: `timeline_edit:${handle}`,
      kind: "timeline_edit",
      store: "canonical-project",
      phase: "completed",
    },
  );
  assert.equal(committed.approvedPlanDigest, planDigest);
  assert.equal(committed.revisionEffect, "canonical-project-revision-advanced");

  const revalidated = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: {
        taskId: `timeline_edit:${handle}`,
        intent: "revalidate",
      },
    }),
    {
      taskId: `timeline_edit:${handle}`,
      kind: "timeline_edit",
      store: "canonical-project",
      phase: "committed",
    },
  );
  assert.deepEqual(revalidated.availableActions, ["inspect"]);
  assert.deepEqual(calls, ["stage", "commit", "status"]);
});

test("timeline_edit rejects unsupported mutations and invalid batches before workflow dispatch", async (t) => {
  let stageCalls = 0;
  const workflow = {
    async stageTimelineEdit() {
      stageCalls += 1;
      return { handle: "must-not-stage", digest: "8".repeat(64) };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const unsupported = await current.client.callTool({
    name: "takegraph_task_stage",
    arguments: {
      kind: "timeline_edit",
      operations: [
        {
          op: "native_voice_update",
          entityId: "voice-01",
          realizationId: "11111111-1111-4111-8111-111111111111",
        },
      ],
    },
  });
  assert.equal(unsupported.isError, true);

  const duplicate = await current.client.callTool({
    name: "takegraph_task_stage",
    arguments: {
      kind: "timeline_edit",
      operations: [
        {
          op: "portable_voice_create",
          entityId: "duplicate",
          caption: "one",
          spokenText: "one",
          frame: 0,
        },
        {
          op: "native_voice_create",
          entityId: "duplicate",
          displayText: "two",
          spokenText: "two",
          characterName: "character",
          frame: 60,
          layer: 1,
          maxLength: 120,
        },
      ],
    },
  });
  assert.equal(duplicate.isError, true);
  assert.match(textFrom(duplicate), /entityIds must be unique/i);

  const tooLarge = await current.client.callTool({
    name: "takegraph_task_stage",
    arguments: {
      kind: "timeline_edit",
      operations: Array.from({ length: 129 }, (_, index) => ({
        op: "portable_voice_create",
        entityId: `voice-${index}`,
        caption: "caption",
        spokenText: "spoken",
        frame: index,
      })),
    },
  });
  assert.equal(tooLarge.isError, true);
  assert.equal(stageCalls, 0);
});

test("timeline_edit cache miss recovers through durable revalidation instead of requiring re-stage", async (t) => {
  const handle = "timeline-recover-01";
  const taskId = `timeline_edit:${handle}`;
  const planDigest = "9".repeat(64);
  let statusCalls = 0;
  let durableStatus: "previewable" | "approved" = "previewable";
  const workflow = {
    async timelineEditStatus(nativeHandle: string) {
      statusCalls += 1;
      assert.equal(nativeHandle, handle);
      return {
        patchStatus: durableStatus,
        baseRevision: 41,
        digest: planDigest,
        approvedDigest: durableStatus === "approved" ? planDigest : null,
        receipt: null,
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const uncached = await current.client.callTool({
    name: "takegraph_inspect",
    arguments: { view: "task", taskId },
  });
  assert.equal(uncached.isError, true);
  assert.match(textFrom(uncached), /execute.*intent=revalidate/i);
  assert.doesNotMatch(textFrom(uncached), /re-stage it to recover/i);
  assert.equal(statusCalls, 0, "inspection must not trigger durable revalidation");

  const recovered = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "revalidate" },
    }),
    {
      taskId,
      kind: "timeline_edit",
      store: "canonical-project",
      phase: "previewable",
    },
  );
  assert.equal(recovered.planDigest, planDigest);
  assert.equal(recovered.approvedPlanDigest, null);
  assert.deepEqual(recovered.availableActions, ["inspect", "execute"]);
  assert.equal(statusCalls, 1);

  durableStatus = "approved";
  const retryable = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "revalidate" },
    }),
    {
      taskId,
      kind: "timeline_edit",
      store: "canonical-project",
      phase: "approved",
    },
  );
  assert.equal(retryable.approvedPlanDigest, planDigest);
  assert.deepEqual(retryable.availableActions, ["inspect", "execute"]);
  assert.equal(statusCalls, 2);
});

test("timeline_edit durable failure receipts override the approved patch phase", async (t) => {
  const handle = "timeline-recovery-required-01";
  const taskId = `timeline_edit:${handle}`;
  const planDigest = "a".repeat(64);
  let receiptStatus: "recovery_required" | "rolled_back" = "recovery_required";
  const workflow = {
    async timelineEditStatus() {
      return {
        patchStatus: "approved",
        baseRevision: 50,
        digest: planDigest,
        approvedDigest: planDigest,
        receiptStatus,
        receipt: { status: receiptStatus },
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const recovering = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "revalidate" },
    }),
    {
      taskId,
      kind: "timeline_edit",
      store: "canonical-project",
      phase: "recovery_required",
    },
  );
  assert.deepEqual(recovering.availableActions, ["inspect"]);

  receiptStatus = "rolled_back";
  const rolledBack = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "revalidate" },
    }),
    {
      taskId,
      kind: "timeline_edit",
      store: "canonical-project",
      phase: "rolled_back",
    },
  );
  assert.deepEqual(rolledBack.availableActions, ["inspect"]);
});

test("portable_voice remains an atomic exact-digest execute and supports revalidation", async (t) => {
  const nativeTaskId = "portable-task-01";
  const facadeTaskId = `portable_voice:${nativeTaskId}`;
  const planDigest = "2".repeat(64);
  const calls: string[] = [];
  const workflow = {
    async stage(input: StageYmm4Input) {
      calls.push("stage");
      assert.deepEqual(input, {
        entityId: "voice-entity-01",
        caption: "caption",
        spokenText: "spoken text",
        speaker: "春日部つむぎ",
        style: "ノーマル",
        frame: 240,
        audioLayer: 20,
        captionLayer: 21,
      });
      return {
        handle: nativeTaskId,
        digest: planDigest,
        baseRevision: 20,
        impact: { changedEntities: 2 },
      };
    },
    async commit(handle: string, digest: string) {
      calls.push("commit");
      assert.equal(handle, nativeTaskId);
      assert.equal(digest, planDigest);
      return {
        status: "completed",
        baseRevision: 20,
        revision: 21,
        canonicalReplay: true,
      };
    },
    async verify(handle: string) {
      calls.push("verify");
      assert.equal(handle, nativeTaskId);
      return { status: "verified", handle, revision: 21 };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const staged = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "portable_voice",
        entityId: "voice-entity-01",
        caption: "caption",
        spokenText: "spoken text",
        frame: 240,
      },
    }),
    {
      taskId: facadeTaskId,
      kind: "portable_voice",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.equal(staged.planDigest, planDigest);
  assert.deepEqual(staged.availableActions, ["inspect", "execute"]);

  const separateApproval = await current.client.callTool({
    name: "takegraph_task_approve",
    arguments: { taskId: facadeTaskId, planDigest },
  });
  assert.equal(separateApproval.isError, true);
  assert.match(textFrom(separateApproval), /no separate approve|execute/i);
  assert.deepEqual(calls, ["stage"]);

  const wrongDigest = await current.client.callTool({
    name: "takegraph_task_execute",
    arguments: {
      taskId: facadeTaskId,
      intent: "run",
      planDigest: "3".repeat(64),
    },
  });
  assert.equal(wrongDigest.isError, true);
  assert.match(textFrom(wrongDigest), /digest mismatch|exact bound digest/i);
  assert.deepEqual(calls, ["stage"], "wrong digest must not reach the canonical commit");

  const completed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "run", planDigest },
    }),
    {
      taskId: facadeTaskId,
      kind: "portable_voice",
      store: "canonical-project",
      phase: "completed",
    },
  );
  assert.equal(completed.planDigest, planDigest);
  assert.equal(completed.approvedPlanDigest, planDigest);
  assert.equal(
    completed.revisionEffect,
    "canonical-project-replay-no-revision-change",
  );
  assert.deepEqual(completed.source, {
    store: "canonical-project",
    revision: 20,
  });

  const cached = taskFrom(
    await current.client.callTool({
      name: "takegraph_inspect",
      arguments: { view: "task", taskId: facadeTaskId },
    }),
    {
      taskId: facadeTaskId,
      kind: "portable_voice",
      store: "canonical-project",
      phase: "completed",
    },
  );
  assert.equal(cached.approvedPlanDigest, planDigest);
  assert.deepEqual(calls, ["stage", "commit"], "cached inspection must stay read-only");

  const refreshed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "revalidate" },
    }),
    {
      taskId: facadeTaskId,
      kind: "portable_voice",
      store: "canonical-project",
      phase: "verified",
    },
  );
  assert.equal(refreshed.planDigest, planDigest);
  assert.equal(refreshed.approvedPlanDigest, planDigest);
  assert.deepEqual(refreshed.availableActions, ["inspect"]);
  assert.deepEqual(calls, ["stage", "commit", "verify"]);
});

test("render cancellation stays on the consolidated execute route", async (t) => {
  const checkpointOperationId = "44444444-4444-4444-8444-444444444444";
  const nativeTaskId = "55555555-5555-4555-8555-555555555555";
  const facadeTaskId = `render:${nativeTaskId}`;
  const calls: string[] = [];
  const workflow = {
    async stageRender(input: StageRenderInput) {
      calls.push("stage");
      assert.deepEqual(input, {
        checkpointOperationId,
        profile: "youtube-1080p",
        outputPath: "C:\\exports\\episode.mp4",
        overwrite: false,
      });
      return {
        request: { taskId: nativeTaskId, status: "staged" },
        sourceRevision: 30,
      };
    },
    async cancelRender(taskId: string) {
      calls.push("cancel");
      assert.equal(taskId, nativeTaskId);
      return { taskId, status: "cancelled" };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const staged = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "render",
        checkpointOperationId,
        profile: "youtube-1080p",
        outputPath: "C:\\exports\\episode.mp4",
      },
    }),
    {
      taskId: facadeTaskId,
      kind: "render",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.deepEqual(staged.availableActions, ["inspect", "execute", "cancel"]);

  const cancelled = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: facadeTaskId, intent: "cancel" },
    }),
    {
      taskId: facadeTaskId,
      kind: "render",
      store: "canonical-project",
      phase: "cancelled",
    },
  );
  assert.deepEqual(cancelled.availableActions, ["inspect"]);
  assert.equal(cancelled.revisionEffect, "none");
  assert.deepEqual(calls, ["stage", "cancel"]);
});

test("reconciliation materializes public child tasks and hands re-export to a concrete workflow", async (t) => {
  const reportDigest = "4".repeat(64);
  const planDigest = "5".repeat(64);
  const entryId = "6".repeat(64);
  const detachChildId = "7".repeat(64);
  const reExportChildId = "8".repeat(64);
  const importChildId = "9".repeat(64);
  const detachPatchDigest = "a".repeat(64);
  const importPatchDigest = "b".repeat(64);
  const downstreamHandle = "66666666-6666-4666-8666-666666666666";
  const downstreamDigest = "c".repeat(64);
  const reconciliationTaskId = `reconciliation:${reportDigest}`;
  const detachTaskId = `reconciliation_detach:${detachChildId}`;
  const reExportTaskId = `reconciliation_re_export:${reExportChildId}`;
  const importTaskId = `reconciliation_import:${importChildId}`;
  const calls: string[] = [];

  const childStatuses: Record<string, unknown> = {
    [detachChildId]: {
      payload: {
        type: "metadata_detach",
        task: {
          status: "preview_ready",
          patch: {
            status: "previewable",
            digest: detachPatchDigest,
            approvedDigest: null,
          },
        },
      },
    },
    [reExportChildId]: {
      payload: {
        type: "canonical_re_export",
        task: { status: "awaiting_manifest" },
      },
    },
    [importChildId]: {
      payload: {
        type: "import_patch",
        task: {
          patch: {
            status: "previewable",
            digest: importPatchDigest,
            approvedDigest: null,
          },
        },
      },
    },
  };

  const workflow = {
    async reconciliationReport() {
      calls.push("report");
      return {
        payload: {
          report: { reportDigest },
          status: "report_ready",
        },
      };
    },
    async previewReconciliation(
      receivedReportDigest: string,
      decisions: ReconciliationDecisionInput[],
    ) {
      calls.push("preview");
      assert.equal(receivedReportDigest, reportDigest);
      assert.deepEqual(decisions, [
        { entryId, choice: "detach_from_take_graph" },
      ]);
      return {
        payload: {
          preview: { approvalDigest: planDigest },
          status: "decision_preview_ready",
        },
      };
    },
    async applyReconciliation(
      receivedReportDigest: string,
      receivedPlanDigest: string,
    ) {
      calls.push("apply");
      assert.equal(receivedReportDigest, reportDigest);
      assert.equal(receivedPlanDigest, planDigest);
      return {
        payload: {
          status: "actions_materialized",
          materializedChildren: [
            { childTaskId: detachChildId, kind: "metadata_detach" },
            { childTaskId: reExportChildId, kind: "canonical_re_export" },
            { childTaskId: importChildId, kind: "import_patch" },
            { kind: "metadata_detach" },
          ],
        },
      };
    },
    async reconciliationChildStatus(childTaskId: string) {
      calls.push(`status:${childTaskId}`);
      const status = childStatuses[childTaskId];
      assert.ok(status, `unexpected reconciliation child ${childTaskId}`);
      return structuredClone(status);
    },
    async approveReconciliationDetach(
      childTaskId: string,
      receivedPlanDigest: string,
    ) {
      calls.push("detach-approve");
      assert.equal(childTaskId, detachChildId);
      assert.equal(receivedPlanDigest, detachPatchDigest);
      return {
        payload: {
          type: "metadata_detach",
          task: {
            status: "approved",
            patch: {
              status: "approved",
              digest: detachPatchDigest,
              approvedDigest: detachPatchDigest,
            },
          },
        },
      };
    },
    async executeReconciliationDetach(childTaskId: string) {
      calls.push("detach-execute");
      assert.equal(childTaskId, detachChildId);
      return {
        canonicalReplay: false,
        payload: {
          type: "metadata_detach",
          task: {
            status: "verified",
            patch: {
              status: "committed",
              digest: detachPatchDigest,
              approvedDigest: detachPatchDigest,
            },
          },
        },
      };
    },
    async dispatchReconciliationReExport(
      childTaskId: string,
      manifest: Record<string, unknown>,
    ) {
      calls.push("re-export-stage");
      assert.equal(childTaskId, reExportChildId);
      assert.deepEqual(manifest, {
        route: "native_voice_mutation",
        mutations: [],
      });
      return {
        handle: downstreamHandle,
        downstreamRoute: "native_voice_mutation",
        digest: downstreamDigest,
        payload: {
          type: "canonical_re_export",
          task: {
            status: "preview_ready",
            downstreamPreview: {
              route: "native_voice_mutation",
              preview: {
                patch: {
                  status: "previewable",
                  digest: downstreamDigest,
                  approvedDigest: null,
                },
              },
            },
          },
        },
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const report = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: { kind: "reconciliation", mode: "report" },
    }),
    {
      taskId: reconciliationTaskId,
      kind: "reconciliation",
      store: "canonical-project",
      phase: "report_ready",
    },
  );
  assert.deepEqual(report.availableActions, ["inspect", "stage"]);

  const preview = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "reconciliation",
        mode: "preview",
        taskId: report.taskId,
        decisions: [{ entryId, choice: "detach_from_take_graph" }],
      },
    }),
    {
      taskId: reconciliationTaskId,
      kind: "reconciliation",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.equal(preview.planDigest, planDigest);
  assert.deepEqual(preview.availableActions, ["inspect", "execute"]);

  const applied = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: {
        taskId: preview.taskId,
        intent: "run",
        planDigest: preview.planDigest,
      },
    }),
    {
      taskId: reconciliationTaskId,
      kind: "reconciliation",
      store: "canonical-project",
      phase: "actions_materialized",
    },
  );
  assert.equal(applied.approvedPlanDigest, planDigest);
  const childTasks = (
    applied.details as {
      childTasks?: Array<Record<string, unknown>>;
      presentationWarnings?: string[];
    }
  ).childTasks;
  const presentationWarnings = (
    applied.details as { presentationWarnings?: string[] }
  ).presentationWarnings;
  assert.ok(childTasks, "materialized reconciliation children must be exposed");
  assert.equal(presentationWarnings?.length, 1);
  assert.match(presentationWarnings?.[0] ?? "", /childTaskId is missing/);
  assert.deepEqual(
    childTasks.map((task) => ({
      taskId: task.taskId,
      kind: task.kind,
      phase: task.phase,
      planDigest: task.planDigest,
    })),
    [
      {
        taskId: detachTaskId,
        kind: "reconciliation_detach",
        phase: "materialized",
        planDigest: null,
      },
      {
        taskId: reExportTaskId,
        kind: "reconciliation_re_export",
        phase: "materialized",
        planDigest: null,
      },
      {
        taskId: importTaskId,
        kind: "reconciliation_import",
        phase: "materialized",
        planDigest: null,
      },
    ],
  );
  assert.deepEqual(childTasks[0]?.availableActions, ["inspect", "execute"]);
  assert.deepEqual(childTasks[1]?.availableActions, ["inspect", "stage"]);
  assert.deepEqual(childTasks[2]?.availableActions, ["inspect"]);

  const hydratedDetach = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: detachTaskId, intent: "revalidate" },
    }),
    {
      taskId: detachTaskId,
      kind: "reconciliation_detach",
      store: "canonical-project",
      phase: "preview_ready",
    },
  );
  assert.equal(hydratedDetach.planDigest, detachPatchDigest);

  const approvedDetach = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_approve",
      arguments: {
        taskId: detachTaskId,
        planDigest: detachPatchDigest,
      },
    }),
    {
      taskId: detachTaskId,
      kind: "reconciliation_detach",
      store: "canonical-project",
      phase: "approved",
    },
  );
  assert.equal(approvedDetach.approvedPlanDigest, detachPatchDigest);

  const completedDetach = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId: detachTaskId, intent: "run" },
    }),
    {
      taskId: detachTaskId,
      kind: "reconciliation_detach",
      store: "canonical-project",
      phase: "completed",
    },
  );
  assert.equal(completedDetach.revisionEffect, "canonical-project-revision-advanced");

  const downstream = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_stage",
      arguments: {
        kind: "reconciliation_re_export",
        taskId: reExportTaskId,
        manifest: { route: "native_voice_mutation", mutations: [] },
      },
    }),
    {
      taskId: `native_voice_mutation:${downstreamHandle}`,
      kind: "native_voice_mutation",
      store: "canonical-project",
      phase: "staged",
    },
  );
  assert.equal(downstream.planDigest, downstreamDigest);
  assert.deepEqual(downstream.availableActions, ["inspect", "execute"]);
  assert.deepEqual(downstream.source, {
    store: "canonical-project",
    reconciliationChildTaskId: reExportChildId,
  });

  assert.deepEqual(calls, [
    "report",
    "preview",
    "apply",
    `status:${detachChildId}`,
    "detach-approve",
    "detach-execute",
    "re-export-stage",
  ]);
});

test("reconciliation detach terminal retry is reissued before new approval", async (t) => {
  const childId = "d".repeat(64);
  const taskId = `reconciliation_detach:${childId}`;
  const newDigest = "e".repeat(64);
  const calls: string[] = [];
  const workflow = {
    async reconciliationChildStatus() {
      calls.push("status");
      return {
        payload: {
          type: "metadata_detach",
          task: {
            status: "failed",
            receipt: { status: "not_started" },
          },
        },
      };
    },
    async executeReconciliationDetach() {
      calls.push("execute");
      throw new Error(
        "Metadata detach replacement attempt requires new approval in PreviewReady",
      );
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow });
  t.after(current.close);

  const failed = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "revalidate" },
    }),
    {
      taskId,
      kind: "reconciliation_detach",
      store: "canonical-project",
      phase: "failed",
    },
  );
  assert.deepEqual(failed.availableActions, ["inspect", "execute"]);

  workflow.reconciliationChildStatus = async () => {
    calls.push("status");
    return {
      payload: {
        type: "metadata_detach",
        task: {
          status: "preview_ready",
          patch: { status: "previewable", digest: newDigest },
        },
      },
    };
  };
  const reissued = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "run" },
    }),
    {
      taskId,
      kind: "reconciliation_detach",
      store: "canonical-project",
      phase: "preview_ready",
    },
  );
  assert.equal(reissued.planDigest, newDigest);
  assert.deepEqual(reissued.availableActions, ["inspect", "approve"]);
  assert.deepEqual(calls, ["status", "execute", "status"]);
});

test("reconciliation detach reissue replaces the old approved digest", async (t) => {
  const childId = "7".repeat(64);
  const taskId = `reconciliation_detach:${childId}`;
  const oldDigest = "8".repeat(64);
  const newDigest = "9".repeat(64);
  const calls: string[] = [];
  const registry = new TaskFacadeRegistry();
  registry.remember({
    kind: "reconciliation_detach",
    nativeId: childId,
    store: "canonical-project",
    phase: "approved",
    source: { store: "canonical-project" },
    planDigest: oldDigest,
    approvedPlanDigest: oldDigest,
    revisionEffect: "none",
    availableActions: ["inspect", "execute"],
    staleReasons: [],
    details: {},
  });
  const workflow = {
    async executeReconciliationDetach() {
      calls.push("execute");
      throw new Error("replacement attempt requires new approval in PreviewReady");
    },
    async reconciliationChildStatus() {
      calls.push("status");
      return {
        payload: {
          type: "metadata_detach",
          task: {
            status: "preview_ready",
            patch: { status: "previewable", digest: newDigest },
          },
        },
      };
    },
    async approveReconciliationDetach(_childTaskId: string, digest: string) {
      calls.push(`approve:${digest}`);
      return {
        payload: {
          type: "metadata_detach",
          task: {
            status: "approved",
            patch: { status: "approved", digest, approvedDigest: digest },
          },
        },
      };
    },
  } as unknown as Ymm4Workflow;
  const current = await connect({ ymm4Workflow: workflow, facadeRegistry: registry });
  t.after(current.close);

  const reissued = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_execute",
      arguments: { taskId, intent: "run" },
    }),
    { taskId, kind: "reconciliation_detach", store: "canonical-project", phase: "preview_ready" },
  );
  assert.equal(reissued.planDigest, newDigest);
  assert.equal(reissued.approvedPlanDigest, null);
  assert.deepEqual(reissued.availableActions, ["inspect", "approve"]);

  const staleApproval = await current.client.callTool({
    name: "takegraph_task_approve",
    arguments: { taskId, planDigest: oldDigest },
  });
  assert.equal(staleApproval.isError, true);

  const approved = taskFrom(
    await current.client.callTool({
      name: "takegraph_task_approve",
      arguments: { taskId, planDigest: newDigest },
    }),
    { taskId, kind: "reconciliation_detach", store: "canonical-project", phase: "approved" },
  );
  assert.equal(approved.approvedPlanDigest, newDigest);
  assert.deepEqual(calls, ["execute", "status", `approve:${newDigest}`]);
});
