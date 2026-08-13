import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ProjectSession } from "./project-session.js";
import {
  createServer as createBaseServer,
  type CreateServerOptions,
} from "./server.js";
import {
  type ReconciliationDecisionInput,
  type StageRenderInput,
  Ymm4Workflow,
} from "./ymm4-workflow.js";

const checkpointId = "11111111-1111-4111-8111-111111111111";
const renderId = "22222222-2222-4222-8222-222222222222";
const reportDigest = "a".repeat(64);
const approvalDigest = "b".repeat(64);
const childTaskId = "c".repeat(64);

function createServer(options: CreateServerOptions = {}) {
  return createBaseServer({ ...options, legacyTools: true });
}

test("MCP exposes the guarded Phase 5 checkpoint, render, and reconcile surface", async (t) => {
  const calls: string[] = [];
  const workflow = {
    async stageCheckpoint() {
      calls.push("checkpoint-stage");
      return { payload: { request: { operationId: checkpointId }, status: "staged" } };
    },
    async executeCheckpoint(operationId: string) {
      calls.push("checkpoint-execute");
      assert.equal(operationId, checkpointId);
      return { payload: { status: "verified" } };
    },
    async checkpointStatus(operationId: string) {
      calls.push("checkpoint-status");
      assert.equal(operationId, checkpointId);
      return { payload: { status: "verified" } };
    },
    async renderProfiles() {
      calls.push("render-profiles");
      return { profiles: [{ descriptorId: "final-mp4" }] };
    },
    async stageRender(input: StageRenderInput) {
      calls.push("render-stage");
      assert.equal(input.profile, "final-mp4");
      assert.equal(input.overwrite, true);
      return { payload: { request: { taskId: renderId }, status: "staged" } };
    },
    async executeRender(taskId: string) {
      calls.push("render-execute");
      assert.equal(taskId, renderId);
      return { payload: { status: "running" } };
    },
    async renderStatus(taskId: string) {
      calls.push("render-status");
      assert.equal(taskId, renderId);
      return { payload: { status: "verified" } };
    },
    async cancelRender(taskId: string) {
      calls.push("render-cancel");
      assert.equal(taskId, renderId);
      return { payload: { status: "cancelling" } };
    },
    async reconciliationReport() {
      calls.push("reconcile-report");
      return { payload: { report: { reportDigest }, status: "report_ready" } };
    },
    async previewReconciliation(
      receivedReportDigest: string,
      decisions: ReconciliationDecisionInput[],
    ) {
      calls.push("reconcile-preview");
      assert.equal(receivedReportDigest, reportDigest);
      assert.equal(decisions[0]?.choice, "re_export_canonical");
      return {
        payload: {
          preview: { approvalDigest },
          status: "decision_preview_ready",
        },
      };
    },
    async applyReconciliation(
      receivedReportDigest: string,
      receivedApprovalDigest: string,
    ) {
      calls.push("reconcile-apply");
      assert.equal(receivedReportDigest, reportDigest);
      assert.equal(receivedApprovalDigest, approvalDigest);
      return { payload: { status: "actions_accepted" } };
    },
    async reconciliationChildStatus(receivedChildTaskId: string) {
      calls.push("reconcile-child-status");
      assert.equal(receivedChildTaskId, childTaskId);
      return { payload: { type: "metadata_detach", task: { status: "preview_ready" } } };
    },
    async approveReconciliationDetach(
      receivedChildTaskId: string,
      receivedApprovalDigest: string,
    ) {
      calls.push("reconcile-detach-approve");
      assert.equal(receivedChildTaskId, childTaskId);
      assert.equal(receivedApprovalDigest, approvalDigest);
      return { payload: { type: "metadata_detach", task: { status: "approved" } } };
    },
    async executeReconciliationDetach(receivedChildTaskId: string) {
      calls.push("reconcile-detach-execute");
      assert.equal(receivedChildTaskId, childTaskId);
      return { payload: { type: "metadata_detach", task: { status: "verified" } } };
    },
    async dispatchReconciliationReExport(
      receivedChildTaskId: string,
      manifest: Record<string, unknown>,
    ) {
      calls.push("reconcile-re-export-dispatch");
      assert.equal(receivedChildTaskId, childTaskId);
      assert.equal(manifest.route, "native_voice_mutation");
      return {
        handle: "33333333-3333-4333-8333-333333333333",
        downstreamRoute: "native_voice_mutation",
        digest: "e".repeat(64),
        payload: {
          type: "canonical_re_export",
          task: {
            status: "preview_ready",
            downstreamPreview: {
              route: "native_voice_mutation",
              preview: {
                patch: {
                  status: "previewable",
                  approvedDigest: null,
                  digest: "e".repeat(64),
                },
              },
            },
          },
        },
      };
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph Phase 5</body></html>",
    ymm4Workflow: workflow,
  });
  const client = new Client({ name: "takegraph-phase5-test", version: "0.1.0" });
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
  const names = [
    "ymm4_checkpoint_stage",
    "ymm4_checkpoint_execute",
    "ymm4_checkpoint_status",
    "ymm4_render_profiles",
    "ymm4_render_stage",
    "ymm4_render_execute",
    "ymm4_render_status",
    "ymm4_render_cancel",
    "ymm4_reconcile_report",
    "ymm4_reconcile_preview",
    "ymm4_reconcile_apply",
    "ymm4_reconcile_child_status",
    "ymm4_reconcile_detach_approve",
    "ymm4_reconcile_detach_execute",
    "ymm4_reconcile_re_export_dispatch",
  ];
  for (const name of names) {
    assert.ok(listed.tools.some((tool) => tool.name === name), `${name} missing`);
  }
  assert.match(
    listed.tools.find((tool) => tool.name === "ymm4_reconcile_report")
      ?.description ?? "",
    /unmanaged/i,
  );
  assert.match(
    listed.tools.find((tool) => tool.name === "ymm4_reconcile_apply")
      ?.description ?? "",
    /never silently/i,
  );

  await client.callTool({ name: "ymm4_checkpoint_stage", arguments: {} });
  await client.callTool({
    name: "ymm4_checkpoint_execute",
    arguments: { operationId: checkpointId },
  });
  await client.callTool({
    name: "ymm4_checkpoint_status",
    arguments: { operationId: checkpointId },
  });
  await client.callTool({ name: "ymm4_render_profiles", arguments: {} });
  await client.callTool({
    name: "ymm4_render_stage",
    arguments: {
      checkpointOperationId: checkpointId,
      profile: "final-mp4",
      outputPath: path.resolve("final.mp4"),
      overwrite: true,
    },
  });
  await client.callTool({
    name: "ymm4_render_execute",
    arguments: { taskId: renderId },
  });
  await client.callTool({
    name: "ymm4_render_status",
    arguments: { taskId: renderId },
  });
  await client.callTool({
    name: "ymm4_render_cancel",
    arguments: { taskId: renderId },
  });
  await client.callTool({
    name: "ymm4_reconcile_report",
    arguments: {},
  });
  await client.callTool({
    name: "ymm4_reconcile_preview",
    arguments: {
      reportDigest,
      decisions: [
        { entryId: "d".repeat(64), choice: "re_export_canonical" },
      ],
    },
  });
  await client.callTool({
    name: "ymm4_reconcile_apply",
    arguments: { reportDigest, approvalDigest },
  });
  await client.callTool({
    name: "ymm4_reconcile_child_status",
    arguments: { childTaskId },
  });
  await client.callTool({
    name: "ymm4_reconcile_detach_approve",
    arguments: { childTaskId, approvalDigest },
  });
  await client.callTool({
    name: "ymm4_reconcile_detach_execute",
    arguments: { childTaskId },
  });
  const reExport = await client.callTool({
    name: "ymm4_reconcile_re_export_dispatch",
    arguments: {
      childTaskId,
      manifest: { route: "native_voice_mutation", mutations: [] },
    },
  });
  const structured = (reExport as { structuredContent?: Record<string, unknown> })
    .structuredContent as {
      handle?: string;
      downstreamRoute?: string;
      digest?: string;
      payload?: {
        task?: {
          downstreamPreview?: {
            preview?: { patch?: { approvedDigest?: unknown } };
          };
        };
      };
    };
  assert.match(structured.handle ?? "", /^[0-9a-f-]{36}$/i);
  assert.equal(structured.downstreamRoute, "native_voice_mutation");
  assert.equal(structured.digest, "e".repeat(64));
  assert.equal(
    structured.payload?.task?.downstreamPreview?.preview?.patch?.approvedDigest,
    null,
  );

  assert.deepEqual(calls, [
    "checkpoint-stage",
    "checkpoint-execute",
    "checkpoint-status",
    "render-profiles",
    "render-stage",
    "render-execute",
    "render-status",
    "render-cancel",
    "reconcile-report",
    "reconcile-preview",
    "reconcile-apply",
    "reconcile-child-status",
    "reconcile-detach-approve",
    "reconcile-detach-execute",
    "reconcile-re-export-dispatch",
  ]);
});

test("MCP rejects malformed Phase 5 identifiers before workflow dispatch", async (t) => {
  let called = false;
  const workflow = {
    async executeCheckpoint() {
      called = true;
      return {};
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({ ymm4Workflow: workflow });
  const client = new Client({ name: "takegraph-phase5-invalid", version: "0.1.0" });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([
    server.connect(serverTransport),
    client.connect(clientTransport),
  ]);
  t.after(async () => {
    await client.close();
    await server.close();
  });

  const result = await client.callTool({
    name: "ymm4_checkpoint_execute",
    arguments: { operationId: "..\\outside" },
  });
  assert.equal((result as { isError?: boolean }).isError, true);
  assert.equal(called, false);
});
