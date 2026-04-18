use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ainput_output::{OutputConfig, OutputDelivery};
use anyhow::Result;
use winit::event_loop::EventLoopProxy;

use crate::{AppEvent, AppRuntime, hotkey};

const VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);
const STREAMING_OUTPUT_HOTKEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(80);

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

struct StreamingSession {
    recording: ainput_audio::ActiveRecording,
    stream: ainput_asr::StreamingZipformerStream,
    sample_cursor: usize,
    captured_samples: Vec<f32>,
    last_partial: String,
    last_prepared_preview: String,
    last_preview_at: Instant,
    last_preview_sample_count: usize,
    started_at: Instant,
}

impl StreamingSession {
    fn new(
        recording: ainput_audio::ActiveRecording,
        stream: ainput_asr::StreamingZipformerStream,
    ) -> Self {
        Self {
            recording,
            stream,
            sample_cursor: 0,
            captured_samples: Vec::new(),
            last_partial: String::new(),
            last_prepared_preview: String::new(),
            last_preview_at: Instant::now(),
            last_preview_sample_count: 0,
            started_at: Instant::now(),
        }
    }
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
                                            paste_stabilize_delay: Duration::from_millis(35),
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

    let mut active_session: Option<StreamingSession> = None;
    let preview_interval = streaming_preview_interval(&runtime);

