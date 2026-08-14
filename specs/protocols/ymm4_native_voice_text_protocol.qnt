// Native voice display text vs spoken text (読み).
//
// displayText is the visible セリフ and is always TakeGraph-owned. spokenText
// is optional. A present non-empty spokenText is the approved synthesis input
// and is written to Hatsuon. A missing spokenText leaves Hatsuon to YMM
// (KanjiToYomi / character synthesizer). Empty display never admits. An
// explicit empty spokenText also fails closed; omit is the only derive path.
//
// Distinct bound texts require voiceItem.create/update.separateDisplayAndSpokenText.
// Without that property, native admission fails closed. The portable pair may
// still carry distinct caption/spoken values on its own route.
//
// Bound apply writes serif=displayText and hatsuon=spokenText and requires
// exact readback of both. Derived apply writes only serif and requires a
// non-empty observed Hatsuon that is not approval-bound. Changing a bound
// spokenText after staging invalidates the plan. This module does not prove
// YMM4 VoiceItem setter or audio-cache behavior.
module ymm4NativeVoiceTextProtocol {
  type TextStatus =
    | Draft
    | Admitted
    | Previewable
    | Approved
    | Applied
    | Verified
    | Committed
    | Rejected
    | VerificationFailed

  type SpokenPolicy =
    | SpokenBound
    | SpokenDerived

  type Rejection =
    | NoRejection
    | EmptyDisplay
    | EmptySpoken
    | SeparateTextUnsupported
    | ApprovalDigestMismatch
    | ReadBackMismatch

  type VoiceTextBinding = {
    displayText: int,
    spokenText: int,
    spokenPolicy: SpokenPolicy,
    capabilityAdvertised: bool,
    planDigest: int,
  }

  type VoiceTextState = {
    status: TextStatus,
    rejection: Rejection,
    separateCapability: bool,
    requestedDisplay: int,
    requestedSpoken: int,
    spokenPolicy: SpokenPolicy,
    serif: int,
    hatsuon: int,
    synthesizedFrom: int,
    task: VoiceTextBinding,
    approvedBinding: VoiceTextBinding,
    appliedBinding: VoiceTextBinding,
    commitCount: int,
  }

  var state: VoiceTextState

  val emptyText = 0
  val ymmDerivedHatsuon = 7
  val missingBinding = {
    displayText: emptyText,
    spokenText: emptyText,
    spokenPolicy: SpokenBound,
    capabilityAdvertised: false,
    planDigest: 0,
  }

  def textsArePresent(display: int, spoken: int): bool =
    display != emptyText and spoken != emptyText

  def nativeAdmits(state: VoiceTextState): bool =
    state.requestedDisplay != emptyText
      and if (state.spokenPolicy == SpokenDerived)
        true
      else
        textsArePresent(state.requestedDisplay, state.requestedSpoken)
          and (state.requestedDisplay == state.requestedSpoken
            or state.separateCapability)

  def admissionRejection(state: VoiceTextState): Rejection =
    if (state.requestedDisplay == emptyText) EmptyDisplay
    else if (state.spokenPolicy == SpokenBound
      and state.requestedSpoken == emptyText) EmptySpoken
    else if (state.spokenPolicy == SpokenBound
      and state.requestedDisplay != state.requestedSpoken
      and not(state.separateCapability)) SeparateTextUnsupported
    else NoRejection

  def currentBinding(state: VoiceTextState): VoiceTextBinding = {
    displayText: state.requestedDisplay,
    spokenText: if (state.spokenPolicy == SpokenDerived)
      emptyText
    else
      state.requestedSpoken,
    spokenPolicy: state.spokenPolicy,
    capabilityAdvertised: state.separateCapability,
    planDigest: state.task.planDigest,
  }

