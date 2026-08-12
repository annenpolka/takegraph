//! Local media-node adapters for `TakeGraph`.

pub mod voicevox;

pub use voicevox::{
    Speaker, SpeakerStyle, VoiceProvider, VoicevoxCapabilities, VoicevoxClient, VoicevoxError,
};
