use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{VoiceProvider, VoicevoxClient, VoicevoxError, Ymm4NativeVoiceArtifact};

const YMM4_PROVENANCE_SCHEMA: &str = "takegraph/ymm4-native-voice-provenance/v1";
const YMM4_PROVENANCE_PREFIX: &str = "ymm4-native-create-audio+normalized-host-bound-voice-state/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WavMetadata {
    pub duration_samples: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializedVoiceArtifact {
    pub style_id: u32,
    pub query: Value,
    pub query_hash: String,
    pub query_path: String,
    pub audio_hash: String,
    pub audio_path: String,
    pub wav: WavMetadata,
}

/// Service-owned, content-addressed copy of one YMM4 native voice export.
/// `query` is normalized host-bound provenance, not a portable synthesis query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedYmm4NativeVoiceArtifact {
    pub realization_id: Uuid,
    pub audio_sha256: String,
    pub audio_bytes: u64,
    pub audio_path: PathBuf,
    pub query_sha256: String,
    pub query_bytes: u64,
    pub query_path: PathBuf,
    pub provenance: String,
    pub query: Value,
    pub wav: WavMetadata,
}

impl ImportedYmm4NativeVoiceArtifact {
    /// Rehashes, reparses, and path-bounds both immutable CAS objects.
    ///
    /// # Errors
    ///
    /// Returns an error if either artifact escaped the CAS root or changed.
    pub fn verify(&self, artifact_root: &Path) -> Result<(), ArtifactError> {
        let root = fs::canonicalize(artifact_root)?;
        verify_imported_file(
            &root,
            &self.audio_path,
            &self.audio_sha256,
            self.audio_bytes,
        )?;
        verify_imported_file(
            &root,
            &self.query_path,
            &self.query_sha256,
            self.query_bytes,
        )?;
        let audio = fs::read(&self.audio_path)?;
        if parse_wav(&audio)? != self.wav {
            return Err(ArtifactError::ArtifactSemanticMismatch(
                "stored WAV metadata changed".into(),
            ));
        }
        let query: Value = serde_json::from_slice(&fs::read(&self.query_path)?)?;
        if query != self.query {
            return Err(ArtifactError::ArtifactSemanticMismatch(
                "stored provenance JSON changed".into(),
            ));
        }
        validate_ymm4_provenance(&query, self.realization_id, &self.provenance)
    }

    /// Re-verifies the immutable files and binds host provenance to the exact
    /// approved voice semantics instead of trusting editable JSON fields.
    ///
    /// # Errors
    ///
    /// Returns an error when the CAS evidence changed or its character/text
    /// fields do not match the approved native voice mutation.
    pub fn verify_for(
        &self,
        artifact_root: &Path,
        character_name: &str,
        display_text: &str,
        spoken_text: Option<&str>,
    ) -> Result<(), ArtifactError> {
        self.verify(artifact_root)?;
        validate_ymm4_provenance_binding(&self.query, character_name, display_text, spoken_text)
    }
}

/// Imports bridge-staged native voice artifacts into a service-owned CAS.
/// The bridge paths must canonicalize beneath `authorized_bridge_root`; claimed
/// hashes and lengths are checked before and after immutable publication.
///
/// # Errors
///
/// Returns an error for path escape, content mismatch, malformed WAV/JSON,
/// unsupported provenance, or an immutable CAS collision.
pub fn import_ymm4_native_voice_artifact(
    staged: &Ymm4NativeVoiceArtifact,
    authorized_bridge_root: &Path,
    artifact_root: &Path,
    character_name: &str,
    display_text: &str,
    spoken_text: Option<&str>,
) -> Result<ImportedYmm4NativeVoiceArtifact, ArtifactError> {
    let bridge_root = fs::canonicalize(authorized_bridge_root)?;
    let audio_source = canonical_staged_file(&bridge_root, Path::new(&staged.audio_path))?;
    let query_source = canonical_staged_file(&bridge_root, Path::new(&staged.query_path))?;
    let audio = fs::read(&audio_source)?;
    let query_bytes = fs::read(&query_source)?;
    verify_claimed_bytes(&audio, &staged.audio_sha256, staged.audio_bytes, "audio")?;
    verify_claimed_bytes(
        &query_bytes,
        &staged.query_sha256,
        staged.query_bytes,
        "provenance",
    )?;
    let wav = parse_wav(&audio)?;
    let query: Value = serde_json::from_slice(&query_bytes)?;
    validate_ymm4_provenance(&query, staged.realization_id, &staged.provenance)?;
    validate_ymm4_provenance_binding(&query, character_name, display_text, spoken_text)?;

    let audio_path = cas_path(artifact_root, "audio", &staged.audio_sha256, "wav")?;
    let query_path = cas_path(artifact_root, "provenance", &staged.query_sha256, "json")?;
    write_immutable(&audio_path, &audio, &staged.audio_sha256)?;
    write_immutable(&query_path, &query_bytes, &staged.query_sha256)?;

    let imported = ImportedYmm4NativeVoiceArtifact {
        realization_id: staged.realization_id,
        audio_sha256: staged.audio_sha256.clone(),
        audio_bytes: staged.audio_bytes,
        audio_path,
        query_sha256: staged.query_sha256.clone(),
        query_bytes: staged.query_bytes,
        query_path,
        provenance: staged.provenance.clone(),
        query,
        wav,
    };
    imported.verify_for(artifact_root, character_name, display_text, spoken_text)?;
    Ok(imported)
}

