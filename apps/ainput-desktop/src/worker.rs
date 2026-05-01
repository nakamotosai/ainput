use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use ainput_output::{OutputConfig, OutputDelivery};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use winit::event_loop::EventLoopProxy;

use crate::ai_rewrite::AiRewriteRequest;
use crate::streaming_fixtures::{
    StreamingCaseStatus, StreamingFixtureCase, StreamingFixtureManifest,
    StreamingReplayPartialEntry, StreamingReplayReport, StreamingSelftestReport,
};
use crate::streaming_state::{
    StreamingStabilityPolicy, StreamingState, can_append_segment_only_candidate,
    longest_common_prefix_chars, split_frozen_prefix, visible_text_char_count,
};
use crate::{AppEvent, AppRuntime, hotkey};

const VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(300);
const STREAMING_PASTE_STABILIZE_DELAY: Duration = Duration::from_millis(120);
const STREAMING_DEFAULT_TAIL_PADDING_MS: u64 = 720;
const STREAMING_RELEASE_MAX_WAIT_MS: u64 = 500;
const STREAMING_RELEASE_HARD_WAIT_MS: u64 = 650;
const STREAMING_RELEASE_MIN_WAIT_MS: u64 = 160;
const STREAMING_RELEASE_IDLE_SETTLE_MS: u64 = 160;
const STREAMING_RELEASE_POLL_INTERVAL_MS: u64 = 8;
const STREAMING_IDLE_FINALIZE_TAIL_PADDING_MS: u64 = 480;
const STREAMING_HUD_SOFT_FLUSH_MS: u64 = 360;
const STREAMING_HUD_SOFT_FLUSH_MIN_VISIBLE_CHARS: usize = 4;
const STREAMING_HUD_SOFT_FLUSH_TAIL_PADDING_MS: u64 = 240;
const STREAMING_FINAL_AI_REWRITE_WAIT_MS: u64 = 280;
const STREAMING_HUD_FINAL_ACK_TIMEOUT_MS: u64 = 650;
const STREAMING_PREVIEW_SHORTFALL_TOLERANCE_CHARS: usize = 3;
const STREAMING_FINAL_SHORTFALL_TOLERANCE_CHARS: usize = 3;
const STREAMING_DEFAULT_PREROLL_MS: u64 = 180;
const STREAMING_SHERPA_FALLBACK_TRAILING_SILENCE_SECS: f32 = 60.0;
const STREAMING_SHERPA_FALLBACK_MAX_UTTERANCE_SECS: f32 = 60.0;
const STREAMING_FIRST_PARTIAL_TARGET_MS: u128 = 700;
const STREAMING_FIRST_PARTIAL_HARD_MS: u128 = 900;
const STREAMING_RELEASE_TAIL_TARGET_MS: u128 = 500;
const STREAMING_RELEASE_TAIL_HARD_MS: u128 = STREAMING_RELEASE_HARD_WAIT_MS as u128;
const STREAMING_OFFLINE_FINAL_TARGET_MS: u128 = 180;
const STREAMING_OFFLINE_FINAL_HARD_MS: u128 = 650;
const STREAMING_OFFLINE_FINAL_FULL_AUDIO_MAX_MS: u64 = 6_000;
const STREAMING_OFFLINE_FINAL_TAIL_WINDOW_MS: u64 = 3_200;
const STREAMING_PUNCTUATION_TARGET_MS: u128 = 120;
const STREAMING_PUNCTUATION_HARD_MS: u128 = 220;
const STREAMING_RELEASE_TO_COMMIT_TARGET_MS: u128 = 900;
const STREAMING_RELEASE_TO_COMMIT_HARD_MS: u128 = 1200;
const STREAMING_RAW_CAPTURE_LIMIT: usize = 20;
const AUDIO_ACTIVITY_FRAME_SAMPLES: usize = 320;
const AUDIO_ACTIVITY_SPEECH_FRAME_RMS: f32 = 0.004;
const AUDIO_ACTIVITY_FRAME_MS: u64 = 20;
const LOW_CONFIDENCE_SHORT_ENGLISH_SUSTAINED_VOICE_MS: u64 = 80;
const LOW_CONFIDENCE_SHORT_ENGLISH_RMS: f32 = 0.0065;
const LOW_CONFIDENCE_SHORT_ENGLISH_ACTIVE_RATIO: f32 = 0.06;
const STREAMING_FINAL_FUZZY_TAIL_OVERLAP_MIN_CHARS: usize = 4;
const STREAMING_FINAL_FUZZY_TAIL_OVERLAP_MAX_CHARS: usize = 18;
static STREAMING_SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
    StreamingFinalHudCommitRequest {
        final_text: String,
        response_tx: mpsc::Sender<StreamingHudCommitAck>,
    },
    StreamingClipboardFallback(String),
    StreamingFinal(String),
    Error(String),
    Unavailable(String),
}

#[derive(Debug, Clone)]
pub(crate) struct StreamingHudCommitAck {
    pub text: String,
    pub visible: bool,
    pub elapsed_ms: u128,
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
    session_id: String,
    input_sample_rate_hz: i32,
    sample_rate_hz: i32,
    stream: ainput_asr::StreamingZipformerStream,
    pending_feed_samples: Vec<f32>,
    captured_samples: Vec<f32>,
    ingested_input_samples: usize,
    resampler: StreamingResampler,
    state: StreamingState,
    endpoint: StreamingEndpointTracker,
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
    last_partial_audio_ms: Option<u64>,
    last_soft_flush_audio_ms: Option<u64>,
    started_at: Instant,
    commit_locked: bool,
    post_hud_flush_mutation_count: usize,
}

#[derive(Debug, Clone, Default)]
struct StreamingEndpointTracker {
    segment_start_ms: u64,
    speech_started_ms: Option<u64>,
    last_voice_ms: Option<u64>,
}

impl StreamingEndpointTracker {
    fn observe(&mut self, audio_duration_ms: u64, voice_active: bool) {
        if !voice_active {
            return;
        }

        if self.speech_started_ms.is_none() {
            self.speech_started_ms = Some(audio_duration_ms);
        }
        self.last_voice_ms = Some(audio_duration_ms);
    }

    fn reset_after_rollover(&mut self, audio_duration_ms: u64) {
        self.segment_start_ms = audio_duration_ms;
        self.speech_started_ms = None;
        self.last_voice_ms = None;
    }
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
    OfflineFinal,
}

impl StreamingCommitSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::StreamingState => "streaming_state",
            Self::StreamingTailRepair => "streaming_tail_repair",
            Self::OnlineFinal => "online_final",
            Self::OfflineFinal => "offline_final",
        }
    }
}

#[derive(Debug, Clone)]
struct StreamingCommitChoice {
    source: StreamingCommitSource,
    text: String,
}

trait StreamingOutputAdapter {
    fn commit_text(&self, text: &str, config: &OutputConfig) -> Result<OutputDelivery>;
}

struct ClipboardStreamingOutputAdapter<'a> {
    controller: &'a ainput_output::OutputController,
}

impl StreamingOutputAdapter for ClipboardStreamingOutputAdapter<'_> {
    fn commit_text(&self, text: &str, config: &OutputConfig) -> Result<OutputDelivery> {
        let _voice_hotkey_suppression = hotkey::suppress_voice_hotkey_for_output();
        self.controller.deliver_text(text, config)
    }
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
    final_offline_raw_text: String,
    final_prepared_candidate: String,
    final_text: String,
    commit_source: StreamingCommitSource,
    raw_capture_wav: String,
    raw_capture_metadata: String,
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
        let sequence = STREAMING_SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self {
            session_id: format!("streaming-{sequence}"),
            input_sample_rate_hz,
            sample_rate_hz,
            stream,
            pending_feed_samples: Vec::new(),
            captured_samples: Vec::new(),
            ingested_input_samples: 0,
            resampler: StreamingResampler::new(input_sample_rate_hz, sample_rate_hz),
            state: StreamingState::default(),
            endpoint: StreamingEndpointTracker::default(),
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
            last_partial_audio_ms: None,
            last_soft_flush_audio_ms: None,
            started_at: Instant::now(),
            commit_locked: false,
            post_hud_flush_mutation_count: 0,
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
                                    sustained_voice_ms = activity.sustained_voice_ms,
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
                                        sustained_voice_ms = activity.sustained_voice_ms,
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
                                                sustained_voice_ms = activity.sustained_voice_ms,
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
                                            allow_native_edit: false,
                                            restore_clipboard_after_paste: true,
                                            defer_clipboard_restore: false,
                                            preserve_text_exactly: false,
                                        };
                                        let delivery_result = {
                                            let _voice_hotkey_suppression =
                                                hotkey::suppress_voice_hotkey_for_output();
                                            runtime
                                                .output_controller
                                                .deliver_text(&text, &output_config)
                                        };
                                        match delivery_result {
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
                                                    OutputDelivery::NativeEdit
                                                    | OutputDelivery::DirectPaste => {
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
    let final_repair_recognizer = build_streaming_final_recognizer(&runtime).map_or_else(
        |error| {
            tracing::warn!(
                error = %error,
                "streaming offline final repair unavailable; final text will use streaming ASR only"
            );
            None
        },
        Some,
    );
    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Ready(WorkerKind::Streaming)));

    let mut standby_recording: Option<ainput_audio::ActiveRecording> = None;
    let mut active_session: Option<StreamingSession> = None;
    tracing::info!(
        fast_hotkey = %runtime.config.hotkeys.voice_input,
        streaming_effective_hotkey = "Ctrl",
        model_dir = %runtime.runtime_paths.root_dir.join(&runtime.config.voice.streaming.model_dir).display(),
        chunk_ms = runtime.config.voice.streaming.chunk_ms,
        streaming_asr_num_threads = effective_streaming_asr_num_threads(&runtime),
        streaming_final_num_threads = effective_streaming_final_num_threads(&runtime),
        streaming_punctuation_num_threads = effective_streaming_punctuation_num_threads(&runtime),
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
                            let preroll_ms = streaming_endpoint_preroll_ms(
                                &runtime.config.voice.streaming.endpoint,
                            );
                            let preroll_samples =
                                sample_count_for_ms(recording.sample_rate_hz(), preroll_ms);
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
                                preroll_ms,
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
                        let release_drain = match finish_streaming_recording(
                            &mut session,
                            recording,
                            runtime
                                .runtime_paths
                                .logs_dir
                                .join("streaming-raw-captures"),
                            &runtime.config.voice.streaming.finalize,
                        ) {
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
                            sustained_voice_ms = activity.sustained_voice_ms,
                            release_grace_added_samples = release_drain.grace_added_samples,
                            release_stop_added_samples = release_drain.stop_added_samples,
                            release_grace_wait_elapsed_ms = release_drain.grace_wait_elapsed_ms,
                            release_voice_active_observations =
                                release_drain.voice_active_observations,
                            release_tail_timeout_fallback = release_drain.timeout_fallback,
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
                                sustained_voice_ms = activity.sustained_voice_ms,
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
                            final_repair_recognizer.as_ref(),
                            punctuator.as_ref(),
                            &mut session,
                            sample_rate_hz,
                            chunk_samples,
                            streaming_endpoint_tail_padding_ms(
                                &runtime.config.voice.streaming.endpoint,
                            ),
                            runtime.config.voice.streaming.rewrite_enabled,
                        );

                        if prepared_commit.final_text.is_empty()
                            || should_drop_low_signal_result(&prepared_commit.final_text, &activity)
                        {
                            tracing::info!(
                                final_online_raw_text = %prepared_commit.final_online_raw_text,
                                final_offline_raw_text = %prepared_commit.final_offline_raw_text,
                                prepared_final_candidate = %prepared_commit.prepared_final_candidate,
                                display_text_before_final = %prepared_commit.display_text_before_final,
                                candidate_display_text = %prepared_commit.candidate_display_text,
                                selected_commit_source = prepared_commit.commit_source.as_str(),
                                final_decode_elapsed_ms = prepared_commit.final_decode_elapsed_ms,
                                online_final_elapsed_ms = prepared_commit.online_final_elapsed_ms,
                                offline_final_elapsed_ms = prepared_commit.offline_final_elapsed_ms,
                                offline_final_timed_out = prepared_commit.offline_final_timed_out,
                                punctuation_elapsed_ms = prepared_commit.punctuation_elapsed_ms,
                                final_decode_steps = prepared_commit.final_decode_steps,
                                rejected_prefix_rewrite = prepared_commit.rejected_prefix_rewrite,
                                peak_abs = format_args!("{:.6}", activity.peak_abs),
                                rms = format_args!("{:.6}", activity.rms),
                                active_ratio = format_args!("{:.4}", activity.active_ratio),
                                sustained_voice_ms = activity.sustained_voice_ms,
                                "drop empty or low-signal streaming final text"
                            );
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::IgnoredSilence));
                            continue;
                        }

                        let commit_envelope =
                            StreamingCommitEnvelope::from_prepared(&session, &prepared_commit);
                        let commit_text = ensure_terminal_sentence_boundary(
                            &commit_envelope.resolved_commit_text,
                        );
                        tracing::info!(
                            session_id = %commit_envelope.session_id,
                            revision = commit_envelope.revision,
                            last_hud_target_text = %commit_envelope.last_hud_target_text,
                            final_online_raw_text = %commit_envelope.final_online_raw_text,
                            final_offline_raw_text = %commit_envelope.final_offline_raw_text,
                            final_candidate_text = %commit_envelope.final_candidate_text,
                            candidate_display_text = %commit_envelope.candidate_display_text,
                            resolved_commit_text = %commit_envelope.resolved_commit_text,
                            commit_source = commit_envelope.commit_source.as_str(),
                            online_final_elapsed_ms = commit_envelope.online_final_elapsed_ms,
                            offline_final_elapsed_ms = commit_envelope.offline_final_elapsed_ms,
                            offline_final_timed_out = commit_envelope.offline_final_timed_out,
                            punctuation_elapsed_ms = commit_envelope.punctuation_elapsed_ms,
                            "streaming commit envelope created"
                        );

                        session.commit_locked = true;
                        let hud_final_flush_started_at = Instant::now();
                        let hud_ack = if runtime.config.voice.streaming.panel_enabled
                            && runtime
                                .config
                                .voice
                                .streaming
                                .commit
                                .require_hud_flush_before_commit
                        {
                            match request_streaming_final_hud_commit_ack(&proxy, &commit_text) {
                                Ok(ack) => ack,
                                Err(error) => {
                                    tracing::error!(
                                        error = %error,
                                        commit_text = %short_log_text(&commit_text, 120),
                                        "streaming final HUD commit ack failed; skip paste"
                                    );
                                    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                        format!("HUD 最终文本确认失败，已取消上屏：{error}"),
                                    )));
                                    continue;
                                }
                            }
                        } else {
                            StreamingHudCommitAck {
                                text: commit_text.clone(),
                                visible: false,
                                elapsed_ms: 0,
                            }
                        };
                        let hud_commit_text = hud_ack.text.trim().to_string();
                        if hud_commit_text.is_empty() {
                            tracing::error!("streaming final HUD commit ack returned empty text");
                            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Error(
                                "HUD 最终文本为空，已取消上屏".to_string(),
                            )));
                            continue;
                        }
                        let hud_final_flush_elapsed_ms =
                            hud_final_flush_started_at.elapsed().as_millis();

                        tracing::info!(
                            samples = session.captured_samples.len(),
                            audio_duration_ms = captured_audio_duration_ms,
                            final_online_raw_text = %prepared_commit.final_online_raw_text,
                            final_offline_raw_text = %prepared_commit.final_offline_raw_text,
                            display_text_before_final = %prepared_commit.display_text_before_final,
                            prepared_final_candidate = %prepared_commit.prepared_final_candidate,
                            candidate_display_text = %prepared_commit.candidate_display_text,
                            selected_commit_source = prepared_commit.commit_source.as_str(),
                            commit_text = %commit_text,
                            hud_commit_text = %hud_commit_text,
                            hud_ack_elapsed_ms = hud_ack.elapsed_ms,
                            hud_ack_visible = hud_ack.visible,
                            final_decode_steps = prepared_commit.final_decode_steps,
                            final_decode_elapsed_ms = prepared_commit.final_decode_elapsed_ms,
                            online_final_elapsed_ms = prepared_commit.online_final_elapsed_ms,
                            offline_final_elapsed_ms = prepared_commit.offline_final_elapsed_ms,
                            offline_final_timed_out = prepared_commit.offline_final_timed_out,
                            punctuation_elapsed_ms = prepared_commit.punctuation_elapsed_ms,
                            hud_final_flush_elapsed_ms,
                            post_hud_flush_mutation_count = session.post_hud_flush_mutation_count,
                            rejected_prefix_rewrite = prepared_commit.rejected_prefix_rewrite,
                            total_decode_steps = session.total_decode_steps,
                            "streaming final transcription ready"
                        );

                        let hotkey_release_wait_started_at = Instant::now();
                        let modifiers_released = hotkey::wait_for_voice_hotkey_release(
                            VOICE_OUTPUT_HOTKEY_RELEASE_TIMEOUT,
                        );
                        let hotkey_release_wait_elapsed_ms =
                            hotkey_release_wait_started_at.elapsed().as_millis();
                        if !modifiers_released {
                            tracing::warn!(
                                waited_ms = hotkey_release_wait_elapsed_ms,
                                fast_hotkey = %runtime.config.hotkeys.voice_input,
                                streaming_effective_hotkey = "Ctrl",
                                "streaming output started before all modifiers fully released"
                            );
                        }

                        let output_config = OutputConfig {
                            prefer_direct_paste: runtime.config.voice.prefer_direct_paste,
                            fallback_to_clipboard: runtime.config.voice.fallback_to_clipboard,
                            voice_hotkey_uses_alt: hotkey::voice_hotkey_uses_alt(),
                            paste_stabilize_delay: STREAMING_PASTE_STABILIZE_DELAY,
                            allow_native_edit: true,
                            restore_clipboard_after_paste: false,
                            defer_clipboard_restore: false,
                            preserve_text_exactly: true,
                        };

                        let output_started_at = Instant::now();
                        let output_adapter = ClipboardStreamingOutputAdapter {
                            controller: &runtime.output_controller,
                        };
                        let delivery =
                            match output_adapter.commit_text(&hud_commit_text, &output_config) {
                                Ok(delivery) => {
                                    if matches!(delivery, OutputDelivery::ClipboardOnly) {
                                        let _ = proxy.send_event(AppEvent::Worker(
                                            WorkerEvent::StreamingClipboardFallback(
                                                hud_commit_text.clone(),
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
                            .set_last_voice_text(hud_commit_text.clone());
                        runtime.maintenance.persist_voice_result(VoiceHistoryEntry {
                            timestamp: current_timestamp(),
                            delivery_label: streaming_delivery_label(delivery),
                            text: hud_commit_text.clone(),
                        });
                        let pipeline_elapsed_ms = session.started_at.elapsed().as_millis();
                        let realtime_factor = if captured_audio_duration_ms > 0 {
                            pipeline_elapsed_ms as f64 / captured_audio_duration_ms as f64
                        } else {
                            0.0
                        };
                        tracing::info!(
                            ?delivery,
                            text = %hud_commit_text,
                            commit_text = %commit_text,
                            hud_ack_elapsed_ms = hud_ack.elapsed_ms,
                            hud_ack_visible = hud_ack.visible,
                            session_id = %commit_envelope.session_id,
                            revision = commit_envelope.revision,
                            audio_duration_ms = captured_audio_duration_ms,
                            first_partial_elapsed_ms,
                            release_tail_elapsed_ms = release_drain.grace_wait_elapsed_ms,
                            release_tail_timeout_fallback = release_drain.timeout_fallback,
                            hotkey_release_wait_elapsed_ms,
                            final_decode_elapsed_ms = prepared_commit.final_decode_elapsed_ms,
                            online_final_elapsed_ms = prepared_commit.online_final_elapsed_ms,
                            offline_final_elapsed_ms = prepared_commit.offline_final_elapsed_ms,
                            offline_final_timed_out = prepared_commit.offline_final_timed_out,
                            punctuation_elapsed_ms = prepared_commit.punctuation_elapsed_ms,
                            hud_final_flush_elapsed_ms,
                            post_hud_flush_mutation_count = session.post_hud_flush_mutation_count,
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
                            release_drain.grace_wait_elapsed_ms,
                            prepared_commit.offline_final_elapsed_ms,
                            prepared_commit.offline_final_timed_out,
                            prepared_commit.punctuation_elapsed_ms,
                            release_to_commit_elapsed_ms,
                        );

                        let drained_commands = drain_pending_voice_commands(&worker_rx);
                        if drained_commands > 0 {
                            tracing::warn!(
                                drained_commands,
                                "dropped queued voice hotkey commands after streaming commit"
                            );
                        }

                        let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingFinal(
                            hud_commit_text,
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
                let soft_flushed = if let Some(update) = maybe_soft_flush_streaming_tail_core(
                    &runtime,
                    &recognizer,
                    punctuator.as_ref(),
                    session,
                    runtime.config.voice.streaming.rewrite_enabled,
                    audio_duration_ms,
                ) {
                    tracing::debug!(
                        samples = session.captured_samples.len(),
                        audio_duration_ms,
                        decode_steps = session.total_decode_steps,
                        total_chunks_fed = session.total_chunks_fed,
                        selected_preview_source = update.source,
                        stable_chars = update.stable_chars,
                        frozen_chars = update.frozen_chars,
                        volatile_chars = update.volatile_chars,
                        revision = update.revision,
                        raw_text = %update.raw_text,
                        prepared_text = %update.display_text,
                        "streaming HUD soft flush updated"
                    );
                    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
                        raw_text: update.raw_text,
                        prepared_text: update.display_text,
                    }));
                    true
                } else {
                    false
                };

                if !soft_flushed
                    && let Some(committed_text) = maybe_rollover_streaming_segment_core(
                        &runtime,
                        &recognizer,
                        punctuator.as_ref(),
                        session,
                        runtime.config.voice.streaming.rewrite_enabled,
                        audio_duration_ms,
                    )
                {
                    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
                        raw_text: committed_text.clone(),
                        prepared_text: committed_text,
                    }));
                }
            }

            let level = normalize_audio_level(recording.current_level());
            let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::Meter(level)));
        }
    }
}

