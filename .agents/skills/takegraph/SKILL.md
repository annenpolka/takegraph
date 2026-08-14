---
name: takegraph
description: >
  Operate TakeGraph's five MCP tools to inspect, stage, approve, execute, or
  decide video-edit tasks on a YMM4 project. Use when an agent or user asks to
  add or edit voice lines, initialize or adopt a TakeGraph project, capture or
  review a scene, checkpoint, render, or reconcile; or mentions
  takegraph_inspect, takegraph_task_stage, planDigest, VoiceTake, YMM4,
  テイクグラフ, 台詞, 音声生成, or シーン確認.
---

# TakeGraph

TakeGraph is a voice-first YMM4 editor. The script is canonical, each generated voice is an immutable take, and every AI edit is a digest-bound task. You operate it through five MCP tools. Do not edit `.ymmp` files or call undocumented route-specific tools.

## Minimum loop

1. `takegraph_inspect` with `view: "overview"` (or `canonical` once you know the store).
2. If the canonical project is uninitialized or unsaved, initialize it before any ordinary write.
3. `takegraph_task_stage` the smallest owning `kind`. Staging never approves or executes.
4. Follow the envelope's `availableActions` only. Do not pick the next verb from memory.
5. Copy `taskId` and every digest from visible tool text. Never invent or reuse a digest.

`timeline_edit` example (already-initialized saved project, two native creates, one commit):

```
takegraph_inspect { "view": "canonical" }
takegraph_task_stage {
  "kind": "timeline_edit",
  "operations": [
    {
      "op": "native_voice_create",
      "entityId": "line-reimu-01",
      "displayText": "こんにちは",
      "characterName": "ゆっくり霊夢",
      "frame": 60,
      "layer": 2,
      "maxLength": 300
    },
    {
      "op": "native_voice_create",
      "entityId": "line-marisa-01",
      "displayText": "今日はサンプルを作るぜ",
      "spokenText": "きょうはさんぷるをつくるぜ",
      "characterName": "ゆっくり魔理沙",
      "frame": 260,
      "layer": 3,
      "maxLength": 300
    }
  ]
}
takegraph_task_execute {
  "taskId": "<opaque taskId from the envelope>",
  "intent": "run",
  "planDigest": "<exact planDigest from the envelope>"
}
```

After a `timeline_edit` stage, `availableActions` is typically `inspect` + `execute`. There is no separate approve step. Pass the exact `planDigest` on execute.

## Two stores

| Store | What it is | Head field |
|---|---|---|
| `studio-session` | In-memory demo session | `sessionRevision` |
| `canonical-project` | YMM4 + project-store | `canonicalRevision` |

Never copy an ID, take, utterance, or revision from one store into the other. Studio speakers like `霊夢` are not YMM4 `characterName` values. Canonical names look like `ゆっくり霊夢` and come from inspect. If inspect lists no canonical characters yet, map `霊夢` → `ゆっくり霊夢` and `魔理沙` → `ゆっくり魔理沙`; never send the studio speaker string.

## Tools

| Tool | Does | Does not |
|---|---|---|
| `takegraph_inspect` | Read studio, canonical, scene, catalog, tasks | Stage or mutate |
| `takegraph_task_stage` | Create an immutable plan / candidate | Approve or execute |
| `takegraph_task_approve` | Bind the exact `planDigest` when `approve` is listed | Execute |
| `takegraph_task_execute` | `run` / `review` / `revalidate` / `cancel` / `collect_artifacts` | Invent a digest |
| `takegraph_task_decide` | Human accept/reject of reviewed evidence | Automated judgment |

Inspect views: `overview`, `studio`, `canonical`, `scene`, `catalog`, `tasks`, `task` (requires `taskId`). `view: "scene"` is a read-only composition observation. PNG visual review is a separate `scene_inspection` task.

## Choose kind

| User intent | `kind` | Notes |
|---|---|---|
| Adopt the already-saved active YMM4 project | `project_initialization` `mode=adopt_active` | Separate approve, then execute |
| Save an untitled active project | `project_initialization` `mode=save_untitled` | `path` only at stage; user must supply it |
| Add 1–128 portable and/or native voice creates as one edit | `timeline_edit` | Prefer this. One plan, one digest, one commit |
| Change or delete an existing native voice | `native_voice_mutation` | Not in `timeline_edit` yet |
| Portrait / media / effect / template | `native_extension` | Not in `timeline_edit`. Bind exact catalog digests |
| Studio demo take / variant | `studio_take` / `studio_voice_variant` | Studio store only |
| PNG frame review | `scene_inspection` | Approve → execute capture/review → human `decide` |
| Verified save | `checkpoint` | Ordinary writes need an initialized named project |
| Render | `render` | Only after a verified checkpoint, a bindable catalog profile, and an absolute `outputPath` |
| Drift report / follow-up | `reconciliation` | Needs an existing target link; do not invent one |
| Reconciliation child apply | `reconciliation_import` / `reconciliation_detach` / `reconciliation_re_export` | Separate guarded tasks. Do not fold them into `timeline_edit` or a mixed parent kind |

