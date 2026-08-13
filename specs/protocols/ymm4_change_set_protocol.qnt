// YMM4 multi-edit change-set protocol.
//
// The model uses two heterogeneous operations, which is the smallest batch
// that exposes preparation and application ordering bugs. Materialization may
// finish in either order. The sealed plan always applies operation one before
// operation two under one writer reservation, verifies readback for every
// operation, and publishes exactly one canonical revision.
//
// A durable request-bound WAL records partial progress. Exact retries resume
// at the first unapplied operation; a failed rollback retains the reservation
// as RecoveryRequired and can only resume with the same approved binding.
module ymm4ChangeSetProtocol {
  type ChangeSetStatus =
    | Draft
    | Preparing
    | Previewable
    | Approved
    | Applying
    | Verifying
    | RollingBack
    | RecoveryRequired
    | Verified
    | Committed
    | RolledBack
    | Conflicted

  type RecoveryMode =
    | NoRecovery
    | ResumeApply
    | ResumeRollback

  type ChangeSetBinding = {
    operationId: int,
    requestDigest: int,
    baseRevision: int,
    targetFingerprint: int,
    operationCount: int,
    operationOrderDigest: int,
  }

  type ChangeSetState = {
    status: ChangeSetStatus,

    headRevision: int,
    externalCommitCount: int,
    targetFingerprint: int,

    preparationClock: int,
    operationOnePrepared: bool,
    operationTwoPrepared: bool,
    operationOnePreparationOrder: int,
    operationTwoPreparationOrder: int,

    writerOwner: int,
    reservationOwner: int,
    walPresent: bool,
    targetAppliedPrefix: int,
    operationOneWrites: int,
    operationTwoWrites: int,
    operationOneApplyOrder: int,
    operationTwoApplyOrder: int,

    operationOneReadBack: bool,
    operationTwoReadBack: bool,
    canonicalCommitCount: int,
    replayCount: int,
    rollbackWrites: int,
    recoveryMode: RecoveryMode,
    recoveryRejects: int,
    lastRecoveryAttemptDigest: int,

    task: ChangeSetBinding,
    approvedBinding: ChangeSetBinding,
    walBinding: ChangeSetBinding,
    recoveryBinding: ChangeSetBinding,
    commitBinding: ChangeSetBinding,
  }

  val missing = -1
  val noOwner = 0
  val taskOwner = 1
  val initialRevision = 7
  val operationCount = 2
  val canonicalOperationOrderDigest = 12

  val emptyBinding = {
    operationId: missing,
    requestDigest: missing,
    baseRevision: missing,
    targetFingerprint: missing,
    operationCount: missing,
    operationOrderDigest: missing,
  }

  var state: ChangeSetState

  pure def approvalMustRemainBound(status: ChangeSetStatus): bool =
    any {
      status == Approved,
      status == Applying,
      status == Verifying,
      status == RollingBack,
      status == RecoveryRequired,
      status == Verified,
      status == Committed,
      status == RolledBack,
      status == Conflicted,
    }

  pure def executionHasStarted(status: ChangeSetStatus): bool =
    any {
      status == Applying,
      status == Verifying,
      status == RollingBack,
      status == RecoveryRequired,
      status == Verified,
      status == Committed,
      status == RolledBack,
    }

  pure def targetIsFresh(s: ChangeSetState): bool =
    all {
      s.headRevision == s.task.baseRevision,
      s.targetFingerprint == s.task.targetFingerprint,
    }

  pure def bothPrepared(s: ChangeSetState): bool =
    all {
      s.operationOnePrepared,
      s.operationTwoPrepared,
    }

  pure def allReadBack(s: ChangeSetState): bool =
    all {
      s.operationOneReadBack,
      s.operationTwoReadBack,
    }

  action init =
    state' = {
      status: Draft,

      headRevision: initialRevision,
      externalCommitCount: 0,
      targetFingerprint: 70,

