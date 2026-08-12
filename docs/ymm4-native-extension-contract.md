# YMM4 native-extension bridge contract (Phase 4)

This contract covers character portraits/faces, immutable image/video/audio/BGM clips, allowlisted typed effects, and opaque native-template instantiation. Portable `NativeExtensionPlan` values contain no host paths. Host paths exist only in the authenticated node-to-bridge request after content-addressed import.

## Descriptor discovery

`GET /v2/descriptors` uses the existing C# `DescriptorCatalogDto` and `TargetDescriptorDto` JSON shape. The node validates protocol/project/scene, unique descriptor IDs, and every `configDigest`, `schemaDigest`, `driverProfileDigest`, and `catalogDigest` as SHA-256.

The node exposes both the raw target catalog and a portable planning catalog. A caller binds a descriptor by:

1. stable `descriptorId`;
2. exact raw `configDigest` and `schemaDigest` from YMM4;
3. the derived portable descriptor digest used by `DescriptorReference.expectedDigest`.

The complete raw catalog is compared again before approval and apply. Any config, schema, bindability, mutation-policy, driver-profile, project, or scene change fails closed.

## Plan preflight

`POST /v2/native-extension/plan` is read-only.

Request:

```json
{
  "protocolVersion": 2,
  "operationId": "00000000-0000-0000-0000-000000000000",
  "projectId": "project-id",
  "sceneId": "scene-id",
  "expectedFingerprint": "...",
  "descriptorCatalogDigest": "64 lowercase hex characters",
  "intents": [],
  "artifacts": []
}
```

Response:

```json
{
  "fingerprint": "...",
  "descriptorCatalogDigest": "...",
  "driverProfileDigest": "...",
  "observation": {
    "existing": {
      "portrait:portrait-01": {
        "logicalKey": "portrait:portrait-01",
        "realizationId": "...",
        "kind": "portrait",
        "updateMode": {
          "mode": "replace",
          "lossyFields": ["nativeAnimation.keyframes"]
        },
        "preservedFields": [
          { "field": "remark", "stateDigest": "sha256:..." }
        ],
        "unknownEffects": [
          {
            "stableTypeId": "vendor.effect.Glow",
            "instanceKey": "native-effect-7",
            "stateDigest": "sha256:..."
          }
        ]
      }
    }
  },
  "warnings": []
}
```

Kinds are `portrait`, `face`, `image`, `video`, `audio`, `bgm`, `managed_effect`, and `template`. Update modes are `in_place` or `replace`. Replacement reports the complete `lossyFields` set. The portable planner accepts replacement only when it exactly equals the intent's `approvedLossyFields`; a blanket boolean is not supported. Unknown native effects are opaque and approval-bound by stable type, instance key, and state digest.

## Apply and receipt replay

`POST /v2/native-extension/apply` accepts the complete approved plan and performs idempotent apply/replay.

```json
{
  "protocolVersion": 2,
  "operationId": "...",
  "requestDigest": "64 lowercase hex characters",
  "projectId": "...",
  "sceneId": "...",
  "expectedFingerprint": "...",
  "descriptorCatalogDigest": "...",
  "driverProfileDigest": "...",
  "planDigest": "sha256:...",
  "plan": {},
  "artifacts": []
}
```

Each artifact contains `artifactDigest`, `mediaType`, `byteLength`, `kind`, `path`, and `sha256`. `artifactDigest` and `sha256` must be identical `sha256:` identities. The node imports and rehashes bytes under `%LOCALAPPDATA%\TakeGraph\native-extension-artifacts`; the bridge accepts no other root and independently rehashes path, bytes, media declaration, and length.

Response:

