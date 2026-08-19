# Voice Annotation Capture

A voice annotation is a human observation spoken while YMM4 playback
continues: the operator presses a hotkey, talks, and releases. The note is
stored with the exact source position it refers to. Annotation is an input
channel for intent acquisition, not an edit command.

```text
YMM4 playback (never paused)
  -> hotkey toggle + microphone
  -> AnnotationCapture (audio evidence + source anchors)
  -> decoration projection (YMM4 item, editorial fingerprint unchanged)
  -> Transcript (derived, revisable)
  -> Interpretation (derived intent candidate)
  -> existing timeline_edit task with sourceEvidence
       (narration -> VoiceItem, or explicit pin -> annotation marker)
  -> digest-bound review, execute, verified read-back
```

## Concept ladder

```text
AnnotationCapture   what a human said, where, against which source
Transcript          machine-readable rendering of that audio
Interpretation      structured intent candidate derived from a transcript
Patch               concrete reviewable edit staged against a head revision
Realization         YMM4-native or portable projection of an approved patch
```

`Annotation != Intent != Patch`. A capture is immutable evidence. A
transcript is derived from a capture by a specific ASR provider and can be
re-generated or human-corrected as a new revision. An interpretation is an
AI candidate bound to an exact transcript digest; it never mutates YMM4.
Only a staged, approved patch executes, through the existing task envelope.

Captured human voice is never stored as a `VoiceTake`. `VoiceTake` models
generated speech candidates; a capture is observed input evidence.

## Source anchor

Both the start and the end of a recording are anchored. An anchor carries
the validated fields of a `current_scene_composition()` observation:

- `projectId`
- `sceneId`
- `sourceFingerprint`
- `fps`
- `frame`
- `observedCanonicalRevision` (optional)

Comparing the start and end anchors classifies the capture:

- same fingerprint, later frame: normal in-playback note
- different fingerprint, scene, or project: `SourceChanged` — audio is kept,
  but automatic placement and promotion are refused
- a negative frame, zero fps, or empty identity is rejected at the boundary

## Store model

The annotation store is a separate append-only, hash-chained series from
the canonical project store, laid out project-scoped alongside it (for
example `.takegraph/annotation-store/`). Recording a note must never
advance the canonical revision, so annotations cannot invalidate a staged
patch by existing.

Events:

- `CaptureImported`
- `TranscriptAttached`
- `InterpretationAttached`
- `Dismissed`
- `PromotionStaged`
- `PromotionCommitted`

Replay rules:

- re-importing the same capture ID with the same audio hash is an
  idempotent replay
- re-importing the same capture ID with a different audio hash is a
  conflict and is refused
- transcripts and interpretations append new revisions; they never
  overwrite prior ones
- transcription failure leaves the capture intact and re-runnable
- a dismissed capture can only return through an explicit reopen, never
  through implicit re-promotion

## Promotion

Interpretations are promoted as ordinary `timeline_edit` tasks. No new
model-facing task kind or tool is introduced. The staged plan carries a
`sourceEvidence` block — capture ID, transcript digest, interpretation
digest — sealed into the plan digest so provenance stays auditable. Stale
handling is the existing patch machinery: the interpretation must bind to
the current transcript digest, the staged base must equal the current head,
and the target fingerprint must match.

Two realizations may come from the same capture; they are different write
identities:

- `Narration` becomes a native `VoiceItem` (`native_voice_create`)
- an explicit pin becomes an annotation marker (`annotation_marker_create`)

`Highlight`, `Note`, and `Verify` stay evidence-only unless the operator
pins. `CutCandidate` interpretations are retained as evidence only until
timeline delete/split operations exist.

## Timeline presence

A capture must be visible on the YMM4 timeline without becoming an
editorial edit. YMM4 has no comment track. The previous bind treated
"any new item" as a source change because `sourceFingerprint` hashed
every timeline item. That was too coarse: the v2 fingerprint split
already says unrelated decorations must not invalidate a staged plan.

Presence is three layers.

```text
AnnotationCapture   evidence; never writes YMM4; never moves head
Decoration          YMM4-visible working copy; writes YMM4; never moves head
Patch               digest-bound editorial edit; writes YMM4; head after read-back
```

### Editorial identity

Capture classification and staged `timeline_edit` expected fingerprints
use the **editorial fingerprint**: the existing item hash minus items
whose Remark carries `takegraph/annotation/v1`. Native `takegraph/v2`
voice markers stay inside the editorial hash.

`importCapture` still observes first. The decoration is upserted **after**
the end anchor, so start/end of that capture compare equal editorial
identities. A later capture's start/end both exclude decorations, so
they stay comparable. `unmanagedContextCount` also ignores decorations.

This is not a hole that lets capture write dialogue or media. Only the
closed annotation-decoration namespace is excluded.

### Decoration (default item)

The YMM4 bridge, not the capture host, projects the active capture list
onto reserved layer 90 as a **dedicated non-visual item**
(`TakeGraphAnnotationItem : BaseItem`). It is not a `TextItem`,
`VoiceItem`, or `AudioItem`. It does not implement `IVideoItem`, so it
has no preview or render source. A tagged `TextItem` is not an allowed
fallback: that type is visual and appears in the program.

