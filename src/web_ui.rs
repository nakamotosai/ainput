//! Shared helpers for loopback HTML UIs (history + API settings).

use std::io::Write;
use std::net::TcpStream;

use anyhow::{Context, Result, anyhow};

pub fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    let _ = stream.flush();
    Ok(())
}

/// Open default browser without a console window flash.
pub fn open_browser_hidden(url: &str) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::PCWSTR;

        let url_wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        let operation: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let rc = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(operation.as_ptr()),
                PCWSTR(url_wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if rc.0 as usize > 32 {
            return Ok(());
        }
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = std::process::Command::new("explorer.exe")
            .arg(url)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .context("spawn explorer for browser")?;
        if status.success() {
            return Ok(());
        }
        return Err(anyhow!(
            "ShellExecute failed (rc={}) and explorer exited {status}",
            rc.0 as usize
        ));
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err(anyhow!("web UI open is Windows-only"))
    }
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Read full HTTP request body after headers (Content-Length).
pub fn read_http_request(stream: &mut TcpStream) -> Result<(String, Vec<u8>)> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    // Read until headers complete or buffer large.
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            // May still need body bytes.
            if let Some(header_end) = find_header_end(&buf) {
                let headers = &buf[..header_end];
                let content_length = parse_content_length(headers).unwrap_or(0);
                let body_start = header_end + 4;
                while buf.len() < body_start + content_length {
                    let n = stream.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
                let body = if body_start < buf.len() {
                    buf[body_start..body_start + content_length.min(buf.len() - body_start)].to_vec()
                } else {
                    Vec::new()
                };
                return Ok((head, body));
            }
        }
        if buf.len() > 2 * 1024 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    Ok((head, Vec::new()))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let s = String::from_utf8_lossy(headers);
    for line in s.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

pub fn request_path(first_line: &str) -> &str {
    first_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
}

pub fn request_method(first_line: &str) -> &str {
    first_line.split_whitespace().next().unwrap_or("GET")
}
