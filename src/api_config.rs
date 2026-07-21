use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ApiConnections {
    pub path: PathBuf,
    pub config: ApiConnectionsConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ApiConnectionsConfig {
    pub version: u32,
    pub cliproxyapi: CliproxyApiConfig,
    pub rewrite: RewriteApiConfig,
    pub embedding: EmbeddingApiConfig,
    pub whisper: WhisperApiConfig,
    pub asr_sidecar: AsrSidecarApiConfig,
    pub setup_checks: SetupCheckConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CliproxyApiConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub api_key: String,
    pub chat_completions_path: String,
    pub embeddings_path: String,
    pub models_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RewriteApiConfig {
    pub model: String,
    pub fallback_models: Vec<String>,
    /// HTTP timeout for rewrite chat/completions (milliseconds).
    #[serde(default = "default_rewrite_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_rewrite_timeout_ms() -> u64 {
    5_000
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct EmbeddingApiConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub api_key: String,
    pub model: String,
    pub fallback_models: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct WhisperApiConfig {
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AsrSidecarApiConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SetupCheckConfig {
    pub warn_if_missing_models: bool,
    pub required_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiSetupStatus {
    pub ok: bool,
    pub models_url: String,
    pub required_models: Vec<String>,
    pub available_model_count: usize,
    pub missing_models: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
}

impl ApiConnections {
    pub fn load_or_create(state_root: &Path, install_root: &Path) -> Result<Self> {
        let path = state_root.join("config").join("api-connections.json");
        if !path.exists() {
            std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
                .with_context(|| format!("create API config dir {}", path.display()))?;
            let template = install_root
                .join("config")
                .join("api-connections.example.json");
            if template.exists() {
                std::fs::copy(&template, &path).with_context(|| {
                    format!(
                        "copy API config template {} -> {}",
                        template.display(),
                        path.display()
                    )
                })?;
            } else {
                let raw = serde_json::to_string_pretty(&ApiConnectionsConfig::default())
                    .context("serialize default API connections config")?;
                std::fs::write(&path, format!("{raw}\n"))
                    .with_context(|| format!("write API config {}", path.display()))?;
            }
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read API config {}", path.display()))?;
        let raw_value = serde_json::from_str::<Value>(&raw).ok();
        let mut config: ApiConnectionsConfig = serde_json::from_str(&raw)
            .with_context(|| format!("parse API config {}", path.display()))?;
        if config.normalize_missing_defaults(raw_value.as_ref()) {
            let normalized = serde_json::to_string_pretty(&config)
                .context("serialize normalized API connections config")?;
            std::fs::write(&path, format!("{normalized}\n"))
                .with_context(|| format!("write normalized API config {}", path.display()))?;
        }
        Ok(Self { path, config })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create API config dir {}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(&self.config)
            .context("serialize API connections config")?;
        std::fs::write(&self.path, format!("{raw}\n"))
            .with_context(|| format!("write API config {}", self.path.display()))
    }

    pub fn update_openai_compatible(
        &mut self,
        base_url: &str,
        api_key: &str,
        model: &str,
    ) -> Result<()> {
        self.config.cliproxyapi.base_url = base_url.trim().trim_end_matches('/').to_string();
        self.config.cliproxyapi.api_key = api_key.trim().to_string();
        if self.config.cliproxyapi.api_key_env.trim().is_empty() {
            self.config.cliproxyapi.api_key_env = "AINPUT_API_KEY".to_string();
        }
        if self.config.cliproxyapi.chat_completions_path.trim().is_empty() {
            self.config.cliproxyapi.chat_completions_path = "/v1/chat/completions".to_string();
        }
        if self.config.cliproxyapi.models_path.trim().is_empty() {
            self.config.cliproxyapi.models_path = "/v1/models".to_string();
        }
        self.config.rewrite.model = model.trim().to_string();
        self.save()
    }
}

impl ApiConnectionsConfig {
    fn normalize_missing_defaults(&mut self, raw: Option<&Value>) -> bool {
        let mut changed = false;
        let clip_default = CliproxyApiConfig::default();
        // Structural path defaults are non-empty: fill empties and rewrite when the key is missing.
        changed |= fill_string_default(
            &mut self.cliproxyapi.embeddings_path,
            &clip_default.embeddings_path,
            raw,
            &["cliproxyapi", "embeddings_path"],
        );
        changed |= fill_string_default(
            &mut self.cliproxyapi.chat_completions_path,
            &clip_default.chat_completions_path,
            raw,
            &["cliproxyapi", "chat_completions_path"],
        );
        changed |= fill_string_default(
            &mut self.cliproxyapi.models_path,
            &clip_default.models_path,
            raw,
            &["cliproxyapi", "models_path"],
        );
        let embedding_default = EmbeddingApiConfig::default();
        // Optional embedding fields may be empty in public defaults; only rewrite when we actually change a value.
        changed |= fill_optional_string_default(
            &mut self.embedding.model,
            &embedding_default.model,
            raw,
            &["embedding", "model"],
        );
        changed |= fill_optional_array_default(
            &mut self.embedding.fallback_models,
            &embedding_default.fallback_models,
            raw,
            &["embedding", "fallback_models"],
        );
        changed |= fill_optional_string_default(
            &mut self.embedding.base_url,
            &embedding_default.base_url,
            raw,
            &["embedding", "base_url"],
        );
        changed |= fill_optional_string_default(
            &mut self.embedding.api_key_env,
            &embedding_default.api_key_env,
            raw,
            &["embedding", "api_key_env"],
        );
        changed
    }

    pub fn chat_completions_url(&self) -> String {
        join_url(
            &self.cliproxyapi.base_url,
            &self.cliproxyapi.chat_completions_path,
        )
    }

    pub fn models_url(&self) -> String {
        join_url(&self.cliproxyapi.base_url, &self.cliproxyapi.models_path)
    }

    pub fn embeddings_url(&self) -> String {
        let base = if self.embedding.base_url.trim().is_empty() {
            &self.cliproxyapi.base_url
        } else {
            &self.embedding.base_url
        };
        let path = if self.cliproxyapi.embeddings_path.trim().is_empty() {
            "/v1/embeddings"
        } else {
            &self.cliproxyapi.embeddings_path
        };
        join_url(base, path)
    }

    pub fn embedding_api_key_env(&self) -> String {
        let explicit = self.embedding.api_key_env.trim();
        if explicit.is_empty() {
            self.api_key_env()
        } else {
            explicit.to_string()
        }
    }

    pub fn embedding_api_key(&self) -> String {
        let explicit = self.embedding.api_key.trim();
        if explicit.is_empty() {
            self.api_key()
        } else {
            explicit.to_string()
        }
    }

    pub fn asr_sidecar_url(&self) -> String {
        self.asr_sidecar
            .base_url
            .trim()
            .trim_end_matches('/')
            .to_string()
    }

    pub fn asr_api_key(&self) -> String {
        let explicit = self.asr_sidecar.api_key.trim();
        if explicit.is_empty() {
            self.api_key()
        } else {
            explicit.to_string()
        }
    }

    pub fn asr_api_key_env(&self) -> String {
        let explicit = self.asr_sidecar.api_key_env.trim();
        if explicit.is_empty() {
            self.api_key_env()
        } else {
            explicit.to_string()
        }
    }

    pub fn api_key(&self) -> String {
        self.cliproxyapi.api_key.trim().to_string()
    }

    pub fn api_key_env(&self) -> String {
        self.cliproxyapi.api_key_env.trim().to_string()
    }

    pub fn required_models(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut models = Vec::new();
        for model in self
            .setup_checks
            .required_models
            .iter()
            .chain(std::iter::once(&self.rewrite.model))
            .chain(self.rewrite.fallback_models.iter())
            .chain(std::iter::once(&self.embedding.model))
            .chain(self.embedding.fallback_models.iter())
            .chain(std::iter::once(&self.whisper.model))
        {
            let model = model.trim();
            if !model.is_empty() && seen.insert(model.to_ascii_lowercase()) {
                models.push(model.to_string());
            }
        }
        models
    }

    pub fn setup_checks_enabled(&self) -> bool {
        self.setup_checks.warn_if_missing_models
    }

    pub fn probe_setup_status(&self) -> ApiSetupStatus {
        let required_models = self.required_models();
        let models_url = self.models_url();
        let mut status = ApiSetupStatus {
            ok: false,
            models_url: models_url.clone(),
            required_models: required_models.clone(),
            available_model_count: 0,
            missing_models: required_models,
            error: None,
        };
        if !self.setup_checks.warn_if_missing_models {
            status.ok = true;
            status.missing_models.clear();
            return status;
        }
        let client = match Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                status.error = Some(format!("build models probe client: {error}"));
                return status;
            }
        };
        let mut request = client.get(&models_url);
        if let Some(api_key) = read_api_key(&self.api_key_env()).or_else(|| {
            let inline = self.api_key();
            (!inline.is_empty()).then_some(inline)
        }) {
            request = request.bearer_auth(api_key);
        }
        let response = match request
            .send()
            .and_then(|response| response.error_for_status())
        {
            Ok(response) => response,
            Err(error) => {
                status.error = Some(format!("call models endpoint: {error}"));
                return status;
            }
        };
        let body = match response.json::<ModelsResponse>() {
            Ok(body) => body,
            Err(error) => {
                status.error = Some(format!("decode models response: {error}"));
                return status;
            }
        };
        let available = body
            .data
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        let available_set = available
            .iter()
            .map(|model| model.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        status.available_model_count = available.len();
        status.missing_models = status
            .required_models
            .iter()
            .filter(|model| !available_set.contains(&model.to_ascii_lowercase()))
            .cloned()
            .collect();
        status.ok = status.missing_models.is_empty();
        status
    }
}

pub fn write_setup_status(state_root: &Path, status: &ApiSetupStatus) -> Result<()> {
    let path = state_root.join("logs").join("api-setup-status.json");
    let raw = serde_json::to_string_pretty(status).context("serialize API setup status")?;
    std::fs::write(&path, format!("{raw}\n"))
        .with_context(|| format!("write API setup status {}", path.display()))
}

pub fn setup_warning_message(status: &ApiSetupStatus) -> Option<String> {
    if status.ok {
        return None;
    }
    if !status.missing_models.is_empty() {
        return Some(format!(
            "OpenAI-compatible API missing models: {}. Edit api-connections.json and restart.",
            status.missing_models.join(", ")
        ));
    }
    status
        .error
        .as_ref()
        .map(|error| format!("cliproxyapi 模型检查失败：{error}"))
}

/// Join OpenAI-compatible base + path without producing `/v1/v1/...`.
pub fn join_url(base: &str, path: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let path = path.trim();
    if path.is_empty() {
        return base.to_string();
    }
    let path = path.trim_start_matches('/');
    // base=…/v1 + path=v1/models → …/v1/models
    if base.ends_with("/v1") && (path == "v1" || path.starts_with("v1/")) {
        let rest = path.trim_start_matches("v1").trim_start_matches('/');
        if rest.is_empty() {
            return base.to_string();
        }
        return format!("{base}/{rest}");
    }
    format!("{base}/{path}")
}

fn json_at_path<'a>(raw: Option<&'a Value>, path: &[&str]) -> Option<&'a Value> {
    let mut value = raw?;
    for key in path {
        value = value.get(*key)?;
    }
    Some(value)
}

fn json_string_missing_or_empty(raw: Option<&Value>, path: &[&str]) -> bool {
    match json_at_path(raw, path) {
        Some(Value::String(value)) => value.trim().is_empty(),
        Some(_) => false,
        None => true,
    }
}

fn json_array_missing_or_empty(raw: Option<&Value>, path: &[&str]) -> bool {
    match json_at_path(raw, path) {
        Some(Value::Array(values)) => values.is_empty(),
        Some(_) => false,
        None => true,
    }
}

/// Structural non-empty defaults: fill when the key is missing/empty, and report changed
/// even when writing the same non-empty default into a legacy incomplete file (key missing).
fn fill_string_default(
    target: &mut String,
    default: &str,
    raw: Option<&Value>,
    path: &[&str],
) -> bool {
    if !(json_string_missing_or_empty(raw, path) || target.trim().is_empty()) {
        return false;
    }
    let key_missing = json_at_path(raw, path).is_none();
    if target != default {
        *target = default.to_string();
        return true;
    }
    // Key absent from the file even though in-memory value already matches default → rewrite once.
    key_missing && !default.is_empty()
}

/// Optional fields may have empty public defaults. Only report changed when the value mutates.
fn fill_optional_string_default(
    target: &mut String,
    default: &str,
    raw: Option<&Value>,
    path: &[&str],
) -> bool {
    if !(json_string_missing_or_empty(raw, path) || target.trim().is_empty()) {
        return false;
    }
    if target == default {
        return false;
    }
    *target = default.to_string();
    true
}

fn fill_optional_array_default(
    target: &mut Vec<String>,
    default: &[String],
    raw: Option<&Value>,
    path: &[&str],
) -> bool {
    if !(json_array_missing_or_empty(raw, path) || target.is_empty()) {
        return false;
    }
    if target.as_slice() == default {
        return false;
    }
    *target = default.to_vec();
    true
}

fn read_api_key(primary_env: &str) -> Option<String> {
    for name in [
        primary_env,
        "AINPUT_API_KEY",
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

impl Default for ApiConnectionsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            cliproxyapi: CliproxyApiConfig::default(),
            rewrite: RewriteApiConfig::default(),
            embedding: EmbeddingApiConfig::default(),
            whisper: WhisperApiConfig::default(),
            asr_sidecar: AsrSidecarApiConfig::default(),
            setup_checks: SetupCheckConfig::default(),
        }
    }
}

impl Default for CliproxyApiConfig {
    fn default() -> Self {
        Self {
            // Public product preset: NVIDIA hosted OpenAI-compatible NIM API.
            base_url: "https://integrate.api.nvidia.com/v1".to_string(),
            api_key_env: "AINPUT_API_KEY".to_string(),
            api_key: String::new(),
            chat_completions_path: "/v1/chat/completions".to_string(),
            embeddings_path: "/v1/embeddings".to_string(),
            models_path: "/v1/models".to_string(),
        }
    }
}

impl Default for RewriteApiConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            fallback_models: Vec::new(),
            timeout_ms: default_rewrite_timeout_ms(),
        }
    }
}

