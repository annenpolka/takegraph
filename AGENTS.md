# TakeGraph repository guidance

## Product invariants

- Treat `takegraph-core` as the semantic authority. It must remain deterministic and portable.
- MCP Apps are views and gesture surfaces; they never become the canonical project store.
- AI edits are staged as patches. Never bypass validation, digest-bound approval, or base-revision checks.
- Regenerating speech creates a new `VoiceTake`; it never overwrites an existing audio artifact.
- A completed voice task must match its captured project revision, speech-input hash, and query hash before it can be proposed.
- VOICEVOX ENGINE is connected as an external, user-managed loopback service. Do not bundle or download it automatically.

## Specification workflow

- Change Quint semantics before implementation when behavior, ordering, concurrency, retry, invalidation, or approval rules change.
- Keep implementation-only refactors out of Quint.
- Do not hand-edit files under `generated/`; generation will own them once the runner is implemented.
- Never describe a green Quint model as proof of UI, media, network, or operating-system behavior.

## Agent usage skill

- Any agent operating the TakeGraph MCP tools must follow `.agents/skills/takegraph/SKILL.md`.
- Update that file in the same change when you add or rename a task kind, inspect view, store, digest rule, typical `availableActions` sequence, or a worked payload field.
- Keep live contract wording in `TAKEGRAPH_AGENT_GUIDE`; the skill holds recipes and fail-closed stops, not a second copy of the guide.
- `apps/mcp-server` tests fail if a `TASK_KINDS` value is missing from the skill.

## Boundaries

- `crates/takegraph-core`: domain state and pure transitions.
- `crates/takegraph-service`: project/revision/task/artifact authority and persistence adapters.
- `crates/takegraph-node`: media execution and provider adapters.
- `apps/studio-view`: portable MCP App UI behind `StudioHostBridge`.
- `apps/mcp-server`: MCP tools, resources, and view registration.
- `.agents/skills/takegraph`: agent-facing MCP usage skill (cross-client auto-discovery).
- `specs`: executable state-machine specifications.

