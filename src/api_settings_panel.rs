//! Loopback web UI for OpenAI-compatible rewrite credentials.
//! Tray → API / 改写设置… opens default browser to http://127.0.0.1:<port>/

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{info, warn};

use crate::ai_rewrite::SharedRewriter;
use crate::api_config::{self, ApiConnections, ApiConnectionsConfig};
use crate::rewrite_language::RewriteLanguageController;
use crate::web_ui::{
    escape_html, open_browser_hidden, read_http_request, request_method, request_path, write_response,
};

const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const DEFAULT_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone)]
pub struct ApiSettingsPanelController {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    shutdown: Arc<AtomicBool>,
}

struct ServerState {
    api_path: PathBuf,
    rewrite_language: RewriteLanguageController,
    rewriter: SharedRewriter,
}

impl ApiSettingsPanelController {
    pub fn start(
        api_path: PathBuf,
        rewrite_language: RewriteLanguageController,
        rewriter: SharedRewriter,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind API settings web server")?;
        let addr = listener
            .local_addr()
            .context("API settings listener local_addr")?;
        let base_url = format!("http://{addr}");
        let state = Arc::new(ServerState {
            api_path: api_path.clone(),
            rewrite_language,
            rewriter,
        });
        let shutdown_server = Arc::clone(&shutdown);
        let state_server = Arc::clone(&state);

        thread::Builder::new()
            .name("ainput-api-web".into())
            .spawn(move || {
                if let Err(error) = run_server(listener, state_server, shutdown_server) {
                    warn!(error = %error, "API settings web server stopped with error");
                } else {
                    info!("API settings web server stopped");
                }
            })
            .context("spawn API settings web server")?;

        info!(%base_url, path = %api_path.display(), "API settings web UI ready (loopback)");
        Ok(Self {
            inner: Arc::new(Inner {
                base_url,
                shutdown,
            }),
        })
    }

