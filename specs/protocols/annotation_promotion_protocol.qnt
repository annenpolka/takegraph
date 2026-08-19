// Voice-annotation capture and promotion boundary.
//
// This module specifies how a captured human voice annotation becomes
// immutable evidence, how derived transcripts and interpretations bind to
// that evidence, which guards promote an interpretation into the
// canonical revision through the existing patch pipeline, and how a
// decoration projection may appear on the YMM4 timeline without becoming
// an editorial commit.
//
// importCapture never writes YMM4. placeDecoration may follow it. A
// decoration must not move canonicalHead or editorialFingerprint.
// Editorial identity excludes takegraph/annotation/v1 items; that
// exclusion is an implementation residual of the fingerprint function.
//
// It does not specify microphones, audio encoders, ASR engines, LLMs, UI,
// MCP hosts, or operating-system behavior.
module annotationPromotionProtocol {
  type CaptureStatus =
    | Active
    | Dismissed

  type PromotionStatus =
    | None
    | Staged
    | Applied
    | Promoted
    | Stale

  type DecorationStatus =
    | Absent
    | Present

  type Capture = {
    present: bool,
    audioHash: int,
    status: CaptureStatus,
  }

  type Transcript = {
    attached: bool,
    audioHash: int,
    digest: int,
    failed: bool,
  }

  type Interpretation = {
    attached: bool,
    transcriptDigest: int,
    digest: int,
  }

  type Promotion = {
    status: PromotionStatus,
    interpretationDigest: int,
    base: int,
    executions: int,
  }

  var canonicalHead: int
  var verifiedCommits: int
  var capture: Capture
  var transcript: Transcript
  var interpretation: Interpretation
  var promotion: Promotion
  var sourceStale: bool
  var committedRevision: int
  var decoration: DecorationStatus
  var editorialFingerprint: int
  var captureProject: int
  var activeProject: int
  var lastEvent: str

  val missing = -1

  val emptyCapture: Capture = {
    present: false,
    audioHash: missing,
    status: Active,
  }

  val emptyTranscript: Transcript = {
    attached: false,
    audioHash: missing,
    digest: missing,
    failed: false,
  }

  val emptyInterpretation: Interpretation = {
    attached: false,
    transcriptDigest: missing,
    digest: missing,
  }

  val emptyPromotion: Promotion = {
    status: None,
    interpretationDigest: missing,
    base: missing,
    executions: 0,
  }

  action init = all {
    canonicalHead' = 0,
    verifiedCommits' = 0,
    capture' = emptyCapture,
    transcript' = emptyTranscript,
    interpretation' = emptyInterpretation,
    promotion' = emptyPromotion,
    sourceStale' = false,
    committedRevision' = missing,
    decoration' = Absent,
    editorialFingerprint' = 0,
    captureProject' = missing,
    activeProject' = 1,
    lastEvent' = "init",
  }

  // Importing a capture is an observation: it never moves the canonical
  // head. The audio hash is fixed at import time.
  action importCapture = all {
    not(capture.present),
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = { present: true, audioHash: 1, status: Active },
    transcript' = emptyTranscript,
    interpretation' = emptyInterpretation,
    promotion' = emptyPromotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Absent,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = 1,
    activeProject' = activeProject,
    lastEvent' = "capture-imported",
  }

  // Replaying the same capture ID with the same audio hash is idempotent.
  action replayImport = all {
    capture.present,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "capture-replayed",
  }

  // Replaying the same capture ID with a different audio hash is refused.
  // No action in this module ever changes capture.audioHash once present.
  action refuseConflictingImport = all {
    capture.present,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "capture-conflict-refused",
  }

  // Transcription may fail. The capture survives as evidence and can be
  // re-run later by a different provider.
  action transcriptionFails = all {
    capture.present,
    not(transcript.attached),
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript.with("failed", true),
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "transcription-failed",
  }

  action attachTranscript = all {
    capture.present,
    not(transcript.attached),
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = {
      attached: true,
      audioHash: capture.audioHash,
      digest: 10,
      failed: false,
    },
    interpretation' = emptyInterpretation,
    promotion' = emptyPromotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "transcript-attached",
  }

