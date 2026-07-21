//! Local web UI for dictation history (loopback only).
//!
//! Opens the default browser to `http://127.0.0.1:<port>/`.
//! No native Win32 multi-line EDIT (avoids stacked-glyph bugs).
//! No Python required; pure Rust TCP serve.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::history::{self, HistoryRecord};
use crate::web_ui::{escape_html, open_browser_hidden, write_response};

#[derive(Clone)]
pub struct HistoryPanelController {
    inner: Arc<Inner>,
}

struct Inner {
    history_path: PathBuf,
    base_url: String,
    shutdown: Arc<AtomicBool>,
    /// Last open error for diagnostics.
    last_error: Mutex<Option<String>>,
}

impl HistoryPanelController {
    pub fn start(history_path: PathBuf, shutdown: Arc<AtomicBool>) -> Result<Self> {
        // Bind ephemeral port on loopback only.
        let listener = TcpListener::bind("127.0.0.1:0").context("bind history web server")?;
        listener
            .set_nonblocking(false)
            .context("configure history listener")?;
        let addr = listener
            .local_addr()
            .context("history listener local_addr")?;
        let base_url = format!("http://{addr}");
        let path_for_server = history_path.clone();
        let shutdown_server = Arc::clone(&shutdown);

        thread::Builder::new()
            .name("ainput-history-web".into())
            .spawn(move || {
                if let Err(error) = run_server(listener, path_for_server, shutdown_server) {
                    warn!(error = %error, "history web server stopped with error");
                } else {
                    info!("history web server stopped");
                }
            })
            .context("spawn history web server")?;

        info!(%base_url, path = %history_path.display(), "history web UI ready (loopback)");
        Ok(Self {
            inner: Arc::new(Inner {
                history_path,
                base_url,
                shutdown,
                last_error: Mutex::new(None),
            }),
        })
    }