    pub fn open(&self) {
        if self.inner.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let url = self.inner.base_url.clone();
        match open_browser_hidden(&url) {
            Ok(()) => info!(%url, "opened API settings web UI in browser"),
            Err(error) => warn!(error = %error, %url, "open API settings web UI failed"),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }
}

fn run_server(
    listener: TcpListener,
    state: Arc<ServerState>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    listener
        .set_nonblocking(true)
        .context("set nonblocking")?;
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    if let Err(error) = handle_client(stream, &state) {
                        warn!(error = %error, peer = %peer, "API settings web request failed");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                warn!(error = %error, "API settings web accept failed");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Ok(())
}

fn handle_client(mut stream: TcpStream, state: &ServerState) -> Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let (head, body) = read_http_request(&mut stream)?;
    let first = head.lines().next().unwrap_or("");
    let method = request_method(first);
    let path = request_path(first);

    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => {
            let html = render_page(state);
            write_response(&mut stream, "200 OK", "text/html; charset=utf-8", html.as_bytes())?;
        }
        ("GET", "/api/config") => {
            let json = config_json(state);
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                json.as_bytes(),
            )?;
        }
        ("POST", "/api/save") => {
            let resp = save_from_body(state, &body);
            let status = if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                "200 OK"
            } else {
                "400 Bad Request"
            };
            let bytes = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
            write_response(
                &mut stream,
                status,
                "application/json; charset=utf-8",
                &bytes,
            )?;
        }
        ("POST", "/api/models") => {
            let resp = list_models_from_body(&body);
            let status = if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                "200 OK"
            } else {
                "400 Bad Request"
            };
            let bytes = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
            write_response(
                &mut stream,
                status,
                "application/json; charset=utf-8",
                &bytes,
            )?;
        }
        ("POST", "/api/probe") => {
            let resp = probe_from_body(&body);
            let bytes = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                &bytes,
            )?;
        }
        _ => {
            write_response(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"not found",
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct SaveBody {
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    timeout_ms: Option<u64>,
    rewrite_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ModelsBody {
    base_url: Option<String>,
    api_key: Option<String>,
    timeout_ms: Option<u64>,
}

fn parse_timeout_ms(value: u64) -> u64 {
    value.clamp(500, 120_000)
}

fn load_config(api_path: &std::path::Path) -> ApiConnectionsConfig {
    if api_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(api_path) {
            if let Ok(config) = serde_json::from_str::<ApiConnectionsConfig>(&raw) {
                return config;
            }
        }
    }
    ApiConnectionsConfig::default()
}

fn config_json(state: &ServerState) -> String {
    let config = load_config(&state.api_path);
    let base = if config.cliproxyapi.base_url.trim().is_empty() {
        NVIDIA_BASE_URL.to_string()
    } else {
        config.cliproxyapi.base_url.clone()
    };
    let timeout = if config.rewrite.timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        config.rewrite.timeout_ms
    };
    serde_json::json!({
        "base_url": base,
        "api_key": config.cliproxyapi.api_key,
        "model": config.rewrite.model,
        "timeout_ms": timeout,
        "rewrite_enabled": state.rewrite_language.rewrite_enabled(),
        "path": state.api_path.display().to_string(),
        "models_path": if config.cliproxyapi.models_path.trim().is_empty() {
            "/v1/models".to_string()
        } else {
            config.cliproxyapi.models_path.clone()
        },
    })
    .to_string()
}

fn save_from_body(state: &ServerState, body: &[u8]) -> serde_json::Value {
    let parsed: SaveBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(error) => {
            return serde_json::json!({"ok": false, "error": format!("JSON 无效: {error}")});
        }
    };
    let base = parsed
        .base_url
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string();
    let key = parsed.api_key.unwrap_or_default().trim().to_string();
    let model = parsed.model.unwrap_or_default().trim().to_string();
    let timeout_ms = parse_timeout_ms(parsed.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let enable_rewrite = parsed.rewrite_enabled.unwrap_or(false);

    if enable_rewrite && base.is_empty() {
        return serde_json::json!({"ok": false, "error": "启用改写前请先填写 Base URL"});
    }
    if enable_rewrite && model.is_empty() {
        return serde_json::json!({"ok": false, "error": "启用改写前请先填写或选择模型"});
    }
    if enable_rewrite && key.is_empty() {
        return serde_json::json!({"ok": false, "error": "启用改写前请先填写 API Key"});
    }

    let mut config = load_config(&state.api_path);
    config.cliproxyapi.base_url = if base.is_empty() {
        NVIDIA_BASE_URL.to_string()
    } else {
        base
    };
    config.cliproxyapi.api_key = key;
    if config.cliproxyapi.api_key_env.trim().is_empty() {
        config.cliproxyapi.api_key_env = "AINPUT_API_KEY".to_string();
    }
    if config.cliproxyapi.chat_completions_path.trim().is_empty() {
        config.cliproxyapi.chat_completions_path = "/v1/chat/completions".to_string();
    }
    if config.cliproxyapi.models_path.trim().is_empty() {
        config.cliproxyapi.models_path = "/v1/models".to_string();
    }
    config.rewrite.model = model;
    config.rewrite.timeout_ms = timeout_ms;

    let connections = ApiConnections {
        path: state.api_path.clone(),
        config,
    };
    if let Err(error) = connections.save() {
        return serde_json::json!({"ok": false, "error": format!("保存失败：{error}")});
    }

    state
        .rewrite_language
        .set_rewrite_enabled(enable_rewrite);

    let mut hot_reload_error: Option<String> = None;
    if let Err(error) = state.rewriter.apply_connection(
        &connections.config.cliproxyapi.base_url,
        &connections.config.cliproxyapi.api_key,
        &connections.config.rewrite.model,
        &connections.config.cliproxyapi.chat_completions_path,
        timeout_ms,
    ) {
        hot_reload_error = Some(format!("{error:#}"));
    }

    // Connectivity probe (sync on request worker thread — browser waits).
    let probe = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        api_config::probe_connectivity(
            &connections.config.cliproxyapi.base_url,
            &connections.config.cliproxyapi.api_key,
            &connections.config.cliproxyapi.models_path,
            timeout_ms,
        )
    }))
    .unwrap_or_else(|_| api_config::ConnectivityProbe {
        ok: false,
        status: 0,
        latency_ms: 0,
        url: String::new(),
        error: Some("连通探测 panic".to_string()),
    });

    let key_saved = !connections.config.cliproxyapi.api_key.is_empty();
    info!(
        path = %connections.path.display(),
        rewrite_enabled = enable_rewrite,
        timeout_ms,
        key_saved,
        probe_ok = probe.ok,
        probe_status = probe.status,
        probe_ms = probe.latency_ms,
        "API settings saved via web UI"
    );

    serde_json::json!({
        "ok": true,
        "key_saved": key_saved,
        "rewrite_enabled": enable_rewrite,
        "hot_reload_error": hot_reload_error,
        "probe": {
            "ok": probe.ok,
            "status": probe.status,
            "latency_ms": probe.latency_ms,
            "url": probe.url,
            "error": probe.error,
        },
        "message": format_save_message(key_saved, enable_rewrite, hot_reload_error.as_deref(), &probe),
    })
}

