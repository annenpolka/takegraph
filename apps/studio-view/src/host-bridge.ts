import { App } from "@modelcontextprotocol/ext-apps";

export interface ProjectSummary {
  projectName: string;
  revision: number;
  durationMs: number;
  utteranceCount: number;
  voiceEngine: "connected" | "unavailable";
}

/** UI semantic boundary. Components do not depend directly on MCP Apps. */
export interface StudioHostBridge {
  readonly mode: "mcp" | "standalone";
  loadProjectSummary(): Promise<ProjectSummary>;
}

class McpAppsHostBridge implements StudioHostBridge {
  readonly mode = "mcp" as const;
  private readonly app = new App({ name: "TakeGraph Editor", version: "0.1.0" });
  private connection: Promise<void> | undefined;

  async loadProjectSummary(): Promise<ProjectSummary> {
    this.connection ??= this.app.connect();
    await this.connection;
    const result = await this.app.callServerTool({
      name: "studio_project_describe",
      arguments: {},
    });
    const payload = result.structuredContent as
      | { project?: ProjectSummary }
      | undefined;

    if (!payload?.project) {
      throw new Error("MCP server did not return a project summary");
    }
    return payload.project;
  }
}

class StandaloneHostBridge implements StudioHostBridge {
  readonly mode = "standalone" as const;

  async loadProjectSummary(): Promise<ProjectSummary> {
    return {
      projectName: "MVP Sandbox",
      revision: 0,
      durationMs: 48_200,
      utteranceCount: 3,
      voiceEngine: "unavailable",
    };
  }
}

export function createStudioHostBridge(): StudioHostBridge {
  return window.parent === window
    ? new StandaloneHostBridge()
    : new McpAppsHostBridge();
}

