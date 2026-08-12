# YMM4 reconciliation execution boundary

Status: receipt-derived reconciliation, independently approved Remark detach,
and preview-only exporter dispatch are implemented end to end. Import remains
an unapproved canonical patch, and re-export still requires the existing
exporter's separate approval and apply lifecycle.

## Authoritative expected state

Reconciliation does not trust an MCP/CLI caller to describe the canonical YMM4
state. `DurableProjectStore` schema version 3 records a managed-only semantic
projection in the same atomic generation as each verified external commit and
can publish one pending external-mutation reservation without advancing HEAD. The
projection is:

- constructed only from authenticated portable/native-voice receipt items or
  verified native-extension realization read-back;
- bound to the receipt digest, operation ID, target link, and committed revision;
- included in the project-state hash chain and protected by its own
  domain-separated projection digest;
- incrementally replaced by entity for portable audio/caption pairs and by exact
  realization identity for native voice and native-extension
  create/update/delete;
- validated on every store load, including stable sorting, target-link ownership,
  source revision bounds, and digest equality.

`stage_reconciliation_from_durable` resolves the unique durable project/scene
target link, requires the current target identity to match it, and loads this
projection. There is no service, CLI, or MCP input for caller-supplied expected
state. Acceptance loads the durable projection again before materializing
children, so an editable task journal cannot replace the expectation.

Reconciliation report schema version 2 also seals the complete normalized
`capabilityDigest` plus the exact `managedIdentity.detach` feature version,
schema digest, and canonical properties digest. These values participate in the
report, action, child-plan, and approval digests. Acceptance and a first
execution recompute all four values. A bridge that keeps the same driver/version
label but changes a feature property or schema therefore makes the preview stale
before any reservation or target mutation. Once the exact request has a durable
canonical reservation or external-commit record, recovery instead authenticates
that sealed historical evidence and the bridge WAL receipt. It does not require
today's capability contract to equal the old one: doing so could strand a
verified mutation after a plugin upgrade. A missing bridge WAL is never used to
bypass this rule; it is sealed as `not_started` and requires a new preview.
Schema-version-1 reports are not upgraded or grandfathered because they contain
no proof of the approved mutation capability.

Schema-version-1 and schema-version-2 stores remain readable. Version 1 has no
receipt-derived managed projection and therefore fails closed for reconciliation until a verified
portable-pair, native-voice, or native-extension commit seeds a projection.
Version 2 already contains that projection and upgrades to version 3 on the next
publication; an old generation cannot claim a pending reservation because that
field is valid only in version 3.
The service does not infer a baseline from the current live YMM4 scene because
that would silently make drift canonical.

Native-extension finalize atomically records a filtered projection from the
already verified receipt realizations. The general bridge snapshot independently
re-reads those same owned fields from live YMM objects, so portrait/face,
image/video/audio/BGM, typed-effect, and native-template drift participates in
the same report as portable and native-voice items. The reconciliation state
profile is `takegraph-ymm4-managed-semantic-projection/v2`, combined with the
native-extension ownership profile, so older previews become stale rather than
being interpreted under a wider ownership contract.

The native-extension projection intentionally admits only stable identity,
revision, logical key, kind, owned placement, descriptor binding, immutable
artifact hash/length/loop state, typed-effect binding/parameter digest, and
template part count where applicable. `preservedFields`, `unknownEffects`,
`stateDigest`, `hostStateDigest`, and opaque template `footprintDigest` remain
verification/preservation witnesses and never become reconciliation-owned
fields. A user edit to them therefore does not become a TakeGraph import or
re-export action; an edit to an admitted owned field does.

### Semantic identity cardinality

`ManagedSemanticIdentity` identifies the canonical entity plus its optional
native realization ID; `realizationKind` is the typed member key within that
identity. A native realization is one-to-one. A portable export is the sole
legal multi-member shape: when `realizationId` is absent it may contain exactly
one `portable_audio` and exactly one `portable_caption`. This keeps both members
in one reconciliation decision and remains compatible with the entity-scoped
replacement used by `DurableProjectStore`.

Comparison is member-by-kind inside that legal pair. An unchanged audio/caption
pair therefore produces no drift; one missing member is reported as a
kind-qualified `$present` field change, and an edited member reports
kind-qualified owned fields. A repeated kind, more than two members, a portable
pair carrying a native realization ID, or a pair containing an unrelated kind
is `duplicate_managed_identity` and fails closed.

## Acceptance and child materialization

Accepting a digest-approved reconciliation preview re-reads the canonical head,
target identity, durable expected projection, and live managed snapshot. It does
not modify TakeGraph content or YMM4. Each approved typed action is materialized
as an append-only child journal under `reconciliation-children/`. Child keys are
deterministic hashes of the report and action, making retries idempotent even if
a crash occurs after some children are published but before the parent
generation is written.

