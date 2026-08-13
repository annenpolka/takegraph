import assert from "node:assert/strict";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ProjectSession } from "./project-session.js";
import {
  createServer as createBaseServer,
  type CreateServerOptions,
} from "./server.js";
import {
  type StageNativeExtensionInput,
  Ymm4Workflow,
} from "./ymm4-workflow.js";

function createServer(options: CreateServerOptions = {}) {
  return createBaseServer({ ...options, legacyTools: true });
}

test("MCP exposes descriptor-bound native-extension approval and verification", async (t) => {
  const handle = "77777777-7777-4777-8777-777777777777";
  const digest = `sha256:${"a".repeat(64)}`;
  const calls: string[] = [];
  const workflow = {
    async nativeExtensionDescriptors() {
      calls.push("descriptors");
      return {
        targetCatalog: { descriptors: [] },
        planningDescriptorDigests: {},
      };
    },
    async stageNativeExtension(input: StageNativeExtensionInput) {
      calls.push("stage");
      assert.equal(input.operations[0]?.type, "portrait");
      if (input.operations[0]?.type === "portrait") {
        assert.deepEqual(input.operations[0].approvedLossyFields, [
          "nativeAnimation.keyframes",
        ]);
      }
      return {
        handle,
        digest,
        plan: { warnings: ["approved replacement loss"] },
      };
    },
    async approveNativeExtension(receivedHandle: string, receivedDigest: string) {
      calls.push("approve");
      assert.equal(receivedHandle, handle);
      assert.equal(receivedDigest, digest);
      return { status: "approved" };
    },
    async applyNativeExtension(receivedHandle: string) {
      calls.push("apply");
      assert.equal(receivedHandle, handle);
      return { revision: 8, verified: true };
    },
    async verifyNativeExtension(receivedHandle: string) {
      calls.push("verify");
      assert.equal(receivedHandle, handle);
      return { verified: true };
    },
    async nativeExtensionStatus(receivedHandle: string) {
      calls.push("status");
      assert.equal(receivedHandle, handle);
      return { currentVerified: true, recoveryStatus: "verified" };
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({
    session: new ProjectSession(),
    viewHtml: "<!doctype html><html><body>TakeGraph test app</body></html>",
    ymm4Workflow: workflow,
  });
  const client = new Client({ name: "takegraph-phase4-test", version: "0.1.0" });
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
  for (const name of [
    "ymm4_native_extension_descriptors",
    "ymm4_native_extension_stage",
    "ymm4_native_extension_approve",
    "ymm4_native_extension_apply",
    "ymm4_native_extension_verify",
    "ymm4_native_extension_status",
  ]) {
    assert.ok(listed.tools.some((tool) => tool.name === name), `${name} missing`);
  }

  await client.callTool({
    name: "ymm4_native_extension_descriptors",
    arguments: {},
  });
  const staged = await client.callTool({
    name: "ymm4_native_extension_stage",
    arguments: {
      operations: [
        {
          type: "portrait",
          entityId: "portrait-01",
          entityRevision: 2,
          descriptorId: "character.marisa",
          expectedConfigDigest: "b".repeat(64),
          expectedSchemaDigest: "c".repeat(64),
          frame: 120,
          layer: 10,
          durationFrames: 180,
          approvedLossyFields: ["nativeAnimation.keyframes"],
        },
      ],
    },
  });
  assert.equal((staged.structuredContent as { handle?: string }).handle, handle);
  await client.callTool({
    name: "ymm4_native_extension_approve",
    arguments: { handle, digest },
  });
  await client.callTool({
    name: "ymm4_native_extension_apply",
    arguments: { handle },
  });
  await client.callTool({
    name: "ymm4_native_extension_verify",
    arguments: { handle },
  });
  await client.callTool({
    name: "ymm4_native_extension_status",
    arguments: { handle },
  });
  assert.deepEqual(calls, [
    "descriptors",
    "stage",
    "approve",
    "apply",
    "verify",
    "status",
  ]);
});

test("MCP rejects malformed native-extension digests before workflow", async (t) => {
  let called = false;
  const workflow = {
    async stageNativeExtension() {
      called = true;
      return {};
    },
  } as unknown as Ymm4Workflow;
  const server = createServer({ ymm4Workflow: workflow });
  const client = new Client({ name: "takegraph-phase4-invalid", version: "0.1.0" });
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
    name: "ymm4_native_extension_stage",
    arguments: {
      operations: [
        {
          type: "template",
          entityId: "template-01",
          entityRevision: 1,
          descriptorId: "template.bad",
          expectedConfigDigest: "not-a-digest",
          expectedSchemaDigest: "c".repeat(64),
          frame: 0,
          layer: 0,
        },
      ],
    },
  });
  assert.equal((result as { isError?: boolean }).isError, true);
  assert.equal(called, false);
});
