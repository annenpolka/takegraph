import {
  RESOURCE_MIME_TYPE,
  registerAppResource,
  registerAppTool,
} from "@modelcontextprotocol/ext-apps/server";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import fs from "node:fs/promises";
import path from "node:path";

const resourceUri = "ui://takegraph/editor/v1.html";
const viewDirectory = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "studio-view",
  "dist",
);

export function createServer(): McpServer {
  const server = new McpServer({ name: "TakeGraph MCP", version: "0.1.0" });

  registerAppTool(
    server,
    "studio_project_describe",
    {
      title: "Open TakeGraph Editor",
      description: "Describe the active TakeGraph project and open its editor.",
      inputSchema: {},
      _meta: { ui: { resourceUri } },
    },
    async () => {
      const project = {
        projectName: "MVP Sandbox",
        revision: 0,
        durationMs: 48_200,
        utteranceCount: 3,
        voiceEngine: "unavailable" as const,
      };
      return {
        content: [
          {
            type: "text" as const,
            text: `TakeGraph project ${project.projectName} at revision ${project.revision}.`,
          },
        ],
        structuredContent: { project },
      };
    },
  );

  registerAppResource(
    server,
    resourceUri,
    resourceUri,
    { mimeType: RESOURCE_MIME_TYPE },
    async () => {
      const html = await fs.readFile(
        path.join(viewDirectory, "mcp-app.html"),
        "utf8",
      );
      return {
        contents: [
          { uri: resourceUri, mimeType: RESOURCE_MIME_TYPE, text: html },
        ],
      };
    },
  );

  return server;
}

