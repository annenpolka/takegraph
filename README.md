# TakeGraph

TakeGraph is an agent-native, voice-first video editor in which scripts are canonical, generated voices are immutable takes, and AI edits are reviewable patches.

This repository is the MVP foundation described in `AI駆動動画編集ツール.md`. It intentionally starts with the smallest product-defining slice:

- a portable Rust domain core for Patch/Revision and VoiceTake state;
- a loopback-only VOICEVOX provider and probe CLI;
- a React MCP App view behind a host adapter;
- an MCP server that serves the Editor view;
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

Build the single-file MCP App and run the MCP server over stdio:

```powershell
pnpm build:view
pnpm build:mcp
pnpm --filter @takegraph/mcp-server start
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

The repository is initialized, not an MVP-complete editor. The next implementation step from the design is to persist `VoiceProfile`, `VoiceTake`, `AudioQuery`, and content-addressed WAV artifacts, then expose one-shot generation through the Script Sheet.

