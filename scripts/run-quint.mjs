import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const protocolDirectory = path.join(root, "specs", "protocols");
const quintCli = path.join(
  root,
  "node_modules",
  "@informalsystems",
  "quint",
  "dist",
  "src",
  "cli.js",
);
const evaluatorVersion = "v0.6.0";
const defaultSamples = 1000;

/**
 * Quint exploration and tests that `pnpm spec:run` must keep in lockstep with
 * the checked-in protocol files. Typecheck covers every file; run/test matches
 * the previous package.json command set.
 */
const protocols = [
  {
    file: "specs/protocols/patch_protocol.qnt",
    group: "core",
    runs: [{ invariant: "safety" }],
  },
  {
    file: "specs/protocols/ymm4_apply_protocol.qnt",
    group: "core",
    runs: [
      { main: "ymm4ApplyProtocol", invariant: "safety" },
      { main: "ymm4LifecycleProtocol", invariant: "lifecycleSafety" },
      { main: "ymm4ReconciliationProtocol", invariant: "reconciliationSafety" },
      {
        main: "ymm4ExternalMutationFenceProtocol",
        invariant: "externalMutationFenceSafety",
      },
    ],
  },
  {
    file: "specs/protocols/ymm4_project_initialization_protocol.qnt",
    group: "core",
    runs: [
      {
        main: "ymm4ProjectInitializationProtocol",
        invariant: "initializationSafety",
        test: true,
      },
    ],
  },
  {
    file: "specs/protocols/agent_report_protocol.qnt",
    group: "core",
    runs: [{ main: "agentReportProtocol", invariant: "safety" }],
  },
  {
    file: "specs/protocols/annotation_promotion_protocol.qnt",
    group: "core",
    runs: [
      {
        main: "annotationPromotionProtocol",
        invariant: "safety",
        test: true,
      },
    ],
  },
  {
    file: "specs/protocols/ymm4_change_set_protocol.qnt",
    group: "change-set",
    runs: [
      {
        main: "ymm4ChangeSetProtocol",
        invariant: "changeSetSafety",
        test: true,
      },
    ],
  },
  {
    file: "specs/protocols/ymm4_edit_surface_protocol.qnt",
    group: "edit-contracts",
    runs: [
      {
        main: "ymm4EditSurfaceProtocol",
        invariant: "editSurfaceSafety",
        verbosity: 1,
        test: true,
      },
    ],
  },
  {
    file: "specs/protocols/ymm4_composition_graph_protocol.qnt",
    group: "edit-contracts",
    runs: [
      {
        main: "ymm4CompositionGraphProtocol",
        invariant: "compositionGraphSafety",
        verbosity: 1,
        test: true,
      },
    ],
  },
  {
    file: "specs/protocols/ymm4_project_edit_protocol.qnt",
    group: "edit-contracts",
    runs: [
      {
        main: "ymm4ProjectEditProtocol",
        invariant: "projectEditSafety",
        verbosity: 1,
        test: true,
      },
    ],
  },
  {
    file: "specs/protocols/ymm4_edit_transaction_protocol.qnt",
    group: "edit-contracts",
    runs: [
      {
        main: "ymm4EditTransactionProtocol",
        invariant: "editTransactionSafety",
        verbosity: 1,
        test: true,
      },
    ],
  },
  {
    file: "specs/protocols/ymm4_native_voice_text_protocol.qnt",
    group: "edit-contracts",
    runs: [
      {
        main: "ymm4NativeVoiceTextProtocol",
        invariant: "nativeVoiceTextSafety",
        verbosity: 1,
        test: true,
      },
    ],
  },
];

function parseArgs(argv) {
  const mode = argv[0];
  if (mode !== "typecheck" && mode !== "run") {
    throw new Error("usage: node scripts/run-quint.mjs typecheck|run [--group=NAME]");
  }

  let group = "all";
  for (const argument of argv.slice(1)) {
    if (argument.startsWith("--group=")) {
      group = argument.slice("--group=".length);
      continue;
    }
    throw new Error(`unknown argument: ${argument}`);
  }

  if (!["all", "core", "change-set", "edit-contracts"].includes(group)) {
    throw new Error(`unknown group: ${group}`);
  }

  return { mode, group };
}

function selectedProtocols(group) {
  if (group === "all") {
    return protocols;
  }
  return protocols.filter((protocol) => protocol.group === group);
}

