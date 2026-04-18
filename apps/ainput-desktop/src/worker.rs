use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ainput_output::{OutputConfig, OutputDelivery};
use anyhow::Result;
use winit::event_loop::EventLoopProxy;

use crate::{AppEvent, AppRuntime, hotkey};

const VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);
const STREAMING_FINALIZE_POLL_INTERVAL: Duration = Duration::from_millis(8);
const STREAMING_FINALIZE_TIMEOUT: Duration = Duration::from_millis(800);
const STREAMING_SEGMENT_PASTE_INTERVAL: Duration = Duration::from_millis(45);
const STREAMING_STABLE_PREVIEW_MIN_CHARS: usize = 6;
const STREAMING_STABLE_PREVIEW_RESERVE_CHARS: usize = 6;

pub(crate) enum WorkerEvent {
    Started,
    RecordingStarted,
    Meter(f32),
    RecordingStopped,
    Transcribing,
    IgnoredSilence,
    Delivered,
    ClipboardFallback,
    StreamingStarted,
    StreamingPartial {
        raw_text: String,
        prepared_text: String,
    },
    StreamingFlushing,
    StreamingCommitted(String),
    StreamingClipboardFallback(String),
    StreamingFinal(String),
    Error(String),
    Unavailable(String),
}

#[derive(Clone, Copy, Debug)]
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

