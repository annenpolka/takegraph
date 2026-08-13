import type { ProjectState, StagedPatchSummary } from "./project-session.js";

export const STORE_STUDIO = "studio-session";
export const STORE_CANONICAL = "canonical-project";

export const TAKEGRAPH_AGENT_GUIDE = [
  "TakeGraph MCP guide",
  "Two stores:",
  `- ${STORE_STUDIO}: in-memory demo. sessionRevision is not the canonical head.`,
  `- ${STORE_CANONICAL}: YMM4 + project-store. canonicalRevision is not the studio session head.`,
  "Always name the store when inspecting or staging; never carry an ID or revision from one store into the other.",
  "Five model-facing tools:",
  "- takegraph_inspect: read project, studio, composition, capabilities, task state, evidence, artifacts, and drift without changing state.",
  "- takegraph_task_stage: stage project initialization, an edit, inspection, checkpoint, render, or reconcile plan. Staging does not approve or execute it.",
  "- takegraph_task_approve: when availableActions includes approve, approve the exact planDigest visibly reported for a taskId. Approval does not execute the task.",
  "- takegraph_task_execute: run, review, revalidate, cancel, or collect artifacts. Atomic commit workflows take the exact planDigest here instead of exposing a separate approve phase.",
  "- takegraph_task_decide: accept or reject evidence only after an explicit human review. When evidenceDigest is present, the exact value is required; otherwise the durable authenticated receipt is replayed by the owning workflow.",
  "Task envelope:",
  "- taskId is the opaque public identity for every task. Do not substitute a legacy handle, patchId, or childTaskId.",
  "- planDigest binds approval to the exact staged plan. Re-inspect the task and approve again if that digest changes.",
  "- when present, evidenceDigest binds accept/reject to the exact evidence set; it is not a plan approval token. A null value must never be replaced with an invented digest.",
  "Workflow: takegraph_inspect -> takegraph_task_stage -> follow availableActions. Use approve only when exposed; otherwise execute with the exact planDigest when requested.",
  "Batch edits: prefer kind=timeline_edit with operations in canonical order. It accepts 1-128 portable_voice_create and native_voice_create operations as one managed-cue task, one exact planDigest, and one canonical commit. Preparation may run concurrently, but apply order remains the input order.",
  "After a timeline_edit restart or uncertain execution, use takegraph_task_execute with intent=revalidate on the same taskId. This payload-validates read-only durable status; retry only when returned availableActions includes execute.",
  "Native voice update/delete remain on kind=native_voice_mutation until the aggregate receipt represents them.",
  "Native extensions are not part of timeline_edit yet; keep them on kind=native_extension so the workflow never implies cross-route atomicity it cannot prove.",
  "Legacy kind=portable_voice and kind=native_voice still accept items with 1-128 entries; their flat single-item shapes remain compatible.",
  "Use takegraph_task_decide only when availableActions exposes decide after human review.",
  "Ordinary canonical writes require an initialized, named YMM4 project. Use kind=project_initialization with mode=adopt_active to adopt the active saved project.",
  "For an untitled active project, mode=save_untitled requires the user's explicit path during staging only. After staging, use only the opaque taskId and exact planDigest; TakeGraph never invents or repeats a Save As path.",
  "Project initialization uses a separate exact-digest approval before execute. Rendering requires a verified checkpoint, a bindable profile, and an absolute outputPath.",
  "Legacy route-specific tools are opt-in compatibility surfaces. Use them only when legacy routes were explicitly enabled.",
  "Do not rely on hidden structuredContent as the only source of taskId, planDigest, evidenceDigest, blockers, or next actions.",
].join("\n");

export interface AgentField {
  key: string;
  value: string | number | boolean | null | undefined;
}

export interface StagedTaskReport {
  kind: string;
  store: typeof STORE_STUDIO | typeof STORE_CANONICAL;
  identityKey: string;
  identity: string;
  digest?: string;
  next: string;
  fields?: AgentField[];
  blockers?: string[];
}

function line(key: string, value: string | number | boolean | null | undefined): string {
  if (value === undefined || value === null || value === "") {
    return `${key}: (none)`;
  }
  return `${key}: ${value}`;
}

function fieldLines(fields: AgentField[] | undefined): string[] {
  return (fields ?? []).map((field) => line(field.key, field.value));
}

