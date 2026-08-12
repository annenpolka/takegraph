# TakeGraph

TakeGraph is an agent-native, voice-first video editor in which scripts are canonical, generated voices are immutable takes, and AI edits are reviewable patches.

This repository is the MVP foundation described in `AI駆動動画編集ツール.md`. It intentionally starts with the smallest product-defining slice:

- a portable Rust domain core for Patch/Revision and VoiceTake state;
- a loopback-only VOICEVOX provider and probe CLI;
- a React MCP App view behind a host adapter;
- a stateful MCP server that serves the Editor view and review tools;
- a Quint model for approval freshness, stale bases, conflicts, and safe commit.

VOICEVOX ENGINE, FFmpeg, YMM4, a full renderer, and a desktop host are not bundled.

## Quick start

Prerequisites are pinned by `.node-version`, `rust-toolchain.toml`, and the root `packageManager` field:

- Node.js 24.18
- pnpm 11.16
- Rust 1.97.1

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

The server exposes five tools. `studio_project_describe` opens the view;
app-only state refresh, voice-candidate generation, patch preview, and exact
digest commit calls then run through the same stdio session. Patch commit is
authorized by `takegraph-core`, not by the React view or TypeScript adapter.

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

## Repository map

```text
apps/
  studio-view/       React + MCP Apps Editor view
  mcp-server/        MCP tools/resources and transports
crates/
  takegraph-core/    deterministic domain model
  takegraph-service/ canonical project-service façade
  takegraph-node/    VOICEVOX and future media adapters
  takegraph-cli/     headless CLI
specs/               Quint semantic specifications
generated/           future immutable verification outputs
toolchain/           semantic toolchain lock
docs/                architecture and development notes
```

See [docs/architecture.md](docs/architecture.md) for boundaries and [docs/development.md](docs/development.md) for the local workflow.

## Current milestone

The MCP Apps interaction slice is operational: a host can open the bundled
Editor, create an immutable query-ready voice candidate, preview adoption of a
completed take, and commit an exact digest-bound patch. The current project
session is intentionally in-memory. VOICEVOX synthesis, persistent
`VoiceProfile`/`VoiceTake`/`AudioQuery` records, and content-addressed WAV
artifacts are the next implementation step; a query-ready take cannot be
adopted before its audio artifact is complete.