pub(crate) fn streaming_push_to_talk_worker(
    runtime: AppRuntime,
    proxy: EventLoopProxy<AppEvent>,
    shutdown: Arc<AtomicBool>,
    worker_rx: mpsc::Receiver<WorkerCommand>,
) {
    let recognizer = match build_streaming_recognizer(&runtime) {
        Ok(recognizer) => recognizer,
        Err(error) => {
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(format!(
                "初始化流式识别器失败：{error}"
            ))));
            return;
        }
    };

    let mut active_recording: Option<ainput_audio::ActiveRecording> = None;
    let mut active_stream: Option<ainput_asr::StreamingZipformerStream> = None;
    let mut sample_cursor = 0usize;
    let mut captured_samples = Vec::new();
    let mut last_partial = String::new();
    let mut last_prepared_preview = String::new();

    tracing::info!(
        shortcut = %runtime.config.hotkeys.voice_input,
        model_dir = %runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.voice.streaming.model_dir)
            .display(),
        "ainput streaming worker loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        if let Ok(command) = worker_rx.recv_timeout(Duration::from_millis(16)) {
            match command {
                WorkerCommand::HotkeyPressed => {
                    if active_recording.is_none() {
                        match ainput_audio::ActiveRecording::start_default_input() {
                            Ok(recording) => {
                                sample_cursor = 0;
                                captured_samples.clear();
                                last_partial.clear();
                                last_prepared_preview.clear();
                                active_stream = Some(recognizer.create_stream());
                                active_recording = Some(recording);
                                let _ = proxy
                                    .send_event(AppEvent::Worker(WorkerEvent::StreamingStarted));
                                tracing::info!("streaming push-to-talk recording started");
                            }
                            Err(error) => {
                                tracing::error!(error = %error, "failed to start streaming microphone recording");
                                let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                    format!("启动流式录音失败：{error}"),
                                )));
                            }
                        }
                    }
                }
                WorkerCommand::HotkeyReleased => {
                    if let Some(recording) = active_recording.take() {
                        if let Some(stream) = active_stream.take() {
                            flush_streaming_audio_chunk(
                                &recognizer,
                                &stream,
                                &recording,
                                &mut sample_cursor,
                                &mut captured_samples,
                                &mut last_partial,
                                &mut last_prepared_preview,
                                &proxy,
                            );

                            let _ =
                                proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingFlushing));
                            recognizer.input_finished(&stream);

                            let finalize_started_at = Instant::now();
                            while finalize_started_at.elapsed() < STREAMING_FINALIZE_TIMEOUT {
                                let decoded = recognizer.decode_available(&stream);
                                emit_streaming_partial_if_changed(
                                    &recognizer,
                                    &stream,
                                    &mut last_partial,
                                    &mut last_prepared_preview,
                                    &proxy,
                                );

                                let is_final = recognizer
                                    .get_result(&stream)
                                    .is_some_and(|result| result.is_final);
                                if is_final || decoded == 0 {
                                    if is_final {
                                        break;
                                    }
                                    thread::sleep(STREAMING_FINALIZE_POLL_INTERVAL);
                                }
                            }

                            let final_text = recognizer
                                .get_result(&stream)
                                .map(|result| result.text.trim().to_string())
                                .unwrap_or_default();

                            if final_text.is_empty() {
                                let _ = proxy
                                    .send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                            } else {
                                let rewritten_segments =
                                    build_streaming_output_segments(&runtime, &final_text);
                                if rewritten_segments.is_empty() {
                                    let _ = proxy.send_event(AppEvent::Worker(
                                        WorkerEvent::IgnoredSilence,
                                    ));
                                    drop(recording);
                                    continue;
                                }

                                let prepared_segments = rewritten_segments
                                    .iter()
                                    .map(|segment| runtime.output_controller.prepare_streaming_text(segment))
                                    .collect::<Result<Vec<_>>>();

                                let prepared_segments = match prepared_segments {
                                    Ok(segments) => segments,
                                    Err(error) => {
                                        let _ = proxy.send_event(AppEvent::Worker(
                                            WorkerEvent::Error(format!(
                                                "整理流式文本失败：{error}"
                                            )),
                                        ));
                                        drop(recording);
                                        continue;
                                    }
                                };
                                let prepared_full_text = prepared_segments.join("");

                                let hotkey_release_wait_started_at = Instant::now();
                                let modifiers_released = hotkey::wait_for_voice_hotkey_release(
                                    VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT,
                                );
                                let hotkey_release_wait_elapsed_ms =
                                    hotkey_release_wait_started_at.elapsed().as_millis();
                                if !modifiers_released {
                                    tracing::warn!(
                                        waited_ms = hotkey_release_wait_elapsed_ms,
                                        hotkey = %runtime.config.hotkeys.voice_input,
                                        "streaming output started before all modifiers fully released"
                                    );
                                }

                                let output_config = OutputConfig {
                                    prefer_direct_paste: runtime.config.voice.prefer_direct_paste,
                                    fallback_to_clipboard: runtime
                                        .config
                                        .voice
                                        .fallback_to_clipboard,
                                    voice_hotkey_uses_alt: hotkey::voice_hotkey_uses_alt(),
                                };

                                let direct_output_config = OutputConfig {
                                    prefer_direct_paste: true,
                                    fallback_to_clipboard: false,
                                    voice_hotkey_uses_alt: output_config.voice_hotkey_uses_alt,
                                };

                                let delivery = if output_config.prefer_direct_paste {
                                    let mut direct_paste_error = None;

                                    for (index, segment) in prepared_segments.iter().enumerate() {
                                        if let Err(error) = runtime
                                            .output_controller
                                            .paste_text_verbatim(segment, &direct_output_config)
                                        {
                                            direct_paste_error = Some(error);
                                            break;
                                        }

                                        let _ = proxy.send_event(AppEvent::Worker(
                                            WorkerEvent::StreamingCommitted(segment.clone()),
                                        ));

                                        if index + 1 < prepared_segments.len() {
                                            thread::sleep(STREAMING_SEGMENT_PASTE_INTERVAL);
                                        }
                                    }

                                    if let Some(error) = direct_paste_error {
                                        if !output_config.fallback_to_clipboard {
                                            let _ = proxy.send_event(AppEvent::Worker(
                                                WorkerEvent::Error(format!(
                                                    "输出流式文本失败：{error}"
                                                )),
                                            ));
                                            drop(recording);
                                            continue;
                                        }

                                        tracing::warn!(
                                            error = %error,
                                            "streaming direct paste failed, falling back to clipboard"
                                        );
                                        if let Err(copy_error) =
                                            ainput_output::copy_to_clipboard(&prepared_full_text)
                                        {
                                            let _ = proxy.send_event(AppEvent::Worker(
                                                WorkerEvent::Error(format!(
                                                    "复制流式文本失败：{copy_error}"
                                                )),
                                            ));
                                            drop(recording);
                                            continue;
                                        }
                                        let _ = proxy.send_event(AppEvent::Worker(
                                            WorkerEvent::StreamingClipboardFallback(
                                                prepared_full_text.clone(),
                                            ),
                                        ));
                                        OutputDelivery::ClipboardOnly
                                    } else {
                                        OutputDelivery::DirectPaste
                                    }
                                } else if output_config.fallback_to_clipboard {
                                    if let Err(error) =
                                        ainput_output::copy_to_clipboard(&prepared_full_text)
                                    {
                                        let _ = proxy.send_event(AppEvent::Worker(
                                            WorkerEvent::Error(format!(
                                                "复制流式文本失败：{error}"
                                            )),
                                        ));
                                        drop(recording);
                                        continue;
                                    }
                                    let _ = proxy.send_event(AppEvent::Worker(
                                        WorkerEvent::StreamingClipboardFallback(
                                            prepared_full_text.clone(),
                                        ),
                                    ));
                                    OutputDelivery::ClipboardOnly
                                } else {
                                    let _ = proxy.send_event(AppEvent::Worker(
                                        WorkerEvent::Error(
                                            "流式语音输出已关闭直贴和剪贴板回退".to_string(),
                                        ),
                                    ));
                                    drop(recording);
                                    continue;
                                };

                                runtime
                                    .shared_state
                                    .set_last_voice_text(prepared_full_text.clone());
                                runtime.maintenance.persist_voice_result(VoiceHistoryEntry {
                                    timestamp: current_timestamp(),
                                    delivery_label: streaming_delivery_label(delivery),
                                    text: prepared_full_text.clone(),
                                });

                                let _ = proxy.send_event(AppEvent::Worker(
                                    WorkerEvent::StreamingFinal(prepared_full_text),
                                ));
                            }
                        }

                        drop(recording);
                    }
                }
            }
        }

        if let Some(recording) = &active_recording {
            if let Some(stream) = &active_stream {
                flush_streaming_audio_chunk(
                    &recognizer,
                    stream,
                    recording,
                    &mut sample_cursor,
                    &mut captured_samples,
                    &mut last_partial,
                    &mut last_prepared_preview,
                    &proxy,
                );
            }

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

fn build_streaming_recognizer(runtime: &AppRuntime) -> Result<ainput_asr::StreamingZipformerRecognizer> {
    ainput_asr::StreamingZipformerRecognizer::create(&ainput_asr::StreamingZipformerConfig {
        model_dir: runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.voice.streaming.model_dir),
        provider: runtime.config.asr.provider.clone(),
        sample_rate_hz: runtime.config.asr.sample_rate_hz as i32,
        num_threads: runtime.config.asr.num_threads,
        decoding_method: "greedy_search".to_string(),
        enable_endpoint: false,
        rule1_min_trailing_silence: 2.4,
        rule2_min_trailing_silence: 1.2,
        rule3_min_utterance_length: 20.0,
    })
}