The parent record moves to `actions_materialized` only after all children have
been loaded back and checked against the approved action. It returns, for every
entry, the durable child task ID, action digest, downstream task ID where
applicable, and normal patch ID where applicable.

### Import into TakeGraph

`ProposeImportPatch` becomes a `ReconciliationImportPatchDraft` containing the
exact observed managed projection. Its normal core `Patch` is `previewable`, has
no approved digest, and remains bound to the reconciliation source revision.
Consequently acceptance of the reconciliation choice is not approval of the
semantic patch and cannot advance the canonical revision.

### Detach managed identity

`DetachManagedIdentity` becomes a `ReconciliationDetachDraft` with a separate
previewable patch and operation ID. Accepting the reconciliation decision does
not approve this patch. `approve_reconciliation_detach` requires its exact
current digest and canonical source revision before the child may execute.

The bridge advertises `managedIdentity.detach` only when the request-bound
receipt, write-ahead apply, recovery read-back, and supported-driver gates are
also present. `POST /v2/reconciliation/detach` accepts a protocol-v2 request
bound to operation ID, request digest, canonical source revision, project,
scene, expected fingerprint, entity, realization ID, and the exact
`takegraph_remark_v2` carrier. The adapter permits only removal of that identity
marker and requires all of the following:

- preserve every non-remark content field;
- write-ahead-log the old remark before mutation;
- authenticated idempotent replay;
- fresh post-apply read-back;
- proof that the TakeGraph remark is absent;
- proof that the non-remark content digest is unchanged.

Before touching YMM4, the bridge writes an atomic per-operation journal with the
complete request binding, every scene item's stable type/timing/non-Remark
witness, every original Remark preimage, the exact expected Remark after
removing only the selected identity, full before/expected-post Remark-set
digests, and a whole-scene digest computed with every Remark cleared. Duplicate
or ambiguous stable witnesses fail closed. Immediately before mutation the
bridge rechecks that complete sealed state. It then removes only the selected
marker, preserving user-written Remark text and unrelated TakeGraph
markers. An effect-marker detach rewrites only that effect entry and retains
every sibling identity. A base native-extension marker that still contains any
effect identity fails closed: removing the enclosing marker would implicitly
detach the whole effect set, and an explicit batch-detach approval contract does
not exist yet. A fresh snapshot must prove the requested identity is absent,
the complete observed Remark set exactly equals the expected-post set with
one-to-one item cardinality, and the non-Remark scene digest is unchanged. Only
then is a `verified` terminal receipt persisted. The Rust node verifies the same
request/receipt binding and preservation witnesses. On a first execution the
service also performs one more fresh project-scoped semantic read-back before
marking the child `verified`. Historical recovery from an exact reservation or
commit uses the authenticated terminal receipt and sealed canonical evidence,
so a later valid target edit cannot make an already committed operation
unrecoverable. The service also rejects any response whose top-level `success`
disagrees with the receipt's `verified` status; such a response remains
`applying` and cannot persist as `verified`.

Detach is a permanent ownership change, not a temporary removal of target
metadata. After the authenticated receipt and fresh absence check pass, the
service creates a `VerifiedExternalCommit` bound to the detach operation ID,
child patch digest, request digest, complete receipt digest, target identity,
and after fingerprint. A managed-state update must find the exact identity in
the receipt-derived canonical projection, removes only that identity, and
commits with compare-and-swap at the child source revision. This atomically
advances canonical revision by one and records `committedRevision` on the child.
The operation ID makes a crash after canonical commit but before child-journal
publication recoverable: exact retry authenticates the bridge receipt and
replays the same canonical commit; changed evidence conflicts. Consequently the
next reconciliation does not report the detached identity again.

The service closes the canonical revision race before calling the detach route.
The order is fixed: persist the exact child request, acquire the project-scoped
external-mutation OS lock, publish a schema-v3 canonical reservation at the
source revision, and only then call YMM4. The reservation binds operation ID,
base revision, child patch digest, request digest, target link, exact managed
identity, and its own domain-separated digest. It survives service restart but
does not advance HEAD. Every canonical-to-YMM4 exporter holds the same project
lock from before bridge mutation through durable finalization and publishes an
ordinary reservation over the same operation/base/patch/request/target fields.
It permits only exact same-operation recovery and refuses a reservation owned
by another operation. Ordinary target-plan, native-voice mutation, and
native-extension routes also expose apply-gate-serialized `not_started` seals.
On a missing operation record, recovery calls the exact route seal instead of
assuming that a 404 excludes an older POST still delayed in transport. Only a
fully request-bound durable tombstone releases the canonical reservation; a
seal failure or any ambiguous result retains it. Thus neither detach nor an
ordinary export can mutate YMM4 and later lose the canonical CAS after a process
crash or concurrent apply.

