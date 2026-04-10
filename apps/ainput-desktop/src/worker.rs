use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ainput_output::{OutputConfig, OutputDelivery};
use anyhow::Result;
use winit::event_loop::EventLoopProxy;

use crate::{AppEvent, AppRuntime, hotkey};

const VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);

pub(crate) enum WorkerEvent {
    Started,
    RecordingStarted,
    Meter(f32),
    RecordingStopped,
    Transcribing,
    IgnoredSilence,
    Delivered,
    ClipboardFallback,
    Error(String),
}

pub(crate) enum WorkerCommand {
    HotkeyPressed,
    HotkeyReleased,
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceHistoryEntry {
    pub timestamp: String,
    pub delivery_label: &'static str,
    pub text: String,
}

pub(crate) fn push_to_talk_worker(
    runtime: AppRuntime,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    worker_rx: mpsc::Receiver<WorkerCommand>,
) {
    let recognizer = match build_recognizer(&runtime) {
        Ok(recognizer) => recognizer,
        Err(error) => {
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(format!(
                "初始化识别器失败：{error}"
            ))));
            return;
        }
    };

    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Started));
    let mut active_recording: Option<ainput_audio::ActiveRecording> = None;

    tracing::info!(
        shortcut = %runtime.config.hotkeys.voice_input,
        root_dir = %runtime.runtime_paths.root_dir.display(),
        "ainput worker loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        if let Ok(command) = worker_rx.recv_timeout(Duration::from_millis(16)) {
            match command {
                WorkerCommand::HotkeyPressed => {
                    if active_recording.is_none() {
                        match ainput_audio::ActiveRecording::start_default_input() {
                            Ok(recording) => {
                                active_recording = Some(recording);
                                let _ = proxy
                                    .send_event(AppEvent::Worker(WorkerEvent::RecordingStarted));
                                tracing::info!("push-to-talk recording started");
                            }
                            Err(error) => {
                                tracing::error!(error = %error, "failed to start microphone recording");
                                let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                    format!("启动录音失败：{error}"),
                                )));
                            }
                        }
                    }
                }
                WorkerCommand::HotkeyReleased => {
                    if let Some(recording) = active_recording.take() {
                        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::RecordingStopped));

                        match recording.stop() {
                            Ok(audio) => {
                                let pipeline_started_at = Instant::now();
                                let audio_duration_ms = if audio.sample_rate_hz > 0 {
                                    ((audio.samples.len() as f64 / audio.sample_rate_hz as f64)
                                        * 1000.0)
                                        .round() as u64
                                } else {
                                    0
                                };
                                let activity = analyze_audio_activity(&audio.samples);
                                tracing::info!(
                                    sample_rate_hz = audio.sample_rate_hz,
                                    frames = audio.samples.len(),
                                    audio_duration_ms,
                                    peak_abs = format_args!("{:.6}", activity.peak_abs),
                                    rms = format_args!("{:.6}", activity.rms),
                                    active_ratio = format_args!("{:.4}", activity.active_ratio),
                                    "push-to-talk recording captured"
                                );

                                if audio.samples.is_empty() {
                                    continue;
                                }

                                if should_skip_as_silence(&activity) {
                                    tracing::info!(
                                        audio_duration_ms,
                                        peak_abs = format_args!("{:.6}", activity.peak_abs),
                                        rms = format_args!("{:.6}", activity.rms),
                                        active_ratio = format_args!("{:.4}", activity.active_ratio),
                                        "skip transcription because captured audio looks like silence"
                                    );
                                    let _ = proxy
                                        .send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                                    continue;
                                }

                                let _ =
                                    proxy.send_event(AppEvent::Worker(WorkerEvent::Transcribing));
                                let asr_started_at = Instant::now();
                                match recognizer.transcribe_samples(
                                    audio.sample_rate_hz,
                                    &audio.samples,
                                    "microphone",
                                ) {
                                    Ok(transcription) => {
                                        let asr_elapsed_ms = asr_started_at.elapsed().as_millis();
                                        let raw_text = transcription.text.trim().to_string();

                                        if raw_text.is_empty() {
                                            continue;
                                        }

                                        if should_drop_low_signal_result(&raw_text, &activity) {
                                            tracing::info!(
                                                raw = %raw_text,
                                                peak_abs = format_args!("{:.6}", activity.peak_abs),
                                                rms = format_args!("{:.6}", activity.rms),
                                                active_ratio = format_args!("{:.4}", activity.active_ratio),
                                                "drop low-signal hallucinated transcription"
                                            );
                                            let _ = proxy.send_event(AppEvent::Worker(
                                                WorkerEvent::IgnoredSilence,
                                            ));
                                            continue;
                                        }

                                        let normalize_started_at = Instant::now();
                                        let text =
                                            ainput_rewrite::normalize_transcription(&raw_text);
                                        let normalize_elapsed_ms =
                                            normalize_started_at.elapsed().as_millis();
                                        if text != raw_text {
                                            tracing::info!(
                                                raw = %raw_text,
                                                normalized = %text,
                                                "normalized transcription text"
                                            );
                                        }
                                        let output_started_at = Instant::now();
                                        let hotkey_release_wait_started_at = Instant::now();
                                        let modifiers_released =
                                            hotkey::wait_for_voice_hotkey_release(
                                                VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT,
                                            );
                                        let hotkey_release_wait_elapsed_ms =
                                            hotkey_release_wait_started_at.elapsed().as_millis();
                                        if !modifiers_released {
                                            tracing::warn!(
                                                waited_ms = hotkey_release_wait_elapsed_ms,
                                                hotkey = %runtime.config.hotkeys.voice_input,
                                                "voice output started before all modifiers fully released"
                                            );
                                        }
                                        let output_config = OutputConfig {
                                            prefer_direct_paste: runtime
                                                .config
                                                .voice
                                                .prefer_direct_paste,
                                            fallback_to_clipboard: runtime
                                                .config
                                                .voice
                                                .fallback_to_clipboard,
                                            voice_hotkey_uses_alt: hotkey::voice_hotkey_uses_alt(),
                                        };
                                        match runtime
                                            .output_controller
                                            .deliver_text(&text, &output_config)
                                        {
                                            Ok(delivery) => {
                                                let output_elapsed_ms =
                                                    output_started_at.elapsed().as_millis();
                                                runtime
                                                    .shared_state
                                                    .set_last_voice_text(text.clone());
                                                runtime.maintenance.persist_voice_result(
                                                    VoiceHistoryEntry {
                                                        timestamp: current_timestamp(),
                                                        delivery_label: delivery_label(delivery),
                                                        text: text.clone(),
                                                    },
                                                );
                                                let pipeline_elapsed_ms =
                                                    pipeline_started_at.elapsed().as_millis();
                                                let realtime_factor = if audio_duration_ms > 0 {
                                                    pipeline_elapsed_ms as f64
                                                        / audio_duration_ms as f64
                                                } else {
                                                    0.0
                                                };
                                                tracing::info!(
                                                    ?delivery,
                                                    text = %text,
                                                    asr_elapsed_ms,
                                                    normalize_elapsed_ms,
                                                    hotkey_release_wait_elapsed_ms,
                                                    output_elapsed_ms,
                                                    pipeline_elapsed_ms,
                                                    audio_duration_ms,
                                                    realtime_factor = format_args!("{realtime_factor:.3}"),
                                                    "transcription delivered"
                                                );

                                                let event = match delivery {
                                                    OutputDelivery::DirectPaste => {
                                                        WorkerEvent::Delivered
                                                    }
                                                    OutputDelivery::ClipboardOnly => {
                                                        WorkerEvent::ClipboardFallback
                                                    }
                                                };
                                                let _ = proxy.send_event(AppEvent::Worker(event));
                                            }
                                            Err(error) => {
                                                tracing::error!(
                                                    error = %error,
                                                    "failed to deliver transcription output"
                                                );
                                                let _ = proxy.send_event(AppEvent::Worker(
                                                    WorkerEvent::Error(format!(
                                                        "输出结果失败：{error}"
                                                    )),
                                                ));
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        tracing::error!(
                                            error = %error,
                                            "failed to transcribe microphone audio"
                                        );
                                        let _ = proxy.send_event(AppEvent::Worker(
                                            WorkerEvent::Error(format!("语音识别失败：{error}")),
                                        ));
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::error!(error = %error, "failed to stop microphone recording");
                                let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                    format!("停止录音失败：{error}"),
                                )));
                            }
                        }
                    }
                }
            }
        }

        if let Some(recording) = &active_recording {
            let level = normalize_audio_level(recording.current_level());
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Meter(level)));
        }
    }
}

