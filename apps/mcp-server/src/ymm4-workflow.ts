import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const workspaceRoot = path.resolve(import.meta.dirname, "..", "..", "..");
const defaultExecutable =
  process.env.TAKEGRAPH_CORE_GUARD ??
  path.join(
    workspaceRoot,
    "target",
    "debug",
    process.platform === "win32" ? "takegraph.exe" : "takegraph",
  );

interface CanonicalContext {
  projectId: string;
  revision: number;
}

function configuredDirectory(
  explicit: string | undefined,
  environmentName: string,
  fallback: string,
): string {
  const environmentValue = process.env[environmentName];
  const configured = explicit ?? environmentValue ?? fallback;
  if (configured.trim().length === 0) {
    throw new Error(`${environmentName} must not be empty`);
  }
  return path.resolve(workspaceRoot, configured);
}

interface VoiceArtifactResult {
  speaker: string;
  speakerUuid: string;
  style: string;
  artifact: {
    style_id: number;
    query_hash: string;
    query_path: string;
    audio_hash: string;
    audio_path: string;
    wav: {
      duration_samples: number;
      sample_rate: number;
      channels: number;
      bits_per_sample: number;
    };
  };
}

interface Ymm4Snapshot {
  projectId: string;
  projectName: string;
  projectPath: string;
  sceneId: string;
  fps: number;
  fingerprint: string;
  managedItems: unknown[];
  nativeExtensions: unknown[];
  unmanagedContextCount: number;
}

interface StagedResult {
  patchId: string;
  digest: string;
  baseRevision: number;
  operationId: string;
  project: Ymm4Snapshot;
  plan: {
    operationCount: number;
    createCount: number;
    replaceCount: number;
    unchangedCount: number;
  };
  targetPlanDigest: string;
  targetPlan: Record<string, unknown>;
  patchFile: string;
}

interface NativeVoiceStagedResult {
  patchId: string;
  digest: string;
  baseRevision: number;
  operationId: string;
  project: Ymm4Snapshot;
  plan: {
    fingerprint: string;
    createCount: number;
    durationResolution: string;
  };
  targetPlanDigest: string;
  targetPlan: Record<string, unknown>;
  patchFile: string;
}

interface NativeVoiceMutationStagedResult {
  patchId: string;
  digest: string;
  baseRevision: number;
  operationId: string;
  project: Ymm4Snapshot;
  plan: {
    fingerprint: string;
    createCount: number;
    updateCount: number;
    deleteCount: number;
    durationResolution: string;
    preservedFields: string[];
  };
  capabilityDigest: string;
  patchFile: string;
}

export interface PixelRectInput {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SceneRegionInput {
  regionId: string;
  kind: "caption" | "portrait";
  bounds: PixelRectInput;
  background: {
    red: number;
    green: number;
    blue: number;
    alpha: number;
  };
  colorTolerance: number;
  minForegroundPpm: number;
  minimumEdgeClearancePx: number;
}

export interface StageSceneInspectionInput {
  frames: number[];
  expectedWidth: number;
  expectedHeight: number;
  profileId?: string;
  alpha?: boolean;
  maxActualFrameDelta?: number;
  blackLumaThreshold?: number;
  blackPixelRatioPpm?: number;
  blankChannelSpanThreshold?: number;
  safeArea?: PixelRectInput | null;
  regions?: SceneRegionInput[];
}

export interface SceneInspectionStagedResult {
  handle: string;
  digest: string;
  receipt: { status: string; captures: unknown[] };
  profile?: unknown;
  frames?: unknown;
}

export interface StageYmm4Input {
  entityId: string;
  caption: string;
  spokenText: string;
  speaker: string;
  style: string;
  frame: number;
  audioLayer: number;
  captionLayer: number;
}

export interface StageNativeVoiceInput {
  entityId: string;
  displayText: string;
  spokenText: string;
  characterName: string;
  frame: number;
  layer: number;
  maxLength: number;
}

interface NativeVoiceMutationCommonInput {
  entityId: string;
  revision: number;
}

interface NativeVoiceMutationWriteInput extends NativeVoiceMutationCommonInput {
  realizationId?: string;
  characterName: string;
  displayText: string;
  spokenText: string;
  frame: number;
  layer: number;
  maxLength: number;
}

export type NativeVoiceMutationInput =
  | (NativeVoiceMutationWriteInput & { action: "create" })
  | (NativeVoiceMutationWriteInput & {
      action: "update";
      realizationId: string;
    })
  | (NativeVoiceMutationCommonInput & {
      action: "delete";
      realizationId: string;
    });

export interface StageNativeVoiceMutationsInput {
  mutations: NativeVoiceMutationInput[];
}

export interface NativeDescriptorBindingInput {
  descriptorId: string;
  expectedConfigDigest: string;
  expectedSchemaDigest: string;
}

export type NativeEffectParameterInput =
  | { type: "boolean"; value: boolean }
  | { type: "integer"; value: number }
  | { type: "fixed"; value: { scale: number; scaled: number } }
  | { type: "text"; value: string }
  | { type: "choice"; value: string }
  | { type: "color_rgba"; value: [number, number, number, number] };

export type NativeExtensionOperationInput =
  | ({
      type: "portrait" | "face";
      entityId: string;
      entityRevision: number;
      frame: number;
      layer: number;
      durationFrames: number;
      approvedLossyFields?: string[];
    } & NativeDescriptorBindingInput)
  | {
      type: "image" | "video" | "audio" | "bgm";
      entityId: string;
      entityRevision: number;
      sourcePath: string;
      artifactDigest: string;
      mediaType: string;
      byteLength: number;
      frame: number;
      layer: number;
      durationFrames: number;
      loopPlayback?: boolean;
      approvedLossyFields?: string[];
    }
  | ({
      type: "effect";
      targetEntityId: string;
      targetEntityRevision: number;
      effectInstanceId: string;
      action: "upsert" | "remove";
      parameters?: Record<string, NativeEffectParameterInput>;
    } & NativeDescriptorBindingInput)
  | ({
      type: "template";
      entityId: string;
      entityRevision: number;
      frame: number;
      layer: number;
    } & NativeDescriptorBindingInput);

export interface StageNativeExtensionInput {
  operations: NativeExtensionOperationInput[];
  maxChangedEntities?: number;
}

export interface ReconciliationDecisionInput {
  entryId: string;
  choice:
    | "import_into_take_graph"
    | "detach_from_take_graph"
    | "re_export_canonical";
}

export interface StageRenderInput {
  checkpointOperationId: string;
  profile: string;
  outputPath: string;
  overwrite?: boolean;
}

interface NativeExtensionDescriptorResult {
  targetCatalog: {
    descriptors: Array<{
      descriptorId: string;
      configDigest: string;
      schemaDigest: string;
      bindable: boolean;
      mutationAllowed: boolean;
    }>;
  };
  planningDescriptorDigests: Record<string, string>;
  [key: string]: unknown;
}

interface NativeExtensionStagedResult {
  digest: string;
  operationId: string;
  [key: string]: unknown;
}

export class Ymm4Workflow {
  private readonly stateDirectory: string;
  private readonly artifactDirectory: string;
  private readonly nativeVoiceArtifactDirectory: string;
  private readonly projectStateRoot: string;
  private readonly projectOperationRoot: string;
  private readonly executable: string;
  private readonly executableArgs: string[];

