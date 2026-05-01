use crate::overlay::{HudAcceptanceSnapshot, RecordingOverlay};
use crate::streaming_fixtures::{
    StreamingCaseStatus, StreamingFixtureCase, StreamingFixtureManifest, StreamingReplayReport,
    StreamingSelftestReport,
};
use crate::{AppRuntime, hotkey, worker};

use anyhow::{Context, Result, anyhow};
use arboard::Clipboard;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE, ES_WANTRETURN, GUITHREADINFO,
    GetForegroundWindow, GetGUIThreadInfo, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOW, SendMessageW,
    SetForegroundWindow, SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_GETTEXT, WM_GETTEXTLENGTH, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, w};

const DEFAULT_SYNTHETIC_MANIFEST: &str = "fixtures/streaming-hud-e2e/manifest.json";
const DEFAULT_WAV_MANIFEST: &str = "fixtures/streaming-selftest/manifest.json";
const TARGET_CLASS_NAME: windows::core::PCWSTR = w!("ainput_acceptance_target_window");
const POST_COMMIT_DUPLICATE_OBSERVE_WINDOW: Duration = Duration::from_millis(1_500);

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticManifest {
    pub cases: Vec<SyntheticCase>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticCase {
    pub id: String,
    pub expected_text: Option<String>,
    pub events: Vec<SyntheticEvent>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticEvent {
    pub t_ms: u64,
    pub prepared_text: Option<String>,
    pub final_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LiveE2eReport {
    pub overall_status: String,
    pub run_id: String,
    pub report_dir: String,
    pub cases_total: usize,
    pub cases_passed: usize,
    pub failures: Vec<LiveE2eFailure>,
    pub cases: Vec<LiveE2eCaseReport>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LiveE2eCaseReport {
    pub case_id: String,
    pub status: String,
    pub final_text: String,
    pub resolved_commit_text: String,
    pub target_readback: String,
    pub hud_final_display: String,
    pub partial_count: usize,
    pub commit_envelope_count: usize,
    pub commit_request_count: usize,
    pub post_hud_flush_mutation_count: usize,
    pub hud_stability: HudStabilityReport,
    pub failures: Vec<LiveE2eFailure>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LiveE2eFailure {
    pub case_id: String,
    pub category: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct HudStabilityReport {
    pub samples: usize,
    pub max_left_delta_px: i32,
    pub max_top_delta_px: i32,
    pub max_center_x_delta_px: i32,
    pub max_width_delta_px: i32,
    pub max_height_delta_px: i32,
    pub alpha_drop_count: usize,
    pub invisible_sample_count: usize,
    pub white_panel_sample_count: usize,
    pub multiline_panel_sample_count: usize,
    pub short_text_wide_panel_count: usize,
}

pub(crate) fn default_synthetic_manifest_path(runtime: &AppRuntime) -> PathBuf {
    runtime.root_dir().join(DEFAULT_SYNTHETIC_MANIFEST)
}

pub(crate) fn default_wav_manifest_path(runtime: &AppRuntime) -> PathBuf {
    runtime.root_dir().join(DEFAULT_WAV_MANIFEST)
}

pub(crate) fn run_synthetic_live_e2e(
    runtime: &AppRuntime,
    manifest_path: &Path,
    report_dir: Option<&Path>,
) -> Result<LiveE2eReport> {
    let run_id = new_run_id("synthetic");
    let report_dir = report_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        runtime
            .root_dir()
            .join("tmp")
            .join("streaming-live-e2e")
            .join(&run_id)
    });
    fs::create_dir_all(&report_dir)
        .with_context(|| format!("create live e2e report dir {}", report_dir.display()))?;

    let manifest_text = fs::read_to_string(manifest_path)
        .with_context(|| format!("read synthetic manifest {}", manifest_path.display()))?;
    let manifest: SyntheticManifest =
        serde_json::from_str(manifest_text.trim_start_matches('\u{feff}'))
            .with_context(|| format!("parse synthetic manifest {}", manifest_path.display()))?;

    let mut trace = TraceWriter::create(report_dir.join("timeline.jsonl"), &run_id)?;
    trace.event(
        "run_started",
        json!({ "manifest": manifest_path.display().to_string() }),
    )?;

    let mut overlay = RecordingOverlay::create(&runtime.config().hud_overlay)
        .context("create HUD overlay for live e2e")?;
    let target =
        AcceptanceTargetWindow::create(&run_id).context("create acceptance target window")?;
    let output_config = ainput_output::OutputConfig {
        fallback_to_clipboard: false,
        allow_native_edit: true,
        restore_clipboard_after_paste: false,
        defer_clipboard_restore: false,
        preserve_text_exactly: true,
        paste_stabilize_delay: Duration::from_millis(120),
        ..Default::default()
    };
    let mut reports = Vec::new();

    for case in &manifest.cases {
        reports.push(run_synthetic_case(
            runtime,
            case,
            &mut overlay,
            &target,
            &output_config,
            &mut trace,
        )?);
    }

    trace.event("run_finished", json!({}))?;
    let failures = reports
        .iter()
        .flat_map(|case| case.failures.iter().cloned())
        .collect::<Vec<_>>();
    let cases_passed = reports.iter().filter(|case| case.status == "pass").count();
    let overall_status = if failures.is_empty() { "pass" } else { "fail" }.to_string();
    let report = LiveE2eReport {
        overall_status,
        run_id,
        report_dir: report_dir.display().to_string(),
        cases_total: reports.len(),
        cases_passed,
        failures,
        cases: reports,
    };

    let report_json = serde_json::to_vec_pretty(&report)?;
    fs::write(report_dir.join("report.json"), report_json)?;
    write_summary(&report_dir, &report)?;
    Ok(report)
}

pub(crate) fn run_wav_live_e2e(
    runtime: &AppRuntime,
    recognizer: &ainput_asr::StreamingZipformerRecognizer,
    manifest_path: &Path,
    report_dir: Option<&Path>,
    case_limit: Option<usize>,
) -> Result<LiveE2eReport> {
    let run_id = new_run_id("wav");
    let report_dir = report_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        runtime
            .root_dir()
            .join("tmp")
            .join("streaming-live-e2e")
            .join(&run_id)
    });
    fs::create_dir_all(&report_dir)
        .with_context(|| format!("create wav live e2e report dir {}", report_dir.display()))?;

    let manifest_text = fs::read_to_string(manifest_path)
        .with_context(|| format!("read wav manifest {}", manifest_path.display()))?;
    let manifest_text = manifest_text.trim_start_matches('\u{feff}');
    let manifest: StreamingFixtureManifest = serde_json::from_str(manifest_text)
        .with_context(|| format!("parse wav manifest {}", manifest_path.display()))?;
    let mut trace = TraceWriter::create(report_dir.join("timeline.jsonl"), &run_id)?;
    trace.event(
        "run_started",
        json!({
            "mode": "wav",
            "manifest": manifest_path.display().to_string(),
            "case_limit": case_limit,
        }),
    )?;

    let mut overlay = RecordingOverlay::create(&runtime.config().hud_overlay)
        .context("create HUD overlay for wav live e2e")?;
    let target =
        AcceptanceTargetWindow::create(&run_id).context("create acceptance target window")?;
    let output_config = ainput_output::OutputConfig {
        fallback_to_clipboard: false,
        allow_native_edit: true,
        restore_clipboard_after_paste: false,
        defer_clipboard_restore: false,
        preserve_text_exactly: true,
        paste_stabilize_delay: Duration::from_millis(120),
        ..Default::default()
    };
    let mut reports = Vec::new();
    let mut core_cases = Vec::new();
    let manifest_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let fixture_root = manifest
        .fixture_root
        .as_ref()
        .map(|root| manifest_dir.join(root));

    for case in manifest.cases.iter().take(case_limit.unwrap_or(usize::MAX)) {
        let wav_path = resolve_wav_fixture_case_path(&manifest_dir, fixture_root.as_deref(), case);
        let case_started = Instant::now();
        trace.event(
            "core_case_started",
            json!({
                "case_id": case.id,
                "input_wav": wav_path.display().to_string(),
            }),
        )?;
        let replay = worker::replay_streaming_wav(
            runtime,
            recognizer,
            &case.id,
            &wav_path,
            case.expected_text.as_deref(),
            &case.keywords,
            case.min_partial_updates.unwrap_or(1),
            case.min_visible_chars,
            case.shortfall_tolerance_chars.unwrap_or(3),
        )
        .with_context(|| format!("run streaming core replay for {}", case.id))?;
        trace.event(
            "core_case_finished",
            json!({
                "case_id": replay.case_id,
                "elapsed_ms": case_started.elapsed().as_millis(),
                "behavior_status": replay.behavior_status,
                "content_status": replay.content_status,
                "partial_updates": replay.partial_updates,
                "final_text": replay.final_text,
            }),
        )?;
        fs::write(
            report_dir.join(format!(
                "core-case-{}.json",
                sanitize_path_component(&case.id)
            )),
            serde_json::to_vec_pretty(&replay)?,
        )?;
        reports.push(run_replay_case_visible(
            runtime,
            &replay,
            &mut overlay,
            &target,
            &output_config,
            &mut trace,
        )?);
        core_cases.push(replay);
    }

    let core_report = build_core_report(manifest_path, core_cases);
    fs::write(
        report_dir.join("core-report.json"),
        serde_json::to_vec_pretty(&core_report)?,
    )?;
    trace.event(
        "core_report_finished",
        json!({
            "overall_status": core_report.overall_status,
            "passed_cases": core_report.passed_cases,
            "total_cases": core_report.total_cases,
        }),
    )?;
    trace.event("run_finished", json!({}))?;
    let failures = reports
        .iter()
        .flat_map(|case| case.failures.iter().cloned())
        .collect::<Vec<_>>();
    let cases_passed = reports.iter().filter(|case| case.status == "pass").count();
    let overall_status = if failures.is_empty() { "pass" } else { "fail" }.to_string();
    let report = LiveE2eReport {
        overall_status,
        run_id,
        report_dir: report_dir.display().to_string(),
        cases_total: reports.len(),
        cases_passed,
        failures,
        cases: reports,
    };

    fs::write(
        report_dir.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    write_summary(&report_dir, &report)?;
    Ok(report)
}

fn resolve_wav_fixture_case_path(
    manifest_dir: &Path,
    fixture_root: Option<&Path>,
    case: &StreamingFixtureCase,
) -> PathBuf {
    let wav_path = Path::new(&case.wav_path);
    if wav_path.is_absolute() {
        return wav_path.to_path_buf();
    }
    if let Some(fixture_root) = fixture_root {
        return fixture_root.join(wav_path);
    }
    manifest_dir.join(wav_path)
}

fn build_core_report(
    manifest_path: &Path,
    cases: Vec<StreamingReplayReport>,
) -> StreamingSelftestReport {
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

    StreamingSelftestReport {
        manifest_path: manifest_path.display().to_string(),
        total_cases: cases.len(),
        passed_cases,
        behavior_failures,
        content_failures,
        overall_status,
        cases,
    }
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "case".to_string()
    } else {
        sanitized
    }
}

fn run_synthetic_case(
    runtime: &AppRuntime,
    case: &SyntheticCase,
    overlay: &mut RecordingOverlay,
    target: &AcceptanceTargetWindow,
    output_config: &ainput_output::OutputConfig,
    trace: &mut TraceWriter,
) -> Result<LiveE2eCaseReport> {
    let mut failures = Vec::new();
    let case_started = Instant::now();
    let mut partial_count = 0usize;
    let mut final_text = String::new();
    let mut resolved_commit_text = String::new();
    let mut hud_final_display = String::new();
    let mut stale_clipboard_text = String::new();
    let mut last_partial_text = String::new();
    let mut hud_snapshots = Vec::new();
    let mut commit_locked = false;
    let mut commit_envelope_count = 0usize;
    let mut commit_request_count = 0usize;
    let mut post_hud_flush_mutation_count = 0usize;
    let case_id = case.id.clone();

    let target_initial_text = reset_case_surface(overlay, target)
        .with_context(|| format!("reset visible acceptance surface for {}", case_id))?;
    let hud_initial_snapshot = overlay.acceptance_snapshot();
    trace.event(
        "case_started",
        json!({
            "case_id": case_id,
            "expected_text": case.expected_text,
            "target_initial_text": target_initial_text,
        }),
    )?;
    trace_hud_snapshot(
        trace,
        "hud_after_case_reset",
        &case_id,
        hud_initial_snapshot.clone(),
    )?;
    reject_dirty_initial_target(&case_id, &target_initial_text, &mut failures);
    reject_dirty_initial_hud(&case_id, &hud_initial_snapshot, &mut failures);
    pump_for(Duration::from_millis(80), Some(overlay));

    for event in &case.events {
        wait_until(case_started, Duration::from_millis(event.t_ms), overlay);

        if let Some(text) = &event.prepared_text {
            if commit_locked {
                post_hud_flush_mutation_count += 1;
                failures.push(failure(
                    &case_id,
                    "post_hud_flush_mutation",
                    format!("partial arrived after commit lock: {text}"),
                ));
            }
            partial_count += 1;
            reject_duplicate_or_conflicting_punctuation(
                &case_id,
                text,
                "hud_duplicate_punctuation",
                &mut failures,
            );
            last_partial_text = text.clone();
            trace.event(
                "worker_partial",
                json!({
                    "case_id": case_id,
                    "revision": partial_count,
                    "prepared_text": text,
                }),
            )?;
            overlay.show_status_hud(text, true, true);
            pump_for(Duration::from_millis(70), Some(overlay));
            let snapshot = overlay.acceptance_snapshot();
            hud_snapshots.push(snapshot.clone());
            trace_hud_snapshot(trace, "hud_after_partial", &case_id, snapshot)?;
        }

        if let Some(text) = &event.final_text {
            final_text = text.clone();
            resolved_commit_text = text.clone();
            commit_envelope_count += 1;
            if commit_envelope_count > 1 {
                failures.push(failure(
                    &case_id,
                    "duplicate_commit_envelope",
                    format!("commit envelope count={commit_envelope_count}"),
                ));
            }
            reject_duplicate_or_conflicting_punctuation(
                &case_id,
                text,
                "hud_duplicate_punctuation",
                &mut failures,
            );
            trace.event(
                "commit_envelope_created",
                json!({
                    "case_id": case_id,
                    "revision": partial_count + 1,
                    "last_hud_target_text": last_partial_text.clone(),
                    "resolved_commit_text": resolved_commit_text.clone(),
                    "final_text": text,
                }),
            )?;
            overlay.show_status_hud(text, true, false);
            pump_for(Duration::from_millis(90), Some(overlay));
            let snapshot = overlay.acceptance_snapshot();
            hud_final_display = snapshot.display_text.clone();
            hud_snapshots.push(snapshot.clone());
            trace_hud_snapshot(trace, "hud_final_flush", &case_id, snapshot)?;
            let hud_commit_text = hud_final_display.clone();
            trace.event(
                "hud_final_ack",
                json!({
                    "case_id": case_id,
                    "text": hud_commit_text.clone(),
                    "visible": overlay.acceptance_snapshot().visible,
                }),
            )?;
            commit_locked = true;

            refocus_target_for_commit(
                trace,
                "target_focus_before_commit",
                &case_id,
                target,
                overlay,
                &mut failures,
            )?;
            clear_target_before_commit(trace, &case_id, target, overlay)?;
            if !refocus_target_for_commit(
                trace,
                "target_focus_after_clear",
                &case_id,
                target,
                overlay,
                &mut failures,
            )? {
                continue;
            }
            stale_clipboard_text = stale_clipboard_sentinel(&case_id);
            set_clipboard_text(&stale_clipboard_text)?;
            commit_request_count += 1;
            trace.event(
                "output_commit_request",
                json!({
                    "case_id": case_id,
                    "text": hud_commit_text.clone(),
                    "resolved_commit_text": resolved_commit_text.clone(),
                    "stale_clipboard_sentinel": stale_clipboard_text,
                }),
            )?;
            let delivery_result = {
                let _voice_hotkey_suppression = hotkey::suppress_voice_hotkey_for_output();
                runtime
                    .output_controller()
                    .deliver_text(&hud_commit_text, output_config)
            };
            match delivery_result {
                Ok(delivery) => {
                    trace.event(
                        "output_commit_result",
                        json!({
                            "case_id": case_id,
                            "delivery": format!("{delivery:?}"),
                        }),
                    )?;
                }
                Err(error) => {
                    failures.push(failure(
                        &case_id,
                        "output_commit_failed",
                        format!("deliver_text failed: {error}"),
                    ));
                    trace.event(
                        "output_commit_result",
                        json!({
                            "case_id": case_id,
                            "error": error.to_string(),
                        }),
                    )?;
                }
            }
            let post_commit_text =
                read_target_after_commit_observation(target, &hud_commit_text, overlay)
                    .unwrap_or_else(|error| {
                        failures.push(failure(
                            &case_id,
                            "target_readback_unavailable",
                            format!("read target after commit failed: {error}"),
                        ));
                        String::new()
                    });
            trace.event(
                "target_post_commit_observed",
                json!({
                    "case_id": case_id,
                    "text": post_commit_text,
                    "observe_window_ms": POST_COMMIT_DUPLICATE_OBSERVE_WINDOW.as_millis(),
                }),
            )?;
            overlay.show_status_hud(&hud_commit_text, false, false);
            pump_for(Duration::from_millis(120), Some(overlay));
            let snapshot = overlay.acceptance_snapshot();
            trace_hud_snapshot(trace, "hud_after_commit_hold", &case_id, snapshot.clone())?;
            reject_missing_final_hud_hold(&case_id, &snapshot, &hud_commit_text, &mut failures);
        }
    }

    if final_text.trim().is_empty() {
        failures.push(failure(
            &case_id,
            "stability_regression",
            "case did not contain final_text",
        ));
    }
    if commit_envelope_count != 1 {
        failures.push(failure(
            &case_id,
            "commit_envelope_count",
            format!("expected exactly one commit envelope, got {commit_envelope_count}"),
        ));
    }
    if post_hud_flush_mutation_count > 0 {
        failures.push(failure(
            &case_id,
            "post_hud_flush_mutation",
            format!("post_hud_flush_mutation_count={post_hud_flush_mutation_count}"),
        ));
    }
    reject_final_tail_drop(&case_id, &last_partial_text, &final_text, &mut failures);
    reject_commit_request_count(&case_id, commit_request_count, &mut failures);

    let target_readback = target.read_text().unwrap_or_else(|error| {
        failures.push(failure(
            &case_id,
            "target_readback_unavailable",
            format!("read target text failed: {error}"),
        ));
        String::new()
    });
    trace.event(
        "target_readback",
        json!({
            "case_id": case_id,
            "text": target_readback,
        }),
    )?;

    let expected = case.expected_text.as_deref().unwrap_or(&final_text);
    let hud_stability = evaluate_hud_stability(&case_id, &hud_snapshots, &mut failures);
    if normalize_visible_text(&hud_final_display) != normalize_visible_text(&final_text) {
        failures.push(failure(
            &case_id,
            "hud_commit_diverged",
            format!(
                "hud_final_display='{}' final_text='{}'",
                hud_final_display, final_text
            ),
        ));
    }
    if normalize_visible_text(&target_readback) != normalize_visible_text(expected) {
        failures.push(failure(
            &case_id,
            "target_readback_mismatch",
            format!(
                "target_readback='{}' expected='{}'",
                target_readback, expected
            ),
        ));
    }
    reject_duplicate_or_extra_commit_readback(&case_id, &target_readback, expected, &mut failures);
    if !stale_clipboard_text.is_empty()
        && normalize_visible_text(&target_readback) == normalize_visible_text(&stale_clipboard_text)
    {
        failures.push(failure(
            &case_id,
            "clipboard_stale_paste",
            "target received the pre-existing clipboard text instead of the recognition result",
        ));
    }

    let status = if failures.is_empty() { "pass" } else { "fail" }.to_string();
    trace.event(
        "case_finished",
        json!({
            "case_id": case_id,
            "status": status,
            "failure_count": failures.len(),
        }),
    )?;

    Ok(LiveE2eCaseReport {
        case_id,
        status,
        final_text,
        resolved_commit_text,
        target_readback,
        hud_final_display,
        partial_count,
        commit_envelope_count,
        commit_request_count,
        post_hud_flush_mutation_count,
        hud_stability,
        failures,
    })
}

fn run_replay_case_visible(
    runtime: &AppRuntime,
    replay: &StreamingReplayReport,
    overlay: &mut RecordingOverlay,
    target: &AcceptanceTargetWindow,
    output_config: &ainput_output::OutputConfig,
    trace: &mut TraceWriter,
) -> Result<LiveE2eCaseReport> {
    let case_id = replay.case_id.clone();
    let mut failures = Vec::new();
    let mut hud_snapshots = Vec::new();
    let target_initial_text = reset_case_surface(overlay, target)
        .with_context(|| format!("reset visible acceptance surface for {}", case_id))?;
    let hud_initial_snapshot = overlay.acceptance_snapshot();
    trace.event(
        "case_started",
        json!({
            "case_id": case_id,
            "mode": "wav",
            "input_wav": replay.input_wav,
            "core_behavior_status": replay.behavior_status,
            "core_content_status": replay.content_status,
            "target_initial_text": target_initial_text,
        }),
    )?;
    trace_hud_snapshot(
        trace,
        "hud_after_case_reset",
        &case_id,
        hud_initial_snapshot.clone(),
    )?;
    reject_dirty_initial_target(&case_id, &target_initial_text, &mut failures);
    reject_dirty_initial_hud(&case_id, &hud_initial_snapshot, &mut failures);

    if replay.behavior_status != StreamingCaseStatus::Pass {
        failures.push(failure(
            &case_id,
            "asr_behavior_failure",
            format!("core behavior failed: {:?}", replay.failures),
        ));
    }
    if replay.content_status != StreamingCaseStatus::Pass {
        failures.push(failure(
            &case_id,
            "asr_content_failure",
            format!("core content failed: {:?}", replay.failures),
        ));
    }

    let mut previous_partial_text = String::new();
    let mut last_partial_text = String::new();
    for (index, partial) in replay.partial_timeline.iter().enumerate() {
        reject_duplicate_or_conflicting_punctuation(
            &case_id,
            &partial.prepared_text,
            "hud_duplicate_punctuation",
            &mut failures,
        );
        if partial.source == "endpoint_rollover"
            && has_terminal_sentence_punctuation(&partial.prepared_text)
            && !has_terminal_sentence_punctuation(&previous_partial_text)
        {
            failures.push(failure(
                &case_id,
                "hud_punctuation_forced_by_pause",
                "endpoint rollover added terminal punctuation after an unterminated partial",
            ));
        }
        previous_partial_text = partial.prepared_text.clone();
        last_partial_text = partial.prepared_text.clone();
        trace.event(
            "worker_partial",
            json!({
                "case_id": case_id,
                "revision": index + 1,
                "offset_ms": partial.offset_ms,
                "raw_text": partial.raw_text,
                "prepared_text": partial.prepared_text,
                "source": partial.source,
            }),
        )?;
        overlay.show_status_hud(&partial.prepared_text, true, true);
        pump_for(Duration::from_millis(45), Some(overlay));
        let snapshot = overlay.acceptance_snapshot();
        hud_snapshots.push(snapshot.clone());
        trace_hud_snapshot(trace, "hud_after_partial", &case_id, snapshot)?;
    }

    let recognition_final_text = replay.final_text.clone();
    let final_text = ensure_terminal_sentence_boundary(&recognition_final_text);
    let resolved_commit_text = final_text.clone();
    let commit_envelope_count = 1usize;
    let post_hud_flush_mutation_count = 0usize;
    reject_duplicate_or_conflicting_punctuation(
        &case_id,
        &final_text,
        "hud_duplicate_punctuation",
        &mut failures,
    );
    reject_final_tail_drop(&case_id, &last_partial_text, &final_text, &mut failures);
    trace.event(
        "commit_envelope_created",
        json!({
            "case_id": case_id,
            "revision": replay.partial_timeline.len() + 1,
            "last_hud_target_text": last_partial_text.clone(),
            "recognition_final_text": recognition_final_text,
            "resolved_commit_text": resolved_commit_text.clone(),
            "final_text": final_text.clone(),
            "commit_source": replay.commit_source.clone(),
        }),
    )?;
    overlay.show_status_hud(&final_text, true, false);
    pump_for(Duration::from_millis(90), Some(overlay));
    let snapshot = overlay.acceptance_snapshot();
    let hud_final_display = snapshot.display_text.clone();
    hud_snapshots.push(snapshot.clone());
    trace_hud_snapshot(trace, "hud_final_flush", &case_id, snapshot)?;
    let hud_commit_text = hud_final_display.clone();
    trace.event(
        "hud_final_ack",
        json!({
            "case_id": case_id,
            "text": hud_commit_text.clone(),
            "visible": overlay.acceptance_snapshot().visible,
        }),
    )?;

    refocus_target_for_commit(
        trace,
        "target_focus_before_commit",
        &case_id,
        target,
        overlay,
        &mut failures,
    )?;
    clear_target_before_commit(trace, &case_id, target, overlay)?;
    let target_ready = refocus_target_for_commit(
        trace,
        "target_focus_after_clear",
        &case_id,
        target,
        overlay,
        &mut failures,
    )?;
    let stale_clipboard_text = stale_clipboard_sentinel(&case_id);
    set_clipboard_text(&stale_clipboard_text)?;
    let mut commit_request_count = 0usize;
    if target_ready {
        commit_request_count += 1;
        trace.event(
            "output_commit_request",
            json!({
                "case_id": case_id,
                "text": hud_commit_text.clone(),
                "resolved_commit_text": resolved_commit_text.clone(),
                "stale_clipboard_sentinel": stale_clipboard_text,
            }),
        )?;
        let delivery_result = {
            let _voice_hotkey_suppression = hotkey::suppress_voice_hotkey_for_output();
            runtime
                .output_controller()
                .deliver_text(&hud_commit_text, output_config)
        };
        match delivery_result {
            Ok(delivery) => {
                trace.event(
                    "output_commit_result",
                    json!({
                        "case_id": case_id,
                        "delivery": format!("{delivery:?}"),
                    }),
                )?;
            }
            Err(error) => {
                failures.push(failure(
                    &case_id,
                    "output_commit_failed",
                    format!("deliver_text failed: {error}"),
                ));
                trace.event(
                    "output_commit_result",
                    json!({
                        "case_id": case_id,
                        "error": error.to_string(),
                    }),
                )?;
            }
        }
    }

    let target_readback = read_target_after_commit_observation(target, &hud_commit_text, overlay)
        .unwrap_or_else(|error| {
            failures.push(failure(
                &case_id,
                "target_readback_unavailable",
                format!("read target text failed: {error}"),
            ));
            String::new()
        });
    trace.event(
        "target_readback",
        json!({
            "case_id": case_id,
            "text": target_readback,
            "observe_window_ms": POST_COMMIT_DUPLICATE_OBSERVE_WINDOW.as_millis(),
        }),
    )?;

    overlay.show_status_hud(&hud_commit_text, false, false);
    pump_for(Duration::from_millis(120), Some(overlay));
    let snapshot = overlay.acceptance_snapshot();
    trace_hud_snapshot(trace, "hud_after_commit_hold", &case_id, snapshot.clone())?;
    reject_missing_final_hud_hold(&case_id, &snapshot, &hud_commit_text, &mut failures);

    let hud_stability = evaluate_hud_stability(&case_id, &hud_snapshots, &mut failures);
    if commit_envelope_count != 1 {
        failures.push(failure(
            &case_id,
            "commit_envelope_count",
            format!("expected exactly one commit envelope, got {commit_envelope_count}"),
        ));
    }
    if post_hud_flush_mutation_count > 0 {
        failures.push(failure(
            &case_id,
            "post_hud_flush_mutation",
            format!("post_hud_flush_mutation_count={post_hud_flush_mutation_count}"),
        ));
    }
    reject_commit_request_count(&case_id, commit_request_count, &mut failures);
    if normalize_visible_text(&hud_final_display) != normalize_visible_text(&final_text) {
        failures.push(failure(
            &case_id,
            "hud_commit_diverged",
            format!(
                "hud_final_display='{}' final_text='{}'",
                hud_final_display, final_text
            ),
        ));
    }
    if normalize_visible_text(&target_readback) != normalize_visible_text(&final_text) {
        failures.push(failure(
            &case_id,
            "target_readback_mismatch",
            format!(
                "target_readback='{}' final_text='{}'",
                target_readback, final_text
            ),
        ));
    }
    reject_duplicate_or_extra_commit_readback(
        &case_id,
        &target_readback,
        &final_text,
        &mut failures,
    );
    if normalize_visible_text(&target_readback) == normalize_visible_text(&stale_clipboard_text) {
        failures.push(failure(
            &case_id,
            "clipboard_stale_paste",
            "target received the pre-existing clipboard text instead of the recognition result",
        ));
    }

    let status = if failures.is_empty() { "pass" } else { "fail" }.to_string();
    trace.event(
        "case_finished",
        json!({
            "case_id": case_id,
            "status": status,
            "failure_count": failures.len(),
        }),
    )?;

    Ok(LiveE2eCaseReport {
        case_id,
        status,
        final_text,
        resolved_commit_text,
        target_readback,
        hud_final_display,
        partial_count: replay.partial_timeline.len(),
        commit_envelope_count,
        commit_request_count,
        post_hud_flush_mutation_count,
        hud_stability,
        failures,
    })
}

fn reset_case_surface(
    overlay: &mut RecordingOverlay,
    target: &AcceptanceTargetWindow,
) -> Result<String> {
    target.focus();
    ainput_output::copy_to_clipboard("").context("clear clipboard before acceptance case")?;
    pump_for(Duration::from_millis(120), Some(overlay));
    target.clear()?;
    pump_for(Duration::from_millis(80), Some(overlay));
    let mut target_initial_text = target.read_text().unwrap_or_default();
    if !target_initial_text.is_empty() {
        target.clear()?;
        pump_for(Duration::from_millis(80), Some(overlay));
        target_initial_text = target.read_text().unwrap_or_default();
    }
    overlay.reset_status_hud_for_acceptance();
    Ok(target_initial_text)
}

fn reject_dirty_initial_target(
    case_id: &str,
    target_initial_text: &str,
    failures: &mut Vec<LiveE2eFailure>,
) {
    if !target_initial_text.is_empty() {
        failures.push(failure(
            case_id,
            "target_readback_unavailable",
            format!("target text was not empty after reset: {target_initial_text}"),
        ));
    }
}

fn reject_dirty_initial_hud(
    case_id: &str,
    snapshot: &HudAcceptanceSnapshot,
    failures: &mut Vec<LiveE2eFailure>,
) {
    if !snapshot.display_text.trim().is_empty() || !snapshot.target_text.trim().is_empty() {
        failures.push(failure(
            case_id,
            "hud_stale_text",
            format!(
                "HUD was not empty at case start: target='{}' display='{}'",
                snapshot.target_text, snapshot.display_text
            ),
        ));
    }
}

fn reject_missing_final_hud_hold(
    case_id: &str,
    snapshot: &HudAcceptanceSnapshot,
    final_text: &str,
    failures: &mut Vec<LiveE2eFailure>,
) {
    if !snapshot.visible {
        failures.push(failure(
            case_id,
            "hud_final_hold_missing",
            "HUD disappeared before the final text hold window completed",
        ));
    }
    if normalize_visible_text(&snapshot.display_text) != normalize_visible_text(final_text) {
        failures.push(failure(
            case_id,
            "hud_text_diverged",
            format!(
                "HUD hold display='{}' final_text='{}'",
                snapshot.display_text, final_text
            ),
        ));
    }
}

fn read_target_after_commit_observation(
    target: &AcceptanceTargetWindow,
    expected: &str,
    overlay: &mut RecordingOverlay,
) -> Result<String> {
    let _ = target.read_text_until(expected, Duration::from_millis(1_200), overlay)?;
    pump_for(POST_COMMIT_DUPLICATE_OBSERVE_WINDOW, Some(overlay));
    target.read_text()
}

fn evaluate_hud_stability(
    case_id: &str,
    snapshots: &[HudAcceptanceSnapshot],
    failures: &mut Vec<LiveE2eFailure>,
) -> HudStabilityReport {
    let mut report = HudStabilityReport {
        samples: snapshots.len(),
        ..Default::default()
    };
    let Some(first) = snapshots.first() else {
        failures.push(failure(
            case_id,
            "hud_stability_unavailable",
            "no HUD snapshots were captured",
        ));
        return report;
    };

    let first_width = first.rect[2].saturating_sub(first.rect[0]);
    let first_height = first.rect[3].saturating_sub(first.rect[1]);
    let first_center_x = (first.rect[0] + first.rect[2]) / 2;
    let mut previous_alpha = first.alpha;
    for snapshot in snapshots {
        let width = snapshot.rect[2].saturating_sub(snapshot.rect[0]);
        let height = snapshot.rect[3].saturating_sub(snapshot.rect[1]);
        let center_x = (snapshot.rect[0] + snapshot.rect[2]) / 2;
        report.max_left_delta_px = report
            .max_left_delta_px
            .max((snapshot.rect[0] - first.rect[0]).abs());
        report.max_top_delta_px = report
            .max_top_delta_px
            .max((snapshot.rect[1] - first.rect[1]).abs());
        report.max_center_x_delta_px = report
            .max_center_x_delta_px
            .max((center_x - first_center_x).abs());
        report.max_width_delta_px = report.max_width_delta_px.max((width - first_width).abs());
        report.max_height_delta_px = report
            .max_height_delta_px
            .max((height - first_height).abs());
        if is_light_hud_background(&snapshot.background_color) && snapshot.alpha >= 80 {
            report.white_panel_sample_count += 1;
        }
        let single_line_height =
            snapshot.font_height_px + snapshot.padding_y_px.saturating_mul(2) + 18;
        if height > single_line_height.max(snapshot.font_height_px + 14) {
            report.multiline_panel_sample_count += 1;
        }
        let display_chars = normalize_visible_text(&snapshot.display_text)
            .chars()
            .count() as i32;
        let short_text_max_width =
            snapshot.font_height_px.saturating_mul(5) + snapshot.padding_x_px.saturating_mul(2);
        if display_chars > 0 && display_chars <= 4 && width > short_text_max_width.max(120) {
            report.short_text_wide_panel_count += 1;
        }
        if !snapshot.visible {
            report.invisible_sample_count += 1;
        }
        if snapshot.alpha.saturating_add(8) < previous_alpha {
            report.alpha_drop_count += 1;
        }
        previous_alpha = snapshot.alpha;
    }

    if report.max_center_x_delta_px > 3
        || report.max_top_delta_px > 3
        || report.max_height_delta_px > 3
    {
        failures.push(failure(
            case_id,
            "hud_jitter",
            format!(
                "HUD rect is not centered/stable enough: center={}px left={}px top={}px width={}px height={}px",
                report.max_center_x_delta_px,
                report.max_left_delta_px,
                report.max_top_delta_px,
                report.max_width_delta_px,
                report.max_height_delta_px
            ),
        ));
    }
    if report.white_panel_sample_count > 0 {
        failures.push(failure(
            case_id,
            "hud_white_panel",
            format!(
                "HUD still looks like a white panel in {} samples",
                report.white_panel_sample_count
            ),
        ));
    }
    if report.multiline_panel_sample_count > 0 {
        failures.push(failure(
            case_id,
            "hud_multiline_panel",
            format!(
                "HUD exceeded single-line height in {} samples",
                report.multiline_panel_sample_count
            ),
        ));
    }
    if report.short_text_wide_panel_count > 0 {
        failures.push(failure(
            case_id,
            "hud_short_text_wide_panel",
            format!(
                "HUD was too wide for short text in {} samples",
                report.short_text_wide_panel_count
            ),
        ));
    }
    if report.alpha_drop_count > 0 || report.invisible_sample_count > 0 {
        failures.push(failure(
            case_id,
            "hud_flicker",
            format!(
                "HUD flicker samples: alpha_drop_count={} invisible_sample_count={}",
                report.alpha_drop_count, report.invisible_sample_count
            ),
        ));
    }

    report
}

fn trace_hud_snapshot(
    trace: &mut TraceWriter,
    event: &str,
    case_id: &str,
    snapshot: HudAcceptanceSnapshot,
) -> Result<()> {
    trace.event(
        event,
        json!({
            "case_id": case_id,
            "hud": snapshot,
        }),
    )
}

fn trace_target_focus_snapshot(
    trace: &mut TraceWriter,
    event: &str,
    case_id: &str,
    target: &AcceptanceTargetWindow,
) -> Result<()> {
    trace.event(
        event,
        json!({
            "case_id": case_id,
            "target_hwnd": target.hwnd.0 as usize,
            "edit_hwnd": target.edit_hwnd.0 as usize,
            "focused_hwnd": target.focused_hwnd_value(),
            "foreground_hwnd": foreground_hwnd_value(),
            "foreground_is_target": target.is_foreground(),
            "edit_is_focused": target.edit_is_focused(),
            "target_text": target.read_text().unwrap_or_default(),
        }),
    )
}

fn refocus_target_for_commit(
    trace: &mut TraceWriter,
    event: &str,
    case_id: &str,
    target: &AcceptanceTargetWindow,
    overlay: &mut RecordingOverlay,
    failures: &mut Vec<LiveE2eFailure>,
) -> Result<bool> {
    for attempt in 1..=3 {
        let focus_reported_ready = target.focus();
        pump_for(Duration::from_millis(80), Some(overlay));
        if target.is_foreground() && target.edit_is_focused() {
            trace.event(
                "target_refocus_ready",
                json!({
                    "case_id": case_id,
                    "event": event,
                    "attempt": attempt,
                    "focus_reported_ready": focus_reported_ready,
                }),
            )?;
            trace_target_focus_snapshot(trace, event, case_id, target)?;
            return Ok(true);
        }
        trace.event(
            "target_refocus_retry",
            json!({
                "case_id": case_id,
                "event": event,
                "attempt": attempt,
                "focus_reported_ready": focus_reported_ready,
                "foreground_hwnd": foreground_hwnd_value(),
                "foreground_is_target": target.is_foreground(),
                "edit_is_focused": target.edit_is_focused(),
            }),
        )?;
    }

    trace_target_focus_snapshot(trace, event, case_id, target)?;
    failures.push(failure(
        case_id,
        "target_focus_unavailable",
        format!("target was not ready before commit at {event}"),
    ));
    Ok(false)
}

fn clear_target_before_commit(
    trace: &mut TraceWriter,
    case_id: &str,
    target: &AcceptanceTargetWindow,
    overlay: &mut RecordingOverlay,
) -> Result<()> {
    let before = target.read_text().unwrap_or_default();
    if before.is_empty() {
        return Ok(());
    }
    trace.event(
        "target_precommit_dirty",
        json!({
            "case_id": case_id,
            "text": before,
        }),
    )?;
    target.clear()?;
    pump_for(Duration::from_millis(120), Some(overlay));
    trace.event(
        "target_precommit_cleared",
        json!({
            "case_id": case_id,
            "text": target.read_text().unwrap_or_default(),
        }),
    )
}

fn failure(case_id: &str, category: &str, message: impl Into<String>) -> LiveE2eFailure {
    LiveE2eFailure {
        case_id: case_id.to_string(),
        category: category.to_string(),
        message: message.into(),
    }
}

fn normalize_visible_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
}

fn content_text_without_punctuation(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_sentence_punctuation(*ch))
        .collect::<String>()
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

fn has_terminal_sentence_punctuation(text: &str) -> bool {
    text.trim()
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?' | ';' | '。' | '！' | '？' | '；'))
}

fn ensure_terminal_sentence_boundary(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || has_terminal_sentence_punctuation(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed}。")
    }
}

