use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};

use crate::config::{AsrConfig, WhisperConfig};

const AUDIO_FORMAT_HEADER: &str = "X-Ainput-Audio-Format";
const AUDIO_FORMAT_PCM16: &str = "pcm_s16le";

#[derive(Clone)]
pub struct CloudAsrClient {
    http: Client,
    base_url: String,
    language: String,
    api_key: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct WhisperClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct HealthResponse {
    pub ok: bool,
    pub model: Option<String>,
    #[serde(rename = "sessions")]
    pub _sessions: Option<usize>,
    pub session_idle_ttl_sec: Option<f64>,
    pub session_reaper_interval_sec: Option<f64>,
    pub sample_rate_hz: Option<i32>,
    pub streaming_partials: Option<bool>,
    pub whisper_model: Option<String>,
    pub whisper_language: Option<String>,
    pub whisper_function_id: Option<String>,
    pub whisper_offline: Option<bool>,
    pub boost_source: Option<String>,
    pub boost_phrases: Option<usize>,
    pub speech_context_phrases: Option<usize>,
    pub speech_context_limit: Option<usize>,
    pub personal_dictionary_entries: Option<usize>,
    pub personal_dictionary_enabled_entries: Option<usize>,
}

#[derive(Debug, Serialize)]
struct StartRequest {
    context: String,
    language: Option<String>,
    unfixed_chunk_num: i32,
    unfixed_token_num: i32,
    chunk_size_sec: f64,
}

#[derive(Debug, Deserialize)]
pub struct StartResponse {
    pub session_id: String,
    pub sample_rate_hz: i32,
    pub boost_source: Option<String>,
    pub boost_phrases: Option<usize>,
    pub speech_context_phrases: Option<usize>,
    pub speech_context_limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkResponse {
    pub text: String,
    pub language: Option<String>,
    pub audio_ms: u64,
    pub elapsed_ms: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct FinishResponse {
    pub text: String,
    pub language: Option<String>,
    pub audio_ms: u64,
    pub elapsed_ms: f64,
    pub finished: bool,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OfflineTranscriptionResponse {
    pub text: String,
    pub language: Option<String>,
    pub audio_ms: u64,
    pub elapsed_ms: f64,
    pub model: String,
    pub skipped: bool,
}

impl CloudAsrClient {
    pub fn new(config: &AsrConfig) -> Result<Self> {
        let base_url = config.endpoint_url.trim().trim_end_matches('/').to_string();
        let http = Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms.max(1000)))
            .no_proxy()
            .build()
            .context("build ASR HTTP client")?;
        let api_key = read_api_key(config.api_key_env.trim()).or_else(|| {
            let inline = config.api_key.trim().to_string();
            (!inline.is_empty()).then_some(inline)
        });
        Ok(Self {
            http,
            base_url,
            language: config.language.clone(),
            api_key,
        })
    }

    pub fn health(&self) -> Result<HealthResponse> {
        let request = self.http.get(format!("{}/health", self.base_url));
        let health = with_bearer_auth(request, &self.api_key)
            .send()
            .context("call ASR health")?
            .error_for_status()
            .context("ASR health returned error")?
            .json::<HealthResponse>()
            .context("decode ASR health")?;
        if !health.ok {
            bail!("ASR health ok=false");
        }
        Ok(health)
    }

    pub fn start_session(&self) -> Result<StartResponse> {
        let start = StartRequest {
            context: String::new(),
            language: Some(self.language.clone()).filter(|language| !language.trim().is_empty()),
            unfixed_chunk_num: 4,
            unfixed_token_num: 5,
            chunk_size_sec: 0.18,
        };
        let request = self
            .http
            .post(format!("{}/v1/sessions", self.base_url))
            .json(&start);
        with_bearer_auth(request, &self.api_key)
            .send()
            .context("create ASR session")?
            .error_for_status()
            .context("ASR session create returned error")?
            .json::<StartResponse>()
            .context("decode ASR session create")
    }

    pub fn send_chunk(&self, session_id: &str, samples: &[f32]) -> Result<ChunkResponse> {
        let body = pcm16_le_bytes(samples);
        let request = self
            .http
            .post(format!("{}/v1/sessions/{session_id}/chunk", self.base_url))
            .header("Content-Type", "application/octet-stream")
            .header(AUDIO_FORMAT_HEADER, AUDIO_FORMAT_PCM16)
            .body(body);
        with_bearer_auth(request, &self.api_key)
            .send()
            .with_context(|| format!("send ASR chunk for {session_id}"))?
            .error_for_status()
            .context("ASR chunk returned error")?
            .json::<ChunkResponse>()
            .context("decode ASR chunk response")
    }

    pub fn finish_session(&self, session_id: &str) -> Result<FinishResponse> {
        let request = self
            .http
            .post(format!("{}/v1/sessions/{session_id}/finish", self.base_url));
        with_bearer_auth(request, &self.api_key)
            .send()
            .with_context(|| format!("finish ASR session {session_id}"))?
            .error_for_status()
            .context("ASR finish returned error")?
            .json::<FinishResponse>()
            .context("decode ASR finish response")
    }
}

#[allow(dead_code)]
impl WhisperClient {
    pub fn new(config: &WhisperConfig) -> Result<Self> {
        let base_url = config.endpoint_url.trim().trim_end_matches('/').to_string();
        let http = Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms.max(1000)))
            .no_proxy()
            .build()
            .context("build Whisper HTTP client")?;
        let api_key = read_api_key(config.api_key_env.trim()).or_else(|| {
            let inline = config.api_key.trim().to_string();
            (!inline.is_empty()).then_some(inline)
        });
        Ok(Self {
            http,
            base_url,
            api_key,
        })
    }

    pub fn transcribe_zh(&self, samples: &[f32]) -> Result<OfflineTranscriptionResponse> {
        let body = pcm16_le_bytes(samples);
        let request = self
            .http
            .post(format!("{}/v1/whisper-zh/transcriptions", self.base_url))
            .header("Content-Type", "application/octet-stream")
            .header(AUDIO_FORMAT_HEADER, AUDIO_FORMAT_PCM16)
            .body(body);
        with_bearer_auth(request, &self.api_key)
            .send()
            .context("call Whisper zh transcription")?
            .error_for_status()
            .context("Whisper zh transcription returned error")?
            .json::<OfflineTranscriptionResponse>()
            .context("decode Whisper zh transcription response")
    }
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

fn pcm16_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let pcm16 = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        bytes.extend_from_slice(&pcm16.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::pcm16_le_bytes;

    #[test]
    fn encodes_pcm16_little_endian_with_clamping() {
        let bytes = pcm16_le_bytes(&[-2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0]);
        let samples = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        assert_eq!(
            samples,
            vec![-32767, -32767, -16384, 0, 16384, 32767, 32767]
        );
    }
}