  constructor(
    options: {
      stateDirectory?: string;
      artifactDirectory?: string;
      nativeVoiceArtifactDirectory?: string;
      projectStateRoot?: string;
      projectOperationRoot?: string;
      executable?: string;
      executableArgs?: string[];
    } = {},
  ) {
    this.stateDirectory =
      options.stateDirectory ?? path.join(workspaceRoot, ".takegraph", "ymm4");
    this.artifactDirectory =
      options.artifactDirectory ??
      path.join(workspaceRoot, "artifacts", "voicevox");
    this.nativeVoiceArtifactDirectory =
      options.nativeVoiceArtifactDirectory ??
      path.join(workspaceRoot, "artifacts", "ymm4-native-voice");
    this.projectStateRoot =
      configuredDirectory(
        options.projectStateRoot,
        "TAKEGRAPH_PROJECT_STATE_ROOT",
        path.join(workspaceRoot, ".takegraph", "project-store"),
      );
    this.projectOperationRoot =
      configuredDirectory(
        options.projectOperationRoot,
        "TAKEGRAPH_PROJECT_OPERATION_ROOT",
        path.join(workspaceRoot, ".takegraph", "project-operations"),
      );
    this.executable = options.executable ?? defaultExecutable;
    this.executableArgs = options.executableArgs ?? [];
  }

  /** Content-addressed root populated by the scene CLI for MCP image review. */
  sceneCaptureArtifactRoot(): string {
    return path.join(this.stateDirectory, "artifacts", "scene-captures");
  }

  async describe() {
    const [health, capabilities, snapshot] = await Promise.all([
      this.runJson(["ymm4", "health"]),
      this.runJson(["ymm4", "capabilities"]),
      this.runJson(["ymm4", "snapshot"]),
    ]);
    return { health, capabilities, snapshot };
  }

