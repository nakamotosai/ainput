use std::io::Cursor;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::NimWhisperConfig;

/// NVIDIA NIM whisper-large-v3 客户端（云端/本地容器通用）。
///
/// 协议（NVIDIA Speech NIM HTTP ASR 文档）：
/// POST {endpoint_url}（默认 `http://127.0.0.1:9000/v1/audio/transcriptions`），
/// `multipart/form-data`，字段 `file`（WAV）+ `language`（如 `zh-CN`），
/// 回 `{"text": "..."}`。`response_format` 默认 json。
/// 同一客户端既可打本地 NIM 容器，也可打官方云地址，区别只在配置。
/// 连不上/鉴权失败一律返回 Err，由管线层如实报给用户，绝不静默回退。
#[derive(Clone)]
pub struct NimWhisperClient {
    endpoint_url: String,
    model: String,
    language: String,
    timeout: Duration,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    #[serde(default)]
    text: String,
}

impl NimWhisperClient {
    pub fn new(config: &NimWhisperConfig) -> Result<Self> {
        let endpoint_url = config.endpoint_url.trim().trim_end_matches('/').to_string();
        if endpoint_url.is_empty() {
            anyhow::bail!("nim-whisper endpoint_url is empty");
        }
        let api_key = read_api_key(config.api_key_env.trim()).or_else(|| {
            let inline = config.api_key.trim().to_string();
            (!inline.is_empty()).then_some(inline)
        });
        Ok(Self {
            endpoint_url,
            model: config.model.trim().to_string(),
            language: config.language.trim().to_string(),
            timeout: Duration::from_millis(config.request_timeout_ms.max(1000)),
            api_key,
        })
    }

    pub fn transcribe(&self, sample_rate_hz: u32, samples: &[f32]) -> Result<String> {
        let wav = encode_wav_bytes(sample_rate_hz.max(1), samples)?;
        let http = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .no_proxy()
            .build()
            .context("build NIM whisper HTTP client")?;
        let file_part = reqwest::blocking::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .context("build wav part")?;
        let mut form = reqwest::blocking::multipart::Form::new().part("file", file_part);
        if !self.language.is_empty() {
            form = form.text("language", self.language.clone());
        }
        if !self.model.is_empty() {
            form = form.text("model", self.model.clone());
        }
        let mut request = http.post(self.endpoint_url.clone()).multipart(form);
        if let Some(key) = self.api_key.as_ref() {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .context("call NIM whisper endpoint")?
            .error_for_status()
            .context("NIM whisper endpoint returned error")?
            .json::<TranscriptionResponse>()
            .context("decode NIM whisper response")?;
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

fn read_api_key(primary_env: &str) -> Option<String> {
    for name in [primary_env, "NGC_API_KEY", "AINPUT_API_KEY"] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NimWhisperConfig;

    /// mock 端到端：起一个本地 HTTP 服务装成 NIM（收 multipart，回固定文本），
    /// 证明客户端的 multipart 字段名 + JSON 解析是对的。默认忽略，
    /// mock 地址由环境变量 NIM_WHISPER_TEST_URL 传入，例如：
    /// `NIM_WHISPER_TEST_URL=http://127.0.0.1:18923/v1/audio/transcriptions`.
    /// 无该变量时直接通过（不做断言），避免 CI 无网失败。
    #[test]
    #[ignore]
    fn transcribes_against_mock_nim() {
        let url = std::env::var("NIM_WHISPER_TEST_URL").unwrap_or_default();
        if url.trim().is_empty() {
            eprintln!("NIM_WHISPER_TEST_URL unset; skip");
            return;
        }
        let mut config = NimWhisperConfig::default();
        config.endpoint_url = url;
        config.language = "zh-CN".to_string();
        config.request_timeout_ms = 15_000;
        let client = NimWhisperClient::new(&config).expect("build client");
        let samples = vec![0.0f32; 16_000];
        let text = client
            .transcribe(16_000, &samples)
            .expect("mock transcribe");
        eprintln!("nim mock text={text:?}");
        assert_eq!(text, "你好世界");
    }
}