fn build_recognizer(runtime: &AppRuntime) -> Result<ainput_asr::SenseVoiceRecognizer> {
    ainput_asr::SenseVoiceRecognizer::create(&ainput_asr::SenseVoiceConfig {
        model_dir: runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.asr.model_dir),
        provider: runtime.config.asr.provider.clone(),
        sample_rate_hz: runtime.config.asr.sample_rate_hz as i32,
        language: runtime.config.asr.language.clone(),
        use_itn: runtime.config.asr.use_itn,
        num_threads: runtime.config.asr.num_threads,
    })
}

#[derive(Debug, Clone, Copy)]
struct AudioActivity {
    peak_abs: f32,
    rms: f32,
    active_ratio: f32,
}

fn analyze_audio_activity(samples: &[f32]) -> AudioActivity {
    if samples.is_empty() {
        return AudioActivity {
            peak_abs: 0.0,
            rms: 0.0,
            active_ratio: 0.0,
        };
    }

    let mut peak_abs = 0.0f32;
    let mut energy_sum = 0.0f64;
    let mut active_frames = 0usize;

    for sample in samples {
        let abs = sample.abs();
        peak_abs = peak_abs.max(abs);
        energy_sum += (abs as f64) * (abs as f64);
        if abs >= 0.008 {
            active_frames += 1;
        }
    }

    let rms = (energy_sum / samples.len() as f64).sqrt() as f32;
    let active_ratio = active_frames as f32 / samples.len() as f32;

    AudioActivity {
        peak_abs,
        rms,
        active_ratio,
    }
}

fn should_skip_as_silence(activity: &AudioActivity) -> bool {
    activity.peak_abs < 0.006 || (activity.rms < 0.0015 && activity.active_ratio < 0.01)
}

fn should_drop_low_signal_result(text: &str, activity: &AudioActivity) -> bool {
    if activity.rms >= 0.003 || activity.active_ratio >= 0.02 {
        return false;
    }

    let stripped = text
        .trim()
        .trim_matches(|ch: char| ch.is_whitespace() || is_sentence_punctuation(ch));

    if stripped.is_empty() {
        return true;
    }

    stripped.chars().count() <= 2
}

fn is_sentence_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '.' | ','
            | '!'
            | '?'
            | ';'
            | ':'
            | '。'
            | '，'
            | '！'
            | '？'
            | '、'
            | '；'
            | '：'
            | '．'
            | '・'
    )
}

fn normalize_audio_level(raw_level: f32) -> f32 {
    (raw_level * 6.5).sqrt().clamp(0.0, 1.0)
}

fn current_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.to_string()
}

fn delivery_label(delivery: OutputDelivery) -> &'static str {
    match delivery {
        OutputDelivery::DirectPaste => "voice_direct_paste",
        OutputDelivery::ClipboardOnly => "voice_clipboard_only",
    }
}