fn canonical_staged_file(root: &Path, path: &Path) -> Result<PathBuf, ArtifactError> {
    let path = fs::canonicalize(path)?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(ArtifactError::ArtifactPathEscape(path));
    }
    Ok(path)
}

fn cas_path(
    artifact_root: &Path,
    kind: &str,
    hash: &str,
    extension: &str,
) -> Result<PathBuf, ArtifactError> {
    validate_sha256(hash, kind)?;
    let directory = artifact_root
        .join("native-voice")
        .join(kind)
        .join(&hash[..2]);
    fs::create_dir_all(&directory)?;
    Ok(directory.join(format!("{hash}.{extension}")))
}

fn verify_claimed_bytes(
    bytes: &[u8],
    expected_hash: &str,
    expected_length: u64,
    kind: &'static str,
) -> Result<(), ArtifactError> {
    validate_sha256(expected_hash, kind)?;
    let actual_hash = sha256(bytes);
    if actual_hash != expected_hash {
        return Err(ArtifactError::ArtifactHashMismatch {
            kind,
            expected: expected_hash.into(),
            actual: actual_hash,
        });
    }
    let actual_length = u64::try_from(bytes.len())
        .map_err(|_| ArtifactError::ArtifactSemanticMismatch("artifact is too large".into()))?;
    if actual_length != expected_length {
        return Err(ArtifactError::ArtifactLengthMismatch {
            kind,
            expected: expected_length,
            actual: actual_length,
        });
    }
    Ok(())
}

fn verify_imported_file(
    root: &Path,
    path: &Path,
    expected_hash: &str,
    expected_length: u64,
) -> Result<(), ArtifactError> {
    let canonical = fs::canonicalize(path)?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        return Err(ArtifactError::ArtifactPathEscape(canonical));
    }
    verify_claimed_bytes(
        &fs::read(canonical)?,
        expected_hash,
        expected_length,
        "imported",
    )
}

fn validate_sha256(hash: &str, kind: &str) -> Result<(), ArtifactError> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase())
    {
        return Err(ArtifactError::InvalidArtifactHash(kind.into()));
    }
    Ok(())
}

fn validate_ymm4_provenance(
    query: &Value,
    realization_id: Uuid,
    provenance: &str,
) -> Result<(), ArtifactError> {
    if !provenance.starts_with(YMM4_PROVENANCE_PREFIX) {
        return Err(ArtifactError::UnsupportedProvenance(provenance.into()));
    }
    if query.get("schema").and_then(Value::as_str) != Some(YMM4_PROVENANCE_SCHEMA)
        || query.get("realizationId").and_then(Value::as_str)
            != Some(realization_id.hyphenated().to_string().as_str())
    {
        return Err(ArtifactError::ArtifactSemanticMismatch(
            "provenance schema or realization identity does not match".into(),
        ));
    }
    Ok(())
}

