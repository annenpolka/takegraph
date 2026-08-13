// YMM4 project initialization protocol.
//
// Initialization is an approved, digest-bound task. AdoptActive may either
// publish generation zero for a previously unseen saved project or recognize
// an exact canonical store that was already present when the plan was staged.
// SaveUntitled serializes to a same-directory temporary file, atomically
// claims a new .ymmp path without overwrite, rebinds the active YMM4 project,
// and only then publishes canonical generation zero.
//
// Prepared-file evidence, destination publication, canonical publication, and
// their receipts are intentionally separate durable effects. An exact retry
// can therefore recover every partial boundary without repeating a write.
module ymm4ProjectInitializationProtocol {
  type InitMode =
    | AdoptActive
    | SaveUntitled

  type InitStatus =
    | Draft
    | Previewable
    | Approved
    | Saving
    | SavedPartial
    | RecoveryRequired
    | Bootstrapping
    | BootstrapPartial
    | Completed
    | AlreadyInitialized
    | Conflicted

  // The receipt may learn the owned temporary path before serialization has
  // completed. Once complete hash/length evidence exists it is never erased,
  // including after the temporary file is moved to the destination.
  type PreparedEvidence =
    | NoPreparedPath
    | PreparedPathOnly
    | PreparedFileComplete

  type InitBinding = {
    operationId: int,
    requestDigest: int,
    mode: InitMode,
    activeSessionId: int,
    sourceFingerprint: int,
    sourcePathId: int,
    destinationPathId: int,
    destinationFingerprint: int,
    canonicalProjectId: int,
    canonicalRevision: int,
    canonicalPreexisting: bool,
  }

  type InitState = {
    status: InitStatus,
    activeSessionId: int,
    activeFingerprint: int,
    activeSaved: bool,
    activePathId: int,

    saveTargetExists: bool,
    saveTargetOwnerOperationId: int,
    preparedEvidence: PreparedEvidence,
    temporaryWrites: int,
    saveWrites: int,
    rebindWrites: int,

    canonicalInitialized: bool,
    canonicalMatchesSource: bool,
    canonicalOwnerOperationId: int,
    canonicalHead: int,
    bootstrapWrites: int,

    walPrepared: bool,
    saveReceiptPresent: bool,
    bootstrapReceiptPresent: bool,
    completionReceiptPresent: bool,
    recoveryRejects: int,
    lastRecoveryAttemptDigest: int,

    task: InitBinding,
    approvedBinding: InitBinding,
    walBinding: InitBinding,
    preparedBinding: InitBinding,
    saveFileBinding: InitBinding,
    saveReceiptBinding: InitBinding,
    recoveryBinding: InitBinding,
    canonicalBinding: InitBinding,
    bootstrapReceiptBinding: InitBinding,
    completionBinding: InitBinding,
  }

  val missing = -1
  val externalOwner = -2

  val emptyBinding = {
    operationId: missing,
    requestDigest: missing,
    mode: AdoptActive,
    activeSessionId: missing,
    sourceFingerprint: missing,
    sourcePathId: missing,
    destinationPathId: missing,
    destinationFingerprint: missing,
    canonicalProjectId: missing,
    canonicalRevision: missing,
    canonicalPreexisting: false,
  }

  var state: InitState

  pure def approvalMustRemainBound(status: InitStatus): bool =
    any {
      status == Approved,
      status == Saving,
      status == SavedPartial,
      status == RecoveryRequired,
      status == Bootstrapping,
      status == BootstrapPartial,
      status == Completed,
      status == AlreadyInitialized,
    }

  pure def executionHasStarted(status: InitStatus): bool =
    any {
      status == Saving,
      status == SavedPartial,
      status == RecoveryRequired,
      status == Bootstrapping,
      status == BootstrapPartial,
      status == Completed,
      status == AlreadyInitialized,
    }

  pure def saveSourceIsFresh(s: InitState): bool =
    all {
      s.task.mode == SaveUntitled,
      not(s.activeSaved),
      s.activePathId == missing,
      s.activeSessionId == s.task.activeSessionId,
      s.activeFingerprint == s.task.sourceFingerprint,
      s.task.sourcePathId == missing,
      not(s.task.canonicalPreexisting),
    }

  pure def adoptSourceIsFresh(s: InitState): bool =
    all {
      s.task.mode == AdoptActive,
      s.activeSaved,
      s.activeSessionId == s.task.activeSessionId,
      s.activeFingerprint == s.task.sourceFingerprint,
      s.activePathId == s.task.sourcePathId,
      s.task.destinationPathId == s.task.sourcePathId,
      s.task.canonicalProjectId == s.task.sourcePathId,
    }