fn format_save_message(
    key_saved: bool,
    rewrite_enabled: bool,
    hot_reload_error: Option<&str>,
    probe: &api_config::ConnectivityProbe,
) -> String {
    let key_note = if key_saved { "Key 已落盘" } else { "Key 空" };
    let rewrite_note = if rewrite_enabled {
        "改写开"
    } else {
        "仅听写"
    };
    if let Some(err) = hot_reload_error {
        return format!("已保存（{key_note}）· 热加载失败：{err}");
    }
    if probe.ok {
        format!(
            "已保存 · {key_note} · {rewrite_note} · 连通 OK · HTTP {} · {} ms",
            probe.status, probe.latency_ms
        )
    } else if probe.status > 0 {
        format!(
            "已保存 · {key_note} · {rewrite_note} · 连通异常 · HTTP {} · {} ms",
            probe.status, probe.latency_ms
        )
    } else {
        let err = probe.error.as_deref().unwrap_or("网络错误");
        let short = if err.chars().count() > 100 {
            format!("{}…", err.chars().take(100).collect::<String>())
        } else {
            err.to_string()
        };
        format!(
            "已保存 · {key_note} · {rewrite_note} · 连通失败 · {} ms · {short}",
            probe.latency_ms
        )
    }
}

fn list_models_from_body(body: &[u8]) -> serde_json::Value {
    let parsed: ModelsBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(error) => {
            return serde_json::json!({"ok": false, "error": format!("JSON 无效: {error}")});
        }
    };
    let base = parsed.base_url.unwrap_or_default();
    let key = parsed.api_key.unwrap_or_default();
    let timeout_ms = parse_timeout_ms(parsed.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    if base.trim().is_empty() {
        return serde_json::json!({"ok": false, "error": "请先填写 Base URL"});
    }
    if key.trim().is_empty() {
        return serde_json::json!({"ok": false, "error": "请先填写 API Key 再拉取模型"});
    }
    let models_path = "/v1/models";
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        api_config::list_models(base.trim(), key.trim(), models_path, timeout_ms)
    })) {
        Ok(Ok(mut models)) => {
            let total = models.len();
            const MAX: usize = 800;
            if models.len() > MAX {
                models.truncate(MAX);
            }
            serde_json::json!({
                "ok": true,
                "total": total,
                "models": models,
                "message": if total > models.len() {
                    format!("已拉取 {total} 个，列表显示前 {} 个", models.len())
                } else {
                    format!("已拉取 {total} 个模型")
                }
            })
        }
        Ok(Err(error)) => serde_json::json!({"ok": false, "error": format!("{error:#}")}),
        Err(_) => serde_json::json!({"ok": false, "error": "拉取线程内部 panic"}),
    }
}