fn validate_ymm4_provenance_binding(
    query: &Value,
    character_name: &str,
    display_text: &str,
    spoken_text: Option<&str>,
) -> Result<(), ArtifactError> {
    let observed_spoken = query.get("spokenText").and_then(Value::as_str);
    let spoken_mismatch = match spoken_text {
        Some(approved) => observed_spoken != Some(approved),
        None => observed_spoken.is_none_or(str::is_empty),
    };
    if query.get("characterName").and_then(Value::as_str) != Some(character_name)
        || query.get("displayText").and_then(Value::as_str) != Some(display_text)
        || spoken_mismatch
    {
        return Err(ArtifactError::ArtifactSemanticMismatch(
            "provenance character/display/spoken text does not match the approved mutation".into(),
        ));
    }
    Ok(())
}

impl VoicevoxClient {
    /// Creates a VOICEVOX query, synthesizes its WAV, and stores both by hash.
    ///
    /// Existing matching files are reused; generated artifacts are never overwritten.
    ///
    /// # Errors
    ///
    /// Returns a provider, serialization, filesystem, collision, or malformed-WAV error.
    pub async fn materialize(
        &self,
        text: &str,
        style_id: u32,
        artifact_root: &Path,
    ) -> Result<MaterializedVoiceArtifact, ArtifactError> {
        let query = self.create_query(text, style_id).await?;
        let query_bytes = serde_json::to_vec_pretty(&query)?;
        let query_hash = sha256(&query_bytes);
        let audio = self.synthesize(&query, style_id).await?;
        let audio_hash = sha256(&audio);
        let wav = parse_wav(&audio)?;

        let query_directory = artifact_root.join("queries");
        let audio_directory = artifact_root.join("audio");
        fs::create_dir_all(&query_directory)?;
        fs::create_dir_all(&audio_directory)?;
        let query_path = query_directory.join(format!("{query_hash}.json"));
        let audio_path = audio_directory.join(format!("{audio_hash}.wav"));
        write_immutable(&query_path, &query_bytes, &query_hash)?;
        write_immutable(&audio_path, &audio, &audio_hash)?;

        Ok(MaterializedVoiceArtifact {
            style_id,
            query,
            query_hash,
            query_path: query_path.to_string_lossy().into_owned(),
            audio_hash,
            audio_path: audio_path.to_string_lossy().into_owned(),
            wav,
        })
    }
}