/// Result of a lightweight connectivity probe against `/v1/models`.
#[derive(Debug, Clone)]
pub struct ConnectivityProbe {
    pub ok: bool,
    pub status: u16,
    pub latency_ms: u64,
    pub url: String,
    pub error: Option<String>,
}

/// Probe OpenAI-compatible endpoint: GET models path, report HTTP status + latency.
/// Does **not** require a 2xx to return; caller decides UX from `status`/`ok`.
pub fn probe_connectivity(
    base_url: &str,
    api_key: &str,
    models_path: &str,
    timeout_ms: u64,
) -> ConnectivityProbe {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return ConnectivityProbe {
            ok: false,
            status: 0,
            latency_ms: 0,
            url: String::new(),
            error: Some("Base URL 为空".to_string()),
        };
    }
    let path = if models_path.trim().is_empty() {
        "/v1/models"
    } else {
        models_path.trim()
    };
    let url = join_url(base, path);
    let timeout = Duration::from_millis(timeout_ms.clamp(500, 60_000));
    let client = match Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .no_proxy()
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ConnectivityProbe {
                ok: false,
                status: 0,
                latency_ms: 0,
                url,
                error: Some(format!("build client: {error}")),
            };
        }
    };
    let mut request = client.get(&url);
    let key = api_key.trim();
    if !key.is_empty() {
        request = request.bearer_auth(key);
    } else if let Some(env_key) = read_api_key("AINPUT_API_KEY") {
        request = request.bearer_auth(env_key);
    }
    let started = std::time::Instant::now();
    match request.send() {
        Ok(response) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            let status = response.status().as_u16();
            let ok = response.status().is_success();
            // Drain body so connection can close cleanly; ignore parse errors for probe.
            let _ = response.bytes();
            ConnectivityProbe {
                ok,
                status,
                latency_ms,
                url,
                error: if ok {
                    None
                } else {
                    Some(format!("HTTP {status}"))
                },
            }
        }
        Err(error) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            ConnectivityProbe {
                ok: false,
                status: 0,
                latency_ms,
                url,
                error: Some(format!("{error}")),
            }
        }
    }
}

