//! 16 kHz mono 16-bit PCM WAV encode, CAS publish, and evidence mapping.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use takegraph_core::CapturedAudioEvidence;
use takegraph_node::{WavMetadata, parse_wav};
use uuid::Uuid;

use crate::error::CaptureError;

/// Capture-host PCM sample rate.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Capture-host channel count.
pub const TARGET_CHANNELS: u16 = 1;
/// Capture-host sample width.
pub const TARGET_BITS_PER_SAMPLE: u16 = 16;

/// Encodes 16 kHz mono 16-bit PCM as a complete RIFF/WAVE file.
#[must_use]
pub fn encode_pcm_wav(samples: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44usize.saturating_add(data_len as usize));
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&TARGET_CHANNELS.to_le_bytes());
    out.extend_from_slice(&TARGET_SAMPLE_RATE.to_le_bytes());
    let byte_rate =
        TARGET_SAMPLE_RATE * u32::from(TARGET_CHANNELS) * u32::from(TARGET_BITS_PER_SAMPLE / 8);
    out.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = TARGET_CHANNELS * (TARGET_BITS_PER_SAMPLE / 8);
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&TARGET_BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// True when the stream is digital zero. Whisper hallucinates on that input.
#[must_use]
pub fn is_silent_pcm(samples: &[i16]) -> bool {
    samples.iter().all(|sample| *sample == 0)
}

/// SHA-256 digest in the `sha256:<hex>` form required by capture evidence.
#[must_use]
pub fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Hex portion of a `sha256:<hex>` digest.
#[must_use]
pub fn digest_hex(digest: &str) -> Option<&str> {
    digest.strip_prefix("sha256:").filter(|hex| hex.len() == 64)
}

/// Content-addressed path `{store}/audio/{hh}/{hex}.wav`.
#[must_use]
pub fn audio_cas_path(store_directory: &Path, audio_sha256: &str) -> Option<PathBuf> {
    let hex = digest_hex(audio_sha256)?;
    Some(
        store_directory
            .join("audio")
            .join(&hex[..2])
            .join(format!("{hex}.wav")),
    )
}

/// Writes `bytes` to `path` if the path is absent or already holds the same
/// digest. A different occupant is a collision.
///
/// # Errors
///
/// Returns [`CaptureError::Io`] or a collision encoded as [`CaptureError::InvalidWav`].
pub fn write_immutable(
    path: &Path,
    bytes: &[u8],
    expected_sha256: &str,
) -> Result<(), CaptureError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let existing = sha256_prefixed(&fs::read(path)?);
        if existing == expected_sha256 {
            return Ok(());
        }
        return Err(CaptureError::InvalidWav(format!(
            "CAS collision at {}",
            path.display()
        )));
    }
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    let write_result = (|| -> Result<(), CaptureError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

/// Parses WAV bytes into capture evidence. Rejects empty or non-target PCM.
///
/// # Errors
///
/// Returns [`CaptureError::InvalidWav`] or [`CaptureError::EmptyRecording`].
pub fn evidence_from_wav(bytes: &[u8]) -> Result<CapturedAudioEvidence, CaptureError> {
    let metadata = parse_wav(bytes).map_err(|error| CaptureError::InvalidWav(error.to_string()))?;
    require_target_pcm(&metadata)?;
    if metadata.duration_samples == 0 {
        return Err(CaptureError::EmptyRecording);
    }
    let evidence = CapturedAudioEvidence {
        audio_sha256: sha256_prefixed(bytes),
        byte_length: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        duration_samples: metadata.duration_samples,
        sample_rate: metadata.sample_rate,
        channels: metadata.channels,
        bits_per_sample: metadata.bits_per_sample,
    };
    evidence
        .validate()
        .map_err(|error| CaptureError::InvalidWav(error.to_string()))?;
    Ok(evidence)
}

fn require_target_pcm(metadata: &WavMetadata) -> Result<(), CaptureError> {
    if metadata.sample_rate != TARGET_SAMPLE_RATE
        || metadata.channels != TARGET_CHANNELS
        || metadata.bits_per_sample != TARGET_BITS_PER_SAMPLE
    {
        return Err(CaptureError::InvalidWav(format!(
            "expected {TARGET_SAMPLE_RATE} Hz {TARGET_CHANNELS}-ch {TARGET_BITS_PER_SAMPLE}-bit PCM"
        )));
    }
    Ok(())
}

/// Publishes WAV bytes into the project-scoped audio CAS.
///
/// # Errors
///
/// Returns an error for invalid WAV, empty audio, I/O failure, or collision.
pub fn publish_wav(
    store_directory: &Path,
    bytes: &[u8],
) -> Result<(CapturedAudioEvidence, PathBuf), CaptureError> {
    let evidence = evidence_from_wav(bytes)?;
    let path = audio_cas_path(store_directory, &evidence.audio_sha256).ok_or_else(|| {
        CaptureError::InvalidWav("audio digest is not a sha256:<hex> value".into())
    })?;
    write_immutable(&path, bytes, &evidence.audio_sha256)?;
    Ok((evidence, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_parseable_target_pcm() {
        let samples = vec![0i16; 16_000];
        let bytes = encode_pcm_wav(&samples);
        let evidence = evidence_from_wav(&bytes).unwrap();
        assert_eq!(evidence.sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(evidence.channels, TARGET_CHANNELS);
        assert_eq!(evidence.bits_per_sample, TARGET_BITS_PER_SAMPLE);
        assert_eq!(evidence.duration_samples, 16_000);
        assert!(evidence.audio_sha256.starts_with("sha256:"));
    }

    #[test]
    fn silence_is_detected() {
        assert!(is_silent_pcm(&[0; 1600]));
        assert!(!is_silent_pcm(&[0, 0, 1, 0]));
    }

    #[test]
    fn empty_pcm_is_rejected() {
        let bytes = encode_pcm_wav(&[]);
        assert!(matches!(
            evidence_from_wav(&bytes),
            Err(CaptureError::EmptyRecording)
        ));
    }

    #[test]
    fn publish_is_idempotent_for_the_same_bytes() {
        let root = std::env::temp_dir().join(format!("takegraph-wav-{}", Uuid::new_v4()));
        let bytes = encode_pcm_wav(&[1, 2, 3, 4]);
        let (first, path) = publish_wav(&root, &bytes).unwrap();
        let (second, same) = publish_wav(&root, &bytes).unwrap();
        assert_eq!(first.audio_sha256, second.audio_sha256);
        assert_eq!(path, same);
        assert!(path.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
