import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { Ymm4Workflow } from "./ymm4-workflow.js";

const checkpointId = "11111111-1111-4111-8111-111111111111";
const renderId = "22222222-2222-4222-8222-222222222222";
const reportDigest = "a".repeat(64);
const approvalDigest = "b".repeat(64);
const childTaskId = "c".repeat(64);

const fakeCli = String.raw`
import fs from "node:fs";
const args = process.argv.slice(2);
const value = (name) => args[args.indexOf(name) + 1];
const print = (result) => process.stdout.write(JSON.stringify(result));
const common = { stateRoot: value("--state-root"), operationRoot: value("--operation-root") };
if (args[0] !== "ymm4") { process.stderr.write("expected ymm4 command"); process.exit(2); }
if (args[1] === "canonical-head") print({ projectId: "project-1", revision: 0 });
else if (args[1] === "snapshot") print({ projectId: "project-1", projectName: "test", projectPath: "test.ymmp", sceneId: "scene-1", fps: 60, fingerprint: "f".repeat(64), managedItems: [], nativeExtensions: [], unmanagedContextCount: 0 });
else if (args[1] === "checkpoint-stage") print({ command: args[1], ...common, head: Number(value("--head")), payload: { request: { operationId: "${checkpointId}" }, status: "staged" } });
else if (args[1] === "checkpoint-execute") print({ command: args[1], ...common, operationId: value("--operation-id"), head: Number(value("--head")), payload: { status: "verified" } });
else if (args[1] === "checkpoint-status") print({ command: args[1], operationRoot: value("--operation-root"), operationId: value("--operation-id"), payload: { status: "verified" } });
else if (args[1] === "render-profiles") print({ profiles: [{ descriptorId: "final-mp4" }], descriptorSetDigest: "c".repeat(64) });
else if (args[1] === "render-stage") print({ command: args[1], ...common, checkpointOperationId: value("--checkpoint-operation-id"), profile: value("--profile"), outputPath: value("--output"), overwrite: args.includes("--overwrite"), head: Number(value("--head")), payload: { request: { taskId: "${renderId}" }, status: "staged" } });
else if (args[1] === "render-execute") print({ command: args[1], ...common, taskId: value("--task-id"), head: Number(value("--head")), payload: { status: "running" } });
else if (args[1] === "render-status") print({ command: args[1], operationRoot: value("--operation-root"), taskId: value("--task-id"), payload: { status: "verified" } });
else if (args[1] === "render-cancel") print({ command: args[1], operationRoot: value("--operation-root"), taskId: value("--task-id"), payload: { status: "cancelling" } });
else if (args[1] === "reconcile-report") print({ command: args[1], ...common, usesDurableProjection: !args.includes("--expected"), head: Number(value("--head")), payload: { report: { reportDigest: "${reportDigest}" }, status: "report_ready" } });
else if (args[1] === "reconcile-preview") print({ command: args[1], operationRoot: value("--operation-root"), reportDigest: value("--report-digest"), decisions: JSON.parse(fs.readFileSync(value("--decisions"), "utf8")), payload: { preview: { approvalDigest: "${approvalDigest}" }, status: "decision_preview_ready" } });
else if (args[1] === "reconcile-apply") print({ command: args[1], ...common, reportDigest: value("--report-digest"), approvalDigest: value("--digest"), head: Number(value("--head")), payload: { status: "actions_accepted" } });
else if (args[1] === "reconcile-child-status") print({ command: args[1], operationRoot: value("--operation-root"), childTaskId: value("--child-task-id"), payload: { type: "metadata_detach", task: { status: "preview_ready" } } });
else if (args[1] === "reconcile-detach-approve") print({ command: args[1], ...common, childTaskId: value("--child-task-id"), approvalDigest: value("--digest"), head: Number(value("--head")), payload: { type: "metadata_detach", task: { status: "approved" } } });
else if (args[1] === "reconcile-detach-execute") print({ command: args[1], ...common, childTaskId: value("--child-task-id"), head: Number(value("--head")), payload: { type: "metadata_detach", task: { status: "verified" } } });
else if (args[1] === "reconcile-re-export-dispatch") {
  const manifest = JSON.parse(fs.readFileSync(value("--manifest"), "utf8"));
  const preview = { patch: { status: "previewable", approvedDigest: null, digest: "e".repeat(64), base: Number(value("--head")) } };
  fs.writeFileSync(value("--output-task"), JSON.stringify(preview));
  print({ command: args[1], ...common, childTaskId: value("--child-task-id"), manifest, outputTask: value("--output-task"), head: Number(value("--head")), payload: { type: "canonical_re_export", task: { status: "preview_ready", downstreamPreview: { route: manifest.route, preview } } } });
}
else if (args[1] === "native-voice-mutation-commit") {
  const downstream = JSON.parse(fs.readFileSync(value("--patch"), "utf8"));
  print({ command: args[1], ...common, patchFile: value("--patch"), digest: value("--digest"), approvedDigestBefore: downstream.patch.approvedDigest, baseRevision: downstream.patch.base, revision: Number(value("--head")) + 1, canonicalReplay: false });
}
else if (args[1] === "export-commit") {
  const downstream = JSON.parse(fs.readFileSync(value("--patch"), "utf8"));
  print({ command: args[1], ...common, patchFile: value("--patch"), digest: value("--digest"), approvedDigestBefore: downstream.patch.approvedDigest, baseRevision: downstream.patch.base, revision: Number(value("--head")) + 1, canonicalReplay: false });
}
else if (args[1] === "native-extension-approve") {
  const downstream = JSON.parse(fs.readFileSync(value("--task"), "utf8"));
  print({ command: args[1], ...common, taskFile: value("--task"), digest: value("--digest"), approvedDigestBefore: downstream.patch.approvedDigest });
}
else { process.stderr.write("unexpected args: " + args.join(" ")); process.exit(3); }
`;

