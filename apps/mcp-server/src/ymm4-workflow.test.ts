import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { Ymm4Workflow } from "./ymm4-workflow.js";

const fakeCli = String.raw`
import fs from "node:fs";
const args = process.argv.slice(2);
const value = (name) => args[args.indexOf(name) + 1];
const print = (result) => process.stdout.write(JSON.stringify(result));
const projectPinnedCommands = new Set([
  "export-stage",
  "export-commit",
  "native-voice-stage",
  "native-voice-commit",
  "timeline-edit-stage",
  "timeline-edit-commit",
  "native-voice-mutation-stage",
  "native-voice-mutation-commit",
  "scene-stage",
]);
if (
  args[0] === "ymm4" &&
  projectPinnedCommands.has(args[1]) &&
  value("--expected-project-id") !== "project-1"
) {
  process.stderr.write("missing or incorrect expected project id");
  process.exit(4);
}
if (args[0] === "ymm4" && args[1] === "health") print({ status: "running", protocolVersion: 2 });
else if (args[0] === "ymm4" && args[1] === "capabilities") print({ protocolVersion: 2, capabilities: ["managed_audio"] });
else if (args[0] === "ymm4" && args[1] === "snapshot") print({ projectId: "project-1", projectName: "test", projectPath: "test.ymmp", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
else if (args[0] === "ymm4" && args[1] === "canonical-head") print({ projectId: "project-1", revision: 0 });
else if (args[0] === "voicevox" && args[1] === "materialize") print({ speaker: value("--speaker"), speakerUuid: "speaker-1", style: value("--style"), artifact: { style_id: 8, query_hash: "q".repeat(64), query_path: "query.json", audio_hash: "a".repeat(64), audio_path: "take.wav", wav: { duration_samples: 24000, sample_rate: 24000, channels: 1, bits_per_sample: 16 } } });
else if (args[0] === "ymm4" && args[1] === "export-stage") { const manifest = JSON.parse(fs.readFileSync(value("--manifest"), "utf8")); const base = Number(value("--head")); fs.writeFileSync(value("--patch"), JSON.stringify({ patch: { base } })); print({ patchId: "patch-1", digest: "d".repeat(64), baseRevision: base, operationId: "11111111-1111-4111-8111-111111111111", project: { projectId: "project-1" }, plan: { operationCount: manifest.length, createCount: manifest.length, replaceCount: 0, unchangedCount: 0, stateRoot: value("--state-root") }, targetPlanDigest: "sha256:" + "t".repeat(64), targetPlan: { cues: manifest.map(() => ({ strategy: "portable_pair" })) }, patchFile: value("--patch") }); }
else if (args[0] === "ymm4" && args[1] === "export-commit") { const base = JSON.parse(fs.readFileSync(value("--patch"), "utf8")).patch.base; print({ baseRevision: base, revision: Number(value("--head")) + 1, canonicalReplay: false, operationId: "11111111-1111-4111-8111-111111111111", stateRoot: value("--state-root") }); }
else if (args[0] === "ymm4" && args[1] === "export-verify") print({ verified: true });
else if (args[0] === "ymm4" && args[1] === "native-voice-stage") { const manifest = JSON.parse(fs.readFileSync(value("--manifest"), "utf8")); const base = Number(value("--head")); fs.writeFileSync(value("--patch"), JSON.stringify({ patch: { base } })); print({ patchId: "native-patch-1", digest: "n".repeat(64), baseRevision: base, operationId: "22222222-2222-4222-8222-222222222222", project: { projectId: "project-1" }, plan: { fingerprint: "f".repeat(64), createCount: manifest.length, durationResolution: "bounded", stateRoot: value("--state-root") }, targetPlanDigest: "sha256:" + "u".repeat(64), targetPlan: { cues: manifest.map(() => ({ strategy: "native_voice" })) }, patchFile: value("--patch") }); }
else if (args[0] === "ymm4" && args[1] === "native-voice-commit") { const base = JSON.parse(fs.readFileSync(value("--patch"), "utf8")).patch.base; print({ baseRevision: base, revision: Number(value("--head")) + 1, canonicalReplay: false, operationId: "22222222-2222-4222-8222-222222222222", stateRoot: value("--state-root") }); }
else if (args[0] === "ymm4" && args[1] === "native-voice-verify") print({ verified: true, realizationKind: "ymm4_native_voice" });
else if (args[0] === "ymm4" && args[1] === "timeline-edit-stage") { const manifest = JSON.parse(fs.readFileSync(value("--manifest"), "utf8")); const base = Number(value("--head")); const sourceEvidence = (manifest.operations || []).flatMap((operation) => operation.sourceEvidence ? [operation.sourceEvidence] : []); fs.writeFileSync(value("--task"), JSON.stringify({ patch: { base, digest: "1".repeat(64), approved_digest: null, status: "previewable" }, manifest, timelineEditPlan: { sourceEvidence } })); print({ taskFile: value("--task"), patchId: "timeline-patch-1", digest: "1".repeat(64), baseRevision: base, operationId: "55555555-5555-4555-8555-555555555555", project: { projectId: "project-1" }, planDigest: "sha256:" + "2".repeat(64), timelineEditPlan: { operationCount: manifest.operations.length, operationKinds: manifest.operations.map((operation) => operation.type), sourceEvidence } }); }
else if (args[0] === "ymm4" && args[1] === "timeline-edit-commit") { const task = JSON.parse(fs.readFileSync(value("--task"), "utf8")); task.patch.approved_digest = value("--digest"); task.patch.status = "committed"; fs.writeFileSync(value("--task"), JSON.stringify(task)); print({ taskFile: value("--task"), baseRevision: task.patch.base, revision: Number(value("--head")) + 1, canonicalReplay: false, operationId: "55555555-5555-4555-8555-555555555555", receipt: { status: "verified" }, status: "committed" }); }
else if (args[0] === "ymm4" && args[1] === "timeline-edit-verify") print({ taskFile: value("--task"), operationId: "55555555-5555-4555-8555-555555555555", verified: true, status: "verified", receipt: { status: "verified" } });
else if (args[0] === "ymm4" && args[1] === "timeline-edit-status") { const task = JSON.parse(fs.readFileSync(value("--task"), "utf8")); print({ taskFile: value("--task"), operationId: "55555555-5555-4555-8555-555555555555", patchStatus: task.patch.status, baseRevision: task.patch.base, digest: task.patch.digest, approvedDigest: task.patch.approved_digest, receiptStatus: null, receipt: null }); }
else if (args[0] === "ymm4" && args[1] === "native-voice-mutation-stage") { const manifest = JSON.parse(fs.readFileSync(value("--manifest"), "utf8")); const base = Number(value("--head")); fs.writeFileSync(value("--patch"), JSON.stringify({ patch: { base } })); print({ patchId: "mutation-patch-1", digest: "m".repeat(64), baseRevision: base, operationId: "33333333-3333-4333-8333-333333333333", project: { projectId: "project-1" }, plan: { fingerprint: "f".repeat(64), createCount: manifest.filter((v) => v.action === "create").length, updateCount: manifest.filter((v) => v.action === "update").length, deleteCount: manifest.filter((v) => v.action === "delete").length, durationResolution: "bounded", preservedFields: ["audioEffects"], stateRoot: value("--state-root") }, capabilityDigest: "sha256:" + "c".repeat(64), patchFile: value("--patch") }); }
else if (args[0] === "ymm4" && args[1] === "native-voice-mutation-commit") { const base = JSON.parse(fs.readFileSync(value("--patch"), "utf8")).patch.base; print({ baseRevision: base, revision: Number(value("--head")) + 1, canonicalReplay: false, operationId: "33333333-3333-4333-8333-333333333333", replayVerified: true, stateRoot: value("--state-root") }); }
else if (args[0] === "ymm4" && args[1] === "native-voice-mutation-verify") print({ verified: true });
else if (args[0] === "ymm4" && args[1] === "native-voice-mutation-artifacts") print({ operationId: "33333333-3333-4333-8333-333333333333", stateRoot: value("--state-root"), artifactSemantics: { audio: "exact_wav", provenance: "normalized_host_bound_voice_state", portableSynthesisQuery: false }, artifacts: [{ realizationId: "44444444-4444-4444-8444-444444444444", audioSha256: "a".repeat(64) }] });
else if (args[0] === "ymm4" && args[1] === "scene-stage") { const profile = JSON.parse(fs.readFileSync(value("--profile"), "utf8")); fs.writeFileSync(value("--task"), JSON.stringify({ receipt: { status: "staged" } })); print({ digest: "c".repeat(64), profile, frames: args.filter((arg, index) => args[index - 1] === "--frame").map(Number), receipt: { status: "staged", captures: [] } }); }
else if (args[0] === "ymm4" && args[1] === "scene-approve") print({ receipt: { status: "approved", captures: [] } });
else if (args[0] === "ymm4" && args[1] === "scene-capture") print({ receipt: { status: "captured", captures: [{ artifactPath: "artifact.png" }] }, semanticDiff: { changed: false } });
else if (args[0] === "ymm4" && args[1] === "scene-replay") print({ receipt: { status: "captured", captures: [{ artifactPath: "artifact.png" }] }, replayed: true });
else if (args[0] === "ymm4" && args[1] === "scene-review") print({ receipt: { status: "reviewed", review: { reviewer: value("--reviewer") }, captures: [{ artifactPath: "artifact.png" }] } });
else if (args[0] === "ymm4" && args[1] === "scene-decide") print({ receipt: { status: value("--decision") === "accept" ? "accepted" : "rejected", review: { note: value("--note") }, captures: [] } });
else if (args[0] === "ymm4" && args[1] === "scene-status") print({ receipt: { status: "accepted", captures: [] }, stale: false });
else if (args[0] === "ymm4" && args[1] === "save") print({ action: "save", success: true });
else if (args[0] === "annotation" && args[1] === "promote") print({ captureId: value("--capture"), operations: [{ type: "native_voice_create", cue: { entityId: "ann-n0", displayText: "Compression", characterName: value("--character-name"), frame: 10, layer: Number(value("--layer") || 2), maxLength: 300 }, sourceEvidence: { annotationId: value("--capture"), captureAudioSha256: "sha256:" + "a".repeat(64), transcriptDigest: "sha256:" + "b".repeat(64), interpretationDigest: "sha256:" + "c".repeat(64) } }] });
else if (args[0] === "annotation" && args[1] === "promotion-stage") print({ captureId: value("--capture"), status: "staged" });
else if (args[0] === "annotation" && args[1] === "promotion-commit") { const record = { captureId: value("--capture"), taskId: value("--task-id"), committedRevision: value("--committed-revision"), receiptDigest: value("--receipt-digest") }; try { fs.mkdirSync(value("--annotation-root"), { recursive: true }); fs.writeFileSync(value("--annotation-root") + "/last-promotion-commit.json", JSON.stringify(record)); } catch {} print({ captureId: value("--capture"), status: "committed" }); }
else { process.stderr.write("unexpected args: " + args.join(" ")); process.exit(2); }
`;

