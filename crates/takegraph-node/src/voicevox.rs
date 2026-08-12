use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use url::{Host, Url};

/// `VOICEVOX` talk style. The API calls this integer `speaker`, while `TakeGraph`
/// consistently names it `style_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerStyle {
    pub name: String,
    pub id: u32,
    #[serde(rename = "type", default)]
    pub style_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Speaker {
    pub name: String,
    pub speaker_uuid: String,
    pub styles: Vec<SpeakerStyle>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Capability document captured when `TakeGraph` probes an existing engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoicevoxCapabilities {
    pub endpoint: Url,
    pub engine_version: String,
    pub engine_manifest: Value,
    pub core_versions: Vec<String>,
    pub supported_devices: Value,
    pub speakers: Vec<Speaker>,
}

#[async_trait]
pub trait VoiceProvider {
    type Error;

    /// Discovers provider versions, devices, voices, and manifest data.
    ///
    /// # Errors
    ///
    /// Returns a provider-specific connection or response error.
    async fn probe(&self) -> Result<VoicevoxCapabilities, Self::Error>;

    /// Creates an editable provider query without synthesizing audio.
    ///
    /// # Errors
    ///
    /// Returns a provider-specific connection, validation, or response error.
    async fn create_query(&self, text: &str, style_id: u32) -> Result<Value, Self::Error>;

    /// Materializes a query as a WAV artifact payload.
    ///
    /// # Errors
    ///
    /// Returns a provider-specific connection, validation, or response error.
    async fn synthesize(&self, query: &Value, style_id: u32) -> Result<Vec<u8>, Self::Error>;
}

/// Client for an already-running, loopback-only `VOICEVOX ENGINE`.
#[derive(Debug, Clone)]
pub struct VoicevoxClient {
    endpoint: Url,
    http: Client,
}

impl VoicevoxClient {
    /// Creates a client and rejects non-loopback endpoints for the MVP.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint is invalid or is not loopback-only.
    pub fn new(endpoint: &str) -> Result<Self, VoicevoxError> {
        let mut endpoint = Url::parse(endpoint)?;
        if !is_loopback(&endpoint) {
            return Err(VoicevoxError::NonLoopbackEndpoint(endpoint));
        }

        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }

        Ok(Self {
            endpoint,
            http: Client::new(),
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, VoicevoxError> {
        let url = self.endpoint.join(path)?;
        let value = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(value)
    }
}

#[async_trait]
impl VoiceProvider for VoicevoxClient {
    type Error = VoicevoxError;

    async fn probe(&self) -> Result<VoicevoxCapabilities, Self::Error> {
        let (engine_version, engine_manifest, core_versions, supported_devices, speakers) = tokio::try_join!(
            self.get_json("version"),
            self.get_json("engine_manifest"),
            self.get_json("core_versions"),
            self.get_json("supported_devices"),
            self.get_json("speakers"),
        )?;

        Ok(VoicevoxCapabilities {
            endpoint: self.endpoint.clone(),
            engine_version,
            engine_manifest,
            core_versions,
            supported_devices,
            speakers,
        })
    }

    async fn create_query(&self, text: &str, style_id: u32) -> Result<Value, Self::Error> {
        let url = self.endpoint.join("audio_query")?;
        let value = self
            .http
            .post(url)
            .query(&[("text", text), ("speaker", &style_id.to_string())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(value)
    }

    async fn synthesize(&self, query: &Value, style_id: u32) -> Result<Vec<u8>, Self::Error> {
        let url = self.endpoint.join("synthesis")?;
        let audio = self
            .http
            .post(url)
            .query(&[("speaker", style_id)])
            .json(query)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(audio.to_vec())
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[derive(Debug, Error)]
pub enum VoicevoxError {
    #[error("invalid VOICEVOX endpoint: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("VOICEVOX endpoint must be loopback-only in the MVP: {0}")]
    NonLoopbackEndpoint(Url),
    #[error("VOICEVOX request failed: {0}")]
    Http(#[from] reqwest::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_endpoints() {
        assert!(VoicevoxClient::new("http://127.0.0.1:50021").is_ok());
        assert!(VoicevoxClient::new("http://localhost:50021").is_ok());
        assert!(VoicevoxClient::new("http://[::1]:50021").is_ok());
    }

    #[test]
    fn rejects_remote_endpoints() {
        assert!(matches!(
            VoicevoxClient::new("https://example.com"),
            Err(VoicevoxError::NonLoopbackEndpoint(_))
        ));
    }
}
