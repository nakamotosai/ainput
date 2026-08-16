use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tracing::{error, info, warn};

use crate::ai_rewrite::{
    AiRewriter, RewriteAttempt, RewriteTrace, SharedRewriter,
    rewrite_error_is_backend_unavailable, rewrite_prompt_for_language,
};
use crate::asr_pool::AsrSessionPool;
use crate::audio::AudioHub;
use crate::cloud_asr::{ChunkResponse, CloudAsrClient, WhisperClient};
use crate::config::{AppConfig, ClipboardPolicy, OutputConfig, RewriteOutputLanguage};
use crate::debug_panel::DebugPanelController;
use crate::history::{HistoryRecord, HistoryService};
use crate::hotkey::{HotkeyEvent, TriggerPhase};
use crate::hud::HudController;
use crate::local_asr::LocalSenseVoiceRecognizer;
use crate::modes::{InputMode, ModeStore, VoiceProfileId};
use crate::output;
use crate::personal_corrections;
use crate::resample::LinearResampler;
use crate::rewrite_language::RewriteLanguageController;
use crate::rewrite_prompt::RewritePromptController;
use crate::voice_command::{self, VoiceCommandController};

static UTTERANCE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

trait VoicePipeline {
    fn id(&self) -> &'static str;
    fn mode(&self) -> InputMode;
    fn run(
        &self,
        worker: &VoiceWorker,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        profile_id: VoiceProfileId,
    ) -> Result<()>;
}

struct StreamingParakeetPipeline;
struct WhisperZhPipeline;
struct LocalNonstreamingPipeline;

enum StreamingChunkEvent {
    Partial(ChunkResponse),
    Error(String),
    Finished {
        text: String,
        language: Option<String>,
        audio_ms: u64,
        elapsed_ms: f64,
        finished: bool,
    },
    FinishError(String),
}

const STREAMING_EMPTY_SNAPSHOT_FINISH_GRACE_MS: u64 = 3000;
const STREAMING_EMPTY_SNAPSHOT_MIN_AUDIO_MS: u64 = 1000;
const STREAMING_DYNAMIC_RELEASE_GRACE_MIN_MS: u64 = 80;
const STREAMING_DYNAMIC_RELEASE_GRACE_MAX_MS: u64 = 150;
const STREAMING_DYNAMIC_RELEASE_RECENT_PARTIAL_MS: u64 = 220;
// Raised: thinking models (step/qwen) often return after 1.5–8s.
const ASYNC_REWRITE_REPLACEMENT_MAX_AGE_MS: u128 = 12_000;
const HUD_FIRST_REWRITE_DEADLINE_MS: u64 = 450;

struct AsyncWhisperRewriteJob {
    utterance_id: String,
    profile_id: String,
    raw_text: String,
    raw_pasted_text: String,
    rewrite_enabled: bool,
    output_language: RewriteOutputLanguage,
    system_prompt: String,
    audio_ms: u64,
    asr_elapsed_ms: u128,
    started_at: Instant,
    target_summary: Option<output::TargetSummary>,
    target_fingerprint: Option<output::TargetFingerprint>,
    target_context_source: String,
    target_right_context: String,
    output_config: OutputConfig,
    debug_mode: bool,
    /// CapsLock local B′: no result text on HUD — fade particles only.
    silent_hud: bool,
}

struct HudFirstWhisperRewriteJob {
    utterance_id: String,
    profile_id: String,
    raw_text: String,
    raw_paste_fallback: String,
    rewrite_enabled: bool,
    output_language: RewriteOutputLanguage,
    system_prompt: String,
    audio_ms: u64,
    asr_elapsed_ms: u128,
    started_at: Instant,
    target: output::OutputTarget,
    output_config: OutputConfig,
    silent_hud: bool,
}

struct AsyncStreamingRewriteJob {
    utterance_id: String,
    profile_id: String,
    raw_text: String,
    raw_pasted_text: String,
    rewrite_enabled: bool,
    output_language: RewriteOutputLanguage,
    audio_ms: u64,
    started_at: Instant,
    partial_updates: usize,
    target_summary: Option<output::TargetSummary>,
    target_fingerprint: Option<output::TargetFingerprint>,
    target_context_source: String,
    target_right_context: String,
    output_config: OutputConfig,
    debug_mode: bool,
    prewrite_trace: Option<RewriteTrace>,
    prewrite_finished_at: Option<Instant>,
    prewrite_status: String,
}

struct HudFirstStreamingRewriteJob {
    utterance_id: String,
    profile_id: String,
    raw_text: String,
    raw_paste_fallback: String,
    rewrite_enabled: bool,
    output_language: RewriteOutputLanguage,
    audio_ms: u64,
    started_at: Instant,
    partial_updates: usize,
    target: output::OutputTarget,
    output_config: OutputConfig,
    prewrite_trace: Option<RewriteTrace>,
    prewrite_status: String,
}

struct StreamingPrewriteResult {
    source_text: String,
    trace: RewriteTrace,
    finished_at: Instant,
}

struct StreamingPrewriteState {
    enabled: bool,
    min_chars: usize,
    stable_ms: u64,
    debounce_ms: u64,
    max_inflight: usize,
    output_language: RewriteOutputLanguage,
    rewriter: Option<AiRewriter>,
    tx: mpsc::Sender<StreamingPrewriteResult>,
    rx: mpsc::Receiver<StreamingPrewriteResult>,
    inflight: Arc<AtomicUsize>,
    last_spawned_at: Option<Instant>,
    last_source_hash: u64,
    history_path: PathBuf,
    context_history_count: usize,
}

struct StreamingChunkPump {
    tx: Option<mpsc::Sender<Vec<f32>>>,
    events: mpsc::Receiver<StreamingChunkEvent>,
}

impl StreamingChunkPump {
    fn start(asr: CloudAsrClient, session_id: String) -> Self {
        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<f32>>();
        let (event_tx, event_rx) = mpsc::channel::<StreamingChunkEvent>();
        thread::spawn(move || {
            let started_at = Instant::now();
            let mut chunks_sent = 0usize;
            while let Ok(chunk) = chunk_rx.recv() {
                match asr.send_chunk(&session_id, &chunk) {
                    Ok(response) => {
                        chunks_sent += 1;
                        let _ = event_tx.send(StreamingChunkEvent::Partial(response));
                    }
                    Err(error) => {
                        let _ = event_tx.send(StreamingChunkEvent::Error(error.to_string()));
                        break;
                    }
                }
            }
            match asr.finish_session(&session_id) {
                Ok(finish) => {
                    info!(
                        session_id = %session_id,
                        chunks_sent,
                        audio_ms = finish.audio_ms,
                        elapsed_ms = finish.elapsed_ms,
                        finished = finish.finished,
                        finish_text_chars = finish.text.chars().count(),
                        background_finish_ms = started_at.elapsed().as_millis(),
                        "streaming ASR chunk pump finished session"
                    );
                    let _ = event_tx.send(StreamingChunkEvent::Finished {
                        text: finish.text,
                        language: finish.language,
                        audio_ms: finish.audio_ms,
                        elapsed_ms: finish.elapsed_ms,
                        finished: finish.finished,
                    });
                }
                Err(error) => {
                    warn!(
                        session_id = %session_id,
                        chunks_sent,
                        error = %error,
                        background_finish_ms = started_at.elapsed().as_millis(),
                        "streaming ASR chunk pump finish failed"
                    );
                    let _ = event_tx.send(StreamingChunkEvent::FinishError(error.to_string()));
                }
            }
        });
        Self {
            tx: Some(chunk_tx),
            events: event_rx,
        }
    }

    fn send_chunk(&self, chunk: Vec<f32>) -> Result<()> {
        let Some(tx) = &self.tx else {
            bail!("streaming chunk pump is closed");
        };
        tx.send(chunk)
            .map_err(|_| anyhow!("streaming chunk pump disconnected"))
    }

    fn close(&mut self) {
        self.tx.take();
    }
}

impl VoicePipeline for StreamingParakeetPipeline {
    fn id(&self) -> &'static str {
        "streaming_parakeet"
    }

    fn mode(&self) -> InputMode {
        InputMode::StreamingAsr
    }

    fn run(
        &self,
        worker: &VoiceWorker,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        profile_id: VoiceProfileId,
    ) -> Result<()> {
        worker.run_streaming_session(hotkey_rx, profile_id)
    }
}

impl VoicePipeline for WhisperZhPipeline {
    fn id(&self) -> &'static str {
        "whisper_zh"
    }

    fn mode(&self) -> InputMode {
        InputMode::WhisperZh
    }

    fn run(
        &self,
        worker: &VoiceWorker,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        profile_id: VoiceProfileId,
    ) -> Result<()> {
        worker.run_whisper_session(hotkey_rx, profile_id)
    }
}

impl VoicePipeline for LocalNonstreamingPipeline {
    fn id(&self) -> &'static str {
        "local_nonstreaming"
    }

    fn mode(&self) -> InputMode {
        InputMode::LocalNonstreaming
    }

    fn run(
        &self,
        worker: &VoiceWorker,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        profile_id: VoiceProfileId,
    ) -> Result<()> {
        worker.run_local_nonstreaming_session(hotkey_rx, profile_id)
    }
}

pub struct VoiceWorker {
    config: AppConfig,
    asr: CloudAsrClient,
    whisper: WhisperClient,
    local_recognizer: Option<LocalSenseVoiceRecognizer>,
    asr_sessions: AsrSessionPool,
    modes: ModeStore,
    audio: AudioHub,
    hud: HudController,
    debug_panel: DebugPanelController,
    history: HistoryService,
    rewriter: SharedRewriter,
    rewrite_language: RewriteLanguageController,
    rewrite_prompt: RewritePromptController,
    voice_command: VoiceCommandController,
    shutdown: Arc<AtomicBool>,
}

