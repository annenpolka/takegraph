import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { STORE_CANONICAL, STORE_STUDIO, TAKEGRAPH_AGENT_GUIDE } from "./agent-text.js";
import { INSPECT_VIEWS } from "./facade.js";
import { TASK_KINDS, TASK_STORES } from "./task-envelope.js";

const FACADE_TOOLS = [
  "takegraph_inspect",
  "takegraph_task_stage",
  "takegraph_task_approve",
  "takegraph_task_execute",
  "takegraph_task_decide",
] as const;

function readProjectSkill(): string {
  return readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "../../../.agents/skills/takegraph/SKILL.md"),
    "utf8",
  );
}

test("project skill names the live facade tools, stores, kinds, and inspect views", () => {
  const skill = readProjectSkill();

  for (const tool of FACADE_TOOLS) {
    assert.match(skill, new RegExp(`\`${tool}\``), `skill must name ${tool}`);
    assert.match(TAKEGRAPH_AGENT_GUIDE, new RegExp(`- ${tool}:`));
  }

  for (const store of TASK_STORES) {
    assert.match(skill, new RegExp(`\`${store}\``), `skill must name store ${store}`);
  }
  assert.match(skill, new RegExp(STORE_STUDIO));
  assert.match(skill, new RegExp(STORE_CANONICAL));

  for (const kind of TASK_KINDS) {
    assert.match(
      skill,
      new RegExp(`\`${kind}\``),
      `skill must name TASK_KINDS entry ${kind}; update .agents/skills/takegraph/SKILL.md`,
    );
  }

  for (const view of INSPECT_VIEWS) {
    assert.match(skill, new RegExp(`\`${view}\``), `skill must name inspect view ${view}`);
  }

  assert.match(skill, /planDigest/);
  assert.match(skill, /evidenceDigest/);
  assert.match(skill, /availableActions/);
  assert.match(skill, /intent:\s*"revalidate"/);
});
