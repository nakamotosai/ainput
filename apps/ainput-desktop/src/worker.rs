use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
};

use ainput_output::{OutputConfig, OutputDelivery};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use winit::event_loop::EventLoopProxy;

use crate::ai_rewrite::AiRewriteRequest;
use crate::streaming_fixtures::{
    StreamingCaseStatus, StreamingFixtureCase, StreamingFixtureManifest,
    StreamingReplayPartialEntry, StreamingReplayReport, StreamingSelftestReport,
};
use crate::streaming_state::{
    StreamingState, can_append_segment_only_candidate, longest_common_prefix_chars,
    split_frozen_prefix, visible_text_char_count,
};
use crate::{AppEvent, AppRuntime, hotkey};

const VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);
const STREAMING_PASTE_STABILIZE_DELAY: Duration = Duration::from_millis(15);
const STREAMING_TAIL_PADDING_MS: u64 = 300;
const STREAMING_RELEASE_GRACE_MS: u64 = 120;
const STREAMING_RELEASE_MIN_WAIT_MS: u64 = 24;
const STREAMING_RELEASE_IDLE_SETTLE_MS: u64 = 32;
const STREAMING_RELEASE_POLL_INTERVAL_MS: u64 = 8;
const STREAMING_FINAL_AI_REWRITE_WAIT_MS: u64 = 320;
const STREAMING_PREVIEW_SHORTFALL_TOLERANCE_CHARS: usize = 3;
const STREAMING_FINAL_SHORTFALL_TOLERANCE_CHARS: usize = 3;
const STREAMING_PREROLL_MS: u64 = 180;
const STREAMING_ENDPOINT_TRAILING_SILENCE_SECS: f32 = 10.0;
const STREAMING_ENDPOINT_MAX_UTTERANCE_SECS: f32 = 20.0;
const STREAMING_FIRST_PARTIAL_TARGET_MS: u128 = 300;
const STREAMING_FIRST_PARTIAL_HARD_MS: u128 = 450;
const STREAMING_RELEASE_TO_COMMIT_TARGET_MS: u128 = 220;
const STREAMING_RELEASE_TO_COMMIT_HARD_MS: u128 = 450;

pub(crate) enum WorkerEvent {
    Ready(WorkerKind),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerKind {
    Fast,
    Streaming,
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
    core: StreamingCoreSession,
    sample_cursor: usize,
}

struct StreamingCoreSession {
    input_sample_rate_hz: i32,
    sample_rate_hz: i32,
    stream: ainput_asr::StreamingZipformerStream,
    pending_feed_samples: Vec<f32>,
    captured_samples: Vec<f32>,
    ingested_input_samples: usize,
    resampler: StreamingResampler,
    state: StreamingState,
    rolled_over_prefix: String,
    awaiting_post_rollover_speech: bool,
    last_fast_preview_text: String,
    last_raw_partial: String,
    last_display_text: String,
    ai_rewrite_result_rx: Option<mpsc::Receiver<StreamingAiRewriteOutcome>>,
    ai_rewrite_inflight_input: String,
    last_ai_rewrite_input: String,
    last_ai_rewrite_output: String,
    last_ai_rewrite_at: Option<Instant>,
    total_decode_steps: usize,
    total_chunks_fed: usize,
    partial_updates: usize,
    first_partial_at: Option<Instant>,
    started_at: Instant,
}

#[derive(Debug, Clone, Copy)]
struct StreamingTextChoice<'a> {
    source: &'static str,
    text: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StreamingCommitSource {
    StreamingState,
    StreamingTailRepair,
    OnlineFinal,
}

impl StreamingCommitSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::StreamingState => "streaming_state",
            Self::StreamingTailRepair => "streaming_tail_repair",
            Self::OnlineFinal => "online_final",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct StreamingCommitChoice<'a> {
    source: StreamingCommitSource,
    text: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StreamingLiveProbeReport {
    seconds_requested: u64,
    input_sample_rate_hz: i32,
    sample_rate_hz: i32,
    captured_samples: usize,
    audio_duration_ms: u64,
    peak_abs: f32,
    rms: f32,
    active_ratio: f32,
    total_chunks_fed: usize,
    total_decode_steps: usize,
    partial_updates: usize,
    last_partial_text: String,
    final_online_raw_text: String,
    final_prepared_candidate: String,
    final_text: String,
    commit_source: StreamingCommitSource,
}

impl StreamingSession {
    fn new(
        input_sample_rate_hz: i32,
        stream: ainput_asr::StreamingZipformerStream,
        sample_rate_hz: i32,
        sample_cursor: usize,
    ) -> Self {
        Self {
            core: StreamingCoreSession::new(input_sample_rate_hz, stream, sample_rate_hz),
            sample_cursor,
        }
    }
}

impl Deref for StreamingSession {
    type Target = StreamingCoreSession;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl DerefMut for StreamingSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

impl StreamingCoreSession {
    fn new(
        input_sample_rate_hz: i32,
        stream: ainput_asr::StreamingZipformerStream,
        sample_rate_hz: i32,
    ) -> Self {
        Self {
            input_sample_rate_hz,
            sample_rate_hz,
            stream,
            pending_feed_samples: Vec::new(),
            captured_samples: Vec::new(),
            ingested_input_samples: 0,
            resampler: StreamingResampler::new(input_sample_rate_hz, sample_rate_hz),
            state: StreamingState::default(),
            rolled_over_prefix: String::new(),
            awaiting_post_rollover_speech: false,
            last_fast_preview_text: String::new(),
            last_raw_partial: String::new(),
            last_display_text: String::new(),
            ai_rewrite_result_rx: None,
            ai_rewrite_inflight_input: String::new(),
            last_ai_rewrite_input: String::new(),
            last_ai_rewrite_output: String::new(),
            last_ai_rewrite_at: None,
            total_decode_steps: 0,
            total_chunks_fed: 0,
            partial_updates: 0,
            first_partial_at: None,
            started_at: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
struct StreamingResampler {
    passthrough: bool,
    step: f64,
    cursor: f64,
    buffer: Vec<f32>,
}

impl StreamingResampler {
    fn new(input_sample_rate_hz: i32, output_sample_rate_hz: i32) -> Self {
        let input = input_sample_rate_hz.max(1) as f64;
        let output = output_sample_rate_hz.max(1) as f64;
        let passthrough = (input - output).abs() < f64::EPSILON;

        Self {
            passthrough,
            step: input / output,
            cursor: 0.0,
            buffer: Vec::new(),
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        if self.passthrough {
            return input.to_vec();
        }

        self.buffer.extend_from_slice(input);
        let mut output = Vec::new();

        while self.cursor + 1.0 < self.buffer.len() as f64 {
            output.push(interpolate_sample(&self.buffer, self.cursor));
            self.cursor += self.step;
        }

        let drop_count = (self.cursor.floor() as usize).min(self.buffer.len());
        if drop_count > 0 {
            self.buffer.drain(..drop_count);
            self.cursor -= drop_count as f64;
        }

        output
    }

    fn flush(&mut self) -> Vec<f32> {
        if self.passthrough {
            return Vec::new();
        }
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let mut output = Vec::new();
        while self.cursor < self.buffer.len() as f64 {
            output.push(interpolate_sample(&self.buffer, self.cursor));
            self.cursor += self.step;
        }

        self.buffer.clear();
        self.cursor = 0.0;
        output
    }
}

fn interpolate_sample(samples: &[f32], cursor: f64) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let left_index = cursor.floor() as usize;
    let right_index = (left_index + 1).min(samples.len().saturating_sub(1));
    let fraction = (cursor - left_index as f64) as f32;
    let left = samples[left_index];
    let right = samples[right_index];
    left + (right - left) * fraction
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

    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Ready(WorkerKind::Fast)));
    let mut active_recording: Option<ainput_audio::ActiveRecording> = None;

    tracing::info!(
        shortcut = %runtime.config.hotkeys.voice_input,
        root_dir = %runtime.runtime_paths.root_dir.display(),
        "ainput worker loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        if let Ok(command) = worker_rx.recv_timeout(Duration::from_millis(8)) {
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
    let punctuator = build_streaming_punctuator(&runtime).map_or_else(
        |error| {
            tracing::warn!(
                error = %error,
                "streaming punctuation unavailable; falling back to unpunctuated text"
            );
            None
        },
        Some,
    );
    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Ready(WorkerKind::Streaming)));

