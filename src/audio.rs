use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, SyncSender, TrySendError},
};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tracing::{info, warn};

/// Instantaneous mic level in milli-units (0..=1000) for HUD live meter.
pub type AudioLevelShare = Arc<AtomicU32>;

pub struct AudioHub {
    _stream: cpal::Stream,
    state: Arc<Mutex<AudioState>>,
    pub sample_rate_hz: u32,
    level_milli: AudioLevelShare,
}

pub struct AudioSession {
    pub rx: mpsc::Receiver<Vec<f32>>,
}

struct AudioState {
    ring: VecDeque<f32>,
    max_ring_samples: usize,
    subscribers: Vec<AudioSubscriber>,
    next_subscriber_id: u64,
}

struct AudioSubscriber {
    id: u64,
    tx: SyncSender<Vec<f32>>,
    dropped_chunks: u64,
}

impl AudioHub {
    pub fn start_default(ring_ms: u64) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default input device")?;
        let supported = device
            .default_input_config()
            .context("read default input config")?;
        let sample_rate_hz = supported.sample_rate();
        let channels = usize::from(supported.channels()).max(1);
        let stream_config: cpal::StreamConfig = supported.clone().into();
        let max_ring_samples = samples_for_ms(sample_rate_hz, ring_ms.max(100));
        let state = Arc::new(Mutex::new(AudioState {
            ring: VecDeque::with_capacity(max_ring_samples),
            max_ring_samples,
            subscribers: Vec::new(),
            next_subscriber_id: 1,
        }));
        let level_milli = Arc::new(AtomicU32::new(0));
        let err_fn = |error| tracing::error!(error = %error, "microphone input stream error");

        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                let state = Arc::clone(&state);
                let level_milli = Arc::clone(&level_milli);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| push_mono_f32(data, channels, &state, &level_milli),
                    err_fn,
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                let state = Arc::clone(&state);
                let level_milli = Arc::clone(&level_milli);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        let converted = data
                            .iter()
                            .map(|sample| f32::from(*sample) / f32::from(i16::MAX))
                            .collect::<Vec<_>>();
                        push_mono_f32(&converted, channels, &state, &level_milli);
                    },
                    err_fn,
                    None,
                )?
            }
            cpal::SampleFormat::U16 => {
                let state = Arc::clone(&state);
                let level_milli = Arc::clone(&level_milli);
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| {
                        let converted = data
                            .iter()
                            .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0)
                            .collect::<Vec<_>>();
                        push_mono_f32(&converted, channels, &state, &level_milli);
                    },
                    err_fn,
                    None,
                )?
            }
            other => bail!("unsupported input sample format: {other:?}"),
        };
        let started_at = Instant::now();
        stream
            .play()
            .context("start resident microphone input stream")?;
        #[allow(deprecated)]
        let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
        info!(
            device = %device_name,
            sample_rate_hz,
            channels,
            format = ?supported.sample_format(),
            ring_ms,
            max_ring_samples,
            startup_ms = started_at.elapsed().as_millis(),
            "resident microphone input started"
        );
        Ok(Self {
            _stream: stream,
            state,
            sample_rate_hz,
            level_milli,
        })
    }

    /// Shared 0..=1000 mic level for HUD (live update from the capture callback).
    pub fn level_share(&self) -> AudioLevelShare {
        Arc::clone(&self.level_milli)
    }

    pub fn subscribe(&self, pre_roll_ms: u64) -> AudioSession {
        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(32);
        let mut pre_roll = Vec::<f32>::new();
        if let Ok(mut state) = self.state.lock() {
            let subscriber_id = state.next_subscriber_id;
            state.next_subscriber_id += 1;
            let pre_roll_samples = samples_for_ms(self.sample_rate_hz, pre_roll_ms)
                .min(state.ring.len())
                .min(state.max_ring_samples);
            if pre_roll_samples > 0 {
                pre_roll.extend(state.ring.iter().rev().take(pre_roll_samples).copied());
                pre_roll.reverse();
                let _ = tx.try_send(pre_roll.clone());
            }
            state.subscribers.push(AudioSubscriber {
                id: subscriber_id,
                tx,
                dropped_chunks: 0,
            });
            info!(
                subscriber_id,
                pre_roll_ms,
                pre_roll_samples = pre_roll.len(),
                subscribers = state.subscribers.len(),
                queue_capacity_chunks = 32,
                "audio session subscribed to resident microphone"
            );
        } else {
            warn!("audio hub state lock poisoned while subscribing");
        }
        AudioSession { rx }
    }
}

fn push_mono_f32(
    data: &[f32],
    channels: usize,
    state: &Arc<Mutex<AudioState>>,
    level_milli: &AtomicU32,
) {
    if data.is_empty() {
        return;
    }
    let mut mono = Vec::with_capacity(data.len() / channels.max(1));
    for frame in data.chunks(channels) {
        let sum = frame.iter().copied().sum::<f32>();
        mono.push(sum / frame.len().max(1) as f32);
    }
    level_milli.store(sample_level_milli(&mono), Ordering::Relaxed);
    let Ok(mut state) = state.lock() else {
        return;
    };
    for sample in &mono {
        if state.ring.len() >= state.max_ring_samples {
            state.ring.pop_front();
        }
        state.ring.push_back(*sample);
    }
    let mut active = Vec::with_capacity(state.subscribers.len());
    for mut subscriber in state.subscribers.drain(..) {
        match subscriber.tx.try_send(mono.clone()) {
            Ok(()) => active.push(subscriber),
            Err(TrySendError::Full(_)) => {
                subscriber.dropped_chunks += 1;
                if subscriber.dropped_chunks == 1 || subscriber.dropped_chunks % 50 == 0 {
                    warn!(
                        subscriber_id = subscriber.id,
                        dropped_chunks = subscriber.dropped_chunks,
                        chunk_samples = mono.len(),
                        "audio subscriber backlog full; microphone chunk dropped"
                    );
                }
                active.push(subscriber);
            }
            Err(TrySendError::Disconnected(_)) => {
                info!(
                    subscriber_id = subscriber.id,
                    dropped_chunks = subscriber.dropped_chunks,
                    "audio subscriber disconnected"
                );
            }
        }
    }
    state.subscribers = active;
}

/// Map PCM RMS to 0..=1000. Room-mic speech often sits around -55..-12 dBFS;
/// use a hotter curve so quiet talk still fills most of the meter.
fn sample_level_milli(mono: &[f32]) -> u32 {
    if mono.is_empty() {
        return 0;
    }
    let mean_sq = mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32;
    let rms = mean_sq.sqrt();
    let db = if rms > 1e-9 {
        20.0 * rms.log10()
    } else {
        -100.0
    };
    // -55 dB → 0, -12 dB → 1, then mild gamma so mid speech jumps higher
    let linear = ((db + 55.0) / 43.0).clamp(0.0, 1.0);
    let hot = linear.powf(0.72);
    (hot * 1000.0).round() as u32
}

fn samples_for_ms(sample_rate_hz: u32, ms: u64) -> usize {
    ((sample_rate_hz.max(1) as u128 * ms as u128) / 1000) as usize
}