test("YMM4 Phase 5 workflow delegates durable checkpoint and render commands", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-phase5-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "workflow-state");
  const projectStateRoot = path.join(root, "custom-project-store");
  const projectOperationRoot = path.join(root, "project-operations");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot,
    projectOperationRoot,
  });

  const checkpoint = (await workflow.stageCheckpoint()) as {
    stateRoot: string;
    operationRoot: string;
    payload: { request: { operationId: string } };
  };
  assert.equal(checkpoint.stateRoot, projectStateRoot);
  assert.equal(checkpoint.operationRoot, projectOperationRoot);
  assert.equal(checkpoint.payload.request.operationId, checkpointId);
  assert.equal(
    ((await workflow.executeCheckpoint(checkpointId)) as { operationId: string })
      .operationId,
    checkpointId,
  );
  assert.equal(
    ((await workflow.checkpointStatus(checkpointId)) as { operationRoot: string })
      .operationRoot,
    projectOperationRoot,
  );

  assert.deepEqual(await workflow.renderProfiles(), {
    profiles: [{ descriptorId: "final-mp4" }],
    descriptorSetDigest: "c".repeat(64),
  });
  await assert.rejects(
    () =>
      workflow.stageRender({
        checkpointOperationId: checkpointId,
        profile: "final-mp4",
        outputPath: "relative.mp4",
      }),
    /outputPath must be absolute/,
  );
  const stagedRender = (await workflow.stageRender({
    checkpointOperationId: checkpointId,
    profile: "final-mp4",
    outputPath: path.join(root, "final.mp4"),
    overwrite: true,
  })) as { checkpointOperationId: string; overwrite: boolean; payload: { request: { taskId: string } } };
  assert.equal(stagedRender.checkpointOperationId, checkpointId);
  assert.equal(stagedRender.overwrite, true);
  assert.equal(stagedRender.payload.request.taskId, renderId);
  assert.equal(
    ((await workflow.executeRender(renderId)) as { taskId: string }).taskId,
    renderId,
  );
  assert.equal(
    ((await workflow.renderStatus(renderId)) as { payload: { status: string } })
      .payload.status,
    "verified",
  );
  assert.equal(
    ((await workflow.cancelRender(renderId)) as { payload: { status: string } })
      .payload.status,
    "cancelling",
  );
});

