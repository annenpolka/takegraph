module patchProtocol {
  type Status =
    | Draft
    | Validated
    | Previewable
    | Approved
    | Committed
    | Conflicted

  type Patch = {
    status: Status,
    base: int,
    digest: int,
    approvedDigest: int,
    touchesHardLock: bool,
  }

  var headRevision: int
  var patch: Patch

  action init = all {
    headRevision' = 0,
    patch' = {
      status: Draft,
      base: 0,
      digest: 0,
      approvedDigest: -1,
      touchesHardLock: false,
    },
  }

  action validate = all {
    patch.status == Draft,
    headRevision' = headRevision,
    patch' = patch.with("status", Validated),
  }

  action materialize = all {
    patch.status == Validated,
    headRevision' = headRevision,
    patch' = patch.with("status", Previewable),
  }

  action approve = all {
    patch.status == Previewable,
    headRevision' = headRevision,
    patch' = patch
      .with("status", Approved)
      .with("approvedDigest", patch.digest),
  }

  action mutateAfterApproval = all {
    patch.status == Approved,
    headRevision' = headRevision,
    patch' = patch
      .with("status", Previewable)
      .with("digest", patch.digest + 1)
      .with("approvedDigest", -1),
  }

  action concurrentCommit = all {
    patch.status != Committed,
    headRevision' = headRevision + 1,
    patch' = patch,
  }

  action commit = all {
    patch.status == Approved,
    patch.base == headRevision,
    patch.approvedDigest == patch.digest,
    not(patch.touchesHardLock),
    headRevision' = headRevision + 1,
    patch' = patch.with("status", Committed),
  }

  action markConflict = all {
    patch.status == Approved,
    patch.base != headRevision,
    headRevision' = headRevision,
    patch' = patch.with("status", Conflicted),
  }

  action stutter = all {
    patch.status == Committed or patch.status == Conflicted,
    headRevision' = headRevision,
    patch' = patch,
  }

  action step = any {
    validate,
    materialize,
    approve,
    mutateAfterApproval,
    concurrentCommit,
    commit,
    markConflict,
    stutter,
  }

  val approvalIsFresh =
    patch.status != Approved
      or patch.approvedDigest == patch.digest

  val committedSafely =
    patch.status != Committed
      or and {
        patch.base + 1 == headRevision,
        patch.approvedDigest == patch.digest,
        not(patch.touchesHardLock),
      }

  val safety = approvalIsFresh and committedSafely
}