export function formatStagedTaskReport(report: StagedTaskReport): string {
  const blockers = report.blockers ?? [];
  return [
    `${report.kind} staged.`,
    line("store", report.store),
    line(report.identityKey, report.identity),
    line("digest", report.digest),
    ...fieldLines(report.fields),
    ...blockers.map((blocker) => `blocked: ${blocker}`),
    line("next", report.next),
  ].join("\n");
}

export function formatFollowUpReport(input: {
  lead: string;
  store: typeof STORE_STUDIO | typeof STORE_CANONICAL;
  identityKey?: StagedTaskReport["identityKey"];
  identity?: string;
  digest?: string;
  next: string;
  fields?: AgentField[];
  blockers?: string[];
}): string {
  const lines = [input.lead, line("store", input.store)];
  if (input.identityKey && input.identity) {
    lines.push(line(input.identityKey, input.identity));
  }
  if (input.digest) {
    lines.push(line("digest", input.digest));
  }
  lines.push(...fieldLines(input.fields));
  for (const blocker of input.blockers ?? []) {
    lines.push(`blocked: ${blocker}`);
  }
  lines.push(line("next", input.next));
  return lines.join("\n");
}

export function formatStudioSessionText(state: ProjectState, lead: string): string {
  const readyTakes = state.takes.filter((take) => take.readiness === "ready");
  const queryReadyTakes = state.takes.filter((take) => take.readiness === "query-ready");
  const next = state.stagedPatch
    ? `studio_patch_commit { patchId: ${state.stagedPatch.id}, digest: ${state.stagedPatch.digest} }`
    : readyTakes.length > 0
      ? "voice_stage_take_patch { takeId } with a ready take, then studio_patch_commit { patchId, digest }"
      : "voice_generate_variant creates query-ready takes only; stage a ready take";
  const blockers: string[] = [];
  if (queryReadyTakes.length > 0) {
    blockers.push(
      `query-ready takes cannot be staged: ${queryReadyTakes.map((take) => take.id).join(", ")}`,
    );
  }
  const lines = [
    lead,
    line("store", STORE_STUDIO),
    "This is the in-memory studio session. sessionRevision is not the canonical YMM4/project-store head.",
    line("projectName", state.projectName),
    line("sessionRevision", state.revision),
    line("durationMs", state.durationMs),
    line("voiceEngine", state.voiceEngine),
    line("activeTakeId", state.activeTakeId),
    "utterances:",
    ...state.utterances.map(
      (utterance) =>
        `- ${utterance.id} speaker=${utterance.speaker} caption=${utterance.caption} startMs=${utterance.startMs}`,
    ),
    "takes:",
    ...state.takes.map(
      (take) =>
        `- ${take.id} utterance=${take.utteranceId} status=${take.status} readiness=${take.readiness} durationMs=${take.durationMs}`,
    ),
    ...formatStagedPatchLines(state.stagedPatch),
    ...blockers.map((blocker) => `blocked: ${blocker}`),
    line("next", next),
    "canonical: takegraph_status or ymm4_link_describe",
  ];
  return lines.join("\n");
}

function formatStagedPatchLines(patch: StagedPatchSummary | undefined): string[] {
  if (!patch) {
    return [line("stagedPatch", "none")];
  }
  return [
    "stagedPatch:",
    `  patchId: ${patch.id}`,
    `  digest: ${patch.digest}`,
    `  baseRevision: ${patch.baseRevision}`,
    `  takeId: ${patch.takeId}`,
    `  durationDeltaMs: ${patch.durationDeltaMs}`,
  ];
}

export function formatStudioGenerateText(state: ProjectState): string {
  const created = state.takes.at(-1);
  const lead = created
    ? `Created VoiceTake candidate ${created.id}.`
    : "Created a VoiceTake candidate.";
  return formatStudioSessionText(state, lead);
}

export function formatStudioStageText(state: ProjectState): string {
  const patch = state.stagedPatch;
  if (!patch) {
    return formatStudioSessionText(state, "Studio patch staging did not produce a staged patch.");
  }
  return formatStudioSessionText(
    state,
    `A previewable studio patch for ${patch.takeId} was staged.`,
  );
}

export function formatStudioCommitText(state: ProjectState): string {
  return formatStudioSessionText(
    state,
    `Studio patch committed. sessionRevision is now ${state.revision}.`,
  );
}

interface RecordLike {
  [key: string]: unknown;
}