fn drain_pending_voice_commands(worker_rx: &mpsc::Receiver<WorkerCommand>) -> usize {
    let mut drained = 0usize;
    while worker_rx.try_recv().is_ok() {
        drained += 1;
    }
    drained
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
                runtime,
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
    let offline_final_recognizer = build_streaming_final_recognizer(runtime).ok();
    let prepared_commit = prepare_final_streaming_commit(
        recognizer,
        offline_final_recognizer.as_ref(),
        punctuator.as_ref(),
        &mut session,
        sample_rate_hz,
        chunk_samples,
        streaming_endpoint_tail_padding_ms(&runtime.config.voice.streaming.endpoint),
        runtime.config.voice.streaming.rewrite_enabled,
    );
    let final_text = ensure_terminal_sentence_boundary(&prepared_commit.final_text);
    let recorded = recording.stop()?;
    let raw_capture = save_streaming_raw_capture(
        runtime
            .runtime_paths
            .logs_dir
            .join("streaming-raw-captures"),
        recorded,
    )?;

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
        final_offline_raw_text: prepared_commit.final_offline_raw_text,
        final_prepared_candidate: prepared_commit.prepared_final_candidate,
        final_text,
        commit_source: prepared_commit.commit_source,
        raw_capture_wav: raw_capture.wav_path.display().to_string(),
        raw_capture_metadata: raw_capture.json_path.display().to_string(),
    })
}