    tracing::info!(
        shortcut = %runtime.config.hotkeys.voice_input,
        model_dir = %runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.voice.streaming.model_dir)
            .display(),
        preview_interval_ms = preview_interval.as_millis(),
        "ainput streaming worker loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        if let Ok(command) = worker_rx.recv_timeout(Duration::from_millis(16)) {
            match command {
                WorkerCommand::HotkeyPressed => {
                    if active_session.is_none() {
                        match ainput_audio::ActiveRecording::start_default_input() {
                            Ok(recording) => {
                                let stream = recognizer.create_stream();
                                active_session = Some(StreamingSession::new(recording, stream));
                                let _ = proxy
                                    .send_event(AppEvent::Worker(WorkerEvent::StreamingStarted));
                                tracing::info!("streaming push-to-talk recording started");
                            }
                            Err(error) => {
                                tracing::error!(
                                    error = %error,
                                    "failed to start streaming microphone recording"
                                );
                                let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                    format!("启动流式录音失败：{error}"),
                                )));
                            }
                        }
                    }
                }
                WorkerCommand::HotkeyReleased => {
                    if let Some(mut session) = active_session.take() {
                        let final_chunk_update = collect_streaming_audio_chunk(
                            &session.recording,
                            &mut session.sample_cursor,
                            &mut session.captured_samples,
                            &recognizer,
                            &session.stream,
                        );

                        let audio_duration_ms = audio_duration_ms(
                            session.recording.sample_rate_hz(),
                            session.captured_samples.len(),
                        );
                        let activity = analyze_audio_activity(&session.captured_samples);
                        tracing::info!(
                            sample_rate_hz = session.recording.sample_rate_hz(),
                            frames = session.captured_samples.len(),
                            audio_duration_ms,
                            peak_abs = format_args!("{:.6}", activity.peak_abs),
                            rms = format_args!("{:.6}", activity.rms),
                            active_ratio = format_args!("{:.4}", activity.active_ratio),
                            added_samples = final_chunk_update.added_samples,
                            decoded_steps = final_chunk_update.decoded_steps,
                            "streaming push-to-talk recording captured"
                        );

                        if session.captured_samples.is_empty() || should_skip_as_silence(&activity) {
                            tracing::info!(
                                audio_duration_ms,
                                peak_abs = format_args!("{:.6}", activity.peak_abs),
                                rms = format_args!("{:.6}", activity.rms),
                                active_ratio = format_args!("{:.4}", activity.active_ratio),
                                "skip streaming transcription because captured audio looks like silence"
                            );
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                            continue;
                        }

                        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingFlushing));

                        let final_drain_started_at = Instant::now();
                        recognizer.input_finished(&session.stream);
                        let final_decode_steps = recognizer.decode_available(&session.stream);
                        let final_drain_elapsed_ms = final_drain_started_at.elapsed().as_millis();

                        let raw_text = recognizer
                            .get_result(&session.stream)
                            .map(|result| result.text.trim().to_string())
                            .unwrap_or_default();
                        if raw_text.is_empty()
                            || should_drop_low_signal_result(&raw_text, &activity)
                        {
                            tracing::info!(
                                raw_text = %raw_text,
                                audio_duration_ms,
                                final_drain_elapsed_ms,
                                final_decode_steps,
                                peak_abs = format_args!("{:.6}", activity.peak_abs),
                                rms = format_args!("{:.6}", activity.rms),
                                active_ratio = format_args!("{:.4}", activity.active_ratio),
                                "drop empty or low-signal streaming transcription"
                            );
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                            continue;
                        }

                        let rewrite_started_at = Instant::now();
                        let prepared_full_text = build_streaming_output_text(&runtime, &raw_text);
                        let rewrite_elapsed_ms = rewrite_started_at.elapsed().as_millis();
                        if prepared_full_text.is_empty() {
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                            continue;
                        }

                        tracing::info!(
                            samples = session.captured_samples.len(),
                            audio_duration_ms,
                            final_decode_steps,
                            final_drain_elapsed_ms,
                            rewrite_elapsed_ms,
                            raw_text = %raw_text,
                            prepared_text = %prepared_full_text,
                            "streaming final transcription ready"
                        );

                        let hotkey_release_wait_started_at = Instant::now();
                        let modifiers_released = hotkey::wait_for_voice_hotkey_release(
                            STREAMING_OUTPUT_HOTKEY_RELEASE_TIMEOUT,
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
                            fallback_to_clipboard: runtime.config.voice.fallback_to_clipboard,
                            voice_hotkey_uses_alt: hotkey::voice_hotkey_uses_alt(),
                            paste_stabilize_delay: Duration::from_millis(10),
                        };

                        let output_started_at = Instant::now();
                        let delivery = match runtime
                            .output_controller
                            .deliver_text(&prepared_full_text, &output_config)
                        {
                            Ok(delivery) => {
                                if matches!(delivery, OutputDelivery::ClipboardOnly) {
                                    let _ = proxy.send_event(AppEvent::Worker(
                                        WorkerEvent::StreamingClipboardFallback(
                                            prepared_full_text.clone(),
                                        ),
                                    ));
                                }
                                delivery
                            }
                            Err(error) => {
                                let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                    format!("输出流式文本失败：{error}"),
                                )));
                                continue;
                            }
                        };
                        let output_elapsed_ms = output_started_at.elapsed().as_millis();

                        runtime
                            .shared_state
                            .set_last_voice_text(prepared_full_text.clone());
                        runtime.maintenance.persist_voice_result(VoiceHistoryEntry {
                            timestamp: current_timestamp(),
                            delivery_label: streaming_delivery_label(delivery),
                            text: prepared_full_text.clone(),
                        });
                        let pipeline_elapsed_ms = session.started_at.elapsed().as_millis();
                        let realtime_factor = if audio_duration_ms > 0 {
                            pipeline_elapsed_ms as f64 / audio_duration_ms as f64
                        } else {
                            0.0
                        };
                        tracing::info!(
                            ?delivery,
                            text = %prepared_full_text,
                            audio_duration_ms,
                            final_decode_steps,
                            final_drain_elapsed_ms,
                            rewrite_elapsed_ms,
                            hotkey_release_wait_elapsed_ms,
                            output_elapsed_ms,
                            pipeline_elapsed_ms,
                            realtime_factor = format_args!("{realtime_factor:.3}"),
                            "streaming transcription delivered"
                        );

                        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingFinal(
                            prepared_full_text,
                        )));
                    }
                }
            }
        }

        if let Some(session) = &mut active_session {
            let chunk_update = collect_streaming_audio_chunk(
                &session.recording,
                &mut session.sample_cursor,
                &mut session.captured_samples,
                &recognizer,
                &session.stream,
            );
            let now = Instant::now();
            let min_preview_samples = streaming_preview_min_samples(
                session.recording.sample_rate_hz(),
                runtime.config.voice.streaming.chunk_ms as u64,
            );
            if session.captured_samples.len() >= min_preview_samples
                && session.captured_samples.len() > session.last_preview_sample_count
                && (chunk_update.added_samples > 0
                    || now.duration_since(session.last_preview_at) >= preview_interval)
            {
                if let Err(error) = emit_streaming_partial_if_changed(
                    &recognizer,
                    &session.stream,
                    session.recording.sample_rate_hz(),
                    &session.captured_samples,
                    runtime.config.voice.streaming.rewrite_enabled,
                    &mut session.last_partial,
                    &mut session.last_prepared_preview,
                    &proxy,
                ) {
                    tracing::warn!(
                        error = %error,
                        samples = session.captured_samples.len(),
                        decoded_steps = chunk_update.decoded_steps,
                        "streaming live preview decode failed"
                    );
                }
                session.last_preview_at = now;
                session.last_preview_sample_count = session.captured_samples.len();
            }

            let level = normalize_audio_level(session.recording.current_level());
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

fn build_streaming_recognizer(
    runtime: &AppRuntime,
) -> Result<ainput_asr::StreamingZipformerRecognizer> {
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

struct StreamingChunkUpdate {
    added_samples: usize,
    decoded_steps: usize,
}

fn collect_streaming_audio_chunk(
    recording: &ainput_audio::ActiveRecording,
    sample_cursor: &mut usize,
    captured_samples: &mut Vec<f32>,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    stream: &ainput_asr::StreamingZipformerStream,
) -> StreamingChunkUpdate {
    let chunk = recording.take_new_samples(sample_cursor);
    if chunk.is_empty() {
        return StreamingChunkUpdate {
            added_samples: 0,
            decoded_steps: 0,
        };
    }

    recognizer.accept_waveform(stream, recording.sample_rate_hz(), &chunk);
    let decoded_steps = recognizer.decode_available(stream);
    captured_samples.extend_from_slice(&chunk);

    StreamingChunkUpdate {
        added_samples: chunk.len(),
        decoded_steps,
    }
}

fn emit_streaming_partial_if_changed(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    stream: &ainput_asr::StreamingZipformerStream,
    sample_rate_hz: i32,
    captured_samples: &[f32],
    rewrite_enabled: bool,
    last_partial: &mut String,
    last_prepared_preview: &mut String,
    proxy: &EventLoopProxy<AppEvent>,
) -> Result<()> {
    let audio_duration_ms = audio_duration_ms(sample_rate_hz, captured_samples.len());
    let activity = analyze_recent_audio_activity(captured_samples, sample_rate_hz, 700);
    if should_skip_streaming_preview(&activity) {
        tracing::debug!(
            samples = captured_samples.len(),
            audio_duration_ms,
            peak_abs = format_args!("{:.6}", activity.peak_abs),
            rms = format_args!("{:.6}", activity.rms),
            active_ratio = format_args!("{:.4}", activity.active_ratio),
            "skip streaming preview because audio still looks like background noise"
        );
        return Ok(());
    }

    let result_started_at = Instant::now();
    let Some(result) = recognizer.get_result(stream) else {
        return Ok(());
    };
    let result_elapsed_ms = result_started_at.elapsed().as_millis();
    let text = result.text.trim().to_string();
    if text.is_empty() {
        tracing::info!(
            samples = captured_samples.len(),
            audio_duration_ms,
            result_elapsed_ms,
            "streaming preview produced empty text"
        );
        return Ok(());
    }

    if should_drop_streaming_preview_result(&text, &activity) {
        tracing::info!(
            samples = captured_samples.len(),
            audio_duration_ms,
            result_elapsed_ms,
            raw_text = %text,
            peak_abs = format_args!("{:.6}", activity.peak_abs),
            rms = format_args!("{:.6}", activity.rms),
            active_ratio = format_args!("{:.4}", activity.active_ratio),
            "drop low-signal streaming preview text"
        );
        return Ok(());
    }

    let prepared_text = build_streaming_prepared_preview(&text, rewrite_enabled);
    if *last_partial == text && *last_prepared_preview == prepared_text {
        return Ok(());
    }
    tracing::info!(
        samples = captured_samples.len(),
        audio_duration_ms,
        result_elapsed_ms,
        result_is_final = result.is_final,
        raw_text = %text,
        prepared_text = %prepared_text,
        "streaming partial updated"
    );
    *last_partial = text.clone();
    if *last_prepared_preview != prepared_text {
        *last_prepared_preview = prepared_text.clone();
    }
    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
        raw_text: text,
        prepared_text,
    }));
    Ok(())
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

