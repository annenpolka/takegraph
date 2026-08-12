# Development environment

## Toolchain

The repository pins the tools already available in the initialized Windows environment:

| Tool | Version | Purpose |
|---|---:|---|
| Rust | 1.97.1 | Core, service, media node, CLI |
| Node.js | 24.18.0 | MCP App and MCP server |
| pnpm | 11.16.0 | JavaScript workspace and Quint |
| Quint | 0.32.0 | State-machine specification |
| .NET | 10.0.x | YMM4 4 bridge plugin |

FFmpeg and Python are not prerequisites for the initialized slice. Add them when reference rendering and analysis workers begin; do not make them implicit global dependencies before then.

## Commands

```powershell
pnpm check          # TypeScript + rustfmt + clippy
pnpm test           # TypeScript compilation checks + Rust tests
pnpm spec:typecheck # Quint type/effect check
pnpm spec:run       # Random exploration of patch invariants
pnpm verify         # All of the above
```

For the UI, `pnpm dev:view` opens a standalone adapter with sample data.
Embedded hosts use the same UI through `McpAppsHostBridge`; its mutations call
MCP tools and never write canonical project state directly. `pnpm test` builds
the Rust patch guard and production single-file view before exercising both an
in-memory MCP transport and the built stdio process.

For VOICEVOX, start an engine separately and run:

```powershell
$env:TAKEGRAPH_VOICEVOX_ENDPOINT = "http://127.0.0.1:50021"
cargo run -p takegraph-cli -- voicevox probe
```

Only loopback endpoints are accepted in the MVP.

## YMM4 bridge development

Set `YMM4_PATH` to the installed directory containing
`YukkuriMovieMaker.Plugin.dll`, then build:

```powershell
$env:YMM4_PATH = "C:\path\to\YMM4"
dotnet build bridges\ymm4\TakeGraph.Ymm4Bridge\TakeGraph.Ymm4Bridge.csproj -c Release
```

The normal build above always compiles against YMM4's real plugin contract.
Repository and CI verification can run the reflection-bound bridge tests without
redistributing YMM4 by using the explicit contract-only stub:

```powershell
pnpm test:bridge
```

That stub exists only to compile and execute the pure boundary tests; never copy
its output into YMM4. The installer always performs a normal real-contract build.

Copy only the built `TakeGraph.Ymm4Bridge.dll` to
`<YMM4>\user\plugin\TakeGraph.Ymm4Bridge\` while YMM4 is stopped, then restart
YMM4. Validate the private protocol before exporting:

```powershell
cargo run -p takegraph-cli -- ymm4 health
cargo run -p takegraph-cli -- ymm4 capabilities
cargo run -p takegraph-cli -- ymm4 controls
cargo run -p takegraph-cli -- ymm4 snapshot
```

The checked-in installer refuses to run while either YMM4 process is alive,
hash-verifies an incoming copy, and preserves the previous DLL under
`<YMM4>\user\TakeGraph\plugin-backups\TakeGraph.Ymm4Bridge` with a non-DLL
suffix. This location is outside YMM4's recursive `user\plugin` discovery
tree; the installer also migrates backups made by older versions out of that
tree before installation:

```powershell
.\scripts\Install-Ymm4Bridge.ps1 -Ymm4Path "C:\path\to\YMM4"
```

After restarting YMM4 with a named project open, run the read-only driver smoke
and recovery check. Scene capture is opt-in because it temporarily moves YMM4's
preview state; the bridge must prove that it restored that state and did not
change the dirty flag before the script accepts the evidence:

```powershell
.\scripts\Test-Ymm4BridgeLive.ps1 -MinimumManagedItems 1
.\scripts\Test-Ymm4BridgeLive.ps1 -RunSceneCapture `
  -Frame 0,120,240 -ExpectedWidth 1920 -ExpectedHeight 1080
# Review the returned task, frames, dimensions, and exact digest, then:
.\scripts\Test-Ymm4BridgeLive.ps1 -RunSceneCapture `
  -SceneTaskPath <returned-task-path> -ApprovedSceneDigest <exact-digest>
