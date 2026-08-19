/**
 * Stable task vocabulary exposed by the model-facing MCP facade.
 *
 * Native workflow identifiers remain internal to their owning store. A task ID
 * only adds an allowlisted kind so the facade can route an opaque native ID
 * without guessing its workflow from payload details.
 *
 * Adding or renaming a kind also requires updating
 * `.agents/skills/takegraph/SKILL.md`. `agent-skill.test.ts` fails if a
 * `TASK_KINDS` value is missing from that file.
 */
export const TASK_KINDS = [
  "studio_take",
  "studio_voice_variant",
  "timeline_edit",
  "portable_voice",
  "native_voice",
  "native_voice_mutation",
  "native_extension",
  "project_initialization",
  "scene_inspection",
  "annotation_derive",
  "checkpoint",
  "render",
  "reconciliation",
  "reconciliation_import",
  "reconciliation_detach",
  "reconciliation_re_export",
] as const;

export type TaskKind = (typeof TASK_KINDS)[number];

export const TASK_STORES = ["studio-session", "canonical-project"] as const;

export type TaskStore = (typeof TASK_STORES)[number];

export const TASK_ACTIONS = [
  "inspect",
  "stage",
  "approve",
  "execute",
  "cancel",
  "decide",
] as const;

export type TaskAction = (typeof TASK_ACTIONS)[number];

export type JsonPrimitive = string | number | boolean | null;
export type JsonObject = { [key: string]: JsonValue };
export type JsonValue = JsonPrimitive | JsonObject | JsonValue[];

export type TaskId = `${TaskKind}:${string}`;

export interface ParsedTaskId {
  kind: TaskKind;
  nativeId: string;
}

export interface TaskEnvelope {
  taskId: TaskId;
  kind: TaskKind;
  store: TaskStore;
  phase: string;
  source: JsonValue;
  planDigest: string | null;
  approvedPlanDigest: string | null;
  evidenceDigest: string | null;
  receiptDigest: string | null;
  revisionEffect: string;
  availableActions: TaskAction[];
  staleReasons: string[];
  details: JsonValue;
}

export interface TaskEnvelopeInput {
  kind: TaskKind;
  nativeId: string;
  store: TaskStore;
  phase: string;
  source?: unknown;
  planDigest?: string;
  approvedPlanDigest?: string;
  evidenceDigest?: string;
  receiptDigest?: string;
  revisionEffect: string;
  availableActions: readonly TaskAction[];
  staleReasons: readonly string[];
  details: unknown;
}

const TASK_KIND_SET = new Set<string>(TASK_KINDS);
const TASK_STORE_SET = new Set<string>(TASK_STORES);
const TASK_ACTION_SET = new Set<string>(TASK_ACTIONS);
const TASK_ID_SEPARATOR = ":";
const MAX_NATIVE_ID_LENGTH = 256;
const NATIVE_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

function requireTaskKind(value: unknown): TaskKind {
  if (typeof value !== "string" || !TASK_KIND_SET.has(value)) {
    throw new Error(`Unknown task kind: ${String(value)}`);
  }
  return value as TaskKind;
}

function requireNativeId(value: unknown): string {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > MAX_NATIVE_ID_LENGTH ||
    !NATIVE_ID_PATTERN.test(value) ||
    value === "." ||
    value === ".."
  ) {
    throw new Error(
      "Invalid native task ID: expected 1-256 ASCII letters, digits, dots, underscores, or hyphens, starting with a letter or digit",
    );
  }
  return value;
}

/** Create the only model-facing representation of a native task identifier. */
export function makeTaskId(kind: TaskKind, nativeId: string): TaskId {
  return `${requireTaskKind(kind)}${TASK_ID_SEPARATOR}${requireNativeId(nativeId)}`;
}

/**
 * Parse a model-facing task ID without accepting unknown task kinds or an ID
 * that could be interpreted as a path, control sequence, or nested separator.
 */
