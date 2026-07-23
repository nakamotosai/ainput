//! Loopback UI: capture / set voice hotkey (keyboard + mouse side buttons).

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

use crate::hotkey::{
    capture_next_hotkey, hotkey_supports_suppress, validate_hotkey_label, vk_display_name,
};
use crate::hotkey_user::HotkeyUserController;
use crate::web_ui::{
    escape_html, open_browser_hidden, read_http_request, request_method, request_path, write_response,
};

#[derive(Clone)]
pub struct HotkeyPanelController {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    shutdown: Arc<AtomicBool>,
}

struct ServerState {
    hotkey: HotkeyUserController,
    #[allow(dead_code)]
    path: PathBuf,
}

impl HotkeyPanelController {
    pub fn start(hotkey: HotkeyUserController, shutdown: Arc<AtomicBool>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind hotkey web server")?;
        let addr = listener.local_addr().context("hotkey listener local_addr")?;
        let base_url = format!("http://{addr}");
        let state = Arc::new(ServerState {
            path: hotkey.path().to_path_buf(),
            hotkey,
        });
        let shutdown_server = Arc::clone(&shutdown);
        let state_server = Arc::clone(&state);
        thread::Builder::new()
            .name("ainput-hotkey-web".into())
            .spawn(move || {
                if let Err(error) = run_server(listener, state_server, shutdown_server) {
                    warn!(error = %error, "hotkey web server stopped with error");
                } else {
                    info!("hotkey web server stopped");
                }
            })
            .context("spawn hotkey web server")?;
        info!(%base_url, "hotkey web UI ready (loopback)");
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
            Ok(()) => info!(%url, "opened hotkey web UI"),
            Err(error) => warn!(error = %error, %url, "open hotkey web UI failed"),
        }
    }
}

