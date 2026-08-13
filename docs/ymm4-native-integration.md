# YMM4 Native Integration Boundary

Status: accepted boundary. Phase 0 safety/persistence and public Phase 1-5 code
vertical slices are implemented as of 2026-08-12, including the unified target
plan, Remark-only detach, and preview-only reconciliation exporter dispatch.
The supported live-YMM4 driver matrix has not yet been completed.

This document defines how TakeGraph should use YMM4-native features without
making YMM4 the canonical TakeGraph project store. It extends the general
[architecture](architecture.md) and the current
[YMM4 bridge protocol](../bridges/ymm4/README.md). The current audio/caption
pair exporter remains the portable compatibility path while this design is
introduced incrementally.

## Implemented slice

The first implementation deliberately stops short of general native editing:

- protocol version 2 binds apply receipts to operation ID, canonical request
  digest, project, scene, and precondition fingerprint;
- approve/apply/finalize rehash the editable export payload, and finalization
  requires replay through the token-authenticated bridge instead of trusting a
  receipt loaded from disk;
- the bridge durably records `applying` before mutation, performs best-effort
  compensating rollback, recovers exact before/after states by read-back, and
  returns `recovery_required` for ambiguous partial state;
- YMM4 4.55.1.1 is the only mutation-enabled driver profile; other versions
  remain observation-only, and native voice capabilities require runtime API
  probes;
- CLI and MCP expose digest-bound stage/commit/verify workflows for native
  `VoiceItem` creation and for batched create/update/delete. Mutation commit
  requires an authenticated idempotent replay, receipt read-back, a fresh live
  snapshot, and durable canonical-revision CAS;
- a post-commit artifact tool replay-authenticates the mutation, asks YMM4 for
  the exact generated WAV plus normalized host-bound voice-state provenance,
  then path-bounds, hashes, parses, and imports both into TakeGraph-owned CAS.
  The provenance JSON is not advertised as a portable synthesis query;
- the Rust core now defines one portable `ManagedCueIntent`, sealed
  `TargetPlan`, ownership mask, scoped fingerprint set, change budget, and
  normalized `NativeRealization` contract shared by native voice and the
  portable pair compatibility path;
- `/v2/target-plan/validate` and `/v2/target-plan/apply` now carry that sealed
  plan directly across the bridge boundary. `native_voice` and `portable_pair`
  are explicit strategies; the bridge consumes approved resolved placement,
  artifact/character bindings, duration, ownership, capability schemas, scope,
  and budget without re-resolving frame, layer, or character from external
  caller values. The older physical routes remain compatibility APIs;
- bridge health and legacy string features are normalized into structured,
  schema-digested capabilities. Driver, feature-schema, and capability changes
  alter the target-plan approval digest and are checked again before apply;
- `takegraph-service` persists an append-only hash chain containing the
  canonical revision, verified external-operation evidence, and target links.
  CLI stage/commit paths use this store and finalize receipt plus revision
  idempotently in one published generation. MCP reads the active project's
  head through `ymm4 canonical-head`; it has no separate `revision.json` mirror;
- native identity uses `VoiceItem.Remark`, one item is verified per
  realization, and actual length must remain within the approved `maxLength`;
- the initial native slice requires identical display/spoken text and exact
  character name. Other cases must use the portable realization explicitly;
- the Quint model and suite now cover request binding, WAL-before-mutation,
  partial apply, crash/restart recovery, rollback, at-most-once apply, and
  capability invalidation.

The bridge now stores a durable before-state preimage and retries or rolls back
from it during recovery. A partial state that cannot be proven equal to the
approved before or after state still fails closed as `recovery_required` rather
than guessing.

## Decision summary

TakeGraph owns editing meaning, dependencies, revision history, approval, and
artifact provenance. YMM4 owns platform-specific realization: native item
construction, configured character behavior, concrete timeline mutation,
preview/render execution, and project controls.

The integration unit changes from a physical `AudioItem + TextItem` pair to a
semantic cue that is planned and then realized for a particular YMM4 runtime:

```text
ManagedCueIntent (portable meaning)
  -> TargetPlan (approved YMM4 realization)
  -> NativeRealization (read-back YMM4 result)
```

Audio and caption remain separate dependency nodes inside TakeGraph. A caption
style change must not invalidate audio, and a spoken-text change must not
silently rewrite display text. In YMM4, however, an ordinary spoken cue should
normally be one native `VoiceItem`, not two unrelated timeline items.

Full bidirectional synchronization is not a goal. TakeGraph manages only items
and fields that it explicitly owns. YMM4-local state outside that ownership
boundary is preserved, observed for conflicts where relevant, and never
silently imported into the canonical project.

## Authority and responsibility boundary

| Concern | `takegraph-core` | `takegraph-service` | `takegraph-node` | YMM4 bridge / YMM4 |
| --- | --- | --- | --- | --- |
| Cue semantics | Display text, spoken text, segmentation, locks, role references | Persistence and query APIs | None | Resolve to supported native fields |
| Voice takes | Lifecycle, accepted take, freshness rules | Task identity, provenance, artifact records | Provider calls and media inspection | Native synthesis and `VoiceItem` state when selected |
| Character styling | Portable role and binding intent, not YMM property graphs | Store target binding and expected digest | Typed capability client | Resolve YMM character, voice, caption, and portrait settings |
| Timeline | Anchors, ordering, ripple policy, change budget | Resolve dependencies and seal concrete placement in target plans | Typed sealed-plan transport | Validate approved placement and perform concrete mutation without re-resolution |
| Assets | Content identity and semantic use | Content-addressed metadata and availability | Hash, materialize, transfer, probe | Import, decode, cache, and use native media items |
| Effects | Portable typed effects only | Store target-extension intent | Capability/schema transport | Apply allowlisted YMM effects and return normalized read-back |
| Templates | Portable macros | Pin YMM template reference and digest | Capability/schema transport | Resolve and instantiate YMM templates |
| Approval | Pure patch guards | Canonical digest, base revision, durable approval | None | Execute only the approved target plan |
| Undo/redo | Inverse semantic patch | Canonical revision history | None | Native transaction/undo group for atomic apply and rollback |
| Save/render | Output intent and policy | Permission, task, checkpoint, artifact receipt | Task transport and media verification | Native save/render execution and progress |
| Recovery | Legal state transitions | Cross-system operation saga | Retry and polling | Write-ahead mutation journal and post-crash read-back |

The bridge may use reflection internally as a version-compatibility mechanism,
but it must expose only a small typed protocol. General reflection, arbitrary
property mutation, and arbitrary YMM command execution are outside the product
boundary.