test("YMM4 reconciliation workflow uses durable expectations and binds exact digests", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-reconcile-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "workflow-state");
  const projectStateRoot = path.join(root, "custom-project-store");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot,
    projectOperationRoot: path.join(root, "project-operations"),
  });
  const report = await workflow.reconciliationReport();
  assert.match(report.handle, /^[0-9a-f-]{36}$/i);
  assert.equal(
    (report as unknown as { usesDurableProjection: boolean }).usesDurableProjection,
    true,
  );

  const decisions = [
    {
      entryId: `sha256:${"d".repeat(64)}`,
      choice: "re_export_canonical" as const,
    },
  ];
  const preview = await workflow.previewReconciliation(
    `sha256:${reportDigest}`,
    decisions,
  );
  assert.equal(
    (preview as unknown as { reportDigest: string }).reportDigest,
    reportDigest,
  );
  assert.deepEqual(
    (preview as unknown as { decisions: unknown }).decisions,
    [{ ...decisions[0], entryId: "d".repeat(64) }],
  );
  const applied = (await workflow.applyReconciliation(
    `sha256:${reportDigest}`,
    `sha256:${approvalDigest}`,
  )) as { reportDigest: string; approvalDigest: string };
  assert.equal(applied.reportDigest, reportDigest);
  assert.equal(applied.approvalDigest, approvalDigest);

  const child = (await workflow.reconciliationChildStatus(
    `sha256:${childTaskId}`,
  )) as { childTaskId: string; payload: { task: { status: string } } };
  assert.equal(child.childTaskId, childTaskId);
  assert.equal(child.payload.task.status, "preview_ready");

  const detachApproval = (await workflow.approveReconciliationDetach(
    `sha256:${childTaskId}`,
    `sha256:${approvalDigest}`,
  )) as { childTaskId: string; approvalDigest: string; payload: { task: { status: string } } };
  assert.equal(detachApproval.childTaskId, childTaskId);
  assert.equal(detachApproval.approvalDigest, approvalDigest);
  assert.equal(detachApproval.payload.task.status, "approved");

  const detachExecution = (await workflow.executeReconciliationDetach(
    `sha256:${childTaskId}`,
  )) as { payload: { task: { status: string } } };
  assert.equal(detachExecution.payload.task.status, "verified");

  const manifest = {
    route: "native_voice_mutation",
    mutations: [{ action: "delete", entityId: "utt-01" }],
  };
  const reExport = (await workflow.dispatchReconciliationReExport(
    `sha256:${childTaskId}`,
    manifest,
  )) as unknown as {
    handle: string;
    downstreamRoute: string;
    digest: string;
    outputTask: string;
    manifest: unknown;
    payload: {
      task: {
        status: string;
        downstreamPreview: {
          route: string;
          preview: { patch: { approvedDigest: null } };
        };
      };
    };
  };
  assert.match(reExport.handle, /^[0-9a-f-]{36}$/i);
  assert.equal(reExport.downstreamRoute, "native_voice_mutation");
  assert.equal(reExport.digest, "e".repeat(64));
  assert.deepEqual(reExport.manifest, manifest);
  assert.equal(reExport.payload.task.status, "preview_ready");
  assert.equal(reExport.payload.task.downstreamPreview.route, "native_voice_mutation");
  assert.equal(
    reExport.payload.task.downstreamPreview.preview.patch.approvedDigest,
    null,
  );
  assert.equal(
    reExport.outputTask,
    path.join(
      stateDirectory,
      `${reExport.handle}.native-voice-mutation.patch.json`,
    ),
  );
  assert.deepEqual(
    JSON.parse(await fs.readFile(reExport.outputTask, "utf8")),
    reExport.payload.task.downstreamPreview.preview,
  );
  assert.notEqual(reExport.digest, approvalDigest);
  const downstreamCommit = (await workflow.commitNativeVoiceMutations(
    reExport.handle,
    reExport.digest,
  )) as {
    patchFile: string;
    digest: string;
    approvedDigestBefore: null;
    revision: number;
  };
  assert.equal(downstreamCommit.patchFile, reExport.outputTask);
  assert.equal(downstreamCommit.digest, reExport.digest);
  assert.equal(downstreamCommit.approvedDigestBefore, null);
  assert.equal(downstreamCommit.revision, 1);

  await assert.rejects(
    () =>
      workflow.dispatchReconciliationReExport(`sha256:${childTaskId}`, {
        route: "unknown",
      }),
    /route must be portable_pair, native_voice_mutation, or native_extension/,
  );

  await assert.rejects(
    () => workflow.executeRender("..\\outside"),
    /Invalid YMM4 render taskId/,
  );
});

test("re-export handles use the existing exporter task file for every route", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "takegraph-re-export-handles-"));
  t.after(async () => fs.rm(root, { recursive: true, force: true }));
  const script = path.join(root, "fake-cli.mjs");
  const stateDirectory = path.join(root, "workflow-state");
  const projectStateRoot = path.join(root, "custom-project-store");
  await fs.writeFile(script, fakeCli, "utf8");
  const workflow = new Ymm4Workflow({
    executable: process.execPath,
    executableArgs: [script],
    stateDirectory,
    projectStateRoot,
    projectOperationRoot: path.join(root, "project-operations"),
  });

  const cases = [
    {
      route: "portable_pair",
      suffix: "patch.json",
      consume: (handle: string, digest: string) => workflow.commit(handle, digest),
    },
    {
      route: "native_voice_mutation",
      suffix: "native-voice-mutation.patch.json",
      consume: (handle: string, digest: string) =>
        workflow.commitNativeVoiceMutations(handle, digest),
    },
    {
      route: "native_extension",
      suffix: "native-extension.task.json",
      consume: (handle: string, digest: string) =>
        workflow.approveNativeExtension(handle, digest),
    },
  ] as const;

  for (const candidate of cases) {
    const result = (await workflow.dispatchReconciliationReExport(childTaskId, {
      route: candidate.route,
    })) as {
      handle: string;
      digest: string;
      downstreamRoute: string;
    };
    const expectedFile = path.join(
      stateDirectory,
      `${result.handle}.${candidate.suffix}`,
    );
    assert.equal(result.downstreamRoute, candidate.route);
    assert.equal(
      (JSON.parse(await fs.readFile(expectedFile, "utf8")) as {
        patch: { approvedDigest: unknown };
      }).patch.approvedDigest,
      null,
    );
    const consumed = (await candidate.consume(result.handle, result.digest)) as {
      patchFile?: string;
      taskFile?: string;
      digest: string;
      approvedDigestBefore: unknown;
      stateRoot: string;
    };
    assert.equal(consumed.patchFile ?? consumed.taskFile, expectedFile);
    assert.equal(consumed.digest, result.digest);
    assert.equal(consumed.approvedDigestBefore, null);
    assert.equal(consumed.stateRoot, projectStateRoot);
  }
});