  action init = all {
    state' = {
      status: Draft,
      rejection: NoRejection,
      separateCapability: false,
      requestedDisplay: 1,
      requestedSpoken: 1,
      spokenPolicy: SpokenBound,
      serif: emptyText,
      hatsuon: emptyText,
      synthesizedFrom: emptyText,
      task: missingBinding,
      approvedBinding: missingBinding,
      appliedBinding: missingBinding,
      commitCount: 0,
    }
  }

  action requestEqualTexts = all {
    state.status == Draft,
    state' = state
      .with("requestedDisplay", 1)
      .with("requestedSpoken", 1)
      .with("spokenPolicy", SpokenBound)
      .with("rejection", NoRejection),
  }

  action requestSeparateTexts = all {
    state.status == Draft,
    state' = state
      .with("requestedDisplay", 1)
      .with("requestedSpoken", 2)
      .with("spokenPolicy", SpokenBound)
      .with("rejection", NoRejection),
  }

  action requestEmptyDisplay = all {
    state.status == Draft,
    state' = state
      .with("requestedDisplay", emptyText)
      .with("requestedSpoken", 2)
      .with("spokenPolicy", SpokenBound)
      .with("rejection", NoRejection),
  }

  action requestEmptySpoken = all {
    state.status == Draft,
    state' = state
      .with("requestedDisplay", 1)
      .with("requestedSpoken", emptyText)
      .with("spokenPolicy", SpokenBound)
      .with("rejection", NoRejection),
  }

  action requestDerivedSpoken = all {
    state.status == Draft,
    state' = state
      .with("requestedDisplay", 1)
      .with("requestedSpoken", emptyText)
      .with("spokenPolicy", SpokenDerived)
      .with("rejection", NoRejection),
  }

  action advertiseSeparateText = all {
    state.status == Draft,
    state' = state.with("separateCapability", true),
  }

  action revokeSeparateText = all {
    state.status == Draft,
    state' = state.with("separateCapability", false),
  }

  action admit = all {
    state.status == Draft,
    nativeAdmits(state),
    state' = state
      .with("status", Admitted)
      .with("rejection", NoRejection),
  }

  action rejectAdmission = all {
    state.status == Draft,
    not(nativeAdmits(state)),
    state' = state
      .with("status", Rejected)
      .with("rejection", admissionRejection(state)),
  }

  action stagePreview = all {
    state.status == Admitted,
    nativeAdmits(state),
    state' = state
      .with("status", Previewable)
      .with("task", currentBinding(state)
        .with("planDigest", state.task.planDigest + 1)),
  }

  action approveExact = all {
    state.status == Previewable,
    nativeAdmits(state),
    state' = state
      .with("status", Approved)
      .with("approvedBinding", state.task),
  }

  action reviseSpokenAfterStage = all {
    state.status == Previewable,
    state.spokenPolicy == SpokenBound,
    state.requestedSpoken != emptyText,
    state' = state
      .with("requestedSpoken", state.requestedSpoken + 10)
      .with("status", Draft)
      .with("task", missingBinding)
      .with("approvedBinding", missingBinding),
  }

  action applyApproved = all {
    state.status == Approved,
    state.task == state.approvedBinding,
    nativeAdmits(state),
    state' = state
      .with("status", Applied)
      .with("serif", state.approvedBinding.displayText)
      .with("hatsuon",
        if (state.approvedBinding.spokenPolicy == SpokenBound)
          state.approvedBinding.spokenText
        else ymmDerivedHatsuon)
      .with("synthesizedFrom",
        if (state.approvedBinding.spokenPolicy == SpokenBound)
          state.approvedBinding.spokenText
        else ymmDerivedHatsuon)
      .with("appliedBinding", state.approvedBinding),
  }

  action acceptExactReadBack = all {
    state.status == Applied,
    state.serif == state.approvedBinding.displayText,
    state.hatsuon != emptyText,
    if (state.approvedBinding.spokenPolicy == SpokenBound)
      all {
        state.hatsuon == state.approvedBinding.spokenText,
        state.synthesizedFrom == state.approvedBinding.spokenText,
      }
    else
      state.synthesizedFrom == state.hatsuon,
    state' = state.with("status", Verified),
  }

  action recordMismatchedReadBack = all {
    state.status == Applied,
    state.approvedBinding.spokenPolicy == SpokenBound,
    state' = state
      .with("status", VerificationFailed)
      .with("rejection", ReadBackMismatch)
      .with("hatsuon", state.hatsuon + 1),
  }

  action recordMissingDerivedHatsuon = all {
    state.status == Applied,
    state.approvedBinding.spokenPolicy == SpokenDerived,
    state' = state
      .with("status", VerificationFailed)
      .with("rejection", ReadBackMismatch)
      .with("hatsuon", emptyText),
  }

  action commit = all {
    state.status == Verified,
    state.task == state.approvedBinding,
    state.appliedBinding == state.approvedBinding,
    state.serif == state.approvedBinding.displayText,
    state.hatsuon != emptyText,
    if (state.approvedBinding.spokenPolicy == SpokenBound)
      all {
        state.hatsuon == state.approvedBinding.spokenText,
        state.synthesizedFrom == state.approvedBinding.spokenText,
      }
    else true,
    state.commitCount == 0,
    state' = state
      .with("status", Committed)
      .with("commitCount", 1),
  }

  action step = any {
    requestEqualTexts,
    requestSeparateTexts,
    requestEmptyDisplay,
    requestEmptySpoken,
    requestDerivedSpoken,
    advertiseSeparateText,
    revokeSeparateText,
    admit,
    rejectAdmission,
    stagePreview,
    approveExact,
    reviseSpokenAfterStage,
    applyApproved,
    acceptExactReadBack,
    recordMismatchedReadBack,
    recordMissingDerivedHatsuon,
    commit,
  }

  val emptyDisplayNeverAdmits =
    state.requestedDisplay == emptyText
      implies (state.status == Draft
        or (state.status == Rejected and state.rejection == EmptyDisplay))

  val boundEmptySpokenNeverAdmits =
    (state.spokenPolicy == SpokenBound
      and state.requestedSpoken == emptyText
      and state.requestedDisplay != emptyText)
      implies (state.status == Draft
        or (state.status == Rejected and state.rejection == EmptySpoken))

  val derivedSpokenDoesNotRequireBoundHatsuon =
    (state.spokenPolicy == SpokenDerived
      and state.requestedDisplay != emptyText)
      implies (state.status != Rejected
        or state.rejection != EmptySpoken)

  val separateWithoutCapabilityNeverApplies = all {
    (state.spokenPolicy == SpokenBound
      and textsArePresent(state.requestedDisplay, state.requestedSpoken)
      and state.requestedDisplay != state.requestedSpoken
      and not(state.separateCapability))
      implies (state.status == Draft
        or (state.status == Rejected
          and state.rejection == SeparateTextUnsupported)),
    state.appliedBinding != missingBinding
      implies (state.appliedBinding.spokenPolicy == SpokenDerived
        or state.appliedBinding.displayText == state.appliedBinding.spokenText
        or state.appliedBinding.capabilityAdvertised),
  }

  val appliedNativeWritesOwnedFields =
    state.status != Applied
      and state.status != Verified
      and state.status != Committed
      or all {
        state.serif == state.appliedBinding.displayText,
        state.appliedBinding.displayText != emptyText,
        state.hatsuon != emptyText,
        if (state.appliedBinding.spokenPolicy == SpokenBound)
          all {
            state.hatsuon == state.appliedBinding.spokenText,
            state.synthesizedFrom == state.appliedBinding.spokenText,
            state.appliedBinding.spokenText != emptyText,
          }
        else
          state.synthesizedFrom == state.hatsuon,
      }

  val verifiedReadbackMatchesPolicy =
    state.status != Committed
      or all {
        state.serif == state.approvedBinding.displayText,
        state.hatsuon != emptyText,
        state.commitCount == 1,
        if (state.approvedBinding.spokenPolicy == SpokenBound)
          all {
            state.hatsuon == state.approvedBinding.spokenText,
            state.synthesizedFrom == state.approvedBinding.spokenText,
          }
        else true,
      }

  val mismatchedReadBackNeverCommits =
    state.status != VerificationFailed
      or state.commitCount == 0

  val approvalBindsSpokenPolicy =
    state.status != Approved
      and state.status != Applied
      and state.status != Verified
      and state.status != Committed
      or all {
        state.approvedBinding.displayText == state.task.displayText,
        state.approvedBinding.spokenText == state.task.spokenText,
        state.approvedBinding.spokenPolicy == state.task.spokenPolicy,
        state.approvedBinding.capabilityAdvertised
          == state.task.capabilityAdvertised,
      }

  val nativeVoiceTextSafety = and {
    emptyDisplayNeverAdmits,
    boundEmptySpokenNeverAdmits,
    derivedSpokenDoesNotRequireBoundHatsuon,
    separateWithoutCapabilityNeverApplies,
    appliedNativeWritesOwnedFields,
    verifiedReadbackMatchesPolicy,
    mismatchedReadBackNeverCommits,
    approvalBindsSpokenPolicy,
  }

  run equalTextsAdmitWithoutCapabilityTest = init
    .then(requestEqualTexts)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(acceptExactReadBack)
    .then(commit)

  run separateTextsRequireAdvertisedCapabilityTest = init
    .then(requestSeparateTexts)
    .then(rejectAdmission)

  run advertisedCapabilityAppliesSeparateTextsTest = init
    .then(advertiseSeparateText)
    .then(requestSeparateTexts)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(acceptExactReadBack)
    .then(commit)

  run emptyDisplayFailsClosedTest = init
    .then(requestEmptyDisplay)
    .then(rejectAdmission)

  run boundEmptySpokenFailsClosedTest = init
    .then(requestEmptySpoken)
    .then(rejectAdmission)

  run derivedSpokenLeavesHatsuonToYmmTest = init
    .then(requestDerivedSpoken)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(acceptExactReadBack)
    .then(commit)

  run spokenChangeAfterStageRequiresRestageTest = init
    .then(advertiseSeparateText)
    .then(requestSeparateTexts)
    .then(admit)
    .then(stagePreview)
    .then(reviseSpokenAfterStage)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(acceptExactReadBack)
    .then(commit)

  run mismatchedBoundHatsuonCannotCommitTest = init
    .then(requestEqualTexts)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordMismatchedReadBack)

  run missingDerivedHatsuonCannotCommitTest = init
    .then(requestDerivedSpoken)
    .then(admit)
    .then(stagePreview)
    .then(approveExact)
    .then(applyApproved)
    .then(recordMissingDerivedHatsuon)
}