## API tiers

Each bridge feature belongs to an explicit risk tier. A higher tier never
inherits authorization from a lower tier.

### Tier 0: observation

Read-only and safe to call without a mutation approval:

- health, protocol range, plugin/YMM4 versions;
- structured runtime capabilities and their digest;
- active project/scene identity;
- scoped timeline snapshot and managed realization read-back;
- character, template, and effect descriptors;
- project-control availability and render-profile discovery.

Observation must not lazily generate media, modify selection, seek playback, or
alter project dirty state. If a YMM API cannot satisfy that rule, the feature is
not Tier 0.

### Tier 1: managed project mutation

Digest-approved, scoped, idempotent writes to TakeGraph-owned entities:

- create/update/delete a managed cue;
- create a native voice, standalone caption, image/audio clip, portrait, or
  face item;
- move or resize managed realizations according to an approved placement plan;
- apply a known target template at creation time;
- add/update/remove an allowlisted effect instance owned by TakeGraph.

Tier 1 requests require a base TakeGraph revision, target identity, expected
scope fingerprint, exact target-plan digest, stable operation ID, and change
budget. They may not touch unmanaged items or global YMM settings.

### Tier 2: project controls and external outputs

Separately approved operations with broader or externally visible effects:

- save the existing project path;
- capture one or more YMM-rendered scene frames into a service-owned inspection
  task;
- render/export to an explicitly authorized output path;
- cancel or resume a render task;
- explicitly bind or rebind a YMM character/template;
- reconcile a detected YMM edit into a proposed TakeGraph patch.

`Save As`, overwrite, global character mutation, output-profile mutation, and
deletion of unmanaged content require dedicated contracts if they are added at
all. They are not implied by a Tier 1 cue approval.

### Excluded surface

The bridge must not expose arbitrary reflection, arbitrary command names,
unbounded property setters, process termination, or raw delete-by-frame/layer
operations. These are useful diagnostics during adapter development but are
not stable product APIs.

## Logical cue and realization modes

A `ManagedCueIntent` is a portable aggregate. At minimum it contains:

- stable entity ID and entity revision;
- display text and spoken text;
- speaker role / voice profile reference;
- caption style reference and segmentation locks;
- timing anchor, ordering policy, and track role;
- accepted portable take, native-voice preference, or explicit fallback policy;
- managed effects/template references;
- change budget and hard-lock preconditions.

YMM4 planning chooses one of two explicit voice realization modes.

### `native_voice`

The bridge resolves a pinned YMM character binding and invokes the supported
native voice creation path. YMM4 owns pronunciation data, configured provider,
caption rendering, lip sync, voice cache, and actual duration. Read-back returns
the native item identity, resolved position and length, character/profile
digest, pronunciation/cache provenance when available, and the semantic state
needed for verification.

Native generation does not weaken the current rule that an accepted portable
`VoiceTake` has an immutable audio artifact. The tested bridge path exports the
exact generated WAV and a normalized snapshot of host-bound voice state. The
service imports both into content-addressed storage, but the JSON is provenance,
not the engine's portable synthesis query. The result therefore remains a
`host_bound_realization` and is not misrepresented as a portable materialized
take.

### `portable_pair`

TakeGraph supplies its immutable audio artifact and caption. The adapter may
use the version 1 audio/caption pair when YMM4 cannot represent the requested
semantics, including exact A/B take reproduction or separate display/spoken
text. This fallback is part of `TargetPlan`; it is never silent. Changing the
strategy changes the digest and invalidates approval.

Portable manifests carry `caption` and `spokenText` independently. The latter
records the text used to materialize the hash-bound audio artifact and is
sealed into `ManagedCueIntent`; the bridge never infers it from WAV bytes. A
legacy manifest without `spokenText` falls back to `caption`. Equal values keep
the legacy physical-route request digest, while a distinct spoken value is
included in both the Rust and C# request digests.

### Native duration

Native voice duration may be unknown until synthesis completes. Plans therefore
declare `durationResolution` as `exact`, `bounded`, or `unknown`.

The preferred capability prepares a native realization without mutating the
active timeline, producing an exact duration before approval. Until that is
proven for a supported YMM4 version:

- exact ripple previews use the portable path;
- bounded native insertion includes maximum duration/shift/entity budgets;
- an out-of-budget result is rolled back and returned as a new preview;
- an `unknown` duration cannot authorize automatic downstream ripple.

## Field ownership and preservation

Every planned realization declares an ownership mask:

- **strict**: entity identity, cue text, character binding, timing intent, and
  explicitly managed effects;
- **derived**: native length, pronunciation, voice cache, lip-sync data, and
  resolved frame/layer;
- **preserve**: user-added effects, keyframes, UI flags, and unknown native
  fields not claimed by TakeGraph;
- **global**: character definitions, project settings, render profiles, and
  dictionaries, which normal cue patches never mutate.

Updates should mutate an existing native item through the supported YMM4 path
where possible. If replacement is the only option, planning must enumerate any
host-local fields that cannot be preserved. A lossy replacement is blocked or
requires a new explicit approval.

For a YMM-specific effect, TakeGraph owns only the effect instance it created
and its allowlisted parameters. Other effects on the same item remain
YMM-local. Effect descriptors use stable type IDs and parameter schema hashes;
type-name substring searches are not accepted contracts.

Portable TakeGraph templates expand into semantic patch operations before
approval. YMM templates remain target assets referenced by stable descriptor
and content/configuration digest. A missing or changed template invalidates the
plan rather than selecting another template by name.

## Identity and metadata

Version 1 appends an invisible marker to caption text and associates its audio
using audio path and layer. Version 2 uses a native non-rendered metadata carrier
when the active YMM4 driver proves it is persisted and round-trips correctly.
`Remark` is the preferred carrier for the currently observed runtime; it is a
capability, not a cross-version assumption.

The embedded record is deliberately small:

```json
{
  "namespace": "takegraph/v2",
  "projectId": "project-...",
  "entityId": "utt-...",
  "realizationId": "realization-..."
}
```

Speaker names, absolute paths, layers, artifact hashes, and revisions remain in
the canonical service/receipt rather than mutable embedded metadata. The bridge
preserves any user remark content around its namespaced record.

Native item ID, metadata identity, and bridge-side mapping are returned
separately. Duplicate markers caused by copy/paste are detected; neither copy is
silently selected as canonical. Unsaved projects and `Save As` cannot derive
identity solely from a path hash. Rebinding a target is an explicit Tier 2
operation.

