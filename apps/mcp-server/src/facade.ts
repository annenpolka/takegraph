import { registerAppTool } from "@modelcontextprotocol/ext-apps/server";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import {
  formatAgentError,
  formatCompositionText,
  formatDescriptorInventoryText,
  formatRenderProfilesText,
  formatStatusText,
  formatStudioSessionText,
  formatTaskListText,
  formatYmm4DescribeText,
  TAKEGRAPH_AGENT_GUIDE,
  type CanonicalDescribeInput,
} from "./agent-text.js";
import type { ProjectSession, ProjectState } from "./project-session.js";
import {
  TASK_KINDS,
  buildTaskEnvelope,
  makeTaskId,
  parseTaskId,
  type JsonValue,
  type TaskAction,
  type TaskEnvelope,
  type TaskEnvelopeInput,
  type TaskKind,
} from "./task-envelope.js";
import type {
  NativeExtensionOperationInput,
  NativeVoiceMutationInput,
  ReconciliationDecisionInput,
  StageNativeExtensionInput,
  StageNativeVoiceRequest,
  StageNativeVoiceMutationsInput,
  StageRenderInput,
  StageSceneInspectionInput,
  StageTimelineEditInput,
  StageYmm4Request,
  TimelineEditOperationInput,
  Ymm4Workflow,
} from "./ymm4-workflow.js";

type UnknownRecord = Record<string, unknown>;

export type FacadeSceneResult = (
  result: unknown,
  message: string,
  includeImages?: boolean,
) => Promise<CallToolResult>;

export interface RegisterFacadeToolsOptions {
  session: ProjectSession;
  ymm4: Ymm4Workflow;
  registry: TaskFacadeRegistry;
  sceneResult: FacadeSceneResult;
  resourceUri: string;
}

export const INSPECT_VIEWS = [
  "overview",
  "studio",
  "canonical",
  "scene",
  "catalog",
  "tasks",
  "task",
] as const;

/**
 * Process-local presentation cache for the uniform facade contract.
 *
 * It is deliberately not an authority: every approval and execution is still
 * checked by the owning studio session or durable YMM4 workflow. Encoding the
 * workflow kind in the public task ID also means persisted native tasks remain
 * routable after this cache is empty.
 */
export class TaskFacadeRegistry {
  private readonly tasks = new Map<string, TaskEnvelope>();

  constructor(private readonly maxTasks = 256) {
    if (!Number.isSafeInteger(maxTasks) || maxTasks < 1) {
      throw new Error("TaskFacadeRegistry maxTasks must be a positive safe integer");
    }
  }

  remember(input: TaskEnvelopeInput): TaskEnvelope {
    const task = buildTaskEnvelope(input);
    this.tasks.delete(task.taskId);
    this.tasks.set(task.taskId, structuredClone(task));
    while (this.tasks.size > this.maxTasks) {
      const oldest = this.tasks.keys().next().value as string | undefined;
      if (!oldest) break;
      this.tasks.delete(oldest);
    }
    return structuredClone(task);
  }

  get(taskId: string): TaskEnvelope | undefined {
    const task = this.tasks.get(taskId);
    return task ? structuredClone(task) : undefined;
  }

  list(): TaskEnvelope[] {
    return [...this.tasks.values()]
      .map((task) => structuredClone(task))
      .sort((left, right) =>
        left.taskId < right.taskId ? -1 : left.taskId > right.taskId ? 1 : 0,
      );
  }
}

const sha256Schema = z
  .string()
  .regex(/^(?:sha256:)?[0-9a-f]{64}$/i, "Expected a SHA-256 digest");

const pixelRectSchema = z.object({
  x: z.number().int().min(0),
  y: z.number().int().min(0),
  width: z.number().int().positive(),
  height: z.number().int().positive(),
});

const sceneRegionSchema = z.object({
  regionId: z.string().min(1),
  kind: z.enum(["caption", "portrait"]),
  bounds: pixelRectSchema,
  background: z.object({
    red: z.number().int().min(0).max(255),
    green: z.number().int().min(0).max(255),
    blue: z.number().int().min(0).max(255),
    alpha: z.number().int().min(0).max(255),
  }),
  colorTolerance: z.number().int().min(0).max(255).default(8),
  minForegroundPpm: z.number().int().min(0).max(1_000_000).default(1),
  minimumEdgeClearancePx: z.number().int().min(0).default(0),
});

const optionalSpokenText = z.string().min(1).optional();

const nativeVoiceMutationWriteFields = {
  entityId: z.string().min(1),
  revision: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
  characterName: z.string().min(1),
  displayText: z.string().min(1),
  spokenText: optionalSpokenText,
  frame: z.number().int().min(0),
  layer: z.number().int().min(0),
  maxLength: z.number().int().positive(),
};

const nativeVoiceMutationSchema = z
  .discriminatedUnion("action", [
    z.object({
      action: z.literal("create"),
      realizationId: z.string().uuid().optional(),
      ...nativeVoiceMutationWriteFields,
    }),
    z.object({
      action: z.literal("update"),
      realizationId: z.string().uuid(),
      ...nativeVoiceMutationWriteFields,
    }),
    z.object({
      action: z.literal("delete"),
      realizationId: z.string().uuid(),
      entityId: z.string().min(1),
      revision: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
    }),
  ]);

const nativeDescriptorBindingFields = {
  descriptorId: z.string().min(1),
  expectedConfigDigest: sha256Schema,
  expectedSchemaDigest: sha256Schema,
};

const nativePlacementFields = {
  frame: z.number().int().min(0),
  layer: z.number().int().min(0),
};

const nativeLossFields = {
  approvedLossyFields: z.array(z.string().min(1)).default([]),
};

const nativeEffectParameterSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("boolean"), value: z.boolean() }),
  z.object({ type: z.literal("integer"), value: z.number().int() }),
  z.object({
    type: z.literal("fixed"),
    value: z.object({
      scale: z.number().int().positive(),
      scaled: z.number().int(),
    }),
  }),
  z.object({ type: z.literal("text"), value: z.string() }),
  z.object({ type: z.literal("choice"), value: z.string() }),
  z.object({
    type: z.literal("color_rgba"),
    value: z.tuple([
      z.number().int().min(0).max(255),
      z.number().int().min(0).max(255),
      z.number().int().min(0).max(255),
      z.number().int().min(0).max(255),
    ]),
  }),
]);

const nativePortraitOperationSchema = (type: "portrait" | "face") =>
  z.object({
    type: z.literal(type),
    entityId: z.string().min(1),
    entityRevision: z.number().int().min(0),
    ...nativeDescriptorBindingFields,
    ...nativePlacementFields,
    durationFrames: z.number().int().positive(),
    ...nativeLossFields,
  });

const nativeAssetOperationSchema = (
  type: "image" | "video" | "audio" | "bgm",
) =>
  z.object({
    type: z.literal(type),
    entityId: z.string().min(1),
    entityRevision: z.number().int().min(0),
    sourcePath: z.string().min(1),
    artifactDigest: sha256Schema,
    mediaType: z.string().regex(/^(?:image|video|audio)\//),
    byteLength: z.number().int().positive(),
    ...nativePlacementFields,
    durationFrames: z.number().int().positive(),
    loopPlayback: z.boolean().optional(),
    ...nativeLossFields,
  });

const nativeExtensionOperationSchema = z.discriminatedUnion("type", [
  nativePortraitOperationSchema("portrait"),
  nativePortraitOperationSchema("face"),
  nativeAssetOperationSchema("image"),
  nativeAssetOperationSchema("video"),
  nativeAssetOperationSchema("audio"),
  nativeAssetOperationSchema("bgm"),
  z.object({
    type: z.literal("effect"),
    targetEntityId: z.string().min(1),
    targetEntityRevision: z.number().int().min(0),
    effectInstanceId: z.string().min(1),
    ...nativeDescriptorBindingFields,
    action: z.enum(["upsert", "remove"]),
    parameters: z.record(z.string(), nativeEffectParameterSchema).default({}),
  }),
  z.object({
    type: z.literal("template"),
    entityId: z.string().min(1),
    entityRevision: z.number().int().min(0),
    ...nativeDescriptorBindingFields,
    ...nativePlacementFields,
  }),
]);

const reconciliationDecisionSchema = z.object({
  entryId: sha256Schema,
  choice: z.enum([
    "import_into_take_graph",
    "detach_from_take_graph",
    "re_export_canonical",
  ]),
});

const studioTakeStageSchema = z.object({
  kind: z.literal("studio_take"),
  takeId: z.string().min(1),
});

const studioVariantStageSchema = z.object({
  kind: z.literal("studio_voice_variant"),
  utteranceId: z.string().min(1),
  speed: z.number().min(0.5).max(2),
  intonation: z.number().min(0).max(2),
});

const portableVoiceItemFields = {
  entityId: z.string().min(1),
  caption: z.string().min(1),
  spokenText: z.string().min(1),
  speaker: z.string().min(1).default("春日部つむぎ"),
  style: z.string().min(1).default("ノーマル"),
  frame: z.number().int().min(0),
  audioLayer: z.number().int().min(0).default(20),
  captionLayer: z.number().int().min(0).default(21),
};

const portableVoiceItemSchema = z.object(portableVoiceItemFields);

const portableVoiceStageSchema = z.union([
  z.object({
    kind: z.literal("portable_voice"),
    items: z.array(portableVoiceItemSchema).min(1).max(128),
  }).superRefine((value, context) => {
    const seen = new Set<string>();
    value.items.forEach((item, index) => {
      if (seen.has(item.entityId)) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          message: "entityIds must be unique within one task",
          path: ["items", index, "entityId"],
        });
      }
      seen.add(item.entityId);
    });
  }),
  z.object({
    kind: z.literal("portable_voice"),
    ...portableVoiceItemFields,
  }),
]);

const nativeVoiceItemSchema = z
  .object({
    entityId: z.string().min(1),
    displayText: z.string().min(1),
    spokenText: optionalSpokenText,
    characterName: z.string().min(1),
    frame: z.number().int().min(0),
    layer: z.number().int().min(0),
    maxLength: z.number().int().positive(),
  });

const nativeVoiceStageSchema = z.union([
  z.object({
    kind: z.literal("native_voice"),
    items: z.array(nativeVoiceItemSchema).min(1).max(128),
  }).superRefine((value, context) => {
    const seen = new Set<string>();
    value.items.forEach((item, index) => {
      if (seen.has(item.entityId)) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          message: "entityIds must be unique within one task",
          path: ["items", index, "entityId"],
        });
      }
      seen.add(item.entityId);
    });
  }),
  z.object({
    kind: z.literal("native_voice"),
    entityId: z.string().min(1),
    displayText: z.string().min(1),
    spokenText: optionalSpokenText,
    characterName: z.string().min(1),
    frame: z.number().int().min(0),
    layer: z.number().int().min(0),
    maxLength: z.number().int().positive(),
  }),
]);