/// List model ids from an OpenAI-compatible `/v1/models` endpoint.
pub fn list_models(
    base_url: &str,
    api_key: &str,
    models_path: &str,
    timeout_ms: u64,
) -> Result<Vec<String>> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        anyhow::bail!("Base URL 为空");
    }
    let path = if models_path.trim().is_empty() {
        "/v1/models"
    } else {
        models_path.trim()
    };
    let url = join_url(base, path);
    let timeout = Duration::from_millis(timeout_ms.clamp(500, 60_000));
    let client = Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .no_proxy()
        .build()
        .context("build models list client")?;
    let mut request = client.get(&url);
    let key = api_key.trim();
    if !key.is_empty() {
        request = request.bearer_auth(key);
    } else if let Some(env_key) = read_api_key("AINPUT_API_KEY") {
        request = request.bearer_auth(env_key);
    }
    let response = request
        .send()
        .and_then(|response| response.error_for_status())
        .with_context(|| format!("GET {url} 失败（超时 {timeout_ms} ms 或网络错误）"))?;
    let body = response
        .json::<ModelsResponse>()
        .context("解析 /v1/models 响应失败")?;
    let mut models = body
        .data
        .into_iter()
        .map(|entry| entry.id)
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    if models.is_empty() {
        anyhow::bail!("上游返回了空模型列表");
    }
    Ok(models)
}