Character lookup likewise must not depend on display-name substring matching.
The service stores a project-scoped binding to a bridge descriptor plus its
expected configuration digest. If YMM4 exposes no durable character ID, a
one-time explicit binding uses a bridge mapping or supported metadata carrier.

## Fingerprints and conflict scope

One project-wide fingerprint is both too strict and too weak. Version 2 uses:

1. `targetIdentityDigest`: project, scene, FPS/timebase, and adapter identity;
2. `capabilityDigest`: protocol feature versions and runtime schemas;
3. `managedStateDigest`: semantic state of managed realizations;
4. `conflictScopeDigest`: unmanaged items and settings that can affect the
   approved operation;
5. per-binding digests for character, template, dictionary, and render profile
   dependencies.

Unrelated edits outside the affected time/layer/entity scope do not invalidate
a plan. Changes to a managed item, its overlapping/anchored context, or a pinned
binding do. File-backed dependencies include content hash, not only path.

Read-back verifies semantic postconditions rather than physical item count. A
native cue can be one `VoiceItem`; a fallback cue can be two items. Receipts
report realization kind, native IDs, resolved timing, owned-field digest,
preserved-field digest, artifact information, and any approved variance.

## Version 2 protocol

The examples below describe required meaning, not a frozen JSON schema.

### Capabilities

```json
{
  "protocol": { "major": 2, "minMinor": 0, "maxMinor": 0 },
  "driver": {
    "id": "ymm4-4.55",
    "pluginVersion": "0.2.0",
    "ymm4Version": "4.55.1.1",
    "mutationStatus": "tested"
  },
  "features": {
    "targetPlan.apply": {
      "version": 1,
      "canonicalVersion": 1,
      "resolvedPlacement": true,
      "resolvedBindings": true,
      "mixedStrategies": false
    },
    "voiceItem.create": {
      "version": 1,
      "prepare": false,
      "separateDisplayAndSpokenText": false,
      "artifactCapture": false,
      "artifactExportAvailable": true,
      "identityCarrier": "remark",
      "durationResolution": "bounded"
    },
    "voiceItem.update": {
      "version": 1,
      "mutationMode": "replace_preserving_user_state",
      "preservedStateVerified": true,
      "identityCarrier": "remark",
      "separateDisplayAndSpokenText": false,
      "durationResolution": "bounded"
    },
    "voiceItem.delete": {
      "version": 1,
      "identityCarrier": "remark",
      "deleteReadback": "realization_absent"
    },
    "voiceItem.artifactExport": {
      "version": 1,
      "audio": "exact_wav",
      "provenance": "normalized_host_bound_voice_state",
      "portableSynthesisQuery": false,
      "contentHash": "sha256"
    },
    "timeline.transaction": {
      "version": 1,
      "recoveryReadback": true,
      "durableRollback": false,
      "undoBatch": true
    },
    "project.checkpoint": { "version": 1, "existingPathOnly": true },
    "project.render": { "version": 0, "available": false }
  },
  "capabilityDigest": "sha256:..."
}
```

Unknown YMM4 versions may retain Tier 0 observation. Tier 1 and Tier 2 fail
closed unless a compatible driver successfully probes every required feature.

### Sealed target-plan validation

```json
{
  "protocolVersion": 2,
  "targetPlanDigest": "sha256:...",
  "targetPlan": {
    "canonicalVersion": 1,
    "operationId": "...",
    "baseRevision": 18,
    "target": {
      "adapterId": "ymm4-4.55",
      "projectId": "...",
      "sceneId": "...",
      "fps": 60,
      "driverVersion": "4.55.1.1/0.2.0"
    },
    "capabilityDigest": "sha256:...",
    "expectedScope": {
      "targetIdentityDigest": "sha256:...",
      "managedStateDigest": "sha256:...",
      "conflictScopeDigest": "sha256:..."
    },
    "changeBudget": {
      "maxChangedEntities": 1,
      "maxShiftedEntities": 0,
      "maxShiftFrames": 0,
      "allowLockedChanges": false,
      "allowUnmanagedChanges": false
    },
    "cues": [{
      "strategy": "native_voice",
      "placement": { "frame": 120, "primaryLayer": 20, "secondaryLayer": null },
      "resolvedRealization": {
        "kind": "native_voice",
        "characterName": "魔理沙",
        "characterBindingDigest": "sha256:..."
      }
    }]
  }
}
```

The abbreviated cue above omits the unchanged intent, duration, ownership, and
dependency members for readability. The actual JSON boundary rejects unknown,
missing, duplicate, malformed, or strategy-inconsistent fields. Validation
rehashes the complete plan, checks current target identity, structured
capability digest and per-feature schema digests, managed/conflict scope,
change budget, exact ownership profile, character or artifact binding, and
artifact bytes. It returns only counts and the currently bound fingerprint; it
does not mutate YMM4.

### Target plan

The response includes the chosen strategy, capability and binding dependencies,
resolved placement, duration resolution, owned/preserved fields, exact operation
summary, expected postconditions, and warnings. The service computes the final
approval digest over the portable semantic patch and this complete target plan.

The digest also binds target identity, capability/driver versions, expected
scope fingerprints, change budget, operation ID, and canonical serialization
version. Approval/apply always recompute this digest from payload; they never
trust a digest field loaded from an editable patch file.

### Apply request and receipt

`POST /v2/target-plan/apply` carries `protocolVersion`, `requestDigest`, the
raw approved `expectedFingerprint`, `targetPlanDigest`, and the sealed
`targetPlan`. Operation ID and all physical target/placement fields occur only
inside the sealed plan. The separately carried fingerprint is not an alternate
authority: it is included in the request digest and exists so an
apply-gate-serialized no-start seal can reproduce the exact approved preimage
receipt without trying to invert a scope digest. The request digest binds the
fingerprint, plan, and plan digest. Reusing an operation ID with a different
request digest is a conflict. The bridge translates a validated plan internally
to the existing WAL/transaction implementation; the public compatibility DTOs
are not used by the service's normal cue stage/commit path.

The mutating routes have apply-gate-serialized no-start seals with the same
request body: `POST /v2/target-plan/not-started`,
`POST /v2/native-voice/mutation/not-started`, and
`POST /v2/native-extension/not-started`. If no exact WAL/receipt exists, the
bridge validates only the immutable request shape, canonical digest, and stored
operation binding, then durably writes a request-bound `not_started` receipt
(`verified: false`, before equals after, no realizations). It deliberately does
not require the current project fingerprint, capability catalog, descriptors,
driver, or artifacts: the seal proves that this operation did not start; it
does not re-authorize the approved mutation against newer runtime state.
If a delayed apply already won the gate, the endpoint returns that exact stored
operation instead. A later delayed apply therefore replays the tombstone rather
than mutating. An operation ID can never be rebound to another request.