    pub fn open(&self) {
        if self.inner.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let url = self.inner.base_url.clone();
        match open_browser_hidden(&url) {
            Ok(()) => {
                info!(%url, "opened history web UI in browser");
                if let Ok(mut slot) = self.inner.last_error.lock() {
                    *slot = None;
                }
            }
            Err(error) => {
                warn!(error = %error, %url, "open history web UI failed");
                if let Ok(mut slot) = self.inner.last_error.lock() {
                    *slot = Some(error.to_string());
                }
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.history_path
    }

    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }
}

fn run_server(listener: TcpListener, history_path: PathBuf, shutdown: Arc<AtomicBool>) -> Result<()> {
    // Accept with short timeout so shutdown can exit.
    listener
        .set_nonblocking(true)
        .context("set nonblocking")?;
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let path = history_path.clone();
                thread::spawn(move || {
                    if let Err(error) = handle_client(stream, &path) {
                        warn!(error = %error, peer = %peer, "history web request failed");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                warn!(error = %error, "history web accept failed");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Ok(())
}

fn handle_client(mut stream: TcpStream, history_path: &Path) -> Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let first_line = req.lines().next().unwrap_or("");
    let path = first_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/");

    match path {
        "/" | "/index.html" => {
            let records = history::load_recent(history_path, 500).unwrap_or_default();
            let html = render_html_page(history_path, &records);
            write_response(&mut stream, "200 OK", "text/html; charset=utf-8", html.as_bytes())?;
        }
        "/api/history" | "/api/history.json" => {
            let records = history::load_recent(history_path, 500).unwrap_or_default();
            let payload = serde_json::json!({
                "path": history_path.display().to_string(),
                "count": records.len(),
                "rewrite_enabled_count": records.iter().filter(|r| r.rewrite_enabled).count(),
                "records": records,
            });
            let body = serde_json::to_vec_pretty(&payload).unwrap_or_else(|_| b"[]".to_vec());
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                &body,
            )?;
        }
        "/api/summary" => {
            let records = history::load_recent(history_path, 500).unwrap_or_default();
            let rewrite_n = records.iter().filter(|r| r.rewrite_enabled).count();
            let with_ba = records
                .iter()
                .filter(|r| {
                    r.rewrite_enabled
                        && !r.raw_text.trim().is_empty()
                        && (!r.rewrite_text.trim().is_empty() || !r.pasted_text.trim().is_empty())
                })
                .count();
            let body = serde_json::to_vec(&serde_json::json!({
                "total": records.len(),
                "rewrite_enabled": rewrite_n,
                "with_before_after": with_ba,
                "path": history_path.display().to_string(),
            }))
            .unwrap_or_default();
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                &body,
            )?;
        }
        _ if path.starts_with("/open-folder") => {
            open_folder(history_path);
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                br#"{"ok":true}"#,
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

fn open_folder(history_path: &Path) {
    let folder = history_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| history_path.to_path_buf());
    // explorer is fine; not a console python flash.
    let _ = std::process::Command::new("explorer.exe")
        .arg(folder.as_os_str())
        .spawn();
}

fn short_error(text: &str, max_chars: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let mut out: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn render_html_page(path: &Path, records: &[HistoryRecord]) -> String {
    let total = records.len();
    let rewrite_n = records.iter().filter(|r| r.rewrite_enabled).count();
    let with_ba = records
        .iter()
        .filter(|r| {
            r.rewrite_enabled
                && !r.raw_text.trim().is_empty()
                && (!r.rewrite_text.trim().is_empty() || !r.pasted_text.trim().is_empty())
        })
        .count();

    let mut cards = String::new();
    for (index, record) in records.iter().rev().enumerate() {
        let n = index + 1;
        let mode = if record.rewrite_enabled {
            "AI改写"
        } else {
            "原文直出"
        };
        let mode_class = if record.rewrite_enabled {
            "tag-ai"
        } else {
            "tag-raw"
        };
        let target = if record.target_process.trim().is_empty() {
            "未知应用"
        } else {
            record.target_process.as_str()
        };
        let raw = first_nonempty(&[&record.raw_text, &record.finalized_text]);
        let rewritten = first_nonempty(&[&record.rewrite_text]);
        let pasted = record.pasted_text.trim();

        cards.push_str("<article class=\"card\">");
        cards.push_str(&format!(
            "<header><span class=\"idx\">#{n}</span> <span class=\"tag {mode_class}\">{mode}</span> <span class=\"meta\">{target} · {}ms</span></header>",
            record.total_elapsed_ms
        ));

        if record.rewrite_enabled {
            cards.push_str(&format!(
                "<div class=\"row\"><div class=\"label\">改写前</div><div class=\"text\">{}</div></div>",
                escape_html(if raw.is_empty() { "(空)" } else { raw })
            ));
            if rewritten.is_empty() && !record.rewrite_error.is_empty() {
                cards.push_str(
                    "<div class=\"row\"><div class=\"label\">改写后</div><div class=\"text fail\">(失败，见错误)</div></div>",
                );
            } else {
                cards.push_str(&format!(
                    "<div class=\"row\"><div class=\"label\">改写后</div><div class=\"text\">{}</div></div>",
                    escape_html(if rewritten.is_empty() { "(空)" } else { rewritten })
                ));
            }
            if !pasted.is_empty() && pasted != rewritten {
                cards.push_str(&format!(
                    "<div class=\"row\"><div class=\"label\">最终粘贴</div><div class=\"text\">{}</div></div>",
                    escape_html(pasted)
                ));
            }
            if !record.rewrite_model.is_empty() || record.rewrite_elapsed_ms > 0 {
                cards.push_str(&format!(
                    "<div class=\"hint\">模型: {} · 改写耗时 {}ms</div>",
                    escape_html(if record.rewrite_model.is_empty() {
                        "(未记)"
                    } else {
                        &record.rewrite_model
                    }),
                    record.rewrite_elapsed_ms
                ));
            }
            if !record.rewrite_error.is_empty() {
                cards.push_str(&format!(
                    "<div class=\"err\">改写错误: {}</div>",
                    escape_html(&short_error(&record.rewrite_error, 360))
                ));
            }
        } else {
            let preview = if !pasted.is_empty() {
                pasted
            } else if !raw.is_empty() {
                raw
            } else {
                "(空)"
            };
            cards.push_str(&format!(
                "<div class=\"row\"><div class=\"label\">原文</div><div class=\"text\">{}</div></div>",
                escape_html(preview)
            ));
        }

        if !record.error.is_empty() {
            cards.push_str(&format!(
                "<div class=\"err\">状态: {}</div>",
                escape_html(&short_error(&record.error, 200))
            ));
        } else if !record.skipped_reason.is_empty()
            && record.skipped_reason != "rewrite_disabled_raw_paste"
        {
            cards.push_str(&format!(
                "<div class=\"hint\">状态: {}</div>",
                escape_html(&short_error(&record.skipped_reason, 160))
            ));
        }

        cards.push_str("</article>");
    }

    if cards.is_empty() {
        cards.push_str(
            r#"<div class="empty">暂无记录。按住 CapsLock 说几句后点「刷新」。</div>"#,
        );
    }

    let path_esc = escape_html(&path.display().to_string());
    format!(
        r##"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>ainput · 听写历史</title>
<style>
  :root {{
    color-scheme: dark;
    --bg: #141414;
    --panel: #1c1a1a;
    --card: #242020;
    --text: #f2f0f0;
    --muted: #a39a96;
    --line: #3a3230;
    --ai: #6aa84f;
    --raw: #888;
    --fail: #e06c75;
    --btn: #322a2a;
    --btn-hover: #433838;
    font-family: "Segoe UI", "Microsoft YaHei UI", "PingFang SC", sans-serif;
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0;
    background: var(--bg);
    color: var(--text);
    line-height: 1.55;
    font-size: 16px;
  }}
  header.app {{
    position: sticky;
    top: 0;
    z-index: 10;
    background: rgba(20,20,20,.92);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--line);
    padding: 16px 20px 14px;
  }}
  h1 {{
    margin: 0 0 6px;
    font-size: 22px;
    font-weight: 650;
  }}
  .summary {{ color: var(--muted); font-size: 14px; }}
  .path {{ color: var(--muted); font-size: 12px; word-break: break-all; margin-top: 4px; }}
  .actions {{
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 12px;
  }}
  button, a.btn {{
    appearance: none;
    border: 1px solid var(--line);
    background: var(--btn);
    color: var(--text);
    border-radius: 10px;
    padding: 10px 14px;
    font-size: 14px;
    cursor: pointer;
    text-decoration: none;
  }}
  button:hover, a.btn:hover {{ background: var(--btn-hover); }}
  main {{
    max-width: 920px;
    margin: 0 auto;
    padding: 18px 16px 40px;
    display: grid;
    gap: 12px;
  }}
  .card {{
    background: var(--card);
    border: 1px solid var(--line);
    border-radius: 14px;
    padding: 14px 14px 12px;
  }}
  .card header {{
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-bottom: 10px;
  }}
  .idx {{ color: var(--muted); font-size: 13px; }}
  .tag {{
    font-size: 12px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--line);
  }}
  .tag-ai {{ color: #c6efb0; border-color: #3d5a2e; background: #1f2a18; }}
  .tag-raw {{ color: #ccc; }}
  .meta {{ color: var(--muted); font-size: 12px; }}
  .row {{
    display: grid;
    grid-template-columns: 72px 1fr;
    gap: 8px;
    margin: 6px 0;
  }}
  .label {{ color: var(--muted); font-size: 13px; padding-top: 2px; }}
  .text {{ white-space: pre-wrap; word-break: break-word; font-size: 15px; }}
  .text.fail {{ color: var(--fail); }}
  .hint {{ color: var(--muted); font-size: 12px; margin-top: 6px; }}
  .err {{ color: var(--fail); font-size: 12px; margin-top: 6px; word-break: break-word; }}
  .empty {{
    text-align: center;
    color: var(--muted);
    padding: 48px 12px;
    border: 1px dashed var(--line);
    border-radius: 14px;
  }}
  footer {{
    max-width: 920px;
    margin: 0 auto 28px;
    padding: 0 16px;
    color: var(--muted);
    font-size: 12px;
  }}
</style>
</head>
<body>
<header class="app">
  <h1>听写历史</h1>
  <div class="summary">共 {total} 条 · 开启过改写 {rewrite_n} 条 · 有前后对比 {with_ba} 条</div>
  <div class="path">存档: {path_esc}</div>
  <div class="actions">
    <button type="button" onclick="location.reload()">刷新</button>
    <button type="button" onclick="openFolder()">打开存档目录</button>
    <a class="btn" href="/api/history.json" target="_blank" rel="noreferrer">原始 JSON</a>
  </div>
</header>
<main>
{cards}
</main>
<footer>本地 loopback 页面 · 仅本机 · 不上云 · ainput {version}</footer>
<script>
async function openFolder() {{
  try {{
    await fetch('/open-folder');
  }} catch (e) {{
    console.warn(e);
  }}
}}
</script>
</body>
</html>
"##,
        total = total,
        rewrite_n = rewrite_n,
        with_ba = with_ba,
        path_esc = path_esc,
        cards = cards,
        version = env!("CARGO_PKG_VERSION"),
    )
}

fn first_nonempty<'a>(parts: &[&'a str]) -> &'a str {
    for part in parts {
        if !part.trim().is_empty() {
            return part.trim();
        }
    }
    ""
}
