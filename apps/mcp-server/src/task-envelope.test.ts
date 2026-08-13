import assert from "node:assert/strict";
import test from "node:test";
import {
  buildTaskEnvelope,
  makeTaskId,
  parseTaskId,
  type TaskKind,
} from "./task-envelope.js";

const taskKinds: TaskKind[] = [
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
  "reconciliation_import",
  "reconciliation_detach",
  "reconciliation_re_export",
];

test("task IDs round-trip for every model-facing task kind", () => {
  for (const kind of taskKinds) {
    const taskId = makeTaskId(kind, "native_01.v2-final");
    assert.equal(taskId, `${kind}:native_01.v2-final`);
    assert.deepEqual(parseTaskId(taskId), {
      kind,
      nativeId: "native_01.v2-final",
    });
  }
});

test("task ID parsing rejects unknown kinds", () => {
  assert.throws(
    () => parseTaskId("voice_of_unknown_origin:abc"),
    /Unknown task kind/,
  );
  assert.throws(
    () => makeTaskId("unknown" as TaskKind, "abc"),
    /Unknown task kind/,
  );
});

test("task IDs reject separators, paths, empty IDs, and control characters", () => {
  for (const taskId of [
    "render:",
    "render:abc:def",
    "render:../secret",
    "render:folder\\secret",
    "render:C:\\secret",
    "render:abc\nnext",
    "render:abc\u0000def",
    "render:..",
  ]) {
    assert.throws(() => parseTaskId(taskId), /Invalid/);
  }

  for (const nativeId of ["", "abc:def", "../secret", "a/b", "a\\b", "a\tb"] ) {
    assert.throws(() => makeTaskId("render", nativeId), /Invalid native task ID/);
  }
});

test("builder produces an explicit, JSON-safe task envelope", () => {
  const details = {
    request: { frame: 42, include: ["composition"] },
    ready: true,
  };
  const source = { projectId: "project-1", revision: 7 };
  const envelope = buildTaskEnvelope({
    kind: "scene_inspection",
    nativeId: "inspection-7",
    store: "canonical-project",
    phase: "approved",
    source,
    planDigest: "plan-sha256",
    approvedPlanDigest: "plan-sha256",
    evidenceDigest: "evidence-sha256",
    receiptDigest: "receipt-sha256",
    revisionEffect: "none",
    availableActions: ["inspect", "execute", "decide"],
    staleReasons: [],
    details,
  });

  assert.deepEqual(envelope, {
    taskId: "scene_inspection:inspection-7",
    kind: "scene_inspection",
    store: "canonical-project",
    phase: "approved",
    source,
    planDigest: "plan-sha256",
    approvedPlanDigest: "plan-sha256",
    evidenceDigest: "evidence-sha256",
    receiptDigest: "receipt-sha256",
    revisionEffect: "none",
    availableActions: ["inspect", "execute", "decide"],
    staleReasons: [],
    details,
  });
  assert.equal("digest" in envelope, false);

  details.request.frame = 99;
  source.revision = 8;
  assert.equal(
    (envelope.details as { request: { frame: number } }).request.frame,
    42,
  );
  assert.equal((envelope.source as { revision: number }).revision, 7);
  assert.doesNotThrow(() => JSON.stringify(envelope));
});

test("builder rejects values that JSON cannot preserve", () => {
  const base = {
    kind: "render" as const,
    nativeId: "render-1",
    store: "canonical-project" as const,
    phase: "staged",
    revisionEffect: "none",
    availableActions: ["approve"] as const,
    staleReasons: [] as string[],
  };

  const withMissingBindings = buildTaskEnvelope({
    ...base,
    details: {},
  });
  assert.deepEqual(
    {
      source: withMissingBindings.source,
      planDigest: withMissingBindings.planDigest,
      approvedPlanDigest: withMissingBindings.approvedPlanDigest,
      evidenceDigest: withMissingBindings.evidenceDigest,
      receiptDigest: withMissingBindings.receiptDigest,
    },
    {
      source: {},
      planDigest: null,
      approvedPlanDigest: null,
      evidenceDigest: null,
      receiptDigest: null,
    },
  );

  assert.throws(
    () => buildTaskEnvelope({ ...base, details: { progress: Number.NaN } }),
    /finite JSON numbers/,
  );
  assert.throws(
    () => buildTaskEnvelope({ ...base, details: { callback: () => undefined } }),
    /JSON-safe/,
  );
  assert.throws(
    () => buildTaskEnvelope({ ...base, details: new Date() }),
    /JSON objects and arrays/,
  );

  const cyclic: { self?: unknown } = {};
  cyclic.self = cyclic;
  assert.throws(
    () => buildTaskEnvelope({ ...base, details: cyclic }),
    /must not contain a cycle/,
  );
});