Before any YMM mutation, the bridge durably records an `Applying` journal entry
containing the request digest, before-state fingerprint, expected postconditions,
and rollback material. The state progression is:

```text
Prepared
  -> Applying
  -> AppliedUnverified
  -> Verified

Applying
  -> RolledBack
  -> RecoveryRequired   (when rollback/read-back cannot prove either state)

AppliedUnverified
  -> Verified
  -> RecoveryRequired   (when the durable read-back evidence is contradictory)
```

`AppliedUnverified` means semantic read-back already succeeded and its evidence
is durable; from that state only `Verified` or `RecoveryRequired` is legal.
`RolledBack` is available only while an `Applying` operation has not durably
recorded successful read-back.

Only a verified receipt whose actual result remains inside the approved change
budget allows the service to advance the TakeGraph revision. If the service
crashes after verification, replay returns the same receipt and finalization is
idempotent. A receipt deserialized from the editable patch file is informational,
not trusted evidence: the service must replay the operation through the
token-authenticated bridge before finalization. If the bridge crashes, startup
recovery scans the journal and native
identity records. A durable successful-read-back transition is historical
evidence and is never rolled back during startup: it is reconstructed as
`Verified`, or becomes `RecoveryRequired` if its exact binding cannot be
proven. An operation that had not completed read-back may become `Verified`
from an exact post-state, `RolledBack` from an exact before-state, or
`RecoveryRequired` when partial or ambiguous.

All project-mutating bridge calls share one operation gate. Save, undo, render,
and another apply cannot interleave between an apply mutation and its read-back.

## Undo, save, render, and reconciliation

YMM4 Undo is not TakeGraph revision history. A TakeGraph user-level undo creates
and approves an inverse semantic patch. The bridge may use a YMM native undo
transaction to make one apply atomic and to roll back failure. If a user invokes
YMM Undo directly, the next snapshot reports target drift; TakeGraph does not
silently decrement its revision.

Save remains separate from apply. A save receipt records project identity,
existing path, TakeGraph revision, pre/post file fingerprint, and target state
digest. The standard API does not choose a `Save As` path.

Render is a separate Tier 2 task with explicit output path, overwrite policy,
render-profile digest, source revision, progress, cancellation, and final media
hash/probe result. It additionally requires the ID and request digest of a
verified checkpoint plus that checkpoint file's exact absolute path, SHA-256,
and byte length. The bridge copies those bytes to a task-local immutable input,
re-hashes both the original checkpoint and the copy before and after encode,
holds read-only leases that deny writes/deletes for the entire encode, and runs
the encoder only against the immutable copy. The copy is placed beside the
checkpoint so YMM-relative asset paths retain their original base directory.
Preview frame capture or
loopback audio recording is not a substitute for an authoritative final render
receipt.

`replace_existing` is protected by a durable bridge-side write-ahead record.
The record is persisted before the worker is scheduled and pins whether an
original output existed, its hash/length, and deterministic backup/quarantine
paths. Cancellation is registered before scheduling and serialized with every
queued → running → completed phase transition, so a queued cancel cannot be
lost. Failure, cancellation, source drift, or bridge restart quarantines any
partial/stale output and restores the original only after independently
verifying the backup bytes. An ambiguous or failed restoration is
`recovery_required`; it is never reported as success.

The 4.55.1.1 driver can bind the actually selected command-line writer and its
resolved video/audio arguments, selector and writer settings, YMM/writer/FFmpeg
binaries and dependencies, codec (`h264`), pixel format (`yuv420p`), AAC-LC
sample rate, dimensions, and frame rate into one profile manifest. Those inputs
can be leased and rechecked for the render duration. The gated task path renders
to a private same-volume candidate, probes the final single-video/single-audio
MP4 evidence, then publishes through the overwrite WAL. Unknown
writers/settings/codecs still return `bindable: false`.

That writer proof is necessary but not sufficient for an authoritative render.
YMM4 4.55.1.1's resource-list surface is a UI inventory, not a supported
exhaustive contract for every path and configuration the child encoder may
resolve. In particular, the current snapshot's file witness is only canonical
path, length, and modification time; it cannot prevent a same-length asset from
being substituted with its timestamp restored. Fonts, voice engines,
tachie/effect inputs, and plugin settings also lack a proven complete closure.
The bridge therefore always returns the current render profile as
`bindable: false` with an explicit dependency-manifest error and does not
advertise `project_render`, `project_render_cancel`, or
`project_render_media_receipt`. The completed writer/probe/WAL machinery stays
behind this gate. Enabling it requires an exhaustive project-relative
dependency manifest whose canonical no-follow identities and SHA-256 values are
bound into the checkpoint/profile digest, held under read leases for the whole
child process, and reverified after exit.

### Scene capture and visual inspection

Semantic read-back proves that the expected YMM items and fields exist, but it
does not prove that the composed scene looks correct. The implemented
scene-inspection workflow adds a second, explicitly non-authoritative visual
check after semantic verification:

1. The service stages an inspection plan bound to project, scene, TakeGraph
   revision, managed/conflict fingerprints, capture profile, and exact frame
   positions. Default sampling includes each changed cue's first stable frame,
   midpoint, and last stable frame, with deduplication for short cues.
2. The bridge prefers a YMM-native preview/frame export that renders the scene
   without window chrome. If only a seek-based API exists, the operation runs
   under the project-operation gate, records and restores playhead/selection,
   and must leave the project clean. A temporary short render may be used as a
   driver-specific fallback. OS/window screenshots are diagnostic-only because
   focus, DPI, overlays, and occlusion make them non-deterministic.
3. The bridge returns a capture receipt containing target identity, requested
   and actual frame, source fingerprints, capture/driver profile digests,
   dimensions, media type, and SHA-256 for every image. It writes only beneath a
   service-authorized staging directory and never accepts an arbitrary path from
   an MCP caller.
4. The service imports the images into content-addressed artifact storage and
   reads them back before review. Automated checks can flag blank/black frames,
   missing or clipped captions, safe-area violations, unexpected portrait
   absence, and obvious overlap. OCR or image-model findings are advisory and
   retain the source image and detector version for audit.