pub(crate) fn replay_streaming_wav(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    case_id: &str,
    wav_path: &Path,
    expected_text: Option<&str>,
    keywords: &[String],
    min_partial_updates: usize,
    min_visible_chars: Option<usize>,
    shortfall_tolerance_chars: usize,
) -> Result<StreamingReplayReport> {
    let punctuator = build_streaming_punctuator(runtime).ok();
    let offline_final_recognizer = build_streaming_final_recognizer(runtime).ok();
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
    let processing_started_at = Instant::now();

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
        if let Some(fast_update) = prepare_fast_streaming_partial(
            recognizer,
            &mut session,
            total_audio_duration_ms,
            streaming_stability_policy(&runtime.config.voice.streaming.stability),
        ) {
            let processing_elapsed_ms = processing_started_at.elapsed().as_millis();
            partial_timeline.push(StreamingReplayPartialEntry {
                offset_ms: total_audio_duration_ms,
                processing_elapsed_ms,
                processing_realtime_factor: streaming_replay_realtime_factor(
                    processing_elapsed_ms,
                    total_audio_duration_ms,
                ),
                raw_text: fast_update.raw_text,
                content_chars: content_chars_without_sentence_punctuation(
                    &fast_update.display_text,
                ),
                prepared_text: fast_update.display_text,
                source: fast_update.source.to_string(),
                stable_chars: fast_update.stable_chars,
                frozen_chars: fast_update.frozen_chars,
                volatile_chars: fast_update.volatile_chars,
                rejected_prefix_rewrite: fast_update.rejected_prefix_rewrite,
            });
        }
        if let Some(update) = update_streaming_partial_state(
            runtime,
            recognizer,
            punctuator.as_ref(),
            &mut session,
            runtime.config.voice.streaming.rewrite_enabled,
            total_audio_duration_ms,
        )? {
            let processing_elapsed_ms = processing_started_at.elapsed().as_millis();
            partial_timeline.push(StreamingReplayPartialEntry {
                offset_ms: total_audio_duration_ms,
                processing_elapsed_ms,
                processing_realtime_factor: streaming_replay_realtime_factor(
                    processing_elapsed_ms,
                    total_audio_duration_ms,
                ),
                raw_text: update.raw_text,
                content_chars: content_chars_without_sentence_punctuation(&update.display_text),
                prepared_text: update.display_text,
                source: update.source.to_string(),
                stable_chars: update.stable_chars,
                frozen_chars: update.frozen_chars,
                volatile_chars: update.volatile_chars,
                rejected_prefix_rewrite: update.rejected_prefix_rewrite,
            });
        }

        let soft_flushed = if let Some(update) = maybe_soft_flush_streaming_tail_core(
            runtime,
            recognizer,
            punctuator.as_ref(),
            &mut session,
            runtime.config.voice.streaming.rewrite_enabled,
            total_audio_duration_ms,
        ) {
            let processing_elapsed_ms = processing_started_at.elapsed().as_millis();
            partial_timeline.push(StreamingReplayPartialEntry {
                offset_ms: total_audio_duration_ms,
                processing_elapsed_ms,
                processing_realtime_factor: streaming_replay_realtime_factor(
                    processing_elapsed_ms,
                    total_audio_duration_ms,
                ),
                raw_text: update.raw_text,
                content_chars: content_chars_without_sentence_punctuation(&update.display_text),
                prepared_text: update.display_text,
                source: update.source.to_string(),
                stable_chars: update.stable_chars,
                frozen_chars: update.frozen_chars,
                volatile_chars: update.volatile_chars,
                rejected_prefix_rewrite: update.rejected_prefix_rewrite,
            });
            true
        } else {
            false
        };

        if !soft_flushed
            && let Some(committed_text) = maybe_rollover_streaming_segment_core(
                runtime,
                recognizer,
                punctuator.as_ref(),
                &mut session,
                runtime.config.voice.streaming.rewrite_enabled,
                total_audio_duration_ms,
            )
        {
            let processing_elapsed_ms = processing_started_at.elapsed().as_millis();
            partial_timeline.push(StreamingReplayPartialEntry {
                offset_ms: total_audio_duration_ms,
                processing_elapsed_ms,
                processing_realtime_factor: streaming_replay_realtime_factor(
                    processing_elapsed_ms,
                    total_audio_duration_ms,
                ),
                raw_text: committed_text.clone(),
                content_chars: content_chars_without_sentence_punctuation(&committed_text),
                prepared_text: committed_text,
                source: "endpoint_rollover".to_string(),
                stable_chars: session.last_display_text.chars().count(),
                frozen_chars: session.state.committed_prefix.chars().count(),
                volatile_chars: session.state.current_tail().chars().count(),
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
        offline_final_recognizer.as_ref(),
        punctuator.as_ref(),
        &mut session,
        runner_sample_rate_hz,
        stream_chunk_num_samples,
        streaming_endpoint_tail_padding_ms(&runtime.config.voice.streaming.endpoint),
        runtime.config.voice.streaming.rewrite_enabled,
    );
    let processing_wall_elapsed_ms = processing_started_at.elapsed().as_millis();
    let input_duration_ms = audio_duration_ms(input_sample_rate_hz, session.ingested_input_samples);
    let processing_realtime_factor = if input_duration_ms > 0 {
        processing_wall_elapsed_ms as f64 / input_duration_ms as f64
    } else {
        0.0
    };
    let final_text = ensure_terminal_sentence_boundary(&prepared_commit.final_text);
    let final_visible_chars = visible_text_char_count(&final_text);
    if should_append_release_final_preview(&partial_timeline, &final_text) {
        let final_content_chars = content_chars_without_sentence_punctuation(&final_text);
        partial_timeline.push(StreamingReplayPartialEntry {
            offset_ms: input_duration_ms,
            processing_elapsed_ms: processing_wall_elapsed_ms,
            processing_realtime_factor,
            raw_text: prepared_commit.final_online_raw_text.clone(),
            content_chars: final_content_chars,
            prepared_text: final_text.clone(),
            source: "release_final_preview".to_string(),
            stable_chars: final_content_chars,
            frozen_chars: session.state.committed_prefix.chars().count(),
            volatile_chars: 0,
            rejected_prefix_rewrite: false,
        });
        session.last_display_text = final_text.clone();
    }
    let last_partial_content_chars = partial_timeline
        .last()
        .map(|entry| entry.content_chars)
        .unwrap_or(0);
    let final_content_chars = content_chars_without_sentence_punctuation(&final_text);
    let final_extra_content_chars = final_content_chars.saturating_sub(last_partial_content_chars);
    let final_missing_content_chars =
        last_partial_content_chars.saturating_sub(final_content_chars);
    let last_partial_to_final_gap_ms = partial_timeline
        .last()
        .map(|entry| input_duration_ms.saturating_sub(entry.offset_ms));
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
    failures.extend(streaming_final_quality_failures(&final_text));

    let behavior_status = if failures.is_empty() {
        StreamingCaseStatus::Pass
    } else {
        StreamingCaseStatus::FailBehavior
    };
    let (rollback_count, max_rollback_chars) =
        summarize_streaming_timeline_rollbacks(&partial_timeline);

    let mut content_failures = Vec::new();
    let matched_keywords = matched_replay_keywords(&final_text, keywords);
    let keyword_coverage =
        (!keywords.is_empty()).then_some(matched_keywords.len() as f32 / keywords.len() as f32);
    let exact_content_match = expected_text
        .as_deref()
        .map(|expected_text| {
            normalize_replay_text(&final_text) == normalize_replay_text(expected_text)
        })
        .unwrap_or(true);
    let keyword_content_match = !keywords.is_empty() && matched_keywords.len() == keywords.len();
    if !exact_content_match && !keyword_content_match {
        let expected_text = expected_text.as_deref().unwrap_or("");
        content_failures.push(format!(
            "final_text_mismatch expected='{}' actual='{}' matched_keywords={}/{}",
            expected_text,
            final_text,
            matched_keywords.len(),
            keywords.len()
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
        input_duration_ms,
        captured_samples: session.captured_samples.len(),
        peak_abs: activity.peak_abs,
        rms: activity.rms,
        active_ratio: activity.active_ratio,
        total_chunks_fed: session.total_chunks_fed,
        total_decode_steps: session.total_decode_steps,
        partial_updates: session.partial_updates,
        first_partial_ms: partial_timeline.first().map(|entry| entry.offset_ms),
        final_commit_ms: input_duration_ms,
        processing_wall_elapsed_ms,
        processing_realtime_factor,
        final_decode_elapsed_ms: prepared_commit.final_decode_elapsed_ms,
        online_final_elapsed_ms: prepared_commit.online_final_elapsed_ms,
        offline_final_elapsed_ms: prepared_commit.offline_final_elapsed_ms,
        offline_final_timed_out: prepared_commit.offline_final_timed_out,
        punctuation_elapsed_ms: prepared_commit.punctuation_elapsed_ms,
        rollback_count,
        max_rollback_chars,
        last_partial_content_chars,
        final_extra_content_chars,
        final_missing_content_chars,
        last_partial_to_final_gap_ms,
        partial_timeline,
        last_partial_text: session.last_display_text.clone(),
        final_online_raw_text: prepared_commit.final_online_raw_text,
        final_offline_raw_text: prepared_commit.final_offline_raw_text,
        final_prepared_candidate: prepared_commit.prepared_final_candidate,
        final_text,
        final_visible_chars,
        commit_source: prepared_commit.commit_source.as_str().to_string(),
        expected_text,
        expected_visible_chars,
        keywords: keywords.to_vec(),
        matched_keywords,
        keyword_coverage,
        min_partial_updates,
        shortfall_tolerance_chars,
        behavior_status,
        content_status,
        failures,
    })
}

fn streaming_replay_realtime_factor(processing_elapsed_ms: u128, audio_offset_ms: u64) -> f64 {
    if audio_offset_ms == 0 {
        0.0
    } else {
        processing_elapsed_ms as f64 / audio_offset_ms as f64
    }
}

fn should_append_release_final_preview(
    partial_timeline: &[StreamingReplayPartialEntry],
    final_text: &str,
) -> bool {
    let final_trimmed = final_text.trim();
    if final_trimmed.is_empty() {
        return false;
    }
    partial_timeline
        .last()
        .map(|entry| entry.prepared_text.trim() != final_trimmed)
        .unwrap_or(true)
}

fn matched_replay_keywords(final_text: &str, keywords: &[String]) -> Vec<String> {
    let normalized_final = normalize_replay_text(final_text);
    keywords
        .iter()
        .filter(|keyword| {
            let normalized_keyword = normalize_replay_text(keyword);
            !normalized_keyword.is_empty() && normalized_final.contains(&normalized_keyword)
        })
        .cloned()
        .collect()
}

fn summarize_streaming_timeline_rollbacks(
    partial_timeline: &[StreamingReplayPartialEntry],
) -> (usize, usize) {
    let mut rollback_count = partial_timeline
        .iter()
        .filter(|entry| entry.rejected_prefix_rewrite)
        .count();
    let mut max_rollback_chars = 0usize;

    for pair in partial_timeline.windows(2) {
        let previous = pair[0].prepared_text.trim();
        let current = pair[1].prepared_text.trim();
        if previous.is_empty() || current.is_empty() {
            continue;
        }

        let previous_chars = previous.chars().count();
        let common_chars = longest_common_prefix_chars(previous, current);
        let rollback_chars = previous_chars.saturating_sub(common_chars);
        if rollback_chars > 0 && common_chars < previous_chars {
            rollback_count += 1;
            max_rollback_chars = max_rollback_chars.max(rollback_chars);
        }
    }

    (rollback_count, max_rollback_chars)
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
            &case.keywords,
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

fn build_streaming_final_recognizer(
    runtime: &AppRuntime,
) -> Result<ainput_asr::SenseVoiceRecognizer> {
    ainput_asr::SenseVoiceRecognizer::create(&ainput_asr::SenseVoiceConfig {
        model_dir: runtime
            .runtime_paths
            .root_dir
            .join(&runtime.config.asr.model_dir),
        provider: runtime.config.asr.provider.clone(),
        sample_rate_hz: runtime.config.asr.sample_rate_hz as i32,
        language: runtime.config.asr.language.clone(),
        use_itn: runtime.config.asr.use_itn,
        num_threads: effective_streaming_final_num_threads(runtime),
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
        num_threads: effective_streaming_asr_num_threads(runtime),
        decoding_method: "greedy_search".to_string(),
        enable_endpoint: false,
        rule1_min_trailing_silence: STREAMING_SHERPA_FALLBACK_TRAILING_SILENCE_SECS,
        rule2_min_trailing_silence: STREAMING_SHERPA_FALLBACK_TRAILING_SILENCE_SECS,
        rule3_min_utterance_length: STREAMING_SHERPA_FALLBACK_MAX_UTTERANCE_SECS,
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
        num_threads: effective_streaming_punctuation_num_threads(runtime),
    })
}

fn effective_streaming_asr_num_threads(runtime: &AppRuntime) -> i32 {
    let configured = runtime.config.voice.streaming.performance.asr_num_threads;
    if configured <= 0 {
        runtime.config.asr.num_threads.max(1)
    } else {
        configured.clamp(1, 12)
    }
}

fn effective_streaming_final_num_threads(runtime: &AppRuntime) -> i32 {
    let configured = runtime.config.voice.streaming.performance.final_num_threads;
    if configured <= 0 {
        runtime.config.asr.num_threads.max(1)
    } else {
        configured.clamp(1, 16)
    }
}

fn effective_streaming_punctuation_num_threads(runtime: &AppRuntime) -> i32 {
    let configured = runtime
        .config
        .voice
        .streaming
        .performance
        .punctuation_num_threads;
    if configured <= 0 {
        runtime
            .config
            .voice
            .streaming
            .punctuation_num_threads
            .max(1)
    } else {
        configured.clamp(1, 4)
    }
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
    raw_capture_dir: PathBuf,
    finalize_config: &ainput_shell::StreamingFinalizeConfig,
) -> Result<StreamingReleaseDrainStats> {
    let grace_started_at = Instant::now();
    let max_wait_ms = streaming_release_drain_max_ms(finalize_config);
    let min_wait_ms = streaming_release_drain_min_ms(finalize_config);
    let idle_settle_ms = streaming_release_drain_idle_settle_ms(finalize_config);
    let deadline = grace_started_at + Duration::from_millis(max_wait_ms);
    let min_wait = Duration::from_millis(min_wait_ms);
    let idle_settle = Duration::from_millis(idle_settle_ms);
    let poll_interval = Duration::from_millis(STREAMING_RELEASE_POLL_INTERVAL_MS);

    let mut grace_added_samples = 0usize;
    let mut last_voice_at: Option<Instant> = None;
    let mut voice_active_observations = 0usize;
    let mut timeout_fallback = false;
    tracing::info!(
        max_wait_ms,
        min_wait_ms,
        idle_settle_ms,
        "streaming release tail drain started"
    );

    loop {
        let added = collect_streaming_audio_chunk(session, &recording);
        if added > 0 {
            grace_added_samples += added;
        }

        let activity =
            analyze_recent_audio_activity(&session.captured_samples, session.sample_rate_hz, 180);
        if is_streaming_endpoint_voice_active(&activity) {
            last_voice_at = Some(Instant::now());
            voice_active_observations += 1;
        }

        let now = Instant::now();
        if now >= deadline {
            timeout_fallback = true;
            break;
        }

        let waited = now.saturating_duration_since(grace_started_at);
        if waited >= min_wait {
            match last_voice_at {
                Some(instant) if now.saturating_duration_since(instant) >= idle_settle => break,
                None if waited >= idle_settle => break,
                _ => {}
            }
        }

        std::thread::sleep(poll_interval);
    }

    let recorded = recording.stop()?;
    let stop_added_samples = collect_stopped_recording_tail(session, &recorded);
    let recorded_total_samples = recorded.samples.len();
    save_streaming_raw_capture_async(raw_capture_dir, recorded);
    let grace_wait_elapsed_ms = grace_started_at.elapsed().as_millis();
    tracing::info!(
        grace_added_samples,
        stop_added_samples,
        grace_wait_elapsed_ms,
        voice_active_observations,
        timeout_fallback = ?timeout_fallback,
        recorded_total_samples,
        "streaming release tail drain finished"
    );

    Ok(StreamingReleaseDrainStats {
        grace_added_samples,
        stop_added_samples,
        grace_wait_elapsed_ms,
        voice_active_observations,
        timeout_fallback,
    })
}

fn streaming_release_drain_min_ms(config: &ainput_shell::StreamingFinalizeConfig) -> u64 {
    if config.release_drain_min_ms == 0 {
        STREAMING_RELEASE_MIN_WAIT_MS
    } else {
        config.release_drain_min_ms.clamp(80, 300)
    }
}

fn streaming_release_drain_idle_settle_ms(config: &ainput_shell::StreamingFinalizeConfig) -> u64 {
    if config.release_drain_idle_settle_ms == 0 {
        STREAMING_RELEASE_IDLE_SETTLE_MS
    } else {
        config.release_drain_idle_settle_ms.clamp(80, 320)
    }
}

fn streaming_release_drain_max_ms(config: &ainput_shell::StreamingFinalizeConfig) -> u64 {
    if config.release_drain_max_ms == 0 {
        STREAMING_RELEASE_MAX_WAIT_MS
    } else {
        config.release_drain_max_ms.clamp(
            STREAMING_RELEASE_MIN_WAIT_MS,
            STREAMING_RELEASE_HARD_WAIT_MS,
        )
    }
}

fn streaming_chunk_num_samples(sample_rate_hz: i32, chunk_ms: u32) -> usize {
    let effective_sample_rate = sample_rate_hz.max(1) as usize;
    let effective_chunk_ms = chunk_ms.clamp(60, 500) as usize;
    ((effective_sample_rate * effective_chunk_ms) / 1000).max(effective_sample_rate / 20)
}

fn streaming_endpoint_preroll_ms(config: &ainput_shell::StreamingEndpointConfig) -> u64 {
    if config.preroll_ms == 0 {
        STREAMING_DEFAULT_PREROLL_MS
    } else {
        config.preroll_ms.min(1_000)
    }
}

fn streaming_endpoint_tail_padding_ms(config: &ainput_shell::StreamingEndpointConfig) -> u64 {
    if config.tail_padding_ms == 0 {
        STREAMING_DEFAULT_TAIL_PADDING_MS
    } else {
        config.tail_padding_ms.clamp(80, 1_000)
    }
}

fn streaming_endpoint_soft_flush_ms(config: &ainput_shell::StreamingEndpointConfig) -> u64 {
    if config.soft_flush_ms == 0 {
        STREAMING_HUD_SOFT_FLUSH_MS
    } else {
        config.soft_flush_ms.clamp(200, 1_200)
    }
}

fn streaming_idle_finalize_tail_padding_ms(config: &ainput_shell::StreamingEndpointConfig) -> u64 {
    if config.tail_padding_ms == 0 {
        STREAMING_IDLE_FINALIZE_TAIL_PADDING_MS
    } else {
        config.tail_padding_ms.clamp(120, 800)
    }
}

fn streaming_soft_flush_tail_padding_ms(config: &ainput_shell::StreamingEndpointConfig) -> u64 {
    if config.tail_padding_ms == 0 {
        STREAMING_HUD_SOFT_FLUSH_TAIL_PADDING_MS
    } else {
        config
            .tail_padding_ms
            .clamp(120, STREAMING_HUD_SOFT_FLUSH_TAIL_PADDING_MS)
    }
}

fn streaming_stability_policy(
    config: &ainput_shell::StreamingStabilityConfig,
) -> StreamingStabilityPolicy {
    StreamingStabilityPolicy {
        min_agreement: config.min_agreement,
        max_rollback_chars: config.max_rollback_chars,
    }
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
    tail_padding_ms: u64,
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
        ((sample_rate_hz.max(1) as usize) * tail_padding_ms as usize / 1000).max(1);
    let tail_padding = vec![0.0f32; tail_padding_num_samples];
    recognizer.accept_waveform(&session.stream, sample_rate_hz, &tail_padding);
    decode_steps += recognizer.decode_available(&session.stream);
    recognizer.input_finished(&session.stream);
    decode_steps += recognizer.decode_available(&session.stream);
    session.total_decode_steps += decode_steps;
    decode_steps
}

fn finalize_streaming_pause_boundary_decode(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    session: &mut StreamingCoreSession,
    sample_rate_hz: i32,
    chunk_num_samples: usize,
    tail_padding_ms: u64,
) -> usize {
    let _ = feed_streaming_pending_chunks(
        recognizer,
        session,
        sample_rate_hz,
        chunk_num_samples,
        false,
    );

    let mut decode_steps = 0usize;
    let tail_padding_num_samples =
        ((sample_rate_hz.max(1) as usize) * tail_padding_ms as usize / 1000).max(1);
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
    final_offline_raw_text: String,
    prepared_final_candidate: String,
    display_text_before_final: String,
    candidate_display_text: String,
    final_text: String,
    commit_source: StreamingCommitSource,
    final_decode_steps: usize,
    final_decode_elapsed_ms: u128,
    online_final_elapsed_ms: u128,
    offline_final_elapsed_ms: u128,
    offline_final_timed_out: bool,
    punctuation_elapsed_ms: u128,
    rejected_prefix_rewrite: bool,
}

#[derive(Debug, Clone, Copy)]
struct StreamingReleaseDrainStats {
    grace_added_samples: usize,
    stop_added_samples: usize,
    grace_wait_elapsed_ms: u128,
    voice_active_observations: usize,
    timeout_fallback: bool,
}

#[derive(Debug, Clone)]
struct StreamingCommitEnvelope {
    session_id: String,
    revision: u64,
    last_hud_target_text: String,
    final_online_raw_text: String,
    final_offline_raw_text: String,
    final_candidate_text: String,
    candidate_display_text: String,
    resolved_commit_text: String,
    commit_source: StreamingCommitSource,
    online_final_elapsed_ms: u128,
    offline_final_elapsed_ms: u128,
    offline_final_timed_out: bool,
    punctuation_elapsed_ms: u128,
}

impl StreamingCommitEnvelope {
    fn from_prepared(session: &StreamingCoreSession, prepared: &PreparedStreamingCommit) -> Self {
        Self {
            session_id: session.session_id.clone(),
            revision: session.state.revision,
            last_hud_target_text: prepared.display_text_before_final.clone(),
            final_online_raw_text: prepared.final_online_raw_text.clone(),
            final_offline_raw_text: prepared.final_offline_raw_text.clone(),
            final_candidate_text: prepared.prepared_final_candidate.clone(),
            candidate_display_text: prepared.candidate_display_text.clone(),
            resolved_commit_text: prepared.final_text.clone(),
            commit_source: prepared.commit_source,
            online_final_elapsed_ms: prepared.online_final_elapsed_ms,
            offline_final_elapsed_ms: prepared.offline_final_elapsed_ms,
            offline_final_timed_out: prepared.offline_final_timed_out,
            punctuation_elapsed_ms: prepared.punctuation_elapsed_ms,
        }
    }
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
    revision: u64,
    rejected_prefix_rewrite: bool,
}

#[derive(Debug, Clone)]
struct StreamingAiRewriteCandidate {
    frozen_prefix: String,
    current_tail: String,
    revision: u64,
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
    revision: u64,
}

fn prepare_final_streaming_commit(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    offline_final_recognizer: Option<&ainput_asr::SenseVoiceRecognizer>,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    session: &mut StreamingCoreSession,
    sample_rate_hz: i32,
    chunk_num_samples: usize,
    tail_padding_ms: u64,
    rewrite_enabled: bool,
) -> PreparedStreamingCommit {
    let online_final_started_at = Instant::now();
    let final_decode_steps = finalize_streaming_decode(
        recognizer,
        session,
        sample_rate_hz,
        chunk_num_samples,
        tail_padding_ms,
    );
    let online_final_elapsed_ms = online_final_started_at.elapsed().as_millis();
    let final_decode_elapsed_ms = online_final_elapsed_ms;
    let final_online_raw_text = recognizer
        .get_result(&session.stream)
        .map(|result| result.text.trim().to_string())
        .unwrap_or_default();
    let offline_final = transcribe_streaming_offline_final(
        offline_final_recognizer,
        session.sample_rate_hz,
        &session.captured_samples,
    );
    let offline_final_elapsed_ms = offline_final.elapsed_ms;
    let offline_final_timed_out = offline_final_elapsed_ms > STREAMING_OFFLINE_FINAL_HARD_MS;
    let final_offline_raw_text = if offline_final_timed_out {
        tracing::warn!(
            offline_final_elapsed_ms,
            hard_ms = STREAMING_OFFLINE_FINAL_HARD_MS,
            scope = offline_final.scope.as_str(),
            "streaming offline final repair exceeded hard budget; ignoring repair text"
        );
        String::new()
    } else {
        offline_final.text
    };
    let display_text_before_final = effective_streaming_display_text(session);
    let (selected_final_raw_text, selected_final_raw_source) = select_streaming_final_raw_text(
        &final_online_raw_text,
        &final_offline_raw_text,
        offline_final.scope,
        &display_text_before_final,
    );
    let punctuation_started_at = Instant::now();
    let (prepared_final_candidate, prepared_candidate_source) =
        prepare_streaming_output_text(&selected_final_raw_text, rewrite_enabled, punctuator);
    let punctuation_elapsed_ms = punctuation_started_at.elapsed().as_millis();
    let candidate_source = match selected_final_raw_source {
        StreamingCommitSource::OfflineFinal => StreamingCommitSource::OfflineFinal,
        _ => prepared_candidate_source,
    };
    let resolved_commit = resolve_final_streaming_commit(
        session,
        &display_text_before_final,
        &prepared_final_candidate,
        candidate_source,
    );

    PreparedStreamingCommit {
        final_online_raw_text,
        final_offline_raw_text,
        prepared_final_candidate,
        display_text_before_final,
        candidate_display_text: resolved_commit.candidate_display_text,
        final_text: resolved_commit.final_text,
        commit_source: resolved_commit.commit_source,
        final_decode_steps,
        final_decode_elapsed_ms,
        online_final_elapsed_ms,
        offline_final_elapsed_ms,
        offline_final_timed_out,
        punctuation_elapsed_ms,
        rejected_prefix_rewrite: resolved_commit.rejected_prefix_rewrite,
    }
}

#[derive(Debug, Clone)]
struct StreamingOfflineFinalResult {
    text: String,
    elapsed_ms: u128,
    scope: StreamingOfflineFinalScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamingOfflineFinalScope {
    None,
    FullAudio,
    TailWindow,
}

impl StreamingOfflineFinalScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FullAudio => "full_audio",
            Self::TailWindow => "tail_window",
        }
    }
}

fn transcribe_streaming_offline_final(
    offline_final_recognizer: Option<&ainput_asr::SenseVoiceRecognizer>,
    sample_rate_hz: i32,
    samples: &[f32],
) -> StreamingOfflineFinalResult {
    let started_at = Instant::now();
    let Some(recognizer) = offline_final_recognizer else {
        return StreamingOfflineFinalResult {
            text: String::new(),
            elapsed_ms: started_at.elapsed().as_millis(),
            scope: StreamingOfflineFinalScope::None,
        };
    };
    if sample_rate_hz <= 0 || samples.is_empty() {
        return StreamingOfflineFinalResult {
            text: String::new(),
            elapsed_ms: started_at.elapsed().as_millis(),
            scope: StreamingOfflineFinalScope::None,
        };
    }

    let scope = streaming_offline_final_scope(sample_rate_hz, samples.len());
    let sample_start = streaming_offline_final_sample_start(sample_rate_hz, samples.len(), scope);
    let repair_samples = &samples[sample_start..];
    let text = match recognizer.transcribe_samples(
        sample_rate_hz,
        repair_samples,
        "streaming-final-repair",
    ) {
        Ok(transcription) => transcription.text.trim().to_string(),
        Err(error) => {
            tracing::warn!(
                error = %error,
                scope = scope.as_str(),
                "streaming offline final repair failed; keeping streaming final"
            );
            String::new()
        }
    };
    let elapsed_ms = started_at.elapsed().as_millis();
    tracing::debug!(
        scope = scope.as_str(),
        input_audio_ms = audio_duration_ms(sample_rate_hz, samples.len()),
        repair_audio_ms = audio_duration_ms(sample_rate_hz, repair_samples.len()),
        elapsed_ms,
        "streaming offline final repair finished"
    );
    StreamingOfflineFinalResult {
        text,
        elapsed_ms,
        scope,
    }
}

fn streaming_offline_final_scope(
    sample_rate_hz: i32,
    sample_count: usize,
) -> StreamingOfflineFinalScope {
    if sample_rate_hz <= 0 || sample_count == 0 {
        return StreamingOfflineFinalScope::None;
    }
    let duration_ms = audio_duration_ms(sample_rate_hz, sample_count);
    if duration_ms > STREAMING_OFFLINE_FINAL_FULL_AUDIO_MAX_MS {
        StreamingOfflineFinalScope::TailWindow
    } else {
        StreamingOfflineFinalScope::FullAudio
    }
}

fn streaming_offline_final_sample_start(
    sample_rate_hz: i32,
    sample_count: usize,
    scope: StreamingOfflineFinalScope,
) -> usize {
    if !matches!(scope, StreamingOfflineFinalScope::TailWindow) {
        return 0;
    }
    let tail_samples = sample_count_for_ms(sample_rate_hz, STREAMING_OFFLINE_FINAL_TAIL_WINDOW_MS);
    sample_count.saturating_sub(tail_samples.max(1))
}

fn select_streaming_final_raw_text(
    online_raw_text: &str,
    offline_raw_text: &str,
    offline_scope: StreamingOfflineFinalScope,
    display_text_before_final: &str,
) -> (String, StreamingCommitSource) {
    let online = online_raw_text.trim();
    let offline = offline_raw_text.trim();
    if offline.is_empty() {
        return (online.to_string(), StreamingCommitSource::OnlineFinal);
    }
    if online.is_empty() {
        if let Some(repaired_tail) =
            repair_offline_short_english_tail_artifact(display_text_before_final, offline)
        {
            return (repaired_tail, StreamingCommitSource::OfflineFinal);
        }
        if should_reject_offline_short_english_tail_artifact(display_text_before_final, offline) {
            return (String::new(), StreamingCommitSource::OnlineFinal);
        }
        return (offline.to_string(), StreamingCommitSource::OfflineFinal);
    }

    if let Some(repaired_tail) =
        repair_offline_short_english_tail_artifact(display_text_before_final, offline)
    {
        return (repaired_tail, StreamingCommitSource::OfflineFinal);
    }
    if should_reject_offline_short_english_tail_artifact(display_text_before_final, offline) {
        return (online.to_string(), StreamingCommitSource::OnlineFinal);
    }

    if matches!(offline_scope, StreamingOfflineFinalScope::TailWindow) {
        if let Some(repaired) = merge_streaming_offline_tail_repair(online, offline) {
            return (repaired, StreamingCommitSource::OfflineFinal);
        }
        return (online.to_string(), StreamingCommitSource::OnlineFinal);
    }

    let online_content = content_text_without_sentence_punctuation(online);
    let offline_content = content_text_without_sentence_punctuation(offline);
    let online_chars = online_content.chars().count();
    let offline_chars = offline_content.chars().count();
    if offline_chars > online_chars
        && (offline_content.starts_with(&online_content)
            || longest_common_prefix_chars(&offline_content, &online_content)
                >= online_chars.saturating_sub(1).max(4))
    {
        return (offline.to_string(), StreamingCommitSource::OfflineFinal);
    }

    (online.to_string(), StreamingCommitSource::OnlineFinal)
}

fn repair_offline_short_english_tail_artifact(
    display_text_before_final: &str,
    offline_raw_text: &str,
) -> Option<String> {
    if !is_single_i_artifact_text(offline_raw_text) {
        return None;
    }
    let display_content = content_text_without_sentence_punctuation(display_text_before_final);
    display_content.ends_with('不').then(|| "对。".to_string())
}

fn should_reject_offline_short_english_tail_artifact(
    display_text_before_final: &str,
    offline_raw_text: &str,
) -> bool {
    if !contains_cjk_char(display_text_before_final) {
        return false;
    }
    is_short_english_tail_artifact_text(offline_raw_text)
}

fn is_single_i_artifact_text(text: &str) -> bool {
    normalized_ascii_letters(text) == "i"
}

fn is_short_english_tail_artifact_text(text: &str) -> bool {
    matches!(
        normalized_ascii_letters(text).as_str(),
        "i" | "yeah"
            | "yea"
            | "yes"
            | "yep"
            | "ok"
            | "okay"
            | "uh"
            | "um"
            | "hmm"
            | "hm"
            | "ah"
            | "oh"
            | "hey"
            | "hi"
    )
}

fn normalized_ascii_letters(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn streaming_final_quality_failures(final_text: &str) -> Vec<String> {
    let mut failures = Vec::new();
    if has_isolated_i_tail_artifact(final_text) {
        failures.push(format!(
            "final_text_contains_isolated_i_tail_artifact: {final_text}"
        ));
    }
    if final_text.contains("标点，符号") {
        failures.push(format!(
            "final_text_splits_fixed_word_punctuation: {final_text}"
        ));
    }
    failures
}

fn has_isolated_i_tail_artifact(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch != 'I' || index == 0 {
            continue;
        }
        if chars[index - 1].is_ascii_alphanumeric() {
            continue;
        }
        if !chars[..index]
            .iter()
            .rev()
            .take(8)
            .any(|ch| is_cjk_char(*ch))
        {
            continue;
        }
        let mut cursor = index + 1;
        while cursor < chars.len() && chars[cursor].is_whitespace() {
            cursor += 1;
        }
        if cursor >= chars.len() {
            return true;
        }
        if is_sentence_punctuation(chars[cursor]) {
            let rest_is_boundary = chars[cursor + 1..]
                .iter()
                .all(|ch| ch.is_whitespace() || is_sentence_punctuation(*ch));
            if rest_is_boundary {
                return true;
            }
        }
    }
    false
}

fn merge_streaming_offline_tail_repair(
    online_raw_text: &str,
    offline_tail_text: &str,
) -> Option<String> {
    let online = online_raw_text.trim();
    let tail = offline_tail_text.trim();
    if online.is_empty() || tail.is_empty() {
        return None;
    }

    let online_content = content_text_without_sentence_punctuation(online);
    let tail_content = content_text_without_sentence_punctuation(tail);
    if online_content.is_empty() || tail_content.is_empty() {
        return None;
    }
    if online_content.contains(&tail_content) {
        return None;
    }

    let raw_overlap = longest_suffix_prefix_overlap_chars(online, tail, 12);
    let content_overlap = longest_suffix_prefix_overlap_chars(&online_content, &tail_content, 12);
    let overlap = raw_overlap.max(content_overlap);
    if overlap < 2 {
        return None;
    }

    let tail_content_chars = tail_content.chars().count();
    if tail_content_chars <= overlap {
        return None;
    }

    Some(append_with_suffix_prefix_overlap(online, tail))
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
    let selected_commit_text = selected_commit.text.trim();
    let final_text = finalize_streaming_commit_text(selected_commit_text);
    let commit_source = if final_text != selected_commit_text {
        StreamingCommitSource::StreamingTailRepair
    } else if final_text == display_trimmed {
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

fn finalize_streaming_commit_text(text: &str) -> String {
    let normalized = ainput_rewrite::normalize_transcription(text);
    apply_streaming_semantic_commas(&dedupe_streaming_punctuation(&normalized))
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
            sustained_voice_ms = activity.sustained_voice_ms,
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
            sustained_voice_ms = activity.sustained_voice_ms,
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
        Some(total_audio_duration_ms),
        streaming_stability_policy(&runtime.config.voice.streaming.stability),
    ))
}

fn prepare_fast_streaming_partial(
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    session: &mut StreamingCoreSession,
    total_audio_duration_ms: u64,
    stability_policy: StreamingStabilityPolicy,
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
        Some(total_audio_duration_ms),
        stability_policy,
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
    if let Some(fast_update) = prepare_fast_streaming_partial(
        recognizer,
        session,
        total_audio_duration_ms,
        streaming_stability_policy(&runtime.config.voice.streaming.stability),
    ) {
        tracing::debug!(
            samples = session.captured_samples.len(),
            audio_duration_ms = total_audio_duration_ms,
            decode_steps = session.total_decode_steps,
            total_chunks_fed = session.total_chunks_fed,
            selected_preview_source = fast_update.source,
            stable_chars = fast_update.stable_chars,
            frozen_chars = fast_update.frozen_chars,
            volatile_chars = fast_update.volatile_chars,
            revision = fast_update.revision,
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
        revision = update.revision,
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
    if let Some(update) = poll_streaming_ai_rewrite_result(session, config) {
        tracing::info!(
            request_key = %short_log_text(&update.request_key, 120),
            revision = update.revision,
            display_text = %short_log_text(&update.display_text, 120),
            "streaming AI rewrite result adopted into current preview"
        );
        return Ok(update.display_text);
    }
    let trimmed_preview = selected_preview_text.trim();
    if trimmed_preview.is_empty() {
        tracing::info!("streaming AI rewrite skipped because preview text is empty");
        return Ok(trimmed_preview.to_string());
    }

    let effective_min_visible_chars =
        effective_ai_rewrite_min_visible_chars(config.min_visible_chars);
    let Some(candidate) = build_streaming_ai_rewrite_candidate(
        selected_preview_text,
        effective_min_visible_chars,
        session.state.revision,
    ) else {
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
        revision = candidate.revision,
        frozen_prefix = %short_log_text(&candidate.frozen_prefix, 120),
        current_tail = %short_log_text(&candidate.current_tail, 120),
        process_name = context.process_name.as_deref().unwrap_or("unknown"),
        window_title = context.window_title.as_deref().unwrap_or("unknown"),
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
            session.last_ai_rewrite_output.clear();

            if outcome.candidate.revision != session.state.revision {
                tracing::info!(
                    request_key = %short_log_text(&outcome.request_key, 120),
                    request_revision = outcome.candidate.revision,
                    current_revision = session.state.revision,
                    "streaming AI rewrite result dropped because state revision moved on"
                );
                return None;
            }

            session.last_ai_rewrite_input = outcome.request_key;

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
                    revision: outcome.candidate.revision,
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
            streaming_stability_policy(&runtime.config.voice.streaming.stability),
        );
    }

    let wait_started_at = Instant::now();
    let wait_budget = Duration::from_millis(STREAMING_FINAL_AI_REWRITE_WAIT_MS);
    let config = &runtime.config.voice.streaming.ai_rewrite;
    let mut received_result = false;

    while wait_started_at.elapsed() < wait_budget {
        if apply_ready_streaming_ai_rewrite_result(
            session,
            config,
            proxy,
            streaming_stability_policy(&runtime.config.voice.streaming.stability),
        ) {
            received_result = true;
            break;
        }
        if session.ai_rewrite_result_rx.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(STREAMING_RELEASE_POLL_INTERVAL_MS));
    }

    if !received_result {
        received_result = apply_ready_streaming_ai_rewrite_result(
            session,
            config,
            proxy,
            streaming_stability_policy(&runtime.config.voice.streaming.stability),
        );
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
    stability_policy: StreamingStabilityPolicy,
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
        stability_policy,
    );
    tracing::info!(
        request_key = %short_log_text(&update.request_key, 120),
        display_text = %short_log_text(&update.display_text, 120),
        "streaming AI rewrite result synced into HUD before final commit"
    );
    true
}

fn request_streaming_final_hud_commit_ack(
    proxy: &EventLoopProxy<AppEvent>,
    final_text: &str,
) -> Result<StreamingHudCommitAck> {
    let expected_text = final_text.trim().to_string();
    if expected_text.is_empty() {
        bail!("final text is empty");
    }

    let (response_tx, response_rx) = mpsc::channel();
    proxy
        .send_event(AppEvent::Worker(
            WorkerEvent::StreamingFinalHudCommitRequest {
                final_text: expected_text.clone(),
                response_tx,
            },
        ))
        .map_err(|_| anyhow!("send HUD final commit request failed because event loop closed"))?;

    let ack = response_rx
        .recv_timeout(Duration::from_millis(STREAMING_HUD_FINAL_ACK_TIMEOUT_MS))
        .with_context(|| {
            format!(
                "HUD final commit ack timed out after {}ms",
                STREAMING_HUD_FINAL_ACK_TIMEOUT_MS
            )
        })?;
    if ack.text.trim() != expected_text {
        bail!(
            "HUD final text mismatch: hud='{}' expected='{}'",
            ack.text,
            expected_text
        );
    }
    if !ack.visible {
        bail!("HUD final text is not visible");
    }

    Ok(ack)
}

fn emit_streaming_partial_override(
    session: &mut StreamingCoreSession,
    raw_text: &str,
    display_text: &str,
    source: &'static str,
    proxy: &EventLoopProxy<AppEvent>,
    stability_policy: StreamingStabilityPolicy,
) {
    let Some(update) = apply_streaming_partial_update(
        session,
        raw_text,
        display_text,
        display_text,
        source,
        None,
        stability_policy,
    ) else {
        return;
    };

    let _ = proxy.send_event(AppEvent::Worker(WorkerEvent::StreamingPartial {
        raw_text: update.raw_text,
        prepared_text: update.display_text,
    }));
}

fn apply_streaming_partial_update(
    session: &mut StreamingCoreSession,
    raw_text: &str,
    candidate_display_text: &str,
    online_prepared_text: &str,
    source: &'static str,
    audio_offset_ms: Option<u64>,
    stability_policy: StreamingStabilityPolicy,
) -> Option<PreparedStreamingPartial> {
    if session.commit_locked {
        session.post_hud_flush_mutation_count += 1;
        tracing::warn!(
            session_id = %session.session_id,
            post_hud_flush_mutation_count = session.post_hud_flush_mutation_count,
            raw_text = %short_log_text(raw_text, 120),
            candidate_display_text = %short_log_text(candidate_display_text, 120),
            "dropped streaming partial because commit envelope is locked"
        );
        return None;
    }

    let update = session
        .state
        .apply_online_partial_with_policy(candidate_display_text, stability_policy)?;
    if update.rejected_prefix_rewrite && session.last_display_text == update.display_text {
        return None;
    }

    session.last_raw_partial = raw_text.to_string();
    session.last_display_text = update.display_text.clone();
    session.last_fast_preview_text = update.display_text.clone();
    if let Some(audio_offset_ms) = audio_offset_ms {
        session.last_partial_audio_ms = Some(audio_offset_ms);
    }
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
        revision: update.revision,
        rejected_prefix_rewrite: update.rejected_prefix_rewrite,
    })
}

