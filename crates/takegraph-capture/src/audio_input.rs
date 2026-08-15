//! Microphone enumeration and 16 kHz mono 16-bit capture.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use serde::{Deserialize, Serialize};

use crate::error::CaptureError;
use crate::wav::TARGET_SAMPLE_RATE;

/// One input device the operator can select.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Receives already-converted 16 kHz mono 16-bit samples.
pub trait SampleSink: Send + Sync {
    /// Appends PCM samples. Returning an error stops further appends.
    ///
    /// # Errors
    ///
    /// Returns a duration or size limit when the recording is over budget.
    fn append_i16(&self, samples: &[i16]) -> Result<(), CaptureError>;
}

/// Opens and lists capture devices. Production uses [`CpalMicrophone`].
pub trait AudioInput: Send + Sync {
    /// Lists currently visible input devices.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Audio`] when the host cannot enumerate.
    fn list_devices(&self) -> Result<Vec<CaptureDevice>, CaptureError>;

    /// Returns the configured device id, if any.
    fn selected_device_id(&self) -> Option<String>;

    /// Selects a device by id, or the default when `id` is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Audio`] when the id is unknown.
    fn set_device(&self, id: Option<&str>) -> Result<(), CaptureError>;

    /// Starts streaming converted PCM into `sink`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Audio`] when the device cannot be opened.
    fn start(&self, sink: Arc<dyn SampleSink>) -> Result<(), CaptureError>;

    /// Stops the active stream.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Audio`] when the stream cannot be closed cleanly.
    fn stop(&self) -> Result<(), CaptureError>;
}

/// cpal-backed default microphone. The cpal `Stream` is `!Send`, so it lives
/// on a dedicated thread and is stopped by a channel.
pub struct CpalMicrophone {
    selected: Mutex<Option<String>>,
    stop: Mutex<Option<Sender<()>>>,
}

impl CpalMicrophone {
    /// Creates an unopened microphone using the system default device.
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected: Mutex::new(None),
            stop: Mutex::new(None),
        }
    }
}

impl Default for CpalMicrophone {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioInput for CpalMicrophone {
    fn list_devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        list_input_devices()
    }

    fn selected_device_id(&self) -> Option<String> {
        lock(&self.selected).clone()
    }

    fn set_device(&self, id: Option<&str>) -> Result<(), CaptureError> {
        if lock(&self.stop).is_some() {
            return Err(CaptureError::DeviceChangeWhileRecording);
        }
        if let Some(id) = id {
            let devices = list_input_devices()?;
            if !devices.iter().any(|device| device.id == id) {
                return Err(CaptureError::Audio(format!("unknown microphone: {id}")));
            }
            *lock(&self.selected) = Some(id.to_owned());
        } else {
            *lock(&self.selected) = None;
        }
        Ok(())
    }

    fn start(&self, sink: Arc<dyn SampleSink>) -> Result<(), CaptureError> {
        let mut stop_slot = lock(&self.stop);
        if stop_slot.is_some() {
            return Err(CaptureError::AlreadyRecording);
        }
        let selected = lock(&self.selected).clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        thread::Builder::new()
            .name("takegraph-capture-mic".into())
            .spawn(move || {
                let started = start_cpal_stream(selected.as_deref(), sink);
                match started {
                    Ok(_stream) => {
                        let _ = ready_tx.send(Ok(()));
                        let _ = stop_rx.recv();
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .map_err(CaptureError::Io)?;
        ready_rx
            .recv()
            .map_err(|_| CaptureError::Audio("microphone thread exited before opening".into()))??;
        *stop_slot = Some(stop_tx);
        Ok(())
    }

    fn stop(&self) -> Result<(), CaptureError> {
        if let Some(stop) = lock(&self.stop).take() {
            let _ = stop.send(());
        }
        Ok(())
    }
}

fn start_cpal_stream(
    selected: Option<&str>,
    sink: Arc<dyn SampleSink>,
) -> Result<cpal::Stream, CaptureError> {
    let (device, config) = open_input_device(selected)?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let format = config.sample_format();
    let stream_config: StreamConfig = config.into();
    let overflow = Arc::new(AtomicBool::new(false));
    let err_overflow = Arc::clone(&overflow);
    let push = move |data: Vec<i16>| {
        if overflow.load(Ordering::Relaxed) {
            return;
        }
        let converted = to_target_pcm(&data, channels, sample_rate);
        if sink.append_i16(&converted).is_err() {
            overflow.store(true, Ordering::Relaxed);
        }
    };
    let stream = match format {
        SampleFormat::F32 => device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    push(
                        data.iter()
                            .map(|sample| {
                                #[allow(clippy::cast_possible_truncation)]
                                {
                                    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
                                }
                            })
                            .collect(),
                    );
                },
                move |error| {
                    err_overflow.store(true, Ordering::Relaxed);
                    let _ = error;
                },
                None,
            )
            .map_err(|error| CaptureError::Audio(error.to_string()))?,
        SampleFormat::I16 => device
            .build_input_stream(
                &stream_config,
                move |data: &[i16], _| push(data.to_vec()),
                move |error| {
                    err_overflow.store(true, Ordering::Relaxed);
                    let _ = error;
                },
                None,
            )
            .map_err(|error| CaptureError::Audio(error.to_string()))?,
        SampleFormat::U16 => device
            .build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    push(
                        data.iter()
                            .map(|sample| i16::try_from(i32::from(*sample) - 32_768).unwrap_or(0))
                            .collect(),
                    );
                },
                move |error| {
                    err_overflow.store(true, Ordering::Relaxed);
                    let _ = error;
                },
                None,
            )
            .map_err(|error| CaptureError::Audio(error.to_string()))?,
        other => {
            return Err(CaptureError::Audio(format!(
                "unsupported microphone sample format: {other}"
            )));
        }
    };
    stream
        .play()
        .map_err(|error| CaptureError::Audio(error.to_string()))?;
    Ok(stream)
}

