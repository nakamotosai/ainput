//! Loopback web UI to edit the AI rewrite system prompt.

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

use crate::rewrite_prompt::{
    PRESET_COMPACT, PRESET_CUSTOM, PRESET_LIGHT, PRESET_STANDARD, RewritePromptController,
};
use crate::web_ui::{
    escape_html, open_browser_hidden, read_http_request, request_method, request_path, write_response,
};

#[derive(Clone)]
pub struct RewritePromptPanelController {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    shutdown: Arc<AtomicBool>,
}

struct ServerState {
    prompt: RewritePromptController,
    #[allow(dead_code)]
    path: PathBuf,
}

impl RewritePromptPanelController {
    pub fn start(prompt: RewritePromptController, shutdown: Arc<AtomicBool>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind rewrite prompt web server")?;
        let addr = listener
            .local_addr()
            .context("rewrite prompt listener local_addr")?;
        let base_url = format!("http://{addr}");
        let state = Arc::new(ServerState {
            path: prompt.path().to_path_buf(),
            prompt,
        });
        let shutdown_server = Arc::clone(&shutdown);
        let state_server = Arc::clone(&state);

        thread::Builder::new()
            .name("ainput-prompt-web".into())
            .spawn(move || {
                if let Err(error) = run_server(listener, state_server, shutdown_server) {
                    warn!(error = %error, "rewrite prompt web server stopped with error");
                } else {
                    info!("rewrite prompt web server stopped");
                }
            })
            .context("spawn rewrite prompt web server")?;

        info!(%base_url, "rewrite prompt web UI ready (loopback)");
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
            Ok(()) => info!(%url, "opened rewrite prompt web UI"),
            Err(error) => warn!(error = %error, %url, "open rewrite prompt web UI failed"),
        }
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
                        warn!(error = %error, peer = %peer, "rewrite prompt web request failed");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                warn!(error = %error, "rewrite prompt accept failed");
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
            write_response(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                page_html(state).as_bytes(),
            )?;
        }
        ("GET", "/api/state") => {
            let json = state_json(state);
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
    preset: Option<u8>,
    custom_prompt: Option<String>,
}

