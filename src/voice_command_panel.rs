//! Loopback web UI to edit the voice-command system prompt and toggle.

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

use crate::voice_command::{command_system_prompt, VoiceCommandController};
use crate::web_ui::{
    escape_html, open_browser_hidden, read_http_request, request_method, request_path, write_response,
};

#[derive(Clone)]
pub struct VoiceCommandPanelController {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    shutdown: Arc<AtomicBool>,
}

struct ServerState {
    voice: VoiceCommandController,
    #[allow(dead_code)]
    path: PathBuf,
}

impl VoiceCommandPanelController {
    pub fn start(voice: VoiceCommandController, shutdown: Arc<AtomicBool>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind voice command web server")?;
        let addr = listener
            .local_addr()
            .context("voice command listener local_addr")?;
        let base_url = format!("http://{addr}");
        let state = Arc::new(ServerState {
            path: voice.path().to_path_buf(),
            voice,
        });
        let shutdown_server = Arc::clone(&shutdown);
        let state_server = Arc::clone(&state);

        thread::Builder::new()
            .name("ainput-voice-cmd-web".into())
            .spawn(move || {
                if let Err(error) = run_server(listener, state_server, shutdown_server) {
                    warn!(error = %error, "voice command web server stopped with error");
                } else {
                    info!("voice command web server stopped");
                }
            })
            .context("spawn voice command web server")?;

        info!(%base_url, "voice command web UI ready (loopback)");
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
            Ok(()) => info!(%url, "opened voice command web UI"),
            Err(error) => warn!(error = %error, %url, "open voice command web UI failed"),
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
                        warn!(error = %error, peer = %peer, "voice command web request failed");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                warn!(error = %error, "voice command accept failed");
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
    enabled: Option<bool>,
    custom_prompt: Option<String>,
    use_default: Option<bool>,
}

fn state_json(state: &ServerState) -> String {
    serde_json::json!({
        "enabled": state.voice.enabled(),
        "custom_prompt": state.voice.custom_prompt(),
        "active_prompt": state.voice.active_prompt(),
        "default_prompt": command_system_prompt(),
        "path": state.voice.path().display().to_string(),
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
    if parsed.use_default == Some(true) {
        state.voice.set_custom_prompt("");
    } else if let Some(custom) = parsed.custom_prompt {
        state.voice.set_custom_prompt(&custom);
    }
    if let Some(enabled) = parsed.enabled {
        state.voice.set_enabled(enabled);
    }
    serde_json::json!({
        "ok": true,
        "enabled": state.voice.enabled(),
        "active_prompt": state.voice.active_prompt(),
        "custom_prompt": state.voice.custom_prompt(),
        "message": format!(
            "已保存 · 语音指令{}",
            if state.voice.enabled() { "已开启" } else { "已关闭" }
        ),
    })
}

fn page_html(state: &ServerState) -> String {
    let active = escape_html(&state.voice.active_prompt());
    let custom = escape_html(&state.voice.custom_prompt());
    let default_prompt = escape_html(command_system_prompt());
    let enabled = state.voice.enabled();
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>ainput · 语音指令</title>
<style>
:root {{ color-scheme: dark; --bg:#0f1419; --card:#1a222c; --fg:#e7ecf1; --muted:#8b9aab; --acc:#3d9cf0; --ok:#3dd68c; --bd:#2a3542; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:14px/1.5 "Segoe UI","Microsoft YaHei UI",sans-serif; background:var(--bg); color:var(--fg); }}
main {{ max-width:880px; margin:24px auto; padding:0 16px 48px; }}
h1 {{ font-size:20px; font-weight:650; margin:0 0 6px; }}
.sub {{ color:var(--muted); margin-bottom:18px; }}
.card {{ background:var(--card); border:1px solid var(--bd); border-radius:12px; padding:16px; margin-bottom:14px; }}
label {{ display:block; margin:0 0 6px; color:var(--muted); font-size:12px; }}
textarea, button {{ width:100%; font:inherit; color:var(--fg); background:#0c1117; border:1px solid var(--bd); border-radius:8px; padding:10px 12px; }}
textarea {{ min-height:220px; resize:vertical; line-height:1.55; }}
button {{ cursor:pointer; background:var(--acc); border:none; font-weight:600; margin-top:10px; }}
button.secondary {{ background:#243041; }}
.row {{ display:flex; gap:10px; }}
.row button {{ flex:1; }}
.toggle {{ display:flex; align-items:center; gap:10px; font-size:14px; color:var(--fg); }}
.toggle input {{ width:18px; height:18px; }}
#status {{ margin-top:10px; color:var(--muted); min-height:1.4em; }}
#status.ok {{ color:var(--ok); }}
pre {{ white-space:pre-wrap; word-break:break-word; background:#0c1117; border-radius:8px; padding:12px; border:1px solid var(--bd); color:#c5d0db; max-height:240px; overflow:auto; }}
.hint {{ font-size:12px; color:var(--muted); margin-top:8px; }}
</style>
</head>
<body>
<main>
  <h1>语音指令（老蔡老蔡）</h1>
  <div class="sub">说「老蔡老蔡 + 指令」会走生成，不走普通听写改写。配置保存在 state/config/voice-command.toml。</div>
  <div class="card">
    <label class="toggle">
      <input type="checkbox" id="enabled" {checked}/>
      <span>启用语音指令</span>
    </label>
    <div class="hint">关闭后，含「老蔡老蔡」的语音按普通听写处理。</div>
  </div>
  <div class="card">
    <label for="custom">指令 system prompt（留空=默认）</label>
    <textarea id="custom" placeholder="留空则使用内置默认提示词…">{custom}</textarea>
    <div class="row">
      <button id="save" type="button">保存</button>
      <button id="reset" class="secondary" type="button">恢复默认提示词</button>
    </div>
    <div id="status"></div>
  </div>
  <div class="card">
    <label>当前生效提示词</label>
    <pre id="preview">{active}</pre>
  </div>
  <div class="card">
    <label>内置默认（只读参考）</label>
    <pre>{default_prompt}</pre>
  </div>
</main>
<script>
const enabledEl = document.getElementById('enabled');
const customEl = document.getElementById('custom');
const statusEl = document.getElementById('status');
const previewEl = document.getElementById('preview');

async function save(useDefault) {{
  statusEl.className = '';
  statusEl.textContent = '保存中…';
  const body = {{
    enabled: enabledEl.checked,
    custom_prompt: customEl.value,
    use_default: !!useDefault,
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
    if (typeof data.custom_prompt === 'string') customEl.value = data.custom_prompt;
    if (typeof data.enabled === 'boolean') enabledEl.checked = data.enabled;
  }} catch (e) {{
    statusEl.textContent = String(e.message || e);
  }}
}}

enabledEl.addEventListener('change', () => save(false));
document.getElementById('save').addEventListener('click', () => save(false));
document.getElementById('reset').addEventListener('click', () => {{
  customEl.value = '';
  save(true);
}});
</script>
</body>
</html>
"#,
        checked = if enabled { "checked" } else { "" },
        custom = custom,
        active = active,
        default_prompt = default_prompt,
    )
}
