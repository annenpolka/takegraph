import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

test("built stdio server serves the production MCP App bundle", async (t) => {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [path.resolve(import.meta.dirname, "../dist/main.js"), "--stdio"],
    stderr: "pipe",
  });
  const client = new Client({ name: "takegraph-stdio-test", version: "0.1.0" });
  await client.connect(transport);
  t.after(async () => client.close());

  const tools = await client.listTools();
  assert.deepEqual(
    tools.tools.map((tool) => tool.name).sort(),
    [
      "studio_patch_commit",
      "studio_project_describe",
      "studio_ui_get_state",
      "voice_generate_variant",
      "voice_stage_take_patch",
    ],
  );

  const opened = await client.callTool({
    name: "studio_project_describe",
    arguments: {},
  });
  assert.equal(
    (
      opened as {
        structuredContent?: { state?: { projectName?: string } };
      }
    ).structuredContent?.state?.projectName,
    "MVP Sandbox",
  );

  const resource = await client.readResource({
    uri: "ui://takegraph/editor/v1.html",
  });
  const content = resource.contents[0];
  assert.equal(content?.mimeType, "text/html;profile=mcp-app");
  assert.ok(content && "text" in content);
  assert.match(content.text, /TakeGraph/);
  assert.ok(content.text.length > 500_000, "the production bundle is inlined");
});