  // An ASR retry or a human correction appends a new transcript revision.
  // Interpretations and promotions bound to the previous digest are dropped.
  action reviseTranscript = all {
    transcript.attached,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript
      .with("digest", transcript.digest + 1)
      .with("failed", false),
    interpretation' = emptyInterpretation,
    promotion' = emptyPromotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "transcript-revised",
  }

  action attachInterpretation = all {
    transcript.attached,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = {
      attached: true,
      transcriptDigest: transcript.digest,
      digest: 100,
    },
    promotion' = emptyPromotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "interpretation-attached",
  }

  action reviseInterpretation = all {
    interpretation.attached,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation.with("digest", interpretation.digest + 1),
    promotion' = emptyPromotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "interpretation-revised",
  }

  // Dismissal keeps the audio evidence but ends the promotion path.
  action dismiss = all {
    capture.present,
    capture.status == Active,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture.with("status", Dismissed),
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = emptyPromotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Absent,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "capture-dismissed",
  }

  // Returning a dismissed capture to the active path is an explicit human
  // action, never a side effect of promotion.
  action reopen = all {
    capture.status == Dismissed,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture.with("status", Active),
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "capture-reopened",
  }

  // The observed source fingerprint no longer matches the project source.
  // A staged promotion degrades to Stale; nothing is deleted.
  action markSourceStale = all {
    not(sourceStale),
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = if (promotion.status == Staged)
      promotion.with("status", Stale)
    else
      promotion,
    sourceStale' = true,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "source-marked-stale",
  }

  action stagePromotion = all {
    capture.present,
    capture.status == Active,
    interpretation.attached,
    not(sourceStale),
    promotion.status == None,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = {
      status: Staged,
      interpretationDigest: interpretation.digest,
      base: canonicalHead,
      executions: 0,
    },
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "promotion-staged",
  }

  // Fail-closed stops for staging.
  action refuseStaleSourceStage = all {
    sourceStale,
    capture.status == Active,
    interpretation.attached,
    promotion.status == None,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "stage-refused-stale-source",
  }

  action refuseDismissedStage = all {
    capture.status == Dismissed,
    interpretation.attached,
    promotion.status == None,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "stage-refused-dismissed",
  }

  // Some other verified task commits and advances the canonical head.
  // It cannot interleave with an in-flight apply because YMM4 mutation is
  // serialized behind the bridge operation gate.
  action concurrentVerifiedCommit = all {
    promotion.status != Applied,
    canonicalHead' = canonicalHead + 1,
    verifiedCommits' = verifiedCommits + 1,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint + 1,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "concurrent-verified-commit",
  }

  // Applying the promotion does not move the canonical head. The head
  // advances only after a verified read-back.
  action executePromotion = all {
    promotion.status == Staged,
    promotion.base == canonicalHead,
    promotion.interpretationDigest == interpretation.digest,
    not(sourceStale),
    capture.status == Active,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion
      .with("status", Applied)
      .with("executions", promotion.executions + 1),
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "promotion-applied",
  }

  action refuseStaleBaseExecute = all {
    promotion.status == Staged,
    promotion.base != canonicalHead,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "execute-refused-stale-base",
  }

  action confirmVerifiedReadback = all {
    promotion.status == Applied,
    canonicalHead' = canonicalHead + 1,
    verifiedCommits' = verifiedCommits + 1,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion.with("status", Promoted),
    sourceStale' = sourceStale,
    committedRevision' = canonicalHead + 1,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint + 1,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "promotion-verified",
  }

  // A decoration is a YMM4-visible projection of an active capture. It
  // is not an editorial commit: head and editorial fingerprint stay put.
  action placeDecoration = all {
    capture.present,
    capture.status == Active,
    decoration == Absent,
    captureProject == activeProject,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Present,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "decoration-placed",
  }

  action replayDecoration = all {
    decoration == Present,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Present,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "decoration-replayed",
  }

  action refusePlaceDismissed = all {
    capture.status == Dismissed,
    decoration == Absent,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Absent,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "decoration-refused-dismissed",
  }