fn has_duplicate_or_conflicting_punctuation(text: &str) -> bool {
    let chars = text.chars().collect::<Vec<_>>();
    for pair in chars.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if is_comma_punctuation(left) && is_comma_punctuation(right) {
            return true;
        }
        if is_terminal_punctuation(left) && is_terminal_punctuation(right) {
            return true;
        }
        if (is_comma_punctuation(left) && is_terminal_punctuation(right))
            || (is_terminal_punctuation(left) && is_comma_punctuation(right))
        {
            return true;
        }
    }
    false
}

fn is_comma_punctuation(ch: char) -> bool {
    matches!(ch, ',' | '，')
}

fn is_terminal_punctuation(ch: char) -> bool {
    matches!(ch, '.' | '!' | '?' | ';' | '。' | '！' | '？' | '；')
}

fn reject_duplicate_or_conflicting_punctuation(
    case_id: &str,
    text: &str,
    category: &str,
    failures: &mut Vec<LiveE2eFailure>,
) {
    if has_duplicate_or_conflicting_punctuation(text) {
        failures.push(failure(
            case_id,
            category,
            format!("text contains duplicate or conflicting punctuation: {text}"),
        ));
    }
}

fn reject_duplicate_or_extra_commit_readback(
    case_id: &str,
    target_readback: &str,
    expected: &str,
    failures: &mut Vec<LiveE2eFailure>,
) {
    let normalized_readback = normalize_visible_text(target_readback);
    let normalized_expected = normalize_visible_text(expected);
    if normalized_expected.is_empty() || normalized_readback == normalized_expected {
        return;
    }

    let occurrence_count = count_occurrences(&normalized_readback, &normalized_expected);
    if occurrence_count > 1 {
        failures.push(failure(
            case_id,
            "target_duplicate_commit",
            format!(
                "target contains the final text {occurrence_count} times: readback='{target_readback}' expected='{expected}'"
            ),
        ));
    } else if occurrence_count == 1 {
        failures.push(failure(
            case_id,
            "target_extra_commit_fragment",
            format!(
                "target contains final text plus extra committed text: readback='{target_readback}' expected='{expected}'"
            ),
        ));
    }
}