fn run_server(
    listener: TcpListener,
    state: Arc<ServerState>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    listener.set_nonblocking(true).context("set nonblocking")?;
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    if let Err(error) = handle_client(stream, &state) {
                        warn!(error = %error, peer = %peer, "hotkey web request failed");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                warn!(error = %error, "hotkey accept failed");
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
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                state_json(state).as_bytes(),
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
        ("POST", "/api/capture") => {
            // Blocking capture up to 8s — client waits.
            let resp = run_capture();
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
        ("POST", "/api/reset") => {
            let resp = match state.hotkey.set_local_nonstreaming("CapsLock") {
                Ok(label) => serde_json::json!({
                    "ok": true,
                    "hotkey": label,
                    "message": "已恢复 CapsLock · 请重启 ainput 后生效",
                    "restart_required": true,
                }),
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            };
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
    hotkey: Option<String>,
}

fn state_json(state: &ServerState) -> String {
    let label = state.hotkey.local_nonstreaming();
    serde_json::json!({
        "hotkey": label,
        "suppress_key": hotkey_supports_suppress(&label),
        "path": state.hotkey.path().display().to_string(),
        "restart_required_note": "修改后需重启 ainput 才会切换监听键",
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
    let Some(hotkey) = parsed.hotkey else {
        return serde_json::json!({"ok": false, "error": "缺少 hotkey 字段"});
    };
    match state.hotkey.set_local_nonstreaming(&hotkey) {
        Ok(label) => serde_json::json!({
            "ok": true,
            "hotkey": label,
            "suppress_key": hotkey_supports_suppress(&label),
            "message": format!("已保存：{label} · 请重启 ainput 后生效"),
            "restart_required": true,
        }),
        Err(error) => serde_json::json!({"ok": false, "error": error}),
    }
}

fn run_capture() -> serde_json::Value {
    // Wait until all probe keys released, then capture first new press (max 8s).
    match capture_next_hotkey(Duration::from_secs(8)) {
        Ok(label) => {
            // validate
            if let Err(error) = validate_hotkey_label(&label) {
                return serde_json::json!({"ok": false, "error": error.to_string()});
            }
            serde_json::json!({
                "ok": true,
                "hotkey": label,
                "display": vk_display_name(&label),
                "message": format!("捕获到：{label}"),
            })
        }
        Err(error) => serde_json::json!({"ok": false, "error": error.to_string()}),
    }
}

fn page_html(state: &ServerState) -> String {
    let current = escape_html(&state.hotkey.local_nonstreaming());
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>ainput · 语音快捷键</title>
<style>
:root {{ color-scheme: dark; --bg:#0f1419; --card:#1a222c; --fg:#e7ecf1; --muted:#8b9aab; --acc:#3d9cf0; --ok:#3dd68c; --bd:#2a3542; --warn:#f0b429; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:14px/1.5 "Segoe UI","Microsoft YaHei UI",sans-serif; background:var(--bg); color:var(--fg); }}
main {{ max-width:720px; margin:24px auto; padding:0 16px 48px; }}
h1 {{ font-size:20px; font-weight:650; margin:0 0 6px; }}
.sub {{ color:var(--muted); margin-bottom:18px; }}
.card {{ background:var(--card); border:1px solid var(--bd); border-radius:12px; padding:16px; margin-bottom:14px; }}
label {{ display:block; margin:0 0 6px; color:var(--muted); font-size:12px; }}
input, button {{ width:100%; font:inherit; color:var(--fg); background:#0c1117; border:1px solid var(--bd); border-radius:8px; padding:10px 12px; }}
button {{ cursor:pointer; background:var(--acc); border:none; font-weight:600; margin-top:10px; }}
button.secondary {{ background:#243041; }}
button.pick {{ margin-top:0; font-size:13px; padding:8px 10px; }}
.row {{ display:flex; gap:10px; flex-wrap:wrap; }}
.row button {{ flex:1; min-width:100px; }}
#status {{ margin-top:10px; color:var(--muted); min-height:1.4em; }}
#status.ok {{ color:var(--ok); }}
#status.warn {{ color:var(--warn); }}
.hint {{ font-size:12px; color:var(--muted); margin-top:8px; }}
code {{ background:#0c1117; padding:1px 6px; border-radius:4px; }}
ul {{ margin:8px 0 0 18px; color:var(--muted); font-size:13px; }}
.capture-on {{ outline:2px solid var(--warn); outline-offset:2px; }}
</style>
</head>
<body>
<main>
  <h1>语音快捷键</h1>
  <div class="sub">按住说话用的键。支持键盘（含 F13–F24）与鼠标侧键 MouseX1/X2。雷蛇专有键请在 Synapse 映射到侧键或 F13+。</div>
  <div class="card">
    <label>当前（重启后生效）</label>
    <input id="hotkey" value="{current}" spellcheck="false"/>
    <div class="row" style="margin-top:10px">
      <button id="capture" type="button">录制按键/侧键…</button>
      <button id="save" class="secondary" type="button">保存</button>
      <button id="reset" class="secondary" type="button">恢复 CapsLock</button>
    </div>
    <div id="status"></div>
    <div class="hint">
      <strong>MouseX1/X2</strong> 保存并重启后，ainput 会像 CapsLock 一样<strong>剥掉系统后退/前进</strong>，只保留按住说话。
      设置页里仍会拦截导航，方便录制。改完必须<strong>退出并重新打开 ainput</strong>。
    </div>
  </div>
  <div class="card">
    <label>一键选用（推荐侧键用这个）</label>
    <div class="row">
      <button type="button" class="secondary pick" data-hk="MouseX1">MouseX1 侧键·退</button>
      <button type="button" class="secondary pick" data-hk="MouseX2">MouseX2 侧键·进</button>
      <button type="button" class="secondary pick" data-hk="CapsLock">CapsLock</button>
      <button type="button" class="secondary pick" data-hk="F13">F13</button>
    </div>
  </div>
  <div class="card">
    <label>说明</label>
    <ul>
      <li><code>MouseX1</code> / <code>MouseX2</code>：系统级吞掉后退/前进（WH_MOUSE_LL），与 CapsLock 同路径</li>
      <li>录制可在本页完成；侧键也可点一键选用</li>
      <li>组合键可手填，如 <code>Ctrl+F8</code>（组合键无法吞系统行为，仅轮询）</li>
      <li>雷蛇专有键：Synapse 映到 MouseX1/X2 或 F13–F24</li>
    </ul>
  </div>
</main>
<script>
const hotkeyEl = document.getElementById('hotkey');
const statusEl = document.getElementById('status');
const captureBtn = document.getElementById('capture');
let capturing = false;
let captureTimer = null;

function setStatus(text, cls) {{
  statusEl.className = cls || '';
  statusEl.textContent = text;
}}

// —— 关键：MouseX1/X2 在浏览器里默认 = 后退/前进，会刷掉本页 ——
// button 3 = X1(back), button 4 = X2(forward)
function isSideButton(e) {{
  return e.button === 3 || e.button === 4;
}}
function sideLabel(e) {{
  return e.button === 3 ? 'MouseX1' : 'MouseX2';
}}
function blockBrowserNav(e) {{
  if (!isSideButton(e)) return;
  e.preventDefault();
  e.stopPropagation();
  if (typeof e.stopImmediatePropagation === 'function') e.stopImmediatePropagation();
  return false;
}}
['mousedown','mouseup','auxclick','pointerdown','pointerup','click'].forEach(type => {{
  window.addEventListener(type, blockBrowserNav, true);
  document.addEventListener(type, blockBrowserNav, true);
}});
// 再挡一层 history 后退
try {{
  history.pushState({{ainput:1}}, '', location.href);
  window.addEventListener('popstate', () => {{
    history.pushState({{ainput:1}}, '', location.href);
  }});
}} catch (_) {{}}

function finishCapture(label) {{
  if (!capturing) return;
  capturing = false;
  if (captureTimer) {{ clearTimeout(captureTimer); captureTimer = null; }}
  captureBtn.classList.remove('capture-on');
  captureBtn.textContent = '录制按键/侧键…';
  hotkeyEl.value = label;
  setStatus('捕获到：' + label + ' · 点「保存」写入', 'ok');
}}

function startCapture() {{
  if (capturing) return;
  capturing = true;
  captureBtn.classList.add('capture-on');
  captureBtn.textContent = '录制中…再按目标键';
  setStatus('录制中：请按键盘键或鼠标侧键（本页已拦截后退/前进）', 'warn');
  if (captureTimer) clearTimeout(captureTimer);
  captureTimer = setTimeout(() => {{
    if (!capturing) return;
    capturing = false;
    captureBtn.classList.remove('capture-on');
    captureBtn.textContent = '录制按键/侧键…';
    setStatus('超时：未捕获到按键', '');
  }}, 10000);
}}

// 侧键：在捕获模式写入；平时也拦截导航
window.addEventListener('mousedown', (e) => {{
  if (!isSideButton(e)) return;
  blockBrowserNav(e);
  if (capturing) finishCapture(sideLabel(e));
}}, true);

// 键盘捕获（忽略单独修饰键）
window.addEventListener('keydown', (e) => {{
  if (!capturing) return;
  if (['Control','Alt','Shift','Meta'].includes(e.key)) return;
  e.preventDefault();
  e.stopPropagation();
  let label = '';
  if (e.key === 'CapsLock') label = 'CapsLock';
  else if (e.key === ' ') label = 'Space';
  else if (e.key === 'Tab') label = 'Tab';
  else if (e.key === 'Enter') label = 'Enter';
  else if (e.key === 'Escape') label = 'Esc';
  else if (/^F([1-9]|1[0-9]|2[0-4])$/i.test(e.key)) label = e.key.toUpperCase();
  else if (e.key.length === 1 && /[a-zA-Z0-9]/.test(e.key)) {{
    const parts = [];
    if (e.ctrlKey) parts.push('Ctrl');
    if (e.altKey) parts.push('Alt');
    if (e.shiftKey) parts.push('Shift');
    if (e.metaKey) parts.push('Win');
    parts.push(e.key.length === 1 ? e.key.toUpperCase() : e.key);
    label = parts.join('+');
  }} else {{
    return;
  }}
  finishCapture(label);
}}, true);

captureBtn.addEventListener('click', (e) => {{
  e.preventDefault();
  startCapture();
}});

document.querySelectorAll('button.pick').forEach(btn => {{
  btn.addEventListener('click', (e) => {{
    e.preventDefault();
    const hk = btn.getAttribute('data-hk');
    if (!hk) return;
    hotkeyEl.value = hk;
    setStatus('已填入 ' + hk + ' · 点「保存」写入', 'ok');
  }});
}});

document.getElementById('save').addEventListener('click', async (e) => {{
  e.preventDefault();
  setStatus('保存中…', '');
  try {{
    const resp = await fetch('/api/save', {{
      method: 'POST',
      headers: {{'Content-Type':'application/json'}},
      body: JSON.stringify({{ hotkey: hotkeyEl.value }}),
    }});
    const data = await resp.json();
    if (!data.ok) throw new Error(data.error || '保存失败');
    hotkeyEl.value = data.hotkey;
    setStatus(data.message, 'warn');
  }} catch (err) {{
    setStatus(String(err.message || err), '');
  }}
}});
document.getElementById('reset').addEventListener('click', async (e) => {{
  e.preventDefault();
  try {{
    const resp = await fetch('/api/reset', {{ method: 'POST' }});
    const data = await resp.json();
    if (!data.ok) throw new Error(data.error || '重置失败');
    hotkeyEl.value = data.hotkey;
    setStatus(data.message, 'warn');
  }} catch (err) {{
    setStatus(String(err.message || err), '');
  }}
}});
</script>
</body>
</html>
"#,
        current = current,
    )
}