fn build_streaming_ai_rewrite_candidate(
    display_text: &str,
    min_visible_chars: usize,
    revision: u64,
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
        revision,
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

fn maybe_soft_flush_streaming_tail_core(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    session: &mut StreamingCoreSession,
    rewrite_enabled: bool,
    total_audio_duration_ms: u64,
) -> Option<PreparedStreamingPartial> {
    let config = &runtime.config.voice.streaming.endpoint;
    if !config.enabled {
        return None;
    }

    let activity =
        analyze_recent_audio_activity(&session.captured_samples, session.sample_rate_hz, 220);
    let voice_active = is_streaming_endpoint_voice_active(&activity);
    session
        .endpoint
        .observe(total_audio_duration_ms, voice_active);

    if session.awaiting_post_rollover_speech {
        if voice_active {
            session.awaiting_post_rollover_speech = false;
        } else {
            return None;
        }
    }
    if voice_active {
        return None;
    }

    let current_display = effective_streaming_display_text(session);
    if visible_text_char_count(&current_display) < STREAMING_HUD_SOFT_FLUSH_MIN_VISIBLE_CHARS {
        return None;
    }

    let segment_elapsed_ms =
        total_audio_duration_ms.saturating_sub(session.endpoint.segment_start_ms);
    let min_segment_ms = config.min_segment_ms.clamp(200, 5_000);
    if segment_elapsed_ms < min_segment_ms {
        return None;
    }

    let soft_flush_ms = streaming_endpoint_soft_flush_ms(config);
    let Some(last_voice_ms) = session.endpoint.last_voice_ms else {
        return None;
    };
    if total_audio_duration_ms.saturating_sub(last_voice_ms) < soft_flush_ms {
        return None;
    }

    let Some(last_partial_audio_ms) = session.last_partial_audio_ms else {
        return None;
    };
    if total_audio_duration_ms.saturating_sub(last_partial_audio_ms) < soft_flush_ms {
        return None;
    }
    if session
        .last_soft_flush_audio_ms
        .is_some_and(|last| total_audio_duration_ms.saturating_sub(last) < soft_flush_ms * 2)
    {
        return None;
    }

    let sample_rate_hz = session.sample_rate_hz;
    let chunk_samples =
        streaming_chunk_num_samples(sample_rate_hz, runtime.config.voice.streaming.chunk_ms);
    let soft_flush_tail_padding_ms =
        streaming_soft_flush_tail_padding_ms(&runtime.config.voice.streaming.endpoint);
    let soft_flush_decode_steps = finalize_streaming_pause_boundary_decode(
        recognizer,
        session,
        sample_rate_hz,
        chunk_samples,
        soft_flush_tail_padding_ms,
    );
    let current_raw_text = recognizer
        .get_result(&session.stream)
        .map(|result| result.text.trim().to_string())
        .unwrap_or_default();
    let resolved_text = resolve_streaming_rollover_commit_text(
        &session.rolled_over_prefix,
        &current_display,
        &current_raw_text,
    );
    let resolved_text = if resolved_text.trim().is_empty() {
        current_display.clone()
    } else {
        resolved_text
    };
    let prepared_text =
        prepare_streaming_pause_boundary_text(&resolved_text, rewrite_enabled, punctuator);
    let before_content_chars = content_chars_without_sentence_punctuation(&current_display);
    let after_content_chars = content_chars_without_sentence_punctuation(&prepared_text);
    if after_content_chars <= before_content_chars {
        recognizer.reset(&session.stream);
        session
            .endpoint
            .reset_after_rollover(total_audio_duration_ms);
        session.rolled_over_prefix = current_display.clone();
        session.awaiting_post_rollover_speech = true;
        session.last_soft_flush_audio_ms = Some(total_audio_duration_ms);
        tracing::debug!(
            audio_duration_ms = total_audio_duration_ms,
            current_display = %short_log_text(&current_display, 120),
            current_raw_text = %short_log_text(&current_raw_text, 120),
            soft_flush_decode_steps,
            "streaming HUD soft flush reset recognizer without content growth"
        );
        return None;
    }

    let update = session.state.rollover_with_display_text(&prepared_text);
    session.rolled_over_prefix = update.display_text.clone();
    session.awaiting_post_rollover_speech = true;
    session
        .endpoint
        .reset_after_rollover(total_audio_duration_ms);
    session.last_raw_partial = current_raw_text.clone();
    session.last_display_text = update.display_text.clone();
    session.last_fast_preview_text = update.display_text.clone();
    session.last_partial_audio_ms = Some(total_audio_duration_ms);
    session.last_soft_flush_audio_ms = Some(total_audio_duration_ms);
    if session.first_partial_at.is_none() && !update.display_text.trim().is_empty() {
        session.first_partial_at = Some(Instant::now());
    }
    session.partial_updates += 1;
    reset_streaming_ai_rewrite_cache(session);
    recognizer.reset(&session.stream);

    tracing::info!(
        audio_duration_ms = total_audio_duration_ms,
        soft_flush_ms,
        soft_flush_tail_padding_ms,
        current_display = %short_log_text(&current_display, 120),
        current_raw_text = %short_log_text(&current_raw_text, 120),
        prepared_text = %short_log_text(&update.display_text, 120),
        before_content_chars,
        after_content_chars,
        soft_flush_decode_steps,
        "streaming HUD soft flush appended tail while hotkey is still held"
    );

    Some(PreparedStreamingPartial {
        raw_text: current_raw_text,
        online_prepared_text: prepared_text,
        display_text: update.display_text,
        source: "idle_soft_flush",
        stable_chars: update.stable_chars,
        frozen_chars: update.frozen_chars,
        volatile_chars: update.volatile_chars,
        revision: update.revision,
        rejected_prefix_rewrite: update.rejected_prefix_rewrite,
    })
}

fn maybe_rollover_streaming_segment_core(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    session: &mut StreamingCoreSession,
    rewrite_enabled: bool,
    total_audio_duration_ms: u64,
) -> Option<String> {
    let activity =
        analyze_recent_audio_activity(&session.captured_samples, session.sample_rate_hz, 160);
    let voice_active = is_streaming_endpoint_voice_active(&activity);
    session
        .endpoint
        .observe(total_audio_duration_ms, voice_active);

    if session.awaiting_post_rollover_speech {
        if !voice_active {
            return None;
        }
        session.awaiting_post_rollover_speech = false;
    }

    let app_endpoint = should_rollover_streaming_segment(
        session,
        &runtime.config.voice.streaming.endpoint,
        total_audio_duration_ms,
    );
    let sherpa_endpoint = recognizer.is_endpoint(&session.stream);
    if !app_endpoint && !sherpa_endpoint {
        return None;
    }

    let sample_rate_hz = session.sample_rate_hz;
    let chunk_samples =
        streaming_chunk_num_samples(sample_rate_hz, runtime.config.voice.streaming.chunk_ms);
    let pause_decode_steps = finalize_streaming_pause_boundary_decode(
        recognizer,
        session,
        sample_rate_hz,
        chunk_samples,
        streaming_idle_finalize_tail_padding_ms(&runtime.config.voice.streaming.endpoint),
    );
    let current_display = effective_streaming_display_text(session);
    if visible_text_char_count(&current_display) == 0 {
        recognizer.reset(&session.stream);
        session
            .endpoint
            .reset_after_rollover(total_audio_duration_ms);
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
    let committed_text = resolve_streaming_rollover_commit_text(
        &session.rolled_over_prefix,
        &current_display,
        &current_raw_text,
    );
    let committed_text = if committed_text.trim().is_empty() {
        current_display.clone()
    } else {
        committed_text
    };
    let committed_text =
        prepare_streaming_pause_boundary_text(&committed_text, rewrite_enabled, punctuator);

    let update = session.state.rollover_with_display_text(&committed_text);
    session.rolled_over_prefix = update.display_text.clone();
    session.awaiting_post_rollover_speech = true;
    session
        .endpoint
        .reset_after_rollover(total_audio_duration_ms);
    session.last_raw_partial.clear();
    session.last_fast_preview_text.clear();
    session.last_display_text = update.display_text.clone();
    reset_streaming_ai_rewrite_cache(session);
    recognizer.reset(&session.stream);

    tracing::info!(
        audio_duration_ms = total_audio_duration_ms,
        app_endpoint,
        sherpa_endpoint,
        endpoint_raw_text = %current_raw_text,
        endpoint_text_before_reset = %current_display,
        endpoint_committed_text = %update.display_text,
        pause_decode_steps,
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

fn select_streaming_commit_text(
    display_text: &str,
    final_candidate: &str,
    final_candidate_source: StreamingCommitSource,
) -> StreamingCommitChoice {
    let display_trimmed = display_text.trim();
    let candidate_trimmed = final_candidate.trim();
    let selected_candidate = if visible_text_char_count(candidate_trimmed) > 0 {
        StreamingCommitChoice {
            source: final_candidate_source,
            text: candidate_trimmed.to_string(),
        }
    } else {
        StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: String::new(),
        }
    };

    if display_trimmed.is_empty() {
        return selected_candidate;
    }

    if selected_candidate.text.is_empty() {
        return StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: display_trimmed.to_string(),
        };
    }

    if let Some((text, overlap_chars)) =
        repair_final_candidate_tail_overlap(display_trimmed, &selected_candidate.text)
    {
        tracing::info!(
            overlap_chars,
            display_text = %short_log_text(display_trimmed, 120),
            final_candidate = %short_log_text(&selected_candidate.text, 120),
            repaired_text = %short_log_text(&text, 120),
            "streaming final commit repaired duplicated tail overlap"
        );
        return StreamingCommitChoice {
            source: selected_candidate.source,
            text,
        };
    }

    let display_visible = visible_text_char_count(display_trimmed);
    let selected_visible = visible_text_char_count(&selected_candidate.text);
    if let Some(text) = preserve_display_tail_when_final_candidate_is_shorter(
        display_trimmed,
        &selected_candidate.text,
    ) {
        return StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text,
        };
    }

    if selected_visible + STREAMING_FINAL_SHORTFALL_TOLERANCE_CHARS < display_visible
        && !is_segment_only_streaming_candidate(display_trimmed, &selected_candidate.text)
    {
        return StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: display_trimmed.to_string(),
        };
    }

    let common_prefix_chars =
        longest_common_prefix_chars(display_trimmed, &selected_candidate.text);
    if common_prefix_chars < 4
        && display_visible > selected_visible
        && !is_segment_only_streaming_candidate(display_trimmed, &selected_candidate.text)
    {
        return StreamingCommitChoice {
            source: StreamingCommitSource::StreamingState,
            text: display_trimmed.to_string(),
        };
    }

    selected_candidate
}