fn reject_commit_request_count(
    case_id: &str,
    commit_request_count: usize,
    failures: &mut Vec<LiveE2eFailure>,
) {
    if commit_request_count != 1 {
        failures.push(failure(
            case_id,
            "output_commit_count_mismatch",
            format!("expected exactly 1 output_commit_request, got {commit_request_count}"),
        ));
    }
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.match_indices(needle).count()
}

fn reject_final_tail_drop(
    case_id: &str,
    last_partial_text: &str,
    final_text: &str,
    failures: &mut Vec<LiveE2eFailure>,
) {
    if last_partial_text.trim().is_empty() || final_text.trim().is_empty() {
        return;
    }

    let last_content = content_text_without_punctuation(last_partial_text);
    let final_content = content_text_without_punctuation(final_text);
    let last_chars = last_content.chars().count();
    let final_chars = final_content.chars().count();
    if final_chars < last_chars {
        failures.push(failure(
            case_id,
            "hud_final_tail_dropped",
            format!(
                "final has fewer content chars than last HUD partial: final={} last_partial={}",
                final_chars, last_chars
            ),
        ));
    }

    if let Some(tail) = last_content.chars().next_back()
        && is_tail_particle(tail)
        && !final_content.contains(&last_content)
    {
        failures.push(failure(
            case_id,
            "hud_final_tail_dropped",
            format!("final dropped tail particle '{tail}' from last HUD partial"),
        ));
    }
}

