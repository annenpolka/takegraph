# TakeGraph

TakeGraph is an agent-native, voice-first video editor in which scripts are canonical, generated voices are immutable takes, and AI edits are reviewable patches.

This repository is the MVP foundation described in `AI駆動動画編集ツール.md`. It intentionally starts with the smallest product-defining slice:

- a portable Rust domain core for Patch/Revision and VoiceTake state;
- a loopback-only VOICEVOX provider with content-addressed query/WAV artifacts;
- a token-authenticated YMM4 4 bridge with native `VoiceItem` and portable
  audio/caption realizations;
- a React MCP App view behind a host adapter;
- a stateful MCP server that serves the Editor view and review tools;
- a Quint model for approval freshness, stale bases, conflicts, and safe commit.

VOICEVOX ENGINE, YMM4, FFmpeg, and a full renderer are not bundled. TakeGraph
connects only to already-installed, user-managed VOICEVOX and YMM4 instances.

## Quick start

Prerequisites are pinned by `.node-version`, `rust-toolchain.toml`, and the root `packageManager` field:

- Node.js 24.18
- pnpm 11.16
- Rust 1.97.1
- .NET 10 SDK (YMM4 bridge only)

Install and verify:

```powershell
pnpm install
pnpm verify
```

Run the standalone Editor view:

```powershell
pnpm dev:view
```

Build the Rust guard, single-file MCP App, and MCP server, then run it over stdio:

```powershell
pnpm build
pnpm --filter @takegraph/mcp-server start
```

The server exposes Editor-view tools plus headless YMM4 workflows. Its default
model-facing surface is deliberately limited to five task-oriented tools:
`takegraph_inspect`, `takegraph_task_stage`, `takegraph_task_approve`,
`takegraph_task_execute`, and `takegraph_task_decide`. Studio gesture tools stay
app-only. Every staged operation returns a common task envelope so callers can
follow `taskId`, digests, phase, and `availableActions` instead of selecting
among route-specific tool families.

Project initialization uses that same five-tool surface. Stage
`kind: "project_initialization"` with `mode: "adopt_active"` for an already
saved active YMM4 project, or use `mode: "save_untitled"` with a user-selected
`path`. The path is accepted only while staging and is not repeated in the task
envelope; subsequent approval and execution use the opaque `taskId` and exact
`planDigest`.

Canonical voice creation can be grouped without adding more tool families.
Prefer `kind: "timeline_edit"` with 1–128 ordered `operations` when portable
and native voice creates belong to one logical edit. It produces one sealed
plan, one task, one exact `planDigest`, and one canonical commit. Independent
VOICEVOX artifacts are prepared with bounded parallelism, then restored to
input order before the plan is sealed; YMM4 apply remains serialized and
deterministic. Native voice update/delete and native extensions are not in this
aggregate yet and stay on their separately guarded `native_voice_mutation` and
`native_extension` workflows.

Legacy `kind: "portable_voice"` and `kind: "native_voice"` still accept an
`items` array of 1–128 entries, and their previous flat single-item fields
remain accepted for compatibility.

```json
{
  "kind": "timeline_edit",
  "operations": [
    {
      "op": "portable_voice_create",
      "entityId": "line-001",
      "caption": "表示する字幕",
      "spokenText": "読み上げる文章",
      "speaker": "春日部つむぎ",
      "style": "ノーマル",
      "frame": 0
    },
    {
      "op": "native_voice_create",
      "entityId": "line-002",
      "displayText": "次の台詞",
      "spokenText": "次の台詞",
      "characterName": "春日部つむぎ",
      "frame": 180,
      "layer": 2,
      "maxLength": 300
    }
  ]
}
```

Staging returns one opaque `taskId` and exact `planDigest`; execute that same
task with the exact digest. After a process restart or an uncertain execution
outcome, revalidate the same task ID. Revalidation payload-checks the durable
task file without contacting YMM4 and reports whether exact retry is available;
it does not mistake a merely staged edit for an applied one.

Composition is part of that same read surface, not another tool family. Call
`takegraph_inspect` with `view: "scene"` (or request `include: ["composition"]`)
to observe the active YMM4 scene at its current preview frame. The observation
is read-only, source-bound to project/scene/fingerprint/fps, and reports
unavailable viewport or element geometry explicitly instead of guessing it.
Older bridges safely fall back to timeline-only placement. PNG scene capture
remains the separate review path for visual qualities the structured observer
cannot expose. Display text is exposed only for TakeGraph-owned voice/caption
realizations; arbitrary unmanaged plugin item text and file paths remain opaque.

Set `TAKEGRAPH_LEGACY_TOOLS=1` before starting the MCP server to additionally
register the route-specific compatibility tools described below. For example,
`ymm4_native_voice_stage` previews an explicit `native_voice` realization;
`ymm4_export_stage` selects `portable_pair` and materializes a VOICEVOX take.
Its caption and synthesis `spokenText` remain distinct and are both sealed;
legacy manifests without `spokenText` fall back to the caption. Both return
the sealed target plan and its digest. Neither stage tool changes
the timeline. The corresponding commit tool submits that plan directly through
the unified v2 bridge route,
accepts only its exact digest, applies once, verifies YMM4 read-back, and only
then advances the TakeGraph revision. `ymm4_project_save` is deliberately a
separate operation and only saves to an existing project path.