    let mut standby_recording: Option<ainput_audio::ActiveRecording> = None;
    let mut active_session: Option<StreamingSession> = None;
    tracing::info!(
        shortcut = %runtime.config.hotkeys.voice_input,
        model_dir = %runtime.runtime_paths.root_dir.join(&runtime.config.voice.streaming.model_dir).display(),
        chunk_ms = runtime.config.voice.streaming.chunk_ms,
        "ainput streaming worker loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        if let Ok(command) = worker_rx.recv_timeout(Duration::from_millis(8)) {
            match command {
                WorkerCommand::HotkeyPressed => {
                    if active_session.is_none() {
                        if let Err(error) = ensure_streaming_recording_ready(&mut standby_recording)
                        {
                            tracing::error!(
                                error = %error,
                                "failed to start streaming microphone recording"
                            );
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                format!("启动流式录音失败：{error}"),
                            )));
                            continue;
                        }

                        if let Some(recording) = standby_recording.as_ref() {
                            let stream = recognizer.create_stream();
                            let current_cursor = recording.sample_count();
                            let preroll_samples = sample_count_for_ms(
                                recording.sample_rate_hz(),
                                STREAMING_PREROLL_MS,
                            );
                            let start_cursor = current_cursor.saturating_sub(preroll_samples);
                            active_session = Some(StreamingSession::new(
                                recording.sample_rate_hz(),
                                stream,
                                runtime.config.asr.sample_rate_hz as i32,
                                start_cursor,
                            ));
                            let _ =
                                proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingStarted));
                            tracing::info!(
                                sample_rate_hz = recording.sample_rate_hz(),
                                current_cursor,
                                start_cursor,
                                preroll_ms = STREAMING_PREROLL_MS,
                                "streaming push-to-talk recording started"
                            );
                        }
                    }
                }
                WorkerCommand::HotkeyReleased => {
                    if let Some(mut session) = active_session.take() {
                        let Some(recording) = standby_recording.take() else {
                            tracing::error!(
                                "streaming recording disappeared before hotkey release handling"
                            );
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                "流式录音状态异常，请重试".to_string(),
                            )));
                            continue;
                        };
                        let release_started_at = Instant::now();
                        let release_drain =
                            match finish_streaming_recording(&mut session, recording) {
                                Ok(stats) => stats,
                                Err(error) => {
                                    tracing::error!(
                                        error = %error,
                                        "failed to finalize streaming recording on hotkey release"
                                    );
                                    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                        format!("结束流式录音失败：{error}"),
                                    )));
                                    continue;
                                }
                            };
                        let sample_rate_hz = session.sample_rate_hz;
                        let chunk_samples = streaming_chunk_num_samples(
                            sample_rate_hz,
                            runtime.config.voice.streaming.chunk_ms,
                        );
                        let streamed_samples = feed_streaming_pending_chunks(
                            &recognizer,
                            &mut session,
                            sample_rate_hz,
                            chunk_samples,
                            false,
                        );
                        let captured_audio_duration_ms =
                            audio_duration_ms(sample_rate_hz, session.captured_samples.len());
                        let activity = analyze_audio_activity(&session.captured_samples);
                        tracing::info!(
                            sample_rate_hz,
                            input_sample_rate_hz = session.input_sample_rate_hz,
                            frames = session.captured_samples.len(),
                            audio_duration_ms = captured_audio_duration_ms,
                            peak_abs = format_args!("{:.6}", activity.peak_abs),
                            rms = format_args!("{:.6}", activity.rms),
                            active_ratio = format_args!("{:.4}", activity.active_ratio),
                            release_grace_added_samples = release_drain.grace_added_samples,
                            release_stop_added_samples = release_drain.stop_added_samples,
                            release_grace_wait_elapsed_ms = release_drain.grace_wait_elapsed_ms,
                            streamed_samples,
                            total_chunks_fed = session.total_chunks_fed,
                            "streaming push-to-talk recording captured"
                        );

                        if session.captured_samples.is_empty() || should_skip_as_silence(&activity)
                        {
                            tracing::info!(
                                audio_duration_ms = captured_audio_duration_ms,
                                peak_abs = format_args!("{:.6}", activity.peak_abs),
                                rms = format_args!("{:.6}", activity.rms),
                                active_ratio = format_args!("{:.4}", activity.active_ratio),
                                "skip streaming transcription because captured audio looks like silence"
                            );
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                            continue;
                        }

                        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingFlushing));
                        if let Err(error) =
                            drain_final_streaming_ai_rewrite(&runtime, &mut session, &proxy)
                        {
                            tracing::warn!(
                                error = %error,
                                "streaming final AI rewrite drain failed; continue with final ASR result"
                            );
                        }
                        let prepared_commit = prepare_final_streaming_commit(
                            &recognizer,
                            punctuator.as_ref(),
                            &mut session,
                            sample_rate_hz,
                            chunk_samples,
                            runtime.config.voice.streaming.rewrite_enabled,
                        );

                        if prepared_commit.final_text.is_empty()
                            || should_drop_low_signal_result(&prepared_commit.final_text, &activity)
                        {
                            tracing::info!(
                                final_online_raw_text = %prepared_commit.final_online_raw_text,
                                prepared_final_candidate = %prepared_commit.prepared_final_candidate,
                                display_text_before_final = %prepared_commit.display_text_before_final,
                                candidate_display_text = %prepared_commit.candidate_display_text,
                                selected_commit_source = prepared_commit.commit_source.as_str(),
                                final_decode_elapsed_ms = prepared_commit.final_decode_elapsed_ms,
                                final_decode_steps = prepared_commit.final_decode_steps,
                                rejected_prefix_rewrite = prepared_commit.rejected_prefix_rewrite,
                                peak_abs = format_args!("{:.6}", activity.peak_abs),
                                rms = format_args!("{:.6}", activity.rms),
                                active_ratio = format_args!("{:.4}", activity.active_ratio),
                                "drop empty or low-signal streaming final text"
                            );
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                            continue;
                        }

                        if prepared_commit.final_text != prepared_commit.display_text_before_final {
                            emit_streaming_final_preview_sync(
                                &mut session,
                                &prepared_commit.final_text,
                                &proxy,
                            );
                        }

                        tracing::info!(
                            samples = session.captured_samples.len(),
                            audio_duration_ms = captured_audio_duration_ms,
                            final_online_raw_text = %prepared_commit.final_online_raw_text,
                            display_text_before_final = %prepared_commit.display_text_before_final,
                            prepared_final_candidate = %prepared_commit.prepared_final_candidate,
                            candidate_display_text = %prepared_commit.candidate_display_text,
                            selected_commit_source = prepared_commit.commit_source.as_str(),
                            commit_text = %prepared_commit.final_text,
                            final_decode_steps = prepared_commit.final_decode_steps,
                            final_decode_elapsed_ms = prepared_commit.final_decode_elapsed_ms,
                            rejected_prefix_rewrite = prepared_commit.rejected_prefix_rewrite,
                            total_decode_steps = session.total_decode_steps,
                            "streaming final transcription ready"
                        );

                        let output_config = OutputConfig {
                            prefer_direct_paste: runtime.config.voice.prefer_direct_paste,
                            fallback_to_clipboard: runtime.config.voice.fallback_to_clipboard,
                            voice_hotkey_uses_alt: hotkey::voice_hotkey_uses_alt(),
                            paste_stabilize_delay: STREAMING_PASTE_STABILIZE_DELAY,
                        };

                        let output_started_at = Instant::now();
                        let delivery = match runtime
                            .output_controller
                            .deliver_text(&prepared_commit.final_text, &output_config)
                        {
                            Ok(delivery) => {
                                if matches!(delivery, OutputDelivery::ClipboardOnly) {
                                    let _ = proxy.send_event(AppEvent::Worker(
                                        WorkerEvent::StreamingClipboardFallback(
                                            prepared_commit.final_text.clone(),
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
                        let release_to_commit_elapsed_ms = release_started_at.elapsed().as_millis();
                        let first_partial_elapsed_ms = session
                            .first_partial_at
                            .map(|instant| instant.duration_since(session.started_at).as_millis());

                        runtime
                            .shared_state
                            .set_last_voice_text(prepared_commit.final_text.clone());
                        runtime.maintenance.persist_voice_result(VoiceHistoryEntry {
                            timestamp: current_timestamp(),
                            delivery_label: streaming_delivery_label(delivery),
                            text: prepared_commit.final_text.clone(),
                        });
                        let pipeline_elapsed_ms = session.started_at.elapsed().as_millis();
                        let realtime_factor = if captured_audio_duration_ms > 0 {
                            pipeline_elapsed_ms as f64 / captured_audio_duration_ms as f64
                        } else {
                            0.0
                        };
                        tracing::info!(
                            ?delivery,
                            text = %prepared_commit.final_text,
                            audio_duration_ms = captured_audio_duration_ms,
                            first_partial_elapsed_ms,
                            release_tail_elapsed_ms = release_drain.grace_wait_elapsed_ms,
                            final_decode_elapsed_ms = prepared_commit.final_decode_elapsed_ms,
                            final_decode_steps = prepared_commit.final_decode_steps,
                            total_decode_steps = session.total_decode_steps,
                            output_elapsed_ms,
                            release_to_commit_elapsed_ms,
                            pipeline_elapsed_ms,
                            realtime_factor = format_args!("{realtime_factor:.3}"),
                            "streaming transcription delivered"
                        );
                        log_streaming_timing_gate_results(
                            first_partial_elapsed_ms,
                            release_to_commit_elapsed_ms,
                        );

                        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingFinal(
                            prepared_commit.final_text,
                        )));
                    }
                }
            }
        }

        if let Some(session) = &mut active_session {
            let Some(recording) = standby_recording.as_ref() else {
                continue;
            };
            let added_samples = collect_streaming_audio_chunk(session, recording);
            let sample_rate_hz = session.sample_rate_hz;
            let chunk_samples = streaming_chunk_num_samples(
                sample_rate_hz,
                runtime.config.voice.streaming.chunk_ms,
            );
            let streamed_samples = feed_streaming_pending_chunks(
                &recognizer,
                session,
                sample_rate_hz,
                chunk_samples,
                true,
            );
            if streamed_samples > 0 || added_samples > 0 {
                let audio_duration_ms =
                    audio_duration_ms(sample_rate_hz, session.captured_samples.len());
                if let Err(error) = emit_streaming_partial_if_changed(
                    &runtime,
                    &recognizer,
                    punctuator.as_ref(),
                    session,
                    runtime.config.voice.streaming.rewrite_enabled,
                    &proxy,
                    audio_duration_ms,
                ) {
                    tracing::warn!(
                        error = %error,
                        samples = session.captured_samples.len(),
                        streamed_samples,
                        "streaming live preview decode failed"
                    );
                }
            }

            let level = normalize_audio_level(recording.current_level());
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Meter(level)));
        }
    }
}