fn is_tail_particle(ch: char) -> bool {
    matches!(
        ch,
        '了' | '啊' | '呢' | '吧' | '吗' | '呀' | '嘛' | '哦' | '噢' | '诶'
    )
}

fn is_light_hud_background(color: &str) -> bool {
    let Some(hex) = color.trim().strip_prefix('#') else {
        return false;
    };
    if hex.len() != 6 {
        return false;
    }
    let Ok(rgb) = u32::from_str_radix(hex, 16) else {
        return false;
    };
    let r = ((rgb >> 16) & 0xFF) as i32;
    let g = ((rgb >> 8) & 0xFF) as i32;
    let b = (rgb & 0xFF) as i32;
    r >= 220 && g >= 220 && b >= 220
}

fn stale_clipboard_sentinel(case_id: &str) -> String {
    format!("ainput stale clipboard sentinel {case_id}")
}

fn set_clipboard_text(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().context("open clipboard for acceptance sentinel")?;
    clipboard
        .set_text(text.to_string())
        .context("write acceptance clipboard sentinel")?;
    Ok(())
}

fn foreground_hwnd_value() -> usize {
    unsafe { GetForegroundWindow().0 as usize }
}

fn wait_until(started: Instant, target: Duration, overlay: &mut RecordingOverlay) {
    while started.elapsed() < target {
        pump_for(Duration::from_millis(12), Some(overlay));
    }
}

