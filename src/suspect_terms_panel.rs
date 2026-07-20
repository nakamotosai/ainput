use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::blocking::{Client, RequestBuilder};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::ai_rewrite::{AiRewriter, default_rewrite_prompt};
use crate::config::RewriteConfig;
use crate::config::{AppConfig, HudUserConfig};
use crate::history;
use crate::hud::HudController;
use crate::hud_font_panel;
use crate::personal_corrections;
use crate::suspect_terms::{
    self, SuspectTermItem, SuspectTermReviewUpdate, SuspectTermsController,
};

#[derive(Clone)]
pub struct SuspectTermsPanelController {
    url: String,
}

#[derive(Clone)]
struct ConsoleState {
    suspect_terms: SuspectTermsController,
    hud: HudController,
    history_path: PathBuf,
    hud_preview_path: PathBuf,
    debug_endpoint_url: String,
    debug_api_key: Option<String>,
    app_config_path: PathBuf,
    rewrite_model: String,
    prompt_rewriter: Option<AiRewriter>,
    http: Client,
}

impl SuspectTermsPanelController {
    pub fn start(
        suspect_terms: SuspectTermsController,
        hud: HudController,
        history_path: PathBuf,
        debug_endpoint_url: String,
        debug_api_key_env: String,
        debug_api_key: String,
        app_config_path: PathBuf,
        rewrite_model: String,
        prompt_config: RewriteConfig,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind ainput2 web console")?;
        listener
            .set_nonblocking(true)
            .context("set web console listener nonblocking")?;
        let port = listener
            .local_addr()
            .context("read web console local addr")?
            .port();
        let url = format!("http://127.0.0.1:{port}/");
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let worker_url = url.clone();
        thread::spawn(move || {
            let prompt_rewriter = match AiRewriter::new(prompt_config) {
                Ok(rewriter) => Some(rewriter),
                Err(error) => {
                    warn!(error = %error, "Prompt Studio web rewriter disabled");
                    None
                }
            };
            let state = ConsoleState {
                suspect_terms,
                hud,
                hud_preview_path: history_path
                    .parent()
                    .map(|parent| parent.join("hud-preview.json"))
                    .unwrap_or_else(|| PathBuf::from("hud-preview.json")),
                history_path,
                debug_endpoint_url,
                debug_api_key: read_api_key(&debug_api_key_env).or_else(|| {
                    let inline = debug_api_key.trim().to_string();
                    (!inline.is_empty()).then_some(inline)
                }),
                app_config_path,
                rewrite_model,
                prompt_rewriter,
                http: Client::builder()
                    .timeout(Duration::from_millis(2500))
                    .no_proxy()
                    .build()
                    .unwrap_or_else(|_| Client::new()),
            };
            let result = run_web_console(listener, state, shutdown);
            if let Err(error) = result {
                warn!(error = %error, "ainput2 web console failed");
            }
        });
        ready_tx
            .send(Ok(()))
            .map_err(|error| anyhow!("signal ainput2 web console ready failed: {error}"))?;
        ready_rx
            .recv_timeout(Duration::from_millis(100))
            .unwrap_or(Ok(()))
            .map_err(|error| anyhow!(error))?;
        info!(url = %worker_url, "ainput2 web console started");
        Ok(Self { url })
    }

    pub fn open(&self) {
        self.open_path("");
    }

    pub fn open_suspect_terms(&self) {
        self.open_path("suspect");
    }

    fn open_path(&self, path: &str) {
        let url = format!("{}{}", self.url, path.trim_start_matches('/'));
        if let Err(error) = open_url(&url) {
            warn!(error = %error, url, "open ainput2 web console failed");
        }
    }
}

fn run_web_console(
    listener: TcpListener,
    state: ConsoleState,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = handle_connection(stream, &state) {
                    warn!(error = %error, "ainput2 web console request failed");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error).context("accept web console connection"),
        }
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream, state: &ConsoleState) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .context("set web console read timeout")?;
    if let Err(error) = handle_request(&mut stream, state) {
        let message = error.to_string();
        warn!(error = %message, "ainput2 web console request failed");
        write_error_json(&mut stream, 500, &message)?;
    }
    Ok(())
}

fn handle_request(stream: &mut TcpStream, state: &ConsoleState) -> Result<()> {
    let buffer = read_http_request(stream)?;
    if buffer.is_empty() {
        return Ok(());
    }
    let request = String::from_utf8_lossy(&buffer);
    let Some((head, body)) = request.split_once("\r\n\r\n") else {
        return write_response(stream, 400, "text/plain; charset=utf-8", "bad request");
    };
    let mut lines = head.lines();
    let Some(request_line) = lines.next() else {
        return write_response(stream, 400, "text/plain; charset=utf-8", "bad request");
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let raw_path = parts.next().unwrap_or_default();
    let (path, query) = split_path_query(raw_path);
    match (method, path) {
        ("GET", "/") => write_response(stream, 200, "text/html; charset=utf-8", HOME_HTML),
        ("GET", "/suspect") => {
            write_response(stream, 200, "text/html; charset=utf-8", SUSPECT_HTML)
        }
        ("GET", "/hud") => write_response(stream, 200, "text/html; charset=utf-8", HUD_HTML),
        ("GET", "/corrections") => {
            write_response(stream, 200, "text/html; charset=utf-8", CORRECTIONS_HTML)
        }
        ("GET", "/history") => {
            write_response(stream, 200, "text/html; charset=utf-8", HISTORY_HTML)
        }
        ("GET", "/debug") => write_response(stream, 200, "text/html; charset=utf-8", DEBUG_HTML),
        ("GET", "/prompt") => write_response(stream, 200, "text/html; charset=utf-8", PROMPT_HTML),
        ("GET", "/settings") => {
            write_response(stream, 200, "text/html; charset=utf-8", SETTINGS_HTML)
        }
        ("GET", "/api/book") | ("GET", "/api/suspect/book") => {
            let status = query_value(query, "status").unwrap_or("pending");
            let payload = book_json(
                state.suspect_terms.suspect_path().to_path_buf(),
                state.suspect_terms.corrections_path(),
                status,
            )?;
            write_json(stream, &payload)
        }
        ("POST", "/api/apply") | ("POST", "/api/suspect/apply") => {
            let payload = apply_updates_json(&state.suspect_terms, body)?;
            write_json(stream, &payload)
        }
        ("POST", "/api/analyze") | ("POST", "/api/suspect/analyze-now") => {
            state.suspect_terms.analyze_now();
            write_json(stream, r#"{"ok":true}"#)
        }
        ("POST", "/api/open-logs") | ("POST", "/api/logs/open") => {
            let payload = open_logs_json(state.suspect_terms.suspect_path().to_path_buf())?;
            write_json(stream, &payload)
        }
        ("GET", "/api/hud/config") => {
            let payload = serde_json::to_string(&json!({
                "ok": true,
                "config": state.hud.hud_user_config()
            }))
            .context("serialize HUD config")?;
            write_json(stream, &payload)
        }
        ("GET", "/api/hud/fonts") => {
            let current = state
                .hud
                .hud_user_config()
                .font_family
                .unwrap_or_else(|| "Microsoft YaHei UI".to_string());
            let payload = serde_json::to_string(&json!({
                "ok": true,
                "fonts": hud_font_panel::installed_font_families(&current)
            }))
            .context("serialize HUD fonts")?;
            write_json(stream, &payload)
        }
        ("GET", "/api/hud/preview-text") => {
            let text = load_hud_preview_text(&state.hud_preview_path);
            let payload = serde_json::to_string(&json!({"ok": true, "text": text}))
                .context("serialize HUD preview text")?;
            write_json(stream, &payload)
        }
        ("POST", "/api/hud/config") => {
            let request: HudUserConfig =
                serde_json::from_str(body).context("parse HUD config request")?;
            let config = state.hud.apply_hud_user_config(request)?;
            state
                .hud
                .show_text("HUD 预览：这里就是真实 HUD", true, false);
            let payload = serde_json::to_string(&json!({"ok": true, "config": config}))
                .context("serialize HUD apply response")?;
            write_json(stream, &payload)
        }
        ("POST", "/api/hud/preview") => {
            let text = parse_preview_text(body);
            save_hud_preview_text(&state.hud_preview_path, &text)?;
            state.hud.show_text(&text, true, false);
            write_json(stream, r#"{"ok":true}"#)
        }
        ("GET", "/api/corrections") => {
            let payload = corrections_json(state.suspect_terms.corrections_path())?;
            write_json(stream, &payload)
        }
        ("POST", "/api/corrections/add") => {
            let payload = add_correction_json(state.suspect_terms.corrections_path(), body)?;
            write_json(stream, &payload)
        }
        ("POST", "/api/corrections/update") => {
            let payload = update_correction_json(state.suspect_terms.corrections_path(), body)?;
            write_json(stream, &payload)
        }
        ("POST", "/api/protected/add") => {
            let payload = add_protected_json(state.suspect_terms.corrections_path(), body)?;
            write_json(stream, &payload)
        }
        ("POST", "/api/protected/update") => {
            let payload = update_protected_json(state.suspect_terms.corrections_path(), body)?;
            write_json(stream, &payload)
        }
        ("GET", "/api/history") => {
            let payload = history_json(state.history_path.clone())?;
            write_json(stream, &payload)
        }
        ("GET", "/api/debug/settings") => {
            let payload = debug_settings_json(state)?;
            write_json(stream, &payload)
        }
        ("GET", "/api/prompt/latest") => {
            let payload = prompt_latest_json(state.history_path.clone())?;
            write_json(stream, &payload)
        }
        ("POST", "/api/prompt/test") => {
            let payload = prompt_test_json(state, body)?;
            write_json(stream, &payload)
        }
        ("GET", "/api/settings/rewrite") => {
            let payload = rewrite_settings_json(state)?;
            write_json(stream, &payload)
        }
        ("POST", "/api/settings/rewrite") => {
            let payload = update_rewrite_settings_json(state, body)?;
            write_json(stream, &payload)
        }
        _ => write_response(stream, 404, "text/plain; charset=utf-8", "not found"),
    }
}

fn split_path_query(raw_path: &str) -> (&str, &str) {
    raw_path.split_once('?').unwrap_or((raw_path, ""))
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then_some(value)
    })
}

fn read_http_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(16 * 1024);
    let mut chunk = [0u8; 8192];
    let mut sent_continue = false;
    loop {
        let read = stream
            .read(&mut chunk)
            .context("read web console request")?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = find_header_end(&buffer) {
            let header = &buffer[..header_end];
            let content_length = parse_content_length(header).unwrap_or(0);
            let expected = header_end + 4 + content_length;
            if !sent_continue && buffer.len() < expected && has_expect_continue(header) {
                stream
                    .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                    .context("write web console continue response")?;
                sent_continue = true;
            }
            if buffer.len() >= expected {
                buffer.truncate(expected);
                break;
            }
        }
        if buffer.len() > 512 * 1024 {
            return Err(anyhow!("web console request is too large"));
        }
    }
    Ok(buffer)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header: &[u8]) -> Option<usize> {
    let header = String::from_utf8_lossy(header);
    header.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

fn has_expect_continue(header: &[u8]) -> bool {
    let header = String::from_utf8_lossy(header);
    header.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("expect")
            && value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("100-continue"))
    })
}

