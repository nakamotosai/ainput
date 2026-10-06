use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, SyncSender, TrySendError},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tracing::{info, warn};

/// Instantaneous mic level in milli-units (0..=1000) for HUD live meter.
pub type AudioLevelShare = Arc<AtomicU32>;

/// No capture callback for this long => the resident stream is considered dead
/// (e.g. the USB mic was unplugged: cpal reports one error and then goes silent
/// forever). The watchdog rebuilds the stream instead of staying mute.
const STALL_THRESHOLD_MS: u64 = 5_000;
/// Watchdog poll interval.
const WATCHDOG_POLL_MS: u64 = 2_000;
/// Minimum gap between two rebuild-failure warnings (avoid log spam while the
/// device is still missing).
const REBUILD_WARN_QUIET_MS: u64 = 30_000;
/// Fallback idle-pause threshold when config omits it. 0 disables idle-pausing.
const DEFAULT_IDLE_PAUSE_MS: u64 = 30_000;
/// After a cold resume (`play()` on a paused handle) wait up to this long for the
/// first capture callback before falling back to a full rebuild — covers a dead
/// handle after real suspend / USB re-enumeration.
const RESUME_PRIMING_MS: u64 = 200;

pub struct AudioHub {
    /// The live capture stream plus whether it is currently paused. Kept for the
    /// hub lifetime; the watchdog owns a clone and does the actual pause/resume
    /// and rebuild swapping.
    mic: Arc<Mutex<MicState>>,
    state: Arc<Mutex<AudioState>>,
    /// Current input sample rate. Updated by the watchdog when a rebuild lands
    /// on a device with a different rate (USB mic unplug/replug can do that).
    sample_rate: Arc<AtomicU32>,
    level_milli: AudioLevelShare,
    health: Arc<HubHealth>,
    /// Ring length in ms, needed when the watchdog cold-rebuilds after a stall.
    ring_ms: u64,
    /// Pause the microphone after this many ms with no dictation activity
    /// (no subscribers). 0 disables idle-pausing (resident behavior).
    idle_pause_ms: u64,
    /// Epoch millis of the last subscribe() / active session. Drives idle-pause.
    last_activity_ms: Arc<AtomicU64>,
}

/// The capture stream and its paused flag. When `paused` is true the cpal stream
/// has been `pause()`d (WASAPI `IAudioClient::Stop`), which releases the
/// usbaudio SYSTEM power request so the machine can auto-sleep; the handle is
/// retained so resume is a cheap same-handle `play()`.
struct MicState {
    stream: Option<cpal::Stream>,
    paused: bool,
}

