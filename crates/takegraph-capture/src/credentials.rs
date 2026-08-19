//! Loopback credential file for the capture host.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CaptureError;

/// Default loopback listen URL.
pub const DEFAULT_CAPTURE_ENDPOINT: &str = "http://127.0.0.1:8767";
/// HTTP header copied from the YMM4 bridge credential model.
pub const CAPTURE_TOKEN_HEADER: &str = "x-takegraph-token";

/// Persisted loopback endpoint, token, and the last operator-selected hotkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureHostCredentials {
    pub endpoint: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
}

/// Default `%LOCALAPPDATA%\TakeGraph\capture-host.json`.
#[must_use]
pub fn default_credentials_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map_or_else(
        || PathBuf::from("capture-host.json"),
        |root| {
            PathBuf::from(root)
                .join("TakeGraph")
                .join("capture-host.json")
        },
    )
}

impl CaptureHostCredentials {
    /// Loads an existing file or creates a random token and writes it.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or written, or when a
    /// configured token is empty.
    pub fn load_or_create(
        path: &Path,
        endpoint: impl Into<String>,
        configured_token: Option<String>,
    ) -> Result<Self, CaptureError> {
        let endpoint = endpoint.into();
        if let Some(token) = configured_token.filter(|value| !value.trim().is_empty()) {
            let hotkey = read_existing(path).and_then(|existing| existing.hotkey);
            let credentials = Self {
                endpoint,
                token,
                hotkey,
            };
            credentials.save(path)?;
            return Ok(credentials);
        }
        if let Some(existing) = read_existing(path).filter(|value| !value.token.trim().is_empty()) {
            let credentials = Self {
                endpoint,
                token: existing.token,
                hotkey: existing.hotkey,
            };
            credentials.save(path)?;
            return Ok(credentials);
        }
        let created = Self {
            endpoint,
            token: random_token(),
            hotkey: None,
        };
        created.save(path)?;
        Ok(created)
    }

    /// Stores the operator-selected toggle key without rotating the token.
    ///
    /// # Errors
    ///
    /// Returns an I/O or decode error when the credential file cannot be
    /// rewritten.
    pub fn persist_hotkey(path: &Path, hotkey: &str) -> Result<(), CaptureError> {
        let mut credentials = read_existing(path).ok_or_else(|| {
            CaptureError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "capture-host credentials are missing",
            ))
        })?;
        credentials.hotkey = Some(hotkey.to_owned());
        credentials.save(path)
    }

    /// Atomically publishes the credential file.
    ///
    /// # Errors
    ///
    /// Returns an I/O or serialization error.
    pub fn save(&self, path: &Path) -> Result<(), CaptureError> {
        if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
        let write_result = (|| -> Result<(), CaptureError> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result
    }
}

/// Constant-time comparison of equally sized tokens. Length mismatch is
/// treated as inequality.
#[must_use]
pub fn tokens_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn read_existing(path: &Path) -> Option<CaptureHostCredentials> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_reloads_the_same_token() {
        let path = std::env::temp_dir().join(format!("capture-host-{}.json", Uuid::new_v4()));
        let first =
            CaptureHostCredentials::load_or_create(&path, DEFAULT_CAPTURE_ENDPOINT, None).unwrap();
        let second =
            CaptureHostCredentials::load_or_create(&path, DEFAULT_CAPTURE_ENDPOINT, None).unwrap();
        assert_eq!(first.token, second.token);
        assert_eq!(first.token.len(), 64);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn configured_token_overwrites_the_file() {
        let path = std::env::temp_dir().join(format!("capture-host-{}.json", Uuid::new_v4()));
        CaptureHostCredentials::load_or_create(&path, DEFAULT_CAPTURE_ENDPOINT, None).unwrap();
        let overwritten = CaptureHostCredentials::load_or_create(
            &path,
            DEFAULT_CAPTURE_ENDPOINT,
            Some("configured-token-value".into()),
        )
        .unwrap();
        assert_eq!(overwritten.token, "configured-token-value");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn tokens_equal_is_length_sensitive() {
        assert!(tokens_equal("abcd", "abcd"));
        assert!(!tokens_equal("abcd", "abce"));
        assert!(!tokens_equal("abcd", "abc"));
    }

    #[test]
    fn persist_hotkey_keeps_the_token() {
        let path = std::env::temp_dir().join(format!("capture-host-{}.json", Uuid::new_v4()));
        let created =
            CaptureHostCredentials::load_or_create(&path, DEFAULT_CAPTURE_ENDPOINT, None).unwrap();
        CaptureHostCredentials::persist_hotkey(&path, "F9").unwrap();
        let reloaded =
            CaptureHostCredentials::load_or_create(&path, DEFAULT_CAPTURE_ENDPOINT, None).unwrap();
        assert_eq!(reloaded.token, created.token);
        assert_eq!(reloaded.hotkey.as_deref(), Some("F9"));
        fs::remove_file(path).unwrap();
    }
}
