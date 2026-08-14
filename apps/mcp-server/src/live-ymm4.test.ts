import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const inheritedEnvironment = Object.fromEntries(
  Object.entries(process.env).filter(
    (entry): entry is [string, string] => entry[1] !== undefined,
  ),
);

test(
  "live MCP tools describe the protocol-2 project and native descriptors",
  { skip: process.env.TAKEGRAPH_LIVE_YMM4 !== "1" },
  async (t) => {
    const transport = new StdioClientTransport({
      command: process.execPath,
      args: [path.resolve(import.meta.dirname, "../dist/main.js"), "--stdio"],
      env: { ...inheritedEnvironment, TAKEGRAPH_LEGACY_TOOLS: "1" },
      stderr: "pipe",
    });
    const client = new Client({ name: "takegraph-live-ymm4", version: "0.1.0" });
    await client.connect(transport);
    t.after(async () => client.close());

    const described = await client.callTool({
      name: "ymm4_link_describe",
      arguments: {},
    });
    assert.equal(described.isError, undefined);
    const structured = described.structuredContent as {
      health?: { status?: string; protocolVersion?: number };
      capabilities?: { capabilities?: string[] };
      snapshot?: { managedItems?: unknown[]; projectPath?: string };
    };
    assert.equal(structured.health?.status, "running");
    assert.equal(structured.health?.protocolVersion, 2);
    const requiredCapabilities = [
      "request_bound_receipts",
      "write_ahead_apply",
      "native_voice_create",
      "native_voice_update_replace_preserving_user_state",
      "native_voice_exact_wav_export",
      "scene_capture_native_png",
      "scene_capture_playhead_restore",
      "scene_composition_current",
      "native_effect_typed_mutation",
      "project_checkpoint_verified",
    ];
    for (const capability of requiredCapabilities) {
      assert.ok(
        structured.capabilities?.capabilities?.includes(capability),
        `${capability} missing from the live bridge`,
      );
    }
    for (const capability of [
      "project_render",
      "project_render_cancel",
      "project_render_media_receipt",
      "edit_surface_admit",
      "composition_graph_apply",
      "project_settings_mutation",
      "project_scene_mutation",
      "project_timeline_mutation",
      "project_character_mutation",
      "project_template_definition_edit",
      "edit_transaction_apply",
    ]) {
      assert.ok(
        !structured.capabilities?.capabilities?.includes(capability),
        `${capability} must stay fail-closed without exhaustive source dependency evidence`,
      );
    }
    const profiles = await client.callTool({
      name: "ymm4_render_profiles",
      arguments: {},
    });
    assert.equal(profiles.isError, undefined);
    const renderContent = profiles.structuredContent as {
      profiles?: Array<{
        bindable?: boolean;
        container?: string;
        videoCodec?: string;
        audioCodec?: string;
        audioSampleRate?: number;
        pixelFormat?: string;
        bindingManifestDigest?: string;
        bindingError?: string;
      }>;
    };
    const bindable = renderContent.profiles?.filter((profile) => profile.bindable) ?? [];
    assert.equal(bindable.length, 0);
    const unbound = renderContent.profiles?.filter((profile) => !profile.bindable) ?? [];
    assert.equal(unbound.length, 1);
    assert.match(
      unbound[0]?.bindingError ?? "",
      /exhaustive render dependency manifest/,
    );
    const minimumManagedItems = Number.parseInt(
      process.env.TAKEGRAPH_LIVE_YMM4_MIN_MANAGED_ITEMS ?? "1",
      10,
    );
    assert.ok(
      (structured.snapshot?.managedItems?.length ?? 0) >= minimumManagedItems,
    );
    const expectedProject = new RegExp(
      process.env.TAKEGRAPH_LIVE_YMM4_PROJECT_PATTERN ?? "\\.ymmp$",
    );
    assert.match(structured.snapshot?.projectPath ?? "", expectedProject);

    const scene = await client.callTool({
      name: "takegraph_inspect",
      arguments: { view: "scene" },
    });
    assert.equal(scene.isError, undefined);
    const sceneContent = scene.structuredContent as {
      composition?: {
        schemaVersion?: number;
        availability?: string;
        observationStatus?: string;
        evaluatedFrame?: number;
        elements?: unknown[];
      };
    };
    assert.equal(sceneContent.composition?.schemaVersion, 1);
    assert.match(sceneContent.composition?.availability ?? "", /^current_frame_/);
    assert.equal(sceneContent.composition?.observationStatus, "source_bound");
    assert.ok(Number.isInteger(sceneContent.composition?.evaluatedFrame));
    assert.ok(Array.isArray(sceneContent.composition?.elements));

    const descriptors = await client.callTool({
      name: "ymm4_native_extension_descriptors",
      arguments: {},
    });
    assert.equal(descriptors.isError, undefined);
    const descriptorContent = descriptors.structuredContent as {
      targetCatalog?: {
        catalogDigest?: string;
        descriptors?: unknown[];
      };
    };
    assert.match(
      descriptorContent.targetCatalog?.catalogDigest ?? "",
      /^[0-9a-f]{64}$/,
    );
    assert.ok((descriptorContent.targetCatalog?.descriptors?.length ?? 0) > 0);
  },
);
