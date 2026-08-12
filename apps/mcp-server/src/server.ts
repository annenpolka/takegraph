import {
  RESOURCE_MIME_TYPE,
  registerAppResource,
  registerAppTool,
} from "@modelcontextprotocol/ext-apps/server";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import fs from "node:fs/promises";
import path from "node:path";
import { z } from "zod";
import { ProjectSession, type ProjectState } from "./project-session.js";

const resourceUri = "ui://takegraph/editor/v1.html";
const viewDirectory = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "studio-view",
  "dist",
);

export interface CreateServerOptions {
  session?: ProjectSession;
  viewHtml?: string;
}

function stateResult(state: ProjectState, message: string) {
  return {
    content: [{ type: "text" as const, text: message }],
    structuredContent: { state },
  };
}

function errorResult(error: unknown) {
  return {
    isError: true,
    content: [
      {
        type: "text" as const,
        text: error instanceof Error ? error.message : String(error),
      },
    ],
  };
}

export function createServer(options: CreateServerOptions = {}): McpServer {
  const server = new McpServer({ name: "TakeGraph MCP", version: "0.1.0" });
  const session = options.session ?? new ProjectSession();
  const toolMeta = {
    ui: { resourceUri, visibility: ["model", "app"] as const },
  };

  registerAppTool(
    server,
    "studio_project_describe",
    {
      title: "Open TakeGraph Editor",
      description: "Describe the active TakeGraph project and open its editor.",
      inputSchema: {},
      annotations: { readOnlyHint: true },
      _meta: toolMeta,
    },
    async () => {
      const state = session.snapshot();
      return stateResult(
        state,
        `TakeGraph project ${state.projectName} at revision ${state.revision}.`,
      );
    },
  );

  registerAppTool(
    server,
    "studio_ui_get_state",
    {
      title: "Refresh TakeGraph state",
      description: "Return the latest TakeGraph editor state to its app view.",
      inputSchema: {},
      annotations: { readOnlyHint: true },
      _meta: {
        ui: { resourceUri, visibility: ["app"] as const },
      },
    },
    async () => stateResult(session.snapshot(), "TakeGraph state refreshed."),
  );

  registerAppTool(
    server,
    "voice_generate_variant",
    {
      title: "Create voice-take candidate",
      description:
        "Create a new immutable VoiceTake candidate and capture its synthesis settings.",
      inputSchema: {
        utteranceId: z.string().min(1),
        speed: z.number().min(0.5).max(2),
        intonation: z.number().min(0).max(2),
      },
      annotations: { destructiveHint: false },
      _meta: toolMeta,
    },
    async (input) => {
      try {
        return stateResult(
          session.generateVariant(input),
          "A new VoiceTake candidate was created. Its synthesis query is ready; the audio artifact is not complete.",
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  registerAppTool(
    server,
    "voice_stage_take_patch",
    {
      title: "Preview take adoption",
      description:
        "Stage a digest-bound patch that previews adopting an existing ready VoiceTake.",
      inputSchema: { takeId: z.string().min(1) },
      annotations: { destructiveHint: false },
      _meta: toolMeta,
    },
    async ({ takeId }) => {
      try {
        return stateResult(
          session.stageTake(takeId),
          `A previewable patch for ${takeId} was staged.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  registerAppTool(
    server,
    "studio_patch_commit",
    {
      title: "Approve and commit take patch",
      description:
        "Approve the exact staged digest and commit it only if its base revision is current.",
      inputSchema: {
        patchId: z.string().min(1),
        digest: z.string().length(64),
      },
      annotations: { destructiveHint: true },
      _meta: toolMeta,
    },
    async (input) => {
      try {
        const state = await session.commitPatch(input);
        return stateResult(
          state,
          `Patch committed. Project revision is now ${state.revision}.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  registerAppResource(
    server,
    "TakeGraph Editor",
    resourceUri,
    { mimeType: RESOURCE_MIME_TYPE },
    async () => {
      const html =
        options.viewHtml ??
        (await fs.readFile(path.join(viewDirectory, "mcp-app.html"), "utf8"));
      return {
        contents: [
          { uri: resourceUri, mimeType: RESOURCE_MIME_TYPE, text: html },
        ],
      };
    },
  );

  return server;
}