impl VoiceWorker {
    pub fn new(
        config: AppConfig,
        asr: CloudAsrClient,
        whisper: WhisperClient,
        local_recognizer: Option<LocalSenseVoiceRecognizer>,
        asr_sessions: AsrSessionPool,
        modes: ModeStore,
        audio: AudioHub,
        hud: HudController,
        debug_panel: DebugPanelController,
        history: HistoryService,
        rewriter: SharedRewriter,
        rewrite_language: RewriteLanguageController,
        rewrite_prompt: RewritePromptController,
        voice_command: VoiceCommandController,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            asr,
            whisper,
            local_recognizer,
            asr_sessions,
            modes,
            audio,
            hud,
            debug_panel,
            history,
            rewriter,
            rewrite_language,
            rewrite_prompt,
            voice_command,
            shutdown,
        }
    }

    pub fn run(&mut self, hotkey_rx: mpsc::Receiver<HotkeyEvent>) -> Result<()> {
        let _streaming_pipeline = StreamingParakeetPipeline;
        let _whisper_pipeline = WhisperZhPipeline;
        let local_pipeline = LocalNonstreamingPipeline;
        info!(
            streaming_pipeline = _streaming_pipeline.id(),
            streaming_mode = ?_streaming_pipeline.mode(),
            whisper_pipeline = _whisper_pipeline.id(),
            whisper_mode = ?_whisper_pipeline.mode(),
            local_pipeline = local_pipeline.id(),
            local_mode = ?local_pipeline.mode(),
            "voice pipelines registered (cloud paths inactive)"
        );
        info!("voice worker started");
        while !self.shutdown.load(Ordering::Relaxed) {
            match hotkey_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(HotkeyEvent::Voice(event)) if event.phase == TriggerPhase::Pressed => {
                    let mode = if self.debug_panel.is_enabled() {
                        self.modes.get()
                    } else {
                        event.mode
                    };
                    let result = match mode {
                        InputMode::LocalNonstreaming => {
                            local_pipeline.run(self, &hotkey_rx, event.profile_id)
                        }
                        InputMode::StreamingAsr | InputMode::WhisperZh => {
                            warn!(
                                ?mode,
                                profile = event.profile_id.as_str(),
                                "cloud voice profile disabled in public ainput; only local SenseVoice is active"
                            );
                            self.hud
                                .show_text("仅支持本地语音 (CapsLock)", false, false);
                            Ok(())
                        }
                    };
                    if let Err(error) = result {
                        self.hud.clear();
                        error!(
                            ?mode,
                            profile = event.profile_id.as_str(),
                            error = %error,
                            "voice session failed"
                        );
                    }
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        info!("voice worker stopped");
        Ok(())
    }


    fn run_streaming_session(
        &self,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        profile_id: VoiceProfileId,
    ) -> Result<()> {
        let utterance_id = next_utterance_id();
        let started_at = Instant::now();
        self.hud.show_active();
        let asr_session = self
            .asr_sessions
            .acquire()
            .context("acquire cloud ASR session")?;
        let audio = self.audio.subscribe(self.config.asr.pre_roll_ms);
        let mut resampler = LinearResampler::new(
            self.audio.sample_rate_hz,
            asr_session.sample_rate_hz.max(1) as u32,
        );
        let mut pending = Vec::<f32>::new();
        let chunk_samples = streaming_chunk_samples(
            asr_session.sample_rate_hz.max(1) as u32,
            self.config.asr.chunk_ms,
        );
        let mut preview = StreamingPreviewState::default();
        let output_language = self.rewrite_language.current();
        let mut prewrite = StreamingPrewriteState::new(
            &self.config.rewrite,
            self.rewriter.get(),
            output_language,
            self.history.path().to_path_buf(),
        );
        let mut chunk_pump =
            StreamingChunkPump::start(self.asr.clone(), asr_session.session_id.clone());

        info!(
            utterance_id,
            session_id = %asr_session.session_id,
            input_sample_rate_hz = self.audio.sample_rate_hz,
            asr_sample_rate_hz = asr_session.sample_rate_hz,
            chunk_samples,
            pre_roll_ms = self.config.asr.pre_roll_ms,
            boost_source = %asr_session.boost_source.as_deref().unwrap_or("unknown"),
            boost_phrases = asr_session.boost_phrases.unwrap_or_default(),
            speech_context_phrases = asr_session.speech_context_phrases.unwrap_or_default(),
            speech_context_limit = asr_session.speech_context_limit.unwrap_or_default(),
            mode = "streaming_asr",
            "streaming ASR session started"
        );

        let released = match self.streaming_hold_loop(
            hotkey_rx,
            &audio.rx,
            &mut resampler,
            &mut pending,
            chunk_samples,
            &chunk_pump,
            &asr_session.session_id,
            &mut preview,
            &mut prewrite,
            started_at,
            &utterance_id,
            profile_id,
        ) {
            Ok(released) => released,
            Err(error) => {
                chunk_pump.close();
                self.hud.clear();
                return Err(error.context(format!(
                    "streaming ASR session {utterance_id} failed before release"
                )));
            }
        };

        if !released {
            chunk_pump.close();
            self.hud.clear();
            return Ok(());
        }

        let release_started = Instant::now();
        chunk_pump.close();
        let release_grace_decision =
            preview.dynamic_release_grace(Duration::from_millis(self.config.asr.release_grace_ms));
        match release_grace_decision {
            DynamicReleaseGraceDecision::Skip => {
                info!(
                    utterance_id,
                    session_id = %asr_session.session_id,
                    partial_updates = preview.partial_updates,
                    last_partial_age_ms = preview
                        .last_partial_at
                        .map(|instant| instant.elapsed().as_millis())
                        .unwrap_or_default(),
                    "streaming ASR release late partial drain skipped"
                );
            }
            DynamicReleaseGraceDecision::Drain(release_grace) => {
                info!(
                    utterance_id,
                    session_id = %asr_session.session_id,
                    release_grace_ms = release_grace.as_millis(),
                    configured_release_grace_ms = self.config.asr.release_grace_ms,
                    partial_updates = preview.partial_updates,
                    last_partial_age_ms = preview
                        .last_partial_at
                        .map(|instant| instant.elapsed().as_millis())
                        .unwrap_or_default(),
                    "streaming ASR release late partial drain enabled"
                );
                if let Err(error) = self.drain_streaming_chunk_events_for(
                    &chunk_pump,
                    &mut preview,
                    &mut prewrite,
                    started_at,
                    &utterance_id,
                    &asr_session.session_id,
                    release_grace,
                ) {
                    warn!(
                        utterance_id,
                        session_id = %asr_session.session_id,
                        error = %error,
                        "streaming ASR late partial drain failed; using current HUD snapshot"
                    );
                }
            }
        }
        let sent_audio_ms = preview.sent_audio_ms(asr_session.sample_rate_hz.max(1) as u32);
        if preview.release_snapshot().is_empty()
            && sent_audio_ms >= STREAMING_EMPTY_SNAPSHOT_MIN_AUDIO_MS
        {
            let finish_grace = Duration::from_millis(STREAMING_EMPTY_SNAPSHOT_FINISH_GRACE_MS);
            info!(
                utterance_id,
                session_id = %asr_session.session_id,
                audio_ms = sent_audio_ms,
                finish_grace_ms = finish_grace.as_millis(),
                "streaming ASR empty snapshot; waiting briefly for finish text fallback"
            );
            if let Err(error) = self.drain_streaming_chunk_events_for(
                &chunk_pump,
                &mut preview,
                &mut prewrite,
                started_at,
                &utterance_id,
                &asr_session.session_id,
                finish_grace,
            ) {
                warn!(
                    utterance_id,
                    session_id = %asr_session.session_id,
                    error = %error,
                    "streaming ASR finish text fallback drain failed; using current HUD snapshot"
                );
            }
        }
        let snapshot = preview.release_snapshot();
        let finalized = finalize_asr_text_for_paste(&snapshot);
        let paste_snapshot = finalized.text.as_str();
        let streaming_rewrite_enabled = self.rewrite_language.streaming_rewrite_enabled();
        let (prewrite_trace, prewrite_finished_at, prewrite_status) =
            prewrite.take_trace_for_release(&snapshot);
        let sent_rms_dbfs = preview.sent_rms_dbfs();
        let sent_peak_dbfs = preview.sent_peak_dbfs();
        info!(
            utterance_id,
            session_id = %asr_session.session_id,
            snapshot_chars = snapshot.chars().count(),
            paste_snapshot_chars = paste_snapshot.chars().count(),
            partial_updates = preview.partial_updates,
            audio_ms = sent_audio_ms,
            rms_dbfs = sent_rms_dbfs,
            peak_dbfs = sent_peak_dbfs,
            first_audio_sent = preview.first_audio_sent,
            first_audio_sent_age_ms = preview
                .first_audio_sent_at
                .map(|instant| instant.elapsed().as_millis())
                .unwrap_or_default(),
            first_partial_age_ms = preview
                .first_partial_at
                .map(|instant| instant.elapsed().as_millis())
                .unwrap_or_default(),
            last_partial_age_ms = preview
                .last_partial_at
                .map(|instant| instant.elapsed().as_millis())
                .unwrap_or_default(),
            finalizer_actions = %finalized.actions,
            boost_source = %asr_session.boost_source.as_deref().unwrap_or("unknown"),
            boost_phrases = asr_session.boost_phrases.unwrap_or_default(),
            speech_context_phrases = asr_session.speech_context_phrases.unwrap_or_default(),
            speech_context_limit = asr_session.speech_context_limit.unwrap_or_default(),
            total_elapsed_ms = started_at.elapsed().as_millis(),
            "streaming ASR release observed"
        );

        if paste_snapshot.is_empty() {
            self.hud.clear();
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "streaming_asr");
            record.raw_text = snapshot.clone();
            record.finalized_text = finalized.text.clone();
            record.finalizer_actions = finalized.actions.clone();
            record.partial_updates = preview.partial_updates;
            record.audio_ms = sent_audio_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = "empty_hud_snapshot".to_string();
            stamp_rewrite_session(&mut record, streaming_rewrite_enabled);
            self.history.record(record);
            if self.debug_panel.is_enabled() {
                self.debug_panel.display_result(
                    "",
                    format!(
                        "Parakeet 流式 | 无识别文本 | partial={} | audio_ms={} | rms_dbfs={:.1} | peak_dbfs={:.1} | 总耗时={} ms",
                        preview.partial_updates,
                        sent_audio_ms,
                        sent_rms_dbfs,
                        sent_peak_dbfs,
                        started_at.elapsed().as_millis()
                    ),
                );
            }
            info!(
                utterance_id,
                session_id = %asr_session.session_id,
                release_to_discard_ms = release_started.elapsed().as_millis(),
                total_elapsed_ms = started_at.elapsed().as_millis(),
                audio_ms = sent_audio_ms,
                rms_dbfs = sent_rms_dbfs,
                peak_dbfs = sent_peak_dbfs,
                mode = "streaming_asr",
                "streaming ASR release discarded empty HUD snapshot"
            );
            return Ok(());
        }

        if self.debug_panel.is_enabled() {
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "streaming_asr");
            record.raw_text = snapshot.clone();
            record.finalized_text = finalized.text.clone();
            record.pasted_text = paste_snapshot.to_string();
            record.finalizer_actions = finalized.actions.clone();
            record.partial_updates = preview.partial_updates;
            record.audio_ms = sent_audio_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = if streaming_rewrite_enabled {
                "debug_raw_display_before_async_rewrite".to_string()
            } else {
                "debug_raw_display_rewrite_disabled".to_string()
            };
            stamp_rewrite_session(&mut record, streaming_rewrite_enabled);
            self.history.record(record);
            self.hud.show_text(paste_snapshot, false, false);
            if streaming_rewrite_enabled {
                self.debug_panel.display_result(
                    paste_snapshot,
                    format!(
                        "Parakeet 流式 | 原文已显示，改写中 | partial={} | 总耗时={} ms | finalizer={}",
                        preview.partial_updates,
                        started_at.elapsed().as_millis(),
                        finalized.actions
                    ),
                );
                self.spawn_async_streaming_rewrite(AsyncStreamingRewriteJob {
                    utterance_id: utterance_id.clone(),
                    profile_id: profile_id.as_str().to_string(),
                    raw_text: snapshot.clone(),
                    raw_pasted_text: finalized.text.clone(),
                    rewrite_enabled: streaming_rewrite_enabled,
                    output_language,
                    audio_ms: sent_audio_ms,
                    started_at,
                    partial_updates: preview.partial_updates,
                    target_summary: None,
                    target_fingerprint: None,
                    target_context_source: "debug_panel".to_string(),
                    target_right_context: "unknown".to_string(),
                    output_config: self.config.output.clone(),
                    debug_mode: true,
                    prewrite_trace: prewrite_trace.clone(),
                    prewrite_finished_at: prewrite_finished_at.clone(),
                    prewrite_status: prewrite_status.clone(),
                });
            } else {
                self.debug_panel.display_result(
                    paste_snapshot,
                    format!(
                        "Parakeet 流式 | 完成，AI改写关闭 | partial={} | 总耗时={} ms | finalizer={}",
                        preview.partial_updates,
                        started_at.elapsed().as_millis(),
                        finalized.actions
                    ),
                );
            }
            info!(
                utterance_id,
                session_id = %asr_session.session_id,
                raw_snapshot = %short_text(&snapshot, 500),
                text = %short_text(&paste_snapshot, 500),
                partial_updates = preview.partial_updates,
                audio_ms = sent_audio_ms,
                rms_dbfs = sent_rms_dbfs,
                peak_dbfs = sent_peak_dbfs,
                finalizer_actions = %finalized.actions,
                rewrite_enabled = streaming_rewrite_enabled,
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "streaming_asr",
                "streaming ASR debug result displayed without paste"
            );
            return Ok(());
        }
        if !streaming_rewrite_enabled {
            let paste_outcome = match output::paste_text_with_trace(
                paste_snapshot,
                &self.config.output,
                &utterance_id,
            ) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let mut record =
                        HistoryRecord::new(&utterance_id, profile_id.as_str(), "streaming_asr");
                    record.raw_text = snapshot.clone();
                    record.finalized_text = finalized.text.clone();
                    record.finalizer_actions = finalized.actions.clone();
                    record.partial_updates = preview.partial_updates;
                    record.audio_ms = sent_audio_ms;
                    record.total_elapsed_ms = started_at.elapsed().as_millis();
                    record.error = format!("paste failed: {error}");
                    self.history.record(record);
                    self.hud.show_text("已复制", false, false);
                    return Err(error.context("paste streaming ASR HUD snapshot"));
                }
            };
            self.hud.show_text(&paste_outcome.text, false, false);
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "streaming_asr");
            record.raw_text = snapshot.clone();
            record.finalized_text = finalized.text.clone();
            record.pasted_text = paste_outcome.text.clone();
            record.target_process = paste_outcome.target_summary.process_name.clone();
            record.target_class = paste_outcome.target_summary.class_name.clone();
            record.target_title = paste_outcome.target_summary.title.clone();
            record.target_context_source = paste_outcome.target_context.source.to_string();
            record.target_right_context = paste_outcome.target_context.right.as_str().to_string();
            record.finalizer_actions = finalized.actions.clone();
            record.output_actions = paste_outcome.text_actions.clone();
            record.partial_updates = preview.partial_updates;
            record.audio_ms = sent_audio_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.phase_timings = phase_timings(
                Some(sent_audio_ms as u128),
                None,
                None,
                Some(release_started.elapsed().as_millis()),
                None,
                record.total_elapsed_ms,
            );
            record.skipped_reason = "streaming_rewrite_disabled_raw_paste".to_string();
            self.history.record(record);
            info!(
                utterance_id,
                session_id = %asr_session.session_id,
                raw_snapshot = %short_text(&snapshot, 500),
                text = %short_text(&paste_outcome.text, 500),
                partial_updates = preview.partial_updates,
                audio_ms = sent_audio_ms,
                rms_dbfs = sent_rms_dbfs,
                peak_dbfs = sent_peak_dbfs,
                first_audio_sent = preview.first_audio_sent,
                first_audio_sent_age_ms = preview
                    .first_audio_sent_at
                    .map(|instant| instant.elapsed().as_millis())
                    .unwrap_or_default(),
                first_partial_age_ms = preview
                    .first_partial_at
                    .map(|instant| instant.elapsed().as_millis())
                    .unwrap_or_default(),
                finalizer_actions = %finalized.actions,
                target_text_actions = %paste_outcome.text_actions,
                target_right_context = paste_outcome.target_context.right.as_str(),
                target_context_source = paste_outcome.target_context.source,
                boost_source = %asr_session.boost_source.as_deref().unwrap_or("unknown"),
                boost_phrases = asr_session.boost_phrases.unwrap_or_default(),
                speech_context_phrases = asr_session.speech_context_phrases.unwrap_or_default(),
                speech_context_limit = asr_session.speech_context_limit.unwrap_or_default(),
                rewrite_enabled = false,
                release_to_paste_done_ms = release_started.elapsed().as_millis(),
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "streaming_asr",
                "streaming ASR HUD snapshot pasted on release"
            );
            return Ok(());
        }

        let output_target = output::capture_output_target(&self.config.output.rewrite_terminal_allowlist);
        if output_target.route == output::RewriteOutputRoute::HudFirstFinalPaste {
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "streaming_asr");
            record.raw_text = snapshot.clone();
            record.finalized_text = finalized.text.clone();
            record.target_process = output_target.summary.process_name.clone();
            record.target_class = output_target.summary.class_name.clone();
            record.target_title = output_target.summary.title.clone();
            record.target_context_source = output_target.context.source.to_string();
            record.target_right_context = output_target.context.right.as_str().to_string();
            record.finalizer_actions = finalized.actions.clone();
            record.output_actions =
                format!("rewrite_output_route:{}", output_target.route.as_str());
            record.partial_updates = preview.partial_updates;
            record.audio_ms = sent_audio_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = "hud_first_before_async_streaming_rewrite".to_string();
            self.history.record(record);
            self.hud.show_text(paste_snapshot, true, false);
            self.spawn_hud_first_streaming_rewrite(HudFirstStreamingRewriteJob {
                utterance_id: utterance_id.clone(),
                profile_id: profile_id.as_str().to_string(),
                raw_text: snapshot.clone(),
                raw_paste_fallback: finalized.text.clone(),
                rewrite_enabled: streaming_rewrite_enabled,
                output_language,
                audio_ms: sent_audio_ms,
                started_at,
                partial_updates: preview.partial_updates,
                target: output_target.clone(),
                output_config: self.config.output.clone(),
                prewrite_trace: prewrite_trace.clone(),
                prewrite_status: prewrite_status.clone(),
            });
            info!(
                utterance_id,
                session_id = %asr_session.session_id,
                raw_snapshot = %short_text(&snapshot, 500),
                text = %short_text(paste_snapshot, 500),
                partial_updates = preview.partial_updates,
                audio_ms = sent_audio_ms,
                rms_dbfs = sent_rms_dbfs,
                peak_dbfs = sent_peak_dbfs,
                finalizer_actions = %finalized.actions,
                target_process = %output_target.summary.process_name,
                target_class = %output_target.summary.class_name,
                target_right_context = output_target.context.right.as_str(),
                target_context_source = output_target.context.source,
                rewrite_output_route = %output_target.route.as_str(),
                rewrite_enabled = true,
                release_to_paste_done_ms = release_started.elapsed().as_millis(),
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "streaming_asr",
                "streaming ASR raw text held in HUD; async rewrite scheduled for fallback final paste"
            );
            return Ok(());
        }

        let paste_outcome = match output::paste_text_to_target_with_trace(
            paste_snapshot,
            &output_target,
            &self.config.output,
            &utterance_id,
            output::TargetPunctuationPolicy::TargetAware,
            output::TargetMatchPolicy::BestEffort,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                let mut record =
                    HistoryRecord::new(&utterance_id, profile_id.as_str(), "streaming_asr");
                record.raw_text = snapshot.clone();
                record.finalized_text = finalized.text.clone();
                record.finalizer_actions = finalized.actions.clone();
                record.partial_updates = preview.partial_updates;
                record.audio_ms = sent_audio_ms;
                record.total_elapsed_ms = started_at.elapsed().as_millis();
                record.error = format!("paste failed: {error}");
                self.history.record(record);
                self.hud.show_text("已复制", false, false);
                return Err(error.context("paste streaming ASR HUD snapshot"));
            }
        };
        self.hud.show_text("已上屏，改写中...", true, false);
        let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "streaming_asr");
        record.raw_text = snapshot.clone();
        record.finalized_text = finalized.text.clone();
        record.pasted_text = paste_outcome.text.clone();
        record.target_process = paste_outcome.target_summary.process_name.clone();
        record.target_class = paste_outcome.target_summary.class_name.clone();
        record.target_title = paste_outcome.target_summary.title.clone();
        record.target_context_source = paste_outcome.target_context.source.to_string();
        record.target_right_context = paste_outcome.target_context.right.as_str().to_string();
        record.finalizer_actions = finalized.actions.clone();
        record.output_actions = paste_outcome.text_actions.clone();
        record.partial_updates = preview.partial_updates;
        record.audio_ms = sent_audio_ms;
        record.total_elapsed_ms = started_at.elapsed().as_millis();
        record.phase_timings = phase_timings(
            Some(sent_audio_ms as u128),
            None,
            None,
            Some(release_started.elapsed().as_millis()),
            None,
            record.total_elapsed_ms,
        );
        record.skipped_reason = "raw_paste_before_async_streaming_rewrite".to_string();
        self.history.record(record);
        self.spawn_async_streaming_rewrite(AsyncStreamingRewriteJob {
            utterance_id: utterance_id.clone(),
            profile_id: profile_id.as_str().to_string(),
            raw_text: snapshot.clone(),
            raw_pasted_text: paste_outcome.text.clone(),
            rewrite_enabled: streaming_rewrite_enabled,
            output_language,
            audio_ms: sent_audio_ms,
            started_at,
            partial_updates: preview.partial_updates,
            target_summary: Some(paste_outcome.target_summary.clone()),
            target_fingerprint: Some(paste_outcome.target_fingerprint.clone()),
            target_context_source: paste_outcome.target_context.source.to_string(),
            target_right_context: paste_outcome.target_context.right.as_str().to_string(),
            output_config: self.config.output.clone(),
            debug_mode: false,
            prewrite_trace,
            prewrite_finished_at,
            prewrite_status,
        });
        info!(
            utterance_id,
            session_id = %asr_session.session_id,
            raw_snapshot = %short_text(&snapshot, 500),
            text = %short_text(&paste_outcome.text, 500),
            partial_updates = preview.partial_updates,
            audio_ms = sent_audio_ms,
            rms_dbfs = sent_rms_dbfs,
            peak_dbfs = sent_peak_dbfs,
            finalizer_actions = %finalized.actions,
            target_text_actions = %paste_outcome.text_actions,
            target_right_context = paste_outcome.target_context.right.as_str(),
            target_context_source = paste_outcome.target_context.source,
            rewrite_output_route = %output_target.route.as_str(),
            rewrite_enabled = true,
            release_to_paste_done_ms = release_started.elapsed().as_millis(),
            total_elapsed_ms = started_at.elapsed().as_millis(),
            mode = "streaming_asr",
            "streaming ASR raw text pasted; async rewrite scheduled"
        );
        Ok(())
    }

    fn run_whisper_session(
        &self,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        profile_id: VoiceProfileId,
    ) -> Result<()> {
        let utterance_id = next_utterance_id();
        let started_at = Instant::now();
        self.hud.show_active();
        let audio = self.audio.subscribe(self.config.asr.pre_roll_ms);
        let sample_rate_hz = self.config.whisper.sample_rate_hz.max(1);
        let mut resampler = LinearResampler::new(self.audio.sample_rate_hz, sample_rate_hz);
        let mut samples = Vec::<f32>::new();
        info!(
            utterance_id,
            input_sample_rate_hz = self.audio.sample_rate_hz,
            whisper_sample_rate_hz = sample_rate_hz,
            pre_roll_ms = self.config.asr.pre_roll_ms,
            mode = "whisper_zh",
            "Whisper zh session started"
        );

        let released = self.whisper_hold_loop(
            hotkey_rx,
            &audio.rx,
            &mut resampler,
            &mut samples,
            profile_id,
        )?;
        if !released {
            self.hud.clear();
            return Ok(());
        }
        self.drain_whisper_release_audio(&audio.rx, &mut resampler, &mut samples);
        drop(audio);
        self.hud.show_text("识别中...", true, false);

        let audio_ms = audio_ms(samples.len(), sample_rate_hz);
        let rms_dbfs = rms_dbfs(&samples);
        if audio_ms < self.config.whisper.min_audio_ms
            || rms_dbfs < self.config.whisper.min_rms_dbfs
        {
            let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
            record.audio_ms = audio_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = format!("audio_gate:rms_dbfs={rms_dbfs:.1}");
            self.history.record(record);
            info!(
                utterance_id,
                audio_ms,
                rms_dbfs,
                min_audio_ms = self.config.whisper.min_audio_ms,
                min_rms_dbfs = self.config.whisper.min_rms_dbfs,
                mode = "whisper_zh",
                "Whisper zh session skipped because audio gate did not pass"
            );
            if self.debug_panel.is_enabled() {
                self.debug_panel.display_result(
                    "",
                    format!(
                        "Whisper 非流式 | 跳过：音频太短或音量太低 | audio_ms={} | rms_dbfs={:.1}",
                        audio_ms, rms_dbfs
                    ),
                );
            }
            self.hud.clear();
            return Ok(());
        }

        let transcribe_started = Instant::now();
        let response = self
            .whisper
            .transcribe_zh(&samples)
            .context("transcribe with cloud Whisper zh")?;
        let raw_text = prepare_asr_text(&response.text);
        if is_whisper_short_hallucination(&raw_text, response.audio_ms) {
            let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
            record.raw_text = raw_text.clone();
            record.audio_ms = response.audio_ms;
            record.asr_elapsed_ms = response.elapsed_ms.max(0.0) as u128;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = "short_hallucination".to_string();
            self.history.record(record);
            info!(
                utterance_id,
                audio_ms = response.audio_ms,
                raw_text = %short_text(&raw_text, 160),
                mode = "whisper_zh",
                "Whisper zh session skipped because result matches short hallucination guard"
            );
            self.hud.clear();
            return Ok(());
        }
        let output_language = self.rewrite_language.current();
        let rewrite_enabled = self.rewrite_language.rewrite_enabled();
        let raw_finalized =
            finalize_asr_text_for_paste_for_language(&raw_text, RewriteOutputLanguage::Chinese);
        let raw_text_for_paste = raw_finalized.text.as_str();
        let asr_elapsed_ms = response.elapsed_ms.max(0.0) as u128;
        if response.skipped || raw_text_for_paste.is_empty() {
            let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.audio_ms = response.audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = format!("empty_or_skipped:{}", response.skipped);
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            if self.debug_panel.is_enabled() {
                self.debug_panel.display_result(
                    "",
                    format!(
                        "Whisper 非流式 | 无识别文本 | audio_ms={} | elapsed_ms={} | skipped={}",
                        response.audio_ms, response.elapsed_ms, response.skipped
                    ),
                );
            }
            info!(
                utterance_id,
                audio_ms = response.audio_ms,
                elapsed_ms = response.elapsed_ms,
                language = %response.language.as_deref().unwrap_or("unknown"),
                model = %response.model,
                skipped = response.skipped,
                transcribe_ms = transcribe_started.elapsed().as_millis(),
                mode = "whisper_zh",
                "Whisper zh returned no text"
            );
            self.hud.clear();
            return Ok(());
        }
        if self.debug_panel.is_enabled() {
            let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.audio_ms = response.audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = if rewrite_enabled {
                "debug_raw_display_before_async_rewrite".to_string()
            } else {
                "debug_raw_display_rewrite_disabled".to_string()
            };
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            self.hud.show_text(raw_text_for_paste, false, false);
            if !rewrite_enabled {
                self.debug_panel.display_result(
                    raw_text_for_paste,
                    format!(
                        "Whisper 非流式 | 原文已显示，AI改写关闭 | audio_ms={} | elapsed_ms={} | transcribe_ms={} ms",
                        response.audio_ms,
                        response.elapsed_ms,
                        transcribe_started.elapsed().as_millis()
                    ),
                );
                info!(
                    utterance_id,
                    audio_ms = response.audio_ms,
                    elapsed_ms = response.elapsed_ms,
                    language = %response.language.as_deref().unwrap_or("unknown"),
                    model = %response.model,
                    raw_text = %short_text(&raw_text, 500),
                    text = %short_text(raw_text_for_paste, 500),
                    finalizer_actions = %raw_finalized.actions,
                    transcribe_ms = transcribe_started.elapsed().as_millis(),
                    total_elapsed_ms = started_at.elapsed().as_millis(),
                    mode = "whisper_zh",
                    "Whisper zh raw debug result displayed; AI rewrite disabled"
                );
                return Ok(());
            }
            self.debug_panel.display_result(
                raw_text_for_paste,
                format!(
                    "Whisper 非流式 | 原文已显示，改写中 | audio_ms={} | elapsed_ms={} | transcribe_ms={} ms",
                    response.audio_ms,
                    response.elapsed_ms,
                    transcribe_started.elapsed().as_millis()
                ),
            );
            self.spawn_async_whisper_rewrite(AsyncWhisperRewriteJob {
                utterance_id: utterance_id.clone(),
                profile_id: profile_id.as_str().to_string(),
                raw_text: raw_text.clone(),
                raw_pasted_text: raw_finalized.text.clone(),
                rewrite_enabled,
                output_language,
                system_prompt: self.rewrite_prompt.active_prompt(),
                audio_ms: response.audio_ms,
                asr_elapsed_ms,
                started_at,
                target_summary: None,
                target_fingerprint: None,
                target_context_source: "debug_panel".to_string(),
                target_right_context: "unknown".to_string(),
                output_config: self.config.output.clone(),
                debug_mode: true,
                silent_hud: false,
            });
            info!(
                utterance_id,
                audio_ms = response.audio_ms,
                elapsed_ms = response.elapsed_ms,
                language = %response.language.as_deref().unwrap_or("unknown"),
                model = %response.model,
                raw_text = %short_text(&raw_text, 500),
                text = %short_text(raw_text_for_paste, 500),
                finalizer_actions = %raw_finalized.actions,
                transcribe_ms = transcribe_started.elapsed().as_millis(),
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "whisper_zh",
                "Whisper zh raw debug result displayed; async rewrite scheduled"
            );
            return Ok(());
        }
        let output_target = output::capture_output_target(&self.config.output.rewrite_terminal_allowlist);
        if !rewrite_enabled {
            let paste_outcome = match output::paste_text_to_target_with_trace(
                raw_text_for_paste,
                &output_target,
                &self.config.output,
                &utterance_id,
                output::TargetPunctuationPolicy::TargetAware,
                output::TargetMatchPolicy::BestEffort,
            ) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let mut record =
                        HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
                    record.raw_text = raw_text.clone();
                    record.finalized_text = raw_finalized.text.clone();
                    record.finalizer_actions = raw_finalized.actions.clone();
                    record.audio_ms = response.audio_ms;
                    record.asr_elapsed_ms = asr_elapsed_ms;
                    record.total_elapsed_ms = started_at.elapsed().as_millis();
                    record.error = format!("paste failed: {error}");
                    stamp_rewrite_session(&mut record, rewrite_enabled);
                    self.history.record(record);
                    self.hud.show_text("已复制", false, false);
                    return Err(error.context("paste Whisper zh text"));
                }
            };
            let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.pasted_text = paste_outcome.text.clone();
            record.target_process = paste_outcome.target_summary.process_name.clone();
            record.target_class = paste_outcome.target_summary.class_name.clone();
            record.target_title = paste_outcome.target_summary.title.clone();
            record.target_context_source = paste_outcome.target_context.source.to_string();
            record.target_right_context = paste_outcome.target_context.right.as_str().to_string();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.output_actions = paste_outcome.text_actions.clone();
            record.audio_ms = response.audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.phase_timings = phase_timings(
                Some(response.audio_ms as u128),
                Some(asr_elapsed_ms),
                None,
                Some(
                    started_at
                        .elapsed()
                        .as_millis()
                        .saturating_sub(asr_elapsed_ms),
                ),
                None,
                record.total_elapsed_ms,
            );
            record.skipped_reason = "rewrite_disabled_raw_paste".to_string();
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            self.hud.clear();
            info!(
                utterance_id,
                audio_ms = response.audio_ms,
                elapsed_ms = response.elapsed_ms,
                language = %response.language.as_deref().unwrap_or("unknown"),
                model = %response.model,
                text = %short_text(&paste_outcome.text, 500),
                raw_text = %short_text(&raw_text, 500),
                finalizer_actions = %raw_finalized.actions,
                target_text_actions = %paste_outcome.text_actions,
                target_right_context = paste_outcome.target_context.right.as_str(),
                target_context_source = paste_outcome.target_context.source,
                rewrite_output_route = %output_target.route.as_str(),
                transcribe_ms = transcribe_started.elapsed().as_millis(),
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "whisper_zh",
                "Whisper zh raw text pasted; AI rewrite disabled"
            );
            return Ok(());
        }

        if output_target.route == output::RewriteOutputRoute::HudFirstFinalPaste {
            let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.target_process = output_target.summary.process_name.clone();
            record.target_class = output_target.summary.class_name.clone();
            record.target_title = output_target.summary.title.clone();
            record.target_context_source = output_target.context.source.to_string();
            record.target_right_context = output_target.context.right.as_str().to_string();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.output_actions =
                format!("rewrite_output_route:{}", output_target.route.as_str());
            record.audio_ms = response.audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = "hud_first_before_async_rewrite".to_string();
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            self.hud.show_text(raw_text_for_paste, true, false);
            self.spawn_hud_first_whisper_rewrite(HudFirstWhisperRewriteJob {
                utterance_id: utterance_id.clone(),
                profile_id: profile_id.as_str().to_string(),
                raw_text: raw_text.clone(),
                raw_paste_fallback: raw_finalized.text.clone(),
                rewrite_enabled,
                output_language,
                system_prompt: self.rewrite_prompt.active_prompt(),
                audio_ms: response.audio_ms,
                asr_elapsed_ms,
                started_at,
                target: output_target.clone(),
                output_config: self.config.output.clone(),
                silent_hud: false,
            });
            info!(
                utterance_id,
                audio_ms = response.audio_ms,
                elapsed_ms = response.elapsed_ms,
                language = %response.language.as_deref().unwrap_or("unknown"),
                model = %response.model,
                text = %short_text(raw_text_for_paste, 500),
                raw_text = %short_text(&raw_text, 500),
                finalizer_actions = %raw_finalized.actions,
                target_process = %output_target.summary.process_name,
                target_class = %output_target.summary.class_name,
                target_right_context = output_target.context.right.as_str(),
                target_context_source = output_target.context.source,
                rewrite_output_route = %output_target.route.as_str(),
                transcribe_ms = transcribe_started.elapsed().as_millis(),
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "whisper_zh",
                "Whisper zh raw text held in HUD; async rewrite scheduled for fallback final paste"
            );
            return Ok(());
        }

        let paste_outcome = match output::paste_text_to_target_with_trace(
            raw_text_for_paste,
            &output_target,
            &self.config.output,
            &utterance_id,
            output::TargetPunctuationPolicy::TargetAware,
            output::TargetMatchPolicy::BestEffort,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                let mut record =
                    HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
                record.raw_text = raw_text.clone();
                record.finalized_text = raw_finalized.text.clone();
                record.finalizer_actions = raw_finalized.actions.clone();
                record.audio_ms = response.audio_ms;
                record.asr_elapsed_ms = asr_elapsed_ms;
                record.total_elapsed_ms = started_at.elapsed().as_millis();
                record.error = format!("paste failed: {error}");
                stamp_rewrite_session(&mut record, rewrite_enabled);
                self.history.record(record);
                self.hud.show_text("已复制", false, false);
                return Err(error.context("paste Whisper zh text"));
            }
        };
        let mut record = HistoryRecord::new(&utterance_id, profile_id.as_str(), "whisper_zh");
        record.raw_text = raw_text.clone();
        record.finalized_text = raw_finalized.text.clone();
        record.pasted_text = paste_outcome.text.clone();
        record.target_process = paste_outcome.target_summary.process_name.clone();
        record.target_class = paste_outcome.target_summary.class_name.clone();
        record.target_title = paste_outcome.target_summary.title.clone();
        record.target_context_source = paste_outcome.target_context.source.to_string();
        record.target_right_context = paste_outcome.target_context.right.as_str().to_string();
        record.finalizer_actions = raw_finalized.actions.clone();
        record.output_actions = paste_outcome.text_actions.clone();
        record.audio_ms = response.audio_ms;
        record.asr_elapsed_ms = asr_elapsed_ms;
        record.total_elapsed_ms = started_at.elapsed().as_millis();
        record.phase_timings = phase_timings(
            Some(response.audio_ms as u128),
            Some(asr_elapsed_ms),
            None,
            Some(
                started_at
                    .elapsed()
                    .as_millis()
                    .saturating_sub(asr_elapsed_ms),
            ),
            None,
            record.total_elapsed_ms,
        );
        self.hud.show_text("已上屏，改写中...", true, false);
        record.skipped_reason = "raw_paste_before_async_rewrite".to_string();
        stamp_rewrite_session(&mut record, rewrite_enabled);
        self.history.record(record);
        self.spawn_async_whisper_rewrite(AsyncWhisperRewriteJob {
            utterance_id: utterance_id.clone(),
            profile_id: profile_id.as_str().to_string(),
            raw_text: raw_text.clone(),
            raw_pasted_text: paste_outcome.text.clone(),
            rewrite_enabled,
            output_language,
            system_prompt: self.rewrite_prompt.active_prompt(),
            audio_ms: response.audio_ms,
            asr_elapsed_ms,
            started_at,
            target_summary: Some(paste_outcome.target_summary.clone()),
            target_fingerprint: Some(paste_outcome.target_fingerprint.clone()),
            target_context_source: paste_outcome.target_context.source.to_string(),
            target_right_context: paste_outcome.target_context.right.as_str().to_string(),
            output_config: self.config.output.clone(),
            debug_mode: false,
            silent_hud: false,
        });
        info!(
            utterance_id,
            audio_ms = response.audio_ms,
            elapsed_ms = response.elapsed_ms,
            language = %response.language.as_deref().unwrap_or("unknown"),
            model = %response.model,
            text = %short_text(&paste_outcome.text, 500),
            raw_text = %short_text(&raw_text, 500),
            finalizer_actions = %raw_finalized.actions,
            target_text_actions = %paste_outcome.text_actions,
            target_right_context = paste_outcome.target_context.right.as_str(),
            target_context_source = paste_outcome.target_context.source,
            rewrite_output_route = %output_target.route.as_str(),
            transcribe_ms = transcribe_started.elapsed().as_millis(),
            total_elapsed_ms = started_at.elapsed().as_millis(),
            mode = "whisper_zh",
            "Whisper zh raw text pasted; async rewrite scheduled"
        );
        Ok(())
    }

    fn run_local_nonstreaming_session(
        &self,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        profile_id: VoiceProfileId,
    ) -> Result<()> {
        let utterance_id = next_utterance_id();
        let started_at = Instant::now();
        // B′ silent particle meter (no text, no dark rect) for CapsLock local path.
        self.hud.show_meter_listening();
        let audio = self.audio.subscribe(self.config.asr.pre_roll_ms);
        let sample_rate_hz = self.config.local_nonstreaming.sample_rate_hz.max(1);
        let mut resampler = LinearResampler::new(self.audio.sample_rate_hz, sample_rate_hz);
        let mut samples = Vec::<f32>::new();
        info!(
            utterance_id,
            input_sample_rate_hz = self.audio.sample_rate_hz,
            local_sample_rate_hz = sample_rate_hz,
            pre_roll_ms = self.config.asr.pre_roll_ms,
            mode = "local_nonstreaming",
            "local non-streaming SenseVoice session started"
        );

        let released = self.whisper_hold_loop(
            hotkey_rx,
            &audio.rx,
            &mut resampler,
            &mut samples,
            profile_id,
        )?;
        if !released {
            self.hud.clear();
            return Ok(());
        }
        self.drain_release_audio(
            &audio.rx,
            &mut resampler,
            &mut samples,
            self.config.local_nonstreaming.release_grace_ms,
        );
        drop(audio);
        self.hud.show_meter_busy();

        let audio_ms = audio_ms(samples.len(), sample_rate_hz);
        let rms_dbfs = rms_dbfs(&samples);
        if audio_ms < self.config.local_nonstreaming.min_audio_ms
            || rms_dbfs < self.config.local_nonstreaming.min_rms_dbfs
        {
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "local_nonstreaming");
            record.audio_ms = audio_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = format!("audio_gate:rms_dbfs={rms_dbfs:.1}");
            self.history.record(record);
            info!(
                utterance_id,
                audio_ms,
                rms_dbfs,
                min_audio_ms = self.config.local_nonstreaming.min_audio_ms,
                min_rms_dbfs = self.config.local_nonstreaming.min_rms_dbfs,
                mode = "local_nonstreaming",
                "local non-streaming session skipped because audio gate did not pass"
            );
            if self.debug_panel.is_enabled() {
                self.debug_panel.display_result(
                    "",
                    format!(
                        "本地非流式 | 跳过：音频太短或音量太低 | audio_ms={} | rms_dbfs={:.1}",
                        audio_ms, rms_dbfs
                    ),
                );
            }
            self.hud.clear();
            return Ok(());
        }

        let recognizer = self
            .local_recognizer
            .as_ref()
            .ok_or_else(|| anyhow!("local non-streaming SenseVoice recognizer is unavailable"))?;
        let transcribe_started = Instant::now();
        let response = recognizer
            .transcribe_samples(sample_rate_hz, &samples)
            .context("transcribe with local SenseVoice")?;
        let asr_elapsed_ms = transcribe_started.elapsed().as_millis();
        let raw_text = prepare_asr_text(&response.text);
        let output_language = self.rewrite_language.current();
        let rewrite_enabled = self.rewrite_language.rewrite_enabled();

        // Voice command: "老蔡老蔡 …" → generate, not dictation rewrite.
        if self.voice_command.enabled() {
            let wake = self.voice_command.active_wake_phrase();
            if let Some(command) = voice_command::parse_voice_command_with(&raw_text, &wake) {
                return self.handle_voice_command(
                    &utterance_id,
                    profile_id,
                    &raw_text,
                    &command.instruction,
                    audio_ms,
                    asr_elapsed_ms,
                    started_at,
                );
            }
        }

        let raw_finalized =
            finalize_asr_text_for_paste_for_language(&raw_text, RewriteOutputLanguage::Chinese);
        let raw_text_for_paste = raw_finalized.text.as_str();
        if raw_text_for_paste.is_empty() {
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "local_nonstreaming");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.audio_ms = audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = "empty_result".to_string();
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            info!(
                utterance_id,
                audio_ms,
                model_root = %response.model_root.display(),
                transcribe_ms = asr_elapsed_ms,
                mode = "local_nonstreaming",
                "local non-streaming returned no text"
            );
            self.hud.clear();
            return Ok(());
        }

        if self.debug_panel.is_enabled() {
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "local_nonstreaming");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.audio_ms = audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = if rewrite_enabled {
                "debug_raw_display_before_async_rewrite".to_string()
            } else {
                "debug_raw_display_rewrite_disabled".to_string()
            };
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            self.hud.show_text(raw_text_for_paste, false, false);
            if !rewrite_enabled {
                self.debug_panel.display_result(
                    raw_text_for_paste,
                    format!(
                        "本地非流式 | 原文已显示，AI改写关闭 | audio_ms={} | transcribe_ms={} ms",
                        audio_ms, asr_elapsed_ms
                    ),
                );
                return Ok(());
            }
            self.debug_panel.display_result(
                raw_text_for_paste,
                format!(
                    "本地非流式 | 原文已显示，改写中 | audio_ms={} | transcribe_ms={} ms",
                    audio_ms, asr_elapsed_ms
                ),
            );
            self.spawn_async_nonstreaming_rewrite(
                AsyncWhisperRewriteJob {
                    utterance_id: utterance_id.clone(),
                    profile_id: profile_id.as_str().to_string(),
                    raw_text: raw_text.clone(),
                    raw_pasted_text: raw_finalized.text.clone(),
                    rewrite_enabled,
                    output_language,
                    system_prompt: self.rewrite_prompt.active_prompt(),
                    audio_ms,
                    asr_elapsed_ms,
                    started_at,
                    target_summary: None,
                    target_fingerprint: None,
                    target_context_source: "debug_panel".to_string(),
                    target_right_context: "unknown".to_string(),
                    output_config: self.config.output.clone(),
                    debug_mode: true,
                    silent_hud: false, // debug panel keeps normal HUD text
                },
                "local_nonstreaming_async_rewrite",
                "本地非流式",
            );
            return Ok(());
        }

        let output_target = output::capture_output_target(&self.config.output.rewrite_terminal_allowlist);
        if !rewrite_enabled {
            let paste_outcome = match output::paste_text_to_target_with_trace(
                raw_text_for_paste,
                &output_target,
                &self.config.output,
                &utterance_id,
                output::TargetPunctuationPolicy::TargetAware,
                output::TargetMatchPolicy::BestEffort,
            ) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let mut record = HistoryRecord::new(
                        &utterance_id,
                        profile_id.as_str(),
                        "local_nonstreaming",
                    );
                    record.raw_text = raw_text.clone();
                    record.finalized_text = raw_finalized.text.clone();
                    record.finalizer_actions = raw_finalized.actions.clone();
                    record.audio_ms = audio_ms;
                    record.asr_elapsed_ms = asr_elapsed_ms;
                    record.total_elapsed_ms = started_at.elapsed().as_millis();
                    record.error = format!("paste failed: {error}");
                    stamp_rewrite_session(&mut record, rewrite_enabled);
                    self.history.record(record);
                    // B′: no "已复制" chip — just fade meter.
                    self.hud.clear();
                    return Err(error.context("paste local non-streaming text"));
                }
            };
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "local_nonstreaming");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.pasted_text = paste_outcome.text.clone();
            record.target_process = paste_outcome.target_summary.process_name.clone();
            record.target_class = paste_outcome.target_summary.class_name.clone();
            record.target_title = paste_outcome.target_summary.title.clone();
            record.target_context_source = paste_outcome.target_context.source.to_string();
            record.target_right_context = paste_outcome.target_context.right.as_str().to_string();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.output_actions = paste_outcome.text_actions.clone();
            record.audio_ms = audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.phase_timings = phase_timings(
                Some(audio_ms as u128),
                Some(asr_elapsed_ms),
                None,
                Some(
                    started_at
                        .elapsed()
                        .as_millis()
                        .saturating_sub(asr_elapsed_ms),
                ),
                None,
                record.total_elapsed_ms,
            );
            record.skipped_reason = "rewrite_disabled_raw_paste".to_string();
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            self.hud.clear();
            info!(
                utterance_id,
                audio_ms,
                model_root = %response.model_root.display(),
                text = %short_text(&paste_outcome.text, 500),
                raw_text = %short_text(&raw_text, 500),
                finalizer_actions = %raw_finalized.actions,
                target_text_actions = %paste_outcome.text_actions,
                rewrite_output_route = %output_target.route.as_str(),
                transcribe_ms = asr_elapsed_ms,
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "local_nonstreaming",
                "local non-streaming raw text pasted; AI rewrite disabled"
            );
            return Ok(());
        }

        if output_target.route == output::RewriteOutputRoute::HudFirstFinalPaste {
            let mut record =
                HistoryRecord::new(&utterance_id, profile_id.as_str(), "local_nonstreaming");
            record.raw_text = raw_text.clone();
            record.finalized_text = raw_finalized.text.clone();
            record.target_process = output_target.summary.process_name.clone();
            record.target_class = output_target.summary.class_name.clone();
            record.target_title = output_target.summary.title.clone();
            record.target_context_source = output_target.context.source.to_string();
            record.target_right_context = output_target.context.right.as_str().to_string();
            record.finalizer_actions = raw_finalized.actions.clone();
            record.output_actions =
                format!("rewrite_output_route:{}", output_target.route.as_str());
            record.audio_ms = audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.skipped_reason = "hud_first_before_async_rewrite".to_string();
            stamp_rewrite_session(&mut record, rewrite_enabled);
            self.history.record(record);
            // B′: no raw text on HUD — keep soft particles while rewrite runs.
            self.hud.show_meter_busy();
            self.spawn_hud_first_nonstreaming_rewrite(
                HudFirstWhisperRewriteJob {
                    utterance_id: utterance_id.clone(),
                    profile_id: profile_id.as_str().to_string(),
                    raw_text: raw_text.clone(),
                    raw_paste_fallback: raw_finalized.text.clone(),
                    rewrite_enabled,
                    output_language,
                    system_prompt: self.rewrite_prompt.active_prompt(),
                    audio_ms,
                    asr_elapsed_ms,
                    started_at,
                    target: output_target.clone(),
                    output_config: self.config.output.clone(),
                    silent_hud: true,
                },
                "local_nonstreaming_hud_fallback_rewrite",
                "local_nonstreaming",
            );
            info!(
                utterance_id,
                audio_ms,
                model_root = %response.model_root.display(),
                text = %short_text(raw_text_for_paste, 500),
                raw_text = %short_text(&raw_text, 500),
                finalizer_actions = %raw_finalized.actions,
                rewrite_output_route = %output_target.route.as_str(),
                transcribe_ms = asr_elapsed_ms,
                total_elapsed_ms = started_at.elapsed().as_millis(),
                mode = "local_nonstreaming",
                "local non-streaming raw text held in HUD; async rewrite scheduled for fallback final paste"
            );
            return Ok(());
        }

        let paste_outcome = match output::paste_text_to_target_with_trace(
            raw_text_for_paste,
            &output_target,
            &self.config.output,
            &utterance_id,
            output::TargetPunctuationPolicy::TargetAware,
            output::TargetMatchPolicy::BestEffort,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                let mut record =
                    HistoryRecord::new(&utterance_id, profile_id.as_str(), "local_nonstreaming");
                record.raw_text = raw_text.clone();
                record.finalized_text = raw_finalized.text.clone();
                record.finalizer_actions = raw_finalized.actions.clone();
                record.audio_ms = audio_ms;
                record.asr_elapsed_ms = asr_elapsed_ms;
                record.total_elapsed_ms = started_at.elapsed().as_millis();
                record.error = format!("paste failed: {error}");
                stamp_rewrite_session(&mut record, rewrite_enabled);
                self.history.record(record);
                self.hud.clear();
                return Err(error.context("paste local non-streaming text"));
            }
        };
        let mut record =
            HistoryRecord::new(&utterance_id, profile_id.as_str(), "local_nonstreaming");
        record.raw_text = raw_text.clone();
        record.finalized_text = raw_finalized.text.clone();
        record.pasted_text = paste_outcome.text.clone();
        record.target_process = paste_outcome.target_summary.process_name.clone();
        record.target_class = paste_outcome.target_summary.class_name.clone();
        record.target_title = paste_outcome.target_summary.title.clone();
        record.target_context_source = paste_outcome.target_context.source.to_string();
        record.target_right_context = paste_outcome.target_context.right.as_str().to_string();
        record.finalizer_actions = raw_finalized.actions.clone();
        record.output_actions = paste_outcome.text_actions.clone();
        record.audio_ms = audio_ms;
        record.asr_elapsed_ms = asr_elapsed_ms;
        record.total_elapsed_ms = started_at.elapsed().as_millis();
        record.phase_timings = phase_timings(
            Some(audio_ms as u128),
            Some(asr_elapsed_ms),
            None,
            Some(
                started_at
                    .elapsed()
                    .as_millis()
                    .saturating_sub(asr_elapsed_ms),
            ),
            None,
            record.total_elapsed_ms,
        );
        self.hud.show_meter_busy();
        record.skipped_reason = "raw_paste_before_async_rewrite".to_string();
        stamp_rewrite_session(&mut record, rewrite_enabled);
        self.history.record(record);
        self.spawn_async_nonstreaming_rewrite(
            AsyncWhisperRewriteJob {
                utterance_id: utterance_id.clone(),
                profile_id: profile_id.as_str().to_string(),
                raw_text: raw_text.clone(),
                raw_pasted_text: paste_outcome.text.clone(),
                rewrite_enabled,
                output_language,
                system_prompt: self.rewrite_prompt.active_prompt(),
                audio_ms,
                asr_elapsed_ms,
                started_at,
                target_summary: Some(paste_outcome.target_summary.clone()),
                target_fingerprint: Some(paste_outcome.target_fingerprint.clone()),
                target_context_source: paste_outcome.target_context.source.to_string(),
                target_right_context: paste_outcome.target_context.right.as_str().to_string(),
                output_config: self.config.output.clone(),
                debug_mode: false,
                silent_hud: true,
            },
            "local_nonstreaming_async_rewrite",
            "本地非流式",
        );
        info!(
            utterance_id,
            audio_ms,
            model_root = %response.model_root.display(),
            text = %short_text(&paste_outcome.text, 500),
            raw_text = %short_text(&raw_text, 500),
            finalizer_actions = %raw_finalized.actions,
            target_text_actions = %paste_outcome.text_actions,
            rewrite_output_route = %output_target.route.as_str(),
            transcribe_ms = asr_elapsed_ms,
            total_elapsed_ms = started_at.elapsed().as_millis(),
            mode = "local_nonstreaming",
            "local non-streaming raw text pasted; async rewrite scheduled"
        );
        Ok(())
    }

    fn handle_voice_command(
        &self,
        utterance_id: &str,
        profile_id: VoiceProfileId,
        raw_text: &str,
        instruction: &str,
        audio_ms: u64,
        asr_elapsed_ms: u128,
        started_at: Instant,
    ) -> Result<()> {
        self.hud.show_meter_busy();
        info!(
            utterance_id,
            instruction = %short_text(instruction, 200),
            raw_text = %short_text(raw_text, 200),
            "voice command detected (老蔡老蔡)"
        );
        let Some(rewriter) = self.rewriter.get() else {
            let msg = "语音指令需要先配置 API / 模型";
            self.hud.show_text(msg, false, false);
            let mut record =
                HistoryRecord::new(utterance_id, profile_id.as_str(), "voice_command");
            record.raw_text = raw_text.to_string();
            record.finalized_text = instruction.to_string();
            record.audio_ms = audio_ms;
            record.asr_elapsed_ms = asr_elapsed_ms;
            record.total_elapsed_ms = started_at.elapsed().as_millis();
            record.error = "rewriter_unavailable".to_string();
            record.skipped_reason = "voice_command_no_api".to_string();
            self.history.record(record);
            return Ok(());
        };

        let command_started = Instant::now();
        let system_prompt = self.voice_command.active_prompt();
        let generated = match rewriter.generate_command(instruction, &system_prompt) {
            Ok(Some(text)) => text,
            Ok(None) => {
                self.hud.show_text("语音指令无输出", false, false);
                let mut record =
                    HistoryRecord::new(utterance_id, profile_id.as_str(), "voice_command");
                record.raw_text = raw_text.to_string();
                record.finalized_text = instruction.to_string();
                record.audio_ms = audio_ms;
                record.asr_elapsed_ms = asr_elapsed_ms;
                record.total_elapsed_ms = started_at.elapsed().as_millis();
                record.skipped_reason = "voice_command_empty".to_string();
                self.history.record(record);
                return Ok(());
            }
            Err(error) => {
                warn!(error = %error, "voice command generation failed");
                self.hud
                    .show_text(&format!("语音指令失败：{}", short_text(&error.to_string(), 80)), false, false);
                let mut record =
                    HistoryRecord::new(utterance_id, profile_id.as_str(), "voice_command");
                record.raw_text = raw_text.to_string();
                record.finalized_text = instruction.to_string();
                record.audio_ms = audio_ms;
                record.asr_elapsed_ms = asr_elapsed_ms;
                record.total_elapsed_ms = started_at.elapsed().as_millis();
                record.error = error.to_string();
                record.skipped_reason = "voice_command_error".to_string();
                self.history.record(record);
                return Ok(());
            }
        };
        let gen_ms = command_started.elapsed().as_millis();
        let output_target = output::capture_output_target(&self.config.output.rewrite_terminal_allowlist);
        let paste_outcome = match output::paste_text_to_target_with_trace(
            &generated,
            &output_target,
            &self.config.output,
            utterance_id,
            output::TargetPunctuationPolicy::Preserve,
            output::TargetMatchPolicy::BestEffort,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                let mut record =
                    HistoryRecord::new(utterance_id, profile_id.as_str(), "voice_command");
                record.raw_text = raw_text.to_string();
                record.finalized_text = generated.clone();
                record.audio_ms = audio_ms;
                record.asr_elapsed_ms = asr_elapsed_ms;
                record.total_elapsed_ms = started_at.elapsed().as_millis();
                record.error = format!("paste failed: {error}");
                record.skipped_reason = "voice_command_paste_failed".to_string();
                self.history.record(record);
                self.hud.clear();
                return Err(error.context("paste voice command text"));
            }
        };
        let mut record = HistoryRecord::new(utterance_id, profile_id.as_str(), "voice_command");
        record.raw_text = raw_text.to_string();
        record.finalized_text = generated.clone();
        record.pasted_text = paste_outcome.text.clone();
        record.target_process = paste_outcome.target_summary.process_name.clone();
        record.target_class = paste_outcome.target_summary.class_name.clone();
        record.target_title = paste_outcome.target_summary.title.clone();
        record.target_context_source = paste_outcome.target_context.source.to_string();
        record.target_right_context = paste_outcome.target_context.right.as_str().to_string();
        record.finalizer_actions = "voice_command".to_string();
        record.output_actions = paste_outcome.text_actions.clone();
        record.audio_ms = audio_ms;
        record.asr_elapsed_ms = asr_elapsed_ms;
        record.rewrite_elapsed_ms = gen_ms;
        record.rewrite_model = rewriter.model().to_string();
        record.total_elapsed_ms = started_at.elapsed().as_millis();
        record.skipped_reason = "voice_command_applied".to_string();
        self.history.record(record);
        self.hud.clear();
        info!(
            utterance_id,
            instruction = %short_text(instruction, 200),
            generated = %short_text(&generated, 300),
            gen_ms,
            total_elapsed_ms = started_at.elapsed().as_millis(),
            "voice command pasted"
        );
        Ok(())
    }

    /// Load recent dictation history as cross-utterance context for AI rewrite.
    /// Returns None when disabled (context_history_count == 0) or nothing usable.
    fn history_context_for_rewrite(&self) -> Option<String> {
        let count = self.rewriter.snapshot_config().context_history_count;
        if count == 0 {
            return None;
        }
        // Load extra records so empty ones can be skipped while still keeping
        // up to `count` non-empty entries.
        let records = crate::history::load_recent(self.history.path(), count.saturating_mul(3)).ok()?;
        let context = crate::history::format_recent_context(&records, count);
        if context.trim().is_empty() {
            None
        } else {
            Some(context)
        }
    }

    fn spawn_async_whisper_rewrite(&self, job: AsyncWhisperRewriteJob) {
        self.spawn_async_nonstreaming_rewrite(job, "whisper_zh_async_rewrite", "Whisper 非流式");
    }

    fn spawn_async_nonstreaming_rewrite(
        &self,
        job: AsyncWhisperRewriteJob,
        history_mode: &'static str,
        display_label: &'static str,
    ) {
        let history = self.history.clone();
        let hud = self.hud.clone();
        let debug_panel = self.debug_panel.clone();
        let rewriter = self.rewriter.get();
        let rewrite_min_chars = self
            .rewriter
            .snapshot_config()
            .min_chars
            .max(self.config.rewrite.min_chars);
        let history_context = self.history_context_for_rewrite();
        thread::spawn(move || {
            let silent = job.silent_hud;
            let trace = apply_whisper_rewrite_with(
                rewriter.as_ref(),
                job.rewrite_enabled,
                rewrite_min_chars,
                &hud,
                &job.raw_text,
                job.output_language,
                silent,
                Some(job.system_prompt.as_str()),
                history_context.as_deref(),
            );
            let rewrite_source = trace.output.as_deref().unwrap_or(&job.raw_text);
            let finalized =
                finalize_asr_text_for_paste_for_language(rewrite_source, job.output_language);
            let candidate = finalized.text.as_str();
            let mut record = HistoryRecord::new(&job.utterance_id, &job.profile_id, history_mode);
            record.raw_text = job.raw_text.clone();
            stamp_rewrite_session(&mut record, job.rewrite_enabled);
            apply_rewrite_trace_to_record(&mut record, &trace);
            record.finalized_text = finalized.text.clone();
            record.finalizer_actions = finalized.actions.clone();
            record.audio_ms = job.audio_ms;
            record.asr_elapsed_ms = job.asr_elapsed_ms;
            record.total_elapsed_ms = job.started_at.elapsed().as_millis();
            record.target_context_source = job.target_context_source.clone();
            record.target_right_context = job.target_right_context.clone();
            if let Some(target) = &job.target_summary {
                record.target_process = target.process_name.clone();
                record.target_class = target.class_name.clone();
                record.target_title = target.title.clone();
            }
            let replacement_outcome = async_rewrite_replacement_outcome(&trace, candidate, &job);
            if replacement_outcome.applied {
                record.pasted_text = candidate.to_string();
            }
            record.output_actions = replacement_outcome.output_actions.clone();
            record.skipped_reason = replacement_outcome.output_actions.clone();
            record.phase_timings = phase_timings(
                Some(job.audio_ms as u128),
                Some(job.asr_elapsed_ms),
                Some(trace.elapsed_ms),
                None,
                Some(trace.elapsed_ms),
                record.total_elapsed_ms,
            );
            stamp_rewrite_session(&mut record, job.rewrite_enabled);
            history.record(record);

            if silent {
                // B′: fade particles only — never surface rewrite text on HUD.
                hud.clear();
            } else if replacement_outcome.applied {
                let hud_text = format!("已替换：{}", short_text(candidate, 120));
                hud.show_text(&hud_text, false, false);
            } else if trace.output.is_some() && !candidate.trim().is_empty() {
                let hud_text = format!("原文保留，候选：{}", short_text(candidate, 120));
                hud.show_text(&hud_text, false, false);
            } else {
                hud.show_text(rewrite_no_output_hud_text(&trace), false, false);
            }
            if job.debug_mode {
                debug_panel.display_result(
                    candidate,
                    format!(
                        "{} | 异步改写完成 | {} | rewrite={}ms {}",
                        display_label,
                        replacement_outcome.output_actions,
                        trace.elapsed_ms,
                        trace.selected_model
                    ),
                );
            }
            info!(
                utterance_id = %job.utterance_id,
                mode = history_mode,
                rewrite_text = %short_text(candidate, 500),
                rewrite_model = %trace.selected_model,
                rewrite_elapsed_ms = trace.elapsed_ms,
                rewrite_attempts = %format_rewrite_attempts(&trace),
                replacement_outcome = %replacement_outcome.output_actions,
                total_elapsed_ms = job.started_at.elapsed().as_millis(),
                "non-streaming async rewrite completed"
            );
        });
    }

    fn spawn_hud_first_whisper_rewrite(&self, job: HudFirstWhisperRewriteJob) {
        self.spawn_hud_first_nonstreaming_rewrite(
            job,
            "whisper_zh_hud_fallback_rewrite",
            "whisper_zh",
        );
    }

    fn spawn_hud_first_nonstreaming_rewrite(
        &self,
        job: HudFirstWhisperRewriteJob,
        history_mode: &'static str,
        log_mode: &'static str,
    ) {
        let history = self.history.clone();
        let hud = self.hud.clone();
        let rewriter = self.rewriter.get();
        let rewrite_min_chars = self
            .rewriter
            .snapshot_config()
            .min_chars
            .max(self.config.rewrite.min_chars);
        let history_context = self.history_context_for_rewrite();
        thread::spawn(move || {
            let silent = job.silent_hud;
            let rewrite_started = Instant::now();
            let (trace_tx, trace_rx) = mpsc::channel();
            let mut deadline_raw_pasted = false;
            {
                let rewriter = rewriter.clone();
                let hud = hud.clone();
                let raw_text = job.raw_text.clone();
                let rewrite_enabled = job.rewrite_enabled;
                let output_language = job.output_language;
                let system_prompt = job.system_prompt.clone();
                thread::spawn(move || {
                    let trace = apply_whisper_rewrite_with(
                        rewriter.as_ref(),
                        rewrite_enabled,
                        rewrite_min_chars,
                        &hud,
                        &raw_text,
                        output_language,
                        silent,
                        Some(system_prompt.as_str()),
                        history_context.as_deref(),
                    );
                    let _ = trace_tx.send(trace);
                });
            }
            let deadline = Duration::from_millis(HUD_FIRST_REWRITE_DEADLINE_MS);
            let trace = match trace_rx.recv_timeout(deadline) {
                Ok(trace) => trace,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let fallback_started = Instant::now();
                    let paste_result = if output::foreground_matches_target(&job.target.fingerprint)
                    {
                        output::paste_text_to_target_with_trace(
                            &job.raw_paste_fallback,
                            &job.target,
                            &job.output_config,
                            &job.utterance_id,
                            output::TargetPunctuationPolicy::Preserve,
                            output::TargetMatchPolicy::RequireSame,
                        )
                    } else {
                        Err(anyhow!(
                            "target changed before HUD-first deadline fallback paste"
                        ))
                    };
                    let mut fallback_record =
                        HistoryRecord::new(&job.utterance_id, &job.profile_id, history_mode);
                    fallback_record.raw_text = job.raw_text.clone();
                    fallback_record.finalized_text = job.raw_paste_fallback.clone();
                    fallback_record.finalizer_actions = "raw_deadline_fallback".to_string();
                    fallback_record.audio_ms = job.audio_ms;
                    fallback_record.asr_elapsed_ms = job.asr_elapsed_ms;
                    fallback_record.total_elapsed_ms = job.started_at.elapsed().as_millis();
                    fallback_record.phase_timings = phase_timings(
                        Some(job.audio_ms as u128),
                        Some(job.asr_elapsed_ms),
                        Some(HUD_FIRST_REWRITE_DEADLINE_MS as u128),
                        Some(fallback_started.elapsed().as_millis()),
                        None,
                        fallback_record.total_elapsed_ms,
                    );
                    fallback_record.target_process = job.target.summary.process_name.clone();
                    fallback_record.target_class = job.target.summary.class_name.clone();
                    fallback_record.target_title = job.target.summary.title.clone();
                    fallback_record.target_context_source = job.target.context.source.to_string();
                    fallback_record.target_right_context =
                        job.target.context.right.as_str().to_string();
                    match paste_result {
                        Ok(paste_outcome) => {
                            deadline_raw_pasted = true;
                            fallback_record.pasted_text = paste_outcome.text.clone();
                            fallback_record.output_actions = format!(
                                "hud_first_deadline_raw_paste_applied;{}",
                                paste_outcome.text_actions
                            );
                            fallback_record.skipped_reason =
                                "rewrite_deadline_raw_paste_before_late_rewrite".to_string();
                            history.record(fallback_record);
                            if silent {
                                hud.show_meter_busy();
                            } else {
                                hud.show_text(&paste_outcome.text, false, false);
                            }
                        }
                        Err(error) => {
                            fallback_record.error =
                                format!("hud-first deadline raw paste failed: {error}");
                            fallback_record.output_actions =
                                "hud_first_deadline_raw_paste_failed".to_string();
                            fallback_record.skipped_reason =
                                "rewrite_deadline_raw_paste_failed".to_string();
                            history.record(fallback_record);
                            if silent {
                                hud.show_meter_busy();
                            } else {
                                hud.show_text(&job.raw_paste_fallback, false, false);
                            }
                        }
                    }
                    match trace_rx.recv() {
                        Ok(trace) => trace,
                        Err(_) => return,
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let rewrite_total_ms = rewrite_started.elapsed().as_millis();
            let (final_text, finalizer_actions, final_paste_source) =
                final_text_from_rewrite_trace(&trace, &job.raw_paste_fallback, job.output_language);
            if silent {
                hud.show_meter_busy();
            } else {
                hud.show_text(&final_text, true, false);
            }

            let mut record = HistoryRecord::new(&job.utterance_id, &job.profile_id, history_mode);
            record.raw_text = job.raw_text.clone();
            stamp_rewrite_session(&mut record, job.rewrite_enabled);
            apply_rewrite_trace_to_record(&mut record, &trace);
            record.finalized_text = final_text.clone();
            record.finalizer_actions = finalizer_actions;
            record.audio_ms = job.audio_ms;
            record.asr_elapsed_ms = job.asr_elapsed_ms;
            record.total_elapsed_ms = job.started_at.elapsed().as_millis();
            record.phase_timings = phase_timings(
                Some(job.audio_ms as u128),
                Some(job.asr_elapsed_ms),
                Some(rewrite_total_ms),
                None,
                None,
                record.total_elapsed_ms,
            );
            record.target_process = job.target.summary.process_name.clone();
            record.target_class = job.target.summary.class_name.clone();
            record.target_title = job.target.summary.title.clone();
            record.target_context_source = job.target.context.source.to_string();
            record.target_right_context = job.target.context.right.as_str().to_string();

            if deadline_raw_pasted {
                let replace_started = Instant::now();
                let replacement_outcome = async_streaming_rewrite_replacement_outcome(
                    &trace,
                    &final_text,
                    &job.raw_paste_fallback,
                    false,
                    Some(&job.target.fingerprint),
                    &job.output_config,
                    &job.utterance_id,
                    Some(rewrite_total_ms),
                );
                if replacement_outcome.applied {
                    record.pasted_text = final_text.clone();
                }
                record.output_actions = format!(
                    "hud_first_deadline_late_rewrite:{final_paste_source};{}",
                    replacement_outcome.output_actions
                );
                record.skipped_reason = replacement_outcome.output_actions.clone();
                record.phase_timings = phase_timings(
                    Some(job.audio_ms as u128),
                    Some(job.asr_elapsed_ms),
                    Some(rewrite_total_ms),
                    None,
                    Some(replace_started.elapsed().as_millis()),
                    record.total_elapsed_ms,
                );
                history.record(record);
                if silent {
                    hud.clear();
                } else if replacement_outcome.applied {
                    hud.show_text(&final_text, false, false);
                } else if trace.output.is_some() && !final_text.trim().is_empty() {
                    hud.show_text(
                        &format!("原文保留，候选：{}", short_text(&final_text, 120)),
                        false,
                        false,
                    );
                } else {
                    hud.show_text(rewrite_no_output_hud_text(&trace), false, false);
                }
                info!(
                    utterance_id = %job.utterance_id,
                    mode = log_mode,
                    rewrite_text = %short_text(&final_text, 500),
                    rewrite_model = %trace.selected_model,
                    rewrite_elapsed_ms = trace.elapsed_ms,
                    rewrite_attempts = %format_rewrite_attempts(&trace),
                    final_paste_source = %final_paste_source,
                    replacement_outcome = %replacement_outcome.output_actions,
                    total_elapsed_ms = job.started_at.elapsed().as_millis(),
                    "non-streaming HUD-first late rewrite completed after deadline raw paste"
                );
                return;
            }

            if let Some(reason) = hud_first_raw_fallback_skip_reason(&trace) {
                match copy_hud_first_fallback_to_clipboard(
                    &job.raw_paste_fallback,
                    &job.target,
                    &job.output_config,
                    &job.utterance_id,
                ) {
                    Ok(copy_outcome) => {
                        record.output_actions = format!(
                            "hud_first_final_paste_skipped:{reason};{}",
                            copy_outcome.text_actions
                        );
                        record.skipped_reason = format!("{reason}_raw_copied_not_pasted");
                        history.record(record);
                        if silent {
                            hud.clear();
                        } else {
                            hud.show_text("改写后端不可用，原文已复制未上屏", false, false);
                        }
                        info!(
                            utterance_id = %job.utterance_id,
                            mode = log_mode,
                            rewrite_model = %trace.selected_model,
                            rewrite_elapsed_ms = trace.elapsed_ms,
                            rewrite_attempts = %format_rewrite_attempts(&trace),
                            final_paste_source = %final_paste_source,
                            output_actions = %copy_outcome.text_actions,
                            total_elapsed_ms = job.started_at.elapsed().as_millis(),
                            "Whisper zh HUD-first raw fallback copied instead of pasted because rewrite backend is unavailable"
                        );
                    }
                    Err(error) => {
                        record.error = format!("hud-first raw fallback copy failed: {error}");
                        record.output_actions =
                            format!("hud_first_final_paste_skipped:{reason}:copy_failed");
                        record.skipped_reason = reason.to_string();
                        history.record(record);
                        if silent {
                            hud.clear();
                        } else {
                            hud.show_text("改写后端不可用，原文未上屏", false, false);
                        }
                        warn!(
                            utterance_id = %job.utterance_id,
                            mode = log_mode,
                            error = %error,
                            rewrite_model = %trace.selected_model,
                            rewrite_elapsed_ms = trace.elapsed_ms,
                            rewrite_attempts = %format_rewrite_attempts(&trace),
                            final_paste_source = %final_paste_source,
                            total_elapsed_ms = job.started_at.elapsed().as_millis(),
                            "Whisper zh HUD-first raw fallback paste skipped and copy failed"
                        );
                    }
                }
                return;
            }

            if !output::foreground_matches_target(&job.target.fingerprint) {
                let replacement_outcome = async_streaming_rewrite_replacement_outcome(
                    &trace,
                    &final_text,
                    &job.raw_paste_fallback,
                    false,
                    Some(&job.target.fingerprint),
                    &job.output_config,
                    &job.utterance_id,
                    Some(rewrite_total_ms),
                );
                if replacement_outcome.applied {
                    record.pasted_text = final_text.clone();
                    record.output_actions = format!(
                        "hud_first_late_replacement_applied:{final_paste_source};{}",
                        replacement_outcome.output_actions
                    );
                    record.skipped_reason = replacement_outcome.output_actions.clone();
                    history.record(record);
                    if silent {
                        hud.clear();
                    } else {
                        hud.show_text(&final_text, false, false);
                    }
                    return;
                }
                record.output_actions = format!(
                    "hud_first_final_paste_skipped:target_changed:{final_paste_source};{}",
                    replacement_outcome.output_actions
                );
                record.skipped_reason = "target_changed_before_final_paste".to_string();
                history.record(record);
                info!(
                    utterance_id = %job.utterance_id,
                    mode = log_mode,
                    rewrite_text = %short_text(&final_text, 500),
                    rewrite_model = %trace.selected_model,
                    rewrite_elapsed_ms = trace.elapsed_ms,
                    rewrite_attempts = %format_rewrite_attempts(&trace),
                    final_paste_source = %final_paste_source,
                    total_elapsed_ms = job.started_at.elapsed().as_millis(),
                    "Whisper zh HUD-first rewrite completed; final paste skipped because target changed"
                );
                return;
            }

            let paste_started = Instant::now();
            match output::paste_text_to_target_with_trace(
                &final_text,
                &job.target,
                &job.output_config,
                &job.utterance_id,
                output::TargetPunctuationPolicy::Preserve,
                output::TargetMatchPolicy::RequireSame,
            ) {
                Ok(paste_outcome) => {
                    record.pasted_text = paste_outcome.text.clone();
                    record.output_actions = format!(
                        "hud_first_final_paste_applied:{final_paste_source};{}",
                        paste_outcome.text_actions
                    );
                    record.phase_timings = phase_timings(
                        Some(job.audio_ms as u128),
                        Some(job.asr_elapsed_ms),
                        Some(rewrite_total_ms),
                        Some(paste_started.elapsed().as_millis()),
                        None,
                        job.started_at.elapsed().as_millis(),
                    );
                    history.record(record);
                    if silent {
                        hud.clear();
                    } else {
                        hud.show_text(&paste_outcome.text, false, false);
                    }
                    info!(
                        utterance_id = %job.utterance_id,
                        mode = log_mode,
                        rewrite_text = %short_text(&paste_outcome.text, 500),
                        rewrite_model = %trace.selected_model,
                        rewrite_elapsed_ms = trace.elapsed_ms,
                        rewrite_attempts = %format_rewrite_attempts(&trace),
                        final_paste_source = %final_paste_source,
                        output_actions = %paste_outcome.text_actions,
                        total_elapsed_ms = job.started_at.elapsed().as_millis(),
                        "Whisper zh HUD-first rewrite completed; final text pasted"
                    );
                }
                Err(error) => {
                    record.error = format!("hud-first final paste failed: {error}");
                    record.output_actions =
                        format!("hud_first_final_paste_failed:{final_paste_source}");
                    history.record(record);
                    if silent {
                        hud.clear();
                    } else {
                        hud.show_text(&final_text, false, false);
                    }
                    warn!(
                        utterance_id = %job.utterance_id,
                        mode = log_mode,
                        error = %error,
                        rewrite_text = %short_text(&final_text, 500),
                        rewrite_model = %trace.selected_model,
                        rewrite_elapsed_ms = trace.elapsed_ms,
                        rewrite_attempts = %format_rewrite_attempts(&trace),
                        final_paste_source = %final_paste_source,
                        total_elapsed_ms = job.started_at.elapsed().as_millis(),
                        "Whisper zh HUD-first rewrite completed but final paste failed"
                    );
                }
            }
        });
    }

    fn spawn_async_streaming_rewrite(&self, job: AsyncStreamingRewriteJob) {
        let history = self.history.clone();
        let hud = self.hud.clone();
        let debug_panel = self.debug_panel.clone();
        let rewriter = self.rewriter.get();
        let rewrite_min_chars = self
            .rewriter
            .snapshot_config()
            .min_chars
            .max(self.config.rewrite.min_chars);
        let history_context = self.history_context_for_rewrite();
        thread::spawn(move || {
            let (trace, replacement_age_ms) = if let Some(trace) = job.prewrite_trace {
                // FIX-1: prewrite results get their real age (produced-at time), so
                // stale prewrites are subject to ASYNC_REWRITE_REPLACEMENT_MAX_AGE_MS
                // just like fresh rewrites. Falls back to 0 only if the timestamp is
                // missing (should not happen).
                let age_ms = job
                    .prewrite_finished_at
                    .map(|finished_at| finished_at.elapsed().as_millis())
                    .unwrap_or(0);
                (trace, Some(age_ms))
            } else {
                let trace = apply_whisper_rewrite_with(
                    rewriter.as_ref(),
                    job.rewrite_enabled,
                    rewrite_min_chars,
                    &hud,
                    &job.raw_text,
                    job.output_language,
                    false,
                    None, // streaming path keeps language default prompt
                    history_context.as_deref(),
                );
                let age_ms = trace.elapsed_ms;
                (trace, Some(age_ms))
            };
            let rewrite_source = trace.output.as_deref().unwrap_or(&job.raw_text);
            let finalized =
                finalize_asr_text_for_paste_for_language(rewrite_source, job.output_language);
            let candidate = finalized.text.as_str();
            let mut record = HistoryRecord::new(
                &job.utterance_id,
                &job.profile_id,
                "streaming_asr_async_rewrite",
            );
            record.raw_text = job.raw_text.clone();
            stamp_rewrite_session(&mut record, job.rewrite_enabled);
            apply_rewrite_trace_to_record(&mut record, &trace);
            record.finalized_text = finalized.text.clone();
            record.finalizer_actions = finalized.actions.clone();
            record.audio_ms = job.audio_ms;
            record.partial_updates = job.partial_updates;
            record.total_elapsed_ms = job.started_at.elapsed().as_millis();
            record.target_context_source = job.target_context_source.clone();
            record.target_right_context = job.target_right_context.clone();
            if let Some(target) = &job.target_summary {
                record.target_process = target.process_name.clone();
                record.target_class = target.class_name.clone();
                record.target_title = target.title.clone();
            }
            let replacement_outcome = async_streaming_rewrite_replacement_outcome(
                &trace,
                candidate,
                &job.raw_pasted_text,
                job.debug_mode,
                job.target_fingerprint.as_ref(),
                &job.output_config,
                &job.utterance_id,
                replacement_age_ms,
            );
            if replacement_outcome.applied {
                record.pasted_text = candidate.to_string();
            }
            record.output_actions = replacement_outcome.output_actions.clone();
            record.skipped_reason = replacement_outcome.output_actions.clone();
            record.phase_timings = phase_timings(
                Some(job.audio_ms as u128),
                None,
                Some(trace.elapsed_ms),
                None,
                replacement_age_ms,
                record.total_elapsed_ms,
            );
            stamp_rewrite_session(&mut record, job.rewrite_enabled);
            history.record(record);

            if replacement_outcome.applied {
                let hud_text = format!("已替换：{}", short_text(candidate, 120));
                hud.show_text(&hud_text, false, false);
            } else if trace.output.is_some() && !candidate.trim().is_empty() {
                let hud_text = format!("原文保留，候选：{}", short_text(candidate, 120));
                hud.show_text(&hud_text, false, false);
            } else {
                hud.show_text(rewrite_no_output_hud_text(&trace), false, false);
            }
            if job.debug_mode {
                debug_panel.display_result(
                    candidate,
                    format!(
                        "Parakeet 流式 | 异步改写完成 | {} | rewrite={}ms {}",
                        replacement_outcome.output_actions, trace.elapsed_ms, trace.selected_model
                    ),
                );
            }
            info!(
                utterance_id = %job.utterance_id,
                mode = "streaming_asr",
                rewrite_text = %short_text(candidate, 500),
                rewrite_model = %trace.selected_model,
                rewrite_elapsed_ms = trace.elapsed_ms,
                rewrite_attempts = %format_rewrite_attempts(&trace),
                prewrite_status = %job.prewrite_status,
                replacement_outcome = %replacement_outcome.output_actions,
                total_elapsed_ms = job.started_at.elapsed().as_millis(),
                "streaming ASR async rewrite completed"
            );
        });
    }

    fn spawn_hud_first_streaming_rewrite(&self, job: HudFirstStreamingRewriteJob) {
        let history = self.history.clone();
        let hud = self.hud.clone();
        let rewriter = self.rewriter.get();
        let rewrite_min_chars = self
            .rewriter
            .snapshot_config()
            .min_chars
            .max(self.config.rewrite.min_chars);
        let history_context = self.history_context_for_rewrite();
        thread::spawn(move || {
            let rewrite_started = Instant::now();
            let (trace_tx, trace_rx) = mpsc::channel();
            let mut deadline_raw_pasted = false;
            if let Some(trace) = job.prewrite_trace {
                let _ = trace_tx.send(trace);
            } else {
                let rewriter = rewriter.clone();
                let hud = hud.clone();
                let raw_text = job.raw_text.clone();
                thread::spawn(move || {
                    let trace = apply_whisper_rewrite_with(
                        rewriter.as_ref(),
                        job.rewrite_enabled,
                        rewrite_min_chars,
                        &hud,
                        &raw_text,
                        job.output_language,
                        false,
                        None, // streaming path keeps language default prompt
                        history_context.as_deref(),
                    );
                    let _ = trace_tx.send(trace);
                });
            }
            let trace = match trace_rx
                .recv_timeout(Duration::from_millis(HUD_FIRST_REWRITE_DEADLINE_MS))
            {
                Ok(trace) => trace,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let fallback_started = Instant::now();
                    let paste_result = if output::foreground_matches_target(&job.target.fingerprint)
                    {
                        output::paste_text_to_target_with_trace(
                            &job.raw_paste_fallback,
                            &job.target,
                            &job.output_config,
                            &job.utterance_id,
                            output::TargetPunctuationPolicy::Preserve,
                            output::TargetMatchPolicy::RequireSame,
                        )
                    } else {
                        Err(anyhow!(
                            "target changed before streaming HUD-first deadline fallback paste"
                        ))
                    };
                    let mut fallback_record = HistoryRecord::new(
                        &job.utterance_id,
                        &job.profile_id,
                        "streaming_asr_hud_fallback_rewrite",
                    );
                    fallback_record.raw_text = job.raw_text.clone();
                    fallback_record.finalized_text = job.raw_paste_fallback.clone();
                    fallback_record.finalizer_actions = "raw_deadline_fallback".to_string();
                    fallback_record.audio_ms = job.audio_ms;
                    fallback_record.partial_updates = job.partial_updates;
                    fallback_record.total_elapsed_ms = job.started_at.elapsed().as_millis();
                    fallback_record.phase_timings = phase_timings(
                        Some(job.audio_ms as u128),
                        None,
                        Some(HUD_FIRST_REWRITE_DEADLINE_MS as u128),
                        Some(fallback_started.elapsed().as_millis()),
                        None,
                        fallback_record.total_elapsed_ms,
                    );
                    fallback_record.target_process = job.target.summary.process_name.clone();
                    fallback_record.target_class = job.target.summary.class_name.clone();
                    fallback_record.target_title = job.target.summary.title.clone();
                    fallback_record.target_context_source = job.target.context.source.to_string();
                    fallback_record.target_right_context =
                        job.target.context.right.as_str().to_string();
                    match paste_result {
                        Ok(paste_outcome) => {
                            deadline_raw_pasted = true;
                            fallback_record.pasted_text = paste_outcome.text.clone();
                            fallback_record.output_actions = format!(
                                "hud_first_deadline_raw_paste_applied;{}",
                                paste_outcome.text_actions
                            );
                            fallback_record.skipped_reason =
                                "rewrite_deadline_raw_paste_before_late_rewrite".to_string();
                            history.record(fallback_record);
                            hud.show_text(&paste_outcome.text, false, false);
                        }
                        Err(error) => {
                            fallback_record.error =
                                format!("hud-first deadline raw paste failed: {error}");
                            fallback_record.output_actions =
                                "hud_first_deadline_raw_paste_failed".to_string();
                            fallback_record.skipped_reason =
                                "rewrite_deadline_raw_paste_failed".to_string();
                            history.record(fallback_record);
                            hud.show_text(&job.raw_paste_fallback, false, false);
                        }
                    }
                    match trace_rx.recv() {
                        Ok(trace) => trace,
                        Err(_) => return,
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let rewrite_total_ms = rewrite_started.elapsed().as_millis();
            let (final_text, finalizer_actions, final_paste_source) =
                final_text_from_rewrite_trace(&trace, &job.raw_paste_fallback, job.output_language);
            hud.show_text(&final_text, true, false);

            let mut record = HistoryRecord::new(
                &job.utterance_id,
                &job.profile_id,
                "streaming_asr_hud_fallback_rewrite",
            );
            record.raw_text = job.raw_text.clone();
            stamp_rewrite_session(&mut record, job.rewrite_enabled);
            apply_rewrite_trace_to_record(&mut record, &trace);
            record.finalized_text = final_text.clone();
            record.finalizer_actions = finalizer_actions;
            record.audio_ms = job.audio_ms;
            record.partial_updates = job.partial_updates;
            record.total_elapsed_ms = job.started_at.elapsed().as_millis();
            record.phase_timings = phase_timings(
                Some(job.audio_ms as u128),
                None,
                Some(rewrite_total_ms),
                None,
                None,
                record.total_elapsed_ms,
            );
            record.target_process = job.target.summary.process_name.clone();
            record.target_class = job.target.summary.class_name.clone();
            record.target_title = job.target.summary.title.clone();
            record.target_context_source = job.target.context.source.to_string();
            record.target_right_context = job.target.context.right.as_str().to_string();

            if deadline_raw_pasted {
                let replace_started = Instant::now();
                let replacement_outcome = async_streaming_rewrite_replacement_outcome(
                    &trace,
                    &final_text,
                    &job.raw_paste_fallback,
                    false,
                    Some(&job.target.fingerprint),
                    &job.output_config,
                    &job.utterance_id,
                    Some(rewrite_total_ms),
                );
                if replacement_outcome.applied {
                    record.pasted_text = final_text.clone();
                }
                record.output_actions = format!(
                    "hud_first_deadline_late_rewrite:{final_paste_source};{}",
                    replacement_outcome.output_actions
                );
                record.skipped_reason = replacement_outcome.output_actions.clone();
                record.phase_timings = phase_timings(
                    Some(job.audio_ms as u128),
                    None,
                    Some(rewrite_total_ms),
                    None,
                    Some(replace_started.elapsed().as_millis()),
                    record.total_elapsed_ms,
                );
                history.record(record);
                if replacement_outcome.applied {
                    hud.show_text(&final_text, false, false);
                } else if trace.output.is_some() && !final_text.trim().is_empty() {
                    hud.show_text(
                        &format!("原文保留，候选：{}", short_text(&final_text, 120)),
                        false,
                        false,
                    );
                } else {
                    hud.show_text(rewrite_no_output_hud_text(&trace), false, false);
                }
                info!(
                    utterance_id = %job.utterance_id,
                    mode = "streaming_asr",
                    rewrite_text = %short_text(&final_text, 500),
                    rewrite_model = %trace.selected_model,
                    rewrite_elapsed_ms = trace.elapsed_ms,
                    rewrite_attempts = %format_rewrite_attempts(&trace),
                    prewrite_status = %job.prewrite_status,
                    final_paste_source = %final_paste_source,
                    replacement_outcome = %replacement_outcome.output_actions,
                    total_elapsed_ms = job.started_at.elapsed().as_millis(),
                    "streaming ASR HUD-first late rewrite completed after deadline raw paste"
                );
                return;
            }

            if let Some(reason) = hud_first_raw_fallback_skip_reason(&trace) {
                match copy_hud_first_fallback_to_clipboard(
                    &job.raw_paste_fallback,
                    &job.target,
                    &job.output_config,
                    &job.utterance_id,
                ) {
                    Ok(copy_outcome) => {
                        record.output_actions = format!(
                            "hud_first_final_paste_skipped:{reason};{}",
                            copy_outcome.text_actions
                        );
                        record.skipped_reason = format!("{reason}_raw_copied_not_pasted");
                        history.record(record);
                        hud.show_text("改写后端不可用，原文已复制未上屏", false, false);
                        info!(
                            utterance_id = %job.utterance_id,
                            mode = "streaming_asr",
                            rewrite_model = %trace.selected_model,
                            rewrite_elapsed_ms = trace.elapsed_ms,
                            rewrite_attempts = %format_rewrite_attempts(&trace),
                            prewrite_status = %job.prewrite_status,
                            final_paste_source = %final_paste_source,
                            output_actions = %copy_outcome.text_actions,
                            total_elapsed_ms = job.started_at.elapsed().as_millis(),
                            "streaming ASR HUD-first raw fallback copied instead of pasted because rewrite backend is unavailable"
                        );
                    }
                    Err(error) => {
                        record.error = format!("hud-first raw fallback copy failed: {error}");
                        record.output_actions =
                            format!("hud_first_final_paste_skipped:{reason}:copy_failed");
                        record.skipped_reason = reason.to_string();
                        history.record(record);
                        hud.show_text("改写后端不可用，原文未上屏", false, false);
                        warn!(
                            utterance_id = %job.utterance_id,
                            mode = "streaming_asr",
                            error = %error,
                            rewrite_model = %trace.selected_model,
                            rewrite_elapsed_ms = trace.elapsed_ms,
                            rewrite_attempts = %format_rewrite_attempts(&trace),
                            prewrite_status = %job.prewrite_status,
                            final_paste_source = %final_paste_source,
                            total_elapsed_ms = job.started_at.elapsed().as_millis(),
                            "streaming ASR HUD-first raw fallback paste skipped and copy failed"
                        );
                    }
                }
                return;
            }

            if !output::foreground_matches_target(&job.target.fingerprint) {
                record.output_actions =
                    format!("hud_first_final_paste_skipped:target_changed:{final_paste_source}");
                record.skipped_reason = "target_changed_before_final_paste".to_string();
                history.record(record);
                info!(
                    utterance_id = %job.utterance_id,
                    mode = "streaming_asr",
                    rewrite_text = %short_text(&final_text, 500),
                    rewrite_model = %trace.selected_model,
                    rewrite_elapsed_ms = trace.elapsed_ms,
                    rewrite_attempts = %format_rewrite_attempts(&trace),
                    prewrite_status = %job.prewrite_status,
                    final_paste_source = %final_paste_source,
                    total_elapsed_ms = job.started_at.elapsed().as_millis(),
                    "streaming ASR HUD-first rewrite completed; final paste skipped because target changed"
                );
                return;
            }

            let paste_started = Instant::now();
            match output::paste_text_to_target_with_trace(
                &final_text,
                &job.target,
                &job.output_config,
                &job.utterance_id,
                output::TargetPunctuationPolicy::Preserve,
                output::TargetMatchPolicy::RequireSame,
            ) {
                Ok(paste_outcome) => {
                    record.pasted_text = paste_outcome.text.clone();
                    record.output_actions = format!(
                        "hud_first_final_paste_applied:{final_paste_source};{}",
                        paste_outcome.text_actions
                    );
                    record.phase_timings = phase_timings(
                        Some(job.audio_ms as u128),
                        None,
                        Some(rewrite_total_ms),
                        Some(paste_started.elapsed().as_millis()),
                        None,
                        job.started_at.elapsed().as_millis(),
                    );
                    history.record(record);
                    hud.show_text(&paste_outcome.text, false, false);
                    info!(
                        utterance_id = %job.utterance_id,
                        mode = "streaming_asr",
                        rewrite_text = %short_text(&paste_outcome.text, 500),
                        rewrite_model = %trace.selected_model,
                        rewrite_elapsed_ms = trace.elapsed_ms,
                        rewrite_attempts = %format_rewrite_attempts(&trace),
                        prewrite_status = %job.prewrite_status,
                        final_paste_source = %final_paste_source,
                        output_actions = %paste_outcome.text_actions,
                        total_elapsed_ms = job.started_at.elapsed().as_millis(),
                        "streaming ASR HUD-first rewrite completed; final text pasted"
                    );
                }
                Err(error) => {
                    record.error = format!("hud-first final paste failed: {error}");
                    record.output_actions =
                        format!("hud_first_final_paste_failed:{final_paste_source}");
                    history.record(record);
                    hud.show_text(&final_text, false, false);
                    warn!(
                        utterance_id = %job.utterance_id,
                        mode = "streaming_asr",
                        error = %error,
                        rewrite_text = %short_text(&final_text, 500),
                        rewrite_model = %trace.selected_model,
                        rewrite_elapsed_ms = trace.elapsed_ms,
                        rewrite_attempts = %format_rewrite_attempts(&trace),
                        prewrite_status = %job.prewrite_status,
                        final_paste_source = %final_paste_source,
                        total_elapsed_ms = job.started_at.elapsed().as_millis(),
                        "streaming ASR HUD-first rewrite completed but final paste failed"
                    );
                }
            }
        });
    }

    fn whisper_hold_loop(
        &self,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        audio_rx: &mpsc::Receiver<Vec<f32>>,
        resampler: &mut LinearResampler,
        samples: &mut Vec<f32>,
        profile_id: VoiceProfileId,
    ) -> Result<bool> {
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return Ok(false);
            }
            while let Ok(event) = hotkey_rx.try_recv() {
                let HotkeyEvent::Voice(voice_event) = event;
                if voice_event.profile_id == profile_id && voice_event.phase == TriggerPhase::Released {
                    return Ok(true);
                }
            }
            match audio_rx.recv_timeout(Duration::from_millis(12)) {
                Ok(chunk) => {
                    resampler.push(&chunk);
                    samples.extend_from_slice(&resampler.take_available());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    warn!("resident microphone subscription disconnected during Whisper session");
                    return Ok(true);
                }
            }
        }
    }

    fn drain_whisper_release_audio(
        &self,
        audio_rx: &mpsc::Receiver<Vec<f32>>,
        resampler: &mut LinearResampler,
        samples: &mut Vec<f32>,
    ) {
        self.drain_release_audio(
            audio_rx,
            resampler,
            samples,
            self.config.whisper.release_grace_ms,
        );
    }

    fn drain_release_audio(
        &self,
        audio_rx: &mpsc::Receiver<Vec<f32>>,
        resampler: &mut LinearResampler,
        samples: &mut Vec<f32>,
        release_grace_ms: u64,
    ) {
        let deadline = Instant::now() + Duration::from_millis(release_grace_ms);
        while Instant::now() < deadline {
            match audio_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(chunk) => {
                    resampler.push(&chunk);
                    samples.extend_from_slice(&resampler.take_available());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        while let Ok(chunk) = audio_rx.try_recv() {
            resampler.push(&chunk);
        }
        samples.extend_from_slice(&resampler.take_available());
    }

    #[allow(clippy::too_many_arguments)]
    fn streaming_hold_loop(
        &self,
        hotkey_rx: &mpsc::Receiver<HotkeyEvent>,
        audio_rx: &mpsc::Receiver<Vec<f32>>,
        resampler: &mut LinearResampler,
        pending: &mut Vec<f32>,
        chunk_samples: usize,
        chunk_pump: &StreamingChunkPump,
        session_id: &str,
        preview: &mut StreamingPreviewState,
        prewrite: &mut StreamingPrewriteState,
        session_started_at: Instant,
        utterance_id: &str,
        profile_id: VoiceProfileId,
    ) -> Result<bool> {
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return Ok(false);
            }
            while let Ok(event) = hotkey_rx.try_recv() {
                let HotkeyEvent::Voice(voice_event) = event;
                if voice_event.profile_id == profile_id && voice_event.phase == TriggerPhase::Released {
                    self.drain_streaming_chunk_events(
                        chunk_pump,
                        preview,
                        prewrite,
                        session_started_at,
                        utterance_id,
                        session_id,
                    )?;
                    return Ok(true);
                }
            }
            match audio_rx.recv_timeout(Duration::from_millis(12)) {
                Ok(samples) => self.feed_streaming_samples(
                    resampler,
                    pending,
                    &samples,
                    chunk_samples,
                    chunk_pump,
                    session_id,
                    preview,
                    prewrite,
                    session_started_at,
                    utterance_id,
                )?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    warn!("resident microphone subscription disconnected during streaming session");
                    return Ok(true);
                }
            }
            self.drain_streaming_chunk_events(
                chunk_pump,
                preview,
                prewrite,
                session_started_at,
                utterance_id,
                session_id,
            )?;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn feed_streaming_samples(
        &self,
        resampler: &mut LinearResampler,
        pending: &mut Vec<f32>,
        samples: &[f32],
        chunk_samples: usize,
        chunk_pump: &StreamingChunkPump,
        session_id: &str,
        preview: &mut StreamingPreviewState,
        prewrite: &mut StreamingPrewriteState,
        session_started_at: Instant,
        utterance_id: &str,
    ) -> Result<()> {
        resampler.push(samples);
        pending.extend_from_slice(&resampler.take_available());
        while pending.len() >= chunk_samples {
            let chunk = pending.drain(0..chunk_samples).collect::<Vec<_>>();
            if !preview.first_audio_sent {
                preview.first_audio_sent = true;
                preview.first_audio_sent_at = Some(Instant::now());
                info!(
                    utterance_id,
                    session_id,
                    chunk_samples = chunk.len(),
                    first_audio_sent_ms = session_started_at.elapsed().as_millis(),
                    "streaming ASR first audio sent"
                );
            }
            preview.observe_sent_audio(&chunk);
            chunk_pump.send_chunk(chunk)?;
            self.drain_streaming_chunk_events(
                chunk_pump,
                preview,
                prewrite,
                session_started_at,
                utterance_id,
                session_id,
            )?;
        }
        Ok(())
    }

    fn drain_streaming_chunk_events(
        &self,
        chunk_pump: &StreamingChunkPump,
        preview: &mut StreamingPreviewState,
        prewrite: &mut StreamingPrewriteState,
        session_started_at: Instant,
        utterance_id: &str,
        session_id: &str,
    ) -> Result<()> {
        while let Ok(event) = chunk_pump.events.try_recv() {
            self.handle_streaming_chunk_event(
                event,
                preview,
                prewrite,
                session_started_at,
                utterance_id,
                session_id,
                true,
            )?;
        }
        Ok(())
    }

    fn drain_streaming_chunk_events_for(
        &self,
        chunk_pump: &StreamingChunkPump,
        preview: &mut StreamingPreviewState,
        prewrite: &mut StreamingPrewriteState,
        session_started_at: Instant,
        utterance_id: &str,
        session_id: &str,
        duration: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + duration;
        loop {
            self.drain_streaming_chunk_events(
                chunk_pump,
                preview,
                prewrite,
                session_started_at,
                utterance_id,
                session_id,
            )?;
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let wait_for = (deadline - now).min(Duration::from_millis(12));
            match chunk_pump.events.recv_timeout(wait_for) {
                Ok(event) => self.handle_streaming_chunk_event(
                    event,
                    preview,
                    prewrite,
                    session_started_at,
                    utterance_id,
                    session_id,
                    false,
                )?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn handle_streaming_chunk_event(
        &self,
        event: StreamingChunkEvent,
        preview: &mut StreamingPreviewState,
        prewrite: &mut StreamingPrewriteState,
        session_started_at: Instant,
        utterance_id: &str,
        session_id: &str,
        fail_on_error: bool,
    ) -> Result<()> {
        match event {
            StreamingChunkEvent::Partial(response) => {
                self.apply_streaming_partial(
                    response,
                    preview,
                    prewrite,
                    session_started_at,
                    utterance_id,
                    session_id,
                );
            }
            StreamingChunkEvent::Error(error) => {
                if fail_on_error {
                    bail!("{error}");
                }
                warn!(
                    utterance_id,
                    session_id, error, "streaming ASR chunk pump reported late error"
                );
            }
            StreamingChunkEvent::Finished {
                text,
                language,
                audio_ms,
                elapsed_ms,
                finished,
            } => {
                let prepared_text = prepare_asr_text(&text);
                if let Some(display_text) = preview.apply_partial(&prepared_text) {
                    prewrite.maybe_spawn(&display_text, utterance_id, session_id);
                    self.hud.show_text(&display_text, true, true);
                    if self.debug_panel.is_enabled() {
                        self.debug_panel.display_result(
                            &display_text,
                            format!(
                                "Parakeet 流式 | finish final | audio_ms={} | partial={}",
                                audio_ms, preview.partial_updates
                            ),
                        );
                    }
                    info!(
                        utterance_id,
                        session_id,
                        audio_ms,
                        elapsed_ms,
                        language = %language.as_deref().unwrap_or("unknown"),
                        raw_text = %short_text(&text, 220),
                        display_text = %short_text(&display_text, 220),
                        partial_updates = preview.partial_updates,
                        "streaming ASR finish text applied to HUD snapshot"
                    );
                }
                info!(
                    utterance_id,
                    session_id,
                    audio_ms,
                    elapsed_ms,
                    finished,
                    finish_text_chars = text.chars().count(),
                    "streaming ASR chunk pump finish observed"
                );
            }
            StreamingChunkEvent::FinishError(error) => {
                warn!(
                    utterance_id,
                    session_id, error, "streaming ASR chunk pump finish error observed"
                );
            }
        }
        Ok(())
    }

    fn apply_streaming_partial(
        &self,
        response: ChunkResponse,
        preview: &mut StreamingPreviewState,
        prewrite: &mut StreamingPrewriteState,
        session_started_at: Instant,
        utterance_id: &str,
        session_id: &str,
    ) {
        let text = prepare_asr_text(&response.text);
        if let Some(display_text) = preview.apply_partial(&text) {
            prewrite.maybe_spawn(&display_text, utterance_id, session_id);
            if preview.partial_updates == 1 {
                info!(
                    utterance_id,
                    session_id,
                    audio_ms = response.audio_ms,
                    elapsed_ms = response.elapsed_ms,
                    first_partial_ms = session_started_at.elapsed().as_millis(),
                    language = %response.language.as_deref().unwrap_or("unknown"),
                    text = %short_text(&display_text, 220),
                    "streaming ASR first partial received"
                );
            }
            self.hud.show_text(&display_text, true, true);
            if self.debug_panel.is_enabled() {
                self.debug_panel.display_result(
                    &display_text,
                    format!(
                        "Parakeet 流式 | 实时 partial | audio_ms={} | partial={}",
                        response.audio_ms, preview.partial_updates
                    ),
                );
            }
            info!(
                utterance_id,
                session_id,
                audio_ms = response.audio_ms,
                elapsed_ms = response.elapsed_ms,
                language = %response.language.as_deref().unwrap_or("unknown"),
                raw_text = %short_text(&text, 220),
                display_text = %short_text(&display_text, 220),
                partial_updates = preview.partial_updates,
                "streaming ASR HUD partial updated"
            );
        }
    }
}

fn apply_whisper_rewrite_with(
    rewriter: Option<&AiRewriter>,
    rewrite_enabled: bool,
    rewrite_min_chars: usize,
    hud: &HudController,
    raw_text: &str,
    output_language: RewriteOutputLanguage,
    silent_hud: bool,
    custom_system_prompt: Option<&str>,
    history_context: Option<&str>,
) -> RewriteTrace {
    let Some(rewriter) = rewriter else {
        return RewriteTrace {
            enabled: rewrite_enabled,
            attempts: if rewrite_enabled {
                vec![RewriteAttempt {
                    model: String::new(),
                    error: "rewriter_unavailable".to_string(),
                    ..Default::default()
                }]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
    };
    let text_chars = raw_text.trim().chars().count();
    if !rewrite_enabled || text_chars == 0 {
        return RewriteTrace {
            enabled: rewrite_enabled,
            ..Default::default()
        };
    }
    if text_chars < rewrite_min_chars {
        return RewriteTrace {
            enabled: rewrite_enabled,
            ..Default::default()
        };
    }
    if silent_hud {
        hud.show_meter_busy();
    } else {
        hud.show_text(
            if output_language.is_translation() {
                "翻译中..."
            } else {
                "改写中..."
            },
            true,
            false,
        );
    }
    // Chinese: user-selected tray prompt. Other languages: built-in translation prompt.
    let owned_default;
    let prompt = if output_language == RewriteOutputLanguage::Chinese {
        if let Some(custom) = custom_system_prompt.filter(|p| !p.trim().is_empty()) {
            custom
        } else {
            owned_default = rewrite_prompt_for_language(output_language);
            owned_default.as_str()
        }
    } else {
        owned_default = rewrite_prompt_for_language(output_language);
        owned_default.as_str()
    };
    let mut trace =
        rewriter.rewrite_with_prompt_trace_enabled_ctx(raw_text, prompt, rewrite_enabled, history_context);
    if let Some(output) = trace.output.clone() {
        if let Some(decision) = personal_corrections::guard_rewrite_output(raw_text, &output) {
            trace.attempts.push(RewriteAttempt {
                model: "rewrite_guard".to_string(),
                elapsed_ms: 0,
                ok: false,
                changed: false,
                error: decision.reason.clone(),
                ..Default::default()
            });
            trace.output = None;
            warn!(
                reason = %decision.reason,
                raw_text = %short_text(raw_text, 160),
                rewrite_text = %short_text(&output, 160),
                "Whisper AI rewrite blocked by protected replacement"
            );
        }
    }
    if trace.output.is_none() && trace.enabled {
        warn!(
            attempts = %format_rewrite_attempts(&trace),
            errors = %format_rewrite_errors(&trace),
            elapsed_ms = trace.elapsed_ms,
            output_language = ?output_language,
            "Whisper AI rewrite fell back to raw ASR text"
        );
    }
    trace
}

fn final_text_from_rewrite_trace(
    trace: &RewriteTrace,
    raw_paste_fallback: &str,
    output_language: RewriteOutputLanguage,
) -> (String, String, String) {
    let (candidate, finalizer_actions, final_paste_source) = if let Some(rewrite_output) =
        trace.output.as_deref()
    {
        let finalized = finalize_asr_text_for_paste_for_language(rewrite_output, output_language);
        (
            finalized.text,
            finalized.actions,
            "rewrite_output".to_string(),
        )
    } else {
        (
            raw_paste_fallback.to_string(),
            "raw_finalized_fallback".to_string(),
            rewrite_no_output_paste_source(trace).to_string(),
        )
    };
    let final_text = if candidate.trim().is_empty() {
        raw_paste_fallback.to_string()
    } else {
        candidate
    };
    (final_text, finalizer_actions, final_paste_source)
}

fn phase_timings(
    record_ms: Option<u128>,
    asr_ms: Option<u128>,
    rewrite_ms: Option<u128>,
    paste_ms: Option<u128>,
    replacement_ms: Option<u128>,
    total_ms: u128,
) -> String {
    let mut parts = Vec::new();
    if let Some(value) = record_ms {
        parts.push(format!("record={value}ms"));
    }
    if let Some(value) = asr_ms {
        parts.push(format!("asr={value}ms"));
    }
    if let Some(value) = rewrite_ms {
        parts.push(format!("rewrite={value}ms"));
    }
    if let Some(value) = paste_ms {
        parts.push(format!("paste={value}ms"));
    }
    if let Some(value) = replacement_ms {
        parts.push(format!("replace={value}ms"));
    }
    parts.push(format!("total={total_ms}ms"));
    parts.join(";")
}

#[derive(Debug, Default)]
struct StreamingPreviewState {
    last_display_text: String,
    best_snapshot: String,
    last_partial_at: Option<Instant>,
    partial_updates: usize,
    first_audio_sent: bool,
    first_audio_sent_at: Option<Instant>,
    first_partial_at: Option<Instant>,
    sent_samples: usize,
    sent_sum_squares: f64,
    sent_peak_abs: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DynamicReleaseGraceDecision {
    Skip,
    Drain(Duration),
}

impl StreamingPreviewState {
    fn observe_sent_audio(&mut self, samples: &[f32]) {
        self.sent_samples = self.sent_samples.saturating_add(samples.len());
        for sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            self.sent_sum_squares += f64::from(clamped * clamped);
            self.sent_peak_abs = self.sent_peak_abs.max(clamped.abs());
        }
    }

    fn sent_audio_ms(&self, sample_rate_hz: u32) -> u64 {
        audio_ms(self.sent_samples, sample_rate_hz)
    }

    fn sent_rms_dbfs(&self) -> f32 {
        if self.sent_samples == 0 {
            return f32::NEG_INFINITY;
        }
        amplitude_dbfs((self.sent_sum_squares / self.sent_samples as f64).sqrt() as f32)
    }

    fn sent_peak_dbfs(&self) -> f32 {
        amplitude_dbfs(self.sent_peak_abs)
    }

    fn apply_partial(&mut self, text: &str) -> Option<String> {
        let display_text = text.trim();
        if display_text.is_empty() || is_punctuation_only(display_text) {
            return None;
        }
        if should_keep_existing_snapshot(&self.best_snapshot, display_text) {
            self.last_partial_at = Some(Instant::now());
            return None;
        }
        if self.last_display_text == display_text {
            return None;
        }
        self.last_display_text = display_text.to_string();
        self.best_snapshot = display_text.to_string();
        let now = Instant::now();
        self.last_partial_at = Some(now);
        if self.first_partial_at.is_none() {
            self.first_partial_at = Some(now);
        }
        self.partial_updates += 1;
        Some(self.last_display_text.clone())
    }

    fn release_snapshot(&self) -> String {
        let snapshot = if self.best_snapshot.trim().is_empty() {
            &self.last_display_text
        } else {
            &self.best_snapshot
        };
        snapshot.trim().to_string()
    }

    fn dynamic_release_grace(&self, configured: Duration) -> DynamicReleaseGraceDecision {
        if !configured.is_zero() {
            return DynamicReleaseGraceDecision::Drain(configured);
        }
        let Some(last_partial_at) = self.last_partial_at else {
            return DynamicReleaseGraceDecision::Skip;
        };
        if self.partial_updates == 0 || self.release_snapshot().is_empty() {
            return DynamicReleaseGraceDecision::Skip;
        }
        if last_partial_at.elapsed()
            > Duration::from_millis(STREAMING_DYNAMIC_RELEASE_RECENT_PARTIAL_MS)
        {
            return DynamicReleaseGraceDecision::Skip;
        }
        let dynamic_ms = if self.partial_updates == 1 {
            STREAMING_DYNAMIC_RELEASE_GRACE_MIN_MS
        } else {
            STREAMING_DYNAMIC_RELEASE_GRACE_MAX_MS
        };
        DynamicReleaseGraceDecision::Drain(Duration::from_millis(dynamic_ms))
    }
}

impl StreamingPrewriteState {
    fn new(
        config: &crate::config::RewriteConfig,
        rewriter: Option<AiRewriter>,
        output_language: RewriteOutputLanguage,
        history_path: PathBuf,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            enabled: config.streaming_prewrite_enabled,
            min_chars: config.prewrite_min_chars,
            stable_ms: config.prewrite_stable_ms,
            debounce_ms: config.prewrite_debounce_ms,
            max_inflight: config.prewrite_max_inflight.max(1),
            output_language,
            rewriter,
            tx,
            rx,
            inflight: Arc::new(AtomicUsize::new(0)),
            last_spawned_at: None,
            last_source_hash: 0,
            history_path,
            context_history_count: config.context_history_count,
        }
    }

    fn maybe_spawn(&mut self, text: &str, utterance_id: &str, session_id: &str) {
        if !self.enabled {
            return;
        }
        let source = text.trim();
        if source.chars().count() < self.min_chars {
            return;
        }
        let hash = stable_text_hash(source);
        if self.last_source_hash == hash {
            return;
        }
        if self
            .last_spawned_at
            .is_some_and(|instant| instant.elapsed() < Duration::from_millis(self.debounce_ms))
        {
            return;
        }
        if self.inflight.load(Ordering::Relaxed) >= self.max_inflight {
            return;
        }
        let Some(rewriter) = self.rewriter.clone() else {
            return;
        };
        self.last_source_hash = hash;
        self.last_spawned_at = Some(Instant::now());
        self.inflight.fetch_add(1, Ordering::Relaxed);
        let tx = self.tx.clone();
        let inflight = self.inflight.clone();
        let source_text = source.to_string();
        let output_language = self.output_language;
        let stable_ms = self.stable_ms;
        let utterance_id = utterance_id.to_string();
        let session_id = session_id.to_string();
        let log_utterance_id = utterance_id.clone();
        let log_session_id = session_id.clone();
        // Load cross-utterance context from history (disabled when count == 0).
        let history_context = if self.context_history_count > 0 {
            let records = crate::history::load_recent(&self.history_path, self.context_history_count.saturating_mul(3)).ok();
            records.and_then(|r| {
                let ctx = crate::history::format_recent_context(&r, self.context_history_count);
                if ctx.trim().is_empty() { None } else { Some(ctx) }
            })
        } else {
            None
        };
        thread::spawn(move || {
            if stable_ms > 0 {
                thread::sleep(Duration::from_millis(stable_ms));
            }
            let prompt = rewrite_prompt_for_language(output_language);
            let mut trace = rewriter.rewrite_with_prompt_trace_enabled_ctx(&source_text, &prompt, true, history_context.as_deref());
            if let Some(output) = trace.output.clone() {
                if let Some(decision) =
                    personal_corrections::guard_rewrite_output(&source_text, &output)
                {
                    trace.attempts.push(RewriteAttempt {
                        model: "rewrite_guard".to_string(),
                        elapsed_ms: 0,
                        ok: false,
                        changed: false,
                        error: decision.reason.clone(),
                        ..Default::default()
                    });
                    trace.output = None;
                }
            }
            let _ = tx.send(StreamingPrewriteResult {
                source_text: source_text.clone(),
                trace,
                // FIX-1: record when this prewrite result was produced so the
                // async rewrite can apply the real freshness gate instead of 0.
                finished_at: Instant::now(),
            });
            inflight.fetch_sub(1, Ordering::Relaxed);
            info!(
                utterance_id = %utterance_id,
                session_id = %session_id,
                source_chars = source_text.chars().count(),
                "streaming ASR prewrite finished"
            );
        });
        info!(
            utterance_id = %log_utterance_id,
            session_id = %log_session_id,
            source_chars = source.chars().count(),
            stable_ms = self.stable_ms,
            debounce_ms = self.debounce_ms,
            "streaming ASR prewrite scheduled"
        );
    }

    // FIX-1: also return the instant the prewrite result was produced so the
    // async rewrite can gate on real prewrite age instead of bypassing with 0.
    fn take_trace_for_release(
        &mut self,
        final_text: &str,
    ) -> (Option<RewriteTrace>, Option<Instant>, String) {
        if !self.enabled {
            return (None, None, "prewrite_disabled".to_string());
        }
        let mut latest = None;
        while let Ok(result) = self.rx.try_recv() {
            latest = Some(result);
        }
        let Some(result) = latest else {
            return (None, None, "prewrite_miss_no_result".to_string());
        };
        if result.trace.output.is_none() {
            return (None, None, "prewrite_miss_no_output".to_string());
        }
        if !texts_close_enough_for_prewrite(&result.source_text, final_text) {
            return (None, None, "prewrite_miss_changed_text".to_string());
        }
        (
            Some(result.trace),
            Some(result.finished_at),
            "prewrite_hit".to_string(),
        )
    }
}

fn stable_text_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn texts_close_enough_for_prewrite(source: &str, final_text: &str) -> bool {
    let source = normalize_for_prewrite_compare(source);
    let final_text = normalize_for_prewrite_compare(final_text);
    if source.is_empty() || final_text.is_empty() {
        return false;
    }
    if source == final_text {
        return true;
    }
    let distance = levenshtein_distance(&source, &final_text);
    let max_len = source.chars().count().max(final_text.chars().count());
    distance <= 2 || distance * 10 <= max_len
}

fn normalize_for_prewrite_compare(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a = a.chars().collect::<Vec<_>>();
    let b = b.chars().collect::<Vec<_>>();
    let mut previous = (0..=b.len()).collect::<Vec<_>>();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

fn streaming_chunk_samples(sample_rate_hz: u32, chunk_ms: u32) -> usize {
    ((sample_rate_hz.max(1) as usize * chunk_ms.max(20) as usize) / 1000).max(320)
}

fn prepare_asr_text(text: &str) -> String {
    let compacted = text.split_whitespace().collect::<Vec<_>>().join(" ");
    dedupe_streaming_punctuation(&normalize_personal_english_terms(&compacted))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FinalizedText {
    text: String,
    actions: String,
}

fn finalize_asr_text_for_paste(text: &str) -> FinalizedText {
    finalize_asr_text_for_paste_for_language(text, RewriteOutputLanguage::Chinese)
}

fn finalize_asr_text_for_paste_for_language(
    text: &str,
    output_language: RewriteOutputLanguage,
) -> FinalizedText {
    let mut actions = Vec::new();
    let without_spaces = normalize_output_spacing(text, output_language, &mut actions);
    let digit_normalized = normalize_continuous_chinese_digits(&without_spaces);
    if digit_normalized != without_spaces {
        actions.push("zh_digits");
    }
    let personal_normalized = if output_language == RewriteOutputLanguage::English {
        digit_normalized.clone()
    } else {
        normalize_personal_english_terms(&digit_normalized)
    };
    if personal_normalized != digit_normalized {
        actions.push("personal_terms");
    }
    let mut finalized = dedupe_streaming_punctuation(&personal_normalized);
    if finalized != personal_normalized {
        actions.push("dedupe_punctuation");
    }
    // Replace trailing 笑死 with 🤣 before period logic — emoji must not keep
    // the two Chinese characters, and must not be followed by any punctuation.
    let laugh_replaced = replace_trailing_laugh_with_emoji(&mut finalized);
    if laugh_replaced {
        actions.push("laugh_emoji");
    } else if should_append_sentence_period(&finalized) {
        finalized.push(sentence_period_for_language(output_language));
        actions.push("append_period");
    }
    if actions.is_empty() {
        actions.push("none");
    }
    FinalizedText {
        text: finalized,
        actions: actions.join(","),
    }
}

/// If the transcript ends with 笑死 (optional trailing punctuation), replace that
/// ending with a bare 🤣 — the characters 笑死 are removed, no trailing punct.
fn replace_trailing_laugh_with_emoji(text: &mut String) -> bool {
    const MARKER: &str = "笑死";
    const EMOJI: &str = "🤣";
    let trimmed = text.trim_end();
    if trimmed.is_empty() || trimmed.ends_with(EMOJI) {
        return false;
    }
    // Strip trailing sentence punctuation that ASR/period-append may leave.
    let core = trimmed.trim_end_matches(|c: char| {
        matches!(
            c,
            '。' | '.' | '!' | '！' | '?' | '？' | '~' | '～' | '…' | ',' | '，' | ' '
        )
    });
    if !core.ends_with(MARKER) {
        return false;
    }
    let prefix_end = core.len().saturating_sub(MARKER.len());
    let prefix = core[..prefix_end].trim_end();
    if prefix.is_empty() {
        *text = EMOJI.to_string();
    } else {
        *text = format!("{prefix}{EMOJI}");
    }
    true
}

fn normalize_output_spacing(
    text: &str,
    output_language: RewriteOutputLanguage,
    actions: &mut Vec<&'static str>,
) -> String {
    if output_language == RewriteOutputLanguage::English {
        let compacted = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if compacted != text {
            actions.push("compact_ws");
        }
        return compacted;
    }
    let without_spaces = text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    if without_spaces != text {
        actions.push("strip_ws");
    }
    without_spaces
}

fn sentence_period_for_language(output_language: RewriteOutputLanguage) -> char {
    match output_language {
        RewriteOutputLanguage::English => '.',
        RewriteOutputLanguage::Chinese | RewriteOutputLanguage::Japanese => '。',
    }
}

fn normalize_personal_english_terms(text: &str) -> String {
    let mut normalized = text.to_string();
    while normalized.contains("A I A I") || normalized.contains("AIAI") {
        normalized = normalized.replace("A I A I", "AI").replace("AIAI", "AI");
    }
    for (from, to) in [
        ("A I", "AI"),
        ("V P S", "VPS"),
        ("Code X", "Codex"),
        ("code x", "Codex"),
        ("CODE X", "Codex"),
        ("CodeX", "Codex"),
        ("codeX", "Codex"),
        ("codex", "Codex"),
        ("扣代 S", "Codex"),
        ("扣代 s", "Codex"),
        ("抠代 S", "Codex"),
        ("抠代 s", "Codex"),
        ("口代 S", "Codex"),
        ("口代 s", "Codex"),
        ("扣代S", "Codex"),
        ("扣代s", "Codex"),
        ("抠代S", "Codex"),
        ("抠代s", "Codex"),
        ("口代S", "Codex"),
        ("口代s", "Codex"),
        ("扣代斯", "Codex"),
        ("抠代斯", "Codex"),
        ("口代斯", "Codex"),
        ("扣袋斯", "Codex"),
        ("扣带斯", "Codex"),
        ("扣戴斯", "Codex"),
        ("扣得斯", "Codex"),
        ("扣德斯", "Codex"),
        ("扣代次", "Codex"),
        ("扣带", "Codex"),
        ("抠带", "Codex"),
        ("口带", "Codex"),
        ("扣袋", "Codex"),
        ("抠袋", "Codex"),
        ("扣戴", "Codex"),
        ("抠戴", "Codex"),
        ("扣得", "Codex"),
        ("抠得", "Codex"),
        ("扣的", "Codex"),
        ("抠的", "Codex"),
    ] {
        normalized = normalized.replace(from, to);
    }
    if normalized.contains("Codex") {
        for (from, to) in [
            ("Codex编传", "Codex编程"),
            ("Codex编成", "Codex编程"),
            ("编成用的是Codex", "编程用的是Codex"),
            ("变成用的是Codex", "编程用的是Codex"),
        ] {
            normalized = normalized.replace(from, to);
        }
    }
    for token in ["AI", "VPS", "Codex"] {
        normalized = normalized
            .replace(&format!(" {token}"), token)
            .replace(&format!("{token} "), token);
    }
    personal_corrections::normalize_text(&normalized)
}

fn normalize_continuous_chinese_digits(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut run = String::new();
    for ch in text.chars() {
        if chinese_digit_value(ch).is_some() {
            run.push(ch);
        } else {
            flush_chinese_digit_run(&mut result, &mut run);
            result.push(ch);
        }
    }
    flush_chinese_digit_run(&mut result, &mut run);
    result
}

fn flush_chinese_digit_run(result: &mut String, run: &mut String) {
    if run.is_empty() {
        return;
    }
    if run.chars().count() >= 3 {
        for ch in run.chars() {
            if let Some(value) = chinese_digit_value(ch) {
                result.push(value);
            }
        }
    } else {
        result.push_str(run);
    }
    run.clear();
}

fn chinese_digit_value(ch: char) -> Option<char> {
    match ch {
        '零' | '〇' => Some('0'),
        '一' => Some('1'),
        '二' | '两' => Some('2'),
        '三' => Some('3'),
        '四' => Some('4'),
        '五' => Some('5'),
        '六' => Some('6'),
        '七' => Some('7'),
        '八' => Some('8'),
        '九' => Some('9'),
        _ => None,
    }
}

fn should_append_sentence_period(text: &str) -> bool {
    let Some(last) = text.chars().last() else {
        return false;
    };
    !is_sentence_punctuation(last)
        && !matches!(
            last,
            '.' | ',' | '!' | '?' | ';' | ':' | ')' | '）' | ']' | '】' | '}' | '」' | '』'
        )
}

fn should_keep_existing_snapshot(existing: &str, candidate: &str) -> bool {
    if existing.trim().is_empty() {
        return false;
    }
    let existing_body = comparable_body(existing);
    let candidate_body = comparable_body(candidate);
    existing_body == candidate_body && punctuation_count(existing) > punctuation_count(candidate)
}

fn comparable_body(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_sentence_punctuation(*ch))
        .collect()
}

fn punctuation_count(text: &str) -> usize {
    text.chars()
        .filter(|ch| is_sentence_punctuation(*ch))
        .count()
}

fn is_punctuation_only(text: &str) -> bool {
    text.chars()
        .all(|ch| ch.is_whitespace() || is_sentence_punctuation(ch))
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

fn is_sentence_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '.' | ',' | '!' | '?' | ';' | ':' | '。' | '，' | '！' | '？' | '、' | '；' | '：' | '．'
    )
}

fn audio_ms(sample_count: usize, sample_rate_hz: u32) -> u64 {
    ((sample_count as u128 * 1000) / sample_rate_hz.max(1) as u128) as u64
}

fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return f32::NEG_INFINITY;
    }
    let mean_square = samples
        .iter()
        .map(|sample| {
            let clamped = sample.clamp(-1.0, 1.0);
            clamped * clamped
        })
        .sum::<f32>()
        / samples.len() as f32;
    amplitude_dbfs(mean_square.sqrt())
}

fn amplitude_dbfs(amplitude: f32) -> f32 {
    if amplitude <= f32::EPSILON {
        f32::NEG_INFINITY
    } else {
        20.0 * amplitude.log10()
    }
}

fn short_text(text: &str, max_chars: usize) -> String {
    let mut value = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        value.push_str("...");
    }
    value
}

fn apply_rewrite_trace_to_record(record: &mut HistoryRecord, trace: &RewriteTrace) {
    // Prefer true if either session stamp or this trace enabled rewrite.
    record.rewrite_enabled = trace.enabled || record.rewrite_enabled;
    record.rewrite_model = trace.selected_model.clone();
    record.rewrite_attempts = format_rewrite_attempts(trace);
    record.rewrite_elapsed_ms = trace.elapsed_ms;
    record.rewrite_error = format_rewrite_errors(trace);
    record.rewrite_text = trace.output.clone().unwrap_or_default();
}

fn stamp_rewrite_session(record: &mut HistoryRecord, rewrite_enabled: bool) {
    record.rewrite_enabled = rewrite_enabled;
}

#[cfg(test)]
fn whisper_result_hud_text(
    text: &str,
    trace: &RewriteTrace,
    output_language: RewriteOutputLanguage,
) -> String {
    let operation = if output_language.is_translation() {
        "翻译"
    } else {
        "改写"
    };
    let status = if !trace.enabled {
        "未启用AI".to_string()
    } else if trace.output.is_some() {
        format!("{operation}成功 {}ms", trace.elapsed_ms)
    } else if trace.attempts.iter().any(|attempt| attempt.ok) {
        format!("{operation}无变化 {}ms", trace.elapsed_ms)
    } else if trace.attempts.iter().any(|attempt| !attempt.ok) {
        format!("{operation}失败 {}ms", trace.elapsed_ms)
    } else {
        "未调用AI".to_string()
    };
    format!("{status} | {text}")
}

fn rewrite_no_output_hud_text(trace: &RewriteTrace) -> &'static str {
    if rewrite_trace_has_backend_failure(trace) {
        "原文已上屏，改写后端暂不可用"
    } else {
        match classify_rewrite_no_output(trace) {
            "rewrite_content_safety" => "原文已上屏，改写未通过安全校验",
            "rewrite_nochange" => "原文已上屏，无需改写",
            _ => "原文已上屏，未改写",
        }
    }
}

fn rewrite_no_output_paste_source(trace: &RewriteTrace) -> &'static str {
    if rewrite_trace_has_backend_failure(trace) {
        "raw_after_rewrite_backend_unavailable"
    } else {
        match classify_rewrite_no_output(trace) {
            "rewrite_content_safety" => "raw_after_rewrite_content_safety",
            "rewrite_nochange" => "raw_after_rewrite_nochange",
            _ => "raw_after_rewrite_no_output",
        }
    }
}

/// Split the old opaque `rewrite_no_output` bucket for history / HUD.
fn classify_rewrite_no_output(trace: &RewriteTrace) -> &'static str {
    if trace.output.is_some() {
        return "rewrite_no_output";
    }
    if rewrite_trace_has_backend_failure(trace) {
        return "rewrite_backend_unavailable";
    }
    let has_ok = trace.attempts.iter().any(|a| a.ok);
    let any_changed = trace.attempts.iter().any(|a| a.ok && a.changed);
    let any_safety = trace.attempts.iter().any(|a| {
        let e = a.error.to_ascii_lowercase();
        e.contains("content safety")
            || e.contains("rewrite_content_")
            || e.contains("content_safety")
    });
    if any_safety {
        "rewrite_content_safety"
    } else if has_ok && !any_changed {
        "rewrite_nochange"
    } else {
        "rewrite_no_output"
    }
}

