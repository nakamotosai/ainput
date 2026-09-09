use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::WhisperTurboConfig;

/// Whisper-turbo 流式边车客户端（会话式）。
///
/// 协议（与 sidecar/whisper_turbo_server.py 同源定义）：
/// POST {base}/session/start -> {id}
/// POST {base}/session/<id>/append body=pcm16le -> {stable, provisional}
/// POST {base}/session/<id>/finish -> {text}
/// 边车没起一律返回 Err，由管线层如实报给用户，绝不静默回退。
#[derive(Clone)]
pub struct TurboClient {
    base_url: String,
    http: reqwest::blocking::Client,
    append_timeout: Duration,
    finish_timeout: Duration,
}

#[derive(Debug, Deserialize)]
struct StartResponse {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct AppendResponse {
    #[serde(default)]
    stable: String,
    #[serde(default)]
    provisional: String,
}

#[derive(Debug, Deserialize)]
struct FinishResponse {
    #[serde(default)]
    text: String,
}

impl TurboClient {
    pub fn new(config: &WhisperTurboConfig) -> Result<Self> {
        let base_url = config.endpoint_url.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            anyhow::bail!("whisper-turbo endpoint_url is empty");
        }
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .context("build whisper-turbo sidecar HTTP client")?;
        Ok(Self {
            base_url,
            http,
            append_timeout: Duration::from_millis(config.append_timeout_ms.max(2000)),
            finish_timeout: Duration::from_millis(config.finish_timeout_ms.max(5000)),
        })
    }

    pub fn start_session(&self) -> Result<String> {
        let response = self
            .http
            .post(format!("{}/session/start", self.base_url))
            .timeout(Duration::from_millis(10_000))
            .send()
            .context("call whisper-turbo sidecar (边车没起？先跑 scripts/start_turbo_sidecar.ps1)")?
            .error_for_status()
            .context("whisper-turbo sidecar returned error")?
            .json::<StartResponse>()
            .context("decode whisper-turbo sidecar response")?;
        if response.id.trim().is_empty() {
            anyhow::bail!("whisper-turbo sidecar returned empty session id");
        }
        Ok(response.id)
    }

    /// 送一段 16k 音频，拿回（已定稿 stable，未定稿 provisional）。
    pub fn append(&self, session_id: &str, samples_16k: &[f32]) -> Result<(String, String)> {
        let pcm = encode_pcm16_bytes(samples_16k);
        let response = self
            .http
            .post(format!("{}/session/{}/append", self.base_url, session_id))
            .header("Content-Type", "application/octet-stream")
            .timeout(self.append_timeout)
            .body(pcm)
            .send()
            .context("call whisper-turbo sidecar append")?
            .error_for_status()
            .context("whisper-turbo sidecar returned error")?
            .json::<AppendResponse>()
            .context("decode whisper-turbo sidecar response")?;
        Ok((response.stable, response.provisional))
    }

    pub fn finish(&self, session_id: &str) -> Result<String> {
        let response = self
            .http
            .post(format!("{}/session/{}/finish", self.base_url, session_id))
            .timeout(self.finish_timeout)
            .body(Vec::new())
            .send()
            .context("call whisper-turbo sidecar finish")?
            .error_for_status()
            .context("whisper-turbo sidecar returned error")?
            .json::<FinishResponse>()
            .context("decode whisper-turbo sidecar response")?;
        Ok(response.text)
    }

    pub fn cancel(&self, session_id: &str) {
        let _ = self
            .http
            .post(format!("{}/session/{}/cancel", self.base_url, session_id))
            .timeout(Duration::from_millis(3000))
            .body(Vec::new())
            .send();
    }
}

fn encode_pcm16_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        out.extend_from_slice(
            &((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WhisperTurboConfig;

    /// 边车端到端：需要 turbo sidecar 在跑（hub 上 turbosidecar 或手跑）。
    /// 地址由 TURBO_TEST_URL 传入，不设则默认 127.0.0.1:8766 直连，断言会话能走完。
    #[test]
    #[ignore]
    fn streams_against_turbo_sidecar() {
        let mut config = WhisperTurboConfig::default();
        if let Ok(url) = std::env::var("TURBO_TEST_URL") {
            if !url.trim().is_empty() {
                config.endpoint_url = url;
            }
        }
        let client = TurboClient::new(&config).expect("build turbo client");
        let health = client
            .http
            .get(format!("{}/healthz", config.endpoint_url.trim_end_matches('/')))
            .timeout(Duration::from_millis(5000))
            .send();
        let Ok(health) = health else {
            eprintln!("turbo sidecar not running; skip");
            return;
        };
        assert!(health.status().is_success(), "sidecar unhealthy");
        let sid = client.start_session().expect("start session");
        // 用 paraformer 自带 1.wav 切块喂，模拟按住说话。
        let wav_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("models")
            .join("paraformer-streaming")
            .join("sherpa-onnx-streaming-paraformer-bilingual-zh-en")
            .join("test_wavs")
            .join("1.wav");
        assert!(wav_path.exists(), "missing test wav {}", wav_path.display());
        let mut reader = hound::WavReader::open(&wav_path).expect("open wav");
        let spec = reader.spec();
        let raw: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => {
                reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect()
            }
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample as u32 - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| (s.unwrap_or(0) as f32) / max)
                    .collect()
            }
        };
        let mono: Vec<f32> = if spec.channels > 1 {
            let ch = spec.channels as usize;
            raw.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
        } else {
            raw
        };
        // 重采样到 16k（测试 wav 若已是 16k 则直通）。
        let samples_16k: Vec<f32> = if spec.sample_rate == 16_000 {
            mono
        } else {
            let ratio = 16_000.0 / spec.sample_rate.max(1) as f32;
            let out_len = (mono.len() as f32 * ratio) as usize;
            (0..out_len)
                .map(|i| {
                    let pos = i as f32 / ratio;
                    let lo = pos.floor() as usize;
                    let hi = (lo + 1).min(mono.len().saturating_sub(1));
                    mono[lo] * (1.0 - pos.fract()) + mono[hi] * pos.fract()
                })
                .collect()
        };
        let block = 8000; // 500ms 一块
        let mut offset = 0usize;
        let mut last_hyp = String::new();
        while offset < samples_16k.len() {
            let end = (offset + block).min(samples_16k.len());
            let (stable, provisional) = client
                .append(&sid, &samples_16k[offset..end])
                .expect("append");
            last_hyp = format!("{stable}{provisional}");
            offset = end;
        }
        let text = client.finish(&sid).expect("finish");
        eprintln!("turbo interim={last_hyp:?} final={text:?}");
        assert!(!text.trim().is_empty(), "empty final transcription");
    }
}