const nativeVoiceMutationsStageSchema = z.object({
  kind: z.literal("native_voice_mutation"),
  mutations: z.array(nativeVoiceMutationSchema).min(1).max(128),
});

const nativeExtensionStageSchema = z.object({
  kind: z.literal("native_extension"),
  operations: z.array(nativeExtensionOperationSchema).min(1).max(128),
  maxChangedEntities: z.number().int().positive().optional(),
});

const timelineEditOperationSchema = z.discriminatedUnion("op", [
  z.object({
    op: z.literal("portable_voice_create"),
    ...portableVoiceItemFields,
  }),
  z.object({
    op: z.literal("native_voice_create"),
    entityId: z.string().min(1),
    displayText: z.string().min(1),
    spokenText: optionalSpokenText,
    characterName: z.string().min(1),
    frame: z.number().int().min(0),
    layer: z.number().int().min(0),
    maxLength: z.number().int().positive(),
  }),
]);

const timelineEditStageSchema = z
  .object({
    kind: z.literal("timeline_edit"),
    operations: z.array(timelineEditOperationSchema).min(1).max(128),
    maxChangedEntities: z.number().int().min(1).max(128).optional(),
  })
  .superRefine((value, context) => {
    if (
      value.maxChangedEntities !== undefined &&
      value.maxChangedEntities < value.operations.length
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message: "maxChangedEntities cannot be smaller than the operation count",
        path: ["maxChangedEntities"],
      });
    }
    const seenManagedCreates = new Set<string>();
    value.operations.forEach((operation, index) => {
      if (seenManagedCreates.has(operation.entityId)) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          message: "managed create entityIds must be unique within one timeline edit",
          path: ["operations", index, "entityId"],
        });
      }
      seenManagedCreates.add(operation.entityId);
    });
  });

const projectInitializationStageSchema = z.discriminatedUnion("mode", [
  z
    .object({
      kind: z.literal("project_initialization"),
      mode: z.literal("adopt_active"),
    })
    .strict(),
  z
    .object({
      kind: z.literal("project_initialization"),
      mode: z.literal("save_untitled"),
      path: z.string().min(1),
    })
    .strict(),
]);

const sceneStageSchema = z.object({
  kind: z.literal("scene_inspection"),
  frames: z.array(z.number().int().min(0)).min(1).max(16),
  expectedWidth: z.number().int().positive(),
  expectedHeight: z.number().int().positive(),
  profileId: z.string().min(1).default("ymm4-preview-default"),
  alpha: z.boolean().default(false),
  maxActualFrameDelta: z.number().int().min(0).default(0),
  blackLumaThreshold: z.number().int().min(0).max(255).default(16),
  blackPixelRatioPpm: z
    .number()
    .int()
    .min(1)
    .max(1_000_000)
    .default(990_000),
  blankChannelSpanThreshold: z.number().int().min(0).max(255).default(0),
  safeArea: pixelRectSchema.nullable().optional(),
  regions: z.array(sceneRegionSchema).default([]),
});

const renderStageSchema = z.object({
  kind: z.literal("render"),
  checkpointOperationId: z.string().uuid(),
  profile: z.string().min(1),
  outputPath: z.string().min(1),
  overwrite: z.boolean().default(false),
});

const checkpointStageSchema = z.object({
  kind: z.literal("checkpoint"),
});

const reconciliationReportSchema = z.object({
  kind: z.literal("reconciliation"),
  mode: z.literal("report").default("report"),
});

const reconciliationPreviewSchema = z.object({
  kind: z.literal("reconciliation"),
  mode: z.literal("preview"),
  taskId: z.string().min(1),
  decisions: z.array(reconciliationDecisionSchema).max(4096),
});

const reExportStageSchema = z.object({
  kind: z.literal("reconciliation_re_export"),
  taskId: z.string().min(1),
  manifest: z.record(z.string(), z.unknown()),
});

const stageInputSchema = z.union([
  studioTakeStageSchema,
  studioVariantStageSchema,
  timelineEditStageSchema,
  portableVoiceStageSchema,
  nativeVoiceStageSchema,
  nativeVoiceMutationsStageSchema,
  nativeExtensionStageSchema,
  projectInitializationStageSchema,
  sceneStageSchema,
  checkpointStageSchema,
  renderStageSchema,
  reconciliationReportSchema,
  reconciliationPreviewSchema,
  reExportStageSchema,
]);

const STAGE_KINDS = [
  "studio_take",
  "studio_voice_variant",
  "timeline_edit",
  "portable_voice",
  "native_voice",
  "native_voice_mutation",
  "native_extension",
  "project_initialization",
  "scene_inspection",
  "checkpoint",
  "render",
  "reconciliation",
  "reconciliation_re_export",
] as const;

/**
 * Host-visible object schema for takegraph_task_stage.
 *
 * A Zod union/anyOf collapses to `properties: {}` on some MCP hosts, which then
 * reject every real argument. Runtime validation still uses `stageInputSchema`.
 */
const stageHostInputSchema = z.object({
  kind: z.enum(STAGE_KINDS),
  takeId: z.string().min(1).optional(),
  utteranceId: z.string().min(1).optional(),
  speed: z.number().optional(),
  intonation: z.number().optional(),
  operations: z.array(z.record(z.string(), z.unknown())).optional(),
  maxChangedEntities: z.number().int().positive().optional(),
  items: z.array(z.record(z.string(), z.unknown())).optional(),
  entityId: z.string().min(1).optional(),
  caption: z.string().min(1).optional(),
  spokenText: z.string().min(1).optional(),
  speaker: z.string().min(1).optional(),
  style: z.string().min(1).optional(),
  frame: z.number().int().min(0).optional(),
  audioLayer: z.number().int().min(0).optional(),
  captionLayer: z.number().int().min(0).optional(),
  displayText: z.string().min(1).optional(),
  characterName: z.string().min(1).optional(),
  layer: z.number().int().min(0).optional(),
  maxLength: z.number().int().positive().optional(),
  mutations: z.array(z.record(z.string(), z.unknown())).optional(),
  mode: z.string().min(1).optional(),
  path: z.string().min(1).optional(),
  frames: z.array(z.number().int().min(0)).optional(),
  expectedWidth: z.number().int().positive().optional(),
  expectedHeight: z.number().int().positive().optional(),
  profileId: z.string().min(1).optional(),
  alpha: z.boolean().optional(),
  maxActualFrameDelta: z.number().int().min(0).optional(),
  blackLumaThreshold: z.number().int().min(0).max(255).optional(),
  blackPixelRatioPpm: z.number().int().min(1).max(1_000_000).optional(),
  blankChannelSpanThreshold: z.number().int().min(0).max(255).optional(),
  safeArea: pixelRectSchema.nullable().optional(),
  regions: z.array(z.unknown()).optional(),
  checkpointOperationId: z.string().uuid().optional(),
  profile: z.string().min(1).optional(),
  outputPath: z.string().min(1).optional(),
  overwrite: z.boolean().optional(),
  taskId: z.string().min(1).optional(),
  decisions: z.array(z.unknown()).optional(),
  manifest: z.record(z.string(), z.unknown()).optional(),
});

const taskEnvelopeOutputShape = {
  taskId: z.string().min(1),
  kind: z.enum(TASK_KINDS),
  store: z.enum(["studio-session", "canonical-project"]),
  phase: z.string().min(1),
  source: z.unknown(),
  planDigest: z.string().nullable(),
  approvedPlanDigest: z.string().nullable(),
  evidenceDigest: z.string().nullable(),
  receiptDigest: z.string().nullable(),
  revisionEffect: z.string().min(1),
  availableActions: z.array(
    z.enum(["inspect", "stage", "approve", "execute", "cancel", "decide"]),
  ),
  staleReasons: z.array(z.string()),
  details: z.unknown(),
};

function asRecord(value: unknown): UnknownRecord | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as UnknownRecord)
    : undefined;
}

function jsonValue(value: unknown): JsonValue {
  if (value === undefined) return null;
  const serialized = JSON.stringify(value);
  if (serialized === undefined) return null;
  return JSON.parse(serialized) as JsonValue;
}

const INTERNAL_DETAIL_KEYS = new Set([
  "childTaskId",
  "handle",
  "nativeId",
  "operationId",
  "outputTask",
  "patchId",
  "taskId",
]);

function isInternalDetailKey(key: string): boolean {
  return (
    INTERNAL_DETAIL_KEYS.has(key) ||
    /(?:directory|file|path|root|token)$/iu.test(key)
  );
}

function isPublicTaskId(value: JsonValue): boolean {
  if (typeof value !== "string") return false;
  try {
    parseTaskId(value);
    return true;
  } catch {
    return false;
  }
}

/**
 * Workflow results can contain host paths and native routing handles. Keep
 * those inside the owning adapter; the model-facing envelope exposes only the
 * facade taskId and portable lifecycle evidence.
 */
function modelFacingDetails(value: unknown): JsonValue {
  const redact = (candidate: JsonValue): JsonValue => {
    if (Array.isArray(candidate)) return candidate.map(redact);
    if (candidate === null || typeof candidate !== "object") return candidate;

    const result: Record<string, JsonValue> = {};
    for (const [key, nested] of Object.entries(candidate)) {
      const publicTaskId = key === "taskId" && isPublicTaskId(nested);
      if (!isInternalDetailKey(key) || publicTaskId) result[key] = redact(nested);
    }
    return result;
  };

  return redact(jsonValue(value));
}

/**
 * Project initialization journals intentionally contain host-local execution
 * state. Present only the portable review contract, regardless of whether the
 * workflow has already projected the CLI record or returned its durable
 * payload shape directly.
 */