pub(crate) fn probe_streaming_live_session(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    seconds: u64,
) -> Result<StreamingLiveProbeReport> {
    let punctuator = build_streaming_punctuator(runtime).ok();
    let recording = ainput_audio::ActiveRecording::start_default_input()?;
    let mut session = StreamingSession::new(
        recording.sample_rate_hz(),
        recognizer.create_stream(),
        runtime.config.asr.sample_rate_hz as i32,
        0,
    );
    let sample_rate_hz = session.sample_rate_hz;
    let chunk_samples =
        streaming_chunk_num_samples(sample_rate_hz, runtime.config.voice.streaming.chunk_ms);
    let deadline = Instant::now() + Duration::from_secs(seconds.max(1));

    while Instant::now() < deadline {
        let added_samples = collect_streaming_audio_chunk(&mut session, &recording);
        let streamed_samples = feed_streaming_pending_chunks(
            recognizer,
            &mut session,
            sample_rate_hz,
            chunk_samples,
            true,
        );

        if streamed_samples > 0 || added_samples > 0 {
            let total_audio_duration_ms =
                audio_duration_ms(sample_rate_hz, session.captured_samples.len());
            update_streaming_partial_state(
                runtime,
                recognizer,
                punctuator.as_ref(),
                &mut session,
                runtime.config.voice.streaming.rewrite_enabled,
                total_audio_duration_ms,
            )?;
            let _ = maybe_rollover_streaming_segment_core(
                recognizer,
                punctuator.as_ref(),
                &mut session,
                runtime.config.voice.streaming.rewrite_enabled,
                total_audio_duration_ms,
            );
        }

        std::thread::sleep(Duration::from_millis(8));
    }

    let _ = collect_streaming_audio_chunk(&mut session, &recording);
    let _ = feed_streaming_pending_chunks(
        recognizer,
        &mut session,
        sample_rate_hz,
        chunk_samples,
        false,
    );

    let activity = analyze_audio_activity(&session.captured_samples);
    let prepared_commit = prepare_final_streaming_commit(
        recognizer,
        punctuator.as_ref(),
        &mut session,
        sample_rate_hz,
        chunk_samples,
        runtime.config.voice.streaming.rewrite_enabled,
    );
    let final_text = prepared_commit.final_text.clone();

    Ok(StreamingLiveProbeReport {
        seconds_requested: seconds.max(1),
        input_sample_rate_hz: session.input_sample_rate_hz,
        sample_rate_hz,
        captured_samples: session.captured_samples.len(),
        audio_duration_ms: audio_duration_ms(sample_rate_hz, session.captured_samples.len()),
        peak_abs: activity.peak_abs,
        rms: activity.rms,
        active_ratio: activity.active_ratio,
        total_chunks_fed: session.total_chunks_fed,
        total_decode_steps: session.total_decode_steps,
        partial_updates: session.partial_updates,
        last_partial_text: session.last_display_text.clone(),
        final_online_raw_text: prepared_commit.final_online_raw_text,
        final_prepared_candidate: prepared_commit.prepared_final_candidate,
        final_text,
        commit_source: prepared_commit.commit_source,
    })
}

pub(crate) fn replay_streaming_wav(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    case_id: &str,
    wav_path: &Path,
    expected_text: Option<&str>,
    min_partial_updates: usize,
    min_visible_chars: Option<usize>,
    shortfall_tolerance_chars: usize,
) -> Result<StreamingReplayReport> {
    let punctuator = build_streaming_punctuator(runtime).ok();
    let (input_sample_rate_hz, input_samples) = read_wav_samples(wav_path)?;
    let mut session = StreamingCoreSession::new(
        input_sample_rate_hz,
        recognizer.create_stream(),
        runtime.config.asr.sample_rate_hz as i32,
    );
    let replay_chunk_num_samples = streaming_chunk_num_samples(
        input_sample_rate_hz,
        runtime.config.voice.streaming.chunk_ms,
    );
    let stream_chunk_num_samples = streaming_chunk_num_samples(
        session.sample_rate_hz,
        runtime.config.voice.streaming.chunk_ms,
    );
    let runner_sample_rate_hz = session.sample_rate_hz;
    let mut partial_timeline = Vec::new();

    for chunk in input_samples.chunks(replay_chunk_num_samples.max(1)) {
        let added_samples = push_streaming_input_samples(&mut session, chunk);
        let streamed_samples = feed_streaming_pending_chunks(
            recognizer,
            &mut session,
            runner_sample_rate_hz,
            stream_chunk_num_samples,
            true,
        );

        if streamed_samples == 0 && added_samples == 0 {
            continue;
        }

        let total_audio_duration_ms =
            audio_duration_ms(session.sample_rate_hz, session.captured_samples.len());
        if let Some(update) = update_streaming_partial_state(
            runtime,
            recognizer,
            punctuator.as_ref(),
            &mut session,
            runtime.config.voice.streaming.rewrite_enabled,
            total_audio_duration_ms,
        )? {
            partial_timeline.push(StreamingReplayPartialEntry {
                offset_ms: total_audio_duration_ms,
                raw_text: update.raw_text,
                prepared_text: update.display_text,
                source: update.source.to_string(),
                stable_chars: update.stable_chars,
                frozen_chars: update.frozen_chars,
                volatile_chars: update.volatile_chars,
                rejected_prefix_rewrite: update.rejected_prefix_rewrite,
            });
        }
        if let Some(committed_text) = maybe_rollover_streaming_segment_core(
            recognizer,
            punctuator.as_ref(),
            &mut session,
            runtime.config.voice.streaming.rewrite_enabled,
            total_audio_duration_ms,
        ) {
            partial_timeline.push(StreamingReplayPartialEntry {
                offset_ms: total_audio_duration_ms,
                raw_text: committed_text.clone(),
                prepared_text: committed_text,
                source: "endpoint_rollover".to_string(),
                stable_chars: session.last_display_text.chars().count(),
                frozen_chars: session.state.frozen_prefix.chars().count(),
                volatile_chars: session.state.volatile_sentence.chars().count(),
                rejected_prefix_rewrite: false,
            });
        }
    }

    let _ = feed_streaming_pending_chunks(
        recognizer,
        &mut session,
        runner_sample_rate_hz,
        stream_chunk_num_samples,
        false,
    );

    let activity = analyze_audio_activity(&session.captured_samples);
    let prepared_commit = prepare_final_streaming_commit(
        recognizer,
        punctuator.as_ref(),
        &mut session,
        runner_sample_rate_hz,
        stream_chunk_num_samples,
        runtime.config.voice.streaming.rewrite_enabled,
    );
    let final_text = prepared_commit.final_text.clone();
    let final_visible_chars = visible_text_char_count(&final_text);
    let expected_text = expected_text.map(str::to_string);
    let expected_visible_chars = min_visible_chars.or_else(|| {
        expected_text
            .as_deref()
            .map(visible_text_char_count)
            .filter(|count| *count > 0)
    });
    let mut failures = Vec::new();

    if session.partial_updates < min_partial_updates {
        failures.push(format!(
            "partial_updates={} < min_partial_updates={}",
            session.partial_updates, min_partial_updates
        ));
    }

    if let Some(expected_visible_chars) = expected_visible_chars
        && final_visible_chars + shortfall_tolerance_chars < expected_visible_chars
    {
        failures.push(format!(
            "final_visible_chars={} shorter than expected_visible_chars={} with tolerance={}",
            final_visible_chars, expected_visible_chars, shortfall_tolerance_chars
        ));
    }

    if let Some(last_partial) = partial_timeline.last() {
        let partial_visible_chars = visible_text_char_count(&last_partial.prepared_text);
        if final_visible_chars + shortfall_tolerance_chars < partial_visible_chars {
            failures.push(format!(
                "final_visible_chars={} shorter than last_partial_visible_chars={} with tolerance={}",
                final_visible_chars, partial_visible_chars, shortfall_tolerance_chars
            ));
        }
    }

    let behavior_status = if failures.is_empty() {
        StreamingCaseStatus::Pass
    } else {
        StreamingCaseStatus::FailBehavior
    };

    let mut content_failures = Vec::new();
    if let Some(expected_text) = expected_text.as_deref()
        && normalize_replay_text(&final_text) != normalize_replay_text(expected_text)
    {
        content_failures.push(format!(
            "final_text_mismatch expected='{}' actual='{}'",
            expected_text, final_text
        ));
    }
    let content_status = if content_failures.is_empty() {
        StreamingCaseStatus::Pass
    } else {
        StreamingCaseStatus::FailContent
    };
    failures.extend(content_failures);

    Ok(StreamingReplayReport {
        case_id: case_id.to_string(),
        input_wav: wav_path.display().to_string(),
        input_sample_rate_hz,
        runner_sample_rate_hz,
        input_duration_ms: audio_duration_ms(input_sample_rate_hz, session.ingested_input_samples),
        captured_samples: session.captured_samples.len(),
        peak_abs: activity.peak_abs,
        rms: activity.rms,
        active_ratio: activity.active_ratio,
        total_chunks_fed: session.total_chunks_fed,
        total_decode_steps: session.total_decode_steps,
        partial_updates: session.partial_updates,
        first_partial_ms: partial_timeline.first().map(|entry| entry.offset_ms),
        final_commit_ms: audio_duration_ms(input_sample_rate_hz, session.ingested_input_samples),
        partial_timeline,
        last_partial_text: session.last_display_text.clone(),
        final_online_raw_text: prepared_commit.final_online_raw_text,
        final_prepared_candidate: prepared_commit.prepared_final_candidate,
        final_text,
        final_visible_chars,
        commit_source: prepared_commit.commit_source.as_str().to_string(),
        expected_text,
        expected_visible_chars,
        min_partial_updates,
        shortfall_tolerance_chars,
        behavior_status,
        content_status,
        failures,
    })
}

pub(crate) fn replay_streaming_manifest(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    manifest_path: &Path,
    manifest: &StreamingFixtureManifest,
) -> Result<StreamingSelftestReport> {
    let manifest_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let fixture_root = manifest
        .fixture_root
        .as_ref()
        .map(|root| manifest_dir.join(root));
    let mut cases = Vec::new();

    for case in &manifest.cases {
        let wav_path = resolve_fixture_case_path(&manifest_dir, fixture_root.as_deref(), case);
        let report = replay_streaming_wav(
            runtime,
            recognizer,
            &case.id,
            &wav_path,
            case.expected_text.as_deref(),
            case.min_partial_updates.unwrap_or(1),
            case.min_visible_chars,
            case.shortfall_tolerance_chars.unwrap_or(3),
        )?;
        cases.push(report);
    }

    let behavior_failures = cases
        .iter()
        .filter(|report| report.behavior_status == StreamingCaseStatus::FailBehavior)
        .count();
    let content_failures = cases
        .iter()
        .filter(|report| report.content_status == StreamingCaseStatus::FailContent)
        .count();
    let passed_cases = cases
        .iter()
        .filter(|report| {
            report.behavior_status == StreamingCaseStatus::Pass
                && report.content_status == StreamingCaseStatus::Pass
        })
        .count();
    let overall_status = if behavior_failures > 0 {
        StreamingCaseStatus::FailBehavior
    } else if content_failures > 0 {
        StreamingCaseStatus::FailContent
    } else {
        StreamingCaseStatus::Pass
    };

    Ok(StreamingSelftestReport {
        manifest_path: manifest_path.display().to_string(),
        total_cases: cases.len(),
        passed_cases,
        behavior_failures,
        content_failures,
        overall_status,
        cases,
    })
}