  pure def canonicalMatchesStagedPlan(s: InitState): bool =
    all {
      s.canonicalInitialized,
      s.canonicalMatchesSource,
      s.canonicalHead == s.task.canonicalRevision,
    }

  pure def canonicalStateMatchesPreview(s: InitState): bool =
    if (s.task.canonicalPreexisting)
      canonicalMatchesStagedPlan(s)
    else
      not(s.canonicalInitialized)

  pure def previewPreconditionsAreFresh(s: InitState): bool =
    all {
      canonicalStateMatchesPreview(s),
      if (s.task.mode == AdoptActive)
        adoptSourceIsFresh(s)
      else
        all {
          saveSourceIsFresh(s),
          not(s.saveTargetExists),
        },
    }

  pure def preparedPathIsRequestBound(s: InitState): bool =
    all {
      s.preparedEvidence != NoPreparedPath,
      s.preparedBinding == s.task,
      s.walPrepared,
      s.walBinding == s.task,
    }

  pure def preparedFileIsComplete(s: InitState): bool =
    all {
      s.preparedEvidence == PreparedFileComplete,
      s.temporaryWrites == 1,
      preparedPathIsRequestBound(s),
    }

  pure def claimedFileIsRequestBound(s: InitState): bool =
    all {
      s.task.mode == SaveUntitled,
      preparedFileIsComplete(s),
      s.saveWrites == 1,
      s.saveTargetExists,
      s.saveTargetOwnerOperationId == s.task.operationId,
      s.saveFileBinding == s.task,
    }

  pure def activeIsSavedTarget(s: InitState): bool =
    all {
      s.activeSaved,
      s.activeSessionId == s.task.activeSessionId,
      s.activeFingerprint == s.task.destinationFingerprint,
      s.activePathId == s.task.destinationPathId,
    }

  pure def savedFileIsRecoverable(s: InitState): bool =
    all {
      claimedFileIsRequestBound(s),
      activeIsSavedTarget(s),
    }

  pure def savedReceiptIsFresh(s: InitState): bool =
    all {
      savedFileIsRecoverable(s),
      s.saveReceiptPresent,
      s.saveReceiptBinding == s.task,
    }

  pure def bootstrapSourceIsFresh(s: InitState): bool =
    if (s.task.mode == AdoptActive)
      adoptSourceIsFresh(s)
    else
      savedReceiptIsFresh(s)

  pure def saveCanProceedOrRecover(s: InitState): bool =
    if (s.saveWrites == 0)
      all {
        saveSourceIsFresh(s),
        not(s.saveTargetExists),
        not(s.canonicalInitialized),
        s.preparedEvidence == NoPreparedPath
          or preparedPathIsRequestBound(s),
      }
    else
      all {
        claimedFileIsRequestBound(s),
        not(s.canonicalInitialized),
        saveSourceIsFresh(s) or activeIsSavedTarget(s),
      }

  pure def recoveryCanResume(s: InitState): bool =
    all {
      s.recoveryBinding == s.task,
      s.approvedBinding == s.task,
      s.walPrepared,
      s.walBinding == s.task,
      if (s.saveWrites == 0)
        all {
          saveSourceIsFresh(s),
          not(s.saveTargetExists),
          not(s.canonicalInitialized),
          preparedPathIsRequestBound(s),
        }
      else
        all {
          claimedFileIsRequestBound(s),
          not(s.canonicalInitialized),
          saveSourceIsFresh(s) or activeIsSavedTarget(s),
        },
    }

  pure def bootstrapCanProceedOrRecover(s: InitState): bool =
    if (s.bootstrapWrites == 0)
      all {
        bootstrapSourceIsFresh(s),
        not(s.canonicalInitialized),
      }
    else
      all {
        s.bootstrapWrites == 1,
        s.canonicalInitialized,
        s.canonicalMatchesSource,
        s.canonicalOwnerOperationId == s.task.operationId,
        s.canonicalBinding == s.task,
        s.canonicalHead == s.task.canonicalRevision,
      }

  action init =
    state' = {
      status: Draft,
      activeSessionId: 1,
      activeFingerprint: 10,
      activeSaved: false,
      activePathId: missing,

      saveTargetExists: false,
      saveTargetOwnerOperationId: missing,
      preparedEvidence: NoPreparedPath,
      temporaryWrites: 0,
      saveWrites: 0,
      rebindWrites: 0,

      canonicalInitialized: false,
      canonicalMatchesSource: false,
      canonicalOwnerOperationId: missing,
      canonicalHead: missing,
      bootstrapWrites: 0,