5. MCP presents the sampled images, semantic diff, and automated findings for
   human accept/reject. Capture, authenticated replay, and review attach the verified
   PNG bytes as MCP image content. Only receipt-declared paths matching the
   task's `scene-captures/<prefix>/<sha256>.png` CAS root are read; realpath,
   media type, signature/IHDR dimensions, SHA-256, byte/count limits (16 images,
   8 MiB each, 32 MiB total), the 33,177,600-pixel 8K ceiling, and duplicate
   evidence are checked fail-closed. An empty capture set returns no images
   without touching a not-yet-created CAS root. A `SceneInspectionReceipt` records `captured`,
   `reviewed`, `accepted`, `rejected`, or `stale`; any later relevant scene or
   capture-profile change makes it stale.

Scene inspection does not advance the canonical revision and cannot replace the
apply read-back receipt. It is initially a post-apply quality gate for live
testing and user review. A future policy may require an accepted inspection
before save/render, but only an authoritative render receipt proves final video
output. Audio correctness requires a rendered A/V sample or final render and is
outside still-frame inspection.

The MCP vertical slice exposes `ymm4_scene_inspection_stage`, `approve`,
`capture`, `review`, `decide`, `status`, and `replay` tools. The matching CLI
commands are `takegraph ymm4 scene-stage`, `scene-approve`, `scene-capture`,
`scene-review`, `scene-decide`, `scene-status`, and `scene-replay`. Native PNGs
may be claimed only beneath `%LOCALAPPDATA%\TakeGraph\scene-captures`; the node
canonicalizes and re-reads every claimed path, imports bytes below the state
directory's content-addressed artifact root, and verifies the stored hash and
decode again. Persisted receipts deliberately lose authenticated trust after a
restart, so review and decision commands replay the same digest-bound bridge
operation before proceeding. Automated findings are never an accept/reject
authority.

Manual edits to managed YMM fields are reported as a read-only semantic diff.
The user can discard them by re-exporting, detach the item from TakeGraph, or
stage an import patch. Import follows the same revision and approval path; it is
not automatic bidirectional synchronization.

The managed diff now includes native-extension realizations. Verified
portrait/face, media, typed-effect, and native-template receipts update the
managed projection introduced in schema v2, now published in a schema-v3
generation with the external-operation reservation consumed by canonical
finalization. A fresh general snapshot re-reads their owned semantic fields.
Preserved YMM fields, third-party/unknown effects, host-state witnesses, and
opaque template footprint state remain outside reconciliation ownership. The
expected side is always loaded from the receipt-bound project store; CLI/MCP
callers cannot submit it.

## Remaining gaps after the current implementation

The first implementation issues around request binding, write-ahead recovery,
canonical revision persistence, unified cue planning/read-back, and structured
capability binding are now implemented. The remaining work is:

1. Native voice create/update/delete and post-commit artifact export now have a
   public node/service/CLI/MCP vertical slice. Update is a native replacement
   whose bridge-internal preserved-state digest must match; delete is verified
   by realization absence. Artifact capture imports an exact WAV and normalized
   host-bound provenance after path/hash/length/WAV/JSON read-back. It does not
   recover a portable synthesis query. Native Remark detach is executable, but
   legacy portable-pair identity remains embedded in caption text; pair adoption,
   text-marker detach, and migration still need their own explicit contract. A
   base native-extension marker that also carries effect identities likewise
   cannot be detached until an explicit batch approval names the whole identity
   set; effect-only detach is supported and preserves sibling markers.
2. Both physical shapes now pass through the same semantic
   `NativeRealization` verifier. The current bridge read-back does not expose a
   stable native item ID or enough opaque field data to compute a verified
   `preservedFieldDigest`, so those receipt fields remain unavailable.
3. Public CLI/MCP staging inputs still accept an absolute frame/layer for the
   current fixed-placement slice. The service resolves and seals those values
   before bridge validation, so apply has no raw placement override. Relative
   semantic anchors, ripple policy, and narrower conflict scopes remain future
   planner work.
4. Legacy pair identity remains in rendered caption text, and audio association uses path
   and layer rather than a native identity. Copying/reusing an artifact can
   create ambiguous ownership.
5. The whole-scene fingerprint now includes serialized native item state in
   addition to type/timing/text/path/voice metadata, so effects, keyframes,
   flags, and other readable state participate. A file-backed dependency is
   still witnessed only by canonical path, length, and modification time; it is
   not content-hashed in the snapshot fingerprint. Because YMM4 does not expose
   a proven exhaustive encoder-dependency closure, authoritative render remains
   explicitly fail-closed rather than relying on this weak witness.
6. Batch undo retains only the latest batch in memory, while operation receipts
    survive restart.
7. Durable rollback preimages and startup recovery are implemented for managed
   apply operations. Recovery still intentionally requires manual intervention
   when read-back matches neither the sealed before nor after state.
8. The node normalizes bridge strings into structured, schema-digested
    capabilities and binds the resulting digest to approval. Returning this
    structured contract directly from the bridge remains a wire-format cleanup.
9. Save checkpoints and render tasks have typed lifecycle contracts. Exact
    active-writer binding, independent MP4 probing, candidate publication, and
    recovery are implemented for the supported 4.55.1.1 command-line encoder
    profile. The profile remains unbindable until an exhaustive source
    dependency manifest can be content-bound and leased; live rendering is not
    authorized by this implementation.
10. Scene capture, CAS import, deterministic pixel checks, replay, inline MCP
    PNG content, and human decision lifecycle are code-complete. OCR is not
    implemented, and no live test yet proves playhead/selection/dirty-state
    restoration across the driver matrix.
11. Native portraits/assets/effects/templates have a public typed vertical slice
    and preservation tests, but still lack live supported-version matrix proof.
12. Reconciliation derives expected state from receipt-bound durable target
    projections and materializes accepted import/detach/re-export choices as
    append-only child workflows. Import remains a normal unapproved patch.
    Remark detach has an independently approved, request-bound WAL/recovery wire
    with full-scene Remark preimages, an exact expected-post Remark-set digest,
    one-to-one stable item witnesses, fresh absence, and non-Remark preservation
    proofs. Concurrent or ambiguous Remark-only drift fails closed in normal and
    recovery read-back. Contradictory
    `success`/verified-receipt envelopes fail closed before durable verification.
    Base native-extension detach also fails closed while its Remark contains
    effect identities. Successful detach then commits a receipt-bound canonical
    projection tombstone/removal with CAS, advances one revision, and therefore
    does not recur in the next reconciliation. The schema-v2 reconciliation
    source binds the full normalized capability digest plus the exact detach
    feature version/schema/properties; same-version capability drift invalidates
    approval. Before bridge I/O, the service persists the child request, takes a
    project-wide external-mutation lock, and publishes a schema-v3 canonical
    reservation. All canonical exporters share that lock and publish their own
    request/patch/target-bound reservation before bridge mutation, retaining it
    across process loss through durable finalize. Verified detach consumes the
    reservation exactly once;
    authenticated exact rollback may abort it, while transport/recovery ambiguity
    retains it and blocks unrelated writes. Re-export dispatches
    an exact route manifest into an existing exporter, but deliberately stops at
    a new unapproved preview; the exporter still needs its own approval and apply.
    Live supported-driver fault injection remains incomplete. The detach WAL
    fsyncs file contents before atomic replacement, but Windows power-loss
    durability of the parent-directory rename has not yet been certified on the
    supported filesystem matrix; abrupt process-crash recovery is covered,
    hardware power-loss recovery remains an explicit live-test gap.