impl Default for EmbeddingApiConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key_env: "AINPUT_API_KEY".to_string(),
            api_key: String::new(),
            model: String::new(),
            fallback_models: Vec::new(),
        }
    }
}

impl Default for WhisperApiConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
        }
    }
}

impl Default for AsrSidecarApiConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key_env: "AINPUT_API_KEY".to_string(),
            api_key: String::new(),
        }
    }
}

impl Default for SetupCheckConfig {
    fn default() -> Self {
        Self {
            warn_if_missing_models: false,
            required_models: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ApiConnectionsConfig;
    use serde_json::json;

    #[test]
    fn normalize_missing_defaults_marks_legacy_api_config_changed() {
        let raw = json!({
            "version": 1,
            "cliproxyapi": {
                "base_url": "http://example.test:8317",
                "api_key_env": "AINPUT_CLIPROXYAPI_8317_KEY",
                "api_key": "",
                "chat_completions_path": "/v1/chat/completions",
                "models_path": "/v1/models"
            },
            "rewrite": {
                "model": "openai/gpt-oss-120b",
                "fallback_models": ["qwen/qwen3.5-122b-a10b"]
            },
            "whisper": { "model": "openai/whisper-large-v3" },
            "asr_sidecar": { "base_url": "http://example.test:18765" },
            "setup_checks": {
                "warn_if_missing_models": false,
                "required_models": ["openai/gpt-oss-120b"]
            }
        });
        let mut config: ApiConnectionsConfig = serde_json::from_value(raw.clone()).unwrap();

        let changed = config.normalize_missing_defaults(Some(&raw));
        assert!(
            changed,
            "legacy config missing embeddings_path should rewrite defaults"
        );
        assert_eq!(config.cliproxyapi.embeddings_path, "/v1/embeddings");
        // Public defaults keep embedding optional/empty.
        assert_eq!(config.embedding.model, "");
        assert!(config.embedding.fallback_models.is_empty());
        assert_eq!(config.embedding.base_url, "");
        // embedding.api_key_env default is AINPUT_API_KEY when field absent from raw
        assert!(
            config.embedding.api_key_env == "AINPUT_API_KEY"
                || config.embedding.api_key_env.is_empty()
        );
    }

    #[test]
    fn normalize_missing_defaults_leaves_complete_api_config_clean() {
        let raw = serde_json::to_value(ApiConnectionsConfig::default()).unwrap();
        let mut config: ApiConnectionsConfig = serde_json::from_value(raw.clone()).unwrap();

        assert!(!config.normalize_missing_defaults(Some(&raw)));
    }

    #[test]
    fn join_url_avoids_double_v1() {
        use super::join_url;
        assert_eq!(
            join_url("https://integrate.api.nvidia.com/v1", "/v1/models"),
            "https://integrate.api.nvidia.com/v1/models"
        );
        assert_eq!(
            join_url("https://integrate.api.nvidia.com/v1", "v1/chat/completions"),
            "https://integrate.api.nvidia.com/v1/chat/completions"
        );
        assert_eq!(
            join_url("https://api.example.com", "/v1/models"),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            join_url("https://api.example.com/v1/", "models"),
            "https://api.example.com/v1/models"
        );
        assert_eq!(join_url("https://api.example.com/v1", ""), "https://api.example.com/v1");
        assert_eq!(join_url("https://api.example.com/v1", "v1"), "https://api.example.com/v1");
    }
}
