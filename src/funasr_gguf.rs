use std::io::Cursor;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::FunasrGgufConfig;

/// FunASR-GGUF 边车客户端。
///
/// 协议（与 sidecar/funasr_gguf_server.py 同源定义）：
/// POST {endpoint_url}，body = 16k WAV 字节，header
/// `X-Sample-Rate`/`X-Ainput-Audio-Format: wav`，回 `{"text": "..."}`。
/// 边车没起、模型没下完一律返回 Err，由管线层如实报给用户，绝不静默回退。
#[derive(Clone)]
pub struct GgufClient {
    endpoint_url: String,
    timeout: Duration,
}

#[derive(Debug, Deserialize)]
struct TranscribeResponse {
    #[serde(default)]
    text: String,
}

impl GgufClient {
    pub fn new(config: &FunasrGgufConfig) -> Result<Self> {
        let endpoint_url = config.endpoint_url.trim().to_string();
        if endpoint_url.is_empty() {
            anyhow::bail!("funasr-gguf endpoint_url is empty");
        }
        Ok(Self {
            endpoint_url,
            timeout: Duration::from_millis(config.request_timeout_ms.max(1000)),
        })
    }

    pub fn transcribe(&self, sample_rate_hz: u32, samples: &[f32]) -> Result<String> {
        let wav = encode_wav_bytes(sample_rate_hz.max(1), samples)?;
        let http = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .no_proxy()
            .build()
            .context("build gguf sidecar HTTP client")?;
        let response = http
            .post(self.endpoint_url.clone())
            .header("Content-Type", "audio/wav")
            .header("X-Ainput-Audio-Format", "wav")
            .header("X-Sample-Rate", sample_rate_hz.max(1).to_string())
            .body(wav)
            .send()
            .context("call funasr-gguf sidecar")?
            .error_for_status()
            .context("funasr-gguf sidecar returned error")?
            .json::<TranscribeResponse>()
            .context("decode funasr-gguf sidecar response")?;
        Ok(response.text)
    }
}

fn encode_wav_bytes(sample_rate_hz: u32, samples: &[f32]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: sample_rate_hz,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::new(&mut cursor, spec).context("create wav encoder")?;
        for sample in samples {
            let pcm = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            writer
                .write_sample(pcm)
                .context("write wav sample")?;
        }
        writer.finalize().context("finalize wav")?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FunasrGgufConfig;

    /// 边车端到端：需要本机 sidecar 在跑（python sidecar/funasr_gguf_server.py）。
    /// 地址由 GGUF_TEST_URL 传入，不设则跳过。断言 rag_math 级中文能出字。
    #[test]
    #[ignore]
    fn transcribes_against_sidecar() {
        let url = std::env::var("GGUF_TEST_URL").unwrap_or_default();
        if url.trim().is_empty() {
            eprintln!("GGUF_TEST_URL unset; skip");
            return;
        }
        let wav = std::env::var("GGUF_TEST_WAV").unwrap_or_default();
        assert!(!wav.trim().is_empty(), "GGUF_TEST_WAV must point at a 16k wav");
        let mut config = FunasrGgufConfig::default();
        config.endpoint_url = url;
        config.request_timeout_ms = 120_000;
        let client = GgufClient::new(&config).expect("build client");
        let mut reader = hound::WavReader::open(&wav).expect("open wav");
        let spec = reader.spec();
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| (s.unwrap_or(0) as f32) / 32768.0)
            .collect();
        let text = client
            .transcribe(spec.sample_rate, &samples)
            .expect("sidecar transcribe");
        eprintln!("gguf text={text:?}");
        assert!(text.contains("微分") || text.contains("你好"), "unexpected text");
    }
}