A verified receipt finalizes the same reservation, removes the exact identity,
advances HEAD once, and clears the reservation in one canonical generation.
Exact replay after a lost response or restart is idempotent whether the state is
still reserved or the external commit already exists. Transport loss,
`applying`, `failed`, contradictory envelopes, and `recovery_required` retain
the reservation. Only a request-bound `rolled_back` receipt proving the exact
before fingerprint, full before-Remark digest, and unchanged non-Remark digest
may abort it. A second safe terminal is `not_started`:
`POST /v2/reconciliation/detach/not-started` runs under the same bridge apply
gate and durably publishes a request-bound no-mutation tombstone only when no
operation WAL exists. It binds the approved expected fingerprint but does not
require the current project to still have that fingerprint: the seal proves
absence of this operation's mutation rather than re-authorizing it. A delayed
exact detach POST then replays the tombstone instead of mutating. This closes the
pre-WAL conflict/transport window and permits the canonical reservation to be
aborted without guessing. Ambiguous outcomes intentionally block unrelated
external writes instead of allowing YMM4 and canonical ownership to diverge.

An exact retry reuses the stored request and returns the durable terminal
receipt (`replayed: true`); the same operation ID with any changed binding is
rejected. Recovery is status-first. An exact pending reservation reads the
bridge operation before consulting current capabilities: `verified` finalizes
from the sealed historical receipt, `applying` uses the exact idempotent POST,
and `rolled_back` or `not_started` releases the reservation only after its full
binding verifier passes. That terminal attempt is not reusable: the durable
child is reissued with a new operation ID and patch digest in `preview_ready`,
with its request, receipt, error, committed revision, and approval cleared. A
new explicit approval is therefore required before another detach attempt. If
status is 404, the service calls the not-started seal
immediately; it does not execute the old approval against a possibly new feature
schema. `failed`, `recovery_required`, transport ambiguity, or a failed seal
retain the reservation. An exact external-commit record similarly repairs a
child generation lost after canonical commit even if later valid exports have
advanced the head and target. On bridge startup, an `applying` journal is
classified from fresh state and either verified against the full expected-post
Remark set, rolled back and matched against the full sealed before-Remark set,
or moved to `recovery_required`. Any unresolved detach journal participates in
the same global recovery gate as other bridge writes.

The WAL file is flushed with write-through before its atomic replacement.
Process-crash/restart recovery is covered; parent-directory rename persistence
under abrupt hardware power loss is not yet certified across the supported
Windows filesystem matrix and remains a live durability test gap.

This route applies only to identities carried in native YMM4 `Remark` metadata,
including native voice and supported native-extension/effect identities. The
legacy portable caption marker lives in rendered text and is deliberately not
removed by this metadata-only route; such a child fails closed until a distinct
portable detach/migration contract exists. Base native-extension detach while
the same marker owns effects likewise remains unsupported until batch approval
can name every identity that would be detached.

Public surfaces are:

- bridge: `POST /v2/reconciliation/detach`,
  `POST /v2/reconciliation/detach/not-started`, and
  `GET /v2/reconciliation/detach/{operationId}`;
- service: `approve_reconciliation_detach` and
  `execute_reconciliation_detach`;
- CLI: `reconcile-child-status`, `reconcile-detach-approve`, and
  `reconcile-detach-execute`;
- MCP: `ymm4_reconcile_child_status`, `ymm4_reconcile_detach_approve`, and
  `ymm4_reconcile_detach_execute`.

### Re-export canonical state

`ReExportCanonicalState` becomes a `ReconciliationReExportTask`. It selects the
existing portable-pair, native-voice-mutation, or native-extension exporter
route from the report's semantic realization kinds. Materialization records
only the route, canonical semantic projection, deterministic handoff digest,
and a new downstream task ID; it performs no target mutation. That ID is a
durable correlation key and the exact operation ID injected into the exporter
task created at dispatch. It is not approval.

Dispatch is explicit because a semantic projection is not a complete exporter
manifest. The caller supplies a tagged `ReconciliationReExportManifest` for the
selected route. Before calling an exporter, the service projects that manifest
back into `ManagedSemanticItem` values and requires exact equality with the
durable canonical projection. It also rechecks the canonical revision and a
fresh project/scene/structured-capability target identity.

The dispatcher then uses the existing stage path:

- `portable_pair` ->
  `Ymm4ExportPatch::stage_from_snapshot_with_operation_id`;
- `native_voice_mutation` ->
  `Ymm4NativeVoiceMutationPatch::stage_from_snapshot_with_operation_id`;