fn async_rewrite_replacement_outcome(
    trace: &RewriteTrace,
    candidate: &str,
    job: &AsyncWhisperRewriteJob,
) -> output::ReplacementOutcome {
    async_streaming_rewrite_replacement_outcome(
        trace,
        candidate,
        &job.raw_pasted_text,
        job.debug_mode,
        job.target_fingerprint.as_ref(),
        &job.output_config,
        &job.utterance_id,
        Some(trace.elapsed_ms),
    )
}

fn async_streaming_rewrite_replacement_outcome(
    trace: &RewriteTrace,
    candidate: &str,
    raw_pasted_text: &str,
    debug_mode: bool,
    target_fingerprint: Option<&output::TargetFingerprint>,
    output_config: &OutputConfig,
    utterance_id: &str,
    replacement_age_ms: Option<u128>,
) -> output::ReplacementOutcome {
    if trace.output.is_none() && rewrite_trace_has_backend_failure(trace) {
        output::ReplacementOutcome::skipped("rewrite_backend_unavailable")
    } else if trace.output.is_none() {
        output::ReplacementOutcome::skipped(classify_rewrite_no_output(trace))
    } else if candidate.trim().is_empty() || candidate == raw_pasted_text {
        output::ReplacementOutcome::skipped("rewrite_empty_or_same")
    } else if debug_mode {
        output::ReplacementOutcome::skipped("debug_mode")
    } else if replacement_age_ms.is_some_and(|age_ms| age_ms > ASYNC_REWRITE_REPLACEMENT_MAX_AGE_MS)
    {
        output::ReplacementOutcome::skipped("rewrite_result_too_late")
    } else if let Some(target_fingerprint) = target_fingerprint {
        output::replace_recent_paste_with_trace(
            raw_pasted_text,
            candidate,
            target_fingerprint,
            output_config,
            utterance_id,
        )
    } else {
        output::ReplacementOutcome::skipped("target_fingerprint_missing")
    }
}