Prefer `timeline_edit` over legacy `portable_voice` / `native_voice`. Keep update/delete and extensions on their own kinds so you never claim a cross-route atomic commit the server cannot prove.

Typical `availableActions` after a successful stage (still follow the live envelope if it differs):

| kind | Next verbs |
|---|---|
| `project_initialization` | `approve` → `execute` |
| `timeline_edit` | `execute` with `planDigest` (no approve) |
| `native_voice_mutation` | `execute` with `planDigest` (no approve) |
| `native_extension` | `approve` → `execute` |
| `scene_inspection` | `approve` → `execute` until reviewed → `decide` |
| `studio_take` | `execute` |

## Envelope rules

- `taskId` is the only public identity. Do not substitute `patchId`, a child id, or a raw native handle.
- `planDigest` binds approve/execute to the staged plan. If it changes, re-inspect and start the action sequence again.
- `evidenceDigest` binds `decide` to one evidence set. It is not a plan token. If the field is null, omit it; never invent a digest.
- Read `taskId`, digests, blockers, and next actions from visible text, not only from hidden `structuredContent`.
- After a process restart or an uncertain `timeline_edit` outcome: `takegraph_task_execute` with `intent: "revalidate"` on the same `taskId`. Retry `run` only when `availableActions` still includes `execute`. Revalidate does not contact YMM4 and does not treat a merely staged edit as applied.

## Field rules

- `displayText` / `caption` is what appears. `spokenText` is the reading. Do not silently copy one onto the other.
- Omit native `spokenText` only when the user wants YMM4 to derive the reading. Never invent a reading.
- Portable creates need `caption`, `spokenText`, `speaker`, `style`, `frame`. Defaults if unspecified: speaker `春日部つむぎ`, style `ノーマル`.
- Native creates need `entityId`, `displayText`, `characterName`, `frame`, `layer`, `maxLength`. `entityId` values must be unique inside one task.
- Native update/delete go in `mutations[]`. Copy `entityId`, `realizationId`, per-entity `revision`, `characterName`, `frame`, `layer`, and `maxLength` from inspect (overview text often omits `revision`; use `view: "scene"` / structured item fields). Do not invent `revision: 0`. Do not use `canonicalRevision` or `sessionRevision` as the entity revision. Change only the fields the user asked to change.
- `save_untitled` `path` is accepted only while staging. After that, use only `taskId` + exact `planDigest`. Never invent a Save As path.
- Regenerating speech creates a new take. Never overwrite an existing audio artifact.
- VOICEVOX is a user-managed loopback service. Do not download or bundle it.

## Worked payloads

Initialize untitled (path only on stage):

```
takegraph_task_stage {
  "kind": "project_initialization",
  "mode": "save_untitled",
  "path": "<user-supplied absolute .ymmp path>"
}
takegraph_task_approve { "taskId": "<init taskId>", "planDigest": "<init planDigest>" }
takegraph_task_execute { "taskId": "<init taskId>", "intent": "run", "planDigest": "<init planDigest>" }
```

Then start a **new** task for any voice write. `adopt_active` takes `{ "kind": "project_initialization", "mode": "adopt_active" }` only (no path).

Update one native voice:

```
takegraph_task_stage {
  "kind": "native_voice_mutation",
  "mutations": [
    {
      "action": "update",
      "entityId": "<from inspect>",
      "realizationId": "<from inspect>",
      "revision": "<per-entity revision from inspect>",
      "characterName": "<from inspect>",
      "displayText": "<user's new line>",
      "frame": "<from inspect>",
      "layer": "<from inspect>",
      "maxLength": "<from inspect>"
    }
  ]
}
takegraph_task_execute {
  "taskId": "<mutation taskId>",
  "intent": "run",
  "planDigest": "<mutation planDigest>"
}
```

Delete uses `action: "delete"` with `entityId`, `realizationId`, and `revision` only.

PNG review (not a substitute for `view: "scene"`):

```
takegraph_task_stage {
  "kind": "scene_inspection",
  "frames": [60],
  "expectedWidth": 1920,
  "expectedHeight": 1080
}
takegraph_task_approve { "taskId": "<inspection taskId>", "planDigest": "<inspection planDigest>" }
takegraph_task_execute { "taskId": "<inspection taskId>", "intent": "run", "planDigest": "<inspection planDigest>" }
takegraph_task_decide {
  "taskId": "<inspection taskId>",
  "reviewer": "<human reviewer id>",
  "decision": "accept",
  "note": "<human note>",
  "evidenceDigest": "<exact evidenceDigest, or omit if null>"
}
```

Repeat inspection `execute` only while `availableActions` still includes `execute`. Call `decide` only after an explicit human accept/reject. Automated findings cannot decide.

## Fail closed

- Ordinary canonical writes require an initialized, named YMM4 project.
- If inspect reports unsaved / `initialized: false`, stop and initialize. Do not stage `timeline_edit` first.
- If a bindable render profile is missing, do not stage `render`.
- If there is no target link, do not stage `reconciliation` `mode=report`.
- Do not enable or use legacy route-specific tools unless the user said those routes are on.
- Do not treat a green inspect as proof that YMM4, VOICEVOX, or a renderer did I/O you did not run.