      walPrepared: false,
      saveReceiptPresent: false,
      bootstrapReceiptPresent: false,
      completionReceiptPresent: false,
      recoveryRejects: 0,
      lastRecoveryAttemptDigest: missing,

      task: {
        operationId: 1,
        requestDigest: 0,
        mode: SaveUntitled,
        activeSessionId: 1,
        sourceFingerprint: 10,
        sourcePathId: missing,
        destinationPathId: 100,
        destinationFingerprint: 20,
        canonicalProjectId: 100,
        canonicalRevision: 0,
        canonicalPreexisting: false,
      },
      approvedBinding: emptyBinding,
      walBinding: emptyBinding,
      preparedBinding: emptyBinding,
      saveFileBinding: emptyBinding,
      saveReceiptBinding: emptyBinding,
      recoveryBinding: emptyBinding,
      canonicalBinding: emptyBinding,
      bootstrapReceiptBinding: emptyBinding,
      completionBinding: emptyBinding,
    }

  action makeActiveProjectSaved = all {
    state.status == Draft,
    state' = state
      .with("activeSaved", true)
      .with("activePathId", 10),
  }

  // A canonical store that exists before staging is a valid, explicit
  // AdoptActive no-op candidate. Its exact head/source are captured in plan.
  action makeMatchingCanonicalPreexisting = all {
    state.status == Draft,
    state.activeSaved,
    state' = state
      .with("canonicalInitialized", true)
      .with("canonicalMatchesSource", true)
      .with("canonicalOwnerOperationId", externalOwner)
      .with("canonicalHead", 0),
  }

  action stageSaveUntitled = all {
    state.status == Draft,
    not(state.activeSaved),
    not(state.saveTargetExists),
    not(state.canonicalInitialized),
    state' = state
      .with("status", Previewable)
      .with("task", state.task
        .with("requestDigest", state.task.requestDigest + 1)
        .with("mode", SaveUntitled)
        .with("activeSessionId", state.activeSessionId)
        .with("sourceFingerprint", state.activeFingerprint)
        .with("sourcePathId", missing)
        .with("destinationPathId", 100)
        .with("destinationFingerprint", 20)
        .with("canonicalProjectId", 100)
        .with("canonicalRevision", 0)
        .with("canonicalPreexisting", false)),
  }

  action stageAdoptActive = all {
    state.status == Draft,
    state.activeSaved,
    not(state.canonicalInitialized) or state.canonicalMatchesSource,
    state' = state
      .with("status", Previewable)
      .with("task", state.task
        .with("requestDigest", state.task.requestDigest + 1)
        .with("mode", AdoptActive)
        .with("activeSessionId", state.activeSessionId)
        .with("sourceFingerprint", state.activeFingerprint)
        .with("sourcePathId", state.activePathId)
        .with("destinationPathId", state.activePathId)
        .with("destinationFingerprint", state.activeFingerprint)
        .with("canonicalProjectId", state.activePathId)
        .with("canonicalRevision", if (state.canonicalInitialized) state.canonicalHead else 0)
        .with("canonicalPreexisting", state.canonicalInitialized)),
  }

  action refreshPreview = all {
    state.status == Previewable,
    previewPreconditionsAreFresh(state),
    state' = state.with("task", state.task
      .with("requestDigest", state.task.requestDigest + 1)),
  }

  action approve = all {
    state.status == Previewable,
    previewPreconditionsAreFresh(state),
    state' = state
      .with("status", Approved)
      .with("approvedBinding", state.task),
  }

  action reviseAfterApproval = all {
    state.status == Approved,
    state' = state
      .with("status", Previewable)
      .with("task", state.task
        .with("requestDigest", state.task.requestDigest + 1)),
  }

  // Acquiring this durable project-scoped record fences lazy bootstrap and
  // competing initializers before any Save As or canonical write.
  action prepareSaveExecution = all {
    state.status == Approved,
    state.task == state.approvedBinding,
    state.task.mode == SaveUntitled,
    previewPreconditionsAreFresh(state),
    state' = state
      .with("status", Saving)
      .with("walPrepared", true)
      .with("walBinding", state.approvedBinding),
  }

  action prepareAdoptExecution = all {
    state.status == Approved,
    state.task == state.approvedBinding,
    state.task.mode == AdoptActive,
    not(state.task.canonicalPreexisting),
    previewPreconditionsAreFresh(state),
    state' = state
      .with("status", Bootstrapping)
      .with("walPrepared", true)
      .with("walBinding", state.approvedBinding),
  }

  // AlreadyInitialized is success only when this exact canonical store/head
  // was present at stage time. It performs neither Save As nor bootstrap.
  action finishAdoptAlreadyInitialized = all {
    state.status == Approved,
    state.task == state.approvedBinding,
    state.task.mode == AdoptActive,
    state.task.canonicalPreexisting,
    previewPreconditionsAreFresh(state),
    state' = state.with("status", AlreadyInitialized),
  }

  action allocatePreparedPath = all {
    state.status == Saving,
    state.walPrepared,
    state.walBinding == state.task,
    state.approvedBinding == state.task,
    state.preparedEvidence == NoPreparedPath,
    saveSourceIsFresh(state),
    not(state.saveTargetExists),
    not(state.canonicalInitialized),
    state' = state
      .with("preparedEvidence", PreparedPathOnly)
      .with("preparedBinding", state.task),
  }

  action writePreparedFile = all {
    state.status == Saving,
    state.preparedEvidence == PreparedPathOnly,
    preparedPathIsRequestBound(state),
    state.temporaryWrites == 0,
    saveSourceIsFresh(state),
    not(state.saveTargetExists),
    not(state.canonicalInitialized),
    state' = state
      .with("preparedEvidence", PreparedFileComplete)
      .with("temporaryWrites", 1),
  }

  // Atomic move without overwrite is the only transition that can publish
  // the destination. A racing owner prevents this action entirely.
  action claimPreparedFile = all {
    state.status == Saving,
    state.approvedBinding == state.task,
    preparedFileIsComplete(state),
    state.saveWrites == 0,
    saveSourceIsFresh(state),
    not(state.saveTargetExists),
    not(state.canonicalInitialized),
    state' = state
      .with("saveTargetExists", true)
      .with("saveTargetOwnerOperationId", state.task.operationId)
      .with("saveFileBinding", state.task)
      .with("saveWrites", 1),
  }

  action rebindClaimedProject = all {
    state.status == Saving,
    claimedFileIsRequestBound(state),
    saveSourceIsFresh(state),
    not(state.canonicalInitialized),
    state' = state
      .with("activeSaved", true)
      .with("activePathId", state.task.destinationPathId)
      .with("activeFingerprint", state.task.destinationFingerprint)
      .with("rebindWrites", state.rebindWrites + 1),
  }

  action publishSavedReceipt = all {
    state.status == Saving,
    state.walBinding == state.task,
    state.approvedBinding == state.task,
    savedFileIsRecoverable(state),
    not(state.saveReceiptPresent),
    state' = state
      .with("status", SavedPartial)
      .with("saveReceiptPresent", true)
      .with("saveReceiptBinding", state.task),
  }

  // Any uncertain failure after prepared-path publication is durable and may
  // only be resumed by the exact operation/digest binding.
  action markRecoveryRequired = all {
    state.status == Saving,
    state.preparedEvidence != NoPreparedPath,
    state.walBinding == state.task,
    state.approvedBinding == state.task,
    state' = state
      .with("status", RecoveryRequired)
      .with("recoveryBinding", state.task)
      .with("lastRecoveryAttemptDigest", state.task.requestDigest),
  }

  action resumeRecoveryBeforeClaim = all {
    state.status == RecoveryRequired,
    recoveryCanResume(state),
    state.saveWrites == 0,
    state.lastRecoveryAttemptDigest == state.task.requestDigest,
    state' = state.with("status", Saving),
  }

  action resumeRecoveryByRebinding = all {
    state.status == RecoveryRequired,
    recoveryCanResume(state),
    state.saveWrites == 1,
    saveSourceIsFresh(state),
    state.lastRecoveryAttemptDigest == state.task.requestDigest,
    state' = state
      .with("status", SavedPartial)
      .with("activeSaved", true)
      .with("activePathId", state.task.destinationPathId)
      .with("activeFingerprint", state.task.destinationFingerprint)
      .with("rebindWrites", state.rebindWrites + 1)
      .with("saveReceiptPresent", true)
      .with("saveReceiptBinding", state.task),
  }

  action resumeRecoveryAfterRebind = all {
    state.status == RecoveryRequired,
    recoveryCanResume(state),
    state.saveWrites == 1,
    activeIsSavedTarget(state),
    state.lastRecoveryAttemptDigest == state.task.requestDigest,
    state' = state
      .with("status", SavedPartial)
      .with("saveReceiptPresent", true)
      .with("saveReceiptBinding", state.task),
  }

  // A different request using the operation ID is rejected before path rebind
  // or receipt publication. The approved task itself remains immutable.
  action rejectMismatchedRecovery = all {
    state.status == RecoveryRequired,
    state.recoveryBinding == state.task,
    state' = state
      .with("status", Conflicted)
      .with("lastRecoveryAttemptDigest", state.task.requestDigest + 1)
      .with("recoveryRejects", state.recoveryRejects + 1),
  }

  action resumeAfterSavedPartial = all {
    state.status == SavedPartial,
    state.walBinding == state.task,
    state.approvedBinding == state.task,
    savedReceiptIsFresh(state),
    not(state.canonicalInitialized),
    state' = state.with("status", Bootstrapping),
  }

  action writeCanonicalBootstrap = all {
    state.status == Bootstrapping,
    state.walPrepared,
    state.walBinding == state.approvedBinding,
    state.approvedBinding == state.task,
    state.bootstrapWrites == 0,
    bootstrapSourceIsFresh(state),
    not(state.canonicalInitialized),
    state' = state
      .with("canonicalInitialized", true)
      .with("canonicalMatchesSource", true)
      .with("canonicalOwnerOperationId", state.task.operationId)
      .with("canonicalHead", state.task.canonicalRevision)
      .with("canonicalBinding", state.task)
      .with("bootstrapWrites", 1),
  }

  // Reopening an exact Reserved record with generation zero already present
  // yields BootstrapPublishedPartial, then marks the reservation Completed.
  action recoverCanonicalBootstrap = all {
    state.status == Bootstrapping,
    state.walPrepared,
    state.walBinding == state.task,
    state.approvedBinding == state.task,
    state.bootstrapWrites == 1,
    canonicalMatchesStagedPlan(state),
    state.canonicalOwnerOperationId == state.task.operationId,
    state.canonicalBinding == state.task,
    state' = state
      .with("status", BootstrapPartial)
      .with("bootstrapReceiptPresent", true)
      .with("bootstrapReceiptBinding", state.task),
  }

  action publishCompletion = all {
    state.status == BootstrapPartial,
    state.walBinding == state.task,
    state.approvedBinding == state.task,
    state.bootstrapReceiptPresent,
    state.bootstrapReceiptBinding == state.task,
    canonicalMatchesStagedPlan(state),
    state.canonicalOwnerOperationId == state.task.operationId,
    state.canonicalBinding == state.task,
    state.task.mode != SaveUntitled
      or all {
        state.saveReceiptPresent,
        state.saveReceiptBinding == state.task,
      },
    state' = state
      .with("status", Completed)
      .with("completionReceiptPresent", true)
      .with("completionBinding", state.task),
  }

  // Environment changes model user edits, switching the active YMM4 project,
  // destination races, and a winner between preview and reservation.
  action editActiveProject = all {
    any {
      state.status == Previewable,
      state.status == Approved,
      state.status == Saving,
      state.status == SavedPartial,
      state.status == RecoveryRequired,
      state.status == Bootstrapping,
    },
    state' = state.with("activeFingerprint", state.activeFingerprint + 1),
  }

  action switchActiveProject = all {
    any {
      state.status == Previewable,
      state.status == Approved,
      state.status == Saving,
      state.status == SavedPartial,
      state.status == RecoveryRequired,
      state.status == Bootstrapping,
    },
    state' = state
      .with("activeSessionId", state.activeSessionId + 1)
      .with("activeSaved", false)
      .with("activePathId", missing),
  }

  action occupySaveTarget = all {
    state.task.mode == SaveUntitled,
    not(state.saveTargetExists),
    state.saveWrites == 0,
    any {
      state.status == Previewable,
      state.status == Approved,
      state.status == Saving,
      state.status == RecoveryRequired,
    },
    state' = state
      .with("saveTargetExists", true)
      .with("saveTargetOwnerOperationId", externalOwner),
  }

  // A competitor may win only before this operation's reservation is durable.
  // An absent-at-stage plan can never reinterpret that winner as its own
  // AlreadyInitialized success.
  action initializeCanonicalElsewhere = all {
    not(state.canonicalInitialized),
    not(state.walPrepared),
    state.bootstrapWrites == 0,
    any { state.status == Previewable, state.status == Approved },
    state' = state
      .with("canonicalInitialized", true)
      .with("canonicalMatchesSource", true)
      .with("canonicalOwnerOperationId", externalOwner)
      .with("canonicalHead", state.task.canonicalRevision),
  }

  action advancePreexistingCanonical = all {
    state.task.canonicalPreexisting,
    state.canonicalInitialized,
    any { state.status == Previewable, state.status == Approved },
    state' = state
      .with("canonicalHead", state.canonicalHead + 1)
      .with("canonicalMatchesSource", false),
  }

  action markConflict = all {
    any {
      all {
        any { state.status == Previewable, state.status == Approved },
        not(previewPreconditionsAreFresh(state)),
      },
      all {
        state.status == Saving,
        not(saveCanProceedOrRecover(state)),
      },
      all {
        state.status == SavedPartial,
        not(all {
          savedReceiptIsFresh(state),
          not(state.canonicalInitialized),
        }),
      },
      all {
        state.status == RecoveryRequired,
        not(recoveryCanResume(state)),
      },
      all {
        state.status == Bootstrapping,
        not(bootstrapCanProceedOrRecover(state)),
      },
    },
    state' = state.with("status", Conflicted),
  }

  action retryTerminal = all {
    any { state.status == Completed, state.status == AlreadyInitialized },
    state' = state,
  }

  action stutterConflict = all {
    state.status == Conflicted,
    state' = state,
  }

  action step = any {
    makeActiveProjectSaved,
    makeMatchingCanonicalPreexisting,
    stageSaveUntitled,
    stageAdoptActive,
    refreshPreview,
    approve,
    reviseAfterApproval,
    prepareSaveExecution,
    prepareAdoptExecution,
    finishAdoptAlreadyInitialized,
    allocatePreparedPath,
    writePreparedFile,
    claimPreparedFile,
    rebindClaimedProject,
    publishSavedReceipt,
    markRecoveryRequired,
    resumeRecoveryBeforeClaim,
    resumeRecoveryByRebinding,
    resumeRecoveryAfterRebind,
    rejectMismatchedRecovery,
    resumeAfterSavedPartial,
    writeCanonicalBootstrap,
    recoverCanonicalBootstrap,
    publishCompletion,
    editActiveProject,
    switchActiveProject,
    occupySaveTarget,
    initializeCanonicalElsewhere,
    advancePreexistingCanonical,
    markConflict,
    retryTerminal,
    stutterConflict,
  }

  val initializationApprovalIsDigestBound =
    not(approvalMustRemainBound(state.status))
      or all {
        state.approvedBinding.requestDigest == state.task.requestDigest,
        state.approvedBinding.operationId == state.task.operationId,
      }

  val executionIsFullyApprovalBound =
    not(executionHasStarted(state.status))
      or all {
        state.approvedBinding == state.task,
        if (state.status == AlreadyInitialized)
          state.task.canonicalPreexisting
        else
          all {
            state.walPrepared,
            state.walBinding == state.task,
          },
      }

  val durableEffectsAreSourceBound = all {
    state.preparedEvidence == NoPreparedPath
      or state.preparedBinding == state.task,
    state.saveWrites == 0
      or all {
        state.task.mode == SaveUntitled,
        state.saveFileBinding == state.task,
        state.saveFileBinding.activeSessionId == state.task.activeSessionId,
        state.saveFileBinding.sourceFingerprint == state.task.sourceFingerprint,
      },
    state.bootstrapWrites == 0
      or all {
        state.canonicalBinding == state.task,
        state.canonicalBinding.activeSessionId == state.task.activeSessionId,
        state.canonicalBinding.sourceFingerprint == state.task.sourceFingerprint,
      },
  }

  // Consistency plus the transition relation proves the only evidence path is
  // NoPreparedPath -> PreparedPathOnly -> PreparedFileComplete.
  val preparedFileEvidenceIsMonotonic = all {
    state.temporaryWrites <= 1,
    state.preparedEvidence != NoPreparedPath
      or all { state.temporaryWrites == 0, state.saveWrites == 0 },
    state.preparedEvidence != PreparedPathOnly
      or all { state.temporaryWrites == 0, state.saveWrites == 0 },
    state.preparedEvidence != PreparedFileComplete
      or all {
        state.temporaryWrites == 1,
        state.preparedBinding == state.task,
      },
    state.saveWrites == 0
      or state.preparedEvidence == PreparedFileComplete,
  }

  val saveUntitledNeverOverwrites = all {
    state.saveWrites <= 1,
    state.saveWrites == 0
      or all {
        state.saveTargetExists,
        state.saveTargetOwnerOperationId == state.task.operationId,
        state.saveFileBinding == state.task,
      },
    state.saveTargetOwnerOperationId != externalOwner
      or state.saveWrites == 0,
  }

  val canonicalReservationFencesCompetitors =
    not(state.walPrepared)
      or not(state.canonicalInitialized)
      or state.canonicalOwnerOperationId == state.task.operationId

  val initializationIsExactlyOnce = all {
    state.saveWrites <= 1,
    state.bootstrapWrites <= 1,
    not(state.canonicalInitialized)
      or state.canonicalOwnerOperationId != state.task.operationId
      or all {
        state.bootstrapWrites == 1,
        state.canonicalBinding == state.task,
        state.canonicalHead == state.task.canonicalRevision,
      },
  }

  val recoveryResumeIsRequestBound = all {
    state.status != RecoveryRequired
      or all {
        state.recoveryBinding == state.task,
        state.approvedBinding == state.task,
        state.walBinding == state.task,
      },
    state.recoveryRejects == 0
      or all {
        state.lastRecoveryAttemptDigest != state.task.requestDigest,
        state.status == Conflicted,
      },
  }

  val partialProgressIsRequestBound = all {
    not(state.saveReceiptPresent)
      or all {
        state.saveWrites == 1,
        state.saveReceiptBinding == state.task,
        state.saveFileBinding == state.task,
        state.preparedBinding == state.task,
      },
    not(state.bootstrapReceiptPresent)
      or all {
        state.bootstrapWrites == 1,
        state.bootstrapReceiptBinding == state.task,
        state.canonicalBinding == state.task,
      },
    state.status != SavedPartial
      or all {
        state.saveReceiptPresent,
        state.saveReceiptBinding == state.task,
        state.bootstrapWrites == 0,
        not(state.completionReceiptPresent),
      },
    state.status != BootstrapPartial
      or all {
        state.bootstrapReceiptPresent,
        state.bootstrapReceiptBinding == state.task,
        not(state.completionReceiptPresent),
      },
  }

  val alreadyInitializedRequiresPreexistingPlan =
    state.status != AlreadyInitialized
      or all {
        state.task.mode == AdoptActive,
        state.task.canonicalPreexisting,
        canonicalMatchesStagedPlan(state),
        state.canonicalOwnerOperationId != state.task.operationId,
        state.saveWrites == 0,
        state.bootstrapWrites == 0,
        not(state.completionReceiptPresent),
      }

  val completedInitializationHasBothDurableSides = all {
    state.status != Completed
      or all {
        not(state.task.canonicalPreexisting),
        state.bootstrapWrites == 1,
        state.bootstrapReceiptPresent,
        state.bootstrapReceiptBinding == state.task,
        canonicalMatchesStagedPlan(state),
        state.canonicalOwnerOperationId == state.task.operationId,
        state.completionReceiptPresent,
        state.completionBinding == state.task,
        state.task.mode != SaveUntitled
          or all {
            state.saveWrites == 1,
            state.saveReceiptPresent,
            state.saveReceiptBinding == state.task,
          },
      },
    state.status != AlreadyInitialized
      or alreadyInitializedRequiresPreexistingPlan,
  }

  val initializationSafety = and {
    initializationApprovalIsDigestBound,
    executionIsFullyApprovalBound,
    durableEffectsAreSourceBound,
    preparedFileEvidenceIsMonotonic,
    saveUntitledNeverOverwrites,
    canonicalReservationFencesCompetitors,
    initializationIsExactlyOnce,
    recoveryResumeIsRequestBound,
    partialProgressIsRequestBound,
    alreadyInitializedRequiresPreexistingPlan,
    completedInitializationHasBothDurableSides,
  }

  run saveUntitledExactRetryTest = init
    .then(stageSaveUntitled)
    .then(approve)
    .then(prepareSaveExecution)
    .then(allocatePreparedPath)
    .then(writePreparedFile)
    .then(claimPreparedFile)
    .then(rebindClaimedProject)
    .then(publishSavedReceipt)
    .then(resumeAfterSavedPartial)
    .then(writeCanonicalBootstrap)
    .then(recoverCanonicalBootstrap)
    .then(publishCompletion)
    .then(retryTerminal)
    .expect(all {
      initializationSafety,
      state.status == Completed,
      state.temporaryWrites == 1,
      state.saveWrites == 1,
      state.rebindWrites == 1,
      state.bootstrapWrites == 1,
    })

  run adoptActiveExactRetryTest = init
    .then(makeActiveProjectSaved)
    .then(stageAdoptActive)
    .then(approve)
    .then(prepareAdoptExecution)
    .then(writeCanonicalBootstrap)
    .then(recoverCanonicalBootstrap)
    .then(publishCompletion)
    .then(retryTerminal)
    .expect(all {
      initializationSafety,
      state.status == Completed,
      not(state.task.canonicalPreexisting),
      state.saveWrites == 0,
      state.bootstrapWrites == 1,
    })

  run adoptPreexistingReturnsAlreadyInitializedTest = init
    .then(makeActiveProjectSaved)
    .then(makeMatchingCanonicalPreexisting)
    .then(stageAdoptActive)
    .then(approve)
    .then(finishAdoptAlreadyInitialized)
    .then(retryTerminal)
    .expect(all {
      initializationSafety,
      state.status == AlreadyInitialized,
      state.task.canonicalPreexisting,
      state.saveWrites == 0,
      state.bootstrapWrites == 0,
    })

  run adoptAbsentAtStageLosesRaceTest = init
    .then(makeActiveProjectSaved)
    .then(stageAdoptActive)
    .then(approve)
    .then(initializeCanonicalElsewhere)
    .then(markConflict)
    .expect(all {
      initializationSafety,
      state.status == Conflicted,
      not(state.task.canonicalPreexisting),
      state.canonicalOwnerOperationId == externalOwner,
      state.bootstrapWrites == 0,
    })

  run preparedEvidenceProgressionTest = init
    .then(stageSaveUntitled)
    .then(approve)
    .then(prepareSaveExecution)
    .then(allocatePreparedPath)
    .then(writePreparedFile)
    .expect(all {
      initializationSafety,
      state.status == Saving,
      state.preparedEvidence == PreparedFileComplete,
      state.temporaryWrites == 1,
      state.saveWrites == 0,
    })

  run destinationRaceDoesNotOverwriteTest = init
    .then(stageSaveUntitled)
    .then(approve)
    .then(prepareSaveExecution)
    .then(allocatePreparedPath)
    .then(writePreparedFile)
    .then(occupySaveTarget)
    .then(markConflict)
    .expect(all {
      initializationSafety,
      state.status == Conflicted,
      state.preparedEvidence == PreparedFileComplete,
      state.temporaryWrites == 1,
      state.saveWrites == 0,
      state.bootstrapWrites == 0,
      state.saveTargetOwnerOperationId == externalOwner,
    })

  run recoveryAfterClaimRebindsExactlyTest = init
    .then(stageSaveUntitled)
    .then(approve)
    .then(prepareSaveExecution)
    .then(allocatePreparedPath)
    .then(writePreparedFile)
    .then(claimPreparedFile)
    .then(markRecoveryRequired)
    .then(resumeRecoveryByRebinding)
    .then(resumeAfterSavedPartial)
    .then(writeCanonicalBootstrap)
    .then(recoverCanonicalBootstrap)
    .then(publishCompletion)
    .expect(all {
      initializationSafety,
      state.status == Completed,
      state.recoveryBinding == state.task,
      state.rebindWrites == 1,
      state.saveWrites == 1,
      state.bootstrapWrites == 1,
    })

  run mismatchedRecoveryCannotRebindTest = init
    .then(stageSaveUntitled)
    .then(approve)
    .then(prepareSaveExecution)
    .then(allocatePreparedPath)
    .then(writePreparedFile)
    .then(claimPreparedFile)
    .then(markRecoveryRequired)
    .then(rejectMismatchedRecovery)
    .expect(all {
      initializationSafety,
      state.status == Conflicted,
      state.recoveryRejects == 1,
      state.rebindWrites == 0,
      not(state.activeSaved),
      not(state.saveReceiptPresent),
      state.saveWrites == 1,
    })

  run sourceDriftBlocksExecutionTest = init
    .then(makeActiveProjectSaved)
    .then(stageAdoptActive)
    .then(approve)
    .then(editActiveProject)
    .then(markConflict)
    .expect(all {
      initializationSafety,
      state.status == Conflicted,
      state.bootstrapWrites == 0,
      not(state.canonicalInitialized),
    })

  run preexistingHeadDriftIsNotAlreadyInitializedTest = init
    .then(makeActiveProjectSaved)
    .then(makeMatchingCanonicalPreexisting)
    .then(stageAdoptActive)
    .then(approve)
    .then(advancePreexistingCanonical)
    .then(markConflict)
    .expect(all {
      initializationSafety,
      state.status == Conflicted,
      state.task.canonicalPreexisting,
      state.status != AlreadyInitialized,
      state.bootstrapWrites == 0,
    })

  run revisedDigestRequiresNewApprovalTest = init
    .then(stageSaveUntitled)
    .then(approve)
    .then(reviseAfterApproval)
    .expect(all {
      initializationSafety,
      state.status == Previewable,
      state.approvedBinding.requestDigest != state.task.requestDigest,
      state.saveWrites == 0,
      state.bootstrapWrites == 0,
    })

  run canonicalPublicationPartialReplaysTest = init
    .then(stageSaveUntitled)
    .then(approve)
    .then(prepareSaveExecution)
    .then(allocatePreparedPath)
    .then(writePreparedFile)
    .then(claimPreparedFile)
    .then(rebindClaimedProject)
    .then(publishSavedReceipt)
    .then(resumeAfterSavedPartial)
    .then(writeCanonicalBootstrap)
    .then(recoverCanonicalBootstrap)
    .then(publishCompletion)
    .then(retryTerminal)
    .expect(all {
      initializationSafety,
      state.status == Completed,
      state.saveWrites == 1,
      state.bootstrapWrites == 1,
      state.completionReceiptPresent,
    })
}
