use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{info, warn};

use crate::cloud_asr::CloudAsrClient;

const PREHEATED_SESSION_MAX_AGE: Duration = Duration::from_secs(20);
const PREHEAT_RETRY_DELAY: Duration = Duration::from_millis(1500);

#[derive(Clone)]
pub struct AsrSessionPool {
    asr: CloudAsrClient,
    ready: Arc<Mutex<Option<ReadyAsrSession>>>,
    preheating: Arc<AtomicBool>,
    preheat_enabled: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

pub struct AcquiredAsrSession {
    pub session_id: String,
    pub sample_rate_hz: i32,
    pub boost_source: Option<String>,
    pub boost_phrases: Option<usize>,
    pub speech_context_phrases: Option<usize>,
    pub speech_context_limit: Option<usize>,
}

struct ReadyAsrSession {
    session_id: String,
    sample_rate_hz: i32,
    boost_source: Option<String>,
    boost_phrases: Option<usize>,
    speech_context_phrases: Option<usize>,
    speech_context_limit: Option<usize>,
    preheated_at: Instant,
    generation: u64,
}

impl AsrSessionPool {
    pub fn new(asr: CloudAsrClient) -> Self {
        let pool = Self {
            asr,
            ready: Arc::new(Mutex::new(None)),
            preheating: Arc::new(AtomicBool::new(false)),
            preheat_enabled: Arc::new(AtomicBool::new(true)),
            generation: Arc::new(AtomicU64::new(0)),
        };
        pool.preheat_next();
        pool
    }

    pub fn new_without_preheat(asr: CloudAsrClient) -> Self {
        Self {
            asr,
            ready: Arc::new(Mutex::new(None)),
            preheating: Arc::new(AtomicBool::new(false)),
            preheat_enabled: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
        // Intentionally no preheat_next — public product has streaming disabled.
    }


    pub fn acquire(&self) -> Result<AcquiredAsrSession> {
        let started_at = Instant::now();
        if let Some(session) = self.take_ready() {
            info!(
                session_id = %session.session_id,
                sample_rate_hz = session.sample_rate_hz,
                boost_source = %session.boost_source.as_deref().unwrap_or("unknown"),
                boost_phrases = session.boost_phrases.unwrap_or_default(),
                speech_context_phrases = session.speech_context_phrases.unwrap_or_default(),
                speech_context_limit = session.speech_context_limit.unwrap_or_default(),
                preheated_age_ms = session.preheated_at.elapsed().as_millis(),
                session_acquire_ms = started_at.elapsed().as_millis(),
                source = "preheated",
                "ASR session acquired"
            );
            self.preheat_next();
            return Ok(AcquiredAsrSession {
                session_id: session.session_id,
                sample_rate_hz: session.sample_rate_hz,
                boost_source: session.boost_source,
                boost_phrases: session.boost_phrases,
                speech_context_phrases: session.speech_context_phrases,
                speech_context_limit: session.speech_context_limit,
            });
        }

        let response = self.asr.start_session()?;
        info!(
            session_id = %response.session_id,
            sample_rate_hz = response.sample_rate_hz,
            boost_source = %response.boost_source.as_deref().unwrap_or("unknown"),
            boost_phrases = response.boost_phrases.unwrap_or_default(),
            speech_context_phrases = response.speech_context_phrases.unwrap_or_default(),
            speech_context_limit = response.speech_context_limit.unwrap_or_default(),
            session_acquire_ms = started_at.elapsed().as_millis(),
            source = "sync_start",
            "ASR session acquired"
        );
        self.preheat_next();
        Ok(AcquiredAsrSession {
            session_id: response.session_id,
            sample_rate_hz: response.sample_rate_hz,
            boost_source: response.boost_source,
            boost_phrases: response.boost_phrases,
            speech_context_phrases: response.speech_context_phrases,
            speech_context_limit: response.speech_context_limit,
        })
    }

    pub fn set_preheat_enabled(&self, enabled: bool, reason: &str) {
        let previous = self.preheat_enabled.swap(enabled, Ordering::AcqRel);
        self.invalidate_ready(reason);
        info!(
            enabled,
            previous, reason, "ASR preheat enabled state changed"
        );
        if enabled {
            self.preheat_next();
        }
    }

    pub fn invalidate_ready(&self, reason: &str) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let stale_session = match self.ready.lock() {
            Ok(mut ready) => ready.take(),
            Err(_) => {
                warn!("ASR session pool lock poisoned while invalidating ready session");
                None
            }
        };
        if let Some(session) = stale_session {
            info!(
                session_id = %session.session_id,
                reason,
                generation,
                preheated_age_ms = session.preheated_at.elapsed().as_millis(),
                "discarding invalidated preheated ASR session"
            );
            finish_streaming_session_in_background(self.asr.clone(), session.session_id);
        } else {
            info!(reason, generation, "invalidated ASR preheat generation");
        }
    }

    fn take_ready(&self) -> Option<ReadyAsrSession> {
        let session = {
            let Ok(mut ready) = self.ready.lock() else {
                warn!("ASR session pool lock poisoned while taking ready session");
                return None;
            };
            ready.take()?
        };
        let preheated_age = session.preheated_at.elapsed();
        let current_generation = self.generation.load(Ordering::Acquire);
        if session.generation != current_generation {
            warn!(
                session_id = %session.session_id,
                session_generation = session.generation,
                current_generation,
                "discarding generation-mismatched preheated ASR session"
            );
            finish_streaming_session_in_background(self.asr.clone(), session.session_id);
            return None;
        }
        if preheated_age <= PREHEATED_SESSION_MAX_AGE {
            return Some(session);
        }
        warn!(
            session_id = %session.session_id,
            preheated_age_ms = preheated_age.as_millis(),
            max_age_ms = PREHEATED_SESSION_MAX_AGE.as_millis(),
            "discarding stale preheated ASR session"
        );
        finish_streaming_session_in_background(self.asr.clone(), session.session_id);
        None
    }

