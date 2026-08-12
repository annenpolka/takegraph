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
      "ymm4_checkpoint_execute",
      "ymm4_checkpoint_stage",
      "ymm4_checkpoint_status",
      "ymm4_export_commit",
      "ymm4_export_stage",
      "ymm4_export_verify",
      "ymm4_link_describe",
      "ymm4_native_extension_apply",
      "ymm4_native_extension_approve",
      "ymm4_native_extension_descriptors",
      "ymm4_native_extension_stage",
      "ymm4_native_extension_status",
      "ymm4_native_extension_verify",
      "ymm4_native_voice_commit",
      "ymm4_native_voice_mutation_artifacts",
      "ymm4_native_voice_mutation_commit",
      "ymm4_native_voice_mutation_stage",
      "ymm4_native_voice_mutation_verify",
      "ymm4_native_voice_stage",
      "ymm4_native_voice_verify",
      "ymm4_project_save",
      "ymm4_reconcile_apply",
      "ymm4_reconcile_child_status",
      "ymm4_reconcile_detach_approve",
      "ymm4_reconcile_detach_execute",
      "ymm4_reconcile_preview",
      "ymm4_reconcile_re_export_dispatch",
      "ymm4_reconcile_report",
      "ymm4_render_cancel",
      "ymm4_render_execute",
      "ymm4_render_profiles",
      "ymm4_render_stage",
      "ymm4_render_status",
      "ymm4_scene_inspection_approve",
      "ymm4_scene_inspection_capture",
      "ymm4_scene_inspection_decide",
      "ymm4_scene_inspection_replay",
      "ymm4_scene_inspection_review",
      "ymm4_scene_inspection_stage",
      "ymm4_scene_inspection_status",
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
