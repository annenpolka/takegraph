# TakeGraph YMM4 Bridge

This .NET 10 WPF plugin is TakeGraph's private platform adapter for YMM4 4. It
is not an MCP server and does not expose general reflection or arbitrary YMM4
commands.

The bridge listens on `http://127.0.0.1:8766/`, requires the random token from
`%LOCALAPPDATA%\TakeGraph\ymm4-bridge.json`, and owns only TakeGraph-marked
realizations. The portable compatibility path uses audio/caption pairs; the
protocol v2 path creates native `VoiceItem` instances. Current unmanaged YMM4
items participate in the project fingerprint but are never rewritten.

## Build and install

```powershell
$env:YMM4_PATH = "C:\path\to\YMM4"
dotnet build .\TakeGraph.Ymm4Bridge\TakeGraph.Ymm4Bridge.csproj -c Release
```

Stop YMM4, copy
`TakeGraph.Ymm4Bridge\bin\Release\net10.0-windows\TakeGraph.Ymm4Bridge.dll`
to `<YMM4>\user\plugin\TakeGraph.Ymm4Bridge\`, and restart YMM4.

The repository installer keeps verified prior DLLs under
`user\TakeGraph\plugin-backups\TakeGraph.Ymm4Bridge` with a `.backup` suffix,
outside YMM4's recursive `user\plugin` discovery tree. It also migrates backups
created by older TakeGraph installers out of that tree before installation.

## Stable protocol surface

- `GET /v1/health`
- `GET /v1/capabilities`
- `GET /v2/descriptors`
- `GET /v2/recovery`
- `GET /v1/project/snapshot`
- `GET /v1/project/controls`
- `POST /v1/project/save`
- `POST /v1/project/undo`
- `POST /v1/project/redo`
- `POST /v1/application/close`
- `POST /v1/managed/plan`
- `POST /v1/managed/apply`
- `POST /v2/target-plan/validate`
- `POST /v2/target-plan/apply`
- `POST /v2/target-plan/not-started`
- `POST /v2/native-voice/plan`
- `POST /v2/native-voice/apply`
- `POST /v2/native-voice/mutation/plan`
- `POST /v2/native-voice/mutation/apply`
- `POST /v2/native-voice/mutation/not-started`
- `POST /v2/native-voice/artifacts`
- `POST /v2/native-extension/plan`
- `POST /v2/native-extension/apply`
- `POST /v2/native-extension/not-started`
- `GET /v2/native-extension/operations/{operationId}`
- `POST /v2/reconciliation/detach`
- `POST /v2/reconciliation/detach/not-started`
- `GET /v2/reconciliation/detach/{operationId}`
- `POST /v2/scene/capture`
- `GET /v2/project/checkpoint-profile`
- `POST /v2/project/checkpoints`
- `GET /v2/project/checkpoints/{operationId}`
- `GET /v2/render/profiles`
- `POST /v2/render/tasks`
- `GET /v2/render/tasks/{taskId}`
- `POST /v2/render/tasks/{taskId}/cancel`
- `GET /v1/operations/{operationId}`

Every route requires `x-takegraph-token`. Apply requests are bound to the
operation ID, payload digest, target project/scene, and exact preview
fingerprint. The bridge persists an `applying` journal before mutation, then
persists a verified read-back receipt before the caller may finalize its
TakeGraph revision. Ambiguous restart state is returned as
`recovery_required`, never applied again automatically.
Receipts loaded from an editable export patch are not trusted on their own;
the service replays the operation through this authenticated bridge before
finalization.

For the supported 4.55.1.1 render path, the driver code reflects the
same `CommandLineEncoder.ResolvePlugin()` selection used by the child encoder.
Its writer digest covers selector/writer JSON,
resolved command lines, YMM/writer/FFmpeg binaries and libraries, dimensions,
frame rate, H.264, AAC-LC at 48 kHz, MP4, and yuv420p. Inputs reject reparse
components and remain read-leased. Output is encoded beneath a task-private
same-volume directory, independently probed, and only then atomically published
through the overwrite WAL. This machinery is currently gated: YMM4 4.55.1.1
does not expose a proven exhaustive manifest of every project asset, font,
voice/tachie/effect input, and plugin setting consumed by the child encoder.
`GET /v2/render/profiles` therefore returns an unbindable profile and the bridge
does not advertise render capabilities. Enabling them requires SHA-256 binding,
no-follow identity checks, whole-child read leases, and post-exit verification
for that complete dependency closure. Unknown or drifting inputs stay
fail-closed.

Metadata detach is a separate protocol-v2 write. It is project/scene/entity/
realization scoped and records every scene Remark preimage, stable item witness,
and the exact expected post-Remark set in a dedicated atomic WAL before mutation.
Duplicate or ambiguous witnesses fail closed. It removes only the selected
TakeGraph marker, then verifies by fresh read-back that the marker is absent,
all unrelated Remarks exactly match their preimages, and a whole-scene
non-Remark content digest is unchanged. The same exact checks apply during
restart recovery. Exact retries replay the durable receipt; operation-ID rebinding is
rejected, and ambiguous restart state closes the normal recovery gate. This route
can remove one effect identity while preserving sibling effect markers. It fails
closed when asked to remove a base native-extension identity whose same marker
still owns effects, because that requires a separately approved batch detach.
This route
does not edit the legacy portable caption marker embedded in rendered text;
portable-pair detach/migration remains unsupported and fails closed.

The caller seals the complete normalized capability digest and the exact
`managedIdentity.detach` feature version/schema/properties in reconciliation
report schema v2. Before this route is called, the service has already persisted
the exact request and published a canonical schema-v3 pending reservation while
holding the project external-mutation fence. A verified receipt consumes that
reservation; only an exact before-state rollback proof may release it without a
canonical commit.

The normal cue workflow uses the unified target-plan routes. Validation and
apply accept a canonical sealed plan containing an explicit `native_voice` or
`portable_pair` strategy, resolved placement, resolved character/artifact
binding, structured capability dependencies, target/conflict scope, ownership,
duration, and change budget. The bridge rehashes every binding and rejects raw
placement overrides or mismatched strategy payloads; it never re-resolves
frame/layer from caller fields outside the plan. The physical v1 managed and v2
native-voice routes remain compatibility surfaces.

For `portable_pair`, display caption and synthesis `spokenText` may differ. The
sealed plan binds both while the physical projection remains caption plus the
already-materialized, hash-bound WAV. Legacy physical JSON omitting
`spokenText` treats it as equal to the caption and retains its historical
request digest; a distinct value is included in the request digest.

The native voice routes invoke YMM4's `MainModel.AddVoiceItemAsync`, store
TakeGraph identity in the non-rendered `Remark`, and write `displayText` to
`VoiceItem.Serif`. `spokenText` is optional: when present it is written to
`VoiceItem.Hatsuon` and read back exactly; when omitted, YMM4 derives Hatsuon
and the bridge only requires that a non-empty pronunciation exists. Distinct
bound display/spoken values require the advertised
`voiceItem.create/update.separateDisplayAndSpokenText` property. The mutation
routes support create, replacement-update with an internal preserved-state
digest, and delete with absence read-back. Artifact export produces an exact
YMM4 WAV plus normalized host-bound voice-state provenance; the JSON is not a
portable synthesis query.

`GET /v1/project/snapshot` also returns `nativeExtensions`, a fresh managed-only
projection used for reconciliation. It contains only TakeGraph-owned identity,
placement, descriptor, immutable artifact, typed-effect, and template-count
fields. Preserved host fields and unknown effects are intentionally absent.

Use `takegraph ymm4 save` followed by `takegraph ymm4 close` for automated
restart tests. Do not terminate the YMM4 process directly; forced termination
is recorded as an abnormal exit and triggers YMM4 recovery on the next launch.