fn write_immutable(path: &Path, bytes: &[u8], expected_hash: &str) -> Result<(), ArtifactError> {
    if path.exists() {
        let actual_hash = sha256(&fs::read(path)?);
        if actual_hash == expected_hash {
            return Ok(());
        }
        return Err(ArtifactError::Collision {
            path: path.to_string_lossy().into_owned(),
        });
    }

    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() && sha256(&fs::read(path)?) == expected_hash => {
            let _ = fs::remove_file(&temporary);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error.into())
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Reads duration and PCM format metadata without decoding samples.
///
/// # Errors
///
/// Returns an error when the payload is not a supported, complete RIFF/WAVE file.
pub fn parse_wav(bytes: &[u8]) -> Result<WavMetadata, ArtifactError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(ArtifactError::InvalidWav("missing RIFF/WAVE header"));
    }

    let mut offset = 12usize;
    let mut format = None;
    let mut data_length = None;
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_length = read_u32(bytes, offset + 4)? as usize;
        let data_start = offset + 8;
        let data_end = data_start
            .checked_add(chunk_length)
            .ok_or(ArtifactError::InvalidWav("chunk length overflow"))?;
        if data_end > bytes.len() {
            return Err(ArtifactError::InvalidWav("truncated chunk"));
        }

        if chunk_id == b"fmt " {
            if chunk_length < 16 {
                return Err(ArtifactError::InvalidWav("short fmt chunk"));
            }
            let audio_format = read_u16(bytes, data_start)?;
            if audio_format != 1 && audio_format != 3 {
                return Err(ArtifactError::InvalidWav("unsupported WAV format"));
            }
            format = Some((
                read_u16(bytes, data_start + 2)?,
                read_u32(bytes, data_start + 4)?,
                read_u16(bytes, data_start + 12)?,
                read_u16(bytes, data_start + 14)?,
            ));
        } else if chunk_id == b"data" {
            data_length = Some(chunk_length as u64);
        }

        offset = data_end + (chunk_length & 1);
    }

    let (channels, sample_rate, block_align, bits_per_sample) =
        format.ok_or(ArtifactError::InvalidWav("missing fmt chunk"))?;
    let data_length = data_length.ok_or(ArtifactError::InvalidWav("missing data chunk"))?;
    if channels == 0 || sample_rate == 0 || block_align == 0 {
        return Err(ArtifactError::InvalidWav("invalid audio format values"));
    }
    Ok(WavMetadata {
        duration_samples: data_length / u64::from(block_align),
        sample_rate,
        channels,
        bits_per_sample,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ArtifactError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(ArtifactError::InvalidWav("truncated u16"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ArtifactError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(ArtifactError::InvalidWav("truncated u32"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error(transparent)]
    Voicevox(#[from] VoicevoxError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("artifact hash collision at {path}")]
    Collision { path: String },
    #[error("invalid WAV: {0}")]
    InvalidWav(&'static str),
    #[error("artifact escaped its authorized root: {0}")]
    ArtifactPathEscape(PathBuf),
    #[error("invalid SHA-256 claim for {0}")]
    InvalidArtifactHash(String),
    #[error("{kind} artifact hash mismatch: expected {expected}, got {actual}")]
    ArtifactHashMismatch {
        kind: &'static str,
        expected: String,
        actual: String,
    },
    #[error("{kind} artifact length mismatch: expected {expected}, got {actual}")]
    ArtifactLengthMismatch {
        kind: &'static str,
        expected: u64,
        actual: u64,
    },
    #[error("unsupported YMM4 provenance descriptor: {0}")]
    UnsupportedProvenance(String),
    #[error("native voice artifact semantic mismatch: {0}")]
    ArtifactSemanticMismatch(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm_wav() -> Vec<u8> {
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&24_000u32.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8u32.to_le_bytes());
        wav.extend_from_slice(&[0; 8]);
        wav
    }

    #[test]
    fn parses_pcm_wav_metadata() {
        let wav = pcm_wav();

        assert_eq!(
            parse_wav(&wav).unwrap(),
            WavMetadata {
                duration_samples: 4,
                sample_rate: 24_000,
                channels: 1,
                bits_per_sample: 16,
            }
        );
    }

    #[test]
    fn rejects_non_wav_payload() {
        assert!(matches!(
            parse_wav(b"not a wav"),
            Err(ArtifactError::InvalidWav(_))
        ));
    }

    #[test]
    fn imports_native_voice_artifact_with_path_hash_and_semantic_checks() {
        let root = std::env::temp_dir().join(format!("takegraph-artifact-{}", Uuid::new_v4()));
        let bridge = root.join("bridge");
        let cas = root.join("cas");
        fs::create_dir_all(&bridge).unwrap();
        let realization_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let audio = pcm_wav();
        let query = serde_json::to_vec(&serde_json::json!({
            "schema": YMM4_PROVENANCE_SCHEMA,
            "realizationId": realization_id,
            "characterName": "春日部つむぎ",
            "displayText": "ここから第二形態です",
            "spokenText": "ここから第二形態です"
        }))
        .unwrap();
        let audio_path = bridge.join("voice.wav");
        let query_path = bridge.join("voice.json");
        fs::write(&audio_path, &audio).unwrap();
        fs::write(&query_path, &query).unwrap();
        let staged = Ymm4NativeVoiceArtifact {
            realization_id,
            audio_path: audio_path.to_string_lossy().into_owned(),
            audio_sha256: sha256(&audio),
            audio_bytes: audio.len() as u64,
            query_path: query_path.to_string_lossy().into_owned(),
            query_sha256: sha256(&query),
            query_bytes: query.len() as u64,
            provenance: format!("{YMM4_PROVENANCE_PREFIX}4.55.1.1"),
        };
        let imported = import_ymm4_native_voice_artifact(
            &staged,
            &bridge,
            &cas,
            "春日部つむぎ",
            "ここから第二形態です",
            Some("ここから第二形態です"),
        )
        .unwrap();
        imported
            .verify_for(
                &cas,
                "春日部つむぎ",
                "ここから第二形態です",
                Some("ここから第二形態です"),
            )
            .unwrap();
        assert_ne!(imported.audio_path, audio_path);

        assert!(matches!(
            imported.verify_for(
                &cas,
                "別のキャラクター",
                "ここから第二形態です",
                Some("ここから第二形態です"),
            ),
            Err(ArtifactError::ArtifactSemanticMismatch(_))
        ));

        fs::write(&imported.audio_path, b"tampered").unwrap();
        assert!(matches!(
            imported.verify(&cas),
            Err(ArtifactError::ArtifactHashMismatch { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }
}
