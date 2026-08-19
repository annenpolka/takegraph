//! Host-local whisper.cpp binding. Paths never enter a model-facing payload.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// User-managed ASR files resolved on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionHostConfig {
    pub executable: PathBuf,
    pub model: PathBuf,
    pub language: String,
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TranscriptionHostFile {
    executable: PathBuf,
    model: PathBuf,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    extra_args: Vec<String>,
}

/// Fail-closed host configuration errors. Messages must not invent paths.
#[derive(Debug, Error)]
pub enum TranscriptionConfigError {
    #[error(
        "user-managed whisper executable and model are not configured; set TAKEGRAPH_WHISPER_EXECUTABLE and TAKEGRAPH_WHISPER_MODEL"
    )]
    Missing,
    #[error("transcription host config could not be read")]
    Unreadable,
    #[error("whisper executable is not a file")]
    ExecutableMissing,
    #[error("whisper model is not a file")]
    ModelMissing,
}

/// Resolves the operator-managed whisper binding.
///
/// Environment variables win, then `TAKEGRAPH_WHISPER_CONFIG`, then
/// `%LOCALAPPDATA%\\TakeGraph\\transcription.json` / `.takegraph/transcription.json`.
///
/// # Errors
///
/// Returns [`TranscriptionConfigError::Missing`] when no complete binding is
/// configured.
pub fn resolve_transcription_host(
    language_override: Option<&str>,
) -> Result<TranscriptionHostConfig, TranscriptionConfigError> {
    resolve_transcription_host_from(
        |name| std::env::var(name),
        language_override,
        default_transcription_config_path,
    )
}

fn resolve_transcription_host_from(
    env: impl Fn(&str) -> Result<String, std::env::VarError>,
    language_override: Option<&str>,
    default_config: impl Fn() -> Option<PathBuf>,
) -> Result<TranscriptionHostConfig, TranscriptionConfigError> {
    let env_nonempty = |name: &str| {
        env(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let env_path = |name: &str| env_nonempty(name).map(PathBuf::from);
    let from_env = match (
        env_path("TAKEGRAPH_WHISPER_EXECUTABLE"),
        env_path("TAKEGRAPH_WHISPER_MODEL"),
    ) {
        (Some(executable), Some(model)) => Some(TranscriptionHostConfig {
            executable,
            model,
            language: language_override
                .map(str::to_owned)
                .or_else(|| env_nonempty("TAKEGRAPH_WHISPER_LANGUAGE"))
                .unwrap_or_else(|| "ja".into()),
            extra_args: Vec::new(),
        }),
        (None, None) => None,
        _ => return Err(TranscriptionConfigError::Missing),
    };
    let resolved = if let Some(config) = from_env {
        config
    } else {
        let path = env_path("TAKEGRAPH_WHISPER_CONFIG")
            .or_else(default_config)
            .ok_or(TranscriptionConfigError::Missing)?;
        let parsed = read_file(&path)?;
        TranscriptionHostConfig {
            executable: parsed.executable,
            model: parsed.model,
            language: language_override
                .map(str::to_owned)
                .or(parsed.language)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "ja".into()),
            extra_args: parsed.extra_args,
        }
    };
    if !resolved.executable.is_file() {
        return Err(TranscriptionConfigError::ExecutableMissing);
    }
    if !resolved.model.is_file() {
        return Err(TranscriptionConfigError::ModelMissing);
    }
    Ok(resolved)
}

/// Returns whether a complete host binding exists without leaking paths.
#[must_use]
pub fn whisper_host_configured() -> bool {
    resolve_transcription_host(None).is_ok()
}

fn read_file(path: &Path) -> Result<TranscriptionHostFile, TranscriptionConfigError> {
    let bytes = fs::read(path).map_err(|_| TranscriptionConfigError::Unreadable)?;
    serde_json::from_slice(&bytes).map_err(|_| TranscriptionConfigError::Unreadable)
}

fn default_transcription_config_path() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        let path = PathBuf::from(root)
            .join("TakeGraph")
            .join("transcription.json");
        if path.is_file() {
            return Some(path);
        }
    }
    let local = PathBuf::from(".takegraph").join("transcription.json");
    local.is_file().then_some(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binding_is_closed() {
        let error =
            resolve_transcription_host_from(|_| Err(std::env::VarError::NotPresent), None, || None)
                .unwrap_err();
        assert!(matches!(error, TranscriptionConfigError::Missing));
        assert!(!error.to_string().contains('\\'));
    }

    #[test]
    fn file_extra_args_are_path_free() {
        let root =
            std::env::temp_dir().join(format!("takegraph-whisper-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let executable = root.join("whisper-cli.exe");
        let model = root.join("model.bin");
        std::fs::write(&executable, b"exe").unwrap();
        std::fs::write(&model, b"model").unwrap();
        let config = root.join("transcription.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "executable": executable,
                "model": model,
                "extraArgs": ["--suppress-nst"],
            }))
            .unwrap(),
        )
        .unwrap();
        let resolved = resolve_transcription_host_from(
            |_| Err(std::env::VarError::NotPresent),
            None,
            || Some(config.clone()),
        )
        .unwrap();
        assert_eq!(resolved.extra_args, vec!["--suppress-nst"]);
        let _ = std::fs::remove_dir_all(root);
    }
}
