//! User-managed whisper.cpp-class executable. TakeGraph never downloads or
//! bundles the binary or model.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use takegraph_core::{CapturedAudioEvidence, canonical_sha256};
use uuid::Uuid;

use crate::transcription::{TranscriptResult, TranscriptionError, TranscriptionProvider};

/// Decoder knobs that skip silent tails Japanese models complete as outros.
///
/// `no_speech_prob > no_speech_thold && avg_logprob < logprob_thold` marks a
/// segment as silence. Lower no-speech and a slightly tighter logprob/entropy
/// pair drop more of those tails. Operator extra args override the same flags.
pub const DEFAULT_WHISPER_DECODER_ARGS: &[&str] = &[
    "--no-speech-thold",
    "0.4",
    "--entropy-thold",
    "2.2",
    "--logprob-thold",
    "-0.8",
];

const DECODER_FLAG_ALIASES: &[(&str, &str)] = &[
    ("--no-speech-thold", "-nth"),
    ("--entropy-thold", "-et"),
    ("--logprob-thold", "-lpt"),
];

/// 20 ms analysis window. Scaled by the WAV sample rate.
const ASR_WINDOW_MS: u32 = 20;
/// RMS below this is treated as silence for ASR-only trim.
const ASR_RMS_THRESHOLD: i64 = 250;
/// Keep this much audio around the voiced span.
const ASR_PAD_MS: u32 = 300;

/// External whisper.cpp CLI invoked with `-m`, `-f`, `-l`, `-otxt`, and `-of`.
#[derive(Debug, Clone)]
pub struct WhisperCppProvider {
    executable: PathBuf,
    model: PathBuf,
    language: String,
    extra_args: Vec<String>,
}

impl WhisperCppProvider {
    /// Builds a provider for an already-installed executable and model.
    ///
    /// # Errors
    ///
    /// Returns [`TranscriptionError::Unavailable`] when either path is missing.
    pub fn new(
        executable: impl Into<PathBuf>,
        model: impl Into<PathBuf>,
        language: impl Into<String>,
        extra_args: Vec<String>,
    ) -> Result<Self, TranscriptionError> {
        let provider = Self {
            executable: executable.into(),
            model: model.into(),
            language: language.into(),
            extra_args: merge_whisper_decoder_args(extra_args),
        };
        if !provider.executable.is_file() {
            return Err(TranscriptionError::Unavailable(format!(
                "whisper executable is not a file: {}",
                provider.executable.display()
            )));
        }
        if !provider.model.is_file() {
            return Err(TranscriptionError::Unavailable(format!(
                "whisper model is not a file: {}",
                provider.model.display()
            )));
        }
        Ok(provider)
    }

    /// Merged decoder args actually passed to the executable.
    #[must_use]
    pub fn extra_args(&self) -> &[String] {
        &self.extra_args
    }
}

#[async_trait]
impl TranscriptionProvider for WhisperCppProvider {
    fn provider_id(&self) -> &str {
        "whisper-cpp"
    }

    fn provider_digest(&self) -> Result<String, TranscriptionError> {
        let executable = sha256_file(&self.executable)?;
        let model = sha256_file(&self.model)?;
        Ok(canonical_sha256(
            "takegraph-transcription-provider-v1",
            &(
                "whisper-cpp",
                executable,
                model,
                self.language.as_str(),
                &self.extra_args,
            ),
        )?)
    }

    async fn transcribe(
        &self,
        _audio: &CapturedAudioEvidence,
        audio_path: &Path,
    ) -> Result<TranscriptResult, TranscriptionError> {
        if !audio_path.is_file() {
            return Err(TranscriptionError::Unavailable(format!(
                "captured audio is missing: {}",
                audio_path.display()
            )));
        }
        let executable = self.executable.clone();
        let model = self.model.clone();
        let language = self.language.clone();
        let extra_args = self.extra_args.clone();
        let audio_path = audio_path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            run_whisper(&executable, &model, &language, &extra_args, &audio_path)
        })
        .await
        .map_err(|error| TranscriptionError::Failed(error.to_string()))?
    }
}