fn resolve_fixture_case_path(
    manifest_dir: &Path,
    fixture_root: Option<&Path>,
    case: &StreamingFixtureCase,
) -> std::path::PathBuf {
    let wav_path = Path::new(&case.wav_path);
    if wav_path.is_absolute() {
        return wav_path.to_path_buf();
    }
    if let Some(fixture_root) = fixture_root {
        return fixture_root.join(wav_path);
    }
    manifest_dir.join(wav_path)
}

fn normalize_replay_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_sentence_punctuation(*ch))
        .collect()
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
        enable_endpoint: true,
        rule1_min_trailing_silence: STREAMING_ENDPOINT_TRAILING_SILENCE_SECS,
        rule2_min_trailing_silence: STREAMING_ENDPOINT_TRAILING_SILENCE_SECS,
        rule3_min_utterance_length: STREAMING_ENDPOINT_MAX_UTTERANCE_SECS,
    })
}

fn build_streaming_punctuator(
    runtime: &AppRuntime,
) -> Result<ainput_asr::OfflinePunctuationRestorer> {
    ainput_asr::OfflinePunctuationRestorer::create(&ainput_asr::OfflinePunctuationConfigBundle {
        model_dir: runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.voice.streaming.punctuation_model_dir),
        provider: runtime.config.asr.provider.clone(),
        num_threads: runtime.config.voice.streaming.punctuation_num_threads,
    })
}

fn ensure_streaming_recording_ready(
    standby_recording: &mut Option<ainput_audio::ActiveRecording>,
) -> Result<()> {
    if standby_recording.is_none() {
        let recording = ainput_audio::ActiveRecording::start_default_input()?;
        tracing::info!(
            sample_rate_hz = recording.sample_rate_hz(),
            "streaming microphone armed on hotkey press"
        );
        *standby_recording = Some(recording);
    }

    Ok(())
}

fn collect_streaming_audio_chunk(
    session: &mut StreamingSession,
    recording: &ainput_audio::ActiveRecording,
) -> usize {
    let chunk = recording.take_new_samples(&mut session.sample_cursor);
    push_streaming_input_samples(&mut session.core, &chunk)
}

fn sample_count_for_ms(sample_rate_hz: i32, duration_ms: u64) -> usize {
    if sample_rate_hz <= 0 {
        return 0;
    }

    ((sample_rate_hz as usize) * duration_ms as usize) / 1000
}

fn push_streaming_input_samples(session: &mut StreamingCoreSession, input: &[f32]) -> usize {
    if input.is_empty() {
        return 0;
    }

    session.ingested_input_samples += input.len();
    let resampled = session.resampler.process(input);
    if resampled.is_empty() {
        return 0;
    }

    session.pending_feed_samples.extend_from_slice(&resampled);
    session.captured_samples.extend_from_slice(&resampled);
    resampled.len()
}

fn flush_streaming_audio_tail(session: &mut StreamingCoreSession) -> usize {
    let tail = session.resampler.flush();
    if tail.is_empty() {
        return 0;
    }

    session.pending_feed_samples.extend_from_slice(&tail);
    session.captured_samples.extend_from_slice(&tail);
    tail.len()
}

fn collect_stopped_recording_tail(
    session: &mut StreamingSession,
    recorded: &ainput_audio::RecordedAudio,
) -> usize {
    if session.sample_cursor >= recorded.samples.len() {
        return 0;
    }

    let tail = &recorded.samples[session.sample_cursor..];
    session.sample_cursor = recorded.samples.len();
    push_streaming_input_samples(&mut session.core, tail)
}

fn finish_streaming_recording(
    session: &mut StreamingSession,
    recording: ainput_audio::ActiveRecording,
) -> Result<StreamingReleaseDrainStats> {
    let grace_started_at = Instant::now();
    let deadline = grace_started_at + Duration::from_millis(STREAMING_RELEASE_GRACE_MS);
    let min_wait = Duration::from_millis(STREAMING_RELEASE_MIN_WAIT_MS);
    let idle_settle = Duration::from_millis(STREAMING_RELEASE_IDLE_SETTLE_MS);
    let poll_interval = Duration::from_millis(STREAMING_RELEASE_POLL_INTERVAL_MS);

    let mut grace_added_samples = 0usize;
    let mut last_new_audio_at: Option<Instant> = None;
    tracing::info!(
        grace_ms = STREAMING_RELEASE_GRACE_MS,
        min_wait_ms = STREAMING_RELEASE_MIN_WAIT_MS,
        idle_settle_ms = STREAMING_RELEASE_IDLE_SETTLE_MS,
        "streaming release tail drain started"
    );

    loop {
        let added = collect_streaming_audio_chunk(session, &recording);
        if added > 0 {
            grace_added_samples += added;
            last_new_audio_at = Some(Instant::now());
        }

        let now = Instant::now();
        if now >= deadline {
            break;
        }

        let waited = now.saturating_duration_since(grace_started_at);
        if waited >= min_wait
            && last_new_audio_at
                .is_some_and(|instant| now.saturating_duration_since(instant) >= idle_settle)
        {
            break;
        }

        std::thread::sleep(poll_interval);
    }

    let recorded = recording.stop()?;
    let stop_added_samples = collect_stopped_recording_tail(session, &recorded);
    let grace_wait_elapsed_ms = grace_started_at.elapsed().as_millis();
    tracing::info!(
        grace_added_samples,
        stop_added_samples,
        grace_wait_elapsed_ms,
        recorded_total_samples = recorded.samples.len(),
        "streaming release tail drain finished"
    );

    Ok(StreamingReleaseDrainStats {
        grace_added_samples,
        stop_added_samples,
        grace_wait_elapsed_ms,
    })
}

fn streaming_chunk_num_samples(sample_rate_hz: i32, chunk_ms: u32) -> usize {
    let effective_sample_rate = sample_rate_hz.max(1) as usize;
    let effective_chunk_ms = chunk_ms.clamp(60, 500) as usize;
    ((effective_sample_rate * effective_chunk_ms) / 1000).max(effective_sample_rate / 20)
}

fn feed_streaming_pending_chunks(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    session: &mut StreamingCoreSession,
    sample_rate_hz: i32,
    chunk_num_samples: usize,
    full_chunks_only: bool,
) -> usize {
    let mut consumed = 0usize;

    while session.pending_feed_samples.len() >= chunk_num_samples {
        let chunk: Vec<f32> = session
            .pending_feed_samples
            .drain(..chunk_num_samples)
            .collect();
        recognizer.accept_waveform(&session.stream, sample_rate_hz, &chunk);
        session.total_decode_steps += recognizer.decode_available(&session.stream);
        session.total_chunks_fed += 1;
        consumed += chunk.len();
    }

    if !full_chunks_only && !session.pending_feed_samples.is_empty() {
        let chunk: Vec<f32> = session.pending_feed_samples.drain(..).collect();
        recognizer.accept_waveform(&session.stream, sample_rate_hz, &chunk);
        session.total_decode_steps += recognizer.decode_available(&session.stream);
        session.total_chunks_fed += 1;
        consumed += chunk.len();
    }

    consumed
}

fn finalize_streaming_decode(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    session: &mut StreamingCoreSession,
    sample_rate_hz: i32,
    chunk_num_samples: usize,
) -> usize {
    let mut decode_steps = 0usize;
    let _ = flush_streaming_audio_tail(session);
    feed_streaming_pending_chunks(
        recognizer,
        session,
        sample_rate_hz,
        chunk_num_samples,
        false,
    );

    let tail_padding_num_samples =
        ((sample_rate_hz.max(1) as usize) * STREAMING_TAIL_PADDING_MS as usize / 1000).max(1);
    let tail_padding = vec![0.0f32; tail_padding_num_samples];
    recognizer.accept_waveform(&session.stream, sample_rate_hz, &tail_padding);
    decode_steps += recognizer.decode_available(&session.stream);
    recognizer.input_finished(&session.stream);
    decode_steps += recognizer.decode_available(&session.stream);
    session.total_decode_steps += decode_steps;
    decode_steps
}

#[derive(Debug, Clone)]
struct PreparedStreamingCommit {
    final_online_raw_text: String,
    prepared_final_candidate: String,
    display_text_before_final: String,
    candidate_display_text: String,
    final_text: String,
    commit_source: StreamingCommitSource,
    final_decode_steps: usize,
    final_decode_elapsed_ms: u128,
    rejected_prefix_rewrite: bool,
}

#[derive(Debug, Clone, Copy)]
struct StreamingReleaseDrainStats {
    grace_added_samples: usize,
    stop_added_samples: usize,
    grace_wait_elapsed_ms: u128,
}

#[derive(Debug, Clone)]
struct PreparedStreamingPartial {
    raw_text: String,
    online_prepared_text: String,
    display_text: String,
    source: &'static str,
    stable_chars: usize,
    frozen_chars: usize,
    volatile_chars: usize,
    rejected_prefix_rewrite: bool,
}

#[derive(Debug, Clone)]
struct StreamingAiRewriteCandidate {
    frozen_prefix: String,
    current_tail: String,
}