fn write_json(stream: &mut TcpStream, body: &str) -> Result<()> {
    write_response(stream, 200, "application/json; charset=utf-8", body)
}

fn write_error_json(stream: &mut TcpStream, status: u16, error: &str) -> Result<()> {
    let body = serde_json::to_string(&json!({
        "ok": false,
        "error": error
    }))
    .context("serialize web console error")?;
    write_response(stream, status, "application/json; charset=utf-8", &body)
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let bytes = body.as_bytes();
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(bytes))
        .context("write web console response")
}

fn book_json(path: PathBuf, corrections_path: &std::path::Path, status: &str) -> Result<String> {
    let book = suspect_terms::load_book(&path)?;
    let learned = learned_correction_items(corrections_path, &book)?;
    let pending = book
        .items
        .iter()
        .filter(|item| item.status == "pending")
        .count();
    let applied = book
        .items
        .iter()
        .filter(|item| item.status == "applied")
        .count();
    let dismissed = book
        .items
        .iter()
        .filter(|item| item.status == "dismissed")
        .count();
    let mut items = book
        .items
        .iter()
        .filter(|item| match status {
            "all" => true,
            "applied" => item.status == "applied",
            "dismissed" => item.status == "dismissed",
            _ => item.status == "pending",
        })
        .map(item_json)
        .collect::<Vec<_>>();
    if matches!(status, "all" | "applied") {
        items.extend(learned.iter().map(item_json));
    }
    Ok(json!({
        "ok": true,
        "updated_ms": book.updated_ms,
        "last_analyzed_history_ms": book.last_analyzed_history_ms,
        "counts": {
            "visible": items.len(),
            "pending": pending,
            "applied": applied,
            "dismissed": dismissed,
            "learned": learned.len()
        },
        "items": items
    })
    .to_string())
}

fn learned_correction_items(
    corrections_path: &std::path::Path,
    book: &suspect_terms::SuspectTermBook,
) -> Result<Vec<SuspectTermItem>> {
    let store = personal_corrections::load_store(corrections_path)?;
    let mut items = Vec::new();
    for (index, rule) in store.rules.iter().enumerate() {
        if !rule.enabled || rule.wrong.trim().is_empty() || rule.correct.trim().is_empty() {
            continue;
        }
        if book
            .items
            .iter()
            .any(|item| item.wrong == rule.wrong && item.suggested == rule.correct)
        {
            continue;
        }
        items.push(SuspectTermItem {
            id: format!("learned-correction-{index}"),
            wrong: rule.wrong.clone(),
            suggested: rule.correct.clone(),
            reason: "已学习的个人纠错规则".to_string(),
            examples: Vec::new(),
            confidence: 1.0,
            status: "applied".to_string(),
            created_ms: rule.created_ms,
            updated_ms: rule.updated_ms,
            source: rule.source.clone(),
        });
    }
    Ok(items)
}

fn item_json(item: &SuspectTermItem) -> serde_json::Value {
    json!({
        "id": item.id,
        "wrong": item.wrong,
        "suggested": item.suggested,
        "reason": item.reason,
        "examples": item.examples,
        "confidence": item.confidence,
        "status": item.status,
        "updated_ms": item.updated_ms
    })
}

#[derive(Debug, Deserialize)]
struct ApplyRequest {
    updates: Vec<ApplyUpdateRequest>,
}

#[derive(Debug, Deserialize)]
struct ApplyUpdateRequest {
    id: String,
    suggested: String,
    #[serde(default)]
    dismiss: bool,
}

fn apply_updates_json(controller: &SuspectTermsController, body: &str) -> Result<String> {
    let updates = parse_apply_updates(body)?;
    let result = suspect_terms::apply_review_updates(
        controller.suspect_path(),
        controller.corrections_path(),
        &updates,
    )?;
    let remaining_pending = suspect_terms::load_book(controller.suspect_path())?
        .items
        .iter()
        .filter(|item| item.status == "pending")
        .count();
    Ok(json!({
        "ok": true,
        "applied": result.applied.len(),
        "dismissed": result.dismissed.len(),
        "disabled_rules": result.disabled_rules,
        "remaining_pending": remaining_pending
    })
    .to_string())
}

fn parse_apply_updates(body: &str) -> Result<Vec<SuspectTermReviewUpdate>> {
    let body = body.trim_start_matches('\u{feff}');
    let request: ApplyRequest =
        serde_json::from_str(body).context("parse suspect apply request")?;
    Ok(request
        .updates
        .into_iter()
        .map(|update| SuspectTermReviewUpdate {
            id: update.id,
            suggested: update.suggested,
            dismiss: update.dismiss,
        })
        .collect::<Vec<_>>())
}