/// Stream liveness markers shared between the cpal callbacks and the watchdog.
struct HubHealth {
    /// Last time any capture callback fired (epoch millis).
    last_callback_ms: AtomicU64,
    /// Last time the stream error callback fired (epoch millis, 0 = never).
    last_error_ms: AtomicU64,
    stream_errors: AtomicU32,
    rebuilds: AtomicU32,
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl AudioHub {
    pub fn start_default(ring_ms: u64) -> Result<Self> {
        Self::start(ring_ms, DEFAULT_IDLE_PAUSE_MS)
    }

    /// Start with an explicit idle-pause threshold (0 = never pause).
    pub fn start(ring_ms: u64, idle_pause_ms: u64) -> Result<Self> {
        let health = Arc::new(HubHealth {
            last_callback_ms: AtomicU64::new(now_ms()),
            last_error_ms: AtomicU64::new(0),
            stream_errors: AtomicU32::new(0),
            rebuilds: AtomicU32::new(0),
        });
        let state = Arc::new(Mutex::new(AudioState {
            ring: VecDeque::new(),
            // max_ring_samples needs a rate; recomputed on first build below.
            max_ring_samples: samples_for_ms(48_000, ring_ms.max(100)),
            subscribers: Vec::new(),
            next_subscriber_id: 1,
        }));
        let level_milli = Arc::new(AtomicU32::new(0));
        let sample_rate = Arc::new(AtomicU32::new(48_000));

        let started_at = Instant::now();
        let (stream, rate_hz, device_name) = build_input_stream(
            Arc::clone(&state),
            Arc::clone(&level_milli),
            Arc::clone(&health),
            ring_ms,
        )?;
        stream
            .play()
            .context("start resident microphone input stream")?;
        sample_rate.store(rate_hz, Ordering::Relaxed);
        if let Ok(mut state) = state.lock() {
            state.max_ring_samples = samples_for_ms(rate_hz, ring_ms.max(100));
        }
        info!(
            device = %device_name,
            sample_rate_hz = rate_hz,
            ring_ms,
            startup_ms = started_at.elapsed().as_millis(),
            "resident microphone input started"
        );

        let stream = Arc::new(Mutex::new(MicState {
            stream: Some(stream),
            paused: false,
        }));
        let last_activity_ms = Arc::new(AtomicU64::new(now_ms()));
        spawn_watchdog(
            Arc::clone(&stream),
            Arc::clone(&state),
            Arc::clone(&sample_rate),
            Arc::clone(&level_milli),
            Arc::clone(&health),
            ring_ms,
            Arc::clone(&last_activity_ms),
            idle_pause_ms,
        );
        info!(
            stall_threshold_ms = STALL_THRESHOLD_MS,
            poll_ms = WATCHDOG_POLL_MS,
            idle_pause_ms,
            "audio watchdog started"
        );

        Ok(Self {
            mic: stream,
            state,
            sample_rate,
            level_milli,
            health,
            ring_ms,
            idle_pause_ms,
            last_activity_ms,
        })
    }

    /// Current input sample rate in Hz (tracks the live device across rebuilds).
    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed).max(1)
    }

    /// Shared 0..=1000 mic level for HUD (live update from the capture callback).
    pub fn level_share(&self) -> AudioLevelShare {
        Arc::clone(&self.level_milli)
    }

    /// Watchdog-visible liveness: millis since the last capture callback.
    #[allow(dead_code)]
    pub fn millis_since_callback(&self) -> u64 {
        now_ms().saturating_sub(self.health.last_callback_ms.load(Ordering::Relaxed))
    }

    pub fn subscribe(&self, pre_roll_ms: u64) -> AudioSession {
        // Mark activity and, if the mic was idle-paused, resume it synchronously
        // (same-handle `play()`, sub-millisecond) so the first syllable of this
        // utterance is still captured inside the pre-press window.
        self.ensure_live();
        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(32);
        let mut pre_roll = Vec::<f32>::new();
        if let Ok(mut state) = self.state.lock() {
            // Opportunistic cleanup: disconnected receivers are normally pruned
            // when the next microphone chunk arrives, but a dead stream delivers
            // no chunks, so prune here too. Probing with an empty chunk is
            // harmless downstream (resampler push of zero samples is a no-op);
            // a full channel counts as alive without penalizing drops.
            let pruned = prune_disconnected(&mut state.subscribers);
            let subscriber_id = state.next_subscriber_id;
            state.next_subscriber_id += 1;
            let rate_hz = self.sample_rate_hz();
            let pre_roll_samples = samples_for_ms(rate_hz, pre_roll_ms)
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
                pruned_disconnected = pruned,
                "audio session subscribed to resident microphone"
            );
        } else {
            warn!("audio hub state lock poisoned while subscribing");
        }
        AudioSession { rx }
    }

    /// Ensure the microphone is live. Bumps the activity clock unconditionally;
    /// if the stream was idle-paused, resumes it (`play()` on the same handle).
    /// If the handle turns out dead (post-suspend / USB re-enumeration), falls
    /// back to a full rebuild.
    fn ensure_live(&self) {
        self.last_activity_ms.store(now_ms(), Ordering::Relaxed);
        let mut mic = match self.mic.lock() {
            Ok(mic) => mic,
            Err(_) => return,
        };
        if !mic.paused {
            return;
        }
        // Try the cheap same-handle resume first.
        let resumed = mic
            .stream
            .as_ref()
            .map(|stream| stream.play().is_ok())
            .unwrap_or(false);
        if resumed && self.wait_first_callback(RESUME_PRIMING_MS) {
            mic.paused = false;
            self.health
                .last_callback_ms
                .store(now_ms(), Ordering::Relaxed);
            if let Ok(mut state) = self.state.lock() {
                // Drop pre-pause audio: it must not leak in as pre-roll.
                state.ring.clear();
            }
            info!("microphone resumed from idle pause (same-handle play)");
            return;
        }
        // Handle is dead: rebuild from scratch.
        warn!("microphone resume failed or produced no audio; rebuilding stream");
        match build_input_stream(
            Arc::clone(&self.state),
            Arc::clone(&self.level_milli),
            Arc::clone(&self.health),
            self.ring_ms,
        ) {
            Ok((fresh, rate_hz, device_name)) => {
                if let Err(error) = fresh.play() {
                    warn!(error = %format!("{error:#}"), "rebuilt microphone failed to play");
                    return;
                }
                let _ = self.wait_first_callback(RESUME_PRIMING_MS);
                mic.stream = Some(fresh);
                mic.paused = false;
                self.sample_rate.store(rate_hz, Ordering::Relaxed);
                self.health
                    .last_callback_ms
                    .store(now_ms(), Ordering::Relaxed);
                if let Ok(mut state) = self.state.lock() {
                    state.ring.clear();
                    state.max_ring_samples = samples_for_ms(rate_hz, self.ring_ms.max(100));
                }
                info!(device = %device_name, sample_rate_hz = rate_hz, "microphone rebuilt on resume");
            }
            Err(error) => {
                warn!(
                    error = %format!("{error:#}"),
                    "microphone rebuild on resume failed; session will run dry and be gate-skipped"
                );
            }
        }
    }

    /// Spin (bounded) until a capture callback lands, so the caller knows the
    /// stream is actually delivering audio. Returns false on timeout.
    fn wait_first_callback(&self, max_ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(max_ms);
        loop {
            let idle =
                now_ms().saturating_sub(self.health.last_callback_ms.load(Ordering::Relaxed));
            if idle < 50 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Drop receivers whose session already ended. Returns the pruned count.
/// A `Full` channel is treated as alive (slow consumer, not a dead one) and an
/// empty probe chunk is sent only when there is room; downstream stages treat
/// zero-sample chunks as no-ops.
fn prune_disconnected(subscribers: &mut Vec<AudioSubscriber>) -> usize {
    let before = subscribers.len();
    subscribers.retain_mut(|subscriber| match subscriber.tx.try_send(Vec::new()) {
        Ok(()) | Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => {
            info!(
                subscriber_id = subscriber.id,
                dropped_chunks = subscriber.dropped_chunks,
                "audio subscriber disconnected (pruned on subscribe)"
            );
            false
        }
    });
    before - subscribers.len()
}

/// Build (but do not play) an input stream on the current default device.
/// Returns the stream plus the device sample rate and name for logging.
fn build_input_stream(
    state: Arc<Mutex<AudioState>>,
    level_milli: Arc<AtomicU32>,
    health: Arc<HubHealth>,
    _ring_ms: u64,
) -> Result<(cpal::Stream, u32, String)> {
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
    #[allow(deprecated)]
    let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());

    // The error callback only marks liveness; the watchdog thread owns recovery
    // (cpal gives no restart handle, so rebuilding from inside the callback is
    // not possible).
    let err_health = Arc::clone(&health);
    let err_fn = move |error| {
        err_health
            .last_error_ms
            .store(now_ms(), Ordering::Relaxed);
        err_health.stream_errors.fetch_add(1, Ordering::Relaxed);
        tracing::error!(error = %error, "microphone input stream error");
    };

    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let state = Arc::clone(&state);
            let level_milli = Arc::clone(&level_milli);
            let health = Arc::clone(&health);
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    push_mono_f32(data, channels, &state, &level_milli, &health)
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let state = Arc::clone(&state);
            let level_milli = Arc::clone(&level_milli);
            let health = Arc::clone(&health);
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    let converted = data
                        .iter()
                        .map(|sample| f32::from(*sample) / f32::from(i16::MAX))
                        .collect::<Vec<_>>();
                    push_mono_f32(&converted, channels, &state, &level_milli, &health);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let state = Arc::clone(&state);
            let level_milli = Arc::clone(&level_milli);
            let health = Arc::clone(&health);
            device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    let converted = data
                        .iter()
                        .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect::<Vec<_>>();
                    push_mono_f32(&converted, channels, &state, &level_milli, &health);
                },
                err_fn,
                None,
            )?
        }
        other => bail!("unsupported input sample format: {other:?}"),
    };
    Ok((stream, sample_rate_hz, device_name))
}