#[derive(Debug)]
struct StreamingAiRewriteOutcome {
    request_key: String,
    candidate: StreamingAiRewriteCandidate,
    rewritten_tail: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct StreamingAiRewriteDisplayUpdate {
    request_key: String,
    display_text: String,
}

fn prepare_final_streaming_commit(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    session: &mut StreamingCoreSession,
    sample_rate_hz: i32,
    chunk_num_samples: usize,
    rewrite_enabled: bool,
) -> PreparedStreamingCommit {
    let final_decode_started_at = Instant::now();
    let final_decode_steps =
        finalize_streaming_decode(recognizer, session, sample_rate_hz, chunk_num_samples);
    let final_decode_elapsed_ms = final_decode_started_at.elapsed().as_millis();
    let final_online_raw_text = recognizer
        .get_result(&session.stream)
        .map(|result| result.text.trim().to_string())
        .unwrap_or_default();
    let (prepared_final_candidate, candidate_source) =
        prepare_streaming_output_text(&final_online_raw_text, rewrite_enabled, punctuator);
    let display_text_before_final = effective_streaming_display_text(session);
    let resolved_commit = resolve_final_streaming_commit(
        session,
        &display_text_before_final,
        &prepared_final_candidate,
        candidate_source,
    );

    PreparedStreamingCommit {
        final_online_raw_text,
        prepared_final_candidate,
        display_text_before_final,
        candidate_display_text: resolved_commit.candidate_display_text,
        final_text: resolved_commit.final_text,
        commit_source: resolved_commit.commit_source,
        final_decode_steps,
        final_decode_elapsed_ms,
        rejected_prefix_rewrite: resolved_commit.rejected_prefix_rewrite,
    }
}

#[derive(Debug, Clone)]
struct ResolvedStreamingCommit {
    candidate_display_text: String,
    final_text: String,
    commit_source: StreamingCommitSource,
    rejected_prefix_rewrite: bool,
}

fn resolve_final_streaming_commit(
    session: &mut StreamingCoreSession,
    display_text_before_final: &str,
    prepared_final_candidate: &str,
    candidate_source: StreamingCommitSource,
) -> ResolvedStreamingCommit {
    let display_trimmed = display_text_before_final.trim().to_string();
    let candidate_trimmed = prepared_final_candidate.trim();

    let (candidate_display_text, rejected_prefix_rewrite) = if candidate_trimmed.is_empty() {
        (String::new(), false)
    } else {
        let mut candidate_state = session.state.clone();
        let candidate_delta = candidate_state.finalize_from_streaming(candidate_trimmed);
        if candidate_delta.rejected_prefix_rewrite {
            (String::new(), true)
        } else {
            (
                merge_rolled_over_prefix(&session.rolled_over_prefix, candidate_state.full_text()),
                false,
            )
        }
    };

    let selected_commit =
        select_streaming_commit_text(&display_trimmed, &candidate_display_text, candidate_source);
    let final_text = selected_commit.text.trim().to_string();
    let commit_source = if final_text == display_trimmed {
        StreamingCommitSource::StreamingState
    } else {
        selected_commit.source
    };
    let _ = session.state.freeze_with_committed_text(&final_text);

    ResolvedStreamingCommit {
        candidate_display_text,
        final_text,
        commit_source,
        rejected_prefix_rewrite,
    }
}

fn update_streaming_partial_state(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    session: &mut StreamingCoreSession,
    rewrite_enabled: bool,
    total_audio_duration_ms: u64,
) -> Result<Option<PreparedStreamingPartial>> {
    let sample_rate_hz = session.sample_rate_hz;
    let activity = analyze_recent_audio_activity(&session.captured_samples, sample_rate_hz, 500);
    if session.awaiting_post_rollover_speech && !should_skip_streaming_preview(&activity) {
        session.awaiting_post_rollover_speech = false;
    }
    if should_skip_streaming_preview(&activity) {
        tracing::debug!(
            samples = session.captured_samples.len(),
            audio_duration_ms = total_audio_duration_ms,
            peak_abs = format_args!("{:.6}", activity.peak_abs),
            rms = format_args!("{:.6}", activity.rms),
            active_ratio = format_args!("{:.4}", activity.active_ratio),
            "skip streaming preview because audio still looks like background noise"
        );
        return Ok(None);
    }

    let online_raw_text = recognizer
        .get_result(&session.stream)
        .map(|result| result.text.trim().to_string())
        .unwrap_or_default();
    let online_prepared_text = if online_raw_text.is_empty() {
        String::new()
    } else if should_drop_streaming_preview_result(&online_raw_text, &activity) {
        tracing::debug!(
            samples = session.captured_samples.len(),
            audio_duration_ms = total_audio_duration_ms,
            raw_text = %online_raw_text,
            peak_abs = format_args!("{:.6}", activity.peak_abs),
            rms = format_args!("{:.6}", activity.rms),
            active_ratio = format_args!("{:.4}", activity.active_ratio),
            "drop low-signal streaming preview text"
        );
        String::new()
    } else {
        prepare_streaming_preview_text(&online_raw_text, rewrite_enabled, punctuator)
    };

    let current_display_text = effective_streaming_display_text(session);
    let selected_preview =
        select_streaming_preview_text(&current_display_text, &online_prepared_text);
    let preview_source = selected_preview.source;
    let mut selected_preview_text = selected_preview.text.to_string();
    if !session.rolled_over_prefix.is_empty()
        && !selected_preview_text.starts_with(&session.rolled_over_prefix)
        && can_append_segment_only_candidate(&selected_preview_text, &session.rolled_over_prefix)
    {
        selected_preview_text = format!("{}{}", session.rolled_over_prefix, selected_preview_text);
    }
    if selected_preview_text.is_empty() {
        tracing::debug!(
            samples = session.captured_samples.len(),
            audio_duration_ms = total_audio_duration_ms,
            raw_text = %online_raw_text,
            "streaming preview produced no usable corrected text"
        );
        return Ok(None);
    }
    if preview_source != "held_display" {
        selected_preview_text =
            maybe_apply_streaming_ai_rewrite(runtime, session, &selected_preview_text)?;
    }

    let selected_raw_text = if preview_source == "held_display" {
        current_display_text.clone()
    } else {
        online_raw_text.clone()
    };
    if session.last_raw_partial == selected_raw_text
        && session.last_display_text == selected_preview_text
    {
        return Ok(None);
    }

    Ok(apply_streaming_partial_update(
        session,
        &selected_raw_text,
        &selected_preview_text,
        &online_prepared_text,
        preview_source,
    ))
}

fn prepare_fast_streaming_partial(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    session: &mut StreamingCoreSession,
    _total_audio_duration_ms: u64,
) -> Option<PreparedStreamingPartial> {
    if !session.last_display_text.is_empty() {
        return None;
    }

    let sample_rate_hz = session.sample_rate_hz;
    let activity = analyze_recent_audio_activity(&session.captured_samples, sample_rate_hz, 500);
    if should_skip_streaming_preview(&activity) {
        return None;
    }

    let online_raw_text = recognizer
        .get_result(&session.stream)
        .map(|result| result.text.trim().to_string())
        .unwrap_or_default();
    if online_raw_text.is_empty()
        || should_drop_streaming_preview_result(&online_raw_text, &activity)
    {
        return None;
    }

    let fast_candidate = ainput_rewrite::normalize_streaming_preview(&online_raw_text);
    if fast_candidate.is_empty() {
        return None;
    }

    let current_display_text = if session.last_fast_preview_text.is_empty() {
        effective_streaming_display_text(session)
    } else {
        session.last_fast_preview_text.clone()
    };
    let selected_preview = select_streaming_preview_text(&current_display_text, &fast_candidate);
    let selected_text = selected_preview.text.trim();
    if selected_text.is_empty() || session.last_fast_preview_text == selected_text {
        return None;
    }

    apply_streaming_partial_update(
        session,
        &online_raw_text,
        selected_text,
        &fast_candidate,
        "fast_online",
    )
}

fn emit_streaming_partial_if_changed(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    session: &mut StreamingCoreSession,
    rewrite_enabled: bool,
    proxy: &EventLoopProxy<AppEvent>,
    total_audio_duration_ms: u64,
) -> Result<()> {
    if let Some(fast_update) =
        prepare_fast_streaming_partial(recognizer, session, total_audio_duration_ms)
    {
        tracing::debug!(
            samples = session.captured_samples.len(),
            audio_duration_ms = total_audio_duration_ms,
            decode_steps = session.total_decode_steps,
            total_chunks_fed = session.total_chunks_fed,
            selected_preview_source = fast_update.source,
            stable_chars = fast_update.stable_chars,
            frozen_chars = fast_update.frozen_chars,
            volatile_chars = fast_update.volatile_chars,
            rejected_frozen_edit = fast_update.rejected_prefix_rewrite,
            raw_text = %fast_update.raw_text,
            online_prepared_text = %fast_update.online_prepared_text,
            prepared_text = %fast_update.display_text,
            "streaming fast partial updated"
        );
        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
            raw_text: fast_update.raw_text,
            prepared_text: fast_update.display_text,
        }));
    }

    let Some(update) = update_streaming_partial_state(
        runtime,
        recognizer,
        punctuator,
        session,
        rewrite_enabled,
        total_audio_duration_ms,
    )?
    else {
        return Ok(());
    };

    tracing::debug!(
        samples = session.captured_samples.len(),
        audio_duration_ms = total_audio_duration_ms,
        decode_steps = session.total_decode_steps,
        total_chunks_fed = session.total_chunks_fed,
        selected_preview_source = update.source,
        stable_chars = update.stable_chars,
        frozen_chars = update.frozen_chars,
        volatile_chars = update.volatile_chars,
        rejected_frozen_edit = update.rejected_prefix_rewrite,
        raw_text = %update.raw_text,
        online_prepared_text = %update.online_prepared_text,
        prepared_text = %update.display_text,
        "streaming partial updated"
    );
    session.last_fast_preview_text = update.display_text.clone();
    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
        raw_text: update.raw_text,
        prepared_text: update.display_text,
    }));
    Ok(())
}

fn maybe_apply_streaming_ai_rewrite(
    runtime: &AppRuntime,
    session: &mut StreamingCoreSession,
    selected_preview_text: &str,
) -> Result<String> {
    let Some(ai_rewriter) = runtime.ai_rewriter.as_ref() else {
        tracing::debug!("streaming AI rewrite skipped because client is unavailable");
        return Ok(selected_preview_text.trim().to_string());
    };
    let config = &runtime.config.voice.streaming.ai_rewrite;
    let _ = poll_streaming_ai_rewrite_result(session, config);
    let trimmed_preview = selected_preview_text.trim();
    if trimmed_preview.is_empty() {
        tracing::info!("streaming AI rewrite skipped because preview text is empty");
        return Ok(trimmed_preview.to_string());
    }

    let effective_min_visible_chars =
        effective_ai_rewrite_min_visible_chars(config.min_visible_chars);
    let Some(candidate) =
        build_streaming_ai_rewrite_candidate(selected_preview_text, effective_min_visible_chars)
    else {
        let (_, current_tail) = split_frozen_prefix(trimmed_preview);
        tracing::info!(
            preview_chars = trimmed_preview.chars().count(),
            current_tail_chars = visible_text_char_count(&current_tail),
            min_visible_chars = effective_min_visible_chars,
            current_tail = %short_log_text(&current_tail, 120),
            "streaming AI rewrite skipped because current tail is too short"
        );
        return Ok(trimmed_preview.to_string());
    };

    let request_key = format!("{}\n{}", candidate.frozen_prefix, candidate.current_tail);
    if session.last_ai_rewrite_input == request_key {
        if !session.last_ai_rewrite_output.is_empty() {
            tracing::info!(
                request_key = %short_log_text(&request_key, 120),
                rewritten_tail = %short_log_text(&session.last_ai_rewrite_output, 120),
                "streaming AI rewrite reused cached output"
            );
            return Ok(format!(
                "{}{}",
                candidate.frozen_prefix, session.last_ai_rewrite_output
            ));
        }
        tracing::info!(
            request_key = %short_log_text(&request_key, 120),
            "streaming AI rewrite reused cached miss"
        );
        return Ok(trimmed_preview.to_string());
    }
    if session.ai_rewrite_inflight_input == request_key {
        tracing::info!(
            request_key = %short_log_text(&request_key, 120),
            "streaming AI rewrite skipped because same request is already inflight"
        );
        return Ok(trimmed_preview.to_string());
    }
    if session.ai_rewrite_result_rx.is_some() {
        tracing::info!("streaming AI rewrite skipped because another request result is pending");
        return Ok(trimmed_preview.to_string());
    }

    let now = Instant::now();
    if session
        .last_ai_rewrite_at
        .is_some_and(|last| now.duration_since(last) < Duration::from_millis(config.debounce_ms))
    {
        tracing::info!(
            debounce_ms = config.debounce_ms,
            request_key = %short_log_text(&request_key, 120),
            "streaming AI rewrite skipped because debounce window is active"
        );
        return Ok(trimmed_preview.to_string());
    }
    session.last_ai_rewrite_at = Some(now);
    spawn_streaming_ai_rewrite_request(
        runtime,
        session,
        ai_rewriter.clone(),
        candidate,
        request_key,
    );
    Ok(selected_preview_text.trim().to_string())
}