fn state_json(state: &ServerState) -> String {
    let presets: Vec<serde_json::Value> = RewritePromptController::presets_for_ui()
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "label": p.label,
                "description": p.description,
            })
        })
        .collect();
    serde_json::json!({
        "preset": state.prompt.preset(),
        "preset_label": state.prompt.preset_label(),
        "custom_prompt": state.prompt.custom_prompt(),
        "active_prompt": state.prompt.active_prompt(),
        "presets": presets,
        "path": state.prompt.path().display().to_string(),
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
    if let Some(custom) = parsed.custom_prompt {
        if parsed.preset.unwrap_or(PRESET_CUSTOM) == PRESET_CUSTOM || !custom.trim().is_empty() {
            if !custom.trim().is_empty() {
                state.prompt.set_custom_prompt(&custom);
            }
        }
    }
    if let Some(preset) = parsed.preset {
        if preset != PRESET_CUSTOM {
            state.prompt.set_preset(preset);
        } else if state.prompt.custom_prompt().trim().is_empty() {
            return serde_json::json!({"ok": false, "error": "自定义提示词为空，请先填写内容"});
        } else {
            state.prompt.set_preset(PRESET_CUSTOM);
        }
    }
    serde_json::json!({
        "ok": true,
        "preset": state.prompt.preset(),
        "preset_label": state.prompt.preset_label(),
        "active_prompt": state.prompt.active_prompt(),
        "message": format!("已保存 · 当前：{}", state.prompt.preset_label()),
    })
}

fn page_html(state: &ServerState) -> String {
    let active = escape_html(&state.prompt.active_prompt());
    let custom = escape_html(&state.prompt.custom_prompt());
    let preset = state.prompt.preset();
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>ainput · 改写提示词</title>
<style>
:root {{ color-scheme: dark; --bg:#0f1419; --card:#1a222c; --fg:#e7ecf1; --muted:#8b9aab; --acc:#3d9cf0; --ok:#3dd68c; --bd:#2a3542; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:14px/1.5 "Segoe UI","Microsoft YaHei UI",sans-serif; background:var(--bg); color:var(--fg); }}
main {{ max-width:880px; margin:24px auto; padding:0 16px 48px; }}
h1 {{ font-size:20px; font-weight:650; margin:0 0 6px; }}
.sub {{ color:var(--muted); margin-bottom:18px; }}
.card {{ background:var(--card); border:1px solid var(--bd); border-radius:12px; padding:16px; margin-bottom:14px; }}
label {{ display:block; margin:0 0 6px; color:var(--muted); font-size:12px; }}
select, textarea, button {{ width:100%; font:inherit; color:var(--fg); background:#0c1117; border:1px solid var(--bd); border-radius:8px; padding:10px 12px; }}
textarea {{ min-height:220px; resize:vertical; line-height:1.55; }}
button {{ cursor:pointer; background:var(--acc); border:none; font-weight:600; margin-top:10px; }}
button.secondary {{ background:#243041; }}
.row {{ display:flex; gap:10px; }}
.row button {{ flex:1; }}
#status {{ margin-top:10px; color:var(--muted); min-height:1.4em; }}
#status.ok {{ color:var(--ok); }}
pre {{ white-space:pre-wrap; word-break:break-word; background:#0c1117; border-radius:8px; padding:12px; border:1px solid var(--bd); color:#c5d0db; max-height:240px; overflow:auto; }}
.hint {{ font-size:12px; color:var(--muted); margin-top:8px; }}
</style>
</head>
<body>
<main>
  <h1>AI 改写提示词</h1>
  <div class="sub">托盘右键也可直接切换预设。自定义内容保存在本机 state/config/rewrite-prompt.toml。</div>
  <div class="card">
    <label for="preset">预设</label>
    <select id="preset">
      <option value="{PRESET_STANDARD}" {s0}>标准（ASR 纠错润色）</option>
      <option value="{PRESET_COMPACT}" {s1}>精简</option>
      <option value="{PRESET_LIGHT}" {s2}>轻润色</option>
      <option value="{PRESET_CUSTOM}" {s3}>自定义</option>
    </select>
    <div class="hint">标准=默认纠错；精简=更短 prompt；轻润色=尽量少改；自定义=下方文本。</div>
  </div>
  <div class="card">
    <label for="custom">自定义提示词</label>
    <textarea id="custom" placeholder="在此编写 system prompt…">{custom}</textarea>
    <div class="row">
      <button id="save" type="button">保存</button>
      <button id="useCustom" class="secondary" type="button">保存并启用自定义</button>
    </div>
    <div id="status"></div>
  </div>
  <div class="card">
    <label>当前生效提示词预览</label>
    <pre id="preview">{active}</pre>
  </div>
</main>
<script>
const presetEl = document.getElementById('preset');
const customEl = document.getElementById('custom');
const statusEl = document.getElementById('status');
const previewEl = document.getElementById('preview');

async function save(forceCustom) {{
  statusEl.className = '';
  statusEl.textContent = '保存中…';
  const preset = forceCustom ? {PRESET_CUSTOM} : Number(presetEl.value);
  const body = {{
    preset,
    custom_prompt: customEl.value,
  }};
  try {{
    const resp = await fetch('/api/save', {{
      method: 'POST',
      headers: {{'Content-Type':'application/json'}},
      body: JSON.stringify(body),
    }});
    const data = await resp.json();
    if (!data.ok) throw new Error(data.error || '保存失败');
    statusEl.className = 'ok';
    statusEl.textContent = data.message || '已保存';
    if (data.active_prompt) previewEl.textContent = data.active_prompt;
    if (typeof data.preset === 'number') presetEl.value = String(data.preset);
  }} catch (e) {{
    statusEl.textContent = String(e.message || e);
  }}
}}

presetEl.addEventListener('change', () => save(false));
document.getElementById('save').addEventListener('click', () => save(false));
document.getElementById('useCustom').addEventListener('click', () => save(true));
</script>
</body>
</html>
"#,
        s0 = if preset == PRESET_STANDARD { "selected" } else { "" },
        s1 = if preset == PRESET_COMPACT { "selected" } else { "" },
        s2 = if preset == PRESET_LIGHT { "selected" } else { "" },
        s3 = if preset == PRESET_CUSTOM { "selected" } else { "" },
        PRESET_STANDARD = PRESET_STANDARD,
        PRESET_COMPACT = PRESET_COMPACT,
        PRESET_LIGHT = PRESET_LIGHT,
        PRESET_CUSTOM = PRESET_CUSTOM,
        custom = custom,
        active = active,
    )
}