/// Background supervisor: if no capture callback arrives for longer than
/// STALL_THRESHOLD_MS (device unplugged, driver hiccup, rate change), rebuild
/// the stream on the current default device. Runs for the process lifetime.
fn spawn_watchdog(
    stream: Arc<Mutex<MicState>>,
    state: Arc<Mutex<AudioState>>,
    sample_rate: Arc<AtomicU32>,
    level_milli: Arc<AtomicU32>,
    health: Arc<HubHealth>,
    ring_ms: u64,
    last_activity_ms: Arc<AtomicU64>,
    idle_pause_ms: u64,
) {
    let _ = std::thread::Builder::new()
        .name("ainput-audio-watchdog".to_string())
        .spawn(move || {
            let mut last_warn_ms = 0u64;
            loop {
                std::thread::sleep(Duration::from_millis(WATCHDOG_POLL_MS));
                // Any live subscriber means a dictation session is in flight.
                let subscribers = state
                    .lock()
                    .map(|s| s.subscribers.len())
                    .unwrap_or(0);
                if subscribers > 0 {
                    last_activity_ms.store(now_ms(), Ordering::Relaxed);
                }
                let mut mic = match stream.lock() {
                    Ok(mic) => mic,
                    Err(_) => continue,
                };
                // Idle-paused: do NOT run the stall detector. Callbacks have
                // stopped by design, so last_callback_ms is frozen; without this
                // guard the stall branch below would rebuild (un-pause) the mic
                // within seconds and defeat the whole feature.
                if mic.paused {
                    continue;
                }
                let idle_ms = now_ms()
                    .saturating_sub(health.last_callback_ms.load(Ordering::Relaxed));
                if idle_ms >= STALL_THRESHOLD_MS {
                    let errors = health.stream_errors.load(Ordering::Relaxed);
                    warn!(
                        idle_ms,
                        stream_errors = errors,
                        "microphone stream stalled, rebuilding on default input device"
                    );
                    match build_input_stream(
                        Arc::clone(&state),
                        Arc::clone(&level_milli),
                        Arc::clone(&health),
                        ring_ms,
                    ) {
                        Ok((fresh, rate_hz, device_name)) => {
                            if let Err(error) = fresh.play() {
                                warn!(
                                    error = %format!("{error:#}"),
                                    device = %device_name,
                                    "rebuilt microphone stream failed to play; will retry"
                                );
                                continue;
                            }
                            mic.stream = Some(fresh);
                            mic.paused = false;
                            sample_rate.store(rate_hz, Ordering::Relaxed);
                            if let Ok(mut state) = state.lock() {
                                // Drop pre-stall audio: transcribing minutes-old
                                // buffered speech after recovery would be wrong.
                                state.ring.clear();
                                state.max_ring_samples =
                                    samples_for_ms(rate_hz, ring_ms.max(100));
                            }
                            level_milli.store(0, Ordering::Relaxed);
                            health
                                .last_callback_ms
                                .store(now_ms(), Ordering::Relaxed);
                            let rebuilds =
                                health.rebuilds.fetch_add(1, Ordering::Relaxed) + 1;
                            info!(
                                device = %device_name,
                                sample_rate_hz = rate_hz,
                                idle_ms,
                                rebuilds,
                                "microphone stream rebuilt after stall"
                            );
                        }
                        Err(error) => {
                            let now = now_ms();
                            if now.saturating_sub(last_warn_ms) >= REBUILD_WARN_QUIET_MS {
                                last_warn_ms = now;
                                warn!(
                                    error = %format!("{error:#}"),
                                    idle_ms,
                                    "microphone rebuild failed (device still missing?); will keep retrying"
                                );
                            }
                        }
                    }
                    continue;
                }
                // Idle-pause: no subscribers and no recent activity. Pause the
                // stream (releases the usbaudio SYSTEM power request) but keep
                // the handle for a cheap same-handle resume.
                if idle_pause_ms > 0
                    && subscribers == 0
                    && now_ms().saturating_sub(last_activity_ms.load(Ordering::Relaxed))
                        >= idle_pause_ms
                    && mic.stream.is_some()
                {
                    if let Some(stream) = mic.stream.as_ref() {
                        if let Err(error) = stream.pause() {
                            warn!(error = %format!("{error:#}"), "idle mic pause failed; will retry");
                            continue;
                        }
                    }
                    mic.paused = true;
                    level_milli.store(0, Ordering::Relaxed);
                    if let Ok(mut state) = state.lock() {
                        state.ring.clear();
                    }
                    info!(
                        idle_ms = now_ms()
                            .saturating_sub(last_activity_ms.load(Ordering::Relaxed)),
                        "microphone idle-paused (power request released)"
                    );
                }
            }
        });
}