/// Fills in [`DEFAULT_WHISPER_DECODER_ARGS`] unless the operator already set
/// the same long or short flag.
#[must_use]
pub fn merge_whisper_decoder_args(operator: Vec<String>) -> Vec<String> {
    let overridden = operator_decoder_flags(&operator);
    let mut merged = Vec::with_capacity(DEFAULT_WHISPER_DECODER_ARGS.len() + operator.len());
    let mut defaults = DEFAULT_WHISPER_DECODER_ARGS.iter();
    while let Some(flag) = defaults.next() {
        let value = defaults.next().copied().unwrap_or("");
        if overridden.contains(*flag) {
            continue;
        }
        merged.push((*flag).to_owned());
        merged.push(value.to_owned());
    }
    merged.extend(operator);
    merged
}

/// Keeps voiced audio plus a short pad. All-quiet input is left unchanged so
/// quiet speech is not dropped. The stored capture WAV is never rewritten.
#[must_use]
pub fn trim_pcm_for_asr(samples: &[i16], sample_rate: u32) -> &[i16] {
    if samples.is_empty() || sample_rate == 0 {
        return samples;
    }
    let window = ((sample_rate as usize) * ASR_WINDOW_MS as usize / 1000).max(1);
    let pad = (sample_rate as usize) * ASR_PAD_MS as usize / 1000;
    let mut first = None;
    let mut last = None;
    let mut index = 0;
    while index < samples.len() {
        let end = (index + window).min(samples.len());
        if !window_is_silent(&samples[index..end]) {
            if first.is_none() {
                first = Some(index);
            }
            last = Some(end);
        }
        index = end;
    }
    let (Some(start), Some(stop)) = (first, last) else {
        return samples;
    };
    let start = start.saturating_sub(pad);
    let stop = stop.saturating_add(pad).min(samples.len());
    &samples[start..stop]
}

fn operator_decoder_flags(args: &[String]) -> HashSet<&'static str> {
    args.iter()
        .filter_map(|argument| {
            DECODER_FLAG_ALIASES
                .iter()
                .find(|(long, short)| argument == *long || argument == *short)
                .map(|(long, _)| *long)
        })
        .collect()
}

fn window_is_silent(samples: &[i16]) -> bool {
    if samples.is_empty() {
        return true;
    }
    let energy: i64 = samples
        .iter()
        .map(|sample| {
            let value = i64::from(*sample);
            value.saturating_mul(value)
        })
        .sum();
    let window_len = i64::try_from(samples.len()).unwrap_or(i64::MAX);
    let limit = ASR_RMS_THRESHOLD
        .saturating_mul(ASR_RMS_THRESHOLD)
        .saturating_mul(window_len);
    energy <= limit
}