These are boundary issues, not reasons to expand the bridge into a general YMM4
remote-control server.

## Staged migration

### Phase 0: harden version 1 (implemented safety and persistence slice)

- Recompute the canonical export envelope digest on approve and apply.
- Bind receipt replay to project, scene, operation ID, and request digest.
- Add write-ahead operation state, rollback, and restart recovery.
- Serialize all project mutations through one gate.
- Persist canonical project revision and target links in `takegraph-service`.
- Fail closed on corrupted receipt/journal storage.

### Phase 1: add the version 2 contract beside version 1 (implemented)

- Introduce `ManagedCueIntent`, `TargetPlan`, `NativeRealization`, ownership
  masks, structured capabilities, and scoped fingerprints.
- Keep the version 1 pair adapter as an explicit realization strategy.
- Make fallback, missing capability, and lossy replacement visible in preview.
- Add read-only character/template/effect descriptors and explicit binding.

The unified semantic types, explicit pair strategy, structured capability
digest, canonical target-plan digest, semantic read-back normalization, and
durable target links are implemented. Character/template/effect descriptors
are exposed by stable ID and pinned config/schema digests. Their Phase 4
preservation/apply contract is documented in
[`ymm4-native-extension-contract.md`](ymm4-native-extension-contract.md).
The normal service stage/commit path now calls `/v2/target-plan/validate` and
`/v2/target-plan/apply`. It sends one canonical sealed plan for either explicit
`portable_pair` or `native_voice`, including driver-ready placement and
artifact/character binding evidence. The bridge rehashes and validates the
plan, current capability/schema contract, target/scope, ownership and budget,
then delegates only the already-resolved operation to its WAL-protected native
mutation implementation. The physical managed-pair/native-voice routes remain
available solely for compatibility and focused mutation workflows.

### Phase 2: native voice for new cues (public mutation/artifact slice implemented; live unverified)

- Implement the tested YMM4 driver for native `VoiceItem` creation/update.
- Use non-rendered metadata identity and semantic read-back.
- Capture query/audio artifacts when the runtime supports it.
- Preserve target-local fields on update.
- Keep existing version 1 pairs readable. Converting them is a normal staged
  patch with a preview, never an automatic project migration.

The bridge implements create, replacement-update with verified preservation,
delete, exact WAV export, and normalized host-bound provenance. Rust node DTOs
and cross-runtime request digests expose those typed routes. The service binds
the mutation plan and structured capability schemas into one approval digest,
requires authenticated idempotent replay plus receipt and fresh-snapshot
semantic read-back, and advances the durable revision through CAS. CLI and MCP
expose `native-voice-mutation-stage|commit|verify|artifacts` and matching
`ymm4_native_voice_mutation_*` tools. Artifact paths are accepted only below the
authorized bridge root and are copied into a reverified TakeGraph CAS. Formal,
unit, and mocked public-surface tests pass; the create/update/delete/artifact
sequence has not yet been certified against the live YMM4 driver matrix.

### Phase 3: scene capture and visual inspection (code vertical slice implemented; live unverified)

- Probe supported YMM4 versions for a chrome-free native frame/preview export;
  keep OS/window capture diagnostic-only.
- Add staged capture plans and `SceneInspectionReceipt` binding the source
  revision, scene fingerprints, requested/actual frames, capture profile, and
  image hashes.
- Import captures into content-addressed storage, then expose an MCP review flow
  that shows images beside semantic diffs and automated findings.
- Start with deterministic blank-frame, dimensions, caption bounds/OCR, safe-area,
  portrait-presence, and overlap checks. Keep probabilistic findings advisory.
- Add stale-result invalidation and live tests proving capture restores transient
  YMM state and never dirties or advances the project.

Capture/CAS/replay/human-decision code is implemented. Deterministic checks cover
dimensions, blank/black frames, configured caption/portrait regions, safe area,
and overlap. MCP attaches bounded, hash-verified PNGs from receipt-declared CAS
paths to capture/authenticated-replay/review results. Status deliberately returns
metadata only instead of trusting persisted image paths. OCR and live transient-state
restoration coverage remain open.

### Phase 4: native portraits, assets, effects, and templates (contract/CLI/MCP code slice implemented; live unverified)

- Add portrait/face items using the same character binding.
- Generalize immutable asset references to image, video, audio, and BGM.
- Add effects one typed schema at a time; preserve unknown effects.
- Add template references only after stable descriptor/digest probing exists.

The Rust/node/service workflow now implements descriptor discovery, immutable
artifact materialization, read-only preservation preview, exact field-level
loss approval, digest-bound apply/replay, semantic verification, and durable
canonical finalization. CLI and MCP expose the full descriptors → stage →
approve → apply → verify/status lifecycle. The YMM4 driver accepts only
allowlisted item/effect/template operations; paths are restricted to
`%LOCALAPPDATA%\TakeGraph\native-extension-artifacts` and independently
rehashed. Unknown effects remain opaque and must round-trip with identical
stable type, instance key, and state digest.

### Phase 5: project checkpoints, render, and reconcile (execution code implemented; live matrix partial)

- Add verified save checkpoints.
- Add render task progress, cancellation, and output artifact receipts.
- Add semantic drift reports and explicit import/detach/re-export choices.
- Retain the managed-subset boundary; do not pursue complete YMM4 round-trip.

The implemented checkpoint flow discovers the bridge profile through
`GET /v2/project/checkpoint-profile`, stages a request bound to project, scene,
canonical source revision, target identity/state, and profile/driver digests,
then replays the exact request through the authenticated bridge. Verification
requires an existing project path, identical before/after semantic state,
pre/post file evidence, and a fresh Rust-side hash and length check. It never
performs `Save As` or advances the canonical revision.