  async stage(input: StageYmm4Input) {
    await fs.mkdir(this.stateDirectory, { recursive: true });
    const canonical = await this.readHead();
    const head = canonical.revision;
    const snapshot = (await this.runJson([
      "ymm4",
      "snapshot",
      ...this.expectedProjectArgs(canonical),
    ])) as Ymm4Snapshot;
    const voice = (await this.runJson([
      "voicevox",
      "materialize",
      "--speaker",
      input.speaker,
      "--style",
      input.style,
      "--text",
      input.spokenText,
      "--artifact-root",
      this.artifactDirectory,
    ])) as VoiceArtifactResult;
    const length = Math.ceil(
      (voice.artifact.wav.duration_samples * snapshot.fps) /
        voice.artifact.wav.sample_rate,
    );
    const handle = randomUUID();
    const manifestFile = this.resolveHandle(handle, "manifest.json");
    const patchFile = this.resolveHandle(handle, "patch.json");
    await fs.writeFile(
      manifestFile,
      JSON.stringify(
        [
          {
            entityId: input.entityId,
            revision: head,
            speaker: voice.speaker,
            caption: input.caption,
            spokenText: input.spokenText,
            audioPath: voice.artifact.audio_path,
            artifactHash: voice.artifact.audio_hash,
            frame: input.frame,
            length,
            audioLayer: input.audioLayer,
            captionLayer: input.captionLayer,
          },
        ],
        null,
        2,
      ),
      "utf8",
    );
    const staged = (await this.runJson([
      "ymm4",
      "export-stage",
      "--state-root",
      this.projectStateRoot,
      "--manifest",
      manifestFile,
      "--patch",
      patchFile,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ])) as StagedResult;
    return {
      handle,
      digest: staged.digest,
      baseRevision: head,
      project: staged.project,
      impact: staged.plan,
      targetPlanDigest: staged.targetPlanDigest,
      targetPlan: staged.targetPlan,
      placement: {
        frame: input.frame,
        length,
        audioLayer: input.audioLayer,
        captionLayer: input.captionLayer,
      },
      voice,
    };
  }