fn run_whisper(
    executable: &Path,
    model: &Path,
    language: &str,
    extra_args: &[String],
    audio_path: &Path,
) -> Result<TranscriptResult, TranscriptionError> {
    let prepared = prepare_whisper_input(audio_path)?;
    let prefix = std::env::temp_dir().join(format!("takegraph-whisper-{}", Uuid::new_v4()));
    let mut command = Command::new(executable);
    command
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&prepared.path)
        .arg("-l")
        .arg(language)
        .arg("-otxt")
        .arg("-of")
        .arg(&prefix);
    for argument in extra_args {
        command.arg(argument);
    }
    let output = command
        .output()
        .map_err(|error| TranscriptionError::Failed(error.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let transcript_path = prefix.with_extension("txt");
    let file_text = fs::read_to_string(&transcript_path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    let _ = fs::remove_file(&transcript_path);
    let text = file_text
        .filter(|value| !value.is_empty())
        .unwrap_or(stdout);
    if text.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(if output.status.success() {
            TranscriptionError::Empty("provider wrote no transcript text".into())
        } else {
            TranscriptionError::Failed(format!("whisper exited {}: {stderr}", output.status))
        });
    }
    Ok(TranscriptResult { text })
}

struct PreparedInput {
    path: PathBuf,
    temporary: Option<PathBuf>,
}

impl Drop for PreparedInput {
    fn drop(&mut self) {
        if let Some(path) = self.temporary.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn prepare_whisper_input(audio_path: &Path) -> Result<PreparedInput, TranscriptionError> {
    let bytes = fs::read(audio_path).map_err(|error| {
        TranscriptionError::Unavailable(format!("captured audio is unreadable: {error}"))
    })?;
    let Some((sample_rate, samples)) = decode_pcm16_mono(&bytes) else {
        return Ok(PreparedInput {
            path: audio_path.to_path_buf(),
            temporary: None,
        });
    };
    let trimmed = trim_pcm_for_asr(&samples, sample_rate);
    if trimmed.len() == samples.len() {
        return Ok(PreparedInput {
            path: audio_path.to_path_buf(),
            temporary: None,
        });
    }
    let encoded = encode_pcm16_mono_wav(trimmed, sample_rate);
    let temporary = std::env::temp_dir().join(format!("takegraph-asr-{}.wav", Uuid::new_v4()));
    fs::write(&temporary, encoded).map_err(|error| {
        TranscriptionError::Failed(format!("could not write trimmed ASR input: {error}"))
    })?;
    Ok(PreparedInput {
        path: temporary.clone(),
        temporary: Some(temporary),
    })
}

fn decode_pcm16_mono(bytes: &[u8]) -> Option<(u32, Vec<i16>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut format = None;
    let mut pcm = None;
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_len = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let data_start = offset + 8;
        let data_end = data_start.checked_add(chunk_len)?;
        if data_end > bytes.len() {
            return None;
        }
        if chunk_id == b"fmt " && chunk_len >= 16 {
            format = Some((
                u16::from_le_bytes(bytes[data_start..data_start + 2].try_into().ok()?),
                u16::from_le_bytes(bytes[data_start + 2..data_start + 4].try_into().ok()?),
                u32::from_le_bytes(bytes[data_start + 4..data_start + 8].try_into().ok()?),
                u16::from_le_bytes(bytes[data_start + 14..data_start + 16].try_into().ok()?),
            ));
        } else if chunk_id == b"data" {
            if !chunk_len.is_multiple_of(2) {
                return None;
            }
            let mut samples = Vec::with_capacity(chunk_len / 2);
            let mut cursor = data_start;
            while cursor + 1 < data_end {
                samples.push(i16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]));
                cursor += 2;
            }
            pcm = Some(samples);
        }
        offset = data_end + (chunk_len & 1);
    }
    let (audio_format, channels, sample_rate, bits) = format?;
    if audio_format != 1 || channels != 1 || bits != 16 || sample_rate == 0 {
        return None;
    }
    let samples = pcm.filter(|values| !values.is_empty())?;
    Some((sample_rate, samples))
}