/// In-memory microphone used by unit tests.
pub struct ScriptedAudio {
    devices: Vec<CaptureDevice>,
    selected: Mutex<Option<String>>,
    pending_samples: Mutex<Vec<i16>>,
    fail_start: AtomicBool,
}

impl ScriptedAudio {
    /// Creates a single default fake device.
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: vec![CaptureDevice {
                id: "scripted".into(),
                name: "Scripted microphone".into(),
                is_default: true,
            }],
            selected: Mutex::new(None),
            pending_samples: Mutex::new(Vec::new()),
            fail_start: AtomicBool::new(false),
        }
    }

    /// Queues 16 kHz mono samples delivered on the next [`AudioInput::start`].
    pub fn queue_samples(&self, samples: Vec<i16>) {
        *lock(&self.pending_samples) = samples;
    }

    /// Forces the next start to fail without producing a capture.
    pub fn fail_next_start(&self) {
        self.fail_start.store(true, Ordering::SeqCst);
    }
}

impl Default for ScriptedAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioInput for ScriptedAudio {
    fn list_devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(self.devices.clone())
    }

    fn selected_device_id(&self) -> Option<String> {
        lock(&self.selected).clone()
    }

    fn set_device(&self, id: Option<&str>) -> Result<(), CaptureError> {
        if let Some(id) = id {
            if !self.devices.iter().any(|device| device.id == id) {
                return Err(CaptureError::Audio(format!("unknown microphone: {id}")));
            }
            *lock(&self.selected) = Some(id.to_owned());
        } else {
            *lock(&self.selected) = None;
        }
        Ok(())
    }

    fn start(&self, sink: Arc<dyn SampleSink>) -> Result<(), CaptureError> {
        if self.fail_start.swap(false, Ordering::SeqCst) {
            return Err(CaptureError::Audio("microphone open failed".into()));
        }
        let samples = lock(&self.pending_samples).clone();
        sink.append_i16(&samples)?;
        Ok(())
    }

    fn stop(&self) -> Result<(), CaptureError> {
        Ok(())
    }
}

fn list_input_devices() -> Result<Vec<CaptureDevice>, CaptureError> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let mut devices = Vec::new();
    let inputs = host
        .input_devices()
        .map_err(|error| CaptureError::Audio(error.to_string()))?;
    for (index, device) in inputs.enumerate() {
        let name = device.name().unwrap_or_else(|_| format!("input-{index}"));
        let is_default = default_name.as_deref() == Some(name.as_str());
        devices.push(CaptureDevice {
            id: format!("{index}:{name}"),
            name,
            is_default,
        });
    }
    Ok(devices)
}

fn is_virtual_input_name(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.contains("steam streaming")
        || lowered.contains("dualsense")
        || lowered.contains("voicemeeter")
        || lowered.contains("vb-audio")
        || lowered.contains("cable input")
        || lowered.contains("virtual")
}