test("YMM4 workflow stages, commits, verifies, and saves through the CLI", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  const projectStateRoot = path.join(root, "custom-project-store");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    artifactDirectory: path.join(root, "artifacts"),
    projectStateRoot,
  });

  const described = await workflow.describe();
  assert.equal((described.health as { status: string }).status, "running");
  assert.equal(
    (described.health as { protocolVersion: number }).protocolVersion,
    2,
  );
  assert.equal(
    (described.capabilities as { protocolVersion: number }).protocolVersion,
    2,
  );

  const staged = await workflow.stage({
    entityId: "utt-01",
    caption: "ここから第二形態だぜ",
    spokenText: "ここからだいにけいたいだぜ",
    speaker: "春日部つむぎ",
    style: "ノーマル",
    frame: 120,
    audioLayer: 20,
    captionLayer: 21,
  });
  assert.equal(staged.digest, "d".repeat(64));
  assert.equal(staged.targetPlanDigest, `sha256:${"t".repeat(64)}`);
  assert.equal(
    (staged.targetPlan.cues as Array<{ strategy: string }>)[0]?.strategy,
    "portable_pair",
  );
  assert.ok(staged.placement);
  assert.equal(staged.placement.length, 60);
  assert.equal(
    (staged.impact as unknown as { stateRoot: string }).stateRoot,
    projectStateRoot,
  );
  const manifest = JSON.parse(
    await fs.readFile(path.join(stateDirectory, `${staged.handle}.manifest.json`), "utf8"),
  ) as Array<{ speaker: string; spokenText: string; length: number }>;
  assert.equal(manifest[0]?.speaker, "春日部つむぎ");
  assert.equal(manifest[0]?.spokenText, "ここからだいにけいたいだぜ");
  assert.equal(manifest[0]?.length, 60);

  const committed = await workflow.commit(staged.handle, staged.digest);
  assert.equal((committed as { revision: number }).revision, 1);
  assert.equal(
    (committed as { stateRoot: string }).stateRoot,
    projectStateRoot,
  );
  await assert.rejects(
    () => fs.access(path.join(stateDirectory, "revision.json")),
    /ENOENT/,
    "MCP must not maintain a second canonical revision mirror",
  );
  assert.deepEqual(await workflow.verify(staged.handle), { verified: true });
  assert.deepEqual(await workflow.save(), { action: "save", success: true });
});