fn history_json(path: PathBuf) -> Result<String> {
    let records = history::load_recent(&path, 200)?;
    let items = records
        .iter()
        .rev()
        .map(|record| {
            json!({
                "timestamp_ms": record.timestamp_ms,
                "mode": record.mode,
                "profile_id": record.profile_id,
                "target_process": record.target_process,
                "text": record.preview_text(),
                "raw_text": record.raw_text,
                "rewrite_text": record.rewrite_text,
                "total_elapsed_ms": record.total_elapsed_ms,
                "error": record.error
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"ok": true, "items": items}).to_string())
}

fn debug_settings_json(state: &ConsoleState) -> Result<String> {
    let config = AppConfig::load(&state.app_config_path)?;
    let endpoint = state.debug_endpoint_url.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        return Ok(r#"{"ok":false,"error":"missing endpoint"}"#.to_string());
    }
    let request = state.http.get(format!("{endpoint}/v1/settings/asr"));
    let response = with_bearer_auth(request, &state.debug_api_key)
        .send()
        .context("request ASR settings")?
        .error_for_status()
        .context("ASR settings status")?
        .text()
        .context("read ASR settings")?;
    Ok(json!({
        "ok": true,
        "settings": serde_json::from_str::<serde_json::Value>(&response).unwrap_or_else(|_| json!({"raw": response})),
        "output": {
            "clipboard_policy": config.output.clipboard_policy.as_str(),
            "clipboard_retry_count": config.output.clipboard_retry_count,
            "clipboard_retry_backoff_ms": config.output.clipboard_retry_backoff_ms,
            "paste_preflight_recheck": config.output.paste_preflight_recheck,
            "replacement_preflight_recheck": config.output.replacement_preflight_recheck,
            "prefer_direct_paste": config.output.prefer_direct_paste,
            "paste_stabilize_ms": config.output.paste_stabilize_ms,
        }
    }).to_string())
}

fn with_bearer_auth(request: RequestBuilder, api_key: &Option<String>) -> RequestBuilder {
    if let Some(api_key) = api_key {
        request.bearer_auth(api_key)
    } else {
        request
    }
}

fn read_api_key(primary_env: &str) -> Option<String> {
    for name in [
        primary_env,
        "AINPUT_API_KEY",
        "AINPUT_CLIPROXYAPI_KEY",
        "AINPUT_CLIPROXYAPI_8317_KEY",
    ] {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
        #[cfg(windows)]
        if let Some(value) = read_windows_user_env_var(name) {
            return Some(value);
        }
    }
    None
}

#[cfg(windows)]
fn read_windows_user_env_var(name: &str) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};

    if name.trim().is_empty() {
        return None;
    }
    let subkey = HSTRING::from("Environment");
    let value_name = HSTRING::from(name);
    let mut bytes = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..len]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn prompt_latest_json(path: PathBuf) -> Result<String> {
    let source = history::load_recent(&path, 20)?
        .into_iter()
        .rev()
        .find_map(|record| {
            let text = record.preview_text().trim().to_string();
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_default();
    Ok(json!({
        "ok": true,
        "source": source,
        "prompt": default_rewrite_prompt()
    })
    .to_string())
}

#[derive(Debug, Deserialize)]
struct PromptTestRequest {
    source: String,
    prompt: String,
}

fn prompt_test_json(state: &ConsoleState, body: &str) -> Result<String> {
    let request: PromptTestRequest =
        serde_json::from_str(body).context("parse prompt test request")?;
    let Some(rewriter) = &state.prompt_rewriter else {
        return Ok(r#"{"ok":false,"error":"Prompt Studio rewriter is not available"}"#.to_string());
    };
    let trace = rewriter.rewrite_with_prompt_trace(&request.source, &request.prompt);
    Ok(json!({
        "ok": trace.output.is_some(),
        "output": trace.output.unwrap_or_default(),
        "elapsed_ms": trace.elapsed_ms,
        "attempts": trace.attempts.iter().map(|attempt| json!({
            "model": attempt.model,
            "elapsed_ms": attempt.elapsed_ms,
            "ok": attempt.ok,
            "changed": attempt.changed,
            "error": attempt.error
        })).collect::<Vec<_>>()
    })
    .to_string())
}

fn rewrite_settings_json(state: &ConsoleState) -> Result<String> {
    let config = AppConfig::load(&state.app_config_path)?;
    Ok(json!({
        "ok": true,
        "restart_required": true,
        "config_path": state.app_config_path.display().to_string(),
        "rewrite": {
            "model": state.rewrite_model,
            "dynamic_budget_enabled": config.rewrite.dynamic_budget_enabled,
            "compact_prompt_enabled": config.rewrite.compact_prompt_enabled,
            "streaming_prewrite_enabled": config.rewrite.streaming_prewrite_enabled,
            "prewrite_min_chars": config.rewrite.prewrite_min_chars,
            "prewrite_stable_ms": config.rewrite.prewrite_stable_ms,
            "prewrite_debounce_ms": config.rewrite.prewrite_debounce_ms,
            "prewrite_max_inflight": config.rewrite.prewrite_max_inflight,
        },
        "output": {
            "clipboard_policy": config.output.clipboard_policy.as_str(),
            "clipboard_retry_count": config.output.clipboard_retry_count,
            "clipboard_retry_backoff_ms": config.output.clipboard_retry_backoff_ms,
            "paste_preflight_recheck": config.output.paste_preflight_recheck,
            "replacement_preflight_recheck": config.output.replacement_preflight_recheck,
        }
    })
    .to_string())
}

#[derive(Debug, Deserialize)]
struct RewriteSettingsRequest {
    compact_prompt_enabled: bool,
    streaming_prewrite_enabled: bool,
}

fn update_rewrite_settings_json(state: &ConsoleState, body: &str) -> Result<String> {
    let request: RewriteSettingsRequest =
        serde_json::from_str(body).context("parse rewrite settings request")?;
    let mut raw = std::fs::read_to_string(&state.app_config_path)
        .with_context(|| format!("read app config {}", state.app_config_path.display()))?;
    raw = set_rewrite_bool_key(
        &raw,
        "compact_prompt_enabled",
        request.compact_prompt_enabled,
    )?;
    raw = set_rewrite_bool_key(
        &raw,
        "streaming_prewrite_enabled",
        request.streaming_prewrite_enabled,
    )?;
    std::fs::write(&state.app_config_path, raw)
        .with_context(|| format!("write app config {}", state.app_config_path.display()))?;
    rewrite_settings_json(state)
}

fn set_rewrite_bool_key(raw: &str, key: &str, value: bool) -> Result<String> {
    let mut output = Vec::new();
    let mut in_rewrite = false;
    let mut inserted = false;
    let existing = rewrite_section_has_key(raw, key);
    let value_line = format!("{key} = {value}");
    let insert_after = match key {
        "compact_prompt_enabled" => "dynamic_budget_enabled",
        "streaming_prewrite_enabled" => "compact_prompt_enabled",
        _ => "enabled",
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_rewrite && !inserted {
                output.push(value_line.clone());
                inserted = true;
            }
            in_rewrite = trimmed == "[rewrite]";
        }
        if in_rewrite && trimmed.starts_with(&format!("{key} ")) {
            if !inserted {
                output.push(value_line.clone());
                inserted = true;
            }
            continue;
        }
        output.push(line.to_string());
        if !existing && in_rewrite && !inserted && trimmed.starts_with(&format!("{insert_after} "))
        {
            output.push(value_line.clone());
            inserted = true;
        }
    }
    if in_rewrite && !inserted {
        output.push(value_line);
        inserted = true;
    }
    if !inserted {
        anyhow::bail!("[rewrite] section not found in app config");
    }
    let mut text = output.join("\n");
    text.push('\n');
    Ok(text)
}

fn rewrite_section_has_key(raw: &str, key: &str) -> bool {
    let mut in_rewrite = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_rewrite = trimmed == "[rewrite]";
            continue;
        }
        if in_rewrite && trimmed.starts_with(&format!("{key} ")) {
            return true;
        }
    }
    false
}

#[derive(Debug, Deserialize)]
struct PreviewTextRequest {
    text: Option<String>,
}

fn default_hud_preview_text() -> String {
    "HUD 预览：这是一段用于测试最大宽度、字体大小、颜色、阴影和彩虹效果的长文本。".to_string()
}

fn load_hud_preview_text(path: &std::path::Path) -> String {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return default_hud_preview_text();
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| {
            value
                .get("text")
                .and_then(|text| text.as_str())
                .map(str::to_string)
        })
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(default_hud_preview_text)
}

fn save_hud_preview_text(path: &std::path::Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create HUD preview dir {}", parent.display()))?;
    }
    std::fs::write(path, json!({"text": text}).to_string())
        .with_context(|| format!("write HUD preview text {}", path.display()))
}

fn parse_preview_text(body: &str) -> String {
    serde_json::from_str::<PreviewTextRequest>(body)
        .ok()
        .and_then(|request| request.text)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(default_hud_preview_text)
}