The host remains read-only. The panel upserts after a successful list
refresh, never while recording, and only when the live YMM4
`projectId` and `sceneId` equal the capture's anchors. A list from
another project is ignored; it must not insert into the open timeline.

- one item per `annotationId`; replay is idempotent
- `frame` is the start frame; `length` is `max(1, end - start)`
- label is the transcript summary, or `メモ`
- carrier: Remark `takegraph/annotation/v1`
  `{ projectId, annotationId, realizationId }`
- must not carry the captured WAV
- dismissed captures lose their decoration on the next sync
- legacy `TextItem` decorations are deleted and not recreated as text
- YMM4 Undo that deletes a decoration is not immediately recreated
- copy/paste duplicates are detected; neither copy is silently canonical
- YMM4 becomes dirty because the file gained items; TakeGraph revision
  does not advance
- persistence uses Newtonsoft `$type` of the plugin assembly; opening
  the project without this plugin is residual

If the factory fails, the panel-local overlay remains a UI fallback.
Overlay drawing is residual and out of Quint.

### Pin (Patch)

A pin is an optional digest-bound `timeline_edit` for a decoration the
operator wants as a durable owned realization. It is not required for
timeline visibility. It still uses `sourceEvidence`, still refuses
dismissed / `SourceChanged` / stale-source captures, and still does not
use `VoiceItem` or the captured WAV. Unpin is its own edit and is not
implied by dismiss. Narration promotion and a pin may both exist.

`AnnotationItem` is a known composition-graph kind (`isVisualKind`
false). Pin rides the existing stage → apply → verified read-back
envelope as `annotation_marker_create` / `annotation_marker_delete`.
The pin remark namespace is `takegraph/annotation-pin/v1`; that item
stays inside the editorial fingerprint. Working decorations stay on
`takegraph/annotation/v1` and remain excluded. `importCapture` still
does not create a pin.

## Capture Host boundary

Recording lives in a dedicated `takegraph-capture` crate, hosted as a
long-lived `takegraph annotation listen` process. It owns microphone input,
the global toggle hotkey, WAV capture (16 kHz mono 16-bit PCM), atomic
`*.partial` to content-addressed publish, and the local loopback HTTP API
authenticated by a random token, mirroring the bridge credential model.

It never owns LLM interpretation, YMM4 timeline mutation, VOICEVOX
generation, patch approval, or canonical revision changes. The YMM4 bridge
plugin panel is a thin client of the capture host: status, device
selection, start/stop, recent list, jump, dismiss, the timeline overlay,
and decoration upsert. Model-facing MCP surfaces cannot start recording;
capture gestures are local-only. The capture host itself still does not
write YMM4.

## Privacy boundary

The model-facing `takegraph_inspect` annotations view reports stable IDs,
frames, transcript summaries, intent candidates, and stale/promotion
status. It never reports local audio paths, microphone device IDs, the
capture-host token, or ASR executable and model paths.

## Specification

Store append/immutability/replay rules, promotion guards, and the
decoration projection (place/remove without moving the head or the
editorial fingerprint) are specified in
`specs/protocols/annotation_promotion_protocol.qnt`.
`importCapture` remains an observation. `placeDecoration` may follow it
and must not change `canonicalHead` or `editorialFingerprint`.

Pin extends `ymm4_composition_graph_protocol.qnt` with `AnnotationItem`
and reuses the generic change-set / apply protocols. It does not add a
second approval lifecycle.

The microphone, audio encoder, ASR engine, overlay drawing, YMM4 item
factory, and operating-system behavior are explicitly out of scope for
the spec.

## Milestones

1. Vertical slice: capture host, annotation store, YMM4 panel list, jump.
   Audio, anchors, and listing survive capture-host restarts; the canonical
   head and YMM4 dirty state are untouched.
2. Transcription: user-managed ASR provider (whisper.cpp-class executable,
   never bundled), provider and model digests recorded per transcript.
3. Interpretation and studio review UI, then promotion through
   `timeline_edit` with `sourceEvidence`.
4. Editorial fingerprint excludes `takegraph/annotation/v1` decorations;
   the bridge upserts layer-90 decorations from the active capture list.
5. Explicit pin: `annotation_marker_create` / `annotation_marker_delete`
   on `timeline_edit`, after `AnnotationItem` was added to the
   composition-graph spec.

## Non-goals (first slice)

- automatic cutting, ripple delete, or any timeline destruction
- always-on recording or push-to-talk hold (toggle only)
- full-video VLM analysis
- auto-finalized narration scripts
- direct *editorial* YMM4 mutation from an annotation (dialogue, media,
  or any item outside `takegraph/annotation/v1`)
- inserting a decoration before the end-anchor observation
- using `VoiceItem`, `TextItem`, or the captured WAV as the decoration
- automatic cloud ASR upload
- unsaved/untitled project support