test("YMM4 workflow stages an ordered portable voice batch with one manifest", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-portable-batch-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    artifactDirectory: path.join(root, "artifacts"),
    projectStateRoot: path.join(root, "project-store"),
  });

  const staged = await workflow.stage({
    items: [
      {
        entityId: "utt-batch-01",
        caption: "first caption",
        spokenText: "first reading",
        speaker: "speaker-first",
        style: "style-first",
        frame: 120,
        audioLayer: 20,
        captionLayer: 21,
      },
      {
        entityId: "utt-batch-02",
        caption: "second caption",
        spokenText: "second reading",
        speaker: "speaker-second",
        style: "style-second",
        frame: 300,
        audioLayer: 22,
        captionLayer: 23,
      },
    ],
  });

  assert.equal(staged.itemCount, 2);
  assert.equal(Object.hasOwn(staged, "placement"), false);
  assert.equal(Object.hasOwn(staged, "voice"), false);
  assert.deepEqual(
    staged.placements.map((placement) => placement.entityId),
    ["utt-batch-01", "utt-batch-02"],
  );
  assert.deepEqual(
    staged.voices.map((voice) => [voice.entityId, voice.speaker]),
    [
      ["utt-batch-01", "speaker-first"],
      ["utt-batch-02", "speaker-second"],
    ],
  );
  assert.equal((staged.targetPlan.cues as unknown[]).length, 2);

  const manifest = JSON.parse(
    await fs.readFile(path.join(stateDirectory, `${staged.handle}.manifest.json`), "utf8"),
  ) as Array<Record<string, unknown>>;
  assert.deepEqual(
    manifest.map((item) => [item.entityId, item.caption, item.spokenText, item.speaker]),
    [
      ["utt-batch-01", "first caption", "first reading", "speaker-first"],
      ["utt-batch-02", "second caption", "second reading", "speaker-second"],
    ],
  );
});

test("portable materialization waits for in-flight workers and stops queued work after failure", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-portable-failure-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-failing-cli.mjs");
  const artifactDirectory = path.join(root, "markers");
  await fs.writeFile(
    script,
    String.raw`
import fs from "node:fs";
import path from "node:path";
const args = process.argv.slice(2);
const value = (name) => args[args.indexOf(name) + 1];
const print = (result) => process.stdout.write(JSON.stringify(result));
if (args[0] === "ymm4" && args[1] === "canonical-head") {
  print({ projectId: "project-1", revision: 0 });
} else if (args[0] === "ymm4" && args[1] === "snapshot") {
  print({ projectId: "project-1", projectName: "test", projectPath: "test.ymmp", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
} else if (args[0] === "voicevox" && args[1] === "materialize") {
  const markerRoot = value("--artifact-root");
  const text = value("--text");
  fs.mkdirSync(markerRoot, { recursive: true });
  fs.writeFileSync(path.join(markerRoot, text + ".start"), "started");
  if (text === "fail") {
    for (let attempt = 0; attempt < 200; attempt += 1) {
      if (["slow-1", "slow-2", "slow-3"].every((name) => fs.existsSync(path.join(markerRoot, name + ".start")))) break;
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    fs.writeFileSync(path.join(markerRoot, text + ".end"), "failed");
    process.stderr.write("materialization failed");
    process.exit(9);
  }
  await new Promise((resolve) => setTimeout(resolve, 200));
  fs.writeFileSync(path.join(markerRoot, text + ".end"), "completed");
  print({ speaker: "speaker", speakerUuid: "speaker-1", style: "style", artifact: { style_id: 8, query_hash: "q".repeat(64), query_path: "query.json", audio_hash: "a".repeat(64), audio_path: "take.wav", wav: { duration_samples: 24000, sample_rate: 24000, channels: 1, bits_per_sample: 16 } } });
} else {
  process.stderr.write("unexpected command");
  process.exit(2);
}
`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory: path.join(root, "state"),
    artifactDirectory,
    projectStateRoot: path.join(root, "project-store"),
  });
  const item = (entityId: string, spokenText: string) => ({
    entityId,
    caption: spokenText,
    spokenText,
    speaker: "speaker",
    style: "style",
    frame: 0,
    audioLayer: 0,
    captionLayer: 1,
  });

  await assert.rejects(
    () =>
      workflow.stage({
        items: [
          item("failure", "fail"),
          item("slow-1", "slow-1"),
          item("slow-2", "slow-2"),
          item("slow-3", "slow-3"),
          item("queued-1", "queued-1"),
          item("queued-2", "queued-2"),
        ],
      }),
    /materialization failed/,
  );
  for (const name of ["fail", "slow-1", "slow-2", "slow-3"]) {
    await fs.access(path.join(artifactDirectory, `${name}.start`));
    await fs.access(path.join(artifactDirectory, `${name}.end`));
  }
  for (const name of ["queued-1", "queued-2"]) {
    await assert.rejects(
      () => fs.access(path.join(artifactDirectory, `${name}.start`)),
      /ENOENT/,
    );
  }
});

test("timeline edit staging does not leak provider paths from a failed preparation", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-timeline-edit-redaction-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-path-leaking-cli.mjs");
  const secretPath = path.join(root, "private", "provider-query.json");
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const print = (result) => process.stdout.write(JSON.stringify(result));
if (args[0] === "ymm4" && args[1] === "canonical-head") {
  print({ projectId: "project-1", revision: 0 });
} else if (args[0] === "ymm4" && args[1] === "snapshot") {
  print({ projectId: "project-1", projectName: "test", projectPath: "test.ymmp", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
} else if (args[0] === "voicevox" && args[1] === "materialize") {
  process.stderr.write(${JSON.stringify(secretPath)});
  process.exit(9);
} else {
  process.stderr.write("unexpected command");
  process.exit(2);
}
`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory: path.join(root, "state"),
    artifactDirectory: path.join(root, "artifacts"),
    projectStateRoot: path.join(root, "project-store"),
  });
  await assert.rejects(
    () =>
      workflow.stageTimelineEdit({
        operations: [
          {
            op: "portable_voice_create",
            entityId: "portable",
            caption: "caption",
            spokenText: "spoken",
            speaker: "speaker",
            style: "style",
            frame: 0,
            audioLayer: 0,
            captionLayer: 1,
          },
        ],
      }),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(error.message, /staging failed.*YMM4 was not changed/i);
      assert.doesNotMatch(error.message, new RegExp(secretPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
      return true;
    },
  );
});

test("timeline edit execution and verification hide host diagnostics and preserve recovery advice", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-timeline-edit-exec-redaction-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-path-leaking-timeline-cli.mjs");
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const print = (result) => process.stdout.write(JSON.stringify(result));
if (args[0] === "ymm4" && args[1] === "canonical-head") {
  print({ projectId: "project-1", revision: 0 });
} else if (args[0] === "ymm4" && args[1] === "snapshot") {
  print({ projectId: "project-1", projectName: "test", projectPath: "test.ymmp", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
} else if (args[0] === "ymm4" && (args[1] === "timeline-edit-commit" || args[1] === "timeline-edit-verify" || args[1] === "timeline-edit-status")) {
  process.stderr.write("C:\\private-host-path\\timeline-edit.task.json");
  process.exit(9);
} else {
  process.stderr.write("unexpected command");
  process.exit(2);
}
`,
    "utf8",
  );
  const stateDirectory = path.join(root, "state");
  await fs.mkdir(stateDirectory, { recursive: true });
  const handle = "66666666-6666-4666-8666-666666666666";
  await fs.writeFile(
    path.join(stateDirectory, `${handle}.timeline-edit.task.json`),
    JSON.stringify({
      patch: {
        base: 0,
        digest: "1".repeat(64),
        approved_digest: null,
        status: "committed",
      },
    }),
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot: path.join(root, "project-store"),
  });

  await assert.rejects(
    () => workflow.commitTimelineEdit(handle, "1".repeat(64)),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(error.message, /outcome may be unknown.*revalidate.*opaque taskId/i);
      assert.doesNotMatch(error.message, /private-host-path|timeline-edit\.task\.json/i);
      assert.doesNotMatch(error.message, new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
      return true;
    },
  );
  await assert.rejects(
    () => workflow.verifyTimelineEdit(handle),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(error.message, /semantic verification failed without exposing host diagnostics/i);
      assert.doesNotMatch(error.message, /private-host-path|timeline-edit\.task\.json/i);
      assert.doesNotMatch(error.message, new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
      return true;
    },
  );
  await assert.rejects(
    () => workflow.timelineEditStatus(handle),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(error.message, /status could not be recovered.*host diagnostics/i);
      assert.doesNotMatch(error.message, /private-host-path|timeline-edit\.task\.json/i);
      assert.doesNotMatch(error.message, new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
      return true;
    },
  );
});

