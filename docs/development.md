# Development environment

## Toolchain

The repository pins the tools already available in the initialized Windows environment:

| Tool | Version | Purpose |
|---|---:|---|
| Rust | 1.97.1 | Core, service, media node, CLI |
| Node.js | 24.18.0 | MCP App and MCP server |
| pnpm | 11.16.0 | JavaScript workspace and Quint |
| Quint | 0.32.0 | State-machine specification |
| .NET | 10.0.x | Reserved for the future YMM4 bridge |

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

## Behavior changes

When a change affects allowed operation order, concurrency, retry, cancellation, stale results, approval, or commit semantics:

1. update the Quint model;
2. run the model and inspect any counterexample;
3. add or update Rust conformance tests;
4. implement the deterministic transition;
5. run `pnpm verify`.

UI layout, codecs, visual quality, platform APIs, and network wiring remain integration, E2E, or manual boundaries; Quint does not prove them.