function assertCatalogMatchesDisk() {
  const listed = new Set(
    fs
      .readdirSync(protocolDirectory)
      .filter((name) => name.endsWith(".qnt"))
      .sort(),
  );
  const catalogued = new Set(
    protocols.map((protocol) => path.basename(protocol.file)).sort(),
  );

  for (const name of listed) {
    if (!catalogued.has(name)) {
      throw new Error(
        `${name} exists under specs/protocols but is missing from scripts/run-quint.mjs`,
      );
    }
  }
  for (const name of catalogued) {
    if (!listed.has(name)) {
      throw new Error(`${name} is catalogued in scripts/run-quint.mjs but is not on disk`);
    }
  }
}

function typecheckTasks(selected) {
  return selected.map((protocol) => ({
    label: `typecheck ${protocol.file}`,
    args: ["typecheck", protocol.file],
  }));
}

function runTasks(selected, samples) {
  const tasks = [];
  for (const protocol of selected) {
    for (const run of protocol.runs) {
      const runArgs = ["run", "--max-samples", String(samples)];
      if (run.main) {
        runArgs.push("--main", run.main);
      }
      if (run.invariant) {
        runArgs.push("--invariant", run.invariant);
      }
      if (run.verbosity !== undefined) {
        runArgs.push("--verbosity", String(run.verbosity));
      }
      runArgs.push(protocol.file);
      tasks.push({
        label: `run ${run.main ?? path.basename(protocol.file, ".qnt")}`,
        args: runArgs,
      });

      if (run.test) {
        const testArgs = ["test", "--max-samples", String(samples)];
        if (run.main) {
          testArgs.push("--main", run.main);
        }
        testArgs.push(protocol.file);
        tasks.push({
          label: `test ${run.main ?? path.basename(protocol.file, ".qnt")}`,
          args: testArgs,
        });
      }
    }
  }
  return tasks;
}

function evaluatorInstalled() {
  const home = process.env.QUINT_HOME ?? path.join(os.homedir(), ".quint");
  const executable =
    process.platform === "win32" ? "quint_evaluator.exe" : "quint_evaluator";
  return fs.existsSync(path.join(home, `rust-evaluator-${evaluatorVersion}`, executable));
}

function runTask(task) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [quintCli, ...task.args], {
      cwd: root,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => {
      output += chunk;
    });
    child.stderr.on("data", (chunk) => {
      output += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => {
      const header = `\n=== ${task.label} ===\n`;
      process.stdout.write(header + output);
      if (code === 0) {
        resolve();
        return;
      }
      reject(new Error(`${task.label} failed with exit ${code ?? "null"}`));
    });
  });
}

async function runPool(tasks, concurrency) {
  const pending = [...tasks];
  const failures = [];

  async function worker() {
    while (pending.length > 0) {
      const task = pending.shift();
      if (!task) {
        return;
      }
      try {
        await runTask(task);
      } catch (error) {
        failures.push(error);
      }
    }
  }

  const workerCount = Math.max(1, Math.min(concurrency, tasks.length));
  await Promise.all(Array.from({ length: workerCount }, () => worker()));
  return failures;
}

function sampleCount() {
  const raw = process.env.QUINT_MAX_SAMPLES;
  if (raw === undefined || raw === "") {
    return defaultSamples;
  }
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`QUINT_MAX_SAMPLES must be a positive integer, got ${raw}`);
  }
  return parsed;
}

function jobCount() {
  const raw = process.env.QUINT_JOBS;
  if (raw === undefined || raw === "") {
    return Math.max(1, Math.min(os.availableParallelism(), 4));
  }
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`QUINT_JOBS must be a positive integer, got ${raw}`);
  }
  return parsed;
}

const { mode, group } = parseArgs(process.argv.slice(2));
assertCatalogMatchesDisk();

if (!fs.existsSync(quintCli)) {
  throw new Error("Quint CLI is missing. Run pnpm install first.");
}

const selected = selectedProtocols(group);
if (selected.length === 0) {
  throw new Error(`no protocols in group ${group}`);
}

const tasks =
  mode === "typecheck" ? typecheckTasks(selected) : runTasks(selected, sampleCount());
const concurrency = jobCount();

let warmup = [];
let remaining = tasks;
if (mode === "run" && !evaluatorInstalled() && tasks.length > 0) {
  warmup = [tasks[0]];
  remaining = tasks.slice(1);
}

const failures = [];
if (warmup.length > 0) {
  process.stdout.write("Downloading the Quint rust evaluator before parallel runs.\n");
  failures.push(...(await runPool(warmup, 1)));
}
failures.push(...(await runPool(remaining, concurrency)));

if (failures.length > 0) {
  for (const failure of failures) {
    process.stderr.write(`${failure instanceof Error ? failure.message : failure}\n`);
  }
  process.exitCode = 1;
}