```

Each run writes a token-free report and any captured content-addressed images
below `.takegraph\live-ymm4`. The script requires the native voice, scene, and
native-extension capability set. It also verifies the current authoritative
render safety boundary: no render capability may be advertised and the active
profile must be unbindable with an explicit binding error. Depending on how far
runtime discovery gets, that error may identify unavailable YMM4 settings or the
unproven exhaustive dependency manifest. It checks that recovery is clear, and
uses authenticated replay for scene evidence. It
never self-approves a capture: the first scene command exits with
`approval_required`, and only the exact persisted digest can resume it.
The live script pins `canonical-head` to the repository's
`.takegraph\project-store` regardless of the caller's working directory; pass
`-ProjectStateRoot` only when intentionally testing another canonical store.

The bridge binds only `127.0.0.1:8766`, requires the token in
`%LOCALAPPDATA%\TakeGraph\ymm4-bridge.json`, sends no CORS headers, and rejects
non-versioned routes. Operation receipts persist under `%LOCALAPPDATA%` so a
retry after a caller crash is idempotent.

The CLI export lifecycle is intentionally two-phase:

```powershell
cargo run -p takegraph-cli -- ymm4 export-stage --manifest utterances.json --patch export.json --head 0
cargo run -p takegraph-cli -- ymm4 export-commit --patch export.json --digest <exact-stage-digest> --head 0
cargo run -p takegraph-cli -- ymm4 export-verify --patch export.json
cargo run -p takegraph-cli -- ymm4 native-voice-stage --manifest voice.json --patch voice-export.json --head 0
cargo run -p takegraph-cli -- ymm4 native-voice-commit --patch voice-export.json --digest <exact-stage-digest> --head 0
cargo run -p takegraph-cli -- ymm4 native-voice-verify --patch voice-export.json
cargo run -p takegraph-cli -- ymm4 save
```

Stage and commit use the service-owned canonical project store at
`.takegraph/project-store` by default. Override the shared root with
`--state-root <path>` or `TAKEGRAPH_PROJECT_STATE_ROOT`. An existing project's
first stage may bootstrap from the supplied `--head`; after that, `--head` must
match the durable canonical head. The MCP server uses the same environment
override in production, and likewise shares `TAKEGRAPH_PROJECT_OPERATION_ROOT`
with the CLI. After `canonical-head`, MCP passes the returned project ID as
`--expected-project-id`; a project-tab switch before the follow-up snapshot is
rejected before store bootstrap or mutation. A verified external receipt, target link, and
new revision are published together in an append-only hash-chained generation,
so exact operation replay is idempotent and corrupt historical state fails
closed.

Stage commands do not mutate YMM4. Commit commands persist approval before
external I/O; the bridge journals the request before YMM4 mutation and
persists a read-back receipt before finalization. The `native-voice-*`
creation workflow is intentionally create-only and requires identical
display/spoken text; use `native-voice-mutation-*` for approved
create/update/delete batches and exact artifact export. Both paths treat
`maxLength` as an approved rollback boundary. `save` never performs Save As;
open or create a named project in YMM4 first.

For restart testing, call `ymm4 save` and then `ymm4 close`. Never use process
termination commands: YMM4 treats them as an abnormal exit and opens recovery
UI on the next launch.

## Behavior changes

When a change affects allowed operation order, concurrency, retry, cancellation, stale results, approval, or commit semantics:

1. update the Quint model;
2. run the model and inspect any counterexample;
3. add or update Rust conformance tests;
4. implement the deterministic transition;
5. run `pnpm verify`.

UI layout, codecs, visual quality, platform APIs, and network wiring remain integration, E2E, or manual boundaries; Quint does not prove them.