fn preferred_input<'a>(devices: &'a [CaptureDevice]) -> Option<&'a CaptureDevice> {
    devices
        .iter()
        .find(|device| device.is_default && !is_virtual_input_name(&device.name))
        .or_else(|| {
            devices
                .iter()
                .find(|device| !is_virtual_input_name(&device.name))
        })
        .or_else(|| devices.iter().find(|device| device.is_default))
}

fn resolve_input_device(host: &cpal::Host) -> Result<cpal::Device, CaptureError> {
    let listed = list_input_devices()?;
    if let Some(preferred) = preferred_input(&listed) {
        if let Some(device) = host
            .input_devices()
            .map_err(|error| CaptureError::Audio(error.to_string()))?
            .find(|device| device.name().ok().as_deref() == Some(preferred.name.as_str()))
        {
            return Ok(device);
        }
    }
    host.default_input_device()
        .ok_or_else(|| CaptureError::Audio("no default microphone is available".into()))
}

fn open_input_device(
    selected_id: Option<&str>,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig), CaptureError> {
    let host = cpal::default_host();
    let device = if let Some(selected_id) = selected_id {
        let devices = list_input_devices()?;
        let wanted = devices
            .iter()
            .find(|device| device.id == selected_id)
            .ok_or_else(|| CaptureError::Audio(format!("unknown microphone: {selected_id}")))?;
        host.input_devices()
            .map_err(|error| CaptureError::Audio(error.to_string()))?
            .find(|device| device.name().ok().as_deref() == Some(wanted.name.as_str()))
            .ok_or_else(|| CaptureError::Audio(format!("unknown microphone: {selected_id}")))?
    } else {
        resolve_input_device(&host)?
    };
    let config = device
        .default_input_config()
        .map_err(|error| CaptureError::Audio(error.to_string()))?;
    Ok((device, config))
}

fn to_target_pcm(samples: &[i16], channels: u16, sample_rate: u32) -> Vec<i16> {
    let mono = if channels <= 1 {
        samples.to_vec()
    } else {
        let channels = usize::from(channels);
        samples
            .chunks(channels)
            .map(|frame| {
                let sum: i32 = frame.iter().map(|sample| i32::from(*sample)).sum();
                i16::try_from(sum / i32::try_from(channels).unwrap_or(1)).unwrap_or(0)
            })
            .collect()
    };
    resample_mono(&mono, sample_rate, TARGET_SAMPLE_RATE)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn resample_mono(input: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if input.is_empty() || from_rate == 0 || from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = f64::from(from_rate) / f64::from(to_rate);
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let last = input.len().saturating_sub(1);
    (0..out_len)
        .map(|index| {
            let src = index as f64 * ratio;
            let left = src.floor() as usize;
            let right = (left + 1).min(last);
            let frac = src - left as f64;
            let mixed = f64::from(input[left]).mul_add(1.0 - frac, f64::from(input[right]) * frac);
            mixed.round() as i16
        })
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Collect(Arc<Mutex<Vec<i16>>>);
    impl SampleSink for Collect {
        fn append_i16(&self, samples: &[i16]) -> Result<(), CaptureError> {
            lock(&self.0).extend_from_slice(samples);
            Ok(())
        }
    }

    #[test]
    fn steam_and_dualsense_are_not_preferred() {
        let devices = vec![
            CaptureDevice {
                id: "0:steam".into(),
                name: "マイク (Steam Streaming Microphone)".into(),
                is_default: true,
            },
            CaptureDevice {
                id: "1:headset".into(),
                name: "マイク (Arctis Nova 7)".into(),
                is_default: false,
            },
        ];
        assert_eq!(preferred_input(&devices).map(|d| d.id.as_str()), Some("1:headset"));
    }

    #[test]
    fn stereo_is_downmixed_and_resampled() {
        let stereo = [1000i16, 2000, 1000, 2000, 1000, 2000, 1000, 2000];
        let mono = to_target_pcm(&stereo, 2, TARGET_SAMPLE_RATE);
        assert_eq!(mono, vec![1500, 1500, 1500, 1500]);
    }

    #[test]
    fn scripted_audio_delivers_queued_samples() {
        let audio = ScriptedAudio::new();
        audio.queue_samples(vec![1, 2, 3]);
        let collected = Arc::new(Mutex::new(Vec::new()));
        audio
            .start(Arc::new(Collect(Arc::clone(&collected))))
            .unwrap();
        assert_eq!(*lock(&collected), vec![1, 2, 3]);
    }
}