test("YMM4 workflow rejects duplicate portable voice entity IDs before staging", async () => {
  const workflow = new Ymm4Workflow();
  await assert.rejects(
    () => workflow.stage({ items: [] }),
    /requires 1-128 items/,
  );
  await assert.rejects(
    () =>
      workflow.stage({
        items: [
          {
            entityId: "duplicate",
            caption: "one",
            spokenText: "one",
            speaker: "speaker",
            style: "style",
            frame: 0,
            audioLayer: 0,
            captionLayer: 1,
          },
          {
            entityId: "duplicate",
            caption: "two",
            spokenText: "two",
            speaker: "speaker",
            style: "style",
            frame: 10,
            audioLayer: 0,
            captionLayer: 1,
          },
        ],
      }),
    /entityIds must be unique/,
  );
});

test("YMM4 workflow rejects path-like export handles", async () => {
  const workflow = new Ymm4Workflow();
  await assert.rejects(() => workflow.verify("..\\outside"), /Invalid YMM4 export handle/);
  await assert.rejects(
    () => workflow.verifyNativeVoice("..\\outside"),
    /Invalid YMM4 export handle/,
  );
  await assert.rejects(
    () => workflow.verifyTimelineEdit("..\\outside"),
    /Invalid YMM4 export handle/,
  );
});

test("YMM4 workflow uses production environment roots by default", {
  concurrency: false,
}, () => {
  const previousStateRoot = process.env.TAKEGRAPH_PROJECT_STATE_ROOT;
  const previousOperationRoot = process.env.TAKEGRAPH_PROJECT_OPERATION_ROOT;
  const stateRoot = path.join(os.tmpdir(), "takegraph-env-project-store");
  const operationRoot = path.join(os.tmpdir(), "takegraph-env-operation-store");
  try {
    process.env.TAKEGRAPH_PROJECT_STATE_ROOT = stateRoot;
    process.env.TAKEGRAPH_PROJECT_OPERATION_ROOT = operationRoot;
    const workflow = new Ymm4Workflow() as unknown as {
      projectStateRoot: string;
      projectOperationRoot: string;
    };
    assert.equal(workflow.projectStateRoot, path.resolve(stateRoot));
    assert.equal(workflow.projectOperationRoot, path.resolve(operationRoot));
  } finally {
    if (previousStateRoot === undefined) {
      delete process.env.TAKEGRAPH_PROJECT_STATE_ROOT;
    } else {
      process.env.TAKEGRAPH_PROJECT_STATE_ROOT = previousStateRoot;
    }
    if (previousOperationRoot === undefined) {
      delete process.env.TAKEGRAPH_PROJECT_OPERATION_ROOT;
    } else {
      process.env.TAKEGRAPH_PROJECT_OPERATION_ROOT = previousOperationRoot;
    }
  }
});

