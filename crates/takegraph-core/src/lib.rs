//! Portable `TakeGraph` domain model.
//!
//! This crate contains deterministic state transitions only. It intentionally
//! has no filesystem, network, database, or operating-system dependencies.

pub mod revision;
pub mod voice;

pub use revision::{Patch, PatchError, PatchId, PatchStatus, RevisionId};
pub use voice::{AudioArtifact, VoiceTake, VoiceTakeError, VoiceTakeStatus, VoiceTaskIdentity};
