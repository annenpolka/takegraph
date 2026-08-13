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
const raw = (byte) => byte.repeat(64);
if (args[0] === "ymm4" && args[1] === "canonical-head") print({ projectId: "project-1", revision: 0 });
else if (args[0] === "ymm4" && args[1] === "snapshot") print({ projectId: "project-1", projectName: "test", projectPath: "test.ymmp", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
else if (args[0] === "ymm4" && args[1] === "native-extension-descriptors") print({
  targetCatalog: {
    catalogDigest: raw("a"),
    descriptors: [{ descriptorId: "character.marisa", configDigest: raw("b"), schemaDigest: raw("c"), bindable: true, mutationAllowed: false }]
  },
  planningCatalog: {},
  planningDescriptorDigests: { "character.marisa": "sha256:" + raw("d") }
});
else if (args[0] === "ymm4" && args[1] === "native-extension-stage") {
  const manifest = JSON.parse(fs.readFileSync(value("--manifest"), "utf8"));
  fs.writeFileSync(value("--task"), JSON.stringify({ patch: { status: "previewable", base: Number(value("--head")) } }));
  print({ digest: "sha256:" + raw("e"), operationId: "11111111-1111-4111-8111-111111111111", stateRoot: value("--state-root"), plan: { operations: manifest.intents }, lossyApprovals: [{ logicalKey: "portrait:portrait-01", approvedLossyFields: manifest.intents[0].intent.replacementGuard.approvedLossyFields }] });
}
else if (args[0] === "ymm4" && args[1] === "native-extension-approve") print({ status: "approved", digest: value("--digest"), stateRoot: value("--state-root") });
else if (args[0] === "ymm4" && args[1] === "native-extension-apply") { const base = JSON.parse(fs.readFileSync(value("--task"), "utf8")).patch.base; print({ status: "committed", baseRevision: base, revision: Number(value("--head")) + 1, canonicalReplay: false, verified: true, stateRoot: value("--state-root") }); }
else if (args[0] === "ymm4" && args[1] === "native-extension-verify") print({ verified: true });
else if (args[0] === "ymm4" && args[1] === "native-extension-status") print({ currentVerified: true });
else { process.stderr.write("unexpected args: " + args.join(" ")); process.exit(2); }
`;

test("native-extension workflow binds descriptors, exact loss, assets, and lifecycle", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-phase4-"));
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

  const descriptors = await workflow.nativeExtensionDescriptors();
  assert.equal(
    descriptors.planningDescriptorDigests["character.marisa"],
    `sha256:${"d".repeat(64)}`,
  );
  const staged = await workflow.stageNativeExtension({
    operations: [
      {
        type: "portrait",
        entityId: "portrait-01",
        entityRevision: 2,
        descriptorId: "character.marisa",
        expectedConfigDigest: "b".repeat(64),
        expectedSchemaDigest: `sha256:${"c".repeat(64)}`,
        frame: 120,
        layer: 10,
        durationFrames: 180,
        approvedLossyFields: ["nativeAnimation.keyframes"],
      },
      {
        type: "bgm",
        entityId: "bgm-01",
        entityRevision: 1,
        sourcePath: path.join(root, "bgm.wav"),
        artifactDigest: "f".repeat(64),
        mediaType: "audio/wav",
        byteLength: 4096,
        frame: 0,
        layer: 30,
        durationFrames: 600,
      },
    ],
  });
  assert.match(staged.handle, /^[0-9a-f-]{36}$/i);
  assert.equal(staged.digest, `sha256:${"e".repeat(64)}`);
  assert.equal(
    (staged as unknown as { stateRoot: string }).stateRoot,
    projectStateRoot,
  );

  const manifest = JSON.parse(
    await fs.readFile(
      path.join(
        stateDirectory,
        `${staged.handle}.native-extension.manifest.json`,
      ),
      "utf8",
    ),
  ) as {
    intents: Array<Record<string, any>>;
    artifactSources: Array<{ artifactDigest: string }>;
    changeBudget: { allowUnmanagedChanges: boolean };
  };
  assert.equal(
    manifest.intents[0]?.intent.characterBinding.expectedDigest,
    `sha256:${"d".repeat(64)}`,
  );
  assert.deepEqual(
    manifest.intents[0]?.intent.replacementGuard.approvedLossyFields,
    ["nativeAnimation.keyframes"],
  );
  assert.equal(manifest.intents[1]?.intent.asset.kind, "bgm");
  assert.equal(
    manifest.artifactSources[0]?.artifactDigest,
    `sha256:${"f".repeat(64)}`,
  );
  assert.equal(manifest.changeBudget.allowUnmanagedChanges, false);

  const approved = (await workflow.approveNativeExtension(
      staged.handle,
      staged.digest as string,
    )) as { status: string; stateRoot: string };
  assert.equal(approved.status, "approved");
  assert.equal(approved.stateRoot, projectStateRoot);
  const applied = (await workflow.applyNativeExtension(staged.handle)) as {
    revision: number;
    stateRoot: string;
  };
  assert.equal(applied.revision, 1);
  assert.equal(applied.stateRoot, projectStateRoot);
  assert.deepEqual(await workflow.verifyNativeExtension(staged.handle), {
    verified: true,
  });
  assert.deepEqual(await workflow.nativeExtensionStatus(staged.handle), {
    currentVerified: true,
  });
});

test("native-extension workflow rejects descriptor drift and unsafe handles", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-phase4-drift-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory: path.join(root, "state"),
  });
  await assert.rejects(
    () =>
      workflow.stageNativeExtension({
        operations: [
          {
            type: "face",
            entityId: "face-01",
            entityRevision: 1,
            descriptorId: "character.marisa",
            expectedConfigDigest: "0".repeat(64),
            expectedSchemaDigest: "c".repeat(64),
            frame: 0,
            layer: 1,
            durationFrames: 30,
          },
        ],
      }),
    /config\/schema drifted/,
  );
  await assert.rejects(
    () => workflow.verifyNativeExtension("..\\outside"),
    /Invalid YMM4 export handle/,
  );
});
