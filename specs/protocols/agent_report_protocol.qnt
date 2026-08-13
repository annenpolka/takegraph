// Agent-visible approval report.
// This module specifies which tokens an adapter must expose after stage
// so a caller that cannot see hidden structured state can still commit.
// It does not specify wording, MCP hosts, UI, media, or OS behavior.
module agentReportProtocol {
  type Store =
    | StudioSession
    | CanonicalProject

  type TakeReadiness =
    | Ready
    | QueryReady

  type NextAction =
    | NeedStage
    | NeedStudioCommit
    | NeedCanonicalCommit
    | NeedVerify
    | Blocked

  type Report = {
    visibleIdentity: int,
    visibleDigest: int,
    store: Store,
    namesStore: bool,
    isStatus: bool,
    next: NextAction,
    reportedStudioHead: int,
    reportedCanonicalHead: int,
    reportedUnsaved: bool,
    reportedHasTargetLink: bool,
    reportedTakeReady: bool,
  }

  type Task = {
    present: bool,
    identity: int,
    digest: int,
    store: Store,
    base: int,
  }

  var studioHead: int
  var canonicalHead: int
  var takeReadiness: TakeReadiness
  var hasTargetLink: bool
  var unsaved: bool
  var task: Task
  var report: Report
  var hasCommitted: bool
  var lastCommittedStore: Store

  val missing = -1

  val emptyTask = {
    present: false,
    identity: missing,
    digest: missing,
    store: StudioSession,
    base: missing,
  }

  val initReport = {
    visibleIdentity: missing,
    visibleDigest: missing,
    store: StudioSession,
    namesStore: true,
    isStatus: true,
    next: NeedStage,
    reportedStudioHead: 0,
    reportedCanonicalHead: 0,
    reportedUnsaved: true,
    reportedHasTargetLink: false,
    reportedTakeReady: true,
  }

  def nextForCurrentTask: NextAction = {
    if (task.present and task.store == StudioSession)
      NeedStudioCommit
    else if (task.present and task.store == CanonicalProject)
      NeedCanonicalCommit
    else
      NeedStage
  }

  def studioInventory(nextAction: NextAction): Report = {
    visibleIdentity: task.identity,
    visibleDigest: task.digest,
    store: if (task.present) task.store else StudioSession,
    namesStore: true,
    isStatus: false,
    next: nextAction,
    reportedStudioHead: studioHead,
    reportedCanonicalHead: canonicalHead,
    reportedUnsaved: unsaved,
    reportedHasTargetLink: hasTargetLink,
    reportedTakeReady: takeReadiness == Ready,
  }

  def canonicalInventory(nextAction: NextAction): Report = {
    visibleIdentity: task.identity,
    visibleDigest: task.digest,
    store: if (task.present) task.store else CanonicalProject,
    namesStore: true,
    isStatus: false,
    next: nextAction,
    reportedStudioHead: studioHead,
    reportedCanonicalHead: canonicalHead,
    reportedUnsaved: unsaved,
    reportedHasTargetLink: hasTargetLink,
    reportedTakeReady: takeReadiness == Ready,
  }

  def statusInventory(nextAction: NextAction): Report = {
    visibleIdentity: task.identity,
    visibleDigest: task.digest,
    store: if (task.present) task.store else CanonicalProject,
    namesStore: true,
    isStatus: true,
    next: nextAction,
    reportedStudioHead: studioHead,
    reportedCanonicalHead: canonicalHead,
    reportedUnsaved: unsaved,
    reportedHasTargetLink: hasTargetLink,
    reportedTakeReady: takeReadiness == Ready,
  }

  action init = all {
    studioHead' = 0,
    canonicalHead' = 0,
    takeReadiness' = Ready,
    hasTargetLink' = false,
    unsaved' = true,
    task' = emptyTask,
    report' = initReport,
    hasCommitted' = false,
    lastCommittedStore' = StudioSession,
  }