The implemented render flow discovers descriptors through
`GET /v2/render/profiles`, creates a task with `POST /v2/render/tasks`, reads
progress with `GET /v2/render/tasks/{id}`, and cancels with
`POST /v2/render/tasks/{id}/cancel`. The task binds the same source and target
identity/state evidence plus an exact verified checkpoint operation/file, an
explicit absolute output path, explicit deny/replace overwrite policy, and
render-profile digest. The CLI/MCP staging surface consequently requires
`checkpointOperationId` (`--checkpoint-operation-id`). A bridge success first
enters `finalizing`; only an independent Rust-side re-hash of the checkpoint and
immutable encode source, plus a same-handle MP4 structure probe matching the
final media receipt, can make it `verified`. The C# and Rust probes require one
video and one audio track, bind dimensions to the video track, derive exact CFR
from `mdhd`/`stts`, prove H.264 and 8-bit 4:2:0 from `avc1`/`avc3`, `avcC`, and
SPS, and prove AAC-LC 48 kHz from `mp4a`, `esds`, AudioSpecificConfig, the sample
entry, and audio-track timescale. The media probe profile is
`takegraph-final-media-probe/mp4-v3`. MP4 `tkhd` version 0 and 1 offsets are
covered by a shared-byte golden vector in .NET and Rust.

The child never writes the final destination directly. It writes beneath a
task-private, same-volume, no-reparse directory whose namespace lease begins
before the worker is scheduled. The bridge records candidate hash/length in the
WAL, publishes with atomic no-replace move or `File.Replace`, and recovery moves
destination bytes only when they match task-owned candidate evidence. A file
that appeared independently at a deny-mode destination is left untouched.

Checkpoint, render, and reconciliation lifecycles use separate append-only,
hash-chained journals published by atomic rename. Corrupt interior generations,
concurrent generation changes, relevant target drift, and changed canonical
heads fail closed. A sealed durable HEAD, handle-relative no-follow namespace,
parent-directory fsync certification, and tail-truncation/power-loss recovery
remain persistence hardening work; the current chain alone cannot detect removal
of its newest generation. Reconciliation compares only TakeGraph-managed identities and
owned fields. Every reported drift entry needs exactly one explicit
`import_into_take_graph`, `detach_from_take_graph`, or
`re_export_canonical` choice. Apply re-reads the durable receipt-derived
expectation and fresh target before writing deterministic child tasks; it does
not mutate YMM4 or canonical content. Import children contain previewable,
unapproved core patches. Detach children bind a Remark-only mutation contract
that requires a separate exact-digest approval, project-scoped identity, WAL,
authenticated replay, full-scene before/expected-post Remark binding, fresh
Remark-absence read-back, and an unchanged non-Remark scene digest. Duplicate
stable item witnesses or any unrelated Remark drift fail closed. The bridge
advertises `managedIdentity.detach` and
exposes request-bound execute/status routes. Effect-only detach retains sibling
effect identities; base-marker detach with a non-empty effect set awaits a
separately approved batch contract. After bridge verification, detach removes
the exact identity from the receipt-derived canonical projection through an
operation-ID-bound CAS commit, advances one canonical revision, and persists
that `committedRevision` on the child. The source and child approval also bind
the full normalized capability digest and the exact detach feature
version/schema/properties. Approval and a fresh first execution recompute those
values. Exact pending or historically committed replay instead authenticates
the durable original request/receipt binding and deliberately does not depend
on today's capability catalog. The
exact request is durable before a project-scoped external-mutation lock and
canonical pending reservation are acquired; bridge mutation happens only
afterward. Verified finalization clears that same reservation atomically, exact
rollback may abort it, and ambiguous recovery retains it. Every other canonical
exporter uses the same lock and its own durable reservation through finalize, so
it cannot mutate YMM4 behind a reserved revision or lose ownership evidence on
process crash. Exact replay returns the same revision;
changed receipt/projection evidence conflicts. Re-export children accept an exact
route-specific manifest and dispatch through the existing portable-pair,
native-voice-mutation, or native-extension stage path. The persisted result is
only a new previewable, unapproved task; reconciliation approval is never reused
and the dispatcher never applies it. Its manifest digest makes an exact retry
return the same child/task/digest and rejects a changed-manifest replay. See
`docs/ymm4-reconciliation-contract.md` for the exact migration and child-task
contract. Rendering remains unavailable: exact active-writer, settings, binary,
and media binding is implemented, but 4.55.1.1 has no proven exhaustive
child-encoder source-dependency manifest. The bridge fails closed until that
additional proof is implemented.

The CLI reconciliation surface is `reconcile-report`, `reconcile-preview`,
`reconcile-apply`, `reconcile-child-status`, `reconcile-detach-approve`,
`reconcile-detach-execute`, and `reconcile-re-export-dispatch`. The MCP surface
mirrors these as `ymm4_reconcile_report`, `ymm4_reconcile_preview`,
`ymm4_reconcile_apply`, `ymm4_reconcile_child_status`,
`ymm4_reconcile_detach_approve`, `ymm4_reconcile_detach_execute`, and
`ymm4_reconcile_re_export_dispatch`. The durable task root is set by
`--operation-root` or `TAKEGRAPH_PROJECT_OPERATION_ROOT`. Re-export dispatch
also requires CLI `--output-task`; MCP returns the new downstream `handle`,
`digest`, and route so the existing route-specific approval/apply tool consumes
the saved exporter task directly. The workflow obtains the canonical revision
from `canonical-head`; it does not maintain an independent `revision.json`
mirror.
When the active project has no approved initialization, `canonical-head`
returns `initialized: false` with a null revision. This read never creates a
store; ordinary workflows require the explicit `project_initialization`
lifecycle first.
For every canonical-store exporter operation, MCP forwards the same configured
`projectStateRoot` used by `canonical-head`: portable and native-voice
stage/commit, native-voice-mutation stage/commit, and native-extension
stage/approve/apply. A custom root therefore cannot split revision reads from
the store an exporter updates.
Both default MCP roots honor the same `TAKEGRAPH_PROJECT_STATE_ROOT` and
`TAKEGRAPH_PROJECT_OPERATION_ROOT` environment variables as the CLI. The
workflow also retains `{projectId, revision}` from `canonical-head` and pins all
snapshot-dependent follow-up commands with `--expected-project-id`; an active
YMM4 project switch therefore fails before a different project's canonical
store can be accessed or its target can be mutated.

## Verification plan