- `native_extension` ->
  `Ymm4NativeExtensionTask::stage_with_operation_id_and_identity_overrides`,
  preserving the canonical realization ID without allowing an observed
  identity to be rebound.

All three calls receive `downstreamTaskId` as their explicit operation ID. The
dispatcher rejects a staged preview whose operation ID differs, so the returned
handle cannot name a phantom handoff while the exporter persists another task.

The resulting exporter task is stored in `downstreamPreview` only if its normal
core patch is `previewable`, has no `approvedDigest`, and remains bound to the
reconciliation source revision. The route-tagged manifest and handoff digest
are sealed into `dispatchManifestDigest`: an exact retry of the same child and
manifest returns the same durable downstream task and digest, while any changed
manifest conflicts. Dispatch never calls approve, apply, or finalize. The
existing exporter retains its ordinary digest approval, capability/fingerprint
checks, WAL, read-back, and finalization rules.

CLI `reconcile-re-export-dispatch --output-task <path>` writes the inner task in
the existing exporter's native file shape. MCP
`ymm4_reconcile_re_export_dispatch` returns a route-specific `handle`, its new
downstream `digest`, and `downstreamRoute`. The handle resolves to
`patch.json`, `native-voice-mutation.patch.json`, or
`native-extension.task.json`, so the normal route-specific approval/apply tools
consume it directly. Neither surface copies the reconciliation approval into
that task. MCP passes its configured `projectStateRoot` to the downstream
portable, native-voice mutation, and native-extension stage/approve/apply
commands, so `canonical-head` and the exporter always address the same durable
project store even when a custom root is configured.

An exact route manifest is mandatory and route-specific constraints still fail
closed. In particular, portable deletion cannot be represented by its
create/replace-only exporter. A native-voice manifest must contain exactly one
mutation for the child entity/realization: canonical absence requires that one
mutation to be `delete`, while canonical presence requires a non-delete at the
sealed entity revision with an exact semantic projection. Additional or
unrelated deletes are rejected instead of being hidden by projection filtering.
Native extension likewise requires one intent and preserves the sealed
realization identity. Import children are unaffected: they remain previewable
and unapproved after reconciliation acceptance.

## Verification coverage

Automated tests cover:

- schema-v1/v2 read compatibility, schema-v2 projection seeding, and schema-v3
  reservation publication;
- receipt-bound projection persistence, identity deletion, and reopen;
- atomic native-extension receipt projection, owned-field drift, and exclusion
  of preserved/unknown/opaque fields;
- legal portable audio/caption pair equality, missing/changed members, duplicate
  kinds, and unrelated cross-kind collisions;
- project-state rejection after managed-projection tampering;
- deterministic/idempotent creation of all three child kinds;
- import/detach children remaining unapproved after reconciliation acceptance;
- cross-runtime detach digest and receipt binding, WAL durability, selective
  effect-marker removal with sibling identity preservation, base-marker batch
  detach rejection, full-scene Remark preimage/expected-post binding, duplicate
  witness rejection, concurrent Remark drift rejection, restart recovery, fresh
  read-back, and non-Remark preservation;
- project-scoped detach lifecycle and authenticated idempotent replay;
- request-first reservation, restart replay after the target committed but the
  response was lost/rejected, competing canonical commit exclusion, exactly-once
  reservation finalization, permanent ownership removal, capability drift after
  verified WAL publication, and lost child-journal repair after later commits;
- apply-gate-serialized `not_started` sealing after a missing/pre-WAL operation,
  exact reservation release, delayed POST replay without mutation, and safe
  reissue under a new operation ID and fresh approval boundary;
- ordinary portable/native-voice/native-mutation/native-extension reservation
  persistence, cross-route OS-lock exclusion, and post-commit exact replay;
- full capability, detach feature version/schema/properties approval binding,
  including same-version schema/property drift rejection;
- rejection of contradictory bridge `success`/verified-receipt envelopes before
  a durable child can become `verified`;
- route-exact re-export semantic projection, identity-preserving native
  extension planning, exact downstream operation-ID injection, and rejection of
  native-voice manifests containing unrelated or additional deletes;
- same-manifest re-export replay returning the same child/task/digest and
  changed-manifest replay failing closed;
- route-specific CLI/MCP task handoff to an existing exporter while the saved
  downstream patch is still previewable and unapproved, including custom
  canonical project-state roots;
- rejection of missing or rebound child references.

The formal `ymm4ReconciliationProtocol` model fixes the same properties:
reconciliation acceptance approves no child, WAL precedes the only detach
mutation, verified detach has a fresh request-bound receipt, recovery-required
closes the mutation gate, dispatch cannot mutate, downstream apply requires a
separate approval, and only verified permanent detach advances the canonical
revision exactly once while preview/import/re-export dispatch remain inert.
