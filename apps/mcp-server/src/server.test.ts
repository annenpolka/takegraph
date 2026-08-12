import assert from "node:assert/strict";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ProjectSession, type ProjectState } from "./project-session.js";
import { createServer } from "./server.js";

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