fn push_mono_f32(
    data: &[f32],
    channels: usize,
    state: &Arc<Mutex<AudioState>>,
    level_milli: &AtomicU32,
    health: &Arc<HubHealth>,
) {
    // Any callback (even an empty one) proves the stream is alive.
    health
        .last_callback_ms
        .store(now_ms(), Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_subscriber() -> (AudioSubscriber, mpsc::Receiver<Vec<f32>>) {
        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(32);
        (
            AudioSubscriber {
                id: 7,
                tx,
                dropped_chunks: 0,
            },
            rx,
        )
    }

    #[test]
    fn silence_maps_to_zero_level() {
        assert_eq!(sample_level_milli(&[]), 0);
        assert_eq!(sample_level_milli(&[0.0; 160]), 0);
    }

    #[test]
    fn loud_signal_maps_near_full_level() {
        let level = sample_level_milli(&[0.5; 160]);
        assert!(level > 900, "expected hot level, got {level}");
    }

    #[test]
    fn samples_for_ms_scales_with_rate() {
        assert_eq!(samples_for_ms(48_000, 160), 7680);
        assert_eq!(samples_for_ms(16_000, 160), 2560);
    }

    #[test]
    fn prune_keeps_live_and_full_but_drops_dead() {
        let (live, _live_rx) = test_subscriber();
        let (dead, dead_rx) = test_subscriber();
        drop(dead_rx);
        let (full, _full_rx) = test_subscriber();
        // Fill the channel so the probe sees `Full`, not `Ok`.
        for _ in 0..32 {
            let _ = full.tx.try_send(vec![0.1]);
        }
        let mut subs = vec![live, dead, full];
        let pruned = prune_disconnected(&mut subs);
        assert_eq!(pruned, 1);
        assert_eq!(subs.len(), 2);
    }
}