    pub fn preheat_next(&self) {
        Self::spawn_preheat_if_needed(
            self.asr.clone(),
            Arc::clone(&self.ready),
            Arc::clone(&self.preheating),
            Arc::clone(&self.preheat_enabled),
            Arc::clone(&self.generation),
        );
    }

    fn spawn_preheat_if_needed(
        asr: CloudAsrClient,
        ready: Arc<Mutex<Option<ReadyAsrSession>>>,
        preheating: Arc<AtomicBool>,
        preheat_enabled: Arc<AtomicBool>,
        generation: Arc<AtomicU64>,
    ) {
        if !preheat_enabled.load(Ordering::Acquire) {
            return;
        }
        if ready.lock().map(|ready| ready.is_some()).unwrap_or(false) {
            return;
        }
        if preheating.swap(true, Ordering::AcqRel) {
            return;
        }
        let requested_generation = generation.load(Ordering::Acquire);
        thread::spawn(move || {
            let started_at = Instant::now();
            let mut restart_after_invalidation = false;
            let mut retry_after_failure = false;
            match asr.start_session() {
                Ok(response) => {
                    let session_id = response.session_id;
                    let sample_rate_hz = response.sample_rate_hz;
                    let mut store_for_cleanup = None;
                    let current_generation = generation.load(Ordering::Acquire);
                    if !preheat_enabled.load(Ordering::Acquire)
                        || current_generation != requested_generation
                    {
                        restart_after_invalidation = preheat_enabled.load(Ordering::Acquire);
                        store_for_cleanup = Some(session_id.clone());
                        info!(
                            session_id = %session_id,
                            requested_generation,
                            current_generation,
                            preheat_enabled = preheat_enabled.load(Ordering::Acquire),
                            "discarding ASR session preheated with stale settings"
                        );
                    } else {
                        match ready.lock() {
                            Ok(mut slot) => {
                                if slot.is_none() {
                                    *slot = Some(ReadyAsrSession {
                                        session_id: session_id.clone(),
                                        sample_rate_hz,
                                        boost_source: response.boost_source.clone(),
                                        boost_phrases: response.boost_phrases,
                                        speech_context_phrases: response.speech_context_phrases,
                                        speech_context_limit: response.speech_context_limit,
                                        preheated_at: Instant::now(),
                                        generation: requested_generation,
                                    });
                                    info!(
                                        session_id = %session_id,
                                        sample_rate_hz,
                                        boost_source = %response.boost_source.as_deref().unwrap_or("unknown"),
                                        boost_phrases = response.boost_phrases.unwrap_or_default(),
                                        speech_context_phrases = response.speech_context_phrases.unwrap_or_default(),
                                        speech_context_limit = response.speech_context_limit.unwrap_or_default(),
                                        generation = requested_generation,
                                        max_age_ms = PREHEATED_SESSION_MAX_AGE.as_millis(),
                                        preheat_next_ms = started_at.elapsed().as_millis(),
                                        "ASR session preheated"
                                    );
                                } else {
                                    store_for_cleanup = Some(session_id.clone());
                                }
                            }
                            Err(_) => {
                                warn!(
                                    "ASR session pool lock poisoned while storing preheated session"
                                );
                                store_for_cleanup = Some(session_id.clone());
                            }
                        }
                    }
                    if let Some(session_id) = store_for_cleanup {
                        finish_streaming_session_in_background(asr.clone(), session_id);
                    }
                }
                Err(error) => {
                    retry_after_failure = preheat_enabled.load(Ordering::Acquire)
                        && generation.load(Ordering::Acquire) == requested_generation;
                    warn!(
                        error = %error,
                        preheat_next_ms = started_at.elapsed().as_millis(),
                        retry_after_failure,
                        retry_delay_ms = PREHEAT_RETRY_DELAY.as_millis(),
                        "ASR session preheat failed"
                    );
                }
            }
            preheating.store(false, Ordering::Release);
            if restart_after_invalidation {
                Self::spawn_preheat_if_needed(asr, ready, preheating, preheat_enabled, generation);
            } else if retry_after_failure {
                thread::sleep(PREHEAT_RETRY_DELAY);
                Self::spawn_preheat_if_needed(asr, ready, preheating, preheat_enabled, generation);
            }
        });
    }
}

fn finish_streaming_session_in_background(asr: CloudAsrClient, session_id: String) {
    thread::spawn(move || {
        let started_at = Instant::now();
        match asr.finish_session(&session_id) {
            Ok(finish) => {
                info!(
                    session_id,
                    audio_ms = finish.audio_ms,
                    elapsed_ms = finish.elapsed_ms,
                    finished = finish.finished,
                    background_finish_ms = started_at.elapsed().as_millis(),
                    "streaming ASR session finished in background"
                );
            }
            Err(error) => {
                warn!(
                    session_id,
                    error = %error,
                    background_finish_ms = started_at.elapsed().as_millis(),
                    "streaming ASR background finish failed"
                );
            }
        }
    });
}