  action switchActiveProject = all {
    capture.present,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Absent,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject + 1,
    lastEvent' = "active-project-switched",
  }

  action refusePlaceWrongProject = all {
    capture.present,
    capture.status == Active,
    decoration == Absent,
    captureProject != activeProject,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Absent,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "decoration-refused-wrong-project",
  }

  action removeDecoration = all {
    decoration == Present,
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = Absent,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = "decoration-removed",
  }

  action stutter = all {
    canonicalHead' = canonicalHead,
    verifiedCommits' = verifiedCommits,
    capture' = capture,
    transcript' = transcript,
    interpretation' = interpretation,
    promotion' = promotion,
    sourceStale' = sourceStale,
    committedRevision' = committedRevision,
    decoration' = decoration,
    editorialFingerprint' = editorialFingerprint,
    captureProject' = captureProject,
    activeProject' = activeProject,
    lastEvent' = lastEvent,
  }

  action step = any {
    importCapture,
    replayImport,
    refuseConflictingImport,
    transcriptionFails,
    attachTranscript,
    reviseTranscript,
    attachInterpretation,
    reviseInterpretation,
    dismiss,
    reopen,
    markSourceStale,
    stagePromotion,
    refuseStaleSourceStage,
    refuseDismissedStage,
    concurrentVerifiedCommit,
    executePromotion,
    refuseStaleBaseExecute,
    confirmVerifiedReadback,
    placeDecoration,
    replayDecoration,
    refusePlaceDismissed,
    switchActiveProject,
    refusePlaceWrongProject,
    removeDecoration,
    stutter,
  }

  // The audio evidence of an imported capture never changes.
  val captureEvidenceIsImmutable =
    not(capture.present) or capture.audioHash == 1

  // A transcript only exists as a derivation of the exact captured audio.
  val transcriptBindsAudioEvidence =
    not(transcript.attached) or transcript.audioHash == capture.audioHash

  // An interpretation only exists as a derivation of an exact transcript.
  val interpretationBindsTranscript =
    not(interpretation.attached)
      or interpretation.transcriptDigest == transcript.digest

  // A staged or executing promotion only exists against the exact
  // interpretation it was derived from.
  val promotionBindsInterpretation =
    promotion.status == None
      or promotion.interpretationDigest == interpretation.digest

  // The canonical head moves only together with a verified commit, so
  // capture, transcript, and interpretation actions never advance it.
  val canonicalHeadTracksVerifiedCommits =
    canonicalHead == verifiedCommits

  // Transcription failure preserves the captured evidence.
  val failedTranscriptionPreservesEvidence =
    not(transcript.failed) or capture.present

  // A stale source never carries a staged promotion.
  val staleSourceBlocksStagedPromotion =
    not(sourceStale) or promotion.status != Staged

  // A dismissed capture holds no promotion path until an explicit reopen.
  val dismissedCapturesNeverPromote =
    capture.status != Dismissed or promotion.status == None

  // A promoted edit executed exactly once.
  val promotionExecutesAtMostOnce =
    promotion.status != Promoted or promotion.executions == 1

  // A promoted edit committed the revision that followed its verified
  // base, and no later revision is attributed to it.
  val promotedRevisionFollowsVerifiedBase =
    promotion.status != Promoted
      or and {
        committedRevision == promotion.base + 1,
        committedRevision <= canonicalHead,
      }

  // Decorations are not verified commits, so they cannot change the
  // editorial identity used by capture classification and staged plans.
  val editorialFingerprintTracksVerifiedCommits =
    editorialFingerprint == verifiedCommits

  // A dismissed capture has no decoration projection.
  val dismissedCapturesHaveNoDecoration =
    capture.status != Dismissed or decoration == Absent

  // Decorations exist only while the live project is the captured one.
  val decorationsStayOnCapturedProject =
    decoration != Present or captureProject == activeProject