fn repair_final_candidate_tail_overlap(
    display_text: &str,
    final_candidate: &str,
) -> Option<(String, usize)> {
    let display = display_text.trim();
    let candidate = final_candidate.trim();
    let appended_tail = candidate.strip_prefix(display)?.trim_start();
    if display.is_empty() || appended_tail.is_empty() {
        return None;
    }

    let overlap_chars = longest_fuzzy_suffix_prefix_overlap_chars(
        display,
        appended_tail,
        STREAMING_FINAL_FUZZY_TAIL_OVERLAP_MAX_CHARS,
        1,
    );
    if overlap_chars < STREAMING_FINAL_FUZZY_TAIL_OVERLAP_MIN_CHARS {
        return None;
    }

    let display_chars = display.chars().collect::<Vec<_>>();
    if overlap_chars > display_chars.len() {
        return None;
    }
    let prefix_without_overlap = display_chars[..display_chars.len() - overlap_chars]
        .iter()
        .collect::<String>();
    let repaired = format!("{prefix_without_overlap}{appended_tail}");
    if repaired == candidate || repaired.trim().is_empty() {
        return None;
    }

    Some((repaired, overlap_chars))
}

fn preserve_display_tail_when_final_candidate_is_shorter(
    display_text: &str,
    final_candidate: &str,
) -> Option<String> {
    if display_text.trim().is_empty() || final_candidate.trim().is_empty() {
        return None;
    }
    if is_segment_only_streaming_candidate(display_text, final_candidate) {
        return None;
    }

    let display_content = content_text_without_sentence_punctuation(display_text);
    let candidate_content = content_text_without_sentence_punctuation(final_candidate);
    if candidate_content.is_empty()
        || candidate_content.chars().count() >= display_content.chars().count()
    {
        return None;
    }

    if !display_content.starts_with(&candidate_content) {
        return None;
    }

    let mut preserved = display_text.trim().to_string();
    if !has_terminal_sentence_boundary(&preserved)
        && let Some(terminal) = trailing_terminal_sentence_punctuation(final_candidate)
    {
        preserved.push(terminal);
    }
    Some(preserved)
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
        if looks_like_rollover_full_replacement(rolled_over_prefix, current_display) {
            tracing::info!(
                rolled_over_prefix = %short_log_text(rolled_over_prefix, 120),
                replacement_candidate = %short_log_text(current_display, 120),
                "streaming rollover prefix merge treated final candidate as full replacement"
            );
            return current_display.to_string();
        }
        return append_with_suffix_prefix_overlap(rolled_over_prefix, current_display);
    }

    current_display.to_string()
}

