import { App } from "@modelcontextprotocol/ext-apps";

export interface UtteranceSummary {
  id: string;
  speaker: string;
  caption: string;
  spokenText: string;
  startMs: number;
}

export interface VoiceTakeSummary {
  id: string;
  utteranceId: string;
  label: string;
  durationMs: number;
  speed: number;
  intonation: number;
  status: "active" | "candidate";
  readiness: "ready" | "query-ready";
}

export interface StagedPatchSummary {
  id: string;
  digest: string;
  baseRevision: number;
  takeId: string;
  durationDeltaMs: number;
  movedItemCount: number;
  status: "previewable";
}

export interface ProjectState {
  projectName: string;
  revision: number;
  durationMs: number;
  voiceEngine: "connected" | "unavailable";
  utterances: UtteranceSummary[];
  takes: VoiceTakeSummary[];
  activeTakeId: string;
  stagedPatch?: StagedPatchSummary;
}

type StateListener = (state: ProjectState) => void;

/** UI semantic boundary. Components do not depend directly on MCP Apps. */
export interface StudioHostBridge {
  readonly mode: "mcp" | "standalone";
  loadProjectState(): Promise<ProjectState>;
  generateVariant(input: {
    utteranceId: string;
    speed: number;
    intonation: number;
  }): Promise<ProjectState>;
  stageTake(takeId: string): Promise<ProjectState>;
  commitPatch(patchId: string, digest: string): Promise<ProjectState>;
  focusUtterance(utterance: UtteranceSummary): Promise<void>;
  requestFullscreen(): Promise<void>;
  subscribe(listener: StateListener): () => void;
}

function projectStateFrom(result: {
  isError?: boolean;
  content?: Array<{ type: string; text?: string }>;
  structuredContent?: unknown;
}): ProjectState {
  if (result.isError) {
    const text = result.content?.find((item) => item.type === "text")?.text;
    throw new Error(text ?? "TakeGraph tool call failed");
  }
  const payload = result.structuredContent as { state?: ProjectState } | undefined;
  if (!payload?.state) {
    throw new Error("MCP server did not return TakeGraph project state");
  }
  return payload.state;
}

class McpAppsHostBridge implements StudioHostBridge {
  readonly mode = "mcp" as const;
  private readonly app = new App(
    { name: "TakeGraph Editor", version: "0.1.0" },
    {},
    { autoResize: true },
  );
  private readonly listeners = new Set<StateListener>();
  private connection: Promise<void> | undefined;

  constructor() {
    this.app.ontoolresult = (params) => {
      try {
        this.publish(projectStateFrom(params));
      } catch {
        // Tool results from unrelated host calls need not contain TakeGraph state.
      }
    };
  }

  async loadProjectState(): Promise<ProjectState> {
    return this.call("studio_ui_get_state", {});
  }

  async generateVariant(input: {
    utteranceId: string;
    speed: number;
    intonation: number;
  }): Promise<ProjectState> {
    return this.call("voice_generate_variant", input);
  }

  async stageTake(takeId: string): Promise<ProjectState> {
    return this.call("voice_stage_take_patch", { takeId });
  }

  async commitPatch(patchId: string, digest: string): Promise<ProjectState> {
    return this.call("studio_patch_commit", { patchId, digest });
  }

  async focusUtterance(utterance: UtteranceSummary): Promise<void> {
    await this.connect();
    await this.app.updateModelContext({
      content: [
        {
          type: "text",
          text: `TakeGraph focus: ${utterance.id} (${utterance.speaker}) “${utterance.caption}”`,
        },
      ],
    });
  }

  async requestFullscreen(): Promise<void> {
    await this.connect();
    const available = this.app.getHostContext()?.availableDisplayModes;
    if (available?.includes("fullscreen")) {
      await this.app.requestDisplayMode({ mode: "fullscreen" });
    }
  }