fn spawn_streaming_ai_rewrite_request(
    runtime: &AppRuntime,
    session: &mut StreamingCoreSession,
    ai_rewriter: Arc<crate::ai_rewrite::AiRewriteClient>,
    candidate: StreamingAiRewriteCandidate,
    request_key: String,
) {
    let (result_tx, result_rx) = mpsc::channel();
    let context = runtime.output_controller.inspect_context_snapshot();
    session.ai_rewrite_inflight_input = request_key.clone();
    session.ai_rewrite_result_rx = Some(result_rx);
    tracing::info!(
        request_key = %short_log_text(&request_key, 120),
        frozen_prefix = %short_log_text(&candidate.frozen_prefix, 120),
        current_tail = %short_log_text(&candidate.current_tail, 120),
        process_name = context.process_name.as_deref().unwrap_or("unknown"),
        context_kind = ?context.kind,
        "streaming AI rewrite request queued"
    );

    std::thread::spawn(move || {
        let outcome = match ai_rewriter.rewrite_tail(AiRewriteRequest {
            frozen_prefix: candidate.frozen_prefix.clone(),
            current_tail: candidate.current_tail.clone(),
            context,
        }) {
            Ok(response) => StreamingAiRewriteOutcome {
                request_key,
                candidate,
                rewritten_tail: response.map(|value| value.rewritten_tail),
                error: None,
            },
            Err(error) => StreamingAiRewriteOutcome {
                request_key,
                candidate,
                rewritten_tail: None,
                error: Some(error.to_string()),
            },
        };
        let _ = result_tx.send(outcome);
    });
}

fn poll_streaming_ai_rewrite_result(
    session: &mut StreamingCoreSession,
    config: &ainput_shell::StreamingAiRewriteConfig,
) -> Option<StreamingAiRewriteDisplayUpdate> {
    let Some(result_rx) = session.ai_rewrite_result_rx.take() else {
        return None;
    };

    match result_rx.try_recv() {
        Ok(outcome) => {
            session.ai_rewrite_inflight_input.clear();
            session.last_ai_rewrite_input = outcome.request_key;
            session.last_ai_rewrite_output.clear();

            if let Some(error) = outcome.error {
                tracing::warn!(
                    request_key = %short_log_text(&session.last_ai_rewrite_input, 120),
                    error = %error,
                    "streaming AI rewrite failed asynchronously; keep online preview"
                );
                return None;
            }

            let Some(raw_tail) = outcome.rewritten_tail else {
                tracing::info!(
                    request_key = %short_log_text(&session.last_ai_rewrite_input, 120),
                    "streaming AI rewrite completed with empty response"
                );
                return None;
            };
            if let Some(rewritten_tail) = sanitize_ai_rewrite_output(
                &outcome.candidate.frozen_prefix,
                &outcome.candidate.current_tail,
                &raw_tail,
                effective_ai_rewrite_min_visible_chars(config.min_visible_chars),
                config.max_output_chars,
            ) {
                tracing::info!(
                    request_key = %short_log_text(&session.last_ai_rewrite_input, 120),
                    raw_tail = %short_log_text(&raw_tail, 120),
                    rewritten_tail = %short_log_text(&rewritten_tail, 120),
                    "streaming AI rewrite result adopted"
                );
                session.last_ai_rewrite_output = rewritten_tail;
                return Some(StreamingAiRewriteDisplayUpdate {
                    request_key: session.last_ai_rewrite_input.clone(),
                    display_text: format!(
                        "{}{}",
                        outcome.candidate.frozen_prefix, session.last_ai_rewrite_output
                    ),
                });
            } else {
                tracing::info!(
                    request_key = %short_log_text(&session.last_ai_rewrite_input, 120),
                    raw_tail = %short_log_text(&raw_tail, 120),
                    "streaming AI rewrite result rejected by sanitizer"
                );
            }
            None
        }
        Err(mpsc::TryRecvError::Empty) => {
            session.ai_rewrite_result_rx = Some(result_rx);
            None
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            session.ai_rewrite_inflight_input.clear();
            tracing::warn!("streaming AI rewrite result channel disconnected");
            None
        }
    }
}

fn drain_final_streaming_ai_rewrite(
    runtime: &AppRuntime,
    session: &mut StreamingCoreSession,
    proxy: &EventLoopProxy<AppEvent>,
) -> Result<()> {
    let current_display = effective_streaming_display_text(session);
    if current_display.trim().is_empty() {
        return Ok(());
    }

    session.last_ai_rewrite_at = None;
    let preview_after_cached_rewrite =
        maybe_apply_streaming_ai_rewrite(runtime, session, &current_display)?;
    if preview_after_cached_rewrite != current_display {
        emit_streaming_partial_override(
            session,
            &current_display,
            &preview_after_cached_rewrite,
            "ai_rewrite_cached",
            proxy,
        );
    }

    let wait_started_at = Instant::now();
    let wait_budget = Duration::from_millis(STREAMING_FINAL_AI_REWRITE_WAIT_MS);
    let config = &runtime.config.voice.streaming.ai_rewrite;
    let mut received_result = false;

    while wait_started_at.elapsed() < wait_budget {
        if apply_ready_streaming_ai_rewrite_result(session, config, proxy) {
            received_result = true;
            break;
        }
        if session.ai_rewrite_result_rx.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(STREAMING_RELEASE_POLL_INTERVAL_MS));
    }

    if !received_result {
        received_result = apply_ready_streaming_ai_rewrite_result(session, config, proxy);
    }

    tracing::info!(
        waited_ms = wait_started_at.elapsed().as_millis(),
        received_result,
        pending_after_wait = session.ai_rewrite_result_rx.is_some(),
        "streaming final AI rewrite wait finished"
    );

    Ok(())
}

fn apply_ready_streaming_ai_rewrite_result(
    session: &mut StreamingCoreSession,
    config: &ainput_shell::StreamingAiRewriteConfig,
    proxy: &EventLoopProxy<AppEvent>,
) -> bool {
    let Some(update) = poll_streaming_ai_rewrite_result(session, config) else {
        return false;
    };

    emit_streaming_partial_override(
        session,
        &update.display_text,
        &update.display_text,
        "ai_rewrite_async",
        proxy,
    );
    tracing::info!(
        request_key = %short_log_text(&update.request_key, 120),
        display_text = %short_log_text(&update.display_text, 120),
        "streaming AI rewrite result synced into HUD before final commit"
    );
    true
}

fn emit_streaming_partial_override(
    session: &mut StreamingCoreSession,
    raw_text: &str,
    display_text: &str,
    source: &'static str,
    proxy: &EventLoopProxy<AppEvent>,
) {
    let Some(update) =
        apply_streaming_partial_update(session, raw_text, display_text, display_text, source)
    else {
        return;
    };

    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
        raw_text: update.raw_text,
        prepared_text: update.display_text,
    }));
}

fn emit_streaming_final_preview_sync(
    session: &mut StreamingCoreSession,
    final_text: &str,
    proxy: &EventLoopProxy<AppEvent>,
) {
    let final_trimmed = final_text.trim();
    if final_trimmed.is_empty() || session.last_display_text == final_trimmed {
        return;
    }

    session.last_raw_partial = final_trimmed.to_string();
    session.last_display_text = final_trimmed.to_string();
    session.last_fast_preview_text = final_trimmed.to_string();

    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
        raw_text: final_trimmed.to_string(),
        prepared_text: final_trimmed.to_string(),
    }));
}

fn apply_streaming_partial_update(
    session: &mut StreamingCoreSession,
    raw_text: &str,
    candidate_display_text: &str,
    online_prepared_text: &str,
    source: &'static str,
) -> Option<PreparedStreamingPartial> {
    let update = session.state.apply_online_partial(candidate_display_text)?;
    if update.rejected_prefix_rewrite && session.last_display_text == update.display_text {
        return None;
    }

    session.last_raw_partial = raw_text.to_string();
    session.last_display_text = update.display_text.clone();
    session.last_fast_preview_text = update.display_text.clone();
    if session.first_partial_at.is_none() && !update.display_text.trim().is_empty() {
        session.first_partial_at = Some(Instant::now());
    }
    session.partial_updates += 1;

    Some(PreparedStreamingPartial {
        raw_text: raw_text.to_string(),
        online_prepared_text: online_prepared_text.to_string(),
        display_text: update.display_text,
        source,
        stable_chars: update.stable_chars,
        frozen_chars: update.frozen_chars,
        volatile_chars: update.volatile_chars,
        rejected_prefix_rewrite: update.rejected_prefix_rewrite,
    })
}