  async commit(handle: string, digest: string) {
    const patchFile = this.resolveHandle(handle, "patch.json");
    const taskBase = await this.readTaskBase(patchFile);
    const canonical = await this.readHead();
    const head = canonical.revision;
    const committed = await this.runJson([
      "ymm4",
      "export-commit",
      "--state-root",
      this.projectStateRoot,
      "--patch",
      patchFile,
      "--digest",
      digest,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
    this.validateCanonicalMutationResult(
      committed,
      taskBase,
      head,
      "YMM4 export",
    );
    return committed;
  }

  async verify(handle: string) {
    const patchFile = this.resolveHandle(handle, "patch.json");
    return this.runJson([
      "ymm4",
      "export-verify",
      "--patch",
      patchFile,
    ]);
  }

  async stageNativeVoice(input: StageNativeVoiceInput) {
    if (input.displayText !== input.spokenText) {
      throw new Error(
        "displayText and spokenText must be identical for the current YMM4 native voice slice",
      );
    }

    await fs.mkdir(this.stateDirectory, { recursive: true });
    const canonical = await this.readHead();
    const head = canonical.revision;
    const handle = randomUUID();
    const realizationId = randomUUID();
    const manifestFile = this.resolveHandle(
      handle,
      "native-voice.manifest.json",
    );
    const patchFile = this.resolveHandle(handle, "native-voice.patch.json");
    await fs.writeFile(
      manifestFile,
      JSON.stringify(
        [
          {
            realizationId,
            entityId: input.entityId,
            revision: head,
            characterName: input.characterName,
            displayText: input.displayText,
            spokenText: input.spokenText,
            frame: input.frame,
            layer: input.layer,
            maxLength: input.maxLength,
          },
        ],
        null,
        2,
      ),
      "utf8",
    );
    const staged = (await this.runJson([
      "ymm4",
      "native-voice-stage",
      "--state-root",
      this.projectStateRoot,
      "--manifest",
      manifestFile,
      "--patch",
      patchFile,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ])) as NativeVoiceStagedResult;
    return {
      handle,
      realizationId,
      digest: staged.digest,
      baseRevision: head,
      project: staged.project,
      impact: staged.plan,
      targetPlanDigest: staged.targetPlanDigest,
      targetPlan: staged.targetPlan,
      placement: {
        frame: input.frame,
        layer: input.layer,
        maxLength: input.maxLength,
      },
    };
  }

  async commitNativeVoice(handle: string, digest: string) {
    const patchFile = this.resolveHandle(handle, "native-voice.patch.json");
    const taskBase = await this.readTaskBase(patchFile);
    const canonical = await this.readHead();
    const head = canonical.revision;
    const committed = await this.runJson([
      "ymm4",
      "native-voice-commit",
      "--state-root",
      this.projectStateRoot,
      "--patch",
      patchFile,
      "--digest",
      digest,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
    this.validateCanonicalMutationResult(
      committed,
      taskBase,
      head,
      "YMM4 native voice export",
    );
    return committed;
  }

  async verifyNativeVoice(handle: string) {
    const patchFile = this.resolveHandle(handle, "native-voice.patch.json");
    return this.runJson([
      "ymm4",
      "native-voice-verify",
      "--patch",
      patchFile,
    ]);
  }

  async stageNativeVoiceMutations(input: StageNativeVoiceMutationsInput) {
    if (input.mutations.length < 1 || input.mutations.length > 128) {
      throw new Error("YMM4 native voice mutation requires 1-128 operations");
    }
    await fs.mkdir(this.stateDirectory, { recursive: true });
    const canonical = await this.readHead();
    const head = canonical.revision;
    const handle = randomUUID();
    const manifestFile = this.resolveHandle(
      handle,
      "native-voice-mutation.manifest.json",
    );
    const patchFile = this.resolveHandle(
      handle,
      "native-voice-mutation.patch.json",
    );
    const realizationIds: string[] = [];
    const mutations = input.mutations.map((mutation) => {
      if (!Number.isSafeInteger(mutation.revision) || mutation.revision < 0) {
        throw new Error("YMM4 native voice mutation revision must be a non-negative safe integer");
      }
      const realizationId =
        mutation.action === "create"
          ? (mutation.realizationId ?? randomUUID())
          : mutation.realizationId;
      requireUuid(realizationId, "native voice realizationId");
      realizationIds.push(realizationId);
      if (mutation.action === "delete") {
        return {
          realizationId,
          entityId: mutation.entityId,
          revision: mutation.revision,
          characterName: "",
          displayText: "",
          spokenText: "",
          frame: 0,
          layer: 0,
          maxLength: 1,
          action: mutation.action,
        };
      }
      if (mutation.displayText !== mutation.spokenText) {
        throw new Error(
          "displayText and spokenText must be identical for YMM4 native voice create/update",
        );
      }
      return { ...mutation, realizationId };
    });
    if (new Set(realizationIds).size !== realizationIds.length) {
      throw new Error("YMM4 native voice mutation realizationIds must be unique");
    }
    if (new Set(mutations.map((mutation) => mutation.entityId)).size !== mutations.length) {
      throw new Error("YMM4 native voice mutation entityIds must be unique");
    }
    await fs.writeFile(manifestFile, JSON.stringify(mutations, null, 2), "utf8");
    const staged = (await this.runJson([
      "ymm4",
      "native-voice-mutation-stage",
      "--state-root",
      this.projectStateRoot,
      "--manifest",
      manifestFile,
      "--patch",
      patchFile,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ])) as NativeVoiceMutationStagedResult;
    return {
      handle,
      realizationIds,
      digest: staged.digest,
      baseRevision: staged.baseRevision,
      operationId: staged.operationId,
      project: staged.project,
      impact: staged.plan,
      capabilityDigest: staged.capabilityDigest,
    };
  }

  async commitNativeVoiceMutations(handle: string, digest: string) {
    const patchFile = this.resolveHandle(handle, "native-voice-mutation.patch.json");
    const taskBase = await this.readTaskBase(patchFile);
    const canonical = await this.readHead();
    const head = canonical.revision;
    const result = await this.runJson([
      "ymm4",
      "native-voice-mutation-commit",
      "--state-root",
      this.projectStateRoot,
      "--patch",
      patchFile,
      "--digest",
      digest,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
    this.validateCanonicalMutationResult(
      result,
      taskBase,
      head,
      "YMM4 native voice mutation",
    );
    return result;
  }

  async verifyNativeVoiceMutations(handle: string) {
    return this.runJson([
      "ymm4",
      "native-voice-mutation-verify",
      "--patch",
      this.resolveHandle(handle, "native-voice-mutation.patch.json"),
    ]);
  }

  async captureNativeVoiceMutationArtifacts(handle: string) {
    return this.runJson([
      "ymm4",
      "native-voice-mutation-artifacts",
      "--state-root",
      this.projectStateRoot,
      "--patch",
      this.resolveHandle(handle, "native-voice-mutation.patch.json"),
      "--artifact-root",
      this.nativeVoiceArtifactDirectory,
    ]);
  }

  async nativeExtensionDescriptors() {
    return this.runJson([
      "ymm4",
      "native-extension-descriptors",
    ]) as Promise<NativeExtensionDescriptorResult>;
  }

  async stageNativeExtension(input: StageNativeExtensionInput) {
    if (input.operations.length === 0) {
      throw new Error("At least one native-extension operation is required");
    }
    await fs.mkdir(this.stateDirectory, { recursive: true });
    const canonical = await this.readHead();
    const head = canonical.revision;
    const descriptors = await this.nativeExtensionDescriptors();
    const handle = randomUUID();
    const manifestFile = this.resolveHandle(
      handle,
      "native-extension.manifest.json",
    );
    const taskFile = this.resolveHandle(handle, "native-extension.task.json");
    const artifactSources = new Map<string, string>();
    const intents = input.operations.map((operation) => {
      const descriptor =
        "descriptorId" in operation
          ? this.resolveNativeDescriptor(descriptors, operation)
          : undefined;
      const placement =
        "frame" in operation
          ? {
              frame: operation.frame,
              primaryLayer: operation.layer,
              secondaryLayer: null,
            }
          : undefined;
      const replacementGuard = {
        approvedLossyFields:
          "approvedLossyFields" in operation
            ? (operation.approvedLossyFields ?? [])
            : [],
      };
      switch (operation.type) {
        case "portrait":
        case "face":
          return {
            type: "upsert_portrait",
            intent: {
              entityId: operation.entityId,
              entityRevision: operation.entityRevision,
              presentation: operation.type,
              characterBinding: descriptor,
              placement,
              durationFrames: operation.durationFrames,
              replacementGuard,
            },
          };
        case "image":
        case "video":
        case "audio":
        case "bgm": {
          const artifactDigest = normalizeSha256(operation.artifactDigest);
          const existing = artifactSources.get(artifactDigest);
          if (existing && path.resolve(existing) !== path.resolve(operation.sourcePath)) {
            throw new Error(
              `Artifact ${artifactDigest} was assigned more than one source path`,
            );
          }
          artifactSources.set(artifactDigest, operation.sourcePath);
          return {
            type: "upsert_asset",
            intent: {
              entityId: operation.entityId,
              entityRevision: operation.entityRevision,
              asset: {
                artifactDigest,
                mediaType: operation.mediaType,
                byteLength: operation.byteLength,
                kind: operation.type,
              },
              placement,
              durationFrames: operation.durationFrames,
              loopPlayback: operation.loopPlayback ?? operation.type === "bgm",
              replacementGuard,
            },
          };
        }
        case "effect":
          return {
            type: "mutate_effect",
            intent: {
              targetEntityId: operation.targetEntityId,
              targetEntityRevision: operation.targetEntityRevision,
              effectInstanceId: operation.effectInstanceId,
              descriptor,
              operation:
                operation.action === "remove"
                  ? { type: "remove" }
                  : {
                      type: "upsert",
                      parameters: operation.parameters ?? {},
                    },
            },
          };
        case "template":
          return {
            type: "instantiate_template",
            intent: {
              entityId: operation.entityId,
              entityRevision: operation.entityRevision,
              template: descriptor,
              placement,
            },
          };
      }
    });
    await fs.writeFile(
      manifestFile,
      JSON.stringify(
        {
          intents,
          artifactSources: [...artifactSources].map(
            ([artifactDigest, sourcePath]) => ({
              artifactDigest,
              sourcePath,
            }),
          ),
          changeBudget: {
            maxChangedEntities:
              input.maxChangedEntities ?? input.operations.length,
            maxShiftedEntities: 0,
            maxShiftFrames: 0,
            allowLockedChanges: false,
            allowUnmanagedChanges: false,
          },
        },
        null,
        2,
      ),
      "utf8",
    );
    const result = (await this.runJson([
      "ymm4",
      "native-extension-stage",
      "--state-root",
      this.projectStateRoot,
      "--manifest",
      manifestFile,
      "--task",
      taskFile,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ])) as NativeExtensionStagedResult;
    return {
      handle,
      descriptors: {
        catalogDigest: (
          descriptors.targetCatalog as { catalogDigest?: string }
        ).catalogDigest,
      },
      ...result,
    };
  }

  async approveNativeExtension(handle: string, digest: string) {
    return this.runNativeExtensionCommand(
      "native-extension-approve",
      handle,
      ["--digest", digest],
    );
  }

  async applyNativeExtension(handle: string) {
    const taskFile = this.resolveHandle(handle, "native-extension.task.json");
    const taskBase = await this.readTaskBase(taskFile);
    const canonical = await this.readHead();
    const head = canonical.revision;
    const result = await this.runNativeExtensionCommand(
      "native-extension-apply",
      handle,
      [],
      true,
      canonical,
    );
    this.validateCanonicalMutationResult(
      result,
      taskBase,
      head,
      "YMM4 native-extension apply",
    );
    return result;
  }

  async verifyNativeExtension(handle: string) {
    return this.runNativeExtensionCommand(
      "native-extension-verify",
      handle,
      [],
      false,
    );
  }

  async nativeExtensionStatus(handle: string) {
    return this.runNativeExtensionCommand(
      "native-extension-status",
      handle,
      [],
      false,
    );
  }

  async stageSceneInspection(
    input: StageSceneInspectionInput,
  ): Promise<SceneInspectionStagedResult> {
    await fs.mkdir(this.stateDirectory, { recursive: true });
    const canonical = await this.readHead();
    const head = canonical.revision;
    const handle = randomUUID();
    const profileFile = this.resolveHandle(handle, "scene-profile.json");
    const taskFile = this.resolveHandle(handle, "scene-inspection.json");
    await fs.writeFile(
      profileFile,
      JSON.stringify(
        {
          expectedWidth: input.expectedWidth,
          expectedHeight: input.expectedHeight,
          blackLumaThreshold: input.blackLumaThreshold ?? 16,
          blackPixelRatioPpm: input.blackPixelRatioPpm ?? 990_000,
          blankChannelSpanThreshold:
            input.blankChannelSpanThreshold ?? 0,
          safeArea: input.safeArea ?? null,
          regions: input.regions ?? [],
        },
        null,
        2,
      ),
      "utf8",
    );
    const result = (await this.runJson([
      "ymm4",
      "scene-stage",
      "--profile",
      profileFile,
      "--profile-id",
      input.profileId ?? "ymm4-preview-default",
      "--max-actual-frame-delta",
      String(input.maxActualFrameDelta ?? 0),
      "--task",
      taskFile,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
      ...(input.alpha ? ["--alpha"] : []),
      ...input.frames.flatMap((frame) => ["--frame", String(frame)]),
    ])) as Omit<SceneInspectionStagedResult, "handle">;
    if (!/^[0-9a-f]{64}$/i.test(result.digest)) {
      throw new Error("YMM4 scene stage returned an invalid plan digest");
    }
    return { ...result, handle };
  }

  async approveSceneInspection(handle: string, digest: string) {
    const taskFile = this.resolveHandle(handle, "scene-inspection.json");
    const canonical = await this.readHead();
    const head = canonical.revision;
    return this.runJson([
      "ymm4",
      "scene-approve",
      "--task",
      taskFile,
      "--digest",
      digest,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
  }

  async captureSceneInspection(handle: string) {
    this.resolveHandle(handle, "scene-inspection.json");
    const canonical = await this.readHead();
    return this.runSceneCommand("scene-capture", handle, canonical);
  }

  async replaySceneInspection(handle: string) {
    this.resolveHandle(handle, "scene-inspection.json");
    const canonical = await this.readHead();
    return this.runSceneCommand("scene-replay", handle, canonical);
  }

  async reviewSceneInspection(handle: string, reviewer: string) {
    this.resolveHandle(handle, "scene-inspection.json");
    const canonical = await this.readHead();
    return this.runSceneCommand("scene-review", handle, canonical, [
      "--reviewer",
      reviewer,
    ]);
  }

  async decideSceneInspection(
    handle: string,
    decision: "accept" | "reject",
    note: string,
  ) {
    this.resolveHandle(handle, "scene-inspection.json");
    const canonical = await this.readHead();
    return this.runSceneCommand("scene-decide", handle, canonical, [
      "--decision",
      decision,
      "--note",
      note,
    ]);
  }

  async sceneInspectionStatus(handle: string) {
    this.resolveHandle(handle, "scene-inspection.json");
    const canonical = await this.readHead();
    return this.runSceneCommand("scene-status", handle, canonical);
  }

  async save() {
    return this.runJson(["ymm4", "save"]);
  }

  async stageCheckpoint() {
    const canonical = await this.readHead();
    const head = canonical.revision;
    return this.runJson([
      "ymm4",
      "checkpoint-stage",
      ...this.projectStoreArgs(),
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
  }

  async executeCheckpoint(operationId: string) {
    requireUuid(operationId, "checkpoint operationId");
    const canonical = await this.readHead();
    const head = canonical.revision;
    return this.runJson([
      "ymm4",
      "checkpoint-execute",
      ...this.projectStoreArgs(),
      "--operation-id",
      operationId,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
  }

  async checkpointStatus(operationId: string) {
    requireUuid(operationId, "checkpoint operationId");
    return this.runJson([
      "ymm4",
      "checkpoint-status",
      "--operation-root",
      this.projectOperationRoot,
      "--operation-id",
      operationId,
    ]);
  }

  async renderProfiles() {
    return this.runJson(["ymm4", "render-profiles"]);
  }

  async stageRender(input: StageRenderInput) {
    requireUuid(input.checkpointOperationId, "render checkpointOperationId");
    if (!input.profile.trim()) {
      throw new Error("YMM4 render profile must be non-empty");
    }
    if (!path.isAbsolute(input.outputPath)) {
      throw new Error("YMM4 render outputPath must be absolute");
    }
    const canonical = await this.readHead();
    const head = canonical.revision;
    return this.runJson([
      "ymm4",
      "render-stage",
      ...this.projectStoreArgs(),
      "--checkpoint-operation-id",
      input.checkpointOperationId,
      "--profile",
      input.profile,
      "--output",
      input.outputPath,
      ...(input.overwrite ? ["--overwrite"] : []),
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
  }

  async executeRender(taskId: string) {
    requireUuid(taskId, "render taskId");
    const canonical = await this.readHead();
    const head = canonical.revision;
    return this.runJson([
      "ymm4",
      "render-execute",
      ...this.projectStoreArgs(),
      "--task-id",
      taskId,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
  }

  async renderStatus(taskId: string) {
    requireUuid(taskId, "render taskId");
    return this.runJson([
      "ymm4",
      "render-status",
      "--operation-root",
      this.projectOperationRoot,
      "--task-id",
      taskId,
    ]);
  }

  async cancelRender(taskId: string) {
    requireUuid(taskId, "render taskId");
    return this.runJson([
      "ymm4",
      "render-cancel",
      "--operation-root",
      this.projectOperationRoot,
      "--task-id",
      taskId,
    ]);
  }

  async reconciliationReport() {
    const canonical = await this.readHead();
    const head = canonical.revision;
    const handle = randomUUID();
    const result = (await this.runJson([
      "ymm4",
      "reconcile-report",
      ...this.projectStoreArgs(),
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ])) as Record<string, unknown>;
    return { handle, ...result };
  }

  async previewReconciliation(
    reportDigest: string,
    decisions: ReconciliationDecisionInput[],
  ) {
    await fs.mkdir(this.stateDirectory, { recursive: true });
    const handle = randomUUID();
    const decisionsFile = this.resolveHandle(
      handle,
      "reconciliation-decisions.json",
    );
    await fs.writeFile(
      decisionsFile,
      JSON.stringify(
        decisions.map((decision) => ({
          ...decision,
          entryId: normalizeRawSha256(decision.entryId),
        })),
        null,
        2,
      ),
      "utf8",
    );
    const result = (await this.runJson([
      "ymm4",
      "reconcile-preview",
      "--operation-root",
      this.projectOperationRoot,
      "--report-digest",
      normalizeRawSha256(reportDigest),
      "--decisions",
      decisionsFile,
    ])) as Record<string, unknown>;
    return { handle, ...result };
  }

  async applyReconciliation(reportDigest: string, approvalDigest: string) {
    const canonical = await this.readHead();
    const head = canonical.revision;
    return this.runJson([
      "ymm4",
      "reconcile-apply",
      ...this.projectStoreArgs(),
      "--report-digest",
      normalizeRawSha256(reportDigest),
      "--digest",
      normalizeRawSha256(approvalDigest),
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
  }

  async reconciliationChildStatus(childTaskId: string) {
    return this.runJson([
      "ymm4",
      "reconcile-child-status",
      "--operation-root",
      this.projectOperationRoot,
      "--child-task-id",
      normalizeRawSha256(childTaskId),
    ]);
  }

  async approveReconciliationDetach(childTaskId: string, approvalDigest: string) {
    const { revision: head } = await this.readHead();
    return this.runJson([
      "ymm4",
      "reconcile-detach-approve",
      ...this.projectStoreArgs(),
      "--child-task-id",
      normalizeRawSha256(childTaskId),
      "--digest",
      normalizeRawSha256(approvalDigest),
      "--head",
      String(head),
    ]);
  }

  async executeReconciliationDetach(childTaskId: string) {
    const canonical = await this.readHead();
    const head = canonical.revision;
    return this.runJson([
      "ymm4",
      "reconcile-detach-execute",
      ...this.projectStoreArgs(),
      "--child-task-id",
      normalizeRawSha256(childTaskId),
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ]);
  }

  async dispatchReconciliationReExport(
    childTaskId: string,
    manifest: Record<string, unknown>,
  ) {
    await fs.mkdir(this.stateDirectory, { recursive: true });
    const canonical = await this.readHead();
    const head = canonical.revision;
    const route = manifest.route;
    const outputSuffix =
      route === "portable_pair"
        ? "patch.json"
        : route === "native_voice_mutation"
          ? "native-voice-mutation.patch.json"
          : route === "native_extension"
            ? "native-extension.task.json"
            : undefined;
    if (!outputSuffix) {
      throw new Error(
        "Reconciliation re-export route must be portable_pair, native_voice_mutation, or native_extension",
      );
    }
    const handle = randomUUID();
    const manifestFile = this.resolveHandle(
      handle,
      "reconciliation-re-export.json",
    );
    const outputTaskFile = this.resolveHandle(handle, outputSuffix);
    await fs.writeFile(manifestFile, JSON.stringify(manifest, null, 2), "utf8");
    const result = (await this.runJson([
      "ymm4",
      "reconcile-re-export-dispatch",
      ...this.projectStoreArgs(),
      "--child-task-id",
      normalizeRawSha256(childTaskId),
      "--manifest",
      manifestFile,
      "--output-task",
      outputTaskFile,
      "--head",
      String(head),
      ...this.expectedProjectArgs(canonical),
    ])) as Record<string, unknown>;
    const payload = result.payload as
      | {
          type?: unknown;
          task?: {
            status?: unknown;
            downstreamPreview?: {
              route?: unknown;
              preview?: { patch?: { status?: unknown; approvedDigest?: unknown; digest?: unknown; base?: unknown } };
            };
          };
        }
      | undefined;
    const downstream = payload?.task?.downstreamPreview;
    const patch = downstream?.preview?.patch;
    if (
      payload?.type !== "canonical_re_export" ||
      payload.task?.status !== "preview_ready" ||
      downstream?.route !== route ||
      patch?.status !== "previewable" ||
      patch.approvedDigest !== null ||
      typeof patch.digest !== "string"
    ) {
      throw new Error(
        "Reconciliation dispatcher did not return the expected unapproved downstream preview",
      );
    }
    return {
      ...result,
      handle,
      outputTask: outputTaskFile,
      downstreamRoute: route,
      digest: patch.digest,
      baseRevision: patch.base,
    };
  }

  private resolveHandle(handle: string, suffix: string): string {
    if (
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
        handle,
      )
    ) {
      throw new Error("Invalid YMM4 export handle");
    }
    return path.join(this.stateDirectory, `${handle}.${suffix}`);
  }

  private async readHead(): Promise<CanonicalContext> {
    const result = (await this.runJson([
      "ymm4",
      "canonical-head",
      "--state-root",
      this.projectStateRoot,
    ])) as { projectId?: unknown; revision?: unknown };
    if (
      typeof result.projectId !== "string" ||
      result.projectId.trim().length === 0 ||
      typeof result.revision !== "number" ||
      !Number.isSafeInteger(result.revision) ||
      result.revision < 0
    ) {
      throw new Error("YMM4 canonical-head returned an invalid revision");
    }
    return { projectId: result.projectId, revision: result.revision };
  }

  private expectedProjectArgs(context: CanonicalContext): string[] {
    return ["--expected-project-id", context.projectId];
  }

  private async readTaskBase(taskFile: string): Promise<number> {
    const task = JSON.parse(await fs.readFile(taskFile, "utf8")) as {
      patch?: { base?: unknown };
    };
    const base = task.patch?.base;
    if (typeof base !== "number" || !Number.isSafeInteger(base) || base < 0) {
      throw new Error("YMM4 task file has an invalid canonical base revision");
    }
    return base;
  }

  private validateCanonicalMutationResult(
    value: unknown,
    taskBase: number,
    currentHead: number,
    label: string,
  ): void {
    const result = value as {
      baseRevision?: unknown;
      revision?: unknown;
      canonicalReplay?: unknown;
    };
    const base = result.baseRevision;
    const revision = result.revision;
    const replay = result.canonicalReplay;
    if (
      typeof base !== "number" ||
      !Number.isSafeInteger(base) ||
      base !== taskBase ||
      typeof revision !== "number" ||
      !Number.isSafeInteger(revision) ||
      typeof replay !== "boolean"
    ) {
      throw new Error(`${label} returned an invalid canonical result`);
    }
    const validFresh =
      !replay && base === currentHead && revision === currentHead + 1;
    const validReplay =
      replay && revision === base + 1 && revision <= currentHead;
    if (!validFresh && !validReplay) {
      throw new Error(`${label} returned an invalid TakeGraph revision`);
    }
  }

  private projectStoreArgs(): string[] {
    return [
      "--state-root",
      this.projectStateRoot,
      "--operation-root",
      this.projectOperationRoot,
    ];
  }

  private async runSceneCommand(
    command: string,
    handle: string,
    canonical: CanonicalContext,
    extraArgs: string[] = [],
  ): Promise<unknown> {
    return this.runJson([
      "ymm4",
      command,
      "--task",
      this.resolveHandle(handle, "scene-inspection.json"),
      "--current-profile",
      this.resolveHandle(handle, "scene-profile.json"),
      "--head",
      String(canonical.revision),
      ...this.expectedProjectArgs(canonical),
      ...extraArgs,
    ]);
  }

  private resolveNativeDescriptor(
    catalog: NativeExtensionDescriptorResult,
    binding: NativeDescriptorBindingInput,
  ) {
    const descriptor = catalog.targetCatalog.descriptors.find(
      (candidate) => candidate.descriptorId === binding.descriptorId,
    );
    if (!descriptor) {
      throw new Error(`YMM4 native descriptor not found: ${binding.descriptorId}`);
    }
    if (!descriptor.bindable) {
      throw new Error(`YMM4 native descriptor is ambiguous: ${binding.descriptorId}`);
    }
    if (
      normalizeRawSha256(descriptor.configDigest) !==
        normalizeRawSha256(binding.expectedConfigDigest) ||
      normalizeRawSha256(descriptor.schemaDigest) !==
        normalizeRawSha256(binding.expectedSchemaDigest)
    ) {
      throw new Error(
        `YMM4 native descriptor config/schema drifted: ${binding.descriptorId}`,
      );
    }
    const expectedDigest =
      catalog.planningDescriptorDigests[binding.descriptorId];
    if (!expectedDigest) {
      throw new Error(
        `YMM4 native descriptor is not enabled for mutation: ${binding.descriptorId}`,
      );
    }
    return { descriptorId: binding.descriptorId, expectedDigest };
  }

  private async runNativeExtensionCommand(
    command: string,
    handle: string,
    extraArgs: string[] = [],
    includeHead = true,
    canonicalContext?: CanonicalContext,
  ): Promise<unknown> {
    const taskFile = this.resolveHandle(handle, "native-extension.task.json");
    const canonical = includeHead
      ? (canonicalContext ?? await this.readHead())
      : undefined;
    return this.runJson([
      "ymm4",
      command,
      "--task",
      taskFile,
      ...(includeHead ? ["--state-root", this.projectStateRoot] : []),
      ...(canonical === undefined
        ? []
        : [
            "--head",
            String(canonical.revision),
            ...this.expectedProjectArgs(canonical),
          ]),
      ...extraArgs,
    ]);
  }

  private async runJson(args: string[]): Promise<unknown> {
    const { stdout } = await execFileAsync(
      this.executable,
      [...this.executableArgs, ...args],
      {
        cwd: workspaceRoot,
        maxBuffer: 16 * 1024 * 1024,
        windowsHide: true,
      },
    );
    return JSON.parse(stdout);
  }
}

function normalizeRawSha256(value: string): string {
  const normalized = value.toLowerCase().replace(/^sha256:/, "");
  if (!/^[0-9a-f]{64}$/.test(normalized)) {
    throw new Error("Expected a SHA-256 digest");
  }
  return normalized;
}

function normalizeSha256(value: string): string {
  return `sha256:${normalizeRawSha256(value)}`;
}

function requireUuid(value: string, field: string): void {
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
      value,
    )
  ) {
    throw new Error(`Invalid YMM4 ${field}`);
  }
}