fn hud_first_raw_fallback_skip_reason(trace: &RewriteTrace) -> Option<&'static str> {
    rewrite_trace_has_backend_failure(trace).then_some("rewrite_backend_unavailable")
}

fn copy_hud_first_fallback_to_clipboard(
    text: &str,
    target: &output::OutputTarget,
    config: &OutputConfig,
    utterance_id: &str,
) -> Result<output::PasteOutcome> {
    let mut copy_config = config.clone();
    copy_config.clipboard_policy = ClipboardPolicy::CopyOnly;
    output::paste_text_to_target_with_trace(
        text,
        target,
        &copy_config,
        utterance_id,
        output::TargetPunctuationPolicy::Preserve,
        output::TargetMatchPolicy::BestEffort,
    )
}

fn format_rewrite_attempts(trace: &RewriteTrace) -> String {
    trace
        .attempts
        .iter()
        .map(|attempt| {
            let status = if attempt.ok {
                if attempt.changed {
                    "ok_changed"
                } else {
                    "ok_nochange"
                }
            } else {
                "error"
            };
            format!(
                "{}:{}:{}ms:tok={}:cap={}:prompt={}",
                attempt.model,
                status,
                attempt.elapsed_ms,
                attempt.max_tokens,
                attempt.output_char_limit,
                attempt.prompt_variant
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn format_rewrite_errors(trace: &RewriteTrace) -> String {
    trace
        .attempts
        .iter()
        .filter(|attempt| !attempt.error.is_empty())
        .map(|attempt| format!("{}:{}", attempt.model, attempt.error))
        .collect::<Vec<_>>()
        .join("|")
}

fn rewrite_trace_has_backend_failure(trace: &RewriteTrace) -> bool {
    trace.output.is_none()
        && trace
            .attempts
            .iter()
            .any(|attempt| rewrite_error_is_backend_failure(&attempt.error))
}

fn rewrite_error_is_backend_failure(error: &str) -> bool {
    rewrite_error_is_backend_unavailable(error)
}

fn is_whisper_short_hallucination(text: &str, audio_ms: u64) -> bool {
    if audio_ms > 2500 {
        return false;
    }
    let normalized = text
        .trim()
        .trim_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch, '。' | '！' | '!' | '.' | '，' | ',' | '？' | '?')
        })
        .replace(' ', "");
    matches!(
        normalized.as_str(),
        "谢谢" | "谢谢大家" | "谢谢观看" | "感谢观看" | "感谢大家"
    )
}

fn next_utterance_id() -> String {
    let sequence = UTTERANCE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("utt-{sequence:08}")
}

#[cfg(test)]
mod tests {
    use super::{
        ASYNC_REWRITE_REPLACEMENT_MAX_AGE_MS, AsyncWhisperRewriteJob, StreamingPreviewState,
        async_rewrite_replacement_outcome, async_streaming_rewrite_replacement_outcome, audio_ms,
        finalize_asr_text_for_paste, finalize_asr_text_for_paste_for_language,
        hud_first_raw_fallback_skip_reason, prepare_asr_text, rewrite_error_is_backend_failure,
        rewrite_no_output_hud_text, rewrite_no_output_paste_source, rms_dbfs,
        texts_close_enough_for_prewrite, whisper_result_hud_text,
    };
    use crate::ai_rewrite::{RewriteAttempt, RewriteTrace};
    use crate::config::{OutputConfig, RewriteOutputLanguage};
    use std::time::Instant;

    #[test]
    fn prepare_asr_text_compacts_only_whitespace() {
        assert_eq!(prepare_asr_text(" 我 现在  test "), "我 现在 test");
        assert_eq!(prepare_asr_text("测试1下。"), "测试1下。");
    }

    #[test]
    fn finalize_replaces_trailing_laugh_with_emoji() {
        // 笑死 is replaced (not kept) and no trailing punctuation after 🤣.
        assert_eq!(finalize_asr_text_for_paste("这个梗笑死").text, "这个梗🤣");
        assert_eq!(finalize_asr_text_for_paste("这个梗笑死。").text, "这个梗🤣");
        assert_eq!(finalize_asr_text_for_paste("这个梗笑死！").text, "这个梗🤣");
        assert_eq!(finalize_asr_text_for_paste("笑死").text, "🤣");
        // Middle 笑死 is ordinary text.
        let mid = finalize_asr_text_for_paste("笑死我了还要继续").text;
        assert!(!mid.contains('🤣'));
        assert!(mid.contains("笑死"));
        // Already ends with emoji — leave alone.
        let mut already = "太好笑🤣".to_string();
        assert!(!super::replace_trailing_laugh_with_emoji(&mut already));
        assert_eq!(already, "太好笑🤣");
    }

    #[test]
    fn prepare_asr_text_normalizes_personal_english_terms_for_hud() {
        assert_eq!(
            prepare_asr_text("我现在再用 A I A I 来改写。"),
            "我现在再用AI来改写。"
        );
        assert_eq!(
            prepare_asr_text("甲骨文的 V P S 是挺好用的。"),
            "甲骨文的VPS是挺好用的。"
        );
        assert_eq!(
            prepare_asr_text("我编成用的是扣代 S。"),
            "我编程用的是Codex。"
        );
    }

    #[test]
    fn audio_duration_uses_sample_rate() {
        assert_eq!(audio_ms(16_000, 16_000), 1000);
        assert_eq!(audio_ms(8_000, 16_000), 500);
    }

    #[test]
    fn whisper_result_hud_text_exposes_rewrite_status() {
        let mut trace = RewriteTrace {
            enabled: true,
            elapsed_ms: 1428,
            output: Some("修正后".to_string()),
            ..Default::default()
        };
        assert_eq!(
            whisper_result_hud_text("修正后", &trace, RewriteOutputLanguage::Chinese),
            "改写成功 1428ms | 修正后"
        );
        assert_eq!(
            whisper_result_hud_text("Translated text.", &trace, RewriteOutputLanguage::English),
            "翻译成功 1428ms | Translated text."
        );

        trace.output = None;
        trace.elapsed_ms = 2007;
        trace.attempts = vec![RewriteAttempt {
            ok: false,
            elapsed_ms: 2007,
            ..Default::default()
        }];
        assert_eq!(
            whisper_result_hud_text("原文", &trace, RewriteOutputLanguage::Chinese),
            "改写失败 2007ms | 原文"
        );

        trace.attempts.clear();
        trace.elapsed_ms = 0;
        assert_eq!(
            whisper_result_hud_text("原文", &trace, RewriteOutputLanguage::Chinese),
            "未调用AI | 原文"
        );
    }

    #[test]
    fn rewrite_backend_failure_gets_explicit_status() {
        let trace = RewriteTrace {
            enabled: true,
            attempts: vec![RewriteAttempt {
                model: "openai/gpt-oss-120b".to_string(),
                ok: false,
                error: "AI rewrite endpoint returned error: HTTP status server error (503 Service Unavailable)".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        assert!(rewrite_error_is_backend_failure("context canceled"));
        assert_eq!(
            rewrite_no_output_hud_text(&trace),
            "原文已上屏，改写后端暂不可用"
        );
        assert_eq!(
            rewrite_no_output_paste_source(&trace),
            "raw_after_rewrite_backend_unavailable"
        );
        assert_eq!(
            hud_first_raw_fallback_skip_reason(&trace),
            Some("rewrite_backend_unavailable")
        );
    }

    #[test]
    fn async_rewrite_outcome_never_claims_unsafe_replacement() {
        let job = |raw_pasted_text: &str| AsyncWhisperRewriteJob {
            utterance_id: "utt-test".to_string(),
            profile_id: "whisper_capslock".to_string(),
            raw_text: raw_pasted_text.to_string(),
            raw_pasted_text: raw_pasted_text.to_string(),
            rewrite_enabled: true,
            output_language: RewriteOutputLanguage::Chinese,
            system_prompt: String::new(),
            audio_ms: 1000,
            asr_elapsed_ms: 100,
            started_at: Instant::now(),
            target_summary: None,
            target_fingerprint: None,
            target_context_source: "test".to_string(),
            target_right_context: "unknown".to_string(),
            output_config: OutputConfig::default(),
            debug_mode: true,
            silent_hud: false,
        };
        let no_output = RewriteTrace {
            enabled: true,
            output: None,
            ..Default::default()
        };
        assert_eq!(
            async_rewrite_replacement_outcome(&no_output, "原文", &job("原文")).output_actions,
            "replacement_skipped:rewrite_no_output"
        );

        let nochange = RewriteTrace {
            enabled: true,
            output: None,
            attempts: vec![RewriteAttempt {
                model: "openai/gpt-oss-120b".to_string(),
                ok: true,
                changed: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            async_rewrite_replacement_outcome(&nochange, "原文", &job("原文")).output_actions,
            "replacement_skipped:rewrite_nochange"
        );

        let safety = RewriteTrace {
            enabled: true,
            output: None,
            attempts: vec![RewriteAttempt {
                model: "openai/gpt-oss-120b".to_string(),
                ok: false,
                error: "AI rewrite output failed content safety: rewrite_content_too_short"
                    .to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            async_rewrite_replacement_outcome(&safety, "原文", &job("原文")).output_actions,
            "replacement_skipped:rewrite_content_safety"
        );

        let backend_unavailable = RewriteTrace {
            enabled: true,
            output: None,
            attempts: vec![RewriteAttempt {
                model: "openai/gpt-oss-120b".to_string(),
                ok: false,
                error: "auth_unavailable: no auth available".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            async_rewrite_replacement_outcome(&backend_unavailable, "原文", &job("原文"))
                .output_actions,
            "replacement_skipped:rewrite_backend_unavailable"
        );

        let same = RewriteTrace {
            enabled: true,
            output: Some("原文".to_string()),
            ..Default::default()
        };
        assert_eq!(
            async_rewrite_replacement_outcome(&same, "原文", &job("原文")).output_actions,
            "replacement_skipped:rewrite_empty_or_same"
        );

        let changed = RewriteTrace {
            enabled: true,
            output: Some("改写后".to_string()),
            ..Default::default()
        };
        assert_eq!(
            async_rewrite_replacement_outcome(&changed, "改写后", &job("原文")).output_actions,
            "replacement_skipped:debug_mode"
        );
    }

    #[test]
    fn async_rewrite_outcome_skips_late_raw_first_replacement() {
        let changed = RewriteTrace {
            enabled: true,
            output: Some("改写后".to_string()),
            elapsed_ms: ASYNC_REWRITE_REPLACEMENT_MAX_AGE_MS + 1,
            ..Default::default()
        };

        let outcome = async_streaming_rewrite_replacement_outcome(
            &changed,
            "改写后",
            "原文",
            false,
            None,
            &OutputConfig::default(),
            "utt-test",
            Some(changed.elapsed_ms),
        );

        assert_eq!(
            outcome.output_actions,
            "replacement_skipped:rewrite_result_too_late"
        );
    }

    #[test]
    fn silence_has_negative_infinity_rms() {
        assert!(rms_dbfs(&[]).is_infinite());
        assert!(rms_dbfs(&[0.0; 16]).is_infinite());
        assert!(rms_dbfs(&[0.5; 16]) > -7.0);
    }

    #[test]
    fn release_snapshot_uses_current_hud_text() {
        let mut preview = StreamingPreviewState::default();
        assert!(preview.release_snapshot().is_empty());
        assert_eq!(
            preview.apply_partial(" 我现在测试 "),
            Some("我现在测试".to_string())
        );
        assert_eq!(preview.release_snapshot(), "我现在测试");
    }

    #[test]
    fn release_snapshot_keeps_better_punctuation_if_later_partial_removes_it() {
        let mut preview = StreamingPreviewState::default();
        assert_eq!(
            preview.apply_partial("为什么？"),
            Some("为什么？".to_string())
        );
        assert_eq!(preview.apply_partial("为什么"), None);
        assert_eq!(preview.release_snapshot(), "为什么？");
    }

    #[test]
    fn release_snapshot_ignores_punctuation_only_partial() {
        let mut preview = StreamingPreviewState::default();
        assert_eq!(preview.apply_partial("。"), None);
        assert!(preview.release_snapshot().is_empty());
    }

    #[test]
    fn streaming_prewrite_similarity_requires_near_final_text() {
        assert!(texts_close_enough_for_prewrite(
            "鱼吃的比较少。豆腐，有时候是麻婆豆腐。",
            "鱼吃的比较少。 豆腐，有时候是麻婆豆腐。"
        ));
        assert!(!texts_close_enough_for_prewrite(
            "晚上的夜宵我最近吃的少了。",
            "晚上的夜宵我最近吃的少了，只有特别饿才会吃一点面包之类的。"
        ));
    }

    #[test]
    fn streaming_preview_tracks_sent_audio_metrics() {
        let mut preview = StreamingPreviewState::default();
        preview.observe_sent_audio(&[0.0; 8_000]);
        preview.observe_sent_audio(&[0.5; 8_000]);

        assert_eq!(preview.sent_audio_ms(16_000), 1000);
        assert!(preview.sent_rms_dbfs() > -13.0);
        assert!(preview.sent_peak_dbfs() > -7.0);
    }

    #[test]
    fn finalizer_removes_all_spaces_before_paste() {
        assert_eq!(
            finalize_asr_text_for_paste("这句话，  后面 有 两个 空格。").text,
            "这句话，后面有两个空格。"
        );
        assert_eq!(finalize_asr_text_for_paste("H U D").text, "HUD。");
    }

    #[test]
    fn finalizer_normalizes_personal_english_terms_before_paste() {
        assert_eq!(
            finalize_asr_text_for_paste("我现在再用AIAI来改写。").text,
            "我现在再用AI来改写。"
        );
        assert_eq!(
            finalize_asr_text_for_paste("我现在在测试一个关键字是AIAI").text,
            "我现在在测试一个关键字是AI。"
        );
        assert_eq!(
            finalize_asr_text_for_paste("我在用扣带编传").text,
            "我在用Codex编程。"
        );
        assert_eq!(
            finalize_asr_text_for_paste("我编成用的是扣代S").text,
            "我编程用的是Codex。"
        );
        assert_eq!(
            finalize_asr_text_for_paste("这个扣带是很好用").text,
            "这个Codex是很好用。"
        );
    }

    #[test]
    fn finalizer_appends_period_when_missing() {
        assert_eq!(
            finalize_asr_text_for_paste("我现在测试").text,
            "我现在测试。"
        );
        assert_eq!(finalize_asr_text_for_paste("为什么？").text, "为什么？");
        assert_eq!(finalize_asr_text_for_paste("").text, "");
    }

    #[test]
    fn finalizer_preserves_english_spaces_for_translation_output() {
        let finalized = finalize_asr_text_for_paste_for_language(
            "Please check why this HUD and Codex are not showing",
            RewriteOutputLanguage::English,
        );
        assert_eq!(
            finalized.text,
            "Please check why this HUD and Codex are not showing."
        );
        assert!(!finalized.text.contains("Pleasecheck"));
        assert!(finalized.text.contains("HUD and Codex"));

        let finalized = finalize_asr_text_for_paste_for_language(
            "この HUD を確認して",
            RewriteOutputLanguage::Japanese,
        );
        assert_eq!(finalized.text, "このHUDを確認して。");
    }

    #[test]
    fn finalizer_converts_continuous_chinese_digits() {
        assert_eq!(
            finalize_asr_text_for_paste("验证码是一二三四五六").text,
            "验证码是123456。"
        );
        assert_eq!(
            finalize_asr_text_for_paste("现在是二零二六年").text,
            "现在是2026年。"
        );
        assert_eq!(finalize_asr_text_for_paste("一两句话").text, "一两句话。");
    }

    #[test]
    fn whisper_short_hallucination_guard_skips_common_outros() {
        assert!(super::is_whisper_short_hallucination("谢谢大家。", 1510));
        assert!(super::is_whisper_short_hallucination("谢谢观看", 2000));
        assert!(!super::is_whisper_short_hallucination("谢谢大家", 3000));
        assert!(!super::is_whisper_short_hallucination(
            "我想说谢谢大家",
            1500
        ));
    }
}