fn looks_like_rollover_full_replacement(rolled_over_prefix: &str, current_display: &str) -> bool {
    let prefix_content = content_text_without_sentence_punctuation(rolled_over_prefix);
    let current_content = content_text_without_sentence_punctuation(current_display);
    let prefix_chars = prefix_content.chars().count();
    let current_chars = current_content.chars().count();
    if prefix_chars < 4 || current_chars < 4 {
        return false;
    }
    if current_chars + 2 < prefix_chars {
        return false;
    }

    let common_subsequence_chars =
        longest_common_subsequence_chars(&prefix_content, &current_content);
    let shorter_chars = prefix_chars.min(current_chars);
    common_subsequence_chars >= 4 && common_subsequence_chars * 100 >= shorter_chars * 70
}

fn append_with_suffix_prefix_overlap(prefix: &str, suffix: &str) -> String {
    let prefix = prefix.trim();
    let suffix = suffix.trim();
    if prefix.is_empty() || suffix.is_empty() {
        return format!("{prefix}{suffix}");
    }

    let overlap_chars = longest_suffix_prefix_overlap_chars(prefix, suffix, 12);
    let suffix_without_overlap = suffix.chars().skip(overlap_chars).collect::<String>();
    format!("{prefix}{suffix_without_overlap}")
}

fn longest_suffix_prefix_overlap_chars(prefix: &str, suffix: &str, max_chars: usize) -> usize {
    let prefix_chars = prefix.chars().collect::<Vec<_>>();
    let suffix_chars = suffix.chars().collect::<Vec<_>>();
    let max_overlap = prefix_chars
        .len()
        .min(suffix_chars.len())
        .min(max_chars.max(1));

    for overlap in (2..=max_overlap).rev() {
        if prefix_chars[prefix_chars.len() - overlap..]
            .iter()
            .eq(suffix_chars[..overlap].iter())
        {
            return overlap;
        }
    }

    0
}

fn longest_fuzzy_suffix_prefix_overlap_chars(
    prefix: &str,
    suffix: &str,
    max_chars: usize,
    max_mismatches: usize,
) -> usize {
    let prefix_chars = prefix.chars().collect::<Vec<_>>();
    let suffix_chars = suffix.chars().collect::<Vec<_>>();
    let max_overlap = prefix_chars
        .len()
        .min(suffix_chars.len())
        .min(max_chars.max(1));

    for overlap in (STREAMING_FINAL_FUZZY_TAIL_OVERLAP_MIN_CHARS..=max_overlap).rev() {
        let mismatches = prefix_chars[prefix_chars.len() - overlap..]
            .iter()
            .zip(suffix_chars[..overlap].iter())
            .filter(|(left, right)| left != right)
            .count();
        if mismatches <= max_mismatches {
            return overlap;
        }
    }

    0
}