export function parseTaskId(taskId: string): ParsedTaskId {
  if (typeof taskId !== "string") {
    throw new Error("Invalid task ID: expected a string");
  }

  const separatorIndex = taskId.indexOf(TASK_ID_SEPARATOR);
  if (
    separatorIndex <= 0 ||
    separatorIndex !== taskId.lastIndexOf(TASK_ID_SEPARATOR)
  ) {
    throw new Error("Invalid task ID: expected exactly one kind separator");
  }

  const kind = requireTaskKind(taskId.slice(0, separatorIndex));
  const nativeId = requireNativeId(taskId.slice(separatorIndex + 1));
  return { kind, nativeId };
}

function requireNonEmptyString(value: unknown, field: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${field} must be a non-empty string`);
  }
  return value;
}

function copyJsonValue(
  value: unknown,
  field: string,
  ancestors: Set<object>,
): JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new Error(`${field} must contain only finite JSON numbers`);
    }
    return value;
  }
  if (typeof value !== "object") {
    throw new Error(`${field} must be JSON-safe`);
  }
  if (ancestors.has(value)) {
    throw new Error(`${field} must not contain a cycle`);
  }

  ancestors.add(value);
  try {
    if (Array.isArray(value)) {
      const result: JsonValue[] = [];
      for (let index = 0; index < value.length; index += 1) {
        if (!Object.hasOwn(value, index)) {
          throw new Error(`${field} must not contain sparse arrays`);
        }
        result.push(copyJsonValue(value[index], `${field}[${index}]`, ancestors));
      }
      return result;
    }

    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      throw new Error(`${field} must contain only JSON objects and arrays`);
    }

    const result: JsonObject = {};
    for (const key of Object.keys(value)) {
      result[key] = copyJsonValue(
        (value as Record<string, unknown>)[key],
        `${field}.${key}`,
        ancestors,
      );
    }
    return result;
  } finally {
    ancestors.delete(value);
  }
}

function copyStringList(values: readonly string[], field: string): string[] {
  if (!Array.isArray(values)) {
    throw new Error(`${field} must be an array`);
  }
  return values.map((value, index) =>
    requireNonEmptyString(value, `${field}[${index}]`),
  );
}

function optionalString(value: string | undefined, field: string): string | undefined {
  return value === undefined ? undefined : requireNonEmptyString(value, field);
}

/**
 * Build a uniform model-facing envelope while leaving workflow-specific details
 * opaque. JSON values are validated and copied so later caller mutation cannot
 * silently change the returned contract.
 */
export function buildTaskEnvelope(input: TaskEnvelopeInput): TaskEnvelope {
  const kind = requireTaskKind(input.kind);
  if (typeof input.store !== "string" || !TASK_STORE_SET.has(input.store)) {
    throw new Error(`Unknown task store: ${String(input.store)}`);
  }
  if (!Array.isArray(input.availableActions)) {
    throw new Error("availableActions must be an array");
  }

  const availableActions = input.availableActions.map((action) => {
    if (typeof action !== "string" || !TASK_ACTION_SET.has(action)) {
      throw new Error(`Unknown task action: ${String(action)}`);
    }
    return action as TaskAction;
  });

  const envelope: TaskEnvelope = {
    taskId: makeTaskId(kind, input.nativeId),
    kind,
    store: input.store,
    phase: requireNonEmptyString(input.phase, "phase"),
    source:
      input.source === undefined
        ? {}
        : copyJsonValue(input.source, "source", new Set()),
    planDigest: optionalString(input.planDigest, "planDigest") ?? null,
    approvedPlanDigest:
      optionalString(input.approvedPlanDigest, "approvedPlanDigest") ?? null,
    evidenceDigest:
      optionalString(input.evidenceDigest, "evidenceDigest") ?? null,
    receiptDigest:
      optionalString(input.receiptDigest, "receiptDigest") ?? null,
    revisionEffect: requireNonEmptyString(
      input.revisionEffect,
      "revisionEffect",
    ),
    availableActions,
    staleReasons: copyStringList(input.staleReasons, "staleReasons"),
    details: copyJsonValue(input.details, "details", new Set()),
  };

  return envelope;
}