      preparationClock: 0,
      operationOnePrepared: false,
      operationTwoPrepared: false,
      operationOnePreparationOrder: 0,
      operationTwoPreparationOrder: 0,

      writerOwner: noOwner,
      reservationOwner: noOwner,
      walPresent: false,
      targetAppliedPrefix: 0,
      operationOneWrites: 0,
      operationTwoWrites: 0,
      operationOneApplyOrder: 0,
      operationTwoApplyOrder: 0,

      operationOneReadBack: false,
      operationTwoReadBack: false,
      canonicalCommitCount: 0,
      replayCount: 0,
      rollbackWrites: 0,
      recoveryMode: NoRecovery,
      recoveryRejects: 0,
      lastRecoveryAttemptDigest: missing,

      task: {
        operationId: taskOwner,
        requestDigest: 0,
        baseRevision: initialRevision,
        targetFingerprint: 70,
        operationCount: operationCount,
        operationOrderDigest: canonicalOperationOrderDigest,
      },
      approvedBinding: emptyBinding,
      walBinding: emptyBinding,
      recoveryBinding: emptyBinding,
      commitBinding: emptyBinding,
    }

  action prepareOperationOne = all {
    any { state.status == Draft, state.status == Preparing },
    not(state.operationOnePrepared),
    state' = state
      .with("status", Preparing)
      .with("preparationClock", state.preparationClock + 1)
      .with("operationOnePrepared", true)
      .with("operationOnePreparationOrder", state.preparationClock + 1),
  }

  action prepareOperationTwo = all {
    any { state.status == Draft, state.status == Preparing },
    not(state.operationTwoPrepared),
    state' = state
      .with("status", Preparing)
      .with("preparationClock", state.preparationClock + 1)
      .with("operationTwoPrepared", true)
      .with("operationTwoPreparationOrder", state.preparationClock + 1),
  }

  // Sealing computes one digest from canonical operation order. It is
  // deliberately independent of materialization completion order.
  action sealPreview = all {
    state.status == Preparing,
    bothPrepared(state),
    state' = state
      .with("status", Previewable)
      .with("task", state.task
        .with("requestDigest", state.task.requestDigest + 1)
        .with("baseRevision", state.headRevision)
        .with("targetFingerprint", state.targetFingerprint)
        .with("operationCount", operationCount)
        .with("operationOrderDigest", canonicalOperationOrderDigest)),
  }

  action approve = all {
    state.status == Previewable,
    bothPrepared(state),
    targetIsFresh(state),
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

  // Another writer may commit only before this change set owns the gate.
  // The approved base and target fingerprint then become stale.
  action externalCommitBeforeApply = all {
    state.status == Approved,
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("headRevision", state.headRevision + 1)
      .with("externalCommitCount", state.externalCommitCount + 1)
      .with("targetFingerprint", state.targetFingerprint + 1),
  }

  action markConflict = all {
    state.status == Approved,
    not(targetIsFresh(state)),
    state' = state.with("status", Conflicted),
  }

  // The durable reservation and WAL are acquired before any target mutation.
  action acquireWriter = all {
    state.status == Approved,
    state.task == state.approvedBinding,
    targetIsFresh(state),
    state.writerOwner == noOwner,
    state.reservationOwner == noOwner,
    state' = state
      .with("status", Applying)
      .with("writerOwner", state.task.operationId)
      .with("reservationOwner", state.task.operationId)
      .with("walPresent", true)
      .with("walBinding", state.task),
  }

  // Application order is canonical even when operation two prepared first.
  action applyOperationOne = all {
    state.status == Applying,
    state.writerOwner == state.task.operationId,
    state.reservationOwner == state.task.operationId,
    state.walPresent,
    state.walBinding == state.task,
    state.targetAppliedPrefix == 0,
    state.operationOneWrites == 0,
    state' = state
      .with("targetAppliedPrefix", 1)
      .with("operationOneWrites", 1)
      .with("operationOneApplyOrder", 1),
  }

  action applyOperationTwo = all {
    state.status == Applying,
    state.writerOwner == state.task.operationId,
    state.reservationOwner == state.task.operationId,
    state.walPresent,
    state.walBinding == state.task,
    state.targetAppliedPrefix == 1,
    state.operationOneWrites == 1,
    state.operationTwoWrites == 0,
    state' = state
      .with("status", Verifying)
      .with("targetAppliedPrefix", 2)
      .with("operationTwoWrites", 1)
      .with("operationTwoApplyOrder", 2),
  }

  // A crash persists the applied prefix and exact binding. Re-acquisition
  // therefore continues at operation two instead of replaying operation one.
  action crashAfterPartialApply = all {
    state.status == Applying,
    state.targetAppliedPrefix == 1,
    state.writerOwner == state.task.operationId,
    state.reservationOwner == state.task.operationId,
    state' = state
      .with("status", RecoveryRequired)
      .with("writerOwner", noOwner)
      .with("recoveryMode", ResumeApply)
      .with("recoveryBinding", state.task),
  }

  action resumeExactApply = all {
    state.status == RecoveryRequired,
    state.recoveryMode == ResumeApply,
    state.recoveryBinding == state.task,
    state.approvedBinding == state.task,
    state.walBinding == state.task,
    state.writerOwner == noOwner,
    state.reservationOwner == state.task.operationId,
    state' = state
      .with("status", Applying)
      .with("writerOwner", state.task.operationId)
      .with("recoveryMode", NoRecovery),
  }

  action readBackOperationOne = all {
    state.status == Verifying,
    state.writerOwner == state.task.operationId,
    state.targetAppliedPrefix == operationCount,
    not(state.operationOneReadBack),
    state' = state.with("operationOneReadBack", true),
  }

  action readBackOperationTwo = all {
    state.status == Verifying,
    state.writerOwner == state.task.operationId,
    state.targetAppliedPrefix == operationCount,
    not(state.operationTwoReadBack),
    state' = state.with("operationTwoReadBack", true),
  }

  action acceptReadBack = all {
    state.status == Verifying,
    allReadBack(state),
    state.targetAppliedPrefix == operationCount,
    state' = state.with("status", Verified),
  }

  action beginRollbackAfterPartialApply = all {
    state.status == Applying,
    state.targetAppliedPrefix == 1,
    state.writerOwner == state.task.operationId,
    state' = state.with("status", RollingBack),
  }

  // A semantic mismatch is decided only after every operation was read back.
  action beginRollbackAfterReadBackMismatch = all {
    state.status == Verifying,
    allReadBack(state),
    state.targetAppliedPrefix == operationCount,
    state.writerOwner == state.task.operationId,
    state' = state.with("status", RollingBack),
  }

  action rollBackOperationTwo = all {
    state.status == RollingBack,
    state.writerOwner == state.task.operationId,
    state.targetAppliedPrefix == 2,
    state' = state
      .with("targetAppliedPrefix", 1)
      .with("rollbackWrites", state.rollbackWrites + 1),
  }

  action rollBackOperationOne = all {
    state.status == RollingBack,
    state.writerOwner == state.task.operationId,
    state.targetAppliedPrefix == 1,
    state' = state
      .with("targetAppliedPrefix", 0)
      .with("rollbackWrites", state.rollbackWrites + 1),
  }

  action finishRollback = all {
    state.status == RollingBack,
    state.targetAppliedPrefix == 0,
    state.writerOwner == state.task.operationId,
    state.reservationOwner == state.task.operationId,
    state' = state
      .with("status", RolledBack)
      .with("writerOwner", noOwner)
      .with("reservationOwner", noOwner)
      .with("recoveryMode", NoRecovery),
  }

  // Failed rollback releases only the process-local writer. The durable
  // reservation remains fail-closed until exact recovery completes.
  action failRollback = all {
    state.status == RollingBack,
    state.targetAppliedPrefix > 0,
    state.writerOwner == state.task.operationId,
    state.reservationOwner == state.task.operationId,
    state' = state
      .with("status", RecoveryRequired)
      .with("writerOwner", noOwner)
      .with("recoveryMode", ResumeRollback)
      .with("recoveryBinding", state.task),
  }

  action resumeExactRollback = all {
    state.status == RecoveryRequired,
    state.recoveryMode == ResumeRollback,
    state.recoveryBinding == state.task,
    state.approvedBinding == state.task,
    state.walBinding == state.task,
    state.writerOwner == noOwner,
    state.reservationOwner == state.task.operationId,
    state' = state
      .with("status", RollingBack)
      .with("writerOwner", state.task.operationId)
      .with("recoveryMode", NoRecovery),
  }

  action rejectMismatchedRecovery = all {
    state.status == RecoveryRequired,
    state.recoveryBinding == state.task,
    state' = state
      .with("recoveryRejects", state.recoveryRejects + 1)
      .with("lastRecoveryAttemptDigest", state.task.requestDigest + 1),
  }

  // One verified batch creates one canonical revision, never one per edit.
  action commitChangeSet = all {
    state.status == Verified,
    state.task == state.approvedBinding,
    state.walBinding == state.task,
    state.writerOwner == state.task.operationId,
    state.reservationOwner == state.task.operationId,
    allReadBack(state),
    state.canonicalCommitCount == 0,
    state.headRevision == state.task.baseRevision,
    state' = state
      .with("status", Committed)
      .with("headRevision", state.headRevision + 1)
      .with("canonicalCommitCount", 1)
      .with("commitBinding", state.task)
      .with("writerOwner", noOwner)
      .with("reservationOwner", noOwner),
  }

  // Retrying the exact terminal request returns the same result. It performs
  // no YMM4 mutation and does not advance the canonical revision again.
  action replayCommitted = all {
    state.status == Committed,
    state.commitBinding == state.task,
    state' = state.with("replayCount", state.replayCount + 1),
  }

  action stutterTerminal = all {
    any {
      state.status == RolledBack,
      state.status == Conflicted,
    },
    state' = state,
  }

  action step = any {
    prepareOperationOne,
    prepareOperationTwo,
    sealPreview,
    approve,
    reviseAfterApproval,
    externalCommitBeforeApply,
    markConflict,
    acquireWriter,
    applyOperationOne,
    applyOperationTwo,
    crashAfterPartialApply,
    resumeExactApply,
    readBackOperationOne,
    readBackOperationTwo,
    acceptReadBack,
    beginRollbackAfterPartialApply,
    beginRollbackAfterReadBackMismatch,
    rollBackOperationTwo,
    rollBackOperationOne,
    finishRollback,
    failRollback,
    resumeExactRollback,
    rejectMismatchedRecovery,
    commitChangeSet,
    replayCommitted,
    stutterTerminal,
  }

  val approvalIsExact =
    not(approvalMustRemainBound(state.status))
      or state.approvedBinding == state.task

  val previewContainsWholeOrderedBatch =
    not(any {
      state.status == Previewable,
      approvalMustRemainBound(state.status),
    })
      or all {
        bothPrepared(state),
        state.task.operationCount == operationCount,
        state.task.operationOrderDigest == canonicalOperationOrderDigest,
        state.operationOnePreparationOrder > 0,
        state.operationTwoPreparationOrder > 0,
        state.operationOnePreparationOrder != state.operationTwoPreparationOrder,
      }

  val applicationUsesCanonicalOrder = all {
    state.operationOneWrites <= 1,
    state.operationTwoWrites <= 1,
    state.operationOneApplyOrder == 0 or state.operationOneApplyOrder == 1,
    state.operationTwoApplyOrder == 0 or state.operationTwoApplyOrder == 2,
    state.operationTwoWrites == 0
      or all {
        state.operationOneWrites == 1,
        state.operationOneApplyOrder == 1,
        state.operationTwoApplyOrder == 2,
      },
  }

  val preparationCompletionDoesNotChooseCanonicalOrder = all {
    previewContainsWholeOrderedBatch,
    applicationUsesCanonicalOrder,
  }

  val walPrecedesMutation =
    state.operationOneWrites + state.operationTwoWrites == 0
      or all {
        state.walPresent,
        state.walBinding == state.task,
        state.approvedBinding == state.task,
      }

  val writerIsExclusive = all {
    state.writerOwner == noOwner or state.writerOwner == state.task.operationId,
    state.writerOwner == state.task.operationId
      or not(any {
        state.status == Applying,
        state.status == Verifying,
        state.status == RollingBack,
        state.status == Verified,
      }),
    not(executionHasStarted(state.status))
      or state.reservationOwner == state.task.operationId
      or any {
        state.status == Committed,
        state.status == RolledBack,
      },
  }

  val staleOrConflictingPlanNeverApplies =
    state.status != Conflicted
      or all {
        state.operationOneWrites == 0,
        state.operationTwoWrites == 0,
        state.canonicalCommitCount == 0,
        state.writerOwner == noOwner,
        state.reservationOwner == noOwner,
      }

  val unresolvedPartialApplyFailsClosed =
    state.status != RecoveryRequired
      or all {
        state.targetAppliedPrefix > 0,
        state.writerOwner == noOwner,
        state.reservationOwner == state.task.operationId,
        state.recoveryBinding == state.task,
        state.walBinding == state.task,
        state.canonicalCommitCount == 0,
        state.headRevision == state.task.baseRevision,
        state.recoveryRejects == 0
          or state.lastRecoveryAttemptDigest != state.task.requestDigest,
      }

  val rollbackDoesNotPublish = all {
    state.status != RolledBack
      or all {
        state.targetAppliedPrefix == 0,
        state.canonicalCommitCount == 0,
        state.writerOwner == noOwner,
        state.reservationOwner == noOwner,
      },
    state.rollbackWrites <= operationCount,
  }

  val allReadBackBeforeCanonicalCommit =
    state.canonicalCommitCount == 0
      or all {
        allReadBack(state),
        state.targetAppliedPrefix == operationCount,
        state.operationOneWrites == 1,
        state.operationTwoWrites == 1,
        state.commitBinding == state.task,
      }

  val oneBatchCreatesOneRevision = all {
    state.canonicalCommitCount <= 1,
    state.headRevision
      == initialRevision + state.externalCommitCount + state.canonicalCommitCount,
    state.status != Committed
      or all {
        state.canonicalCommitCount == 1,
        state.headRevision == state.task.baseRevision + 1,
      },
  }

  val exactReplayIsAtMostOnce = all {
    state.operationOneWrites <= 1,
    state.operationTwoWrites <= 1,
    state.canonicalCommitCount <= 1,
    state.replayCount == 0
      or all {
        state.status == Committed,
        state.commitBinding == state.task,
        state.canonicalCommitCount == 1,
      },
  }

  val changeSetSafety = and {
    approvalIsExact,
    preparationCompletionDoesNotChooseCanonicalOrder,
    walPrecedesMutation,
    writerIsExclusive,
    staleOrConflictingPlanNeverApplies,
    unresolvedPartialApplyFailsClosed,
    rollbackDoesNotPublish,
    allReadBackBeforeCanonicalCommit,
    oneBatchCreatesOneRevision,
    exactReplayIsAtMostOnce,
  }

  run reversePreparationStillAppliesCanonicalOrderTest = init
    .then(prepareOperationTwo)
    .then(prepareOperationOne)
    .then(sealPreview)
    .then(approve)
    .then(acquireWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(readBackOperationTwo)
    .then(readBackOperationOne)
    .then(acceptReadBack)
    .then(commitChangeSet)
    .then(replayCommitted)
    .then(replayCommitted)
    .expect(all {
      changeSetSafety,
      state.status == Committed,
      state.operationTwoPreparationOrder == 1,
      state.operationOnePreparationOrder == 2,
      state.operationOneApplyOrder == 1,
      state.operationTwoApplyOrder == 2,
      state.canonicalCommitCount == 1,
      state.headRevision == initialRevision + 1,
      state.replayCount == 2,
    })

  run forwardPreparationAlsoAppliesCanonicalOrderTest = init
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(acquireWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .expect(all {
      changeSetSafety,
      state.status == Verifying,
      state.operationOnePreparationOrder == 1,
      state.operationTwoPreparationOrder == 2,
      state.operationOneApplyOrder == 1,
      state.operationTwoApplyOrder == 2,
      state.canonicalCommitCount == 0,
    })

  run staleApprovalConflictsBeforeApplyTest = init
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(externalCommitBeforeApply)
    .then(markConflict)
    .expect(all {
      changeSetSafety,
      state.status == Conflicted,
      state.operationOneWrites == 0,
      state.operationTwoWrites == 0,
      state.canonicalCommitCount == 0,
      state.externalCommitCount == 1,
    })

  run partialApplyRollsBackWithoutRevisionTest = init
    .then(prepareOperationTwo)
    .then(prepareOperationOne)
    .then(sealPreview)
    .then(approve)
    .then(acquireWriter)
    .then(applyOperationOne)
    .then(beginRollbackAfterPartialApply)
    .then(rollBackOperationOne)
    .then(finishRollback)
    .expect(all {
      changeSetSafety,
      state.status == RolledBack,
      state.targetAppliedPrefix == 0,
      state.rollbackWrites == 1,
      state.canonicalCommitCount == 0,
      state.headRevision == initialRevision,
    })

  run rollbackFailureRetainsExactReservationTest = init
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(acquireWriter)
    .then(applyOperationOne)
    .then(beginRollbackAfterPartialApply)
    .then(failRollback)
    .then(rejectMismatchedRecovery)
    .expect(all {
      changeSetSafety,
      state.status == RecoveryRequired,
      state.writerOwner == noOwner,
      state.reservationOwner == state.task.operationId,
      state.recoveryBinding == state.task,
      state.recoveryRejects == 1,
      state.lastRecoveryAttemptDigest != state.task.requestDigest,
      state.canonicalCommitCount == 0,
    })

  run recoveredPartialApplyDoesNotReplayFirstOperationTest = init
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(acquireWriter)
    .then(applyOperationOne)
    .then(crashAfterPartialApply)
    .then(resumeExactApply)
    .then(applyOperationTwo)
    .then(readBackOperationOne)
    .then(readBackOperationTwo)
    .then(acceptReadBack)
    .then(commitChangeSet)
    .then(replayCommitted)
    .expect(all {
      changeSetSafety,
      state.status == Committed,
      state.operationOneWrites == 1,
      state.operationTwoWrites == 1,
      state.canonicalCommitCount == 1,
      state.replayCount == 1,
    })

  run oneReadBackCannotAdvanceRevisionTest = init
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(acquireWriter)
    .then(applyOperationOne)
    .then(applyOperationTwo)
    .then(readBackOperationOne)
    .expect(all {
      changeSetSafety,
      state.status == Verifying,
      state.operationOneReadBack,
      not(state.operationTwoReadBack),
      state.canonicalCommitCount == 0,
      state.headRevision == initialRevision,
    })

  run revisedDigestRequiresNewApprovalTest = init
    .then(prepareOperationOne)
    .then(prepareOperationTwo)
    .then(sealPreview)
    .then(approve)
    .then(reviseAfterApproval)
    .expect(all {
      changeSetSafety,
      state.status == Previewable,
      state.task.requestDigest != state.approvedBinding.requestDigest,
      state.operationOneWrites == 0,
      state.operationTwoWrites == 0,
      state.canonicalCommitCount == 0,
    })
}