function projectInitializationPresentation(value: unknown): JsonValue {
  const root = asRecord(value) ?? {};
  const payload = asRecord(root.payload) ?? root;
  const plan = asRecord(payload.plan) ?? asRecord(root.plan) ?? payload;
  const source =
    asRecord(root.source) ?? asRecord(payload.source) ?? asRecord(plan.source) ?? {};
  const destination =
    asRecord(root.destination) ??
    asRecord(payload.destination) ??
    asRecord(plan.destination);
  const result = asRecord(root.result) ?? asRecord(payload.result);

  const presented: Record<string, JsonValue> = {};
  const status = firstNestedString(value, new Set(["status"]));
  const planDigest = planDigestFrom(value);
  const approvedPlanDigest = approvedDigestFrom(value);
  const mode =
    typeof root.mode === "string"
      ? root.mode
      : typeof payload.mode === "string"
        ? payload.mode
        : typeof plan.mode === "string"
          ? plan.mode
          : undefined;
  if (status) presented.status = status;
  if (planDigest) presented.planDigest = planDigest;
  if (approvedPlanDigest) presented.approvedPlanDigest = approvedPlanDigest;
  if (mode === "adopt_active" || mode === "save_untitled") presented.mode = mode;

  const presentedSource: Record<string, JsonValue> = {};
  for (const key of ["projectId", "sceneId", "fingerprint"] as const) {
    if (typeof source[key] === "string" && source[key].length > 0) {
      presentedSource[key] = source[key];
    }
  }
  if (typeof source.projectPathPresent === "boolean") {
    presentedSource.projectPathPresent = source.projectPathPresent;
  } else if (Object.hasOwn(source, "projectPathDigest")) {
    presentedSource.projectPathPresent =
      typeof source.projectPathDigest === "string" &&
      source.projectPathDigest.length > 0;
  }
  presented.source = presentedSource;

  if (destination) {
    const presentedDestination: Record<string, JsonValue> = {};
    if (typeof destination.fileName === "string" && destination.fileName.length > 0) {
      presentedDestination.fileName = destination.fileName;
    }
    if (typeof destination.pathDigest === "string" && destination.pathDigest.length > 0) {
      presentedDestination.pathDigest = destination.pathDigest;
    }
    if (Object.keys(presentedDestination).length > 0) {
      presented.destination = presentedDestination;
    }
  }

  const resultProjectId =
    typeof result?.projectId === "string"
      ? result.projectId
      : typeof payload.resultProjectId === "string"
        ? payload.resultProjectId
        : undefined;
  const resultSceneId =
    typeof result?.sceneId === "string"
      ? result.sceneId
      : typeof payload.resultSceneId === "string"
        ? payload.resultSceneId
        : undefined;
  const canonicalRevision =
    typeof result?.canonicalRevision === "number"
      ? result.canonicalRevision
      : typeof payload.canonicalRevision === "number"
        ? payload.canonicalRevision
        : undefined;
  const outcome =
    typeof result?.outcome === "string"
      ? result.outcome
      : status === "initialized" || status === "already_initialized"
        ? status
        : undefined;
  const executionReplayed = root.executionReplayed === true;
  const failureKind =
    typeof root.failureKind === "string" ? root.failureKind : undefined;
  if (
    resultProjectId !== undefined ||
    resultSceneId !== undefined ||
    canonicalRevision !== undefined ||
    outcome !== undefined
  ) {
    presented.result = {
      ...(resultProjectId ? { projectId: resultProjectId } : {}),
      ...(resultSceneId ? { sceneId: resultSceneId } : {}),
      ...(canonicalRevision !== undefined ? { canonicalRevision } : {}),
      ...(outcome ? { outcome } : {}),
      ...(executionReplayed ? { executionReplayed: true } : {}),
    };
  }
  if (failureKind) presented.failureKind = failureKind;

  if (
    Array.isArray(root.warnings) &&
    root.warnings.every((warning) => typeof warning === "string")
  ) {
    presented.warnings = root.warnings as string[];
  }
  return presented;
}

function firstNestedString(
  value: unknown,
  keys: ReadonlySet<string>,
  depth = 0,
): string | undefined {
  if (depth > 7) return undefined;
  const record = asRecord(value);
  if (!record) return undefined;
  for (const [key, candidate] of Object.entries(record)) {
    if (keys.has(key) && typeof candidate === "string" && candidate.length > 0) {
      return candidate;
    }
  }
  for (const candidate of Object.values(record)) {
    if (Array.isArray(candidate)) {
      for (const item of candidate) {
        const found = firstNestedString(item, keys, depth + 1);
        if (found) return found;
      }
    } else {
      const found = firstNestedString(candidate, keys, depth + 1);
      if (found) return found;
    }
  }
  return undefined;
}