Current automated evidence covers Rust digest/capability normalization, mutation
payload tamper rejection, action-specific feature gating, create/update/delete
semantic read-back, authenticated replay binding, deserialized-receipt distrust,
durable revision CAS/idempotence, artifact root/hash/length/WAV/provenance/CAS
checks, TypeScript CLI orchestration, and MCP tool delegation. Rust and .NET
share request-digest vectors and a full raw-capability fixture. The Quint models
typecheck and sampled safety runs pass. These are code/formal results, not proof
that the same paths work in a running YMM4 process; the live matrix below remains
required.

### Core and service tests

- Cue display/spoken text invalidation remains independent.
- Target-plan digest changes for strategy, capability, binding, scope, or budget
  changes.
- Approval cannot survive payload mutation or fallback selection.
- Actual native timing outside the approved budget cannot finalize.
- Inverse patch and YMM native undo have distinct state transitions.
- Project head and verified external receipt finalize atomically/idempotently.

### Node and protocol tests

- Major/minor protocol negotiation and feature-version gating.
- Required-capability failure and explicit fallback.
- Operation replay with the same digest succeeds; a different digest conflicts.
- Authentication, loopback restriction, timeout, cancellation, and error mapping.
- Canonical JSON vectors agree between Rust and .NET.

### Bridge unit/driver tests

- Character binding is unambiguous and detects configuration drift.
- Native voice create/update read-backs text, pronunciation capability, actual
  length, metadata identity, and artifact provenance.
- Replacement-update preserves unmanaged effects/keyframes/flags and verifies
  an internal before/after preserved-state digest.
- Duplicate metadata, copy/paste, same-audio reuse, and missing target items are
  detected.
- Scope fingerprints distinguish relevant from unrelated edits.
- Each supported YMM4 version has pinned capability and serialization golden
  fixtures.

Reflection-heavy code should be wrapped by per-version drivers and separated
from pure plan/read-back normalization so most behavior can be tested without a
running YMM4 process.

### Failure-injection integration tests

Inject failure or process exit:

- before journal write;
- after journal write but before mutation;
- after partial mutation;
- after mutation but before read-back;
- after verified bridge receipt but before service finalization;
- during rollback;
- during save and render.

After restart, assert exactly one of verified post-state, verified before-state,
or explicit `RecoveryRequired`; never silently accept ambiguity or apply twice.

### Live YMM4 matrix

- one native voice item with actual duration and character caption styling;
- mixed native voice create/update/delete followed by exact WAV and host-bound
  provenance import, including user-added effect/keyframe preservation;
- native scene capture at cue start/mid/end, image ingestion/read-back, and human
  review with no project-dirty or playhead/selection leak;
- blank frame, clipped caption, missing portrait, safe-area, overlap, and stale
  capture detection; OS/window capture remains diagnostic-only;
- separate display/spoken text capability and fallback behavior;
- native item update with user-added effect preservation;
- mixed version 1 pairs and version 2 native items;
- unmanaged edit outside/inside conflict scope;
- manual YMM Undo/Redo drift;
- missing character/template/effect and changed binding digest;
- normal save, close/restart recovery, and Save As rebinding;
- render completion/cancel/failure and output hash when render is implemented;
- unknown/unsupported YMM4 version remaining read-only.

### Formal specification

`specs/protocols/ymm4_apply_protocol.qnt` now models:

- capability and plan-digest invalidation;
- write-ahead apply state;
- replay with request-digest binding;
- crash recovery before and after mutation;
- partial apply, rollback success/failure, and recovery-required state;
- capability revision invalidation and external project drift.

The companion `ymm4LifecycleProtocol` module models rollback failure reaching
terminal `RecoveryRequired`, the monotonic `AppliedUnverified` boundary that
can recover only to `Verified` or `RecoveryRequired`, approved change budgets,
an unmanaged-state witness, manual native Undo without canonical revision
movement, exclusive project-operation gating, and independent source-bound
checkpoint, inspection, and render lifecycles with stale-result invalidation.

Every canonical-to-YMM mutation route also shares one project-scoped external
mutation boundary. The service acquires the OS fence, re-reads the durable
head, and writes an exact operation/base/patch/request/target reservation
before the first bridge mutation. That reservation survives transport loss,
process restart, and durable-finalize failure; a different exporter route is
rejected until the exact owner either publishes a verified receipt or presents
an authenticated full rollback/no-mutation proof. Recovery is status-first. A
missing receipt is passed to the matching no-start seal under the bridge apply
gate; only an exact `not_started` tombstone clears the reservation. A seal that
returns an existing operation continues exact replay. A bridge transport error,
a failed seal, `RecoveryRequired`, or otherwise ambiguous receipt never clears
the reservation. Fresh capability, descriptor, artifact, and target preflight
runs before a new reservation. Exact pending/committed recovery authenticates
the original request and stored WAL/receipt without requiring today's plugin or
capability contract to equal the historical approval.

If the canonical store commit succeeds but saving the task file does not, the
same operation replays its original bridge request, authenticates the receipt
against the durable receipt digest, and restores the task as committed without
requiring current capability or target fingerprints to equal the historical
preview. CLI results expose `baseRevision`, `revision`, and
`canonicalReplay`; MCP accepts a replay only when the returned revision is the
task base plus one and is not ahead of the current durable head. Task JSON is
saved through a same-directory synced temporary file followed by filesystem
rename replacement, avoiding truncate-in-place torn JSON. The exact rename and
directory-entry durability guarantees under abrupt hardware power loss remain
dependent on the host filesystem.

The `ymm4ExternalMutationFenceProtocol` module and merge-blocking `YMM-021`
property model cross-route exclusion, reservation-before-mutation, ambiguous
reservation retention, verified at-most-once publication, exact rollback
or request-bound `not_started` release, delayed-apply exclusion, and task-save
crash replay. It also retains the exact reservation after a durable successful
read-back and forbids any transition from that evidence to rollback or abort.

Required safety properties include:

- no external mutation before durable approved intent;
- one operation ID applies at most one request digest;
- project head advances only after verified in-budget read-back;
- fallback or driver/capability change invalidates approval;
- crash recovery cannot produce a second apply;
- unrelated YMM state is never rewritten;
- YMM-native undo never silently changes TakeGraph revision.

The implemented safety properties are registered as merge-blocking
`YMM-001` through `YMM-021` entries in `specs/suite.toml`.

## Related documents

- [Architecture](architecture.md)
- [YMM4 bridge protocol and installation](../bridges/ymm4/README.md)
- [Development workflow](development.md)
- [Patch protocol specification](../specs/protocols/patch_protocol.qnt)
- [Current YMM4 apply specification](../specs/protocols/ymm4_apply_protocol.qnt)