function asRecord(value: unknown): RecordLike {
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    return value as RecordLike;
  }
  return {};
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function str(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function num(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function bool(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

export interface CanonicalDescribeInput {
  health?: unknown;
  capabilities?: unknown;
  snapshot?: unknown;
  head?: { projectId?: string; revision?: number };
  headError?: string;
}

export function formatYmm4DescribeText(input: CanonicalDescribeInput): string {
  const health = asRecord(input.health);
  const snapshot = asRecord(input.snapshot);
  const capabilities = asRecord(input.capabilities);
  const projectPath = str(snapshot.projectPath) ?? "";
  const unsaved = projectPath.length === 0;
  const managedItems = asArray(snapshot.managedItems);
  const nativeExtensions = asArray(snapshot.nativeExtensions);
  const capabilityList = asArray(capabilities.capabilities).filter(
    (value): value is string => typeof value === "string",
  );
  const blockers: string[] = [];
  if (unsaved) {
    blockers.push(
      "unsaved YMM4 project — canonical stage/apply, ymm4_project_save, and ymm4_checkpoint_stage require an existing path",
    );
  }
  if (input.headError) {
    blockers.push(input.headError);
  }
  const next = unsaved
    ? "takegraph_task_stage with kind=project_initialization, mode=save_untitled, and an explicit user-selected path"
    : "ymm4_native_voice_mutation_stage | ymm4_export_stage | ymm4_native_extension_descriptors";
  return [
    "Canonical YMM4 / project-store described.",
    line("store", STORE_CANONICAL),
    "This is not the in-memory studio session. Do not treat sessionRevision as this head.",
    line("health", str(health.status)),
    line("protocolVersion", num(health.protocolVersion)),
    line("pluginVersion", str(health.pluginVersion)),
    line("ymm4Version", str(health.ymm4Version)),
    line("projectId", str(snapshot.projectId) ?? str(input.head?.projectId)),
    line("canonicalRevision", input.head?.revision),
    line("projectName", str(snapshot.projectName)),
    line("projectPath", unsaved ? "(unsaved)" : projectPath),
    line("sceneId", str(snapshot.sceneId)),
    line("fps", num(snapshot.fps)),
    line("fingerprint", str(snapshot.fingerprint)),
    line("managedItemCount", managedItems.length),
    ...managedItems.slice(0, 32).map((item) => formatManagedItemLine(item)),
    line("nativeExtensionCount", nativeExtensions.length),
    line("unmanagedContextCount", num(snapshot.unmanagedContextCount)),
    line("capabilityCount", capabilityList.length),
    ...blockers.map((blocker) => `blocked: ${blocker}`),
    line("next", next),
    `studio-session: studio_project_describe (separate store ${STORE_STUDIO})`,
  ].join("\n");
}

function formatManagedItemLine(item: unknown): string {
  const record = asRecord(item);
  const entityId = str(record.entityId) ?? "(unknown)";
  const kind = str(record.kind) ?? "item";
  const text = str(record.text) ?? str(record.caption) ?? "";
  const speaker = str(record.speaker) ?? str(record.characterName) ?? "";
  const realizationId = str(record.realizationId);
  const frame = num(record.frame);
  const layer = num(record.layer);
  return `- ${entityId} kind=${kind} speaker=${speaker || "(none)"} text=${text || "(none)"} frame=${frame ?? "?"} layer=${layer ?? "?"} realizationId=${realizationId ?? "(none)"}`;
}

export function formatDescriptorInventoryText(result: unknown): string {
  const root = asRecord(result);
  const target = asRecord(root.targetCatalog);
  const targetDescriptors = asArray(target.descriptors);
  const planning = asRecord(root.planningCatalog);
  const characters = asRecord(planning.characters);
  const characterEntries = Object.values(characters);
  const lines = [
    "YMM4 native descriptors (bind these exact IDs; display names alone are never accepted).",
    line("store", STORE_CANONICAL),
    "targetCatalog.descriptors:",
  ];
  if (targetDescriptors.length === 0) {
    lines.push("- (none)");
  }
  for (const descriptor of targetDescriptors) {
    const record = asRecord(descriptor);
    lines.push(
      `- descriptorId=${str(record.descriptorId) ?? "(unknown)"} configDigest=${str(record.configDigest) ?? str(record.expectedConfigDigest) ?? "(none)"} schemaDigest=${str(record.schemaDigest) ?? str(record.expectedSchemaDigest) ?? "(none)"} bindable=${bool(record.bindable) ?? "?"}`,
    );
  }
  lines.push("planningCatalog.characters:");
  if (characterEntries.length === 0) {
    lines.push("- (none listed; use descriptorId from targetCatalog)");
  }
  for (const character of characterEntries) {
    const record = asRecord(character);
    const configuration = asRecord(record.configuration);
    lines.push(
      `- descriptorId=${str(record.descriptorId) ?? "(unknown)"} displayName=${str(record.displayName) ?? "(none)"} configDigest=${str(configuration["takegraph.targetConfigDigest"]) ?? "(none)"} schemaDigest=${str(configuration["takegraph.targetSchemaDigest"]) ?? "(none)"}`,
    );
  }
  lines.push(
    line(
      "next",
      "ymm4_native_extension_stage { operations with descriptorId, expectedConfigDigest, expectedSchemaDigest }",
    ),
  );
  return lines.join("\n");
}

export function formatRenderProfilesText(result: unknown): string {
  const root = asRecord(result);
  const profiles = asArray(root.profiles);
  const bindable = profiles.filter((profile) => bool(asRecord(profile).bindable) === true);
  const lines = [
    "YMM4 render profiles.",
    line("store", STORE_CANONICAL),
    "profiles:",
  ];
  if (profiles.length === 0) {
    lines.push("- (none)");
  }
  for (const profile of profiles) {
    const record = asRecord(profile);
    const id =
      str(record.descriptorId) ?? str(record.profile) ?? str(record.name) ?? "(unknown)";
    lines.push(
      `- ${id} bindable=${bool(record.bindable) ?? "?"} container=${str(record.container) ?? "(none)"} digest=${str(record.bindingManifestDigest) ?? "(none)"} error=${str(record.bindingError) ?? "(none)"}`,
    );
  }
  if (bindable.length === 0) {
    lines.push("blocked: no bindable render profile; ymm4_render_stage will fail closed");
    lines.push(line("next", "do not call ymm4_render_stage until a bindable profile exists"));
  } else {
    lines.push(
      line(
        "next",
        "ymm4_render_stage { checkpointOperationId, profile, outputPath } after a verified checkpoint",
      ),
    );
  }
  return lines.join("\n");
}

export function formatStatusText(input: {
  studio: ProjectState;
  canonical?: CanonicalDescribeInput;
  canonicalError?: string;
}): string {
  const lines = [
    "TakeGraph agent status.",
    TAKEGRAPH_AGENT_GUIDE,
    "",
    "--- studio-session ---",
    formatStudioSessionText(input.studio, "Studio session inventory."),
    "",
    "--- canonical-project ---",
  ];
  if (input.canonicalError) {
    lines.push(input.canonicalError);
  }
  if (input.canonical) {
    lines.push(formatYmm4DescribeText(input.canonical));
  } else if (!input.canonicalError) {
    lines.push("canonical-project: unavailable");
  }
  return lines.join("\n");
}

export function formatAgentError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const recovery = recoveryAdvice(message);
  return recovery ? `${message}\n${recovery}` : message;
}

function recoveryAdvice(message: string): string | undefined {
  if (/TargetLinkNotFound/i.test(message)) {
    return [
      `store: ${STORE_CANONICAL}`,
      "blocked: this YMM4 scene has no target link yet",
      "next: takegraph_task_stage with kind=native_voice_mutation, then takegraph_task_execute with the returned taskId and exact planDigest can establish the link",
      "cannot: takegraph_task_stage with kind=reconciliation and mode=report until a target link exists",
    ].join("\n");
  }
  if (/staged patch no longer exists/i.test(message) || /No staged studio patch/i.test(message)) {
    return "next: takegraph_task_stage with kind=studio_take and a ready takeId, then takegraph_task_execute with its taskId and exact planDigest";
  }
  if (/no completed audio artifact/i.test(message) || /query-ready/i.test(message)) {
    return "blocked: this take is query-ready only\nnext: takegraph_task_stage with kind=studio_take and a ready takeId";
  }
  if (/approval does not match/i.test(message)) {
    return "next: use takegraph_inspect with view=task when the task is available, or re-stage it, then pass the exact reported planDigest";
  }
  if (/Unknown utterance/i.test(message)) {
    return "next: takegraph_inspect with view=studio and use an utteranceId from that inventory";
  }
  if (/Unknown voice take/i.test(message)) {
    return "next: takegraph_inspect with view=studio and use a takeId from that inventory";
  }
  if (/existing path|Save As|project path|UnsavedProject/i.test(message)) {
    return "blocked: untitled YMM4 cannot stage ordinary writes, apply, save, or checkpoint\nnext: takegraph_task_stage with kind=project_initialization, mode=save_untitled, and an explicit path supplied at staging only; approve the exact planDigest before execute";
  }
  return undefined;
}