fn longest_common_subsequence_chars(left: &str, right: &str) -> usize {
    let left_chars = left.chars().collect::<Vec<_>>();
    let right_chars = right.chars().collect::<Vec<_>>();
    if left_chars.is_empty() || right_chars.is_empty() {
        return 0;
    }

    let mut previous = vec![0usize; right_chars.len() + 1];
    let mut current = vec![0usize; right_chars.len() + 1];
    for left_ch in left_chars {
        for (right_index, right_ch) in right_chars.iter().enumerate() {
            current[right_index + 1] = if left_ch == *right_ch {
                previous[right_index] + 1
            } else {
                previous[right_index + 1].max(current[right_index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
        current.fill(0);
    }

    previous[right_chars.len()]
}

fn resolve_streaming_rollover_commit_text(
    rolled_over_prefix: &str,
    current_display: &str,
    current_raw_text: &str,
) -> String {
    let display = current_display.trim();
    let raw_prepared = ainput_rewrite::normalize_streaming_preview(current_raw_text);
    if raw_prepared.trim().is_empty() {
        return display.to_string();
    }

    let raw_display = merge_rolled_over_prefix(rolled_over_prefix, &raw_prepared);
    let raw_display = raw_display.trim();
    if display.is_empty() {
        return raw_display.to_string();
    }

    let display_visible = visible_text_char_count(display);
    let raw_visible = visible_text_char_count(raw_display);
    if raw_visible > display_visible {
        return raw_display.to_string();
    }

    display.to_string()
}

fn prepare_streaming_pause_boundary_text(
    text: &str,
    rewrite_enabled: bool,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
) -> String {
    let normalized = ainput_rewrite::normalize_streaming_preview(text);
    let _ = (rewrite_enabled, punctuator);
    apply_streaming_semantic_commas(&dedupe_streaming_punctuation(&normalized))
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
    sustained_voice_ms: u64,
}

fn analyze_audio_activity(samples: &[f32]) -> AudioActivity {
    if samples.is_empty() {
        return AudioActivity {
            peak_abs: 0.0,
            rms: 0.0,
            active_ratio: 0.0,
            sustained_voice_ms: 0,
        };
    }

    let mut peak_abs = 0.0f32;
    let mut energy_sum = 0.0f64;
    let mut active_frames = 0usize;
    let mut sustained_voice_frames = 0usize;
    let mut best_sustained_voice_frames = 0usize;

    for sample in samples {
        let abs = sample.abs();
        peak_abs = peak_abs.max(abs);
        energy_sum += (abs as f64) * (abs as f64);
        if abs >= 0.008 {
            active_frames += 1;
        }
    }

    for frame in samples.chunks(AUDIO_ACTIVITY_FRAME_SAMPLES) {
        let frame_rms = (frame
            .iter()
            .map(|sample| {
                let sample = *sample as f64;
                sample * sample
            })
            .sum::<f64>()
            / frame.len() as f64)
            .sqrt() as f32;
        if frame_rms >= AUDIO_ACTIVITY_SPEECH_FRAME_RMS {
            sustained_voice_frames += 1;
            best_sustained_voice_frames = best_sustained_voice_frames.max(sustained_voice_frames);
        } else {
            sustained_voice_frames = 0;
        }
    }

    let rms = (energy_sum / samples.len() as f64).sqrt() as f32;
    let active_ratio = active_frames as f32 / samples.len() as f32;
    let sustained_voice_ms = best_sustained_voice_frames as u64 * AUDIO_ACTIVITY_FRAME_MS;

    AudioActivity {
        peak_abs,
        rms,
        active_ratio,
        sustained_voice_ms,
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

fn is_streaming_endpoint_voice_active(activity: &AudioActivity) -> bool {
    activity.peak_abs >= 0.008 || activity.rms >= 0.0024 || activity.active_ratio >= 0.018
}

fn should_rollover_streaming_segment(
    session: &StreamingCoreSession,
    config: &ainput_shell::StreamingEndpointConfig,
    total_audio_duration_ms: u64,
) -> bool {
    if !config.enabled {
        return false;
    }

    let visible_chars = visible_text_char_count(&effective_streaming_display_text(session));
    if visible_chars == 0 {
        return false;
    }

    let segment_elapsed_ms =
        total_audio_duration_ms.saturating_sub(session.endpoint.segment_start_ms);
    let min_segment_ms = config.min_segment_ms.clamp(200, 5_000);
    let max_segment_ms = config.max_segment_ms.clamp(min_segment_ms + 1_000, 60_000);
    if segment_elapsed_ms >= max_segment_ms {
        return true;
    }

    if segment_elapsed_ms < min_segment_ms {
        return false;
    }

    let Some(last_voice_ms) = session.endpoint.last_voice_ms else {
        return false;
    };
    let pause_ms = config.pause_ms.clamp(200, 2_000);
    total_audio_duration_ms.saturating_sub(last_voice_ms) >= pause_ms
}

fn should_drop_low_signal_result(text: &str, activity: &AudioActivity) -> bool {
    let stripped = text
        .trim()
        .trim_matches(|ch: char| ch.is_whitespace() || is_sentence_punctuation(ch));

    if stripped.is_empty() {
        return true;
    }

    if is_low_confidence_short_english_hallucination(stripped, activity) {
        return true;
    }

    if activity.rms >= 0.003 || activity.active_ratio >= 0.02 {
        return false;
    }

    stripped.chars().count() <= 2
}

fn is_low_confidence_short_english_hallucination(stripped: &str, activity: &AudioActivity) -> bool {
    if !is_short_english_filler(stripped) {
        return false;
    }

    activity.rms < LOW_CONFIDENCE_SHORT_ENGLISH_RMS
        || activity.active_ratio < LOW_CONFIDENCE_SHORT_ENGLISH_ACTIVE_RATIO
        || activity.sustained_voice_ms < LOW_CONFIDENCE_SHORT_ENGLISH_SUSTAINED_VOICE_MS
}

fn is_short_english_filler(text: &str) -> bool {
    if text.chars().any(|ch| {
        !(ch.is_ascii_alphabetic()
            || ch.is_ascii_whitespace()
            || ch == '\''
            || ch == '-'
            || is_sentence_punctuation(ch))
    }) {
        return false;
    }

    let normalized: String = text
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    matches!(
        normalized.as_str(),
        "yeah"
            | "yea"
            | "yes"
            | "yep"
            | "ok"
            | "okay"
            | "uh"
            | "um"
            | "hmm"
            | "hm"
            | "ah"
            | "oh"
            | "hey"
            | "hi"
    )
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

fn contains_cjk_char(text: &str) -> bool {
    text.chars().any(is_cjk_char)
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

fn current_timestamp_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Debug, Serialize)]
struct StreamingRawCaptureMetadata {
    wav_file: String,
    sample_rate_hz: i32,
    source_channels: u16,
    saved_channels: u16,
    samples: usize,
}

#[derive(Debug, Clone)]
struct StreamingRawCaptureSaveResult {
    wav_path: PathBuf,
    json_path: PathBuf,
}

fn save_streaming_raw_capture_async(raw_capture_dir: PathBuf, audio: ainput_audio::RecordedAudio) {
    std::thread::spawn(move || {
        if let Err(error) = save_streaming_raw_capture(raw_capture_dir, audio) {
            tracing::warn!(error = %error, "save streaming raw capture failed");
        }
    });
}

fn save_streaming_raw_capture(
    raw_capture_dir: PathBuf,
    audio: ainput_audio::RecordedAudio,
) -> Result<StreamingRawCaptureSaveResult> {
    if audio.sample_rate_hz <= 0 || audio.samples.is_empty() {
        return Err(anyhow!("streaming raw capture audio was empty"));
    }

    fs::create_dir_all(&raw_capture_dir).with_context(|| {
        format!(
            "create streaming raw capture dir {}",
            raw_capture_dir.display()
        )
    })?;
    let stamp = current_timestamp_millis();
    let (_file_stem, wav_path, json_path) =
        next_streaming_raw_capture_paths(&raw_capture_dir, stamp)?;
    write_mono_i16_wav(&wav_path, audio.sample_rate_hz as u32, &audio.samples)?;

    let metadata = StreamingRawCaptureMetadata {
        wav_file: wav_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| wav_path.display().to_string()),
        sample_rate_hz: audio.sample_rate_hz,
        source_channels: audio.channels,
        saved_channels: 1,
        samples: audio.samples.len(),
    };
    let metadata_json = serde_json::to_vec_pretty(&metadata)?;
    fs::write(&json_path, metadata_json).with_context(|| {
        format!(
            "write streaming raw capture metadata {}",
            json_path.display()
        )
    })?;
    prune_streaming_raw_captures(&raw_capture_dir, STREAMING_RAW_CAPTURE_LIMIT)?;
    tracing::info!(
        wav = %wav_path.display(),
        metadata = %json_path.display(),
        limit = STREAMING_RAW_CAPTURE_LIMIT,
        "streaming raw capture saved"
    );
    Ok(StreamingRawCaptureSaveResult {
        wav_path,
        json_path,
    })
}

fn next_streaming_raw_capture_paths(
    raw_capture_dir: &Path,
    stamp: u128,
) -> Result<(String, PathBuf, PathBuf)> {
    for suffix in 0..1_000 {
        let file_stem = if suffix == 0 {
            format!("streaming-raw-{stamp}")
        } else {
            format!("streaming-raw-{stamp}-{suffix:03}")
        };
        let wav_path = raw_capture_dir.join(format!("{file_stem}.wav"));
        let json_path = raw_capture_dir.join(format!("{file_stem}.json"));
        if !wav_path.exists() && !json_path.exists() {
            return Ok((file_stem, wav_path, json_path));
        }
    }
    Err(anyhow!(
        "unable to allocate unique streaming raw capture path for timestamp {stamp}"
    ))
}

fn write_mono_i16_wav(path: &Path, sample_rate_hz: u32, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("create raw capture wav {}", path.display()))?;
    for sample in samples {
        let sample = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer
            .write_sample(sample)
            .with_context(|| format!("write raw capture wav {}", path.display()))?;
    }
    writer
        .finalize()
        .with_context(|| format!("finalize raw capture wav {}", path.display()))?;
    Ok(())
}

fn prune_streaming_raw_captures(raw_capture_dir: &Path, keep: usize) -> Result<()> {
    let mut captures = fs::read_dir(raw_capture_dir)
        .with_context(|| format!("read raw capture dir {}", raw_capture_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("streaming-raw-") && name.ends_with(".wav"))
        })
        .collect::<Vec<_>>();
    captures.sort();
    let stale_count = captures.len().saturating_sub(keep);
    for wav_path in captures.into_iter().take(stale_count) {
        let json_path = wav_path.with_extension("json");
        if let Err(error) = fs::remove_file(&wav_path) {
            tracing::warn!(error = %error, path = %wav_path.display(), "remove stale raw capture wav failed");
        }
        if json_path.exists()
            && let Err(error) = fs::remove_file(&json_path)
        {
            tracing::warn!(error = %error, path = %json_path.display(), "remove stale raw capture metadata failed");
        }
    }
    Ok(())
}

fn log_streaming_timing_gate_results(
    first_partial_elapsed_ms: Option<u128>,
    release_tail_elapsed_ms: u128,
    offline_final_elapsed_ms: u128,
    offline_final_timed_out: bool,
    punctuation_elapsed_ms: u128,
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

    log_streaming_timing_gate(
        "streaming release-tail timing gate evaluated",
        release_tail_elapsed_ms,
        STREAMING_RELEASE_TAIL_TARGET_MS,
        STREAMING_RELEASE_TAIL_HARD_MS,
        None,
    );
    log_streaming_timing_gate(
        "streaming offline-final timing gate evaluated",
        offline_final_elapsed_ms,
        STREAMING_OFFLINE_FINAL_TARGET_MS,
        STREAMING_OFFLINE_FINAL_HARD_MS,
        Some(offline_final_timed_out),
    );
    log_streaming_timing_gate(
        "streaming punctuation timing gate evaluated",
        punctuation_elapsed_ms,
        STREAMING_PUNCTUATION_TARGET_MS,
        STREAMING_PUNCTUATION_HARD_MS,
        None,
    );

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

fn log_streaming_timing_gate(
    message: &'static str,
    elapsed_ms: u128,
    target_ms: u128,
    hard_ms: u128,
    timeout_fallback: Option<bool>,
) {
    let gate_status = if elapsed_ms <= target_ms {
        "target_pass"
    } else if elapsed_ms <= hard_ms {
        "hard_pass"
    } else {
        "hard_fail"
    };
    tracing::info!(
        elapsed_ms,
        target_ms,
        hard_ms,
        gate_status,
        timeout_fallback,
        gate = message,
        "streaming timing gate evaluated"
    );
}

fn delivery_label(delivery: OutputDelivery) -> &'static str {
    match delivery {
        OutputDelivery::NativeEdit => "voice_native_edit",
        OutputDelivery::DirectPaste => "voice_direct_paste",
        OutputDelivery::ClipboardOnly => "voice_clipboard_only",
    }
}

fn streaming_delivery_label(delivery: OutputDelivery) -> &'static str {
    match delivery {
        OutputDelivery::NativeEdit => "streaming_native_edit",
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
    if rewrite_enabled {
        apply_streaming_semantic_commas(&apply_streaming_punctuation_content_safe(
            &normalized,
            punctuator,
            false,
        ))
    } else {
        apply_streaming_semantic_commas(&dedupe_streaming_punctuation(&normalized))
    }
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
        apply_streaming_semantic_commas(&apply_streaming_punctuation_content_safe(
            &normalized,
            punctuator,
            true,
        ))
    } else {
        apply_streaming_semantic_commas(&dedupe_streaming_punctuation(&normalized))
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
        return dedupe_streaming_punctuation(normalized);
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
    let punctuated_latest = dedupe_streaming_punctuation(&punctuated_latest);
    let punctuated_latest = if finalize
        || should_keep_streaming_preview_terminal_punctuation(target_text, &punctuated_latest)
    {
        punctuated_latest
    } else {
        strip_trailing_terminal_sentence_punctuation(&punctuated_latest)
    };

    apply_streaming_semantic_commas(&dedupe_streaming_punctuation(&format!(
        "{}{}",
        frozen_prefix, punctuated_latest
    )))
}

fn apply_streaming_punctuation_content_safe(
    normalized_text: &str,
    punctuator: Option<&ainput_asr::OfflinePunctuationRestorer>,
    finalize: bool,
) -> String {
    let normalized = normalized_text.trim();
    if normalized.is_empty() {
        return String::new();
    }

    let punctuated = apply_streaming_punctuation(normalized, punctuator, finalize);
    let normalized_content_chars = content_chars_without_sentence_punctuation(normalized);
    let punctuated_content_chars = content_chars_without_sentence_punctuation(&punctuated);
    if punctuated_content_chars < normalized_content_chars {
        tracing::debug!(
            normalized = %short_log_text(normalized, 120),
            punctuated = %short_log_text(&punctuated, 120),
            normalized_content_chars,
            punctuated_content_chars,
            "streaming punctuation rejected because it removed content characters"
        );
        normalized.to_string()
    } else {
        punctuated
    }
}

fn content_chars_without_sentence_punctuation(text: &str) -> usize {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_sentence_punctuation(*ch))
        .count()
}

fn content_text_without_sentence_punctuation(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_sentence_punctuation(*ch))
        .collect()
}

fn dedupe_streaming_punctuation(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut punctuation_run = Vec::new();

    for ch in text.trim().chars() {
        if is_sentence_punctuation(ch) {
            punctuation_run.push(ch);
            continue;
        }

        flush_punctuation_run(&mut result, &mut punctuation_run);
        result.push(ch);
    }
    flush_punctuation_run(&mut result, &mut punctuation_run);
    result.trim().to_string()
}

fn apply_streaming_semantic_commas(text: &str) -> String {
    let mut current = text.trim().to_string();
    for marker in ["另外", "然后", "而且", "但是", "不过", "所以", "现在"] {
        current = insert_comma_after_leading_marker(&current, marker, 4);
    }
    for marker in [
        "但是",
        "不过",
        "而且",
        "然后",
        "还是",
        "尤其是",
        "或者",
        "比如",
    ] {
        current = insert_comma_before_marker(&current, marker, 6);
    }
    apply_streaming_semantic_tail_repairs(&current)
}

fn apply_streaming_semantic_tail_repairs(text: &str) -> String {
    let mut current = text.trim().to_string();
    for (from, to) in [
        ("显示出来两个应该是", "显示出来两个字。应该是"),
        ("出现文明明", "出现文字。明明"),
    ] {
        current = current.replace(from, to);
    }
    current
}

fn insert_comma_after_leading_marker(text: &str, marker: &str, min_tail_chars: usize) -> String {
    let Some(rest) = text.strip_prefix(marker) else {
        return text.to_string();
    };
    if rest.is_empty() {
        return text.to_string();
    }
    if rest.chars().next().is_some_and(is_sentence_punctuation) {
        return text.to_string();
    }
    if rest.chars().filter(|ch| !ch.is_whitespace()).count() < min_tail_chars {
        return text.to_string();
    }
    format!("{marker}，{}", rest.trim_start())
}

fn insert_comma_before_marker(text: &str, marker: &str, min_prefix_chars: usize) -> String {
    if text.is_empty() || marker.is_empty() {
        return text.to_string();
    }

    let mut output = String::with_capacity(text.len() + 8);
    let mut cursor = 0usize;
    while let Some(relative_index) = text[cursor..].find(marker) {
        let index = cursor + relative_index;
        output.push_str(&text[cursor..index]);
        if should_insert_streaming_comma_before(text, index, min_prefix_chars) {
            output.push('，');
        }
        output.push_str(marker);
        cursor = index + marker.len();
    }
    output.push_str(&text[cursor..]);
    output
}

fn should_insert_streaming_comma_before(
    text: &str,
    marker_index: usize,
    min_prefix_chars: usize,
) -> bool {
    let prefix = text[..marker_index].trim_end();
    if prefix.chars().count() < min_prefix_chars {
        return false;
    }
    prefix
        .chars()
        .next_back()
        .is_some_and(|ch| !is_sentence_punctuation(ch))
}

fn flush_punctuation_run(result: &mut String, punctuation_run: &mut Vec<char>) {
    if punctuation_run.is_empty() {
        return;
    }

    result.push(normalize_punctuation_run(punctuation_run));
    punctuation_run.clear();
}

fn normalize_punctuation_run(punctuation_run: &[char]) -> char {
    if punctuation_run.len() == 1 {
        return punctuation_run[0];
    }

    if punctuation_run.iter().any(|ch| matches!(ch, '?' | '？')) {
        return '？';
    }
    if punctuation_run.iter().any(|ch| matches!(ch, '!' | '！')) {
        return '！';
    }
    if punctuation_run
        .iter()
        .any(|ch| matches!(ch, '.' | '。' | '．'))
    {
        return '。';
    }
    if punctuation_run.iter().any(|ch| matches!(ch, ';' | '；')) {
        return '；';
    }
    if punctuation_run.iter().any(|ch| matches!(ch, ':' | '：')) {
        return '：';
    }
    if punctuation_run.iter().all(|ch| *ch == '、') {
        return '、';
    }
    '，'
}

fn trailing_terminal_sentence_punctuation(text: &str) -> Option<char> {
    text.trim().chars().next_back().and_then(|ch| match ch {
        '.' | '。' | '．' => Some('。'),
        '!' | '！' => Some('！'),
        '?' | '？' => Some('？'),
        ';' | '；' => Some('；'),
        _ => None,
    })
}

fn should_keep_streaming_preview_terminal_punctuation(
    source_text: &str,
    punctuated_text: &str,
) -> bool {
    let Some(terminal) = trailing_terminal_sentence_punctuation(punctuated_text) else {
        return false;
    };
    let source = source_text.trim();
    if visible_text_char_count(source) < 4 {
        return false;
    }

    match terminal {
        '？' => has_semantic_question_cue(source),
        '。' => has_semantic_statement_completion_cue(source),
        '！' => has_semantic_exclamation_cue(source),
        _ => false,
    }
}

fn has_semantic_question_cue(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.ends_with('吗')
        || trimmed.ends_with('么')
        || trimmed.contains("是不是")
        || trimmed.contains("对不对")
        || trimmed.contains("能不能")
        || trimmed.contains("可不可以")
        || trimmed.contains("要不要")
}

fn has_semantic_statement_completion_cue(text: &str) -> bool {
    let trimmed = text.trim();
    [
        "完了",
        "好了",
        "结束了",
        "完成了",
        "可以了",
        "没问题了",
        "清楚了",
        "明白了",
        "正常了",
        "成功了",
        "失败了",
        "就这样",
        "到这里",
    ]
    .iter()
    .any(|ending| trimmed.ends_with(ending))
}

fn has_semantic_exclamation_cue(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.ends_with('啊') || trimmed.ends_with('呀') || trimmed.ends_with('啦')
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
        AudioActivity, STREAMING_OFFLINE_FINAL_FULL_AUDIO_MAX_MS, STREAMING_OFFLINE_FINAL_HARD_MS,
        STREAMING_OFFLINE_FINAL_TAIL_WINDOW_MS, STREAMING_RAW_CAPTURE_LIMIT, StreamingCommitSource,
        StreamingOfflineFinalScope, StreamingResampler, analyze_recent_audio_activity,
        append_with_suffix_prefix_overlap, apply_streaming_semantic_commas,
        build_streaming_ai_rewrite_candidate, dedupe_streaming_punctuation,
        effective_ai_rewrite_min_visible_chars, ensure_terminal_sentence_boundary,
        finalize_streaming_commit_text, merge_rolled_over_prefix,
        merge_streaming_offline_tail_repair, prepare_streaming_output_text,
        prepare_streaming_pause_boundary_text, prepare_streaming_preview_text,
        resolve_streaming_rollover_commit_text, sanitize_ai_rewrite_output,
        save_streaming_raw_capture, select_streaming_commit_text, select_streaming_final_raw_text,
        select_streaming_preview_text, should_drop_low_signal_result,
        should_drop_streaming_preview_result, should_skip_streaming_preview,
        streaming_final_quality_failures, streaming_offline_final_sample_start,
        streaming_offline_final_scope, strip_trailing_terminal_sentence_punctuation,
    };
    use std::fs;

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
    fn streaming_punctuation_dedupes_repeated_marks() {
        assert_eq!(
            dedupe_streaming_punctuation("这个标点，，而且重复。。真的？！"),
            "这个标点，而且重复。真的？"
        );
    }

    #[test]
    fn streaming_semantic_commas_handles_leading_discourse_marker() {
        assert_eq!(
            apply_streaming_semantic_commas("另外这个加标点符号的逻辑"),
            "另外，这个加标点符号的逻辑"
        );
    }

    #[test]
    fn streaming_semantic_commas_handles_mid_sentence_markers() {
        assert_eq!(
            apply_streaming_semantic_commas("最后一个字的问题还是可能会漏字尤其是语气词"),
            "最后一个字的问题，还是可能会漏字，尤其是语气词"
        );
    }

    #[test]
    fn streaming_semantic_tail_repairs_restore_obvious_missing_classifier_chars() {
        assert_eq!(
            apply_streaming_semantic_commas(
                "然后，不管我说多少个字，它永远只能显示出来两个应该是我不断的说话之后，它能不断地出现文明明这个 HUD 上面已经把正确的文案显示出来了"
            ),
            "然后，不管我说多少个字，它永远只能显示出来两个字。应该是我不断的说话之后，它能不断地出现文字。明明这个 HUD 上面已经把正确的文案显示出来了"
        );
    }

    #[test]
    fn streaming_preview_skips_background_noise_before_real_speech() {
        let activity = AudioActivity {
            peak_abs: 0.004,
            rms: 0.0012,
            active_ratio: 0.004,
            sustained_voice_ms: 0,
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
            sustained_voice_ms: 240,
        };
        assert!(!should_skip_streaming_preview(&activity));
        assert!(!should_drop_streaming_preview_result(
            "帮我看一下这里有没有问题",
            &activity
        ));
    }

    #[test]
    fn low_confidence_short_english_fillers_are_dropped() {
        let ghost_yeah = AudioActivity {
            peak_abs: 0.081,
            rms: 0.0052,
            active_ratio: 0.0508,
            sustained_voice_ms: 20,
        };
        assert!(should_drop_low_signal_result("Yeah.", &ghost_yeah));

        let ghost_okay = AudioActivity {
            peak_abs: 0.045,
            rms: 0.0029,
            active_ratio: 0.0228,
            sustained_voice_ms: 40,
        };
        assert!(should_drop_low_signal_result("Okay.", &ghost_okay));
    }

    #[test]
    fn low_signal_filter_keeps_clear_or_meaningful_text() {
        let clear_short_english = AudioActivity {
            peak_abs: 0.048,
            rms: 0.008,
            active_ratio: 0.12,
            sustained_voice_ms: 180,
        };
        assert!(!should_drop_low_signal_result(
            "Okay.",
            &clear_short_english
        ));

        let weak_mixed_text = AudioActivity {
            peak_abs: 0.018,
            rms: 0.0032,
            active_ratio: 0.024,
            sustained_voice_ms: 80,
        };
        assert!(!should_drop_low_signal_result(
            "okay 这个问题",
            &weak_mixed_text
        ));
        assert!(!should_drop_low_signal_result("OpenAI", &weak_mixed_text));
    }

    #[test]
    fn recent_audio_activity_prefers_latest_speech_over_old_silence() {
        let mut samples = vec![0.0f32; 16_000];
        samples.extend(std::iter::repeat_n(0.05f32, 4_000));
        let activity = analyze_recent_audio_activity(&samples, 16_000, 700);
        assert!(activity.peak_abs >= 0.05);
        assert!(activity.active_ratio > 0.1);
        assert!(activity.sustained_voice_ms >= 200);
    }

    #[test]
    fn raw_capture_writer_keeps_only_recent_twenty_wavs() {
        let dir = std::env::temp_dir().join(format!(
            "ainput-raw-capture-test-{}",
            super::current_timestamp_millis()
        ));
        fs::create_dir_all(&dir).expect("create temp raw capture dir");
        for _ in 0..(STREAMING_RAW_CAPTURE_LIMIT + 2) {
            save_streaming_raw_capture(
                dir.clone(),
                ainput_audio::RecordedAudio {
                    sample_rate_hz: 16_000,
                    channels: 1,
                    samples: vec![0.0, 0.25, -0.25, 0.0],
                },
            )
            .expect("save raw capture");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let wav_count = fs::read_dir(&dir)
            .expect("read temp raw capture dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("wav"))
            })
            .count();
        assert_eq!(wav_count, STREAMING_RAW_CAPTURE_LIMIT);
        let _ = fs::remove_dir_all(&dir);
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
    fn commit_text_preserves_one_char_tail_drop() {
        assert_eq!(
            select_streaming_commit_text(
                "我已经把这个问题修好了",
                "我已经把这个问题修好。",
                StreamingCommitSource::OnlineFinal
            )
            .text,
            "我已经把这个问题修好了。"
        );
    }

    #[test]
    fn final_commit_repairs_fuzzy_tail_overlap_instead_of_duplicating() {
        let selected = select_streaming_commit_text(
            "我都已经设置了多跳思考的",
            "我都已经设置了多跳思考的设置了多跳思考了。",
            StreamingCommitSource::OfflineFinal,
        );
        assert_eq!(selected.source, StreamingCommitSource::OfflineFinal);
        assert_eq!(selected.text, "我都已经设置了多跳思考了。");
    }

    #[test]
    fn final_commit_dedupes_exact_tail_overlap_after_display_prefix() {
        let selected = select_streaming_commit_text(
            "这个功能应该是这样处理",
            "这个功能应该是这样处理这样处理了。",
            StreamingCommitSource::OfflineFinal,
        );
        assert_eq!(selected.text, "这个功能应该是这样处理了。");
    }

    #[test]
    fn final_commit_keeps_true_tail_append_without_overlap() {
        let selected = select_streaming_commit_text(
            "你这些分辨率",
            "你这些分辨率有问题。",
            StreamingCommitSource::OfflineFinal,
        );
        assert_eq!(selected.text, "你这些分辨率有问题。");
    }

    #[test]
    fn offline_final_raw_text_repairs_streaming_tail_drop() {
        let (selected, source) = select_streaming_final_raw_text(
            "一共花了两天时",
            "一共花了两天时间。",
            StreamingOfflineFinalScope::FullAudio,
            "一共花了两天时",
        );
        assert_eq!(selected, "一共花了两天时间。");
        assert_eq!(source, StreamingCommitSource::OfflineFinal);
    }

    #[test]
    fn offline_final_late_budget_accepts_tail_repair_window() {
        assert_eq!(STREAMING_OFFLINE_FINAL_HARD_MS, 650);
    }

    #[test]
    fn offline_final_rejects_isolated_i_tail_artifact_after_chinese_display() {
        let (selected, source) = select_streaming_final_raw_text(
            "",
            "I.",
            StreamingOfflineFinalScope::TailWindow,
            "很奇怪还是会漏字和重复",
        );
        assert_eq!(selected, "");
        assert_eq!(source, StreamingCommitSource::OnlineFinal);
    }

    #[test]
    fn offline_final_repairs_bu_i_tail_to_budui() {
        let (selected, source) = select_streaming_final_raw_text(
            "",
            "I.",
            StreamingOfflineFinalScope::TailWindow,
            "简直就是灾难，标点符号都不",
        );
        assert_eq!(selected, "对。");
        assert_eq!(source, StreamingCommitSource::OfflineFinal);
    }

    #[test]
    fn output_text_repairs_observed_i_and_punctuation_artifacts() {
        let (prepared, source) =
            prepare_streaming_output_text("强治就是灾难的标点，符号都不I 。", false, None);
        assert_eq!(prepared, "简直就是灾难，标点符号都不对。");
        assert_eq!(source, StreamingCommitSource::StreamingTailRepair);
    }

    #[test]
    fn final_commit_text_repairs_display_selected_i_and_punctuation_artifacts() {
        assert_eq!(
            finalize_streaming_commit_text("很奇怪还是会漏字和重复I 。"),
            "很奇怪还是会漏字和重复。"
        );
        assert_eq!(
            finalize_streaming_commit_text("强距就是灾难的标点，符号都不对I 。"),
            "简直就是灾难，标点符号都不对。"
        );
    }

    #[test]
    fn streaming_final_quality_gate_catches_i_and_split_word_artifacts() {
        assert!(streaming_final_quality_failures("简直就是灾难，标点符号都不对。").is_empty());
        assert!(
            streaming_final_quality_failures("很奇怪还是会漏字和重复I。")
                .iter()
                .any(|failure| failure.contains("isolated_i"))
        );
        assert!(
            streaming_final_quality_failures("标点，符号都不对。")
                .iter()
                .any(|failure| failure.contains("splits_fixed_word"))
        );
    }

    #[test]
    fn offline_final_uses_tail_window_for_long_audio() {
        let sample_rate_hz = 16_000;
        let long_sample_count = ((STREAMING_OFFLINE_FINAL_FULL_AUDIO_MAX_MS + 1)
            * sample_rate_hz as u64
            / 1000) as usize;
        let scope = streaming_offline_final_scope(sample_rate_hz, long_sample_count);
        assert_eq!(scope, StreamingOfflineFinalScope::TailWindow);
        let start = streaming_offline_final_sample_start(sample_rate_hz, long_sample_count, scope);
        let expected_tail_samples =
            (STREAMING_OFFLINE_FINAL_TAIL_WINDOW_MS * sample_rate_hz as u64 / 1000) as usize;
        assert_eq!(long_sample_count - start, expected_tail_samples);
    }

    #[test]
    fn offline_tail_repair_only_appends_with_overlap() {
        assert_eq!(
            merge_streaming_offline_tail_repair("我已经把这个问题修好", "修好了。").as_deref(),
            Some("我已经把这个问题修好了。")
        );
        assert_eq!(
            merge_streaming_offline_tail_repair("我已经把这个问题修好", "完全不相关"),
            None
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
            sustained_voice_ms: 120,
        };
        assert!(!should_drop_streaming_preview_result("我不知", &activity));
    }

    #[test]
    fn streaming_preview_accepts_weaker_but_real_speech_sooner() {
        let activity = AudioActivity {
            peak_abs: 0.0068,
            rms: 0.0019,
            active_ratio: 0.0125,
            sustained_voice_ms: 80,
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
        let candidate =
            build_streaming_ai_rewrite_candidate("第一句已经稳定。第二句先是错字", 2, 7)
                .expect("candidate");
        assert_eq!(candidate.frozen_prefix, "第一句已经稳定。");
        assert_eq!(candidate.current_tail, "第二句先是错字");
        assert_eq!(candidate.revision, 7);
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
    fn pause_boundary_text_does_not_force_terminal_punctuation() {
        assert_eq!(
            prepare_streaming_pause_boundary_text("第一句还没标点", false, None),
            "第一句还没标点"
        );
    }

    #[test]
    fn pause_boundary_text_preserves_existing_text_without_new_boundary() {
        assert_eq!(
            prepare_streaming_pause_boundary_text("第一句已经稳定。第二句继续", false, None),
            "第一句已经稳定。第二句继续"
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
    fn rollover_prefix_merge_does_not_duplicate_full_replacement() {
        assert_eq!(
            merge_rolled_over_prefix("你最些分辨率有问", "你这些分辨率有问题。"),
            "你这些分辨率有问题。"
        );
    }

    #[test]
    fn final_commit_text_matches_output_terminal_boundary() {
        assert_eq!(
            ensure_terminal_sentence_boundary(
                "明明这个HUD上面已经把正确的文案显示出来了，但是它有时候上屏，还是慢"
            ),
            "明明这个HUD上面已经把正确的文案显示出来了，但是它有时候上屏，还是慢。"
        );
    }

    #[test]
    fn rollover_prefix_merge_still_appends_true_segment_tail() {
        assert_eq!(
            merge_rolled_over_prefix("你这些分辨率", "有问题。"),
            "你这些分辨率有问题。"
        );
    }

    #[test]
    fn rollover_prefix_merge_dedupes_short_overlap() {
        assert_eq!(
            append_with_suffix_prefix_overlap("这个功能应该是这样", "这样了"),
            "这个功能应该是这样了"
        );
    }

    #[test]
    fn rollover_commit_prefers_raw_when_display_lags_tail() {
        assert_eq!(
            resolve_streaming_rollover_commit_text(
                "而且这个面",
                "而且这个面上面显示的东西也会",
                "上面显示的东西也会乱跳"
            ),
            "而且这个面上面显示的东西也会乱跳"
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