fn build_streaming_ai_rewrite_candidate(
    display_text: &str,
    min_visible_chars: usize,
) -> Option<StreamingAiRewriteCandidate> {
    let trimmed = display_text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (frozen_prefix, current_tail) = split_frozen_prefix(trimmed);
    if visible_text_char_count(&current_tail) < min_visible_chars.max(1) {
        return None;
    }

    Some(StreamingAiRewriteCandidate {
        frozen_prefix,
        current_tail: current_tail.trim().to_string(),
    })
}

fn effective_ai_rewrite_min_visible_chars(configured_min_visible_chars: usize) -> usize {
    configured_min_visible_chars.clamp(2, 48)
}

fn sanitize_ai_rewrite_output(
    frozen_prefix: &str,
    current_tail: &str,
    rewritten_text: &str,
    min_visible_chars: usize,
    max_output_chars: usize,
) -> Option<String> {
    let mut candidate = rewritten_text
        .trim()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '“' | '”' | '‘' | '’'))
        .trim()
        .to_string();

    for prefix in ["改写后：", "改写后:", "当前尾巴：", "当前尾巴:"] {
        if let Some(stripped) = candidate.strip_prefix(prefix) {
            candidate = stripped.trim().to_string();
        }
    }

    let frozen_prefix = frozen_prefix.trim();
    if !frozen_prefix.is_empty() && candidate.starts_with(frozen_prefix) {
        candidate = candidate[frozen_prefix.len()..].trim().to_string();
    }

    if candidate.is_empty() {
        tracing::info!("streaming AI rewrite sanitizer rejected empty candidate");
        return None;
    }

    let visible_chars = visible_text_char_count(&candidate);
    if visible_chars < min_visible_chars.max(1) || visible_chars > max_output_chars.max(8) {
        tracing::info!(
            visible_chars,
            min_visible_chars = min_visible_chars.max(1),
            max_output_chars = max_output_chars.max(8),
            candidate = %short_log_text(&candidate, 120),
            "streaming AI rewrite sanitizer rejected candidate because visible length is out of range"
        );
        return None;
    }

    if candidate == current_tail.trim() {
        return Some(candidate);
    }

    Some(candidate)
}

fn short_log_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut shortened = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars {
        shortened.push_str("...");
    }
    shortened
}

fn reset_streaming_ai_rewrite_cache(session: &mut StreamingCoreSession) {
    session.ai_rewrite_result_rx = None;
    session.ai_rewrite_inflight_input.clear();
    session.last_ai_rewrite_input.clear();
    session.last_ai_rewrite_output.clear();
    session.last_ai_rewrite_at = None;
}

fn maybe_rollover_streaming_segment_core(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    session: &mut StreamingCoreSession,
    rewrite_enabled: bool,
    total_audio_duration_ms: u64,
) -> Option<String> {
    let activity =
        analyze_recent_audio_activity(&session.captured_samples, session.sample_rate_hz, 500);
    if session.awaiting_post_rollover_speech {
        if should_skip_streaming_preview(&activity) {
            return None;
        }
        session.awaiting_post_rollover_speech = false;
    }

    if !recognizer.is_endpoint(&session.stream) {
        return None;
    }

    let current_display = effective_streaming_display_text(session);
    if visible_text_char_count(&current_display) == 0 {
        recognizer.reset(&session.stream);
        session.last_raw_partial.clear();
        session.last_fast_preview_text.clear();
        session.last_display_text.clear();
        reset_streaming_ai_rewrite_cache(session);
        return None;
    }

    let current_raw_text = recognizer
        .get_result(&session.stream)
        .map(|result| result.text.trim().to_string())
        .unwrap_or_default();
    let committed_text =
        prepare_streaming_pause_boundary_text(&current_display, rewrite_enabled, punctuator);
    let committed_text = if committed_text.is_empty() {
        ensure_terminal_sentence_boundary(&current_display)
    } else {
        committed_text
    };

    let update = session.state.freeze_with_committed_text(&committed_text);
    session.rolled_over_prefix = update.display_text.clone();
    session.awaiting_post_rollover_speech = true;
    session.last_raw_partial.clear();
    session.last_fast_preview_text.clear();
    session.last_display_text = update.display_text.clone();
    reset_streaming_ai_rewrite_cache(session);
    recognizer.reset(&session.stream);

    tracing::info!(
        audio_duration_ms = total_audio_duration_ms,
        endpoint_raw_text = %current_raw_text,
        endpoint_text_before_reset = %current_display,
        endpoint_committed_text = %update.display_text,
        frozen_chars = update.frozen_chars,
        volatile_chars = update.volatile_chars,
        "streaming endpoint detected; rolled over to next segment"
    );

    Some(update.display_text)
}

fn select_streaming_preview_text<'a>(
    current_display: &'a str,
    online_prepared: &'a str,
) -> StreamingTextChoice<'a> {
    let current_trimmed = current_display.trim();
    let online_trimmed = online_prepared.trim();
    let candidate = if visible_text_char_count(online_trimmed) > 0 {
        StreamingTextChoice {
            source: "online",
            text: online_trimmed,
        }
    } else {
        StreamingTextChoice {
            source: "none",
            text: "",
        }
    };

    if candidate.text.is_empty() || current_trimmed.is_empty() {
        return candidate;
    }

    let current_visible = visible_text_char_count(current_trimmed);
    let candidate_visible = visible_text_char_count(candidate.text);
    if current_visible >= 6
        && candidate_visible + STREAMING_PREVIEW_SHORTFALL_TOLERANCE_CHARS < current_visible
        && !is_segment_only_streaming_candidate(current_trimmed, candidate.text)
    {
        let common_prefix_chars = longest_common_prefix_chars(current_trimmed, candidate.text);
        if common_prefix_chars + STREAMING_PREVIEW_SHORTFALL_TOLERANCE_CHARS < current_visible {
            return StreamingTextChoice {
                source: "held_display",
                text: current_trimmed,
            };
        }
    }

    candidate
}

fn select_streaming_commit_text<'a>(
    display_text: &'a str,
    final_candidate: &'a str,
    final_candidate_source: StreamingCommitSource,
) -> StreamingCommitChoice<'a> {
    let display_trimmed = display_text.trim();
    let candidate_trimmed = final_candidate.trim();
    let selected_candidate = if visible_text_char_count(candidate_trimmed) > 0 {
        StreamingCommitChoice {
            source: final_candidate_source,
            text: candidate_trimmed,
        }
    } else {
        StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: "",
        }
    };

    if display_trimmed.is_empty() {
        return selected_candidate;
    }

    if selected_candidate.text.is_empty() {
        return StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: display_trimmed,
        };
    }

    let display_visible = visible_text_char_count(display_trimmed);
    let selected_visible = visible_text_char_count(selected_candidate.text);
    if selected_visible + STREAMING_FINAL_SHORTFALL_TOLERANCE_CHARS < display_visible
        && !is_segment_only_streaming_candidate(display_trimmed, selected_candidate.text)
    {
        return StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: display_trimmed,
        };
    }

    let common_prefix_chars = longest_common_prefix_chars(display_trimmed, selected_candidate.text);
    if common_prefix_chars < 4
        && display_visible > selected_visible
        && !is_segment_only_streaming_candidate(display_trimmed, selected_candidate.text)
    {
        return StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: display_trimmed,
        };
    }

    selected_candidate
}

fn is_segment_only_streaming_candidate(current_display: &str, candidate_text: &str) -> bool {
    let (frozen_prefix, _) = split_frozen_prefix(current_display.trim());
    can_append_segment_only_candidate(candidate_text, &frozen_prefix)
}

fn effective_streaming_display_text(session: &StreamingCoreSession) -> String {
    merge_rolled_over_prefix(
        &session.rolled_over_prefix,
        session.state.full_text().trim(),
    )
}

fn merge_rolled_over_prefix(rolled_over_prefix: &str, current_display: &str) -> String {
    let current_display = current_display.trim();
    if rolled_over_prefix.is_empty()
        || current_display.is_empty()
        || current_display.starts_with(rolled_over_prefix)
    {
        return current_display.to_string();
    }

    if can_append_segment_only_candidate(current_display, rolled_over_prefix) {
        return format!("{}{}", rolled_over_prefix, current_display);
    }

    current_display.to_string()
}

fn prepare_streaming_pause_boundary_text(
    text: &str,
    rewrite_enabled: bool,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
) -> String {
    let (prepared, _) = prepare_streaming_output_text(text, rewrite_enabled, punctuator);
    ensure_terminal_sentence_boundary(if prepared.trim().is_empty() {
        text
    } else {
        &prepared
    })
}

fn ensure_terminal_sentence_boundary(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || has_terminal_sentence_boundary(trimmed) {
        return trimmed.to_string();
    }

    format!("{trimmed}。")
}

fn has_terminal_sentence_boundary(text: &str) -> bool {
    text.chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | ';' | '。' | '！' | '？' | '；'))
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
    activity.peak_abs < 0.0065 || (activity.rms < 0.0018 && activity.active_ratio < 0.012)
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

    if activity.rms < 0.0018 && activity.active_ratio < 0.01 && stripped.chars().count() <= 1 {
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

fn read_wav_samples(wav_path: &Path) -> Result<(i32, Vec<f32>)> {
    let mut reader = hound::WavReader::open(wav_path)
        .with_context(|| format!("read wav file {}", wav_path.display()))?;
    let spec = reader.spec();
    let sample_rate_hz =
        i32::try_from(spec.sample_rate).context("wav sample rate does not fit in i32")?;
    let raw_samples = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|sample| sample.map(|value| f32::from(value) / f32::from(i8::MAX)))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|sample| sample.map(|value| f32::from(value) / f32::from(i16::MAX)))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        (hound::SampleFormat::Int, 24) | (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|sample| sample.map(|value| value as f32 / i32::MAX as f32))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("decode wav samples {}", wav_path.display()))?,
        _ => bail!(
            "unsupported wav format: sample_format={:?}, bits_per_sample={}",
            spec.sample_format,
            spec.bits_per_sample
        ),
    };

    let channels = usize::from(spec.channels.max(1));
    if channels == 1 {
        return Ok((sample_rate_hz, raw_samples));
    }

    let mut mixed = Vec::with_capacity(raw_samples.len() / channels + 1);
    for frame in raw_samples.chunks(channels) {
        let sum: f32 = frame.iter().copied().sum();
        mixed.push(sum / frame.len() as f32);
    }
    Ok((sample_rate_hz, mixed))
}