fn probe_from_body(body: &[u8]) -> serde_json::Value {
    let parsed: ModelsBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(error) => {
            return serde_json::json!({"ok": false, "error": format!("JSON 无效: {error}")});
        }
    };
    let base = parsed.base_url.unwrap_or_default();
    let key = parsed.api_key.unwrap_or_default();
    let timeout_ms = parse_timeout_ms(parsed.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let probe = api_config::probe_connectivity(base.trim(), key.trim(), "/v1/models", timeout_ms);
    serde_json::json!({
        "ok": probe.ok,
        "status": probe.status,
        "latency_ms": probe.latency_ms,
        "url": probe.url,
        "error": probe.error,
    })
}

fn render_page(state: &ServerState) -> String {
    let config = load_config(&state.api_path);
    let base = if config.cliproxyapi.base_url.trim().is_empty() {
        NVIDIA_BASE_URL
    } else {
        config.cliproxyapi.base_url.as_str()
    };
    let key = &config.cliproxyapi.api_key;
    let model = &config.rewrite.model;
    let timeout = if config.rewrite.timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        config.rewrite.timeout_ms
    };
    let rewrite_on = state.rewrite_language.rewrite_enabled();
    let path_esc = escape_html(&state.api_path.display().to_string());
    let base_esc = escape_html(base);
    let key_esc = escape_html(key);
    let model_esc = escape_html(model);
    let checked = if rewrite_on { "checked" } else { "" };
    let version = env!("CARGO_PKG_VERSION");

    format!(
        r##"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>ainput · API / 改写设置</title>
<style>
  :root {{
    color-scheme: dark;
    --bg: #141414;
    --card: #1e1a1a;
    --text: #f2f0f0;
    --muted: #a39a96;
    --line: #3a3230;
    --input: #2a2424;
    --btn: #322a2a;
    --btn-hover: #433838;
    --accent: #6aa84f;
    --fail: #e06c75;
    font-family: "Segoe UI", "Microsoft YaHei UI", "PingFang SC", sans-serif;
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0;
    background: var(--bg);
    color: var(--text);
    line-height: 1.5;
    font-size: 16px;
  }}
  .wrap {{
    max-width: 720px;
    margin: 0 auto;
    padding: 24px 18px 48px;
  }}
  h1 {{ margin: 0 0 6px; font-size: 24px; font-weight: 650; }}
  .sub {{ color: var(--muted); font-size: 13px; margin-bottom: 18px; word-break: break-all; }}
  label {{
    display: block;
    font-size: 13px;
    color: var(--muted);
    margin: 14px 0 6px;
  }}
  input[type=text], input[type=password], input[type=number], select {{
    width: 100%;
    background: var(--input);
    border: 1px solid var(--line);
    border-radius: 10px;
    color: var(--text);
    padding: 12px 14px;
    font-size: 15px;
  }}
  .row-inline {{
    display: flex;
    gap: 10px;
    align-items: center;
  }}
  .row-inline select {{ flex: 1; }}
  .check {{
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 18px 0 8px;
    font-size: 15px;
  }}
  .check input {{ width: 18px; height: 18px; }}
  .actions {{
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 20px;
  }}
  button {{
    appearance: none;
    border: 1px solid var(--line);
    background: var(--btn);
    color: var(--text);
    border-radius: 10px;
    padding: 12px 16px;
    font-size: 14px;
    cursor: pointer;
  }}
  button:hover {{ background: var(--btn-hover); }}
  button.primary {{
    background: #2f4a28;
    border-color: #3d5a2e;
  }}
  button:disabled {{ opacity: .55; cursor: wait; }}
  #status {{
    margin-top: 16px;
    padding: 12px 14px;
    border-radius: 10px;
    background: var(--card);
    border: 1px solid var(--line);
    min-height: 48px;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 14px;
  }}
  #status.ok {{ border-color: #3d5a2e; color: #c6efb0; }}
  #status.err {{ border-color: #6a3030; color: #ffb4b4; }}
  .hint {{ color: var(--muted); font-size: 12px; margin-top: 8px; }}
  datalist {{ display: none; }}