test("annotation list is a one-shot CLI read and never starts listen", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-annotation-list-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const value = (name) => args[args.indexOf(name) + 1];
if (args[0] === "ymm4" && args[1] === "canonical-head") {
  process.stdout.write(JSON.stringify({ projectId: "project-1", revision: 3 }));
} else if (args[0] === "ymm4" && args[1] === "composition") {
  process.stdout.write(JSON.stringify({
    schemaVersion: 1,
    projectId: "project-1",
    sceneId: "scene-1",
    sourceFingerprint: "fp-live",
    fps: 30,
    frame: 12,
    viewport: { availability: "unavailable", width: null, height: null },
    elements: [],
    completeness: "partial",
    unavailableFields: ["viewport"],
  }));
} else if (args[0] === "annotation" && args[1] === "list") {
  if (args.includes("listen")) process.exit(3);
  process.stdout.write(JSON.stringify({
    projectId: value("--project-id"),
    sourceFingerprint: value("--source-fingerprint"),
    annotationRoot: value("--annotation-root"),
    annotations: [],
  }));
} else {
  process.stderr.write("unexpected args: " + args.join(" "));
  process.exit(2);
}
`,
    "utf8",
  );
  const annotationStoreRoot = path.join(root, "annotation-store");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    projectStateRoot: path.join(root, "project-store"),
    annotationStoreRoot,
  });
  const listed = (await workflow.listAnnotations()) as {
    projectId?: string;
    sourceFingerprint?: string;
    annotationRoot?: string;
  };
  assert.equal(listed.projectId, "project-1");
  assert.equal(listed.sourceFingerprint, "fp-live");
  assert.equal(listed.annotationRoot, path.resolve(annotationStoreRoot));
});

test("annotation promote stages a timeline_edit with sourceEvidence", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-annotation-promote-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    artifactDirectory: path.join(root, "artifacts"),
    projectStateRoot: path.join(root, "project-store"),
    annotationStoreRoot: path.join(root, "annotation-store"),
  });
  const captureId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  const promoted = (await workflow.promoteAnnotation({
    annotationId: captureId,
    characterName: "ゆっくり霊夢",
    layer: 2,
  })) as {
    staged?: { handle?: string; planDigest?: string };
    operations?: Array<{ sourceEvidence?: { annotationId?: string } }>;
  };
  assert.equal(promoted.operations?.[0]?.sourceEvidence?.annotationId, captureId);
  assert.equal(typeof promoted.staged?.handle, "string");
  assert.equal(promoted.staged?.planDigest, `sha256:${"2".repeat(64)}`);
  const manifest = JSON.parse(
    await fs.readFile(
      path.join(stateDirectory, `${promoted.staged?.handle}.timeline-edit.manifest.json`),
      "utf8",
    ),
  ) as {
    operations: Array<{ sourceEvidence?: { annotationId?: string } }>;
  };
  assert.equal(manifest.operations[0]?.sourceEvidence?.annotationId, captureId);

  const committed = (await workflow.commitTimelineEdit(
    promoted.staged?.handle as string,
    "1".repeat(64),
  )) as { revision?: number; status?: string };
  assert.equal(committed.revision, 1);
  assert.equal(committed.status, "committed");
  const promotion = JSON.parse(
    await fs.readFile(
      path.join(root, "annotation-store", "last-promotion-commit.json"),
      "utf8",
    ),
  ) as { captureId?: string; taskId?: string };
  assert.equal(promotion.captureId, captureId);
  assert.equal(promotion.taskId, "55555555-5555-4555-8555-555555555555");
});

test("YMM4 workflow rejects an active-project switch after reading canonical head", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-project-switch-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-project-switch.mjs");
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const value = (name) => args[args.indexOf(name) + 1];
const print = (result) => process.stdout.write(JSON.stringify(result));
if (args[0] === "ymm4" && args[1] === "canonical-head") print({ projectId: "project-a", revision: 0 });
else if (args[0] === "ymm4" && args[1] === "snapshot") {
  if (value("--expected-project-id") !== "project-a") process.exit(5);
  process.stderr.write("active YMM4 project changed: expected project-a, got project-b");
  process.exit(6);
} else { process.stderr.write("unexpected command"); process.exit(2); }
`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory: path.join(root, "state"),
    projectStateRoot: path.join(root, "project-store"),
  });

  await assert.rejects(
    () => workflow.stage({
      entityId: "utt-switch",
      caption: "表示",
      spokenText: "読み",
      speaker: "春日部つむぎ",
      style: "ノーマル",
      frame: 0,
      audioLayer: 0,
      captionLayer: 1,
    }),
    /active YMM4 project changed/,
  );
});

test("YMM4 workflow reads current-frame composition with canonical project binding", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-composition-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-composition-cli.mjs");
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const value = (name) => args[args.indexOf(name) + 1];
const print = (result) => process.stdout.write(JSON.stringify(result));
if (args[0] === "ymm4" && args[1] === "canonical-head") {
  print({ projectId: "project-composition", revision: 7 });
} else if (args[0] === "ymm4" && args[1] === "composition") {
  if (value("--expected-project-id") !== "project-composition") {
    process.stderr.write("missing canonical project binding");
    process.exit(4);
  }
  print({
    schemaVersion: 1,
    projectId: "project-composition",
    sceneId: "scene-1",
    sourceFingerprint: "f".repeat(64),
    fps: 60,
    frame: 120,
    viewport: { availability: "available", width: 1920, height: 1080 },
    elements: [{
      elementId: "realization-1",
      stability: "realization_identity",
      kind: "voice",
      frame: 100,
      layer: 3,
      length: 60,
      active: true,
      selected: false,
      text: "caption",
      visual: { availability: "available", x: 10, y: 20, width: 300, height: 80 },
    }],
    completeness: "complete",
    unavailableFields: [],
  });
} else {
  process.stderr.write("unexpected command: " + args.join(" "));
  process.exit(2);
}
`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    projectStateRoot: path.join(root, "project-store"),
  });

  const composition = await workflow.composition();
  assert.equal(composition.projectId, "project-composition");
  assert.equal(composition.frame, 120);
  assert.equal(composition.elements[0]?.elementId, "realization-1");
  assert.deepEqual(composition.viewport, {
    availability: "available",
    width: 1920,
    height: 1080,
  });
});

test("YMM4 workflow projects durable project initialization without leaking host bindings", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-project-init-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-project-init.mjs");
  const destination = path.join(root, "movie.ymmp");
  const operationId = "55555555-5555-4555-8555-555555555555";
  const planDigest = `sha256:${"a".repeat(64)}`;
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const value = (name) => args[args.indexOf(name) + 1];
const command = args[1];
if (command === "project-initialization-stage" && value("--mode") !== "save_untitled") process.exit(4);
if (command === "project-initialization-approve" && value("--digest") !== "${planDigest}") process.exit(5);
const status = command === "project-initialization-stage" ? "staged" : command === "project-initialization-approve" ? "approved" : "initialized";
process.stdout.write(JSON.stringify({
  schemaVersion: 1,
  generation: 2,
  payload: {
    plan: {
      operationId: "${operationId}",
      mode: "save_untitled",
      source: { projectId: "untitled", sceneId: "scene-1", fingerprint: "sha256:${"b".repeat(64)}", projectInstanceId: "private-instance", projectPathDigest: null },
      destination: { projectId: "saved-project", pathDigest: "sha256:${"c".repeat(64)}", fileName: "movie.ymmp" },
      canonicalRevision: 0,
      canonicalPreexisting: false,
      capabilityDigest: "sha256:${"d".repeat(64)}",
      planDigest: "${planDigest}"
    },
    destinationPath: value("--destination") ?? "${destination.replaceAll("\\", "\\\\")}",
    bridgeRequest: { requestDigest: "private-request", sourceProjectInstanceId: "private-instance" },
    approvedPlanDigest: status === "staged" ? null : "${planDigest}",
    status,
    resultProjectId: status === "initialized" ? "saved-project" : null,
    resultSceneId: status === "initialized" ? "scene-1" : null,
    canonicalRevision: status === "initialized" ? 0 : null,
    canonicalCreated: status === "initialized" ? true : null,
    executionReplayed: command === "project-initialization-status",
    error: null
  },
  recordDigest: "sha256:${"e".repeat(64)}"
}));
`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    projectStateRoot: path.join(root, "project-store"),
    projectOperationRoot: path.join(root, "operations"),
  });

  const staged = await workflow.stageProjectInitialization({
    mode: "save_untitled",
    path: destination,
  });
  assert.equal(staged.operationId, operationId);
  assert.equal(staged.planDigest, planDigest);
  assert.equal(staged.status, "staged");
  const encoded = JSON.stringify(staged);
  assert.doesNotMatch(encoded, /private-instance|private-request/);
  assert.doesNotMatch(encoded, new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));

  const approved = await workflow.approveProjectInitialization(operationId, planDigest);
  assert.equal(approved.status, "approved");
  const executed = await workflow.executeProjectInitialization(operationId);
  assert.equal(executed.status, "initialized");
  assert.deepEqual(executed.result, {
    projectId: "saved-project",
    sceneId: "scene-1",
    canonicalRevision: 0,
    canonicalCreated: true,
  });
  const observed = await workflow.projectInitializationStatus(operationId);
  assert.equal(observed.status, "initialized");
  assert.equal(observed.executionReplayed, true);
});