function firstNestedNumber(
  value: unknown,
  keys: ReadonlySet<string>,
  depth = 0,
): number | undefined {
  if (depth > 7) return undefined;
  const record = asRecord(value);
  if (!record) return undefined;
  for (const [key, candidate] of Object.entries(record)) {
    if (keys.has(key) && typeof candidate === "number" && Number.isSafeInteger(candidate)) {
      return candidate;
    }
  }
  for (const candidate of Object.values(record)) {
    if (Array.isArray(candidate)) {
      for (const item of candidate) {
        const found = firstNestedNumber(item, keys, depth + 1);
        if (found !== undefined) return found;
      }
    } else {
      const found = firstNestedNumber(candidate, keys, depth + 1);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}

function firstNestedBoolean(
  value: unknown,
  keys: ReadonlySet<string>,
  depth = 0,
): boolean | undefined {
  if (depth > 7) return undefined;
  const record = asRecord(value);
  if (!record) return undefined;
  for (const [key, candidate] of Object.entries(record)) {
    if (keys.has(key) && typeof candidate === "boolean") return candidate;
  }
  for (const candidate of Object.values(record)) {
    if (Array.isArray(candidate)) {
      for (const item of candidate) {
        const found = firstNestedBoolean(item, keys, depth + 1);
        if (found !== undefined) return found;
      }
    } else {
      const found = firstNestedBoolean(candidate, keys, depth + 1);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}

function firstNestedArray(
  value: unknown,
  key: string,
  depth = 0,
): unknown[] | undefined {
  if (depth > 7) return undefined;
  const record = asRecord(value);
  if (!record) return undefined;
  if (Array.isArray(record[key])) return record[key] as unknown[];
  for (const candidate of Object.values(record)) {
    if (Array.isArray(candidate)) {
      for (const item of candidate) {
        const found = firstNestedArray(item, key, depth + 1);
        if (found) return found;
      }
    } else {
      const found = firstNestedArray(candidate, key, depth + 1);
      if (found) return found;
    }
  }
  return undefined;
}

function normalizeDigest(value: string): string {
  const normalized = value.replace(/^sha256:/i, "").toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(normalized)) {
    throw new Error("Expected a SHA-256 digest");
  }
  return normalized;
}

function planDigestFrom(result: unknown): string | undefined {
  const digest = firstNestedString(
    result,
    new Set(["digest", "planDigest", "approvalDigest"]),
  );
  return digest ? normalizeDigest(digest) : undefined;
}

function evidenceDigests(result: unknown): {
  evidenceDigest?: string;
  receiptDigest?: string;
} {
  const evidence = firstNestedString(result, new Set(["evidenceDigest"]));
  const receipt = firstNestedString(result, new Set(["receiptDigest"]));
  return {
    evidenceDigest: evidence ? normalizeDigest(evidence) : undefined,
    receiptDigest: receipt ? normalizeDigest(receipt) : undefined,
  };
}

function approvedDigestFrom(result: unknown): string | undefined {
  const digest = firstNestedString(
    result,
    new Set(["approvedPlanDigest", "approvedDigest"]),
  );
  return digest ? normalizeDigest(digest) : undefined;
}

function staleReasonsFromResult(result: unknown): string[] {
  const visit = (value: unknown, depth = 0): string[] | undefined => {
    if (depth > 7) return undefined;
    const record = asRecord(value);
    if (!record) return undefined;
    const reasons = record.staleReasons;
    if (
      Array.isArray(reasons) &&
      reasons.every((reason) => typeof reason === "string")
    ) {
      return reasons.filter((reason) => reason.length > 0) as string[];
    }
    for (const candidate of Object.values(record)) {
      if (Array.isArray(candidate)) {
        for (const item of candidate) {
          const found = visit(item, depth + 1);
          if (found) return found;
        }
      } else {
        const found = visit(candidate, depth + 1);
        if (found) return found;
      }
    }
    return undefined;
  };
  const explicit = visit(result);
  if (explicit) return explicit;
  const status = firstNestedString(result, new Set(["status"]))?.toLowerCase();
  return status === "stale"
    ? ["The task's bound source state is stale; inspect details and re-stage."]
    : [];
}

function sourceFromResult(
  result: unknown,
  store: "studio-session" | "canonical-project",
): JsonValue {
  const source: Record<string, JsonValue> = { store };
  const projectId = firstNestedString(result, new Set(["projectId"]));
  const sceneId = firstNestedString(result, new Set(["sceneId"]));
  const fingerprint = firstNestedString(
    result,
    new Set(["sourceFingerprint", "fingerprint"]),
  );
  const revision = firstNestedNumber(
    result,
    new Set(["baseRevision", "sourceRevision", "revision"]),
  );
  if (projectId) source.projectId = projectId;
  if (sceneId) source.sceneId = sceneId;
  if (fingerprint) source.fingerprint = fingerprint;
  if (revision !== undefined) source.revision = revision;
  const projectPathPresent = firstNestedBoolean(
    result,
    new Set(["projectPathPresent"]),
  );
  if (projectPathPresent !== undefined) {
    source.projectPathPresent = projectPathPresent;
  }
  return source;
}

function requireNativeId(result: unknown, keys: string[]): string {
  const value = firstNestedString(result, new Set(keys));
  if (!value) {
    throw new Error(`Workflow result did not expose ${keys.join(" or ")}`);
  }
  return value;
}

function exactKnownDigest(
  registry: TaskFacadeRegistry,
  taskId: string,
  supplied: string,
  field: "planDigest" | "evidenceDigest" = "planDigest",
): string {
  const normalized = normalizeDigest(supplied);
  const expected = registry.get(taskId)?.[field];
  if (expected && normalizeDigest(expected) !== normalized) {
    throw new Error(
      `${field} mismatch for ${taskId}: the exact bound digest is required`,
    );
  }
  return normalized;
}

function requireInputDigest(value: string | undefined, label: string): string {
  if (!value) throw new Error(`${label} is required for this task action`);
  return normalizeDigest(value);
}

function phaseFromResult(result: unknown, fallback: string): string {
  return firstNestedString(result, new Set(["status", "patchStatus"])) ?? fallback;
}

function timelineEditPhaseFromResult(result: unknown, fallback: string): string {
  return (
    firstNestedString(result, new Set(["receiptStatus"])) ??
    phaseFromResult(result, fallback)
  );
}

function canonicalMutationRevisionEffect(
  result: unknown,
  unreported = "canonical-project-revision-advanced",
): string {
  const replay = firstNestedBoolean(result, new Set(["canonicalReplay"]));
  if (replay === true) return "canonical-project-replay-no-revision-change";
  if (replay === false) return "canonical-project-revision-advanced";
  return unreported;
}

function projectInitializationRevisionEffect(result: unknown): string {
  if (firstNestedBoolean(result, new Set(["executionReplayed"])) === true) {
    return "canonical-project-replay-no-revision-change";
  }
  const status = firstNestedString(result, new Set(["status", "outcome"]))?.toLowerCase();
  return status === "initialized" ? "canonical-project-initialized" : "none";
}

function taskText(task: TaskEnvelope, lead: string): string {
  const lines = [
    lead,
    `taskId=${task.taskId}`,
    `kind=${task.kind}`,
    `store=${task.store}`,
    `phase=${task.phase}`,
    `revisionEffect=${task.revisionEffect}`,
    `planDigest=${task.planDigest ?? "(none)"}`,
    `approvedPlanDigest=${task.approvedPlanDigest ?? "(none)"}`,
    `evidenceDigest=${task.evidenceDigest ?? "(none)"}`,
    `receiptDigest=${task.receiptDigest ?? "(none)"}`,
    `availableActions=${task.availableActions.join(",") || "(none)"}`,
  ];
  if (task.staleReasons.length > 0) {
    lines.push(`staleReasons=${task.staleReasons.join(" | ")}`);
  }
  return lines.join("\n");
}

function taskResult(task: TaskEnvelope, lead: string): CallToolResult {
  return {
    content: [{ type: "text", text: taskText(task, lead) }],
    structuredContent: task as unknown as Record<string, unknown>,
  };
}

async function sceneTaskResult(
  options: RegisterFacadeToolsOptions,
  task: TaskEnvelope,
  lead: string,
  result: unknown,
  includeImages: boolean,
): Promise<CallToolResult> {
  let presented: CallToolResult;
  try {
    presented = await options.sceneResult(
      result,
      taskText(task, lead),
      includeImages,
    );
  } catch (error) {
    if (!includeImages) throw error;
    const warning = `Image attachments were omitted after the lifecycle transition: ${formatAgentError(error)}`;
    try {
      presented = await options.sceneResult(
        result,
        `${taskText(task, lead)}\nwarning=${warning}`,
        false,
      );
    } catch {
      presented = {
        content: [
          {
            type: "text",
            text: `${taskText(task, lead)}\nwarning=${warning}`,
          },
        ],
      };
    }
  }
  return {
    ...presented,
    structuredContent: task as unknown as Record<string, unknown>,
  };
}

function errorResult(error: unknown): CallToolResult {
  return {
    isError: true,
    content: [{ type: "text", text: formatAgentError(error) }],
  };
}

async function rawCanonicalDescription(
  ymm4: Ymm4Workflow,
): Promise<CanonicalDescribeInput> {
  const described = (await ymm4.describe()) as CanonicalDescribeInput;
  try {
    const head = await ymm4.canonicalHead();
    return { ...described, head };
  } catch (error) {
    return { ...described, headError: formatAgentError(error) };
  }
}

function partialComposition(canonical: UnknownRecord): UnknownRecord {
  const snapshot = asRecord(canonical.snapshot);
  const managedItems = Array.isArray(snapshot?.managedItems)
    ? snapshot.managedItems
    : [];
  const nativeExtensions = Array.isArray(snapshot?.nativeExtensions)
    ? snapshot.nativeExtensions
    : [];
  const managedElements = managedItems.map((item, index) => {
    const record = asRecord(item) ?? {};
    const realizationId =
      typeof record.realizationId === "string" ? record.realizationId : undefined;
    const entityId = typeof record.entityId === "string" ? record.entityId : undefined;
    const kind = typeof record.kind === "string" ? record.kind : "managed";
    const layer = typeof record.layer === "number" ? record.layer : "unknown";
    const frame = typeof record.frame === "number" ? record.frame : "unknown";
    const element: UnknownRecord = {
      elementId:
        realizationId ||
        (entityId && `${entityId}:${kind}:${layer}:${frame}`) ||
        `managed-${index}`,
      stability: realizationId ? "realization_identity" : "managed_composite",
      observation: "timeline_placement",
    };
    for (const key of [
      "entityId",
      "realizationId",
      "revision",
      "kind",
      "type",
      "frame",
      "length",
      "layer",
      "text",
      "caption",
      "speaker",
      "characterName",
    ]) {
      const value = record[key];
      if (
        typeof value === "string" ||
        typeof value === "number" ||
        typeof value === "boolean" ||
        value === null
      ) {
        element[key] = value;
      }
    }
    return element;
  });
  const nativeElements: UnknownRecord[] = nativeExtensions.map((item, index) => {
    const record = asRecord(item) ?? {};
    const realizationId =
      typeof record.realizationId === "string" ? record.realizationId : undefined;
    const entityId = typeof record.entityId === "string" ? record.entityId : undefined;
    const kind = typeof record.kind === "string" ? record.kind : "native_extension";
    return {
      elementId:
        realizationId ||
        (entityId && `${entityId}:${kind}`) ||
        `native-extension-${index}`,
      stability: realizationId ? "realization_identity" : "managed_composite",
      observation: "managed_owned_fields_only",
      ...(typeof record.logicalKey === "string"
        ? { logicalKey: record.logicalKey }
        : {}),
      ...(realizationId ? { realizationId } : {}),
      ...(entityId ? { entityId } : {}),
      ...(typeof record.entityRevision === "number"
        ? { entityRevision: record.entityRevision }
        : {}),
      kind,
      ...(asRecord(record.ownedFields)
        ? { unevaluatedOwnedFields: record.ownedFields }
        : {}),
    };
  });
  const elements = [...managedElements, ...nativeElements].sort((left, right) => {
    const leftFrame = typeof left.frame === "number" ? left.frame : Number.MAX_SAFE_INTEGER;
    const rightFrame = typeof right.frame === "number" ? right.frame : Number.MAX_SAFE_INTEGER;
    if (leftFrame !== rightFrame) return leftFrame - rightFrame;
    const leftLayer = typeof left.layer === "number" ? left.layer : Number.MAX_SAFE_INTEGER;
    const rightLayer = typeof right.layer === "number" ? right.layer : Number.MAX_SAFE_INTEGER;
    if (leftLayer !== rightLayer) return leftLayer - rightLayer;
    const leftId = String(left.elementId);
    const rightId = String(right.elementId);
    return leftId < rightId ? -1 : leftId > rightId ? 1 : 0;
  });
  const unmanagedContextCount =
    typeof snapshot?.unmanagedContextCount === "number"
      ? snapshot.unmanagedContextCount
      : null;
  return {
    schemaVersion: "takegraph.composition-observation.v0",
    authority: "derived-read-only-observation",
    availability: "timeline_only",
    source: {
      projectId: snapshot?.projectId ?? null,
      sceneId: snapshot?.sceneId ?? null,
      fingerprint: snapshot?.fingerprint ?? null,
      fps: snapshot?.fps ?? null,
      canonicalRevision: asRecord(canonical.head)?.revision ?? null,
    },
    evaluatedFrame: null,
    viewport: { availability: "unavailable" },
    elements,
    completeness: {
      managedTimelineItems: "complete_for_bridge_snapshot",
      managedNativeExtensions: "owned_fields_only",
      unmanagedElements:
        unmanagedContextCount === 0 ? "none_reported" : "count_only",
      unmanagedContextCount,
    },
    unavailableFields: [
      "evaluatedFrame",
      "viewport",
      "visibility",
      "paintOrder",
      "bounds",
      "anchor",
      "position",
      "scale",
      "rotation",
      "opacity",
      "crop",
      "parentAndMaskRelations",
      "unmanagedElementDetails",
    ],
    note:
      "The current bridge exposes timeline placement but not evaluated visual geometry. Missing fields are not inferred.",
  };
}

interface SceneDescription {
  canonical: UnknownRecord;
  rawCanonical: CanonicalDescribeInput;
  composition: UnknownRecord;
  currentFrameObserved: boolean;
}

function compositionBindingMismatches(
  canonical: UnknownRecord,
  observed: UnknownRecord,
): string[] {
  const snapshot = asRecord(canonical.snapshot);
  const head = asRecord(canonical.head);
  const checks: Array<[string, unknown, unknown]> = [
    ["projectId", observed.projectId, snapshot?.projectId],
    ["canonicalHead.projectId", observed.projectId, head?.projectId],
    ["sceneId", observed.sceneId, snapshot?.sceneId],
    ["sourceFingerprint", observed.sourceFingerprint, snapshot?.fingerprint],
    ["fps", observed.fps, snapshot?.fps],
  ];
  return checks
    .filter(([, actual, expected]) => actual !== expected)
    .map(([field]) => field);
}

function boundCurrentFrameComposition(
  canonical: UnknownRecord,
  observed: UnknownRecord,
): UnknownRecord {
  const completeness =
    observed.completeness === "complete" ? "complete" : "partial";
  return {
    ...observed,
    authority: "target-derived-read-only-observation",
    availability: `current_frame_${completeness}`,
    source: {
      projectId: observed.projectId,
      sceneId: observed.sceneId,
      fingerprint: observed.sourceFingerprint,
      fps: observed.fps,
      canonicalRevision: asRecord(canonical.head)?.revision ?? null,
    },
    evaluatedFrame: observed.frame,
    observationStatus: "source_bound",
    note:
      "This current-frame observation was matched to the canonical inventory by project, scene, fingerprint, and fps. Unavailable visual fields were not inferred.",
  };
}

async function sceneDescription(ymm4: Ymm4Workflow): Promise<SceneDescription> {
  const compositionMethod = (
    ymm4 as unknown as { composition?: () => Promise<unknown> }
  ).composition;
  let observed: UnknownRecord | undefined;
  let observationError: string | undefined;
  if (typeof compositionMethod === "function") {
    try {
      observed = asRecord(await compositionMethod.call(ymm4)) ?? undefined;
      if (!observed) {
        observationError = "The bridge returned a non-object composition response.";
      }
    } catch (error) {
      observationError = formatAgentError(error);
    }
  } else {
    observationError = "The connected workflow does not expose current-frame composition.";
  }

  // Read the ordinary snapshot after the current-frame observation. Matching
  // these independently acquired source fields prevents a stale composition
  // from being combined with a newer canonical inventory.
  const rawCanonical = await rawCanonicalDescription(ymm4);
  const canonical =
    (asRecord(modelFacingDetails(rawCanonical)) as UnknownRecord | undefined) ?? {};
  const bindingSource = (asRecord(rawCanonical) as UnknownRecord | undefined) ?? canonical;
  if (observed) {
    const mismatches = compositionBindingMismatches(bindingSource, observed);
    if (mismatches.length === 0) {
      return {
        canonical,
        rawCanonical,
        composition: boundCurrentFrameComposition(bindingSource, observed),
        currentFrameObserved: true,
      };
    }
    observationError = `Current-frame composition did not match the canonical ${mismatches.join(
      ", ",
    )} binding.`;
  }

  return {
    canonical,
    rawCanonical,
    composition: {
      ...partialComposition(canonical),
      observationStatus: "current_frame_unavailable",
      observationError,
    },
    currentFrameObserved: false,
  };
}

function detachReceiptStatus(result: unknown): string | undefined {
  const visit = (candidate: unknown, depth = 0): string | undefined => {
    if (depth > 7) return undefined;
    const record = asRecord(candidate);
    if (!record) return undefined;
    const receipt = asRecord(record.receipt);
    if (typeof receipt?.status === "string") return receipt.status.toLowerCase();
    for (const nested of Object.values(record)) {
      if (Array.isArray(nested)) {
        for (const item of nested) {
          const found = visit(item, depth + 1);
          if (found) return found;
        }
      } else {
        const found = visit(nested, depth + 1);
        if (found) return found;
      }
    }
    return undefined;
  };
  return visit(result);
}

function actionsFor(kind: TaskKind, phase: string, result?: unknown): TaskAction[] {
  const normalized = phase.toLowerCase();
  if (kind === "project_initialization") {
    if (normalized === "staged") return ["inspect", "approve"];
    if (normalized === "approved") return ["inspect", "execute"];
    if (normalized === "executing") return ["inspect", "execute"];
    if (normalized === "recovery_required") return ["inspect", "execute"];
    return ["inspect"];
  }
  if (kind === "reconciliation_detach") {
    if (["verified", "committed", "completed"].includes(normalized)) {
      return ["inspect"];
    }
    if (normalized === "preview_ready") return ["inspect", "approve"];
    if (normalized === "approved") return ["inspect", "execute"];
    if (
      (normalized === "failed" && detachReceiptStatus(result) === "not_started") ||
      (normalized === "rolled_back" && detachReceiptStatus(result) === "rolled_back")
    ) {
      return ["inspect", "execute"];
    }
    return ["inspect"];
  }
  if (kind === "timeline_edit") {
    if (["staged", "previewable", "approved"].includes(normalized)) {
      return ["inspect", "execute"];
    }
    return ["inspect"];
  }
  if (
    [
      "accepted",
      "actions_accepted",
      "actions_materialized",
      "committed",
      "rejected",
      "completed",
      "verified",
      "cancelled",
      "canceled",
      "failed",
      "stale",
      "recovery_required",
    ].includes(normalized)
  ) {
    return ["inspect"];
  }
  if (kind === "scene_inspection") {
    if (normalized === "staged") return ["inspect", "approve"];
    if (normalized === "approved") return ["inspect", "execute"];
    if (normalized === "captured") return ["inspect", "execute"];
    if (normalized === "reviewed") return ["inspect", "decide"];
    return ["inspect"];
  }
  if (kind === "native_extension") {
    if (normalized === "approved") return ["inspect", "execute"];
    if (["staged", "previewable", "preview_ready"].includes(normalized)) {
      return ["inspect", "approve"];
    }
    return ["inspect"];
  }
  if (kind === "render") return ["inspect", "execute", "cancel"];
  if (kind === "reconciliation" && normalized === "report_ready") {
    return ["inspect", "stage"];
  }
  if (kind === "reconciliation_re_export") {
    return ["awaiting_manifest", "preview_ready"].includes(normalized)
      ? ["inspect", "stage"]
      : ["inspect"];
  }
  if (kind === "reconciliation_import") {
    return ["inspect"];
  }
  if (kind === "studio_voice_variant") {
    return ["inspect"];
  }
  return ["inspect", "execute"];
}

function rememberCanonicalTask(
  registry: TaskFacadeRegistry,
  input: {
    kind: TaskKind;
    nativeId: string;
    phase: string;
    result: unknown;
    planDigest?: string;
    approvedPlanDigest?: string;
    revisionEffect?: string;
    availableActions?: TaskAction[];
    source?: JsonValue;
    inferPlanDigest?: boolean;
    clearApprovedPlanDigest?: boolean;
  },
): TaskEnvelope {
  const evidence = evidenceDigests(input.result);
  const existing = registry.get(makeTaskId(input.kind, input.nativeId));
  const observedSource = sourceFromResult(input.result, "canonical-project");
  const existingSource = asRecord(existing?.source);
  const nextSource = asRecord(input.source ?? observedSource);
  const planDigest =
    input.planDigest !== undefined
      ? input.planDigest
      : existing?.planDigest !== null && existing?.planDigest !== undefined
        ? existing.planDigest
        : input.inferPlanDigest === false
          ? undefined
          : planDigestFrom(input.result);
  const requestedActions =
    input.availableActions ?? actionsFor(input.kind, input.phase, input.result);
  const availableActions = requestedActions.filter(
    (action) =>
      action !== "approve" || planDigest !== undefined,
  );
  return registry.remember({
    kind: input.kind,
    nativeId: input.nativeId,
    store: "canonical-project",
    phase: input.phase,
    source: { ...(nextSource ?? {}), ...(existingSource ?? {}) },
    planDigest,
    approvedPlanDigest: input.clearApprovedPlanDigest
      ? undefined
      : input.approvedPlanDigest ??
        existing?.approvedPlanDigest ??
        approvedDigestFrom(input.result),
    evidenceDigest: evidence.evidenceDigest ?? existing?.evidenceDigest ?? undefined,
    receiptDigest: evidence.receiptDigest ?? existing?.receiptDigest ?? undefined,
    revisionEffect: input.revisionEffect ?? "none",
    availableActions,
    staleReasons: staleReasonsFromResult(input.result),
    details: modelFacingDetails(input.result),
  });
}

function rememberStudioTask(
  registry: TaskFacadeRegistry,
  input: {
    kind: "studio_take" | "studio_voice_variant";
    nativeId: string;
    phase: string;
    state: ProjectState;
    planDigest?: string;
    approvedPlanDigest?: string;
    revisionEffect: string;
    availableActions?: TaskAction[];
  },
): TaskEnvelope {
  const existing = registry.get(makeTaskId(input.kind, input.nativeId));
  const source =
    existing?.source ??
    ({
      store: "studio-session",
      projectName: input.state.projectName,
      revision: input.state.revision,
    } satisfies JsonValue);
  return registry.remember({
    kind: input.kind,
    nativeId: input.nativeId,
    store: "studio-session",
    phase: input.phase,
    source,
    planDigest: input.planDigest,
    approvedPlanDigest: input.approvedPlanDigest,
    revisionEffect: input.revisionEffect,
    availableActions:
      input.availableActions ?? actionsFor(input.kind, input.phase),
    staleReasons: [],
    details: modelFacingDetails(input.state),
  });
}

function reconciliationChildPhase(kind: TaskKind, result: unknown): string {
  const status = firstNestedString(result, new Set(["status"]));
  if (status) return status;
  if (kind === "reconciliation_import") {
    return firstNestedString(result, new Set(["patchStatus"])) ?? "preview_ready";
  }
  return kind === "reconciliation_re_export" ? "awaiting_manifest" : "preview_ready";
}

function materializeReconciliationChildEnvelopes(
  options: RegisterFacadeToolsOptions,
  result: unknown,
): { tasks: TaskEnvelope[]; warnings: string[] } {
  const references = firstNestedArray(result, "materializedChildren") ?? [];
  const childTasks: TaskEnvelope[] = [];
  const warnings: string[] = [];
  for (const reference of references) {
    try {
      const childTaskId = firstNestedString(reference, new Set(["childTaskId"]));
      if (!childTaskId) throw new Error("childTaskId is missing");
      const nativeId = normalizeDigest(childTaskId);
      const type = firstNestedString(reference, new Set(["kind", "type"]));
      const kind: TaskKind =
        type === "metadata_detach"
          ? "reconciliation_detach"
          : type === "canonical_re_export"
            ? "reconciliation_re_export"
            : type === "import_patch"
              ? "reconciliation_import"
              : (() => {
                  throw new Error(`unknown kind ${type ?? "(missing)"}`);
                })();
      childTasks.push(
        rememberCanonicalTask(options.registry, {
          kind,
          nativeId,
          phase: "materialized",
          result:
            kind === "reconciliation_import"
              ? {
                  reference,
                  blocker:
                    "The core Patch handoff exists, but this service version exposes no façade approve/commit adapter for reconciliation import.",
                }
              : {
                  reference,
                  next:
                    kind === "reconciliation_detach"
                      ? "takegraph_task_execute with intent=revalidate, then approve the recovered exact planDigest"
                      : "takegraph_task_stage with kind=reconciliation_re_export, this taskId, and a route-tagged manifest",
                },
          availableActions:
            kind === "reconciliation_detach"
              ? ["inspect", "execute"]
              : kind === "reconciliation_re_export"
                ? ["inspect", "stage"]
                : ["inspect"],
          source: {
            store: "canonical-project",
            reconciliationChildTaskId: childTaskId,
          },
          inferPlanDigest: false,
        }),
      );
    } catch (error) {
      warnings.push(`A materialized child could not be presented: ${formatAgentError(error)}`);
    }
  }
  return { tasks: childTasks, warnings };
}

async function stageTask(
  options: RegisterFacadeToolsOptions,
  input: z.infer<typeof stageInputSchema>,
): Promise<CallToolResult> {
  switch (input.kind) {
    case "studio_take": {
      const parsed = studioTakeStageSchema.parse(input);
      const state = options.session.stageTake(parsed.takeId);
      const patch = state.stagedPatch;
      if (!patch) throw new Error("Studio session did not return a staged patch");
      const task = rememberStudioTask(options.registry, {
        kind: "studio_take",
        nativeId: patch.id,
        phase: "staged",
        state,
        planDigest: normalizeDigest(patch.digest),
        revisionEffect: "none",
        availableActions: ["inspect", "execute"],
      });
      return taskResult(task, "Studio take patch staged; no revision changed.");
    }
    case "studio_voice_variant": {
      const parsed = studioVariantStageSchema.parse(input);
      const before = new Set(options.session.snapshot().takes.map((take) => take.id));
      const state = options.session.generateVariant(parsed);
      const take = state.takes.find((candidate) => !before.has(candidate.id));
      if (!take) throw new Error("Studio session did not return a new VoiceTake");
      const task = rememberStudioTask(options.registry, {
        kind: "studio_voice_variant",
        nativeId: take.id,
        phase: take.readiness === "ready" ? "ready" : "query_ready",
        state,
        revisionEffect: "none",
        availableActions: ["inspect"],
      });
      return taskResult(task, "A new immutable studio VoiceTake was created.");
    }
    case "timeline_edit": {
      const parsed = timelineEditStageSchema.parse(input);
      const result = await options.ymm4.stageTimelineEdit({
        operations: parsed.operations as TimelineEditOperationInput[],
        maxChangedEntities: parsed.maxChangedEntities,
      } satisfies StageTimelineEditInput);
      const task = rememberCanonicalTask(options.registry, {
        kind: "timeline_edit",
        nativeId: requireNativeId(result, ["handle"]),
        phase: "staged",
        result,
        planDigest: planDigestFrom(result),
        availableActions: ["inspect", "execute"],
      });
      return taskResult(
        task,
        "Ordered timeline edit staged as one digest-bound task; YMM4 is unchanged.",
      );
    }
    case "portable_voice": {
      const parsed = portableVoiceStageSchema.parse(input);
      const { kind: _kind, ...payload } = parsed;
      const result = await options.ymm4.stage(payload as StageYmm4Request);
      const task = rememberCanonicalTask(options.registry, {
        kind: "portable_voice",
        nativeId: requireNativeId(result, ["handle"]),
        phase: "staged",
        result,
        planDigest: planDigestFrom(result),
        availableActions: ["inspect", "execute"],
      });
      return taskResult(task, "Portable voice export staged; YMM4 is unchanged.");
    }
    case "native_voice": {
      const parsed = nativeVoiceStageSchema.parse(input);
      const { kind: _kind, ...payload } = parsed;
      const result = await options.ymm4.stageNativeVoice(
        payload as StageNativeVoiceRequest,
      );
      const task = rememberCanonicalTask(options.registry, {
        kind: "native_voice",
        nativeId: requireNativeId(result, ["handle"]),
        phase: "staged",
        result,
        planDigest: planDigestFrom(result),
        availableActions: ["inspect", "execute"],
      });
      return taskResult(task, "Native voice export staged; YMM4 is unchanged.");
    }
    case "native_voice_mutation": {
      const parsed = nativeVoiceMutationsStageSchema.parse(input);
      const result = await options.ymm4.stageNativeVoiceMutations({
        mutations: parsed.mutations as NativeVoiceMutationInput[],
      } satisfies StageNativeVoiceMutationsInput);
      const task = rememberCanonicalTask(options.registry, {
        kind: "native_voice_mutation",
        nativeId: requireNativeId(result, ["handle"]),
        phase: "staged",
        result,
        planDigest: planDigestFrom(result),
        availableActions: ["inspect", "execute"],
      });
      return taskResult(task, "Native voice mutation staged; YMM4 is unchanged.");
    }
    case "native_extension": {
      const parsed = nativeExtensionStageSchema.parse(input);
      const result = await options.ymm4.stageNativeExtension({
        operations: parsed.operations as NativeExtensionOperationInput[],
        maxChangedEntities: parsed.maxChangedEntities,
      } satisfies StageNativeExtensionInput);
      const task = rememberCanonicalTask(options.registry, {
        kind: "native_extension",
        nativeId: requireNativeId(result, ["handle"]),
        phase: "staged",
        result,
        planDigest: planDigestFrom(result),
        availableActions: ["inspect", "approve"],
      });
      return taskResult(task, "Native extension plan staged; YMM4 is unchanged.");
    }
    case "project_initialization": {
      const parsed = projectInitializationStageSchema.parse(input);
      const result = await options.ymm4.stageProjectInitialization(
        parsed.mode === "save_untitled"
          ? { mode: parsed.mode, path: parsed.path }
          : { mode: parsed.mode },
      );
      const planDigest = planDigestFrom(result);
      if (!planDigest) {
        throw new Error(
          "Project initialization staging did not expose its exact planDigest",
        );
      }
      const phase = phaseFromResult(result, "staged");
      const presentation = projectInitializationPresentation(result);
      const task = rememberCanonicalTask(options.registry, {
        kind: "project_initialization",
        nativeId: requireNativeId(result, ["taskId", "operationId"]),
        phase,
        result: presentation,
        planDigest,
        availableActions: actionsFor("project_initialization", phase, result),
      });
      return taskResult(
        task,
        "Project initialization staged; no Save As or canonical initialization occurred.",
      );
    }
    case "scene_inspection": {
      const parsed = sceneStageSchema.parse(input);
      const { kind: _kind, ...payload } = parsed;
      const result = await options.ymm4.stageSceneInspection(
        payload as StageSceneInspectionInput,
      );
      const task = rememberCanonicalTask(options.registry, {
        kind: "scene_inspection",
        nativeId: requireNativeId(result, ["handle"]),
        phase: "staged",
        result,
        planDigest: planDigestFrom(result),
        availableActions: ["inspect", "approve"],
      });
      return sceneTaskResult(
        options,
        task,
        "Scene inspection staged; no capture occurred.",
        result,
        false,
      );
    }
    case "checkpoint": {
      const result = await options.ymm4.stageCheckpoint();
      const task = rememberCanonicalTask(options.registry, {
        kind: "checkpoint",
        nativeId: requireNativeId(result, ["operationId"]),
        phase: "staged",
        result,
        availableActions: ["inspect", "execute"],
      });
      return taskResult(task, "Verified checkpoint staged; no save occurred.");
    }
    case "render": {
      const parsed = renderStageSchema.parse(input);
      const { kind: _kind, ...payload } = parsed;
      const result = await options.ymm4.stageRender(payload as StageRenderInput);
      const task = rememberCanonicalTask(options.registry, {
        kind: "render",
        nativeId: requireNativeId(result, ["taskId"]),
        phase: "staged",
        result,
        availableActions: ["inspect", "execute", "cancel"],
      });
      return taskResult(task, "Render staged; rendering has not started.");
    }
    case "reconciliation": {
      if (input.mode === "preview") {
        const parsed = reconciliationPreviewSchema.parse(input);
        const publicTask = parseTaskId(parsed.taskId);
        if (publicTask.kind !== "reconciliation") {
          throw new Error(
            `Reconciliation preview requires a reconciliation taskId, not ${publicTask.kind}`,
          );
        }
        const reportDigest = normalizeDigest(publicTask.nativeId);
        const result = await options.ymm4.previewReconciliation(
          reportDigest,
          parsed.decisions as ReconciliationDecisionInput[],
        );
        const task = rememberCanonicalTask(options.registry, {
          kind: "reconciliation",
          nativeId: reportDigest,
          phase: "staged",
          result,
          planDigest: planDigestFrom(result),
          availableActions: ["inspect", "execute"],
        });
        return taskResult(task, "Reconciliation decisions previewed; neither store changed.");
      }
      const result = await options.ymm4.reconciliationReport();
      const reportDigest = normalizeDigest(
        requireNativeId(result, ["reportDigest"]),
      );
      const task = rememberCanonicalTask(options.registry, {
        kind: "reconciliation",
        nativeId: reportDigest,
        phase: "report_ready",
        result,
        availableActions: ["inspect", "stage"],
      });
      return taskResult(task, "Semantic drift observation is ready for explicit decisions.");
    }
    case "reconciliation_re_export": {
      const parsed = reExportStageSchema.parse(input);
      const publicTask = parseTaskId(parsed.taskId);
      if (publicTask.kind !== "reconciliation_re_export") {
        throw new Error(
          `Re-export dispatch requires a reconciliation_re_export taskId, not ${publicTask.kind}`,
        );
      }
      const childTaskId = normalizeDigest(publicTask.nativeId);
      const result = await options.ymm4.dispatchReconciliationReExport(
        childTaskId,
        parsed.manifest,
      );
      rememberCanonicalTask(options.registry, {
        kind: "reconciliation_re_export",
        nativeId: childTaskId,
        phase: "handed_off",
        result,
        availableActions: ["inspect"],
        inferPlanDigest: false,
      });
      const route = firstNestedString(result, new Set(["downstreamRoute", "route"]));
      const downstreamKind: TaskKind =
        route === "portable_pair"
          ? "portable_voice"
          : route === "native_voice_mutation"
            ? "native_voice_mutation"
            : route === "native_extension"
              ? "native_extension"
              : "reconciliation_re_export";
      const phase = downstreamKind === "reconciliation_re_export" ? "handed_off" : "staged";
      const task = rememberCanonicalTask(options.registry, {
        kind: downstreamKind,
        nativeId: requireNativeId(result, ["handle"]),
        phase,
        result,
        planDigest: planDigestFrom(result),
        availableActions:
          downstreamKind === "native_extension"
            ? ["inspect", "approve"]
            : downstreamKind === "reconciliation_re_export"
              ? ["inspect"]
              : ["inspect", "execute"],
        source: {
          store: "canonical-project",
          reconciliationChildTaskId: childTaskId,
        },
      });
      return taskResult(task, "Canonical re-export was handed to its existing guarded workflow.");
    }
  }
}

async function approveTask(
  options: RegisterFacadeToolsOptions,
  taskId: string,
  suppliedDigest: string,
): Promise<CallToolResult> {
  const parsed = parseTaskId(taskId);
  const digest = exactKnownDigest(options.registry, taskId, suppliedDigest);
  let result: unknown;
  switch (parsed.kind) {
    case "project_initialization":
      result = await options.ymm4.approveProjectInitialization(
        parsed.nativeId,
        digest,
      );
      break;
    case "native_extension":
      result = await options.ymm4.approveNativeExtension(parsed.nativeId, digest);
      break;
    case "scene_inspection":
      result = await options.ymm4.approveSceneInspection(parsed.nativeId, digest);
      break;
    case "reconciliation_detach":
      result = await options.ymm4.approveReconciliationDetach(
        parsed.nativeId,
        digest,
      );
      break;
    case "reconciliation_import":
      throw new Error(
        "reconciliation_import uses the core Patch lifecycle, but its approve/commit adapter is not exposed by the current service; no mutation was performed",
      );
    default:
      throw new Error(
        `${parsed.kind} has no separate approve transition; follow availableActions and pass the exact planDigest to execute when required`,
      );
  }
  const presentation =
    parsed.kind === "project_initialization"
      ? projectInitializationPresentation(result)
      : result;
  const task = rememberCanonicalTask(options.registry, {
    kind: parsed.kind,
    nativeId: parsed.nativeId,
    phase: "approved",
    result: presentation,
    planDigest: digest,
    approvedPlanDigest: digest,
    availableActions: ["inspect", "execute"],
  });
  if (parsed.kind === "scene_inspection") {
    return sceneTaskResult(
      options,
      task,
      "Scene inspection approved for the exact plan digest.",
      result,
      false,
    );
  }
  return taskResult(task, `${parsed.kind} approved for the exact plan digest.`);
}

async function executeTask(
  options: RegisterFacadeToolsOptions,
  input: {
    taskId: string;
    intent: "run" | "review" | "revalidate" | "cancel" | "collect_artifacts";
    planDigest?: string;
    reviewer?: string;
  },
): Promise<CallToolResult> {
  const parsed = parseTaskId(input.taskId);
  const known = options.registry.get(input.taskId);
  let result: unknown;
  let phase = "completed";
  let revisionEffect = "none";
  let approvedPlanDigest = known?.approvedPlanDigest ?? undefined;
  let refreshedPlanDigest: string | undefined;
  let clearApprovedPlanDigest = false;
  let reconciliationChildren: TaskEnvelope[] = [];
  let reconciliationWarnings: string[] = [];

  switch (parsed.kind) {
    case "studio_take": {
      if (input.intent !== "run") throw new Error("studio_take supports only run");
      const digest = exactKnownDigest(
        options.registry,
        input.taskId,
        requireInputDigest(input.planDigest, "planDigest"),
      );
      const state = await options.session.commitPatch({
        patchId: parsed.nativeId,
        digest,
      });
      const task = rememberStudioTask(options.registry, {
        kind: "studio_take",
        nativeId: parsed.nativeId,
        phase: "completed",
        state,
        planDigest: digest,
        approvedPlanDigest: digest,
        revisionEffect: "studio-session-revision-advanced",
        availableActions: ["inspect"],
      });
      return taskResult(task, "Studio take patch committed against its exact digest.");
    }
    case "studio_voice_variant":
      throw new Error("studio_voice_variant has no execute transition");
    case "project_initialization": {
      if (input.intent === "revalidate") {
        result = await options.ymm4.projectInitializationStatus(parsed.nativeId);
      } else if (input.intent === "run") {
        result = await options.ymm4.executeProjectInitialization(parsed.nativeId);
      } else {
        throw new Error("project_initialization supports run or revalidate");
      }
      phase = phaseFromResult(result, input.intent === "run" ? "initialized" : "observed");
      revisionEffect = projectInitializationRevisionEffect(result);
      break;
    }
    case "timeline_edit": {
      if (input.intent === "revalidate") {
        result = await options.ymm4.timelineEditStatus(parsed.nativeId);
        phase = timelineEditPhaseFromResult(result, "observed");
        break;
      }
      if (input.intent !== "run") {
        throw new Error("timeline_edit supports run or revalidate");
      }
      const digest = exactKnownDigest(
        options.registry,
        input.taskId,
        requireInputDigest(input.planDigest, "planDigest"),
      );
      result = await options.ymm4.commitTimelineEdit(parsed.nativeId, digest);
      phase = phaseFromResult(result, "completed");
      approvedPlanDigest = digest;
      revisionEffect = canonicalMutationRevisionEffect(result);
      break;
    }
    case "portable_voice": {
      if (input.intent === "revalidate") {
        result = await options.ymm4.verify(parsed.nativeId);
        phase = "verified";
        break;
      }
      if (input.intent !== "run") throw new Error("portable_voice supports run or revalidate");
      const digest = exactKnownDigest(
        options.registry,
        input.taskId,
        requireInputDigest(input.planDigest, "planDigest"),
      );
      result = await options.ymm4.commit(parsed.nativeId, digest);
      approvedPlanDigest = digest;
      revisionEffect = canonicalMutationRevisionEffect(result);
      break;
    }
    case "native_voice": {
      if (input.intent === "revalidate") {
        result = await options.ymm4.verifyNativeVoice(parsed.nativeId);
        phase = "verified";
        break;
      }
      if (input.intent !== "run") throw new Error("native_voice supports run or revalidate");
      const digest = exactKnownDigest(
        options.registry,
        input.taskId,
        requireInputDigest(input.planDigest, "planDigest"),
      );
      result = await options.ymm4.commitNativeVoice(parsed.nativeId, digest);
      approvedPlanDigest = digest;
      revisionEffect = canonicalMutationRevisionEffect(result);
      break;
    }
    case "native_voice_mutation": {
      if (input.intent === "revalidate") {
        result = await options.ymm4.verifyNativeVoiceMutations(parsed.nativeId);
        phase = "verified";
        break;
      }
      if (input.intent === "collect_artifacts") {
        result = await options.ymm4.captureNativeVoiceMutationArtifacts(
          parsed.nativeId,
        );
        phase = "artifacts_collected";
        break;
      }
      if (input.intent !== "run") {
        throw new Error(
          "native_voice_mutation supports run, revalidate, or collect_artifacts",
        );
      }
      const digest = exactKnownDigest(
        options.registry,
        input.taskId,
        requireInputDigest(input.planDigest, "planDigest"),
      );
      result = await options.ymm4.commitNativeVoiceMutations(
        parsed.nativeId,
        digest,
      );
      approvedPlanDigest = digest;
      revisionEffect = canonicalMutationRevisionEffect(result);
      break;
    }
    case "native_extension":
      if (input.intent === "revalidate") {
        result = await options.ymm4.nativeExtensionStatus(parsed.nativeId);
        phase = phaseFromResult(result, known?.phase ?? "observed");
      } else if (input.intent === "run") {
        result = await options.ymm4.applyNativeExtension(parsed.nativeId);
        phase = phaseFromResult(result, "completed");
        revisionEffect = canonicalMutationRevisionEffect(result);
      } else {
        throw new Error("native_extension supports run or revalidate");
      }
      break;
    case "scene_inspection": {
      if (input.intent === "run") {
        result = await options.ymm4.captureSceneInspection(parsed.nativeId);
        phase = "captured";
      } else if (input.intent === "review") {
        if (!input.reviewer?.trim()) {
          throw new Error("reviewer is required for scene review");
        }
        result = await options.ymm4.reviewSceneInspection(
          parsed.nativeId,
          input.reviewer,
        );
        phase = "reviewed";
      } else if (input.intent === "revalidate") {
        result = await options.ymm4.replaySceneInspection(parsed.nativeId);
        phase = phaseFromResult(result, known?.phase ?? "revalidated");
      } else {
        throw new Error("scene_inspection supports run, review, or revalidate");
      }
      const task = rememberCanonicalTask(options.registry, {
        kind: parsed.kind,
        nativeId: parsed.nativeId,
        phase,
        result,
        planDigest: known?.planDigest ?? undefined,
        approvedPlanDigest: known?.approvedPlanDigest ?? undefined,
        availableActions: actionsFor(parsed.kind, phase, result),
        source:
          input.intent === "review"
            ? {
                ...((asRecord(sourceFromResult(result, "canonical-project")) ?? {}) as Record<
                  string,
                  JsonValue
                >),
                reviewer: input.reviewer!,
              }
            : undefined,
      });
      return sceneTaskResult(
        options,
        task,
        `Scene inspection ${phase}; authenticated evidence is attached when available.`,
        result,
        true,
      );
    }
    case "checkpoint":
      if (input.intent === "revalidate") {
        result = await options.ymm4.checkpointStatus(parsed.nativeId);
      } else if (input.intent === "run") {
        result = await options.ymm4.executeCheckpoint(parsed.nativeId);
      } else {
        throw new Error("checkpoint supports run or revalidate");
      }
      phase = phaseFromResult(result, "completed");
      break;
    case "render":
      if (input.intent === "cancel") {
        result = await options.ymm4.cancelRender(parsed.nativeId);
        phase = phaseFromResult(result, "cancelling");
      } else if (input.intent === "run") {
        result = await options.ymm4.executeRender(parsed.nativeId);
        phase = phaseFromResult(result, "running");
      } else if (input.intent === "revalidate") {
        result = await options.ymm4.renderStatus(parsed.nativeId);
        phase = phaseFromResult(result, "observed");
      } else {
        throw new Error("render supports run, cancel, or revalidate");
      }
      break;
    case "reconciliation": {
      if (input.intent !== "run") throw new Error("reconciliation supports only run");
      const digest = exactKnownDigest(
        options.registry,
        input.taskId,
        requireInputDigest(input.planDigest, "planDigest"),
      );
      result = await options.ymm4.applyReconciliation(parsed.nativeId, digest);
      approvedPlanDigest = digest;
      const childPresentation = materializeReconciliationChildEnvelopes(
        options,
        result,
      );
      reconciliationChildren = childPresentation.tasks;
      reconciliationWarnings = childPresentation.warnings;
      phase = phaseFromResult(result, "actions_materialized");
      break;
    }
    case "reconciliation_detach":
      if (input.intent === "revalidate") {
        result = await options.ymm4.reconciliationChildStatus(parsed.nativeId);
        phase = reconciliationChildPhase(parsed.kind, result);
      } else if (input.intent === "run") {
        try {
          result = await options.ymm4.executeReconciliationDetach(parsed.nativeId);
          revisionEffect = canonicalMutationRevisionEffect(
            result,
            "canonical-project-revision-effect-unreported",
          );
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          if (!/replacement attempt|requires new approval|PreviewReady|preview_ready/i.test(message)) {
            throw error;
          }
          result = await options.ymm4.reconciliationChildStatus(parsed.nativeId);
          phase = reconciliationChildPhase(parsed.kind, result);
        }
      } else {
        throw new Error("reconciliation_detach supports run or revalidate");
      }
      if (phase.toLowerCase() === "preview_ready") {
        refreshedPlanDigest = planDigestFrom(result);
        if (!refreshedPlanDigest) {
          throw new Error(
            "Reissued reconciliation detach did not expose its new exact planDigest",
          );
        }
        approvedPlanDigest = undefined;
        clearApprovedPlanDigest = true;
      }
      break;
    case "reconciliation_re_export":
    case "reconciliation_import":
      if (input.intent !== "revalidate") {
        throw new Error(
          parsed.kind === "reconciliation_re_export"
            ? "reconciliation_re_export must be handed to a concrete exporter with takegraph_task_stage"
            : "reconciliation_import is a canonical Patch handoff; this facade has no commit transition for it yet",
        );
      }
      if (parsed.kind === "reconciliation_import") {
        throw new Error(
          "reconciliation_import uses the core Patch lifecycle, but its approve/commit adapter is not exposed by the current service; no mutation was performed",
        );
      }
      result = await options.ymm4.reconciliationChildStatus(parsed.nativeId);
      phase = reconciliationChildPhase(parsed.kind, result);
      break;
  }

  const presentedResult =
    reconciliationChildren.length > 0 || reconciliationWarnings.length > 0
      ? {
          workflowResult: result,
          childTasks: reconciliationChildren.map((child) => ({
            taskId: child.taskId,
            kind: child.kind,
            phase: child.phase,
            planDigest: child.planDigest,
            availableActions: child.availableActions,
          })),
          presentationWarnings: reconciliationWarnings,
        }
      : result;
  const modelResult =
    parsed.kind === "project_initialization"
      ? projectInitializationPresentation(presentedResult)
      : presentedResult;
  const task = rememberCanonicalTask(options.registry, {
    kind: parsed.kind,
    nativeId: parsed.nativeId,
    phase,
    result: modelResult,
    planDigest: refreshedPlanDigest ?? known?.planDigest ?? input.planDigest,
    approvedPlanDigest,
    clearApprovedPlanDigest,
    revisionEffect,
    availableActions: actionsFor(parsed.kind, phase, modelResult),
  });
  const childSummary =
    reconciliationChildren.length === 0
      ? ""
      : ` Child tasks: ${reconciliationChildren.map((child) => child.taskId).join(", ")}.`;
  const warningSummary =
    reconciliationWarnings.length === 0
      ? ""
      : ` Warnings: ${reconciliationWarnings.join(" | ")}`;
  return taskResult(
    task,
    `${parsed.kind} action ${input.intent} finished with phase ${phase}.${childSummary}${warningSummary}`,
  );
}

async function decideTask(
  options: RegisterFacadeToolsOptions,
  input: {
    taskId: string;
    evidenceDigest?: string;
    reviewer: string;
    decision: "accept" | "reject";
    note: string;
  },
): Promise<CallToolResult> {
  const parsed = parseTaskId(input.taskId);
  if (parsed.kind !== "scene_inspection") {
    throw new Error(
      `takegraph_task_decide accepts scene_inspection tasks, not ${parsed.kind}`,
    );
  }
  const known = options.registry.get(input.taskId);
  if (!known) {
    throw new Error(
      "Scene decision requires a recovered reviewed envelope; call takegraph_task_execute with intent=revalidate first",
    );
  }
  if (!known.availableActions.includes("decide")) {
    throw new Error(
      `Scene decision is unavailable in phase ${known.phase}; open human review first`,
    );
  }
  const recordedReviewer =
    firstNestedString(known.source, new Set(["reviewer"])) ??
    firstNestedString(known.details, new Set(["reviewer"]));
  if (recordedReviewer && recordedReviewer !== input.reviewer) {
    throw new Error(
      `reviewer mismatch: evidence was opened by ${recordedReviewer}`,
    );
  }
  if (known.evidenceDigest) {
    exactKnownDigest(
      options.registry,
      input.taskId,
      requireInputDigest(input.evidenceDigest, "evidenceDigest"),
      "evidenceDigest",
    );
  } else if (input.evidenceDigest) {
    throw new Error(
      "This reviewed task did not expose an evidenceDigest; omit it and rely on the durable authenticated-receipt replay",
    );
  }
  const result = await options.ymm4.decideSceneInspection(
    parsed.nativeId,
    input.decision,
    input.note,
  );
  const task = rememberCanonicalTask(options.registry, {
    kind: parsed.kind,
    nativeId: parsed.nativeId,
    phase: input.decision === "accept" ? "accepted" : "rejected",
    result,
    planDigest: known?.planDigest ?? undefined,
    approvedPlanDigest: known?.approvedPlanDigest ?? undefined,
    availableActions: ["inspect"],
    source: {
      ...(asRecord(sourceFromResult(result, "canonical-project")) ?? {}),
      reviewer: input.reviewer,
    },
  });
  return sceneTaskResult(
    options,
    task,
    `Human reviewer ${input.reviewer} recorded ${input.decision}.`,
    result,
    false,
  );
}

export function registerFacadeTools(
  server: McpServer,
  options: RegisterFacadeToolsOptions,
): void {
  registerAppTool(
    server,
    "takegraph_inspect",
    {
      title: "Inspect TakeGraph",
      description:
        "Read one uniform view of the studio session, canonical YMM4 project, task lifecycle, catalogs, or partial scene composition. This never stages or mutates a project.",
      inputSchema: {
        view: z.enum(INSPECT_VIEWS).default("overview"),
        taskId: z.string().min(1).optional(),
        include: z.array(z.enum(["composition"])).default([]),
      },
      annotations: { readOnlyHint: true, destructiveHint: false },
      _meta: {
        ui: {
          resourceUri: options.resourceUri,
          visibility: ["model", "app"],
        },
      },
    },
    async ({ view, taskId, include }) => {
      try {
        if (view === "studio") {
          const studio = options.session.snapshot();
          return {
            content: [
              {
                type: "text",
                text: formatStudioSessionText(studio, "Studio session inventory."),
              },
            ],
            structuredContent: { view, studio },
          };
        }
        if (view === "canonical" || view === "scene") {
          const wantsComposition =
            view === "scene" || include.includes("composition");
          const scene = wantsComposition
            ? await sceneDescription(options.ymm4)
            : undefined;
          const rawCanonical =
            scene?.rawCanonical ?? (await rawCanonicalDescription(options.ymm4));
          const canonical =
            scene?.canonical ??
            ((asRecord(modelFacingDetails(rawCanonical)) ?? {}) as UnknownRecord);
          const structured = scene
            ? { view, canonical, composition: scene.composition }
            : { view, canonical };
          const text =
            view === "scene"
              ? [
                  scene?.currentFrameObserved
                    ? "Source-bound current-frame scene observation returned. Unavailable geometry was not inferred."
                    : "Timeline scene observation returned. Current-frame geometry unavailable from the bridge is explicit and was not inferred.",
                  formatYmm4DescribeText(rawCanonical),
                  formatCompositionText(scene?.composition ?? {}),
                ].join("\n\n")
              : formatYmm4DescribeText(rawCanonical);
          return {
            content: [{ type: "text", text }],
            structuredContent: structured,
          };
        }
        if (view === "catalog") {
          const [nativeExtensions, renderProfiles] = await Promise.all([
            options.ymm4.nativeExtensionDescriptors(),
            options.ymm4.renderProfiles(),
          ]);
          return {
            content: [
              {
                type: "text",
                text: [
                  formatDescriptorInventoryText(nativeExtensions),
                  formatRenderProfilesText(renderProfiles),
                ].join("\n\n"),
              },
            ],
            structuredContent: { view, nativeExtensions, renderProfiles },
          };
        }
        if (view === "tasks") {
          const tasks = options.registry.list();
          return {
            content: [{ type: "text", text: formatTaskListText(tasks) }],
            structuredContent: { view, tasks },
          };
        }
        if (view === "task") {
          if (!taskId) throw new Error("taskId is required for view=task");
          const parsed = parseTaskId(taskId);
          let task = options.registry.get(taskId);
          if (task && parsed.kind === "project_initialization") {
            const recovered = await options.ymm4.projectInitializationStatus(
              parsed.nativeId,
            );
            const phase = phaseFromResult(recovered, task.phase);
            const presentation = projectInitializationPresentation(recovered);
            task = rememberCanonicalTask(options.registry, {
              kind: parsed.kind,
              nativeId: parsed.nativeId,
              phase,
              result: presentation,
              revisionEffect: projectInitializationRevisionEffect(recovered),
              availableActions: actionsFor(parsed.kind, phase, recovered),
            });
          }
          if (!task) {
            let recovered: unknown;
            switch (parsed.kind) {
              case "project_initialization":
                recovered = await options.ymm4.projectInitializationStatus(
                  parsed.nativeId,
                );
                break;
              case "native_extension":
              case "scene_inspection":
                throw new Error(
                  `${taskId} is not cached and its durable revalidation may update workflow metadata. Use takegraph_task_execute with intent=revalidate, then inspect the returned envelope.`,
                );
              case "timeline_edit":
                throw new Error(
                  `${taskId} is not cached. Use takegraph_task_execute with intent=revalidate to validate and recover its read-only durable task status, exact planDigest, and availableActions.`,
                );
              case "checkpoint":
                recovered = await options.ymm4.checkpointStatus(parsed.nativeId);
                break;
              case "render":
                recovered = await options.ymm4.renderStatus(parsed.nativeId);
                break;
              case "reconciliation_detach":
              case "reconciliation_re_export":
              case "reconciliation_import":
                recovered = await options.ymm4.reconciliationChildStatus(parsed.nativeId);
                break;
              default:
                throw new Error(
                  `${taskId} is not cached and this workflow has no read-only durable status adapter. Re-stage it to recover an exact planDigest before approval or execution.`,
                );
            }
            const phase = parsed.kind.startsWith("reconciliation_")
              ? reconciliationChildPhase(parsed.kind, recovered)
              : phaseFromResult(recovered, "observed");
            const presentation =
              parsed.kind === "project_initialization"
                ? projectInitializationPresentation(recovered)
                : recovered;
            task = rememberCanonicalTask(options.registry, {
              kind: parsed.kind,
              nativeId: parsed.nativeId,
              phase,
              result: presentation,
              revisionEffect:
                parsed.kind === "project_initialization"
                  ? projectInitializationRevisionEffect(recovered)
                  : undefined,
              availableActions: actionsFor(parsed.kind, phase, recovered),
            });
          }
          return taskResult(task, "Task lifecycle inspected.");
        }

        const studio = options.session.snapshot();
        try {
          const rawCanonical = await rawCanonicalDescription(options.ymm4);
          const scene = include.includes("composition")
            ? await sceneDescription(options.ymm4)
            : undefined;
          const canonical =
            scene?.canonical ??
            ((asRecord(modelFacingDetails(rawCanonical)) ?? {}) as UnknownRecord);
          return {
            content: [
              {
                type: "text",
                text: formatStatusText({ studio, canonical: rawCanonical }),
              },
            ],
            structuredContent: {
              view: "overview",
              studio,
              canonical,
              ...(scene ? { composition: scene.composition } : {}),
              guide: TAKEGRAPH_AGENT_GUIDE,
            },
          };
        } catch (error) {
          return {
            content: [
              {
                type: "text",
                text: formatStatusText({
                  studio,
                  canonicalError: formatAgentError(error),
                }),
              },
            ],
            structuredContent: {
              view: "overview",
              studio,
              canonicalError: formatAgentError(error),
              guide: TAKEGRAPH_AGENT_GUIDE,
            },
          };
        }
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "takegraph_task_stage",
    {
      title: "Stage TakeGraph task",
      description:
        "Create an immutable candidate, observation, or digest-bound plan through its owning store. timeline_edit accepts 1-128 ordered portable and native voice creates in one atomic managed-cue plan. Native extensions remain on their separate guarded task kind. Staging never substitutes for approval or execution.",
      inputSchema: stageHostInputSchema,
      outputSchema: taskEnvelopeOutputShape,
      annotations: { destructiveHint: false },
    },
    async (input) => {
      try {
        return await stageTask(options, stageInputSchema.parse(input));
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "takegraph_task_approve",
    {
      title: "Approve TakeGraph task",
      description:
        "Approve an exact staged plan digest only for workflows with a separate durable approval transition. Follow availableActions; atomic commit workflows approve during execute.",
      inputSchema: {
        taskId: z.string().min(1),
        planDigest: sha256Schema,
      },
      outputSchema: taskEnvelopeOutputShape,
      annotations: { destructiveHint: false },
    },
    async ({ taskId, planDigest }) => {
      try {
        return await approveTask(options, taskId, planDigest);
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "takegraph_task_execute",
    {
      title: "Execute TakeGraph task",
      description:
        "Run, review, revalidate, cancel, or collect artifacts for one typed task. The owning workflow still enforces source freshness, exact approval, and base-revision checks.",
      inputSchema: {
        taskId: z.string().min(1),
        intent: z
          .enum(["run", "review", "revalidate", "cancel", "collect_artifacts"])
          .default("run"),
        planDigest: sha256Schema.optional(),
        reviewer: z.string().min(1).optional(),
      },
      outputSchema: taskEnvelopeOutputShape,
      annotations: { destructiveHint: true },
    },
    async (input) => {
      try {
        return await executeTask(options, input);
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "takegraph_task_decide",
    {
      title: "Decide TakeGraph evidence",
      description:
        "Record an explicit human accept/reject decision for reviewed scene evidence. A reported evidenceDigest must match exactly; otherwise the owning workflow replays its durable authenticated receipt. Automated findings cannot invoke this decision.",
      inputSchema: {
        taskId: z.string().min(1),
        evidenceDigest: sha256Schema.optional(),
        reviewer: z.string().min(1),
        decision: z.enum(["accept", "reject"]),
        note: z.string().min(1),
      },
      outputSchema: taskEnvelopeOutputShape,
      annotations: { destructiveHint: true },
    },
    async (input) => {
      try {
        return await decideTask(options, input);
      } catch (error) {
        return errorResult(error);
      }
    },
  );
}