  action observeStudio = all {
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = studioInventory(nextForCurrentTask),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action observeCanonical = all {
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = if (task.present and task.store == StudioSession)
      studioInventory(NeedStudioCommit)
    else
      canonicalInventory(
        if (unsaved)
          Blocked
        else
          nextForCurrentTask
      ),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action observeStatus = all {
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = statusInventory(
      if (task.present and task.store == StudioSession)
        NeedStudioCommit
      else if (unsaved)
        Blocked
      else
        nextForCurrentTask
    ),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action generateQueryReady = all {
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = QueryReady,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = studioInventory(Blocked),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action restoreReadyTake = all {
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = Ready,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = studioInventory(NeedStage),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action stageStudio = all {
    takeReadiness == Ready,
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = {
      present: true,
      identity: studioHead + 1,
      digest: studioHead + 10,
      store: StudioSession,
      base: studioHead,
    },
    report' = {
      visibleIdentity: studioHead + 1,
      visibleDigest: studioHead + 10,
      store: StudioSession,
      namesStore: true,
      isStatus: false,
      next: NeedStudioCommit,
      reportedStudioHead: studioHead,
      reportedCanonicalHead: canonicalHead,
      reportedUnsaved: unsaved,
      reportedHasTargetLink: hasTargetLink,
      reportedTakeReady: true,
    },
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action refuseQueryReadyStage = all {
    takeReadiness == QueryReady,
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = studioInventory(Blocked),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action refuseCanonicalWhenUnsaved = all {
    unsaved,
    not(task.present and task.store == StudioSession),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = canonicalInventory(Blocked),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action stageCanonical = all {
    not(unsaved),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = {
      present: true,
      identity: canonicalHead + 1,
      digest: canonicalHead + 20,
      store: CanonicalProject,
      base: canonicalHead,
    },
    report' = {
      visibleIdentity: canonicalHead + 1,
      visibleDigest: canonicalHead + 20,
      store: CanonicalProject,
      namesStore: true,
      isStatus: false,
      next: NeedCanonicalCommit,
      reportedStudioHead: studioHead,
      reportedCanonicalHead: canonicalHead,
      reportedUnsaved: unsaved,
      reportedHasTargetLink: hasTargetLink,
      reportedTakeReady: takeReadiness == Ready,
    },
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  val reportAuthorizesTask =
    and {
      task.present,
      report.visibleIdentity == task.identity,
      report.visibleDigest == task.digest,
      report.visibleIdentity != missing,
      report.visibleDigest != missing,
      report.store == task.store,
    }

  action commitStudio = all {
    reportAuthorizesTask,
    task.store == StudioSession,
    task.base == studioHead,
    studioHead' = studioHead + 1,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = emptyTask,
    report' = {
      visibleIdentity: missing,
      visibleDigest: missing,
      store: StudioSession,
      namesStore: true,
      isStatus: false,
      next: NeedStage,
      reportedStudioHead: studioHead + 1,
      reportedCanonicalHead: canonicalHead,
      reportedUnsaved: unsaved,
      reportedHasTargetLink: hasTargetLink,
      reportedTakeReady: takeReadiness == Ready,
    },
    hasCommitted' = true,
    lastCommittedStore' = StudioSession,
  }

  action commitCanonical = all {
    not(unsaved),
    reportAuthorizesTask,
    task.store == CanonicalProject,
    task.base == canonicalHead,
    studioHead' = studioHead,
    canonicalHead' = canonicalHead + 1,
    takeReadiness' = takeReadiness,
    hasTargetLink' = true,
    unsaved' = unsaved,
    task' = emptyTask,
    report' = {
      visibleIdentity: missing,
      visibleDigest: missing,
      store: CanonicalProject,
      namesStore: true,
      isStatus: false,
      next: NeedVerify,
      reportedStudioHead: studioHead,
      reportedCanonicalHead: canonicalHead + 1,
      reportedUnsaved: unsaved,
      reportedHasTargetLink: true,
      reportedTakeReady: takeReadiness == Ready,
    },
    hasCommitted' = true,
    lastCommittedStore' = CanonicalProject,
  }

  action refuseReconcileWithoutLink = all {
    not(hasTargetLink),
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = canonicalInventory(Blocked),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action reconcile = all {
    hasTargetLink,
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = canonicalInventory(
      if (unsaved)
        Blocked
      else
        NeedStage
    ),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action refuseSaveUnsaved = all {
    unsaved,
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = canonicalInventory(Blocked),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action saveCheckpoint = all {
    not(unsaved),
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = canonicalInventory(NeedStage),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action markSavedPath = all {
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = false,
    task' = task,
    report' = statusInventory(nextForCurrentTask),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  // Closing an untitled project or opening a new one drops any staged
  // canonical task. Studio tasks are in-memory and stay.
  action markUnsavedPath = all {
    not(unsaved),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = true,
    task' = if (task.present and task.store == CanonicalProject)
      emptyTask
    else
      task,
    report' = statusInventory(
      if (task.present and task.store == StudioSession)
        NeedStudioCommit
      else
        Blocked
    ),
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action stutter = all {
    hasCommitted,
    not(task.present),
    studioHead' = studioHead,
    canonicalHead' = canonicalHead,
    takeReadiness' = takeReadiness,
    hasTargetLink' = hasTargetLink,
    unsaved' = unsaved,
    task' = task,
    report' = report,
    hasCommitted' = hasCommitted,
    lastCommittedStore' = lastCommittedStore,
  }

  action step = any {
    observeStudio,
    observeCanonical,
    observeStatus,
    generateQueryReady,
    restoreReadyTake,
    stageStudio,
    refuseQueryReadyStage,
    refuseCanonicalWhenUnsaved,
    stageCanonical,
    commitStudio,
    commitCanonical,
    refuseReconcileWithoutLink,
    reconcile,
    refuseSaveUnsaved,
    saveCheckpoint,
    markSavedPath,
    markUnsavedPath,
    stutter,
  }

  val stagedImpliesCompleteReport =
    not(task.present)
      or and {
        report.visibleIdentity == task.identity,
        report.visibleDigest == task.digest,
        report.visibleIdentity != missing,
        report.visibleDigest != missing,
        report.namesStore,
        report.store == task.store,
        report.next == NeedStudioCommit or report.next == NeedCanonicalCommit,
      }

  val reportHeadsMatchState =
    not(report.namesStore)
      or and {
        report.reportedStudioHead == studioHead,
        report.reportedCanonicalHead == canonicalHead,
      }

  val queryReadyNeverStagesStudio =
    takeReadiness != QueryReady
      or not(task.present)
      or task.store != StudioSession

  val canonicalCommitDoesNotMoveStudio =
    not(hasCommitted)
      or lastCommittedStore != CanonicalProject
      or report.reportedStudioHead == studioHead

  val studioCommitDoesNotMoveCanonical =
    not(hasCommitted)
      or lastCommittedStore != StudioSession
      or report.reportedCanonicalHead == canonicalHead

  val statusKeepsBothHeads =
    not(report.isStatus)
      or and {
        report.reportedStudioHead == studioHead,
        report.reportedCanonicalHead == canonicalHead,
        report.namesStore,
      }

  val unsavedNeverHoldsCanonicalTask =
    not(unsaved)
      or not(task.present)
      or task.store != CanonicalProject

  val unsavedBlocksCanonicalNext =
    not(unsaved)
      or report.store != CanonicalProject
      or report.next == Blocked

  val safety =
    stagedImpliesCompleteReport
      and reportHeadsMatchState
      and queryReadyNeverStagesStudio
      and canonicalCommitDoesNotMoveStudio
      and studioCommitDoesNotMoveCanonical
      and statusKeepsBothHeads
      and unsavedNeverHoldsCanonicalTask
      and unsavedBlocksCanonicalNext
}