fn analyze_recent_audio_activity(
    samples: &[f32],
    sample_rate_hz: i32,
    tail_window_ms: u64,
) -> AudioActivity {
    if samples.is_empty() {
        return analyze_audio_activity(samples);
    }

    let tail_samples = ((sample_rate_hz.max(1) as usize) * (tail_window_ms as usize) / 1000).max(1);
    let start = samples.len().saturating_sub(tail_samples);
    analyze_audio_activity(&samples[start..])
}

fn should_skip_as_silence(activity: &AudioActivity) -> bool {
    activity.peak_abs < 0.006 || (activity.rms < 0.0015 && activity.active_ratio < 0.01)
}

fn should_skip_streaming_preview(activity: &AudioActivity) -> bool {
    activity.peak_abs < 0.0075 || (activity.rms < 0.0020 && activity.active_ratio < 0.015)
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

fn should_drop_streaming_preview_result(text: &str, activity: &AudioActivity) -> bool {
    let stripped = text
        .trim()
        .trim_matches(|ch: char| ch.is_whitespace() || is_sentence_punctuation(ch));
    if stripped.is_empty() {
        return true;
    }

    let char_count = stripped.chars().count();
    if activity.rms < 0.0035 && activity.active_ratio < 0.03 && char_count <= 4 {
        return true;
    }

    if !contains_meaningful_preview_char(stripped) {
        return true;
    }

    false
}

fn contains_meaningful_preview_char(text: &str) -> bool {
    text.chars()
        .any(|ch| ch.is_ascii_alphanumeric() || is_cjk_char(ch))
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
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

fn audio_duration_ms(sample_rate_hz: i32, samples_len: usize) -> u64 {
    if sample_rate_hz <= 0 {
        return 0;
    }

    ((samples_len as f64 / sample_rate_hz as f64) * 1000.0).round() as u64
}

fn streaming_preview_interval(runtime: &AppRuntime) -> Duration {
    Duration::from_millis((runtime.config.voice.streaming.chunk_ms as u64).clamp(160, 1200))
}

fn streaming_preview_min_samples(sample_rate_hz: i32, chunk_ms: u64) -> usize {
    let effective_sample_rate = sample_rate_hz.max(1) as usize;
    let effective_chunk_ms = chunk_ms.clamp(160, 1200) as usize;
    ((effective_sample_rate * effective_chunk_ms) / 1000).max(effective_sample_rate / 5)
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

fn build_streaming_prepared_preview(current_partial: &str, rewrite_enabled: bool) -> String {
    if rewrite_enabled {
        let rewritten_segments = ainput_rewrite::rewrite_streaming_text(current_partial);
        if rewritten_segments.is_empty() {
            ainput_rewrite::normalize_streaming_preview(current_partial)
        } else {
            rewritten_segments.join("")
        }
    } else {
        ainput_rewrite::normalize_streaming_preview(current_partial)
    }
}

fn build_streaming_output_text(runtime: &AppRuntime, final_text: &str) -> String {
    if runtime.config.voice.streaming.rewrite_enabled {
        ainput_rewrite::rewrite_streaming_text(final_text).join("")
    } else {
        ainput_rewrite::normalize_transcription(final_text)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioActivity, analyze_recent_audio_activity, build_streaming_prepared_preview,
        should_drop_streaming_preview_result, should_skip_streaming_preview,
    };

    #[test]
    fn streaming_preview_keeps_full_partial_with_rewrite() {
        assert_eq!(
            build_streaming_prepared_preview("帮我看一下这个功能有没有问题", true),
            "帮我看一下这个功能有没有问题。"
        );
    }

    #[test]
    fn streaming_preview_can_skip_rewrite() {
        assert_eq!(
            build_streaming_prepared_preview("嗯， 帮我看一下 这个功能", false),
            "帮我看一下 这个功能"
        );
    }

    #[test]
    fn streaming_preview_skips_background_noise_before_real_speech() {
        let activity = AudioActivity {
            peak_abs: 0.004,
            rms: 0.0012,
            active_ratio: 0.004,
        };
        assert!(should_skip_streaming_preview(&activity));
        assert!(should_drop_streaming_preview_result("喂喂", &activity));
    }

    #[test]
    fn streaming_preview_keeps_real_sentence_once_signal_is_clear() {
        let activity = AudioActivity {
            peak_abs: 0.036,
            rms: 0.008,
            active_ratio: 0.12,
        };
        assert!(!should_skip_streaming_preview(&activity));
        assert!(!should_drop_streaming_preview_result(
            "帮我看一下这里有没有问题",
            &activity
        ));
    }

    #[test]
    fn recent_audio_activity_prefers_latest_speech_over_old_silence() {
        let mut samples = vec![0.0f32; 16_000];
        samples.extend(std::iter::repeat_n(0.05f32, 4_000));
        let activity = analyze_recent_audio_activity(&samples, 16_000, 700);
        assert!(activity.peak_abs >= 0.05);
        assert!(activity.active_ratio > 0.1);
    }
}