fn pump_for(duration: Duration, mut overlay: Option<&mut RecordingOverlay>) {
    let started = Instant::now();
    while started.elapsed() < duration {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if let Some(overlay) = overlay.as_deref_mut() {
            overlay.tick();
        }
        thread::sleep(Duration::from_millis(8));
    }
}

struct TraceWriter {
    run_id: String,
    started: Instant,
    writer: BufWriter<File>,
}

impl TraceWriter {
    fn create(path: PathBuf, run_id: &str) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create trace dir {}", parent.display()))?;
        }
        let file =
            File::create(&path).with_context(|| format!("create trace {}", path.display()))?;
        Ok(Self {
            run_id: run_id.to_string(),
            started: Instant::now(),
            writer: BufWriter::new(file),
        })
    }

    fn event(&mut self, event: &str, payload: serde_json::Value) -> Result<()> {
        let mut event_map = serde_json::Map::new();
        event_map.insert("event".to_string(), json!(event));
        event_map.insert("run_id".to_string(), json!(self.run_id));
        event_map.insert(
            "monotonic_ms".to_string(),
            json!(self.started.elapsed().as_millis()),
        );
        if let serde_json::Value::Object(payload) = payload {
            for (key, payload_value) in payload {
                event_map.insert(key, payload_value);
            }
        }
        serde_json::to_writer(&mut self.writer, &serde_json::Value::Object(event_map))?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

struct AcceptanceTargetWindow {
    hwnd: HWND,
    edit_hwnd: HWND,
}

impl AcceptanceTargetWindow {
    fn create(run_id: &str) -> Result<Self> {
        unsafe {
            let module = GetModuleHandleW(None)
                .map_err(|error| anyhow!("resolve module handle: {error}"))?;
            let instance = HINSTANCE(module.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(target_window_proc),
                hInstance: instance,
                lpszClassName: TARGET_CLASS_NAME,
                ..Default::default()
            };
            let _ = RegisterClassW(&class);

            let title = HSTRING::from(format!("ainput acceptance target {run_id}"));
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                TARGET_CLASS_NAME,
                &title,
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                760,
                260,
                None,
                None,
                Some(instance),
                None,
            )
            .map_err(|error| anyhow!("create acceptance target window failed: {error}"))?;

            let edit_hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                WINDOW_STYLE(
                    (WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | WS_VSCROLL).0
                        | ES_LEFT as u32
                        | ES_MULTILINE as u32
                        | ES_AUTOVSCROLL as u32
                        | ES_WANTRETURN as u32,
                ),
                16,
                16,
                710,
                180,
                Some(hwnd),
                None,
                Some(instance),
                None,
            )
            .map_err(|error| anyhow!("create acceptance edit control failed: {error}"))?;

            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(edit_hwnd));
            Ok(Self { hwnd, edit_hwnd })
        }
    }

    fn clear(&self) -> Result<()> {
        let empty = HSTRING::from("");
        unsafe {
            SetWindowTextW(self.edit_hwnd, &empty)
                .map_err(|error| anyhow!("clear acceptance target failed: {error}"))?;
        }
        Ok(())
    }

    fn focus(&self) -> bool {
        for _ in 0..20 {
            unsafe {
                let foreground = GetForegroundWindow();
                let foreground_thread = if foreground.0.is_null() {
                    0
                } else {
                    GetWindowThreadProcessId(foreground, None)
                };
                let current_thread = GetCurrentThreadId();
                let attach_foreground =
                    foreground_thread != 0 && foreground_thread != current_thread;
                if attach_foreground {
                    let _ = AttachThreadInput(current_thread, foreground_thread, true);
                }
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                let _ = BringWindowToTop(self.hwnd);
                let _ = SetActiveWindow(self.hwnd);
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(Some(self.edit_hwnd));
                if attach_foreground {
                    let _ = AttachThreadInput(current_thread, foreground_thread, false);
                }
            }
            pump_for(Duration::from_millis(50), None);
            if self.is_foreground() && self.edit_is_focused() {
                return true;
            }
        }
        pump_for(Duration::from_millis(80), None);
        self.is_foreground() && self.edit_is_focused()
    }

    fn is_foreground(&self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    fn focused_hwnd_value(&self) -> usize {
        unsafe {
            let thread_id = GetWindowThreadProcessId(self.hwnd, None);
            if thread_id == 0 {
                return 0;
            }
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if GetGUIThreadInfo(thread_id, &mut info).is_err() {
                return 0;
            }
            info.hwndFocus.0 as usize
        }
    }

    fn edit_is_focused(&self) -> bool {
        self.focused_hwnd_value() == self.edit_hwnd.0 as usize
    }

    fn read_text(&self) -> Result<String> {
        unsafe {
            let len = SendMessageW(self.edit_hwnd, WM_GETTEXTLENGTH, None, None).0 as usize;
            if len == 0 {
                return Ok(String::new());
            }
            let mut buffer = vec![0u16; len + 1];
            let copied = SendMessageW(
                self.edit_hwnd,
                WM_GETTEXT,
                Some(WPARAM(buffer.len())),
                Some(LPARAM(buffer.as_mut_ptr() as isize)),
            )
            .0 as usize;
            if copied == 0 {
                let fallback_len = GetWindowTextLengthW(self.edit_hwnd);
                if fallback_len <= 0 {
                    return Ok(String::new());
                }
                let mut fallback = vec![0u16; fallback_len as usize + 1];
                let copied = GetWindowTextW(self.edit_hwnd, &mut fallback);
                return Ok(String::from_utf16_lossy(&fallback[..copied as usize]));
            }
            Ok(String::from_utf16_lossy(&buffer[..copied]))
        }
    }

    fn read_text_until(
        &self,
        expected: &str,
        timeout: Duration,
        overlay: &mut RecordingOverlay,
    ) -> Result<String> {
        let started = Instant::now();
        let mut last_text = String::new();
        while started.elapsed() < timeout {
            last_text = self.read_text()?;
            if normalize_visible_text(&last_text) == normalize_visible_text(expected) {
                return Ok(last_text);
            }
            pump_for(Duration::from_millis(50), Some(overlay));
        }
        Ok(last_text)
    }
}

