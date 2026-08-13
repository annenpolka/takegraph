import { execFile } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import path from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const workspaceRoot = path.resolve(import.meta.dirname, "..", "..", "..");
const guardExecutable =
  process.env.TAKEGRAPH_CORE_GUARD ??
  path.join(
    workspaceRoot,
    "target",
    "debug",
    process.platform === "win32" ? "takegraph.exe" : "takegraph",
  );

export type VoiceEngineStatus = "connected" | "unavailable";
export type TakeReadiness = "ready" | "query-ready";

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
  readiness: TakeReadiness;
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
  voiceEngine: VoiceEngineStatus;
  utterances: UtteranceSummary[];
  takes: VoiceTakeSummary[];
  activeTakeId: string;
  stagedPatch?: StagedPatchSummary;
}

const initialState: ProjectState = {
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

function digestPatch(baseRevision: number, takeId: string): string {
  return createHash("sha256")
    .update(JSON.stringify({ baseRevision, takeId }))
    .digest("hex");
}

async function commitThroughCore(input: {
  baseRevision: number;
  headRevision: number;
  digest: string;
  approvedDigest: string;
}): Promise<number> {
  const { stdout } = await execFileAsync(guardExecutable, [
    "patch-commit",
    "--base",
    String(input.baseRevision),
    "--head",
    String(input.headRevision),
    "--digest",
    input.digest,
    "--approved-digest",
    input.approvedDigest,
  ]);
  const result = JSON.parse(stdout) as { revision?: number };
  if (!Number.isSafeInteger(result.revision)) {
    throw new Error("takegraph-core returned an invalid revision");
  }
  return result.revision!;
}

/**
 * Demo project session owned by the MCP adapter process.
 *
 * It models the review workflow exposed by the app. Persistent projects will
 * replace this adapter with takegraph-service; the view never owns this state.
 */
export class ProjectSession {
  private state: ProjectState = structuredClone(initialState);
  private takeSequence = 3;

  snapshot(): ProjectState {
    return structuredClone(this.state);
  }

  generateVariant(input: {
    utteranceId: string;
    speed: number;
    intonation: number;
  }): ProjectState {
    const utterance = this.state.utterances.find(
      (candidate) => candidate.id === input.utteranceId,
    );
    if (!utterance) {
      const known = this.state.utterances.map((candidate) => candidate.id).join(", ");
      throw new Error(
        `Unknown utterance: ${input.utteranceId}. known: ${known || "(none)"}`,
      );
    }

    this.takeSequence += 1;
    const durationMs = Math.round(2_030 / input.speed);
    this.state.takes.push({
      id: `take-${randomUUID()}`,
      utteranceId: utterance.id,
      label: String.fromCharCode(64 + this.takeSequence),
      durationMs,
      speed: input.speed,
      intonation: input.intonation,
      status: "candidate",
      readiness: "query-ready",
    });
    return this.snapshot();
  }

  stageTake(takeId: string): ProjectState {
    const take = this.requireTake(takeId);
    if (take.readiness !== "ready") {
      const ready = this.state.takes
        .filter((candidate) => candidate.readiness === "ready")
        .map((candidate) => candidate.id)
        .join(", ");
      throw new Error(
        `Take ${takeId} has no completed audio artifact yet; only its synthesis query is ready. readyTakes: ${ready || "(none)"}`,
      );
    }

    const activeTake = this.requireTake(this.state.activeTakeId);
    const durationDeltaMs = take.durationMs - activeTake.durationMs;
    this.state.stagedPatch = {
      id: `patch-${randomUUID()}`,
      digest: digestPatch(this.state.revision, take.id),
      baseRevision: this.state.revision,
      takeId: take.id,
      durationDeltaMs,
      movedItemCount: durationDeltaMs === 0 ? 0 : 5,
      status: "previewable",
    };
    return this.snapshot();
  }

  async commitPatch(input: {
    patchId: string;
    digest: string;
  }): Promise<ProjectState> {
    const patch = this.state.stagedPatch;
    if (!patch) {
      throw new Error("No staged studio patch.");
    }
    if (patch.id !== input.patchId) {
      throw new Error(
        `Staged studio patch is ${patch.id}, not ${input.patchId}. digest: ${patch.digest}`,
      );
    }

    this.requireTake(patch.takeId);
    let nextRevision: number;
    try {
      nextRevision = await commitThroughCore({
        baseRevision: patch.baseRevision,
        headRevision: this.state.revision,
        digest: patch.digest,
        approvedDigest: input.digest,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(
        `${message} stagedPatchId=${patch.id} digest=${patch.digest}`,
      );
    }
    this.state.takes = this.state.takes.map((take) => ({
      ...take,
      status: take.id === patch.takeId ? "active" : "candidate",
    }));
    this.state.activeTakeId = patch.takeId;
    this.state.durationMs += patch.durationDeltaMs;
    this.state.revision = nextRevision;
    delete this.state.stagedPatch;
    return this.snapshot();
  }

  private requireTake(takeId: string): VoiceTakeSummary {
    const take = this.state.takes.find((candidate) => candidate.id === takeId);
    if (!take) {
      throw new Error(`Unknown voice take: ${takeId}`);
    }
    return take;
  }
}