fn corrections_json(path: &std::path::Path) -> Result<String> {
    let store = personal_corrections::load_store(path)?;
    Ok(json!({
        "ok": true,
        "rules": store.rules.iter().enumerate().map(|(index, rule)| json!({
            "index": index,
            "wrong": rule.wrong,
            "correct": rule.correct,
            "enabled": rule.enabled,
            "source": rule.source,
            "updated_ms": rule.updated_ms
        })).collect::<Vec<_>>(),
        "protected": store.protected_replacements.iter().enumerate().map(|(index, rule)| json!({
            "index": index,
            "raw": rule.raw,
            "forbidden": rule.forbidden,
            "enabled": rule.enabled,
            "source": rule.source,
            "updated_ms": rule.updated_ms
        })).collect::<Vec<_>>()
    })
    .to_string())
}

#[derive(Debug, Deserialize)]
struct AddCorrectionRequest {
    wrong: String,
    correct: String,
}

fn add_correction_json(path: &std::path::Path, body: &str) -> Result<String> {
    let request: AddCorrectionRequest =
        serde_json::from_str(body).context("parse add correction request")?;
    personal_corrections::append_or_update_rule(
        path,
        &request.wrong,
        &request.correct,
        "web_manual",
    )?;
    Ok(r#"{"ok":true}"#.to_string())
}

#[derive(Debug, Deserialize)]
struct UpdateCorrectionRequest {
    index: usize,
    enabled: Option<bool>,
    delete: Option<bool>,
}

fn update_correction_json(path: &std::path::Path, body: &str) -> Result<String> {
    let request: UpdateCorrectionRequest =
        serde_json::from_str(body).context("parse update correction request")?;
    let ok = if request.delete.unwrap_or(false) {
        personal_corrections::delete_rule(path, request.index)?
    } else if let Some(enabled) = request.enabled {
        personal_corrections::set_rule_enabled(path, request.index, enabled)?
    } else {
        false
    };
    Ok(json!({"ok": ok}).to_string())
}

#[derive(Debug, Deserialize)]
struct AddProtectedRequest {
    raw: String,
    forbidden: String,
}

fn add_protected_json(path: &std::path::Path, body: &str) -> Result<String> {
    let request: AddProtectedRequest =
        serde_json::from_str(body).context("parse add protected request")?;
    personal_corrections::append_or_update_protected_replacement(
        path,
        &request.raw,
        &request.forbidden,
        "web_manual",
    )?;
    Ok(r#"{"ok":true}"#.to_string())
}

#[derive(Debug, Deserialize)]
struct UpdateProtectedRequest {
    index: usize,
    enabled: Option<bool>,
    delete: Option<bool>,
}

fn update_protected_json(path: &std::path::Path, body: &str) -> Result<String> {
    let request: UpdateProtectedRequest =
        serde_json::from_str(body).context("parse update protected request")?;
    let ok = if request.delete.unwrap_or(false) {
        personal_corrections::delete_protected(path, request.index)?
    } else if let Some(enabled) = request.enabled {
        personal_corrections::set_protected_enabled(path, request.index, enabled)?
    } else {
        false
    };
    Ok(json!({"ok": ok}).to_string())
}

fn open_logs_json(path: PathBuf) -> Result<String> {
    let Some(parent) = path.parent() else {
        return Ok(r#"{"ok":false,"error":"missing parent"}"#.to_string());
    };
    Command::new("explorer.exe")
        .arg(parent)
        .spawn()
        .context("open logs dir")?;
    Ok(r#"{"ok":true}"#.to_string())
}

fn open_url(url: &str) -> Result<()> {
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .with_context(|| format!("open url {url}"))?;
    Ok(())
}

const HOME_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ainput2 控制台</title><style>body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;background:#f6f7f8;color:#111}.bar{padding:14px 18px;border-bottom:1px solid #d7d7d7;background:#fff}main{max-width:980px;margin:0 auto;padding:18px}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px}.card{display:block;padding:16px;border:1px solid #d7d7d7;border-radius:6px;background:#fff;color:#111;text-decoration:none}.card b{display:block;margin-bottom:6px;font-size:17px}.card span{color:#666}</style></head><body><div class="bar"><b>ainput2 控制台</b></div><main><div class="grid"><a class="card" href="/hud"><b>HUD</b><span>任务栏位置、尺寸、字体，实时改真实 HUD</span></a><a class="card" href="/settings"><b>改写设置</b><span>短 prompt、流式预改写和当前模型</span></a><a class="card" href="/suspect"><b>疑似错词</b><span>处理候选和查看归档</span></a><a class="card" href="/history"><b>历史 / 对比</b><span>查看最近语音输出</span></a><a class="card" href="/debug"><b>调试</b><span>查看 ASR 设置和打开日志</span></a><a class="card" href="/prompt"><b>Prompt Studio</b><span>加载最近文本并测试改写 prompt</span></a><a class="card" href="/corrections"><b>个人纠错</b><span>管理错词规则和禁止替换</span></a></div></main></body></html>"#;

const SETTINGS_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ainput2 改写设置</title><style>body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;background:#f6f7f8;color:#111}.bar{display:flex;gap:8px;align-items:center;padding:10px 12px;border-bottom:1px solid #d7d7d7;background:#fff}a,button{height:32px;padding:0 13px;border:1px solid #b8b8b8;border-radius:4px;background:#fff;color:#111;text-decoration:none;display:inline-flex;align-items:center;font:inherit}.primary{background:#0f6cbd;border-color:#0f6cbd;color:#fff}main{max-width:900px;margin:0 auto;padding:14px}.panel{background:#fff;border:1px solid #d7d7d7;border-radius:6px;padding:14px}.row{display:grid;grid-template-columns:220px 1fr;gap:10px;align-items:start;padding:12px 0;border-bottom:1px solid #eee}.row:last-child{border-bottom:0}.muted{color:#666}.status{margin-left:auto;color:#666}.switch{display:flex;gap:8px;align-items:center}.switch input{width:18px;height:18px}.mono{font-family:Consolas,monospace;background:#f3f4f5;padding:2px 5px;border-radius:4px}</style></head><body><div class="bar"><a href="/">控制台</a><button id="save" class="primary">保存</button><button id="reload">刷新</button><span id="status" class="status">加载中</span></div><main><div class="panel"><div class="row"><b>当前改写模型</b><div><span id="model" class="mono"></span><div class="muted">模型来自用户 API 配置；默认空。请在 API 设置中填写 OpenAI 兼容端点与模型。</div></div></div><div class="row"><b>动态输出预算</b><div><span id="dynamic_budget"></span><div class="muted">固定开启：短句减少输出预算，长句保留容量。</div></div></div><div class="row"><b>短 prompt</b><label class="switch"><input id="compact" type="checkbox">启用 compact prompt<div class="muted">减少 prompt 长度，保存后重启生效。</div></label></div><div class="row"><b>流式说话中预改写</b><label class="switch"><input id="prewrite" type="checkbox">启用 streaming prewrite<div class="muted">说话中对稳定 partial 先发起一次 speculative 改写；结果不安全或文本变化会自动丢弃。保存后重启生效。</div></label></div><div class="row"><b>预改写参数</b><div class="muted" id="prewrite_params"></div></div><div class="row"><b>配置文件</b><div><span id="config_path" class="mono"></span></div></div></div></main><script>const statusEl=document.querySelector('#status');async function load(){statusEl.textContent='加载中';const d=await fetch('/api/settings/rewrite').then(r=>r.json());if(!d.ok)throw new Error(d.error||'load failed');const r=d.rewrite;document.querySelector('#model').textContent=r.model||'';document.querySelector('#dynamic_budget').textContent=r.dynamic_budget_enabled?'开启':'关闭';document.querySelector('#compact').checked=!!r.compact_prompt_enabled;document.querySelector('#prewrite').checked=!!r.streaming_prewrite_enabled;document.querySelector('#prewrite_params').textContent=`min_chars=${r.prewrite_min_chars}, stable_ms=${r.prewrite_stable_ms}, debounce_ms=${r.prewrite_debounce_ms}, max_inflight=${r.prewrite_max_inflight}`;document.querySelector('#config_path').textContent=d.config_path||'';statusEl.textContent=d.restart_required?'已加载，改动需重启生效':'已加载'}async function save(){statusEl.textContent='保存中';const body={compact_prompt_enabled:document.querySelector('#compact').checked,streaming_prewrite_enabled:document.querySelector('#prewrite').checked};const d=await fetch('/api/settings/rewrite',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json());if(!d.ok)throw new Error(d.error||'save failed');statusEl.textContent='已保存，重启 ainput2 后生效';await load()}document.querySelector('#save').onclick=()=>save().catch(e=>statusEl.textContent='保存失败：'+e.message);document.querySelector('#reload').onclick=()=>load().catch(e=>statusEl.textContent='加载失败：'+e.message);load().catch(e=>statusEl.textContent='加载失败：'+e.message);</script></body></html>"#;

const SUSPECT_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ainput2 疑似错词</title><style>:root{--line:#d7d7d7;--bg:#f6f7f8;--card:#fff;--text:#111;--muted:#666;--blue:#0f6cbd;--red:#b42318}*{box-sizing:border-box}body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;color:var(--text);background:var(--bg)}.bar{position:sticky;top:0;z-index:5;display:flex;align-items:center;gap:8px;padding:10px 12px;border-bottom:1px solid var(--line);background:#fff}a,button{height:32px;padding:0 13px;border:1px solid #b8b8b8;border-radius:4px;background:#fff;color:#111;font:inherit;cursor:pointer;text-decoration:none;display:inline-flex;align-items:center}.primary{border-color:var(--blue);background:var(--blue);color:#fff}.danger{color:var(--red)}button:disabled{opacity:.55;cursor:default}.status{margin-left:auto;color:var(--muted);white-space:nowrap}main{padding:12px;max-width:1180px;margin:0 auto}.tabs{display:flex;gap:8px;margin-bottom:12px}.tab.active{border-color:var(--blue);color:var(--blue)}.empty{padding:36px;border:1px dashed var(--line);background:#fff;color:var(--muted);text-align:center}.row{display:grid;grid-template-columns:190px 1fr auto;gap:10px;align-items:start;padding:12px;margin-bottom:10px;border:1px solid var(--line);border-radius:6px;background:var(--card)}.badge{display:inline-block;min-width:72px;margin-bottom:8px;padding:2px 8px;border-radius:999px;background:#eef2f6;text-align:center}.badge.pending{background:#fff4ce}.badge.applied{background:#dff6dd}.badge.dismissed{background:#fde7e9}.wrong{font-size:16px;font-weight:600;overflow-wrap:anywhere}.edit label{display:block;margin-bottom:5px;color:var(--muted)}input.suggested{width:100%;height:34px;padding:5px 8px;border:1px solid #999;border-radius:4px;font:16px/1.3 "Microsoft YaHei UI",sans-serif}.changed{border-color:var(--blue)!important;box-shadow:0 0 0 2px rgba(15,108,189,.15)}.detail{margin-top:8px;color:#333;overflow-wrap:anywhere;white-space:pre-wrap}.actions{display:flex;gap:8px}.row.ignored{opacity:.56}.row.ignored input{text-decoration:line-through}@media(max-width:760px){.bar{flex-wrap:wrap}.status{width:100%;margin-left:0}.row{grid-template-columns:1fr}.actions{justify-content:flex-end}}</style></head><body><div class="bar"><a href="/">控制台</a><button id="refresh">刷新</button><button id="analyze">立即分析</button><button id="apply" class="primary">一键应用</button><button id="openLogs">打开日志</button><span id="status" class="status">加载中</span></div><main><div class="tabs"><button class="tab active" data-status="pending">候选</button><button class="tab" data-status="applied">已应用 / 已学习</button><button class="tab" data-status="dismissed">已忽略</button></div><div id="list"></div></main><script>const list=document.querySelector('#list'),statusEl=document.querySelector('#status');let state={items:[],status:'pending',counts:{}};function esc(s){return String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}function detail(i){const p=[];if(i.reason)p.push('原因：'+i.reason);if(i.examples&&i.examples.length)p.push('例子：'+i.examples[0]);return p.join('\n')}function emptyText(){if(state.status==='pending'&&(state.counts.learned||0)>0)return`暂无新候选；已有 ${state.counts.learned} 条已学习纠错在“已应用 / 已学习”页。`;return'暂无项目'}function render(){if(!state.items.length){list.innerHTML=`<div class="empty">${esc(emptyText())}</div>`}else{list.innerHTML=state.items.map(i=>`<section class="row" data-id="${esc(i.id)}" data-status="${esc(i.status)}" data-original="${esc(i.suggested)}"><div><span class="badge ${esc(i.status)}">${esc(i.status)} ${((i.confidence||0)*100).toFixed(0)}%</span><div class="wrong">${esc(i.wrong)}</div></div><div class="edit"><label>${i.status==='pending'?'建议':'归档内容'}</label><input class="suggested" value="${esc(i.suggested)}" ${i.status==='dismissed'||String(i.id).startsWith('learned-correction-')?'readonly':''}/><div class="detail">${esc(detail(i))}</div></div><div class="actions">${i.status==='pending'?'<button class="restore">还原</button><button class="ignore danger">忽略</button>':'<button class="restore">重新编辑</button>'}</div></section>`).join('')}bindRows();updateSummary()}function bindRows(){document.querySelectorAll('.row').forEach(r=>{const input=r.querySelector('.suggested');input.addEventListener('input',()=>{input.classList.toggle('changed',input.value.trim()!==r.dataset.original);updateSummary()});r.querySelector('.restore').addEventListener('click',()=>{r.classList.remove('ignored');input.readOnly=false;input.value=r.dataset.original;input.classList.remove('changed');updateSummary()});const ignore=r.querySelector('.ignore');if(ignore)ignore.addEventListener('click',()=>{r.classList.toggle('ignored');updateSummary()})})}function collectUpdates(){return[...document.querySelectorAll('.row')].flatMap(r=>{if(String(r.dataset.id).startsWith('learned-correction-'))return[];const input=r.querySelector('.suggested'),suggested=input.value.trim(),ignored=r.classList.contains('ignored'),changed=suggested!==r.dataset.original;if(ignored||r.dataset.status==='pending'||changed)return[{id:r.dataset.id,suggested,dismiss:ignored}];return[]})}function updateSummary(){const rows=[...document.querySelectorAll('.row')],pending=rows.filter(r=>r.dataset.status==='pending'&&!r.classList.contains('ignored')).length,changed=rows.filter(r=>!r.classList.contains('ignored')&&r.querySelector('.suggested').value.trim()!==r.dataset.original).length,ignored=rows.filter(r=>r.classList.contains('ignored')).length;statusEl.textContent=`${state.status} | 本页 ${rows.length} | 待处理 ${pending} | 已修改 ${changed} | 准备忽略 ${ignored} | 已学习 ${state.counts.learned||0}`}async function refresh(){statusEl.textContent='刷新中';const res=await fetch('/api/suspect/book?status='+state.status),data=await res.json();state.items=data.items||[];state.counts=data.counts||{};render()}async function applyAll(){const updates=collectUpdates();if(!updates.length){statusEl.textContent='没有需要应用的改动';return}document.querySelector('#apply').disabled=true;statusEl.textContent='应用中';try{const res=await fetch('/api/suspect/apply',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({updates})}),data=await res.json();if(!data.ok)throw new Error(data.error||'apply failed');statusEl.textContent=`已应用 ${data.applied} 条，忽略 ${data.dismissed} 条，剩余待处理 ${data.remaining_pending} 条`;await refresh()}catch(e){statusEl.textContent='应用失败：'+e.message}finally{document.querySelector('#apply').disabled=false}}document.querySelectorAll('.tab').forEach(b=>b.onclick=()=>{document.querySelectorAll('.tab').forEach(x=>x.classList.remove('active'));b.classList.add('active');state.status=b.dataset.status;refresh()});document.querySelector('#refresh').onclick=refresh;document.querySelector('#apply').onclick=applyAll;document.querySelector('#analyze').onclick=async()=>{await fetch('/api/suspect/analyze-now',{method:'POST'});statusEl.textContent='已提交后台增量分析'};document.querySelector('#openLogs').onclick=async()=>fetch('/api/logs/open',{method:'POST'});refresh();</script></body></html>"#;

const HUD_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>ainput2 HUD</title>
<style>
body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;background:#f6f7f8;color:#111}
.bar{display:flex;gap:8px;align-items:center;padding:10px 12px;border-bottom:1px solid #d7d7d7;background:#fff}
a,button{height:32px;padding:0 13px;border:1px solid #b8b8b8;border-radius:4px;background:#fff;color:#111;text-decoration:none;display:inline-flex;align-items:center;font:inherit}
.primary{background:#0f6cbd;border-color:#0f6cbd;color:#fff}
main{max-width:980px;margin:0 auto;padding:14px}
.panel{background:#fff;border:1px solid #d7d7d7;border-radius:6px;padding:14px}
.grid{display:grid;grid-template-columns:180px 1fr 90px;gap:10px;align-items:center}
.grid label{color:#555}
input,select{height:32px;border:1px solid #999;border-radius:4px;padding:4px 8px;font:inherit}
input[type=range]{padding:0}
.status{margin-left:auto;color:#666}
.hint{color:#666;margin-top:12px}
.font_picker{display:grid;gap:6px}
.font_list{max-height:230px;overflow:auto;border:1px solid #999;border-radius:4px;background:#fff}
.font_item{width:100%;height:32px;justify-content:flex-start;border:0;border-radius:0;border-bottom:1px solid #eee;background:#fff;text-align:left}
.font_item:last-child{border-bottom:0}
.font_item.active{background:#e8f2ff;color:#0f4d8f;font-weight:600}
.font_meta{color:#666;align-self:start;padding-top:7px}
</style>
</head>
<body>
<div class="bar"><a href="/">控制台</a><button id="preview">显示真实 HUD 预览</button><span id="status" class="status">加载中</span></div>
<main>
<div class="panel">
<div class="grid">
<label>位置</label><select id="anchor"><option value="taskbar_center">任务栏居中</option><option value="taskbar_left">任务栏左侧</option><option value="taskbar_right">任务栏右侧</option><option value="bottom_center">底部居中 fallback</option><option value="bottom_left">左下 fallback</option></select><span></span>
<label>扩散方式</label><select id="expand_origin"><option value="center">从中间扩散</option><option value="left">从左边扩散</option></select><span></span>
<label>最大宽度</label><input id="width_px" type="range" min="120" max="10000" step="10"><input id="width_num" type="number" min="120" max="10000" step="10">
<label>高度</label><input id="height_px" type="range" min="24" max="1000" step="1"><input id="height_num" type="number" min="24" max="1000" step="1">
<label>X 偏移</label><input id="offset_x_px" type="range" min="-10000" max="10000" step="1"><input id="offset_x_num" type="number" min="-10000" max="10000" step="1">
<label>Y 偏移</label><input id="offset_y_px" type="range" min="-10000" max="10000" step="1"><input id="offset_y_num" type="number" min="-10000" max="10000" step="1">
<label>自动匹配字体</label><select id="auto_font_fit"><option value="true">开启</option><option value="false">关闭</option></select><span></span>
<label>字体</label><div class="font_picker"><input id="font_family" autocomplete="off" placeholder="搜索或直接输入字体名"><div id="font_list" class="font_list"></div></div><span id="font_count" class="font_meta"></span>
<label>字体高度</label><input id="font_height_px" type="range" min="8" max="240" step="1"><input id="font_height_num" type="number" min="8" max="240" step="1">
<label>字重</label><input id="font_weight" type="range" min="100" max="900" step="100"><input id="font_weight_num" type="number" min="100" max="900" step="100">
<label>文字颜色</label><input id="text_color" type="color"><input id="text_color_text" type="text">
<label>文字透明度</label><input id="text_alpha_percent" type="range" min="1" max="100" step="1"><input id="text_alpha_num" type="number" min="1" max="100" step="1">
<label>背景颜色</label><input id="background_color" type="color"><input id="background_color_text" type="text">
<label>背景透明度</label><input id="background_alpha_percent" type="range" min="0" max="100" step="1"><input id="alpha_num" type="number" min="0" max="100" step="1">
<label>阴影</label><select id="shadow_enabled"><option value="false">关闭</option><option value="true">开启</option></select><span></span>
<label>阴影颜色</label><input id="shadow_color" type="color"><input id="shadow_color_text" type="text">
<label>阴影透明度</label><input id="shadow_alpha_percent" type="range" min="1" max="100" step="1"><input id="shadow_alpha_num" type="number" min="1" max="100" step="1">
<label>阴影 X</label><input id="shadow_offset_x_px" type="range" min="-32" max="32" step="1"><input id="shadow_x_num" type="number" min="-32" max="32" step="1">
<label>阴影 Y</label><input id="shadow_offset_y_px" type="range" min="-32" max="32" step="1"><input id="shadow_y_num" type="number" min="-32" max="32" step="1">
<label>文字效果</label><select id="text_effect"><option value="solid">普通</option><option value="rainbow">低饱和彩虹</option></select><span></span>
<label>彩虹饱和度</label><input id="rainbow_saturation_percent" type="range" min="0" max="100" step="1"><input id="rainbow_sat_num" type="number" min="0" max="100" step="1">
<label>彩虹亮度</label><input id="rainbow_lightness_percent" type="range" min="0" max="100" step="1"><input id="rainbow_light_num" type="number" min="0" max="100" step="1">
<label>彩虹步进</label><input id="rainbow_step_degree" type="range" min="1" max="180" step="1"><input id="rainbow_step_num" type="number" min="1" max="180" step="1">
<label>文字对齐</label><select id="text_align"><option value="center">居中</option><option value="left">左对齐</option></select><span></span>
</div>
<p class="hint">预览文字</p>
<textarea id="preview_text" style="width:100%;min-height:76px;border:1px solid #999;border-radius:4px;padding:8px;font:inherit"></textarea>
<p class="hint">这里不是假预览。打开本页会固定显示真实 HUD；任何改动都会保存并直接移动屏幕上的真实 HUD。</p>
</div>
</main>
<script>
const statusEl=document.querySelector('#status');
let cfg={};
let allFonts=[];
const pairs=[['width_px','width_num'],['height_px','height_num'],['offset_x_px','offset_x_num'],['offset_y_px','offset_y_num'],['font_height_px','font_height_num'],['font_weight','font_weight_num'],['text_alpha_percent','text_alpha_num'],['background_alpha_percent','alpha_num'],['shadow_alpha_percent','shadow_alpha_num'],['shadow_offset_x_px','shadow_x_num'],['shadow_offset_y_px','shadow_y_num'],['rainbow_saturation_percent','rainbow_sat_num'],['rainbow_lightness_percent','rainbow_light_num'],['rainbow_step_degree','rainbow_step_num']];
function esc(s){return String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}
function setVal(id,v){const e=document.querySelector('#'+id);if(e)e.value=v??''}
function getNum(id){return Number(document.querySelector('#'+id).value)}
function setColor(id,v){setVal(id,v||'#ffffff');setVal(id+'_text',v||'#ffffff')}
function fill(c){cfg=c;setVal('anchor',c.anchor);setVal('expand_origin',c.expand_origin);setVal('auto_font_fit',String(c.auto_font_fit));setVal('font_family',c.font_family);setVal('text_align',c.text_align);setColor('text_color',c.text_color);setColor('background_color',c.background_color);setColor('shadow_color',c.shadow_color);setVal('shadow_enabled',String(c.shadow_enabled));setVal('text_effect',c.text_effect);for(const[k,n]of pairs){setVal(k,c[k]);setVal(n,c[k])}renderFonts()}
function renderFonts(){const current=document.querySelector('#font_family').value.trim();const fonts=allFonts;document.querySelector('#font_count').textContent=`${fonts.length}/${allFonts.length}`;document.querySelector('#font_list').innerHTML=fonts.map(f=>`<button type="button" class="font_item ${f===current?'active':''}" data-font="${esc(f)}" style="font-family:'${String(f).replaceAll("'","\\'")}','Microsoft YaHei UI',sans-serif">${esc(f)}</button>`).join('')||'<div class="hint" style="padding:8px;margin:0">没有字体</div>';document.querySelectorAll('.font_item').forEach(btn=>btn.onclick=()=>{setVal('font_family',btn.dataset.font);renderFonts();schedule()})}
let timer=null;
async function save(){
const body={anchor:document.querySelector('#anchor').value,expand_origin:document.querySelector('#expand_origin').value,width_px:getNum('width_px'),height_px:getNum('height_px'),offset_x_px:getNum('offset_x_px'),offset_y_px:getNum('offset_y_px'),auto_font_fit:document.querySelector('#auto_font_fit').value==='true',font_family:document.querySelector('#font_family').value.trim(),font_height_px:getNum('font_height_px'),font_weight:getNum('font_weight'),text_color:document.querySelector('#text_color_text').value.trim(),text_alpha_percent:getNum('text_alpha_percent'),background_color:document.querySelector('#background_color_text').value.trim(),background_alpha_percent:getNum('background_alpha_percent'),shadow_enabled:document.querySelector('#shadow_enabled').value==='true',shadow_color:document.querySelector('#shadow_color_text').value.trim(),shadow_alpha_percent:getNum('shadow_alpha_percent'),shadow_offset_x_px:getNum('shadow_offset_x_px'),shadow_offset_y_px:getNum('shadow_offset_y_px'),text_effect:document.querySelector('#text_effect').value,rainbow_saturation_percent:getNum('rainbow_saturation_percent'),rainbow_lightness_percent:getNum('rainbow_lightness_percent'),rainbow_step_degree:getNum('rainbow_step_degree'),text_align:document.querySelector('#text_align').value};
statusEl.textContent='保存中';
const res=await fetch('/api/hud/config',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}),data=await res.json();
if(!data.ok)throw new Error(data.error||'save failed');
fill(data.config);
await showPreview();
statusEl.textContent='已保存并更新真实 HUD';
}
async function showPreview(){await fetch('/api/hud/preview',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({text:document.querySelector('#preview_text').value})})}
function schedule(){clearTimeout(timer);timer=setTimeout(()=>save().catch(e=>statusEl.textContent='保存失败：'+e.message),120)}
for(const[k,n]of pairs){document.addEventListener('input',e=>{if(e.target.id===k){setVal(n,e.target.value);schedule()}if(e.target.id===n){setVal(k,e.target.value);schedule()}})}
['text_color','background_color','shadow_color'].forEach(id=>document.addEventListener('input',e=>{if(e.target.id===id){setVal(id+'_text',e.target.value);schedule()}if(e.target.id===id+'_text'){setVal(id,e.target.value);schedule()}}));
['anchor','expand_origin','auto_font_fit','font_family','text_align','shadow_enabled','text_effect'].forEach(id=>document.querySelector('#'+id).onchange=schedule);
document.querySelector('#font_family').addEventListener('input',renderFonts);
document.querySelector('#font_family').addEventListener('keydown',e=>{if(e.key==='Enter')schedule()});
document.querySelector('#preview').onclick=async()=>{await showPreview();statusEl.textContent='已固定显示真实 HUD 预览'};
async function loadFonts(){const d=await fetch('/api/hud/fonts').then(r=>r.json());allFonts=d.fonts||[];renderFonts()}
Promise.all([fetch('/api/hud/config').then(r=>r.json()),fetch('/api/hud/preview-text').then(r=>r.json()),loadFonts()]).then(async ([d,p])=>{fill(d.config);document.querySelector('#preview_text').value=p.text||'';await showPreview();statusEl.textContent='已加载，真实 HUD 预览保持显示'});
</script>
</body>
</html>"#;

const CORRECTIONS_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ainput2 个人纠错</title><style>body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;background:#f6f7f8;color:#111}.bar{display:flex;gap:8px;align-items:center;padding:10px 12px;border-bottom:1px solid #d7d7d7;background:#fff}a,button{height:32px;padding:0 13px;border:1px solid #b8b8b8;border-radius:4px;background:#fff;color:#111;text-decoration:none;display:inline-flex;align-items:center;font:inherit}button.primary{background:#0f6cbd;border-color:#0f6cbd;color:#fff}main{max-width:1100px;margin:0 auto;padding:12px}.box,.row{background:#fff;border:1px solid #d7d7d7;border-radius:6px;padding:12px;margin-bottom:10px}.row{display:grid;grid-template-columns:1fr auto;gap:10px;align-items:center}.muted{color:#666}.forms{display:grid;grid-template-columns:1fr 1fr auto;gap:8px;margin-top:8px}input{height:32px;border:1px solid #999;border-radius:4px;padding:4px 8px;font:inherit}.actions{display:flex;gap:8px}</style></head><body><div class="bar"><a href="/">控制台</a><button id="refresh">刷新</button><span id="status"></span></div><main><section class="box"><b>普通纠错</b><div class="muted">识别后直接替换，比如“扣带 -> Codex”。不确定的规则可以先关闭。</div><div class="forms"><input id="wrong" placeholder="错词"><input id="correct" placeholder="正确写法"><button id="addRule" class="primary">添加</button></div></section><div id="rules"></div><section class="box"><b>禁止 AI 替换</b><div class="muted">用于阻止“搜索 -> 筛选”这种 AI 改写错误。原文里有左边词、改写里出现右边词时，会保留原文。</div><div class="forms"><input id="raw" placeholder="原文必须保留"><input id="forbidden" placeholder="禁止改成"><button id="addProtected" class="primary">添加</button></div></section><div id="protected"></div></main><script>const statusEl=document.querySelector('#status'),rulesEl=document.querySelector('#rules'),protectedEl=document.querySelector('#protected');function esc(s){return String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}async function refresh(){const d=await fetch('/api/corrections').then(r=>r.json());rulesEl.innerHTML=(d.rules||[]).map(r=>`<div class="row"><div><b>${esc(r.wrong)} -> ${esc(r.correct)}</b><div class="muted">${r.enabled?'启用':'关闭'} | ${esc(r.source)}</div></div><div class="actions"><button onclick="toggleRule(${r.index},${!r.enabled})">${r.enabled?'关闭':'启用'}</button><button onclick="deleteRule(${r.index})">删除</button></div></div>`).join('')||'<div class="box muted">暂无普通纠错规则</div>';protectedEl.innerHTML=(d.protected||[]).map(r=>`<div class="row"><div><b>${esc(r.raw)} 不改成 ${esc(r.forbidden)}</b><div class="muted">${r.enabled?'启用':'关闭'} | ${esc(r.source)}</div></div><div class="actions"><button onclick="toggleProtected(${r.index},${!r.enabled})">${r.enabled?'关闭':'启用'}</button><button onclick="deleteProtected(${r.index})">删除</button></div></div>`).join('')||'<div class="box muted">暂无禁止替换规则</div>';statusEl.textContent='已加载'}async function post(url,body){await fetch(url,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});await refresh()}async function toggleRule(index,enabled){await post('/api/corrections/update',{index,enabled})}async function deleteRule(index){await post('/api/corrections/update',{index,delete:true})}async function toggleProtected(index,enabled){await post('/api/protected/update',{index,enabled})}async function deleteProtected(index){await post('/api/protected/update',{index,delete:true})}document.querySelector('#refresh').onclick=refresh;document.querySelector('#addRule').onclick=()=>post('/api/corrections/add',{wrong:document.querySelector('#wrong').value,correct:document.querySelector('#correct').value});document.querySelector('#addProtected').onclick=()=>post('/api/protected/add',{raw:document.querySelector('#raw').value,forbidden:document.querySelector('#forbidden').value});refresh();</script></body></html>"#;

const HISTORY_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ainput2 历史</title><style>body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;background:#f6f7f8;color:#111}.bar{display:flex;gap:8px;align-items:center;padding:10px 12px;border-bottom:1px solid #d7d7d7;background:#fff}a,button{height:32px;padding:0 13px;border:1px solid #b8b8b8;border-radius:4px;background:#fff;color:#111;text-decoration:none;display:inline-flex;align-items:center;font:inherit}main{max-width:1100px;margin:0 auto;padding:12px}.item{padding:12px;margin-bottom:10px;background:#fff;border:1px solid #d7d7d7;border-radius:6px}.meta{color:#666;margin-bottom:6px}.text{white-space:pre-wrap;overflow-wrap:anywhere;font-size:15px}</style></head><body><div class="bar"><a href="/">控制台</a><button id="refresh">刷新</button><span id="status"></span></div><main id="list"></main><script>const list=document.querySelector('#list'),statusEl=document.querySelector('#status');function esc(s){return String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}async function refresh(){statusEl.textContent='加载中';const d=await fetch('/api/history').then(r=>r.json());list.innerHTML=(d.items||[]).map(i=>`<div class="item"><div class="meta">${esc(i.mode)} | ${esc(i.target_process)} | ${i.total_elapsed_ms}ms</div><div class="text">${esc(i.text)}</div></div>`).join('')||'<div class="item">暂无历史</div>';statusEl.textContent=`${(d.items||[]).length} 条`}document.querySelector('#refresh').onclick=refresh;refresh();</script></body></html>"#;

const DEBUG_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ainput2 调试</title><style>body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;background:#f6f7f8;color:#111}.bar{display:flex;gap:8px;align-items:center;padding:10px 12px;border-bottom:1px solid #d7d7d7;background:#fff}a,button{height:32px;padding:0 13px;border:1px solid #b8b8b8;border-radius:4px;background:#fff;color:#111;text-decoration:none;display:inline-flex;align-items:center;font:inherit}main{max-width:1100px;margin:0 auto;padding:12px}pre{padding:12px;background:#fff;border:1px solid #d7d7d7;border-radius:6px;white-space:pre-wrap;overflow-wrap:anywhere}</style></head><body><div class="bar"><a href="/">控制台</a><button id="refresh">刷新 ASR 设置</button><button id="logs">打开日志</button><span id="status"></span></div><main><pre id="out">加载中</pre></main><script>const out=document.querySelector('#out'),statusEl=document.querySelector('#status');async function refresh(){statusEl.textContent='加载中';try{const d=await fetch('/api/debug/settings').then(r=>r.json());out.textContent=JSON.stringify(d.settings||d,null,2);statusEl.textContent=d.ok?'已加载':'失败'}catch(e){out.textContent=e.message;statusEl.textContent='失败'}}document.querySelector('#refresh').onclick=refresh;document.querySelector('#logs').onclick=()=>fetch('/api/logs/open',{method:'POST'});refresh();</script></body></html>"#;

const PROMPT_HTML: &str = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ainput2 Prompt Studio</title><style>body{margin:0;font:14px/1.45 "Segoe UI","Microsoft YaHei UI",sans-serif;background:#f6f7f8;color:#111}.bar{display:flex;gap:8px;align-items:center;padding:10px 12px;border-bottom:1px solid #d7d7d7;background:#fff}a,button{height:32px;padding:0 13px;border:1px solid #b8b8b8;border-radius:4px;background:#fff;color:#111;text-decoration:none;display:inline-flex;align-items:center;font:inherit}.primary{background:#0f6cbd;border-color:#0f6cbd;color:#fff}main{max-width:1180px;margin:0 auto;padding:12px}.grid{display:grid;grid-template-columns:1fr 1fr;gap:12px}textarea{width:100%;min-height:260px;padding:10px;border:1px solid #999;border-radius:4px;font:14px/1.45 "Microsoft YaHei UI",sans-serif}.box{background:#fff;border:1px solid #d7d7d7;border-radius:6px;padding:12px}pre{white-space:pre-wrap;overflow-wrap:anywhere}.status{margin-left:auto;color:#666}@media(max-width:860px){.grid{grid-template-columns:1fr}}</style></head><body><div class="bar"><a href="/">控制台</a><button id="load">加载最近文本</button><button id="test" class="primary">测试改写</button><span id="status" class="status">加载中</span></div><main><div class="grid"><div class="box"><b>输入文本</b><textarea id="source"></textarea></div><div class="box"><b>系统 Prompt</b><textarea id="prompt"></textarea></div></div><div class="box" style="margin-top:12px"><b>结果</b><pre id="out"></pre></div></main><script>const statusEl=document.querySelector('#status'),source=document.querySelector('#source'),promptBox=document.querySelector('#prompt'),out=document.querySelector('#out');async function load(){const d=await fetch('/api/prompt/latest').then(r=>r.json());source.value=d.source||'';promptBox.value=d.prompt||'';statusEl.textContent='已加载'}async function test(){statusEl.textContent='请求中';out.textContent='';const d=await fetch('/api/prompt/test',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({source:source.value,prompt:promptBox.value})}).then(r=>r.json());out.textContent=d.ok?d.output:JSON.stringify(d,null,2);statusEl.textContent=d.ok?`完成 ${d.elapsed_ms}ms`:'失败'}document.querySelector('#load').onclick=load;document.querySelector('#test').onclick=test;load();</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::{
        HOME_HTML, HUD_HTML, SETTINGS_HTML, book_json, has_expect_continue, parse_apply_updates,
        set_rewrite_bool_key,
    };
    use crate::suspect_terms::{SuspectTermBook, SuspectTermItem, save_book};

    #[test]
    fn book_json_defaults_to_pending_candidates_only() {
        let path = std::env::temp_dir().join(format!(
            "ainput2-panel-book-{}-{}.json",
            std::process::id(),
            "pending-only"
        ));
        let corrections_path = std::env::temp_dir().join(format!(
            "ainput2-panel-corrections-{}-{}.json",
            std::process::id(),
            "pending-only"
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&corrections_path);
        save_book(
            &path,
            &SuspectTermBook {
                items: vec![
                    SuspectTermItem {
                        wrong: "必安".to_string(),
                        suggested: "币安".to_string(),
                        status: "pending".to_string(),
                        ..Default::default()
                    },
                    SuspectTermItem {
                        wrong: "收购".to_string(),
                        suggested: "收口".to_string(),
                        status: "applied".to_string(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        )
        .expect("save book");
        let payload = book_json(path.clone(), &corrections_path, "pending").expect("book json");
        let value: serde_json::Value = serde_json::from_str(&payload).expect("json");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&corrections_path);
        assert_eq!(value["counts"]["pending"], 1);
        assert_eq!(value["counts"]["applied"], 1);
        assert_eq!(value["items"].as_array().expect("items").len(), 1);
        assert_eq!(value["items"][0]["status"], "pending");
    }

    #[test]
    fn hud_page_exposes_font_controls_and_full_offset_ranges() {
        assert!(HUD_HTML.contains(r#"id="font_family""#));
        assert!(HUD_HTML.contains(r#"id="font_list""#));
        assert!(!HUD_HTML.contains("<datalist"));
        assert!(HUD_HTML.contains(r#"id="font_weight""#));
        assert!(HUD_HTML.contains(r#"id="preview_text""#));
        assert!(HUD_HTML.contains(r#"id="text_color""#));
        assert!(HUD_HTML.contains(r#"id="background_color""#));
        assert!(HUD_HTML.contains(r#"id="shadow_enabled""#));
        assert!(HUD_HTML.contains(r#"value="rainbow""#));
        assert!(HUD_HTML.contains(r#"id="offset_y_px" type="range" min="-10000" max="10000""#));
        assert!(
            HUD_HTML.contains(r#"font_family:document.querySelector('#font_family').value.trim()"#)
        );
        assert!(HUD_HTML.contains(r#"font_weight:getNum('font_weight')"#));
    }

    #[test]
    fn home_links_rewrite_settings_panel() {
        assert!(HOME_HTML.contains(r#"href="/settings""#));
        assert!(SETTINGS_HTML.contains(r#"id="compact""#));
        assert!(SETTINGS_HTML.contains(r#"id="prewrite""#));
        assert!(SETTINGS_HTML.contains("OpenAI") || SETTINGS_HTML.contains("模型"));
        assert!(SETTINGS_HTML.contains("/api/settings/rewrite"));
    }

    #[test]
    fn rewrite_settings_update_replaces_without_duplicate_keys() {
        let raw = "[mode]\ndefault = \"streaming_asr\"\n\n[rewrite]\nenabled = true\ndynamic_budget_enabled = true\ncompact_prompt_enabled = false\nstreaming_prewrite_enabled = false\nprewrite_min_chars = 8\n\n[output]\nprefer_direct_paste = true\n";
        let updated =
            set_rewrite_bool_key(raw, "compact_prompt_enabled", true).expect("compact update");
        let updated = set_rewrite_bool_key(&updated, "streaming_prewrite_enabled", true)
            .expect("prewrite update");
        assert_eq!(updated.matches("compact_prompt_enabled").count(), 1);
        assert_eq!(updated.matches("streaming_prewrite_enabled").count(), 1);
        assert!(updated.contains("compact_prompt_enabled = true"));
        assert!(updated.contains("streaming_prewrite_enabled = true"));
        toml::from_str::<crate::config::AppConfig>(&updated).expect("valid config after update");
    }

    #[test]
    fn parse_apply_updates_accepts_utf8_bom() {
        let updates = parse_apply_updates(
            "\u{feff}{\"updates\":[{\"id\":\"abc\",\"suggested\":\"币安\",\"dismiss\":false}]}",
        )
        .expect("parse apply updates");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].id, "abc");
        assert_eq!(updates[0].suggested, "币安");
        assert!(!updates[0].dismiss);
    }

    #[test]
    fn detects_expect_continue_header() {
        assert!(has_expect_continue(
            b"POST /api/suspect/apply HTTP/1.1\r\nHost: 127.0.0.1\r\nExpect: 100-continue\r\nContent-Length: 2"
        ));
        assert!(has_expect_continue(
            b"POST /api/suspect/apply HTTP/1.1\r\nexpect: something, 100-continue\r\nContent-Length: 2"
        ));
        assert!(!has_expect_continue(
            b"POST /api/suspect/apply HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2"
        ));
    }
}