  val safety =
    captureEvidenceIsImmutable
      and transcriptBindsAudioEvidence
      and interpretationBindsTranscript
      and promotionBindsInterpretation
      and canonicalHeadTracksVerifiedCommits
      and failedTranscriptionPreservesEvidence
      and staleSourceBlocksStagedPromotion
      and dismissedCapturesNeverPromote
      and promotionExecutesAtMostOnce
      and promotedRevisionFollowsVerifiedBase
      and editorialFingerprintTracksVerifiedCommits
      and dismissedCapturesHaveNoDecoration
      and decorationsStayOnCapturedProject

  run captureToPromotionHappyPathTest = init
    .then(importCapture)
    .then(attachTranscript)
    .then(attachInterpretation)
    .then(stagePromotion)
    .then(executePromotion)
    .then(confirmVerifiedReadback)
    .expect(all {
      safety,
      promotion.status == Promoted,
      promotion.executions == 1,
      canonicalHead == 1,
      committedRevision == 1,
      capture.status == Active,
    })

  run observationsNeverMoveCanonicalHeadTest = init
    .then(importCapture)
    .then(transcriptionFails)
    .then(attachTranscript)
    .then(reviseTranscript)
    .then(attachInterpretation)
    .then(reviseInterpretation)
    .then(placeDecoration)
    .expect(all {
      safety,
      canonicalHead == 0,
      editorialFingerprint == 0,
      decoration == Present,
      interpretation.transcriptDigest == transcript.digest,
    })

  run decorationDoesNotInvalidateStagedPromotionTest = init
    .then(importCapture)
    .then(attachTranscript)
    .then(attachInterpretation)
    .then(stagePromotion)
    .then(placeDecoration)
    .then(replayDecoration)
    .then(executePromotion)
    .then(confirmVerifiedReadback)
    .expect(all {
      safety,
      decoration == Present,
      promotion.status == Promoted,
      canonicalHead == 1,
      editorialFingerprint == 1,
    })

  run refuseDecorationOnWrongProjectTest = init
    .then(importCapture)
    .then(switchActiveProject)
    .then(refusePlaceWrongProject)
    .expect(all {
      safety,
      decoration == Absent,
      captureProject != activeProject,
      canonicalHead == 0,
      editorialFingerprint == 0,
    })

  run dismissRemovesDecorationTest = init
    .then(importCapture)
    .then(placeDecoration)
    .then(dismiss)
    .then(refusePlaceDismissed)
    .expect(all {
      safety,
      decoration == Absent,
      capture.status == Dismissed,
      canonicalHead == 0,
      editorialFingerprint == 0,
    })

  run transcriptRevisionDropsPromotionTest = init
    .then(importCapture)
    .then(attachTranscript)
    .then(attachInterpretation)
    .then(stagePromotion)
    .then(reviseTranscript)
    .expect(all {
      safety,
      promotion.status == None,
      not(interpretation.attached),
    })

  run staleBaseRefusesExecutionTest = init
    .then(importCapture)
    .then(attachTranscript)
    .then(attachInterpretation)
    .then(stagePromotion)
    .then(concurrentVerifiedCommit)
    .then(refuseStaleBaseExecute)
    .expect(all {
      safety,
      promotion.status == Staged,
      canonicalHead == 1,
      promotion.base == 0,
    })

  run staleSourceBlocksStagingTest = init
    .then(importCapture)
    .then(attachTranscript)
    .then(attachInterpretation)
    .then(markSourceStale)
    .then(refuseStaleSourceStage)
    .expect(all {
      safety,
      promotion.status == None,
      sourceStale,
      capture.present,
    })

  run dismissalRequiresExplicitReopenTest = init
    .then(importCapture)
    .then(attachTranscript)
    .then(attachInterpretation)
    .then(dismiss)
    .then(refuseDismissedStage)
    .then(reopen)
    .then(stagePromotion)
    .expect(all {
      safety,
      promotion.status == Staged,
      capture.status == Active,
    })

  run conflictingReplayChangesNothingTest = init
    .then(importCapture)
    .then(attachTranscript)
    .then(refuseConflictingImport)
    .then(replayImport)
    .expect(all {
      safety,
      capture.audioHash == 1,
      transcript.audioHash == capture.audioHash,
      canonicalHead == 0,
    })
}