```json
{
  "operationId": "...",
  "requestDigest": "...",
  "projectId": "...",
  "sceneId": "...",
  "status": "verified",
  "beforeFingerprint": "...",
  "afterFingerprint": "...",
  "descriptorCatalogDigest": "...",
  "driverProfileDigest": "...",
  "realizations": [
    {
      "logicalKey": "asset:background-01",
      "realizationId": "...",
      "kind": "image",
      "projectId": "project-id",
      "entityId": "background-01",
      "entityRevision": 4,
      "frame": 120,
      "layer": 10,
      "length": 180,
      "ownedStateDigest": "sha256:...",
      "ownedFields": {
        "artifactDigest": "sha256:...",
        "byteLength": "1234",
        "loop": "false",
        "mediaType": "image/png"
      },
      "preservedFields": [
        { "field": "remark", "stateDigest": "sha256:..." }
      ],
      "stateDigest": "sha256:...",
      "unknownEffects": []
    }
  ],
  "verified": true,
  "error": null
}
```

Statuses are `not_started`, `applying`, `verified`, `stale`, `rolled_back`,
`recovery_required`, and `failed`. Deleted realizations are absent; every
non-delete operation appears exactly once. Reusing an operation ID with any
other request digest is a conflict. Status/recovery replay sends the identical
apply request and validates the complete binding again.

`POST /v2/native-extension/not-started` accepts that same request and runs
under the same bridge apply gate. If no operation record exists, it validates
the immutable request shape/digests and durably stores `not_started` with
`verified: false`, the request-bound approved fingerprint as identical
before/after evidence, no realizations, and a non-empty reason. It does not
require that historical preimage, descriptor catalog, driver, artifacts, or
capabilities to remain current: the tombstone proves absence of this operation,
not freshness of the target. A delayed exact apply then replays this tombstone
and cannot mutate. If apply already won the gate, the seal returns the existing
exact record instead. Canonical code releases a pending reservation only after
the full request/catalog/profile/fingerprint tombstone binding verifies.

`ownedStateDigest` is a versioned canonical digest of `ownedFields`. The bridge reads those fields back from YMM4 rather than echoing the request. The service independently recomputes the digest and compares exact descriptor identities, artifact hash/length/media/loop state, typed-effect descriptor and parameter state, or template footprint as appropriate. Project/entity/revision markers, preservation witnesses, and opaque unknown effects are also approval-bound. An effect deletion returns no realization, but still verifies a preservation witness for its host item. A partial or semantically different host mutation therefore cannot be promoted merely because placement and IDs happen to match.

## Canonical digests

The portable plan digest is SHA-256 over:

```text
UTF8("takegraph-native-extension-plan-v1") || 0x00 || canonical_sorted_JSON(plan)
```

The apply digest begins with `takegraph-ymm4-native-extension-apply-v2\n`. It appends `protocolVersion`, then UTF-8-byte-length-framed strings for operation/project/scene/fingerprint/catalog/driver/plan digests, followed by ordered artifact records. Artifact strings include the absolute Unicode path. The authoritative Unicode/one-artifact golden test is `native_extension_unicode_apply_cross_runtime_golden`:

- plan digest: `sha256:85ad4549abab946e6c7db16e5a4cb9771e0f8db0ca9d10f029c9dcd74c926d74`
- apply request digest: `b53a146c0e039448c589b3072f32045dad57e3dfec4572e884d722cb3ee05d3c`

## User-facing lifecycle

CLI commands are `native-extension-descriptors`, `native-extension-stage`, `native-extension-approve`, `native-extension-apply`, `native-extension-verify`, and `native-extension-status` under `takegraph ymm4`.

MCP tools expose the same lifecycle as `ymm4_native_extension_*`. Verification first replays the authenticated receipt, then independently preflights the current YMM4 scene to detect post-apply user edits. Canonical revision advances only after verified apply; descriptor reads, staging, approval, status, and verification do not advance it.

On successful durable finalization, the verified realizations are filtered
through the versioned native-extension ownership profile and committed to the
schema-v3 project-state generation (the projection format introduced in v2)
together with the external commit, cleared pre-I/O reservation, and
new canonical revision. The general `/v1/snapshot` response independently
exposes `nativeExtensions`, a managed-only live observation with no preserved
fields or unknown effects. Reconciliation compares that fresh observation with
the receipt-derived durable projection; callers cannot provide an expected
projection. Preservation witnesses (`preservedFields`, `unknownEffects`, host
state and opaque template footprint digests) remain approval/read-back evidence
and are not imported as TakeGraph-owned semantics.