impl Drop for AcceptanceTargetWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn target_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn write_summary(report_dir: &Path, report: &LiveE2eReport) -> Result<()> {
    let mut lines = Vec::new();
    lines.push(format!("overall_status={}", report.overall_status));
    lines.push(format!(
        "passed_cases={}/{}",
        report.cases_passed, report.cases_total
    ));
    lines.push(format!("report_dir={}", report.report_dir));
    for case in &report.cases {
        lines.push(format!(
            "case={} status={} partials={} commits={} hud_delta(left/top/center/width/height)={}/{}/{}/{}/{} alpha_drops={} invisible={} white_panel={} multiline={} short_wide={} final='{}' readback='{}'",
            case.case_id,
            case.status,
            case.partial_count,
            case.commit_request_count,
            case.hud_stability.max_left_delta_px,
            case.hud_stability.max_top_delta_px,
            case.hud_stability.max_center_x_delta_px,
            case.hud_stability.max_width_delta_px,
            case.hud_stability.max_height_delta_px,
            case.hud_stability.alpha_drop_count,
            case.hud_stability.invisible_sample_count,
            case.hud_stability.white_panel_sample_count,
            case.hud_stability.multiline_panel_sample_count,
            case.hud_stability.short_text_wide_panel_count,
            case.final_text,
            case.target_readback
        ));
        for failure in &case.failures {
            lines.push(format!(
                "  failure={} {}",
                failure.category, failure.message
            ));
        }
    }
    fs::write(report_dir.join("summary.txt"), lines.join("\n"))?;
    Ok(())
}