  subscribe(listener: StateListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private async connect(): Promise<void> {
    this.connection ??= this.app.connect();
    await this.connection;
  }

  private async call(
    name: string,
    args: Record<string, unknown>,
  ): Promise<ProjectState> {
    await this.connect();
    const state = projectStateFrom(
      await this.app.callServerTool({ name, arguments: args }),
    );
    this.publish(state);
    return state;
  }

  private publish(state: ProjectState): void {
    for (const listener of this.listeners) {
      listener(state);
    }
  }
}

function standaloneState(): ProjectState {
  return {
    projectName: "MVP Sandbox",
    revision: 0,
    durationMs: 48_200,
    voiceEngine: "unavailable",
    utterances: [
      {
        id: "utt-01",
        speaker: "魔理沙",
        caption: "ここから第二形態だぜ",
        spokenText: "ここからだいにけいたいだぜ",
        startMs: 12_450,
      },
      {
        id: "utt-02",
        speaker: "霊夢",
        caption: "聞いてないよ",
        spokenText: "きいてないよ",
        startMs: 14_820,
      },
      {
        id: "utt-03",
        speaker: "魔理沙",
        caption: "一度、距離を取ろう",
        spokenText: "いちど、きょりをとろう",
        startMs: 17_260,
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
        speed: 1.12,
        intonation: 0.95,
        status: "candidate",
        readiness: "ready",
      },
      {
        id: "take-c",
        utteranceId: "utt-01",
        label: "C",
        durationMs: 1_810,
        speed: 1.14,
        intonation: 1.04,
        status: "candidate",
        readiness: "ready",
      },
    ],
    activeTakeId: "take-a",
  };
}

class StandaloneHostBridge implements StudioHostBridge {
  readonly mode = "standalone" as const;
  private state = standaloneState();
  private readonly listeners = new Set<StateListener>();
  private sequence = 3;

  async loadProjectState(): Promise<ProjectState> {
    return this.snapshot();
  }

  async generateVariant(input: {
    utteranceId: string;
    speed: number;
    intonation: number;
  }): Promise<ProjectState> {
    this.sequence += 1;
    this.state.takes.push({
      id: `take-standalone-${this.sequence}`,
      utteranceId: input.utteranceId,
      label: String.fromCharCode(64 + this.sequence),
      durationMs: Math.round(2_030 / input.speed),
      speed: input.speed,
      intonation: input.intonation,
      status: "candidate",
      readiness: "query-ready",
    });
    return this.changed();
  }

  async stageTake(takeId: string): Promise<ProjectState> {
    const take = this.state.takes.find((candidate) => candidate.id === takeId);
    const active = this.state.takes.find(
      (candidate) => candidate.id === this.state.activeTakeId,
    );
    if (!take || !active || take.readiness !== "ready") {
      throw new Error("A completed voice artifact is required before preview.");
    }
    const durationDeltaMs = take.durationMs - active.durationMs;
    this.state.stagedPatch = {
      id: `patch-standalone-${this.state.revision}`,
      digest: "a".repeat(64),
      baseRevision: this.state.revision,
      takeId,
      durationDeltaMs,
      movedItemCount: durationDeltaMs === 0 ? 0 : 5,
      status: "previewable",
    };
    return this.changed();
  }

  async commitPatch(patchId: string, digest: string): Promise<ProjectState> {
    const patch = this.state.stagedPatch;
    if (
      !patch ||
      patch.id !== patchId ||
      patch.digest !== digest ||
      patch.baseRevision !== this.state.revision
    ) {
      throw new Error("Patch approval is stale or does not match.");
    }
    this.state.takes = this.state.takes.map((take) => ({
      ...take,
      status: take.id === patch.takeId ? "active" : "candidate",
    }));
    this.state.activeTakeId = patch.takeId;
    this.state.durationMs += patch.durationDeltaMs;
    this.state.revision += 1;
    delete this.state.stagedPatch;
    return this.changed();
  }

  async focusUtterance(): Promise<void> {}
  async requestFullscreen(): Promise<void> {}

  subscribe(listener: StateListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private snapshot(): ProjectState {
    return structuredClone(this.state);
  }

  private changed(): ProjectState {
    const state = this.snapshot();
    for (const listener of this.listeners) {
      listener(state);
    }
    return state;
  }
}

export function createStudioHostBridge(): StudioHostBridge {
  return window.parent === window
    ? new StandaloneHostBridge()
    : new McpAppsHostBridge();
}