test("YMM4 workflow redacts a Save As path echoed by a failed CLI", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-project-init-error-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-project-init-error.mjs");
  const destination = path.join(root, "private-name.ymmp");
  await fs.writeFile(
    script,
    "process.stderr.write(process.argv.join(' ')); process.exit(9);",
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    projectStateRoot: path.join(root, "project-store"),
    projectOperationRoot: path.join(root, "operations"),
  });

  await assert.rejects(
    workflow.stageProjectInitialization({
      mode: "save_untitled",
      path: destination,
    }),
    (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      assert.match(message, /failed without exposing the selected path/i);
      assert.doesNotMatch(
        message,
        new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
      );
      assert.doesNotMatch(message, /private-name\.ymmp/i);
      return true;
    },
  );
});

test("YMM4 workflow redacts host diagnostics from failed project initialization execute", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-project-execute-error-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-project-execute-error.mjs");
  const privatePath = path.join(root, "private-output.ymmp");
  await fs.writeFile(
    script,
    `process.stderr.write(${JSON.stringify(privatePath)}); process.exit(9);`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    projectStateRoot: path.join(root, "project-store"),
    projectOperationRoot: path.join(root, "operations"),
  });

  await assert.rejects(
    workflow.executeProjectInitialization("55555555-5555-4555-8555-555555555555"),
    (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      assert.match(message, /outcome may be unknown/i);
      assert.doesNotMatch(message, /private-output\.ymmp/i);
      assert.doesNotMatch(
        message,
        new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
      );
      return true;
    },
  );
});