MCP obtains the active project's canonical revision from
`takegraph ymm4 canonical-head --state-root ...`; it does not own a second
revision mirror. An uninitialized store is reported explicitly with
`initialized: false` and `revision: null`; ordinary workflows cannot create
generation zero as a side effect. It forwards that command's project ID as
`--expected-project-id` to every snapshot-dependent follow-up, so switching the
active YMM4 project between head lookup and execution fails closed. CLI and MCP
share `TAKEGRAPH_PROJECT_STATE_ROOT` and
`TAKEGRAPH_PROJECT_OPERATION_ROOT` overrides. Scene
capture/authenticated-replay/review tools attach bounded,
hash-verified `image/png` content only from receipt-declared CAS paths.

To register a local build with Codex, add this entry to the Codex MCP config
and start a new task so the MCP server list is reloaded:

```toml
[mcp_servers.takegraph]
command = 'C:\Program Files\nodejs\node.exe'
args = [ '<absolute-repository-path>\apps\mcp-server\dist\main.js', '--stdio' ]
```

Probe an already-running VOICEVOX ENGINE:

```powershell
cargo run -p takegraph-cli -- voicevox probe
```

Override its loopback endpoint with `TAKEGRAPH_VOICEVOX_ENDPOINT` or `--endpoint`.

Build the YMM4 bridge against an installed YMM4 directory:

```powershell
$env:YMM4_PATH = "C:\path\to\YMM4"
dotnet build bridges\ymm4\TakeGraph.Ymm4Bridge\TakeGraph.Ymm4Bridge.csproj -c Release
```

`pnpm test:bridge` runs the bridge's pure contract/recovery/digest tests with an
explicit CI-only `IToolPlugin` stub, so YMM4 binaries are not redistributed.
Production and installer builds continue to require and reference the real YMM4
plugin DLL.

Install the resulting DLL under
`<YMM4>\user\plugin\TakeGraph.Ymm4Bridge\` and restart YMM4. The plugin writes
a random local token to `%LOCALAPPDATA%\TakeGraph\ymm4-bridge.json`; the CLI
and MCP workflow read that file by default. Do not commit or share it.

For day-to-day local updates, the wrapper performs a normal application close,
builds and hash-verifies the production bridge, installs it with a recoverable
backup, restarts YMM4, and waits for authenticated bridge health. When no
`-Ymm4Arguments` are supplied, it also reopens the named project that was
active before shutdown and verifies the restored path. It never force-stops
YMM4, so an unresolved save confirmation fails safely:

```powershell
.\scripts\Update-Ymm4Bridge.ps1 -Ymm4Path "C:\path\to\YMM4"
```

With VOICEVOX ENGINE and YMM4 running:

```powershell
cargo run -p takegraph-cli -- ymm4 health
cargo run -p takegraph-cli -- ymm4 snapshot
cargo run -p takegraph-cli -- voicevox materialize --text "ここからだいにけいたいだぜ"
```

## Repository map

```text
apps/
  studio-view/       React + MCP Apps Editor view
  mcp-server/        MCP tools/resources and transports
crates/
  takegraph-core/    deterministic domain model
  takegraph-service/ canonical project-service façade
  takegraph-node/    VOICEVOX, YMM4 protocol, scene/media adapters
  takegraph-cli/     headless CLI
bridges/
  ymm4/              .NET 10 YMM4 plugin and private loopback API
specs/               Quint semantic specifications
generated/           future immutable verification outputs
toolchain/           semantic toolchain lock
docs/                architecture and development notes
```

See [docs/architecture.md](docs/architecture.md) for boundaries and [docs/development.md](docs/development.md) for the local workflow.

## Current milestone

The YMM4 managed-subset implementation now covers native `VoiceItem`
create/update/delete plus exact WAV/provenance capture, the portable
VOICEVOX audio/caption fallback, scene-frame capture and review, typed native
portrait/media/effect/template extensions, verified save checkpoints, and the
safe render/reconciliation task contracts. Mutations are digest- and
target-bound, journaled before changing YMM4, checked for stale state, and
verified by semantic read-back before canonical revision advancement. The
service publishes receipt evidence, managed target projections, target links,
and revision together in an append-only hash-chained project store.

The implementation intentionally fails closed when the installed YMM4 driver
cannot prove a capability. For YMM4 4.55.1.1 it binds the active command-line
writer, resolved settings, binaries, and independently probed H.264/AAC-LC MP4
evidence. The editor does not, however, expose a proven exhaustive manifest of
every project asset, font, voice engine, tachie/effect input, and plugin setting
that the child encoder may read. Consequently the current driver returns an
unbound render profile and does not advertise `project.render`; the writer,
media-probe, candidate-publication, and recovery code remains gated until that
source closure can be SHA-256-bound and leased. See
[`docs/ymm4-native-integration.md`](docs/ymm4-native-integration.md) for the
boundary, completed phases, and remaining live/interop gaps.