fn flush_streaming_audio_chunk(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    stream: &ainput_asr::StreamingZipformerStream,
    recording: &ainput_audio::ActiveRecording,
    sample_cursor: &mut usize,
    captured_samples: &mut Vec<f32>,
    last_partial: &mut String,
    last_prepared_preview: &mut String,
    proxy: &EventLoopProxy<AppEvent>,
) {
    let chunk = recording.take_new_samples(sample_cursor);
    if chunk.is_empty() {
        return;
    }

    captured_samples.extend_from_slice(&chunk);
    recognizer.accept_waveform(stream, recording.sample_rate_hz(), &chunk);
    let _ = recognizer.decode_available(stream);
    emit_streaming_partial_if_changed(
        recognizer,
        stream,
        last_partial,
        last_prepared_preview,
        proxy,
    );
}

fn emit_streaming_partial_if_changed(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    stream: &ainput_asr::StreamingZipformerStream,
    last_partial: &mut String,
    last_prepared_preview: &mut String,
    proxy: &EventLoopProxy<AppEvent>,
) {
    let Some(result) = recognizer.get_result(stream) else {
        return;
    };

    let text = result.text.trim().to_string();
    if text.is_empty() || *last_partial == text {
        return;
    }

    let prepared_text = build_streaming_prepared_preview(last_partial, &text);
    *last_partial = text.clone();
    if *last_prepared_preview != prepared_text {
        *last_prepared_preview = prepared_text.clone();
    }
    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
        raw_text: text,
        prepared_text,
    }));
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

fn streaming_delivery_label(delivery: OutputDelivery) -> &'static str {
    match delivery {
        OutputDelivery::DirectPaste => "streaming_direct_paste",
        OutputDelivery::ClipboardOnly => "streaming_clipboard_only",
    }
}

fn build_streaming_prepared_preview(previous_partial: &str, current_partial: &str) -> String {
    let common_prefix = common_prefix(previous_partial, current_partial);
    let stable_prefix = trim_stable_preview_prefix(&common_prefix);
    ainput_rewrite::normalize_streaming_preview(&stable_prefix)
}

fn trim_stable_preview_prefix(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len()
        <= STREAMING_STABLE_PREVIEW_MIN_CHARS + STREAMING_STABLE_PREVIEW_RESERVE_CHARS
    {
        return String::new();
    }

    let cutoff = chars.len() - STREAMING_STABLE_PREVIEW_RESERVE_CHARS;
    let adjusted_cutoff = safe_preview_cutoff(&chars, cutoff);
    chars[..adjusted_cutoff].iter().collect::<String>().trim().to_string()
}

fn safe_preview_cutoff(chars: &[char], cutoff: usize) -> usize {
    let mut index = cutoff.min(chars.len());
    while index > STREAMING_STABLE_PREVIEW_MIN_CHARS
        && chars
            .get(index - 1)
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && chars.get(index).is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        index -= 1;
    }

    index.max(STREAMING_STABLE_PREVIEW_MIN_CHARS)
}

fn common_prefix(left: &str, right: &str) -> String {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .map(|(ch, _)| ch)
        .collect()
}

fn build_streaming_output_segments(runtime: &AppRuntime, final_text: &str) -> Vec<String> {
    if runtime.config.voice.streaming.rewrite_enabled {
        ainput_rewrite::rewrite_streaming_text(final_text)
    } else {
        let normalized = ainput_rewrite::normalize_transcription(final_text);
        if normalized.is_empty() {
            Vec::new()
        } else {
            vec![normalized]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_streaming_prepared_preview, common_prefix, trim_stable_preview_prefix};

    #[test]
    fn common_prefix_keeps_shared_prefix_only() {
        assert_eq!(common_prefix("你好世界", "你好明天"), "你好");
    }

    #[test]
    fn stable_preview_keeps_safe_prefix() {
        assert_eq!(trim_stable_preview_prefix("帮我看一下这个功能现在"), "帮我看一下这个");
    }

    #[test]
    fn streaming_preview_uses_previous_partial() {
        assert_eq!(
            build_streaming_prepared_preview("帮我看一下这个功能", "帮我看一下这个功能有没有问题"),
            "帮我看一下这个"
        );
    }
}