</style>
</head>
<body>
<div class="wrap">
  <h1>API / 改写设置</h1>
  <div class="sub">配置文件: {path_esc}<br/>默认 Base 可用 NVIDIA NIM；任意 OpenAI 兼容端点均可。</div>

  <label for="base_url">Base URL</label>
  <input id="base_url" type="text" value="{base_esc}" placeholder="https://integrate.api.nvidia.com/v1" autocomplete="off"/>

  <label for="api_key">API Key</label>
  <input id="api_key" type="password" value="{key_esc}" placeholder="sk-…" autocomplete="off"/>

  <label for="model">模型</label>
  <div class="row-inline">
    <input id="model" type="text" list="model_list" value="{model_esc}" placeholder="填写或拉取后选择" autocomplete="off"/>
  </div>
  <datalist id="model_list"></datalist>

  <label for="timeout_ms">超时 (ms)</label>
  <input id="timeout_ms" type="number" min="500" max="120000" step="100" value="{timeout}"/>

  <label class="check">
    <input id="rewrite_enabled" type="checkbox" {checked}/>
    <span>启用本地语音 AI 改写（非流式）</span>
  </label>

  <div class="actions">
    <button type="button" id="btn_fetch" onclick="fetchModels()">拉取模型</button>
    <button type="button" class="primary" id="btn_save" onclick="saveSettings()">保存并测连通</button>
  </div>
  <div id="status">就绪。保存会把 Key 写入本机 state，并探测 /v1/models。</div>
  <div class="hint">仅本机 loopback · Key 不上传 ainput · ainput {version}</div>
</div>
<script>
function statusEl() {{ return document.getElementById('status'); }}
function setStatus(text, kind) {{
  const el = statusEl();
  el.textContent = text;
  el.className = kind || '';
}}
function payload() {{
  return {{
    base_url: document.getElementById('base_url').value.trim(),
    api_key: document.getElementById('api_key').value,
    model: document.getElementById('model').value.trim(),
    timeout_ms: Number(document.getElementById('timeout_ms').value) || 5000,
    rewrite_enabled: document.getElementById('rewrite_enabled').checked,
  }};
}}
async function fetchModels() {{
  const btn = document.getElementById('btn_fetch');
  btn.disabled = true;
  setStatus('正在拉取模型列表…');
  try {{
    const p = payload();
    const res = await fetch('/api/models', {{
      method: 'POST',
      headers: {{ 'Content-Type': 'application/json' }},
      body: JSON.stringify({{ base_url: p.base_url, api_key: p.api_key, timeout_ms: p.timeout_ms }}),
    }});
    const data = await res.json();
    if (!data.ok) {{
      setStatus(data.error || '拉取失败', 'err');
      return;
    }}
    const list = document.getElementById('model_list');
    list.innerHTML = '';
    (data.models || []).forEach(m => {{
      const opt = document.createElement('option');
      opt.value = m;
      list.appendChild(opt);
    }});
    setStatus(data.message || ('已拉取 ' + (data.total || 0) + ' 个模型'), 'ok');
  }} catch (e) {{
    setStatus('拉取失败: ' + e, 'err');
  }} finally {{
    btn.disabled = false;
  }}
}}
async function saveSettings() {{
  const btn = document.getElementById('btn_save');
  btn.disabled = true;
  setStatus('保存中…');
  try {{
    const res = await fetch('/api/save', {{
      method: 'POST',
      headers: {{ 'Content-Type': 'application/json' }},
      body: JSON.stringify(payload()),
    }});
    const data = await res.json();
    if (!data.ok) {{
      setStatus(data.error || '保存失败', 'err');
      return;
    }}
    setStatus(data.message || '已保存', data.probe && data.probe.ok ? 'ok' : 'err');
  }} catch (e) {{
    setStatus('保存失败: ' + e, 'err');
  }} finally {{
    btn.disabled = false;
  }}
}}
</script>
</body>
</html>
"##,
        path_esc = path_esc,
        base_esc = base_esc,
        key_esc = key_esc,
        model_esc = model_esc,
        timeout = timeout,
        checked = checked,
        version = version,
    )
}