fn new_run_id(mode: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{mode}-{millis}")
}

#[cfg(test)]
mod tests {
    use super::{
        count_occurrences, normalize_visible_text, reject_commit_request_count,
        reject_duplicate_or_extra_commit_readback,
    };

    #[test]
    fn normalize_visible_text_removes_only_whitespace() {
        assert_eq!(normalize_visible_text(" HUD 上屏\n一致 "), "HUD上屏一致");
    }

    #[test]
    fn duplicate_commit_readback_is_a_named_failure() {
        let mut failures = Vec::new();
        reject_duplicate_or_extra_commit_readback(
            "case",
            "你好，世界。你好，世界。",
            "你好，世界。",
            &mut failures,
        );
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].category, "target_duplicate_commit");
    }

    #[test]
    fn extra_commit_fragment_is_a_named_failure() {
        let mut failures = Vec::new();
        reject_duplicate_or_extra_commit_readback(
            "case",
            "你好，世界。错误的第二段。",
            "你好，世界。",
            &mut failures,
        );
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].category, "target_extra_commit_fragment");
    }

    #[test]
    fn count_occurrences_counts_repeated_final_text() {
        assert_eq!(count_occurrences("abcabc", "abc"), 2);
        assert_eq!(count_occurrences("abc", ""), 0);
    }

    #[test]
    fn commit_request_count_must_be_exactly_one() {
        let mut failures = Vec::new();
        reject_commit_request_count("case", 2, &mut failures);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].category, "output_commit_count_mismatch");
    }
}
