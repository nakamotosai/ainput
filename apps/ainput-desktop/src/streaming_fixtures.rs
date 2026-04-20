use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StreamingCaseStatus {
    Pass,
    FailBehavior,
    FailContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StreamingReplayPartialEntry {
    pub(crate) offset_ms: u64,
    pub(crate) raw_text: String,
    pub(crate) prepared_text: String,
    pub(crate) source: String,
    pub(crate) stable_chars: usize,
    pub(crate) frozen_chars: usize,
    pub(crate) volatile_chars: usize,
    pub(crate) rejected_prefix_rewrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StreamingReplayReport {
    pub(crate) case_id: String,
    pub(crate) input_wav: String,
    pub(crate) input_sample_rate_hz: i32,
    pub(crate) runner_sample_rate_hz: i32,
    pub(crate) input_duration_ms: u64,
    pub(crate) captured_samples: usize,
    pub(crate) peak_abs: f32,
    pub(crate) rms: f32,
    pub(crate) active_ratio: f32,
    pub(crate) total_chunks_fed: usize,
    pub(crate) total_decode_steps: usize,
    pub(crate) partial_updates: usize,
    pub(crate) first_partial_ms: Option<u64>,
    pub(crate) final_commit_ms: u64,
    pub(crate) partial_timeline: Vec<StreamingReplayPartialEntry>,
    pub(crate) last_partial_text: String,
    pub(crate) final_online_raw_text: String,
    pub(crate) final_prepared_candidate: String,
    pub(crate) final_text: String,
    pub(crate) final_visible_chars: usize,
    pub(crate) commit_source: String,
    pub(crate) expected_text: Option<String>,
    pub(crate) expected_visible_chars: Option<usize>,
    pub(crate) min_partial_updates: usize,
    pub(crate) shortfall_tolerance_chars: usize,
    pub(crate) behavior_status: StreamingCaseStatus,
    pub(crate) content_status: StreamingCaseStatus,
    pub(crate) failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StreamingFixtureCase {
    pub(crate) id: String,
    pub(crate) wav_path: String,
    #[serde(default)]
    pub(crate) expected_text: Option<String>,
    #[serde(default)]
    pub(crate) min_partial_updates: Option<usize>,
    #[serde(default)]
    pub(crate) min_visible_chars: Option<usize>,
    #[serde(default)]
    pub(crate) shortfall_tolerance_chars: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StreamingFixtureManifest {
    #[serde(default)]
    pub(crate) version: Option<u32>,
    #[serde(default)]
    pub(crate) fixture_root: Option<String>,
    pub(crate) cases: Vec<StreamingFixtureCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StreamingSelftestReport {
    pub(crate) manifest_path: String,
    pub(crate) total_cases: usize,
    pub(crate) passed_cases: usize,
    pub(crate) behavior_failures: usize,
    pub(crate) content_failures: usize,
    pub(crate) overall_status: StreamingCaseStatus,
    pub(crate) cases: Vec<StreamingReplayReport>,
}