fn audio_duration_ms(sample_rate_hz: i32, samples_len: usize) -> u64 {
    if sample_rate_hz <= 0 {
        return 0;
    }

    ((samples_len as f64 / sample_rate_hz as f64) * 1000.0).round() as u64
}

fn current_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.to_string()
}

fn log_streaming_timing_gate_results(
    first_partial_elapsed_ms: Option<u128>,
    release_to_commit_elapsed_ms: u128,
) {
    if let Some(first_partial_elapsed_ms) = first_partial_elapsed_ms {
        let gate_status = if first_partial_elapsed_ms <= STREAMING_FIRST_PARTIAL_TARGET_MS {
            "target_pass"
        } else if first_partial_elapsed_ms <= STREAMING_FIRST_PARTIAL_HARD_MS {
            "hard_pass"
        } else {
            "hard_fail"
        };
        tracing::info!(
            first_partial_elapsed_ms,
            target_ms = STREAMING_FIRST_PARTIAL_TARGET_MS,
            hard_ms = STREAMING_FIRST_PARTIAL_HARD_MS,
            gate_status,
            "streaming first-partial timing gate evaluated"
        );
    }

    let gate_status = if release_to_commit_elapsed_ms <= STREAMING_RELEASE_TO_COMMIT_TARGET_MS {
        "target_pass"
    } else if release_to_commit_elapsed_ms <= STREAMING_RELEASE_TO_COMMIT_HARD_MS {
        "hard_pass"
    } else {
        "hard_fail"
    };
    tracing::info!(
        release_to_commit_elapsed_ms,
        target_ms = STREAMING_RELEASE_TO_COMMIT_TARGET_MS,
        hard_ms = STREAMING_RELEASE_TO_COMMIT_HARD_MS,
        gate_status,
        "streaming release-to-commit timing gate evaluated"
    );
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

fn prepare_streaming_preview_text(
    current_partial: &str,
    rewrite_enabled: bool,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
) -> String {
    let normalized = ainput_rewrite::normalize_streaming_preview(current_partial);
    if !rewrite_enabled {
        return normalized;
    }

    apply_streaming_punctuation(&normalized, punctuator, false)
}

fn prepare_streaming_output_text(
    final_text: &str,
    rewrite_enabled: bool,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
) -> (String, StreamingCommitSource) {
    if final_text.trim().is_empty() {
        return (String::new(), StreamingCommitSource::OnlineFinal);
    }

    let normalized = ainput_rewrite::normalize_transcription(final_text);
    let prepared = if rewrite_enabled {
        apply_streaming_punctuation(&normalized, punctuator, true)
    } else {
        normalized
    };
    let source = if prepared != final_text {
        StreamingCommitSource::StreamingTailRepair
    } else {
        StreamingCommitSource::OnlineFinal
    };
    (prepared, source)
}

fn apply_streaming_punctuation(
    normalized_text: &str,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    finalize: bool,
) -> String {
    let normalized = normalized_text.trim();
    if normalized.is_empty() {
        return String::new();
    }

    let Some(punctuator) = punctuator else {
        return normalized.to_string();
    };

    let (frozen_prefix, latest_sentence) = split_frozen_prefix(normalized);
    let target_text = latest_sentence.trim();
    if target_text.is_empty() {
        return normalized.to_string();
    }

    let punctuated_latest = match punctuator.add_punctuation(target_text) {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(
                error = %error,
                text = %target_text,
                "streaming punctuation failed; keeping normalized text"
            );
            return normalized.to_string();
        }
    };

    let punctuated_latest = ainput_rewrite::normalize_transcription(&punctuated_latest);
    let punctuated_latest = if finalize {
        punctuated_latest
    } else {
        strip_trailing_terminal_sentence_punctuation(&punctuated_latest)
    };

    format!("{}{}", frozen_prefix, punctuated_latest)
        .trim()
        .to_string()
}

fn strip_trailing_terminal_sentence_punctuation(text: &str) -> String {
    text.trim_end_matches(|ch: char| {
        matches!(ch, '.' | '!' | '?' | ';' | '。' | '！' | '？' | '；')
    })
    .trim_end()
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        AudioActivity, StreamingCommitSource, StreamingResampler, analyze_recent_audio_activity,
        build_streaming_ai_rewrite_candidate, effective_ai_rewrite_min_visible_chars,
        ensure_terminal_sentence_boundary, merge_rolled_over_prefix, prepare_streaming_output_text,
        prepare_streaming_pause_boundary_text, prepare_streaming_preview_text,
        sanitize_ai_rewrite_output, select_streaming_commit_text, select_streaming_preview_text,
        should_drop_streaming_preview_result, should_skip_streaming_preview,
        strip_trailing_terminal_sentence_punctuation,
    };

    #[test]
    fn streaming_preview_falls_back_to_normalized_text_without_punctuator() {
        assert_eq!(
            prepare_streaming_preview_text("嗯， 帮我看一下 这个功能", true, None),
            "帮我看一下 这个功能"
        );
    }

    #[test]
    fn streaming_preview_can_skip_rewrite() {
        assert_eq!(
            prepare_streaming_preview_text("嗯， 帮我看一下 这个功能", false, None),
            "帮我看一下 这个功能"
        );
    }

    #[test]
    fn preview_strips_only_trailing_terminal_punctuation() {
        assert_eq!(
            strip_trailing_terminal_sentence_punctuation("第一句，第二句。"),
            "第一句，第二句"
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
        assert!(should_drop_streaming_preview_result("喂", &activity));
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

    #[test]
    fn commit_text_prefers_display_when_final_candidate_is_too_short() {
        assert_eq!(
            select_streaming_commit_text(
                "帮我看一下这里有没有问题",
                "帮我看一下",
                StreamingCommitSource::OnlineFinal
            )
            .text,
            "帮我看一下这里有没有问题"
        );
    }

    #[test]
    fn preview_text_holds_existing_display_when_candidate_regresses() {
        let selected = select_streaming_preview_text("帮我看一下这里有没有问题", "帮我看");
        assert_eq!(selected.source, "held_display");
        assert_eq!(selected.text, "帮我看一下这里有没有问题");
    }

    #[test]
    fn preview_text_accepts_latest_sentence_only_candidate() {
        let selected =
            select_streaming_preview_text("第一句已经稳定。第二句还在继续", "第三句开始");
        assert_eq!(selected.source, "online");
        assert_eq!(selected.text, "第三句开始");
    }

    #[test]
    fn weak_but_real_preview_is_not_dropped_too_early() {
        let activity = AudioActivity {
            peak_abs: 0.014,
            rms: 0.0031,
            active_ratio: 0.0204,
        };
        assert!(!should_drop_streaming_preview_result("我不知", &activity));
    }

    #[test]
    fn streaming_preview_accepts_weaker_but_real_speech_sooner() {
        let activity = AudioActivity {
            peak_abs: 0.0068,
            rms: 0.0019,
            active_ratio: 0.0125,
        };
        assert!(!should_skip_streaming_preview(&activity));
    }

    #[test]
    fn prepare_streaming_output_marks_tail_repair_source() {
        let (prepared, source) = prepare_streaming_output_text("你好 ", true, None);
        assert_eq!(prepared, "你好");
        assert_eq!(source, StreamingCommitSource::StreamingTailRepair);
    }

    #[test]
    fn ai_rewrite_candidate_only_targets_latest_tail() {
        let candidate = build_streaming_ai_rewrite_candidate("第一句已经稳定。第二句先是错字", 2)
            .expect("candidate");
        assert_eq!(candidate.frozen_prefix, "第一句已经稳定。");
        assert_eq!(candidate.current_tail, "第二句先是错字");
    }

    #[test]
    fn ai_rewrite_min_visible_chars_is_capped_to_short_tail_floor() {
        assert_eq!(effective_ai_rewrite_min_visible_chars(1), 2);
        assert_eq!(effective_ai_rewrite_min_visible_chars(2), 2);
        assert_eq!(effective_ai_rewrite_min_visible_chars(6), 6);
    }

    #[test]
    fn ai_rewrite_rejects_too_short_output() {
        assert_eq!(
            sanitize_ai_rewrite_output("第一句已经稳定。", "第二句先是错字", "好", 2, 20),
            None
        );
    }

    #[test]
    fn ai_rewrite_strips_echoed_prefix() {
        assert_eq!(
            sanitize_ai_rewrite_output(
                "第一句已经稳定。",
                "第二句先是错字",
                "第一句已经稳定。第二句已经修正",
                2,
                20
            ),
            Some("第二句已经修正".to_string())
        );
    }

    #[test]
    fn commit_text_accepts_latest_sentence_only_candidate() {
        let selected = select_streaming_commit_text(
            "第一句已经稳定。第二句还在继续",
            "第三句最终修正。",
            StreamingCommitSource::OnlineFinal,
        );
        assert_eq!(selected.source, StreamingCommitSource::OnlineFinal);
        assert_eq!(selected.text, "第三句最终修正。");
    }

    #[test]
    fn ensure_terminal_sentence_boundary_adds_full_stop_when_missing() {
        assert_eq!(
            ensure_terminal_sentence_boundary("第一句还没结束"),
            "第一句还没结束。"
        );
        assert_eq!(
            ensure_terminal_sentence_boundary("第一句已经结束。"),
            "第一句已经结束。"
        );
    }

    #[test]
    fn pause_boundary_text_forces_terminal_punctuation() {
        assert_eq!(
            prepare_streaming_pause_boundary_text("第一句还没标点", false, None),
            "第一句还没标点。"
        );
    }

    #[test]
    fn pause_boundary_text_preserves_existing_prefix() {
        assert_eq!(
            prepare_streaming_pause_boundary_text("第一句已经稳定。第二句继续", false, None),
            "第一句已经稳定。第二句继续。"
        );
    }

    #[test]
    fn effective_display_restores_missing_rollover_prefix() {
        assert_eq!(
            merge_rolled_over_prefix("第一句已经稳定。", "第二句继续"),
            "第一句已经稳定。第二句继续"
        );
    }

    #[test]
    fn streaming_resampler_does_not_drain_past_buffer_end() {
        let mut resampler = StreamingResampler::new(48_000, 16_000);
        let input = vec![0.25f32; 12_446];
        let output = resampler.process(&input);
        assert!(!output.is_empty());
    }
}
