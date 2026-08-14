# Voice Annotation Capture

A voice annotation is a human observation spoken while YMM4 playback
continues: the operator presses a hotkey, talks, and releases. The note is
stored with the exact source position it refers to. Annotation is an input
channel for intent acquisition, not an edit command.

```text
YMM4 playback (never paused)
  -> hotkey toggle + microphone
  -> AnnotationCapture (audio evidence + source anchors)
  -> Transcript (derived, revisable)
  -> Interpretation (derived intent candidate)
  -> existing timeline_edit task with sourceEvidence
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
and the target fingerprint must match. `CutCandidate` interpretations are
retained as evidence only until timeline delete/split operations exist.

## Capture Host boundary

Recording lives in a dedicated `takegraph-capture` crate, hosted as a
long-lived `takegraph annotation listen` process. It owns microphone input,
the global toggle hotkey, WAV capture (16 kHz mono 16-bit PCM), atomic
`*.partial` to content-addressed publish, and the local loopback HTTP API
authenticated by a random token, mirroring the bridge credential model.

It never owns LLM interpretation, YMM4 timeline mutation, VOICEVOX
generation, patch approval, or canonical revision changes. The YMM4 bridge
plugin panel is a thin client of the capture host: status, device
selection, start/stop, recent list, jump, dismiss. Model-facing MCP
surfaces cannot start recording; capture gestures are local-only.

## Privacy boundary

The model-facing `takegraph_inspect` annotations view reports stable IDs,
frames, transcript summaries, intent candidates, and stale/promotion
status. It never reports local audio paths, microphone device IDs, the
capture-host token, or ASR executable and model paths.

## Specification

Store append/immutability/replay rules and the promotion guards are
specified in `specs/protocols/annotation_promotion_protocol.qnt`. The
microphone, audio encoder, ASR engine, UI, and operating-system behavior
are explicitly out of scope for the spec.

## Milestones

1. Vertical slice: capture host, annotation store, YMM4 panel list, jump.
   Audio, anchors, and listing survive capture-host restarts; the canonical
   head and YMM4 dirty state are untouched.
2. Transcription: user-managed ASR provider (whisper.cpp-class executable,
   never bundled), provider and model digests recorded per transcript.
3. Interpretation and studio review UI, then promotion through
   `timeline_edit` with `sourceEvidence`.

## Non-goals (first slice)

- automatic cutting, ripple delete, or any timeline destruction
- always-on recording or push-to-talk hold (toggle only)
- full-video VLM analysis
- auto-finalized narration scripts
- direct YMM4 mutation from an annotation
- automatic cloud ASR upload
- unsaved/untitled project support