test("YMM4 workflow refuses canonical stage on an unsaved project", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-unsaved-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-unsaved-cli.mjs");
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const print = (value) => process.stdout.write(JSON.stringify(value));
if (args[0] === "ymm4" && args[1] === "canonical-head") print({ projectId: "project-1", revision: 0 });
else if (args[0] === "ymm4" && args[1] === "snapshot") print({ projectId: "project-1", projectName: "untitled", projectPath: "", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
else { process.stderr.write("unexpected command"); process.exit(2); }
`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory: path.join(root, "state"),
    projectStateRoot: path.join(root, "project-store"),
  });
  await assert.rejects(
    () =>
      workflow.stageNativeVoiceMutations({
        mutations: [
          {
            action: "create",
            entityId: "utt-1",
            revision: 0,
            characterName: "春日部つむぎ",
            displayText: "test",
            spokenText: "test",
            frame: 0,
            layer: 0,
            maxLength: 180,
          },
        ],
      }),
    /no existing path/,
  );
});

test("YMM4 workflow distinguishes an uninitialized canonical project from revision zero", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-uninitialized-head-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-uninitialized-head.mjs");
  await fs.writeFile(
    script,
    `process.stdout.write(JSON.stringify({ projectId: "project-1", initialized: false, revision: null }));`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    projectStateRoot: path.join(root, "project-store"),
  });
  await assert.rejects(
    workflow.canonicalHead(),
    /not initialized.*project_initialization/i,
  );
});

test("YMM4 workflow accepts an exact historical canonical replay after later commits", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-replay-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const handle = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  const stateDirectory = path.join(root, "state");
  const script = path.join(root, "fake-replay-cli.mjs");
  await fs.mkdir(stateDirectory, { recursive: true });
  await fs.writeFile(
    path.join(stateDirectory, `${handle}.patch.json`),
    JSON.stringify({ patch: { base: 0 } }),
    "utf8",
  );
  await fs.writeFile(
    script,
    String.raw`
const args = process.argv.slice(2);
const print = (value) => process.stdout.write(JSON.stringify(value));
if (args[0] === "ymm4" && args[1] === "canonical-head") print({ projectId: "project-1", revision: 3 });
else if (args[0] === "ymm4" && args[1] === "snapshot") print({ projectId: "project-1", projectName: "test", projectPath: "test.ymmp", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
else if (args[0] === "ymm4" && args[1] === "export-commit") print({ baseRevision: 0, revision: 1, canonicalReplay: true });
else { process.stderr.write("unexpected command"); process.exit(2); }
`,
    "utf8",
  );
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot: path.join(root, "project-store"),
  });

  const replayed = (await workflow.commit(handle, "d".repeat(64))) as {
    revision: number;
    canonicalReplay: boolean;
  };
  assert.equal(replayed.revision, 1);
  assert.equal(replayed.canonicalReplay, true);
});

test("YMM4 native voice workflow stages one v2 realization and verifies it", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-native-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  const projectStateRoot = path.join(root, "custom-project-store");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot,
  });

  const staged = await workflow.stageNativeVoice({
    entityId: "utt-native-01",
    displayText: "ここから第二形態だぜ",
    spokenText: "ここから第二形態だぜ",
    characterName: "春日部つむぎ",
    frame: 3600,
    layer: 0,
    maxLength: 240,
  });
  assert.equal(staged.digest, "n".repeat(64));
  assert.equal(staged.targetPlanDigest, `sha256:${"u".repeat(64)}`);
  assert.equal(
    (staged.targetPlan.cues as Array<{ strategy: string }>)[0]?.strategy,
    "native_voice",
  );
  assert.ok(staged.realizationId);
  assert.match(staged.realizationId, /^[0-9a-f-]{36}$/i);
  assert.deepEqual(staged.placement, {
    frame: 3600,
    layer: 0,
    maxLength: 240,
  });
  assert.equal(
    (staged.impact as unknown as { stateRoot: string }).stateRoot,
    projectStateRoot,
  );

  const manifest = JSON.parse(
    await fs.readFile(
      path.join(
        stateDirectory,
        `${staged.handle}.native-voice.manifest.json`,
      ),
      "utf8",
    ),
  ) as Array<Record<string, unknown>>;
  assert.equal(manifest.length, 1);
  assert.deepEqual(manifest[0], {
    realizationId: staged.realizationId,
    entityId: "utt-native-01",
    revision: 0,
    characterName: "春日部つむぎ",
    displayText: "ここから第二形態だぜ",
    spokenText: "ここから第二形態だぜ",
    frame: 3600,
    layer: 0,
    maxLength: 240,
  });

  const committed = await workflow.commitNativeVoice(
    staged.handle,
    staged.digest,
  );
  assert.equal((committed as { revision: number }).revision, 1);
  assert.equal(
    (committed as { stateRoot: string }).stateRoot,
    projectStateRoot,
  );
  assert.deepEqual(await workflow.verifyNativeVoice(staged.handle), {
    verified: true,
    realizationKind: "ymm4_native_voice",
  });
});

test("YMM4 native voice workflow stages an ordered batch with one manifest", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-native-batch-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot: path.join(root, "project-store"),
  });

  const staged = await workflow.stageNativeVoice({
    items: [
      {
        entityId: "native-batch-01",
        displayText: "first",
        spokenText: "first",
        characterName: "character-first",
        frame: 100,
        layer: 3,
        maxLength: 180,
      },
      {
        entityId: "native-batch-02",
        displayText: "second",
        spokenText: "second",
        characterName: "character-second",
        frame: 300,
        layer: 4,
        maxLength: 240,
      },
    ],
  });

  assert.equal(staged.itemCount, 2);
  assert.equal(Object.hasOwn(staged, "realizationId"), false);
  assert.equal(Object.hasOwn(staged, "placement"), false);
  assert.equal(staged.realizationIds.length, 2);
  assert.equal(new Set(staged.realizationIds).size, 2);
  assert.deepEqual(
    staged.placements.map((placement) => placement.entityId),
    ["native-batch-01", "native-batch-02"],
  );
  assert.equal((staged.targetPlan.cues as unknown[]).length, 2);

  const manifest = JSON.parse(
    await fs.readFile(
      path.join(stateDirectory, `${staged.handle}.native-voice.manifest.json`),
      "utf8",
    ),
  ) as Array<Record<string, unknown>>;
  assert.deepEqual(
    manifest.map((item) => [
      item.entityId,
      item.realizationId,
      item.characterName,
      item.frame,
    ]),
    [
      ["native-batch-01", staged.realizationIds[0], "character-first", 100],
      ["native-batch-02", staged.realizationIds[1], "character-second", 300],
    ],
  );
});

test("YMM4 timeline edit seals mixed managed-cue creates in input order", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-timeline-edit-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    artifactDirectory: path.join(root, "artifacts"),
    projectStateRoot: path.join(root, "project-store"),
  });

  const staged = await workflow.stageTimelineEdit({
    operations: [
      {
        op: "portable_voice_create",
        entityId: "portable-first",
        caption: "first caption",
        spokenText: "first reading",
        speaker: "speaker-first",
        style: "style-first",
        frame: 10,
        audioLayer: 20,
        captionLayer: 21,
      },
      {
        op: "native_voice_create",
        entityId: "native-second",
        displayText: "native second",
        spokenText: "native second",
        characterName: "character-second",
        frame: 120,
        layer: 3,
        maxLength: 240,
      },
      {
        op: "portable_voice_create",
        entityId: "portable-third",
        caption: "third caption",
        spokenText: "third reading",
        speaker: "speaker-third",
        style: "style-third",
        frame: 360,
        audioLayer: 22,
        captionLayer: 23,
      },
    ],
    maxChangedEntities: 4,
  });

  assert.equal(staged.digest, "1".repeat(64));
  assert.equal(staged.planDigest, `sha256:${"2".repeat(64)}`);
  assert.equal(staged.operationCount, 3);
  assert.deepEqual(staged.operationKinds, [
    "portable_voice_create",
    "native_voice_create",
    "portable_voice_create",
  ]);
  const manifest = JSON.parse(
    await fs.readFile(
      path.join(stateDirectory, `${staged.handle}.timeline-edit.manifest.json`),
      "utf8",
    ),
  ) as {
    operations: Array<{
      type: string;
      utterance?: Record<string, unknown>;
      cue?: Record<string, unknown>;
    }>;
    maxChangedEntities: number;
  };
  assert.equal(manifest.maxChangedEntities, 4);
  assert.deepEqual(
    manifest.operations.map((operation) => [
      operation.type,
      operation.utterance?.entityId ?? operation.cue?.entityId,
    ]),
    [
      ["portable_voice_create", "portable-first"],
      ["native_voice_create", "native-second"],
      ["portable_voice_create", "portable-third"],
    ],
  );
  assert.equal(manifest.operations[0]?.utterance?.spokenText, "first reading");
  assert.equal(manifest.operations[0]?.utterance?.length, 60);
  assert.equal(manifest.operations[1]?.cue?.characterName, "character-second");
  assert.match(String(manifest.operations[1]?.cue?.realizationId), /^[0-9a-f-]{36}$/i);

  assert.deepEqual(await workflow.timelineEditStatus(staged.handle), {
    taskFile: path.join(stateDirectory, `${staged.handle}.timeline-edit.task.json`),
    operationId: "55555555-5555-4555-8555-555555555555",
    patchStatus: "previewable",
    baseRevision: 0,
    digest: "1".repeat(64),
    approvedDigest: null,
    receiptStatus: null,
    receipt: null,
  });

  const committed = await workflow.commitTimelineEdit(staged.handle, staged.digest);
  assert.equal((committed as { revision: number }).revision, 1);
  assert.equal((committed as { status: string }).status, "committed");
  assert.deepEqual(await workflow.verifyTimelineEdit(staged.handle), {
    taskFile: path.join(stateDirectory, `${staged.handle}.timeline-edit.task.json`),
    operationId: "55555555-5555-4555-8555-555555555555",
    verified: true,
    status: "verified",
    receipt: { status: "verified" },
  });
  assert.equal(
    (await workflow.timelineEditStatus(staged.handle) as { patchStatus: string }).patchStatus,
    "committed",
  );
});

test("YMM4 timeline edit rejects unsupported or ambiguous operation sets before I/O", async () => {
  const workflow = new Ymm4Workflow();
  await assert.rejects(
    () => workflow.stageTimelineEdit({ operations: [] }),
    /requires 1-128 items/,
  );
  await assert.rejects(
    () =>
      workflow.stageTimelineEdit({
        operations: Array.from({ length: 129 }, (_, index) => ({
          op: "portable_voice_create" as const,
          entityId: `voice-${index}`,
          caption: "caption",
          spokenText: "spoken",
          speaker: "speaker",
          style: "style",
          frame: index,
          audioLayer: 0,
          captionLayer: 1,
        })),
      }),
    /requires 1-128 items/,
  );
  await assert.rejects(
    () =>
      workflow.stageTimelineEdit({
        operations: [
          {
            op: "portable_voice_create",
            entityId: "duplicate",
            caption: "one",
            spokenText: "one",
            speaker: "speaker",
            style: "style",
            frame: 0,
            audioLayer: 0,
            captionLayer: 1,
          },
          {
            op: "native_voice_create",
            entityId: "duplicate",
            displayText: "two",
            spokenText: "two",
            characterName: "character",
            frame: 10,
            layer: 2,
            maxLength: 120,
          },
        ],
      }),
    /entityIds must be unique/,
  );
  await assert.rejects(
    () =>
      workflow.stageTimelineEdit({
        operations: [
          {
            op: "portable_voice_create",
            entityId: "one",
            caption: "one",
            spokenText: "one",
            speaker: "speaker",
            style: "style",
            frame: 0,
            audioLayer: 0,
            captionLayer: 1,
          },
          {
            op: "native_voice_create",
            entityId: "two",
            displayText: "two",
            spokenText: "two",
            characterName: "character",
            frame: 10,
            layer: 2,
            maxLength: 120,
          },
        ],
        maxChangedEntities: 1,
      }),
    /must cover every operation/,
  );
});

test("YMM4 workflow rejects duplicate native voice entity IDs before staging", async () => {
  const workflow = new Ymm4Workflow();
  await assert.rejects(
    () =>
      workflow.stageNativeVoice({
        items: Array.from({ length: 129 }, (_, index) => ({
          entityId: `native-${index}`,
          displayText: "text",
          spokenText: "text",
          characterName: "character",
          frame: index,
          layer: 0,
          maxLength: 180,
        })),
      }),
    /requires 1-128 items/,
  );
  await assert.rejects(
    () =>
      workflow.stageNativeVoice({
        items: [
          {
            entityId: "duplicate",
            displayText: "one",
            spokenText: "one",
            characterName: "character",
            frame: 0,
            layer: 0,
            maxLength: 180,
          },
          {
            entityId: "duplicate",
            displayText: "two",
            spokenText: "two",
            characterName: "character",
            frame: 10,
            layer: 0,
            maxLength: 180,
          },
        ],
      }),
    /entityIds must be unique/,
  );
});



test("YMM4 native voice mutation workflow exposes create update delete and artifact CAS", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-mutation-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  const projectStateRoot = path.join(root, "custom-project-store");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot,
    nativeVoiceArtifactDirectory: path.join(root, "cas"),
  });
  const updateId = "44444444-4444-4444-8444-444444444444";
  const deleteId = "55555555-5555-4555-8555-555555555555";
  const staged = await workflow.stageNativeVoiceMutations({
    mutations: [
      {
        action: "create",
        entityId: "utt-create",
        revision: 1,
        characterName: "魔理沙",
        displayText: "ここから第二形態だぜ",
        spokenText: "ここから第二形態だぜ",
        frame: 100,
        layer: 10,
        maxLength: 180,
      },
      {
        action: "update",
        realizationId: updateId,
        entityId: "utt-update",
        revision: 2,
        characterName: "魔理沙",
        displayText: "更新だぜ",
        spokenText: "更新だぜ",
        frame: 200,
        layer: 11,
        maxLength: 180,
      },
      {
        action: "delete",
        realizationId: deleteId,
        entityId: "utt-delete",
        revision: 3,
      },
    ],
  });
  assert.equal(staged.digest, "m".repeat(64));
  assert.deepEqual(staged.impact, {
    fingerprint: "f".repeat(64),
    createCount: 1,
    updateCount: 1,
    deleteCount: 1,
    durationResolution: "bounded",
    preservedFields: ["audioEffects"],
    stateRoot: projectStateRoot,
  });
  assert.equal(staged.realizationIds[1], updateId);
  assert.equal(staged.realizationIds[2], deleteId);
  const manifest = JSON.parse(
    await fs.readFile(
      path.join(stateDirectory, `${staged.handle}.native-voice-mutation.manifest.json`),
      "utf8",
    ),
  ) as Array<Record<string, unknown>>;
  assert.equal(manifest[2]?.action, "delete");
  assert.equal(manifest[2]?.maxLength, 1);

  const committed = await workflow.commitNativeVoiceMutations(
    staged.handle,
    staged.digest,
  );
  assert.equal((committed as { revision: number }).revision, 1);
  assert.equal((committed as { replayVerified: boolean }).replayVerified, true);
  assert.equal(
    (committed as { stateRoot: string }).stateRoot,
    projectStateRoot,
  );
  assert.deepEqual(await workflow.verifyNativeVoiceMutations(staged.handle), {
    verified: true,
  });
  const artifacts = await workflow.captureNativeVoiceMutationArtifacts(staged.handle);
  assert.equal(
    (artifacts as { stateRoot: string }).stateRoot,
    projectStateRoot,
  );
  assert.equal(
    (artifacts as { artifactSemantics: { portableSynthesisQuery: boolean } })
      .artifactSemantics.portableSynthesisQuery,
    false,
  );
});

test("YMM4 scene workflow persists a handle and exposes the full review lifecycle", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-ymm4-scene-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "state");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
  });

  const staged = await workflow.stageSceneInspection({
    frames: [20, 10, 20],
    expectedWidth: 1920,
    expectedHeight: 1080,
    safeArea: { x: 80, y: 40, width: 1760, height: 1000 },
    regions: [],
  });
  assert.equal(staged.digest, "c".repeat(64));
  assert.deepEqual(staged.frames, [20, 10, 20]);
  assert.deepEqual((staged.profile as { safeArea: unknown }).safeArea, {
    x: 80,
    y: 40,
    width: 1760,
    height: 1000,
  });
  const taskPath = path.join(
    stateDirectory,
    `${staged.handle}.scene-inspection.json`,
  );
  assert.equal(JSON.parse(await fs.readFile(taskPath, "utf8")).receipt.status, "staged");

  assert.equal(
    ((await workflow.approveSceneInspection(staged.handle, "c".repeat(64))) as {
      receipt: { status: string };
    }).receipt.status,
    "approved",
  );
  assert.equal(
    ((await workflow.captureSceneInspection(staged.handle)) as {
      receipt: { status: string };
    }).receipt.status,
    "captured",
  );
  assert.equal(
    ((await workflow.reviewSceneInspection(staged.handle, "lance")) as {
      receipt: { status: string };
    }).receipt.status,
    "reviewed",
  );
  assert.equal(
    ((await workflow.decideSceneInspection(
      staged.handle,
      "accept",
      "manual review complete",
    )) as { receipt: { status: string } }).receipt.status,
    "accepted",
  );
  assert.equal(
    ((await workflow.sceneInspectionStatus(staged.handle)) as {
      stale: boolean;
    }).stale,
    false,
  );
  assert.equal(
    ((await workflow.replaySceneInspection(staged.handle)) as {
      replayed: boolean;
    }).replayed,
    true,
  );
});

test("YMM4 scene workflow rejects path-like receipt handles", async () => {
  const workflow = new Ymm4Workflow();
  await assert.rejects(
    () => workflow.sceneInspectionStatus("..\\receipt"),
    /Invalid YMM4 export handle/,
  );
  await assert.rejects(
    () =>
      workflow.sceneInspectionStatus(
        "55555555-5555-4555-8555-55555555555-",
      ),
    /Invalid YMM4 export handle/,
  );
});
