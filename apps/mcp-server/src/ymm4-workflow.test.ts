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
else if (args[0] === "voicevox" && args[1] === "materialize") print({ speaker: "春日部つむぎ", speakerUuid: "speaker-1", style: "ノーマル", artifact: { style_id: 8, query_hash: "q".repeat(64), query_path: "query.json", audio_hash: "a".repeat(64), audio_path: "take.wav", wav: { duration_samples: 24000, sample_rate: 24000, channels: 1, bits_per_sample: 16 } } });
else if (args[0] === "ymm4" && args[1] === "export-stage") { const base = Number(value("--head")); fs.writeFileSync(value("--patch"), JSON.stringify({ patch: { base } })); print({ patchId: "patch-1", digest: "d".repeat(64), baseRevision: base, operationId: "11111111-1111-4111-8111-111111111111", project: { projectId: "project-1" }, plan: { operationCount: 1, createCount: 1, replaceCount: 0, unchangedCount: 0, stateRoot: value("--state-root") }, targetPlanDigest: "sha256:" + "t".repeat(64), targetPlan: { cues: [{ strategy: "portable_pair" }] }, patchFile: value("--patch") }); }
else if (args[0] === "ymm4" && args[1] === "export-commit") { const base = JSON.parse(fs.readFileSync(value("--patch"), "utf8")).patch.base; print({ baseRevision: base, revision: Number(value("--head")) + 1, canonicalReplay: false, operationId: "11111111-1111-4111-8111-111111111111", stateRoot: value("--state-root") }); }
else if (args[0] === "ymm4" && args[1] === "export-verify") print({ verified: true });
else if (args[0] === "ymm4" && args[1] === "native-voice-stage") { const manifest = JSON.parse(fs.readFileSync(value("--manifest"), "utf8")); if (manifest[0]?.displayText !== manifest[0]?.spokenText) { process.stderr.write("native voice text mismatch"); process.exit(3); } const base = Number(value("--head")); fs.writeFileSync(value("--patch"), JSON.stringify({ patch: { base } })); print({ patchId: "native-patch-1", digest: "n".repeat(64), baseRevision: base, operationId: "22222222-2222-4222-8222-222222222222", project: { projectId: "project-1" }, plan: { fingerprint: "f".repeat(64), createCount: 1, durationResolution: "bounded", stateRoot: value("--state-root") }, targetPlanDigest: "sha256:" + "u".repeat(64), targetPlan: { cues: [{ strategy: "native_voice" }] }, patchFile: value("--patch") }); }
else if (args[0] === "ymm4" && args[1] === "native-voice-commit") { const base = JSON.parse(fs.readFileSync(value("--patch"), "utf8")).patch.base; print({ baseRevision: base, revision: Number(value("--head")) + 1, canonicalReplay: false, operationId: "22222222-2222-4222-8222-222222222222", stateRoot: value("--state-root") }); }
else if (args[0] === "ymm4" && args[1] === "native-voice-verify") print({ verified: true, realizationKind: "ymm4_native_voice" });
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

test("YMM4 workflow rejects path-like export handles", async () => {
  const workflow = new Ymm4Workflow();
  await assert.rejects(() => workflow.verify("..\\outside"), /Invalid YMM4 export handle/);
  await assert.rejects(
    () => workflow.verifyNativeVoice("..\\outside"),
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

test("YMM4 native voice workflow rejects separate display and spoken text", async () => {
  const workflow = new Ymm4Workflow();
  await assert.rejects(
    () =>
      workflow.stageNativeVoice({
        entityId: "utt-native-02",
        displayText: "VOICEVOX",
        spokenText: "ボイスボックス",
        characterName: "春日部つむぎ",
        frame: 0,
        layer: 0,
        maxLength: 240,
      }),
    /displayText and spokenText must be identical/,
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