fn encode_pcm16_mono_wav(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44usize.saturating_add(data_len as usize));
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate.saturating_mul(2);
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn sha256_file(path: &Path) -> Result<String, TranscriptionError> {
    let mut file = File::open(path)
        .map_err(|error| TranscriptionError::Unavailable(format!("{}: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| TranscriptionError::Unavailable(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn audio() -> CapturedAudioEvidence {
        CapturedAudioEvidence {
            audio_sha256: format!("sha256:{}", "a".repeat(64)),
            byte_length: 4,
            duration_samples: 2,
            sample_rate: 16_000,
            channels: 1,
            bits_per_sample: 16,
        }
    }

    fn write_fake_whisper(root: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let path = root.join("fake-whisper.cmd");
            let mut file = File::create(&path).unwrap();
            writeln!(
                file,
                "@echo off\r\nsetlocal EnableDelayedExpansion\r\nset \"OF=\"\r\n:parse\r\nif \"%~1\"==\"\" goto done\r\nif /I \"%~1\"==\"-of\" (\r\n  set \"OF=%~2\"\r\n  shift\r\n  shift\r\n  goto parse\r\n)\r\nshift\r\ngoto parse\r\n:done\r\nif defined OF (\r\n  >\"%OF%.txt\" echo hello from whisper\r\n) else (\r\n  echo hello from whisper\r\n)"
            )
            .unwrap();
            path
        }
        #[cfg(not(windows))]
        {
            let path = root.join("fake-whisper.sh");
            fs::write(
                &path,
                "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-of\" ]; then OF=$2; shift 2; continue; fi\n  shift\ndone\nif [ -n \"$OF\" ]; then printf 'hello from whisper\\n' > \"$OF.txt\"; else printf 'hello from whisper\\n'; fi\n",
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
            path
        }
    }

    #[tokio::test]
    async fn fake_executable_yields_text_and_stable_digest() {
        let root = std::env::temp_dir().join(format!("takegraph-whisper-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let executable = write_fake_whisper(&root);
        let model = root.join("model.bin");
        fs::write(&model, b"not-a-real-model").unwrap();
        let wav = root.join("note.wav");
        fs::write(&wav, b"RIFF").unwrap();

        let provider = WhisperCppProvider::new(&executable, &model, "ja", Vec::new()).unwrap();
        let first = provider.provider_digest().unwrap();
        let second = provider.provider_digest().unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));

        let result = provider.transcribe(&audio(), &wav).await.unwrap();
        assert_eq!(result.text, "hello from whisper");
        assert!(wav.is_file());
        assert!(provider.extra_args().contains(&"--no-speech-thold".into()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_executable_is_unavailable() {
        let error = WhisperCppProvider::new("missing-whisper", "missing-model", "ja", Vec::new());
        assert!(matches!(error, Err(TranscriptionError::Unavailable(_))));
    }

    #[test]
    fn decoder_defaults_fill_and_operator_overrides() {
        let filled = merge_whisper_decoder_args(Vec::new());
        assert_eq!(
            filled,
            vec![
                "--no-speech-thold",
                "0.4",
                "--entropy-thold",
                "2.2",
                "--logprob-thold",
                "-0.8",
            ]
        );
        let overridden =
            merge_whisper_decoder_args(vec!["-nth".into(), "0.2".into(), "--suppress-nst".into()]);
        assert_eq!(
            overridden,
            vec![
                "--entropy-thold",
                "2.2",
                "--logprob-thold",
                "-0.8",
                "-nth",
                "0.2",
                "--suppress-nst",
            ]
        );
    }

    #[test]
    fn trim_drops_trailing_silence_and_keeps_quiet_clips() {
        let rate = 16_000;
        let pad = (rate as usize) * ASR_PAD_MS as usize / 1000;
        let mut samples = vec![0i16; rate as usize];
        samples.extend(std::iter::repeat_n(2_000, 3_200));
        samples.extend(std::iter::repeat_n(0, rate as usize * 2));
        let trimmed = trim_pcm_for_asr(&samples, rate);
        assert_eq!(trimmed.len(), pad + 3_200 + pad);
        assert_eq!(
            &trimmed[pad..pad + 3_200],
            &samples[rate as usize..rate as usize + 3_200]
        );

        let quiet = vec![0i16; 8_000];
        assert_eq!(trim_pcm_for_asr(&quiet, rate).len(), 8_000);

        let speech = vec![3_000i16; 8_000];
        assert_eq!(trim_pcm_for_asr(&speech, rate).len(), 8_000);
    }

    #[tokio::test]
    async fn transcribe_does_not_rewrite_the_source_wav() {
        let root = std::env::temp_dir().join(format!("takegraph-asr-src-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let executable = write_fake_whisper(&root);
        let model = root.join("model.bin");
        fs::write(&model, b"not-a-real-model").unwrap();

        let mut samples = vec![0i16; 16_000];
        samples.extend(std::iter::repeat_n(2_000, 3_200));
        samples.extend(std::iter::repeat_n(0, 32_000));
        let bytes = encode_pcm16_mono_wav(&samples, 16_000);
        let wav = root.join("note.wav");
        fs::write(&wav, &bytes).unwrap();

        let provider = WhisperCppProvider::new(&executable, &model, "ja", Vec::new()).unwrap();
        let result = provider.transcribe(&audio(), &wav).await.unwrap();
        assert_eq!(result.text, "hello from whisper");
        assert_eq!(fs::read(&wav).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }
}
