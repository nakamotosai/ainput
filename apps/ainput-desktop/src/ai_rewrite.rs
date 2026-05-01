use std::env;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ainput_output::{OutputContextKind, OutputContextSnapshot};
use ainput_shell::StreamingAiRewriteConfig;
use anyhow::{Context, Result, anyhow};
use reqwest::Url;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

const AI_REWRITE_FAILURE_BACKOFF: Duration = Duration::from_secs(5);
const LOCAL_OLLAMA_MIN_TIMEOUT_MS: u64 = 6500;

#[derive(Debug, Clone)]
pub(crate) struct AiRewriteRequest {
    pub frozen_prefix: String,
    pub current_tail: String,
    pub context: OutputContextSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct AiRewriteResponse {
    pub rewritten_tail: String,
}

pub(crate) struct AiRewriteClient {
    client: Client,
    endpoint_url: String,
    model: String,
    api_mode: AiRewriteApiMode,
    bearer_token: Option<String>,
    max_context_chars: usize,
    max_output_chars: usize,
    failure_backoff_until: Mutex<Option<Instant>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AiRewriteApiMode {
    OpenAiCompat,
    OllamaNative,
}

impl AiRewriteClient {
    pub(crate) fn from_config(config: &StreamingAiRewriteConfig) -> Result<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }

        let endpoint_url = config.endpoint_url.trim();
        if endpoint_url.is_empty() {
            return Err(anyhow!("voice.streaming.ai_rewrite.endpoint_url 不能为空"));
        }

        let model = config.model.trim();
        if model.is_empty() {
            return Err(anyhow!("voice.streaming.ai_rewrite.model 不能为空"));
        }

        let bearer_token = if config.api_key_env.trim().is_empty() {
            None
        } else {
            Some(env::var(config.api_key_env.trim()).with_context(|| {
                format!(
                    "missing environment variable {} for voice.streaming.ai_rewrite.api_key_env",
                    config.api_key_env.trim()
                )
            })?)
        };

        let (endpoint_url, api_mode) = normalize_endpoint_mode(endpoint_url)?;

        let timeout_ms = match api_mode {
            AiRewriteApiMode::OpenAiCompat => config.timeout_ms.max(50),
            AiRewriteApiMode::OllamaNative => config.timeout_ms.max(LOCAL_OLLAMA_MIN_TIMEOUT_MS),
        };

        let client = Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .no_proxy()
            .build()
            .context("build local AI rewrite http client")?;

        Ok(Some(Self {
            client,
            endpoint_url,
            model: model.to_string(),
            api_mode,
            bearer_token,
            max_context_chars: config.max_context_chars.max(1),
            max_output_chars: config.max_output_chars.max(8),
            failure_backoff_until: Mutex::new(None),
        }))
    }

    pub(crate) fn prewarm(&self) -> Result<()> {
        let request = AiRewriteRequest {
            frozen_prefix: String::new(),
            current_tail: "浏览七".to_string(),
            context: OutputContextSnapshot {
                process_name: Some("ainput-prewarm".to_string()),
                window_title: Some("ainput".to_string()),
                kind: OutputContextKind::EditableAtEnd,
                selected_text: None,
                text_before_cursor: None,
                text_after_cursor: None,
            },
        };

        let started_at = Instant::now();
        match self.rewrite_tail(request) {
            Ok(Some(response)) => {
                tracing::info!(
                    endpoint_url = %self.endpoint_url,
                    model = %self.model,
                    api_mode = self.api_mode.as_str(),
                    elapsed_ms = started_at.elapsed().as_millis(),
                    rewritten_tail = %short_log_text(&response.rewritten_tail, 80),
                    "streaming AI rewrite warmup completed"
                );
                Ok(())
            }
            Ok(None) => {
                tracing::info!(
                    endpoint_url = %self.endpoint_url,
                    model = %self.model,
                    api_mode = self.api_mode.as_str(),
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "streaming AI rewrite warmup completed with empty response"
                );
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn rewrite_tail(
        &self,
        request: AiRewriteRequest,
    ) -> Result<Option<AiRewriteResponse>> {
        if self.is_in_backoff()? {
            tracing::info!(
                endpoint_url = %self.endpoint_url,
                model = %self.model,
                api_mode = self.api_mode.as_str(),
                "streaming AI rewrite skipped because client is in backoff"
            );
            return Ok(None);
        }

        let system_prompt = build_system_prompt(self.max_output_chars);
        let user_prompt = build_user_prompt(&request, self.max_context_chars);
        tracing::info!(
            endpoint_url = %self.endpoint_url,
            model = %self.model,
            api_mode = self.api_mode.as_str(),
            frozen_prefix_chars = request.frozen_prefix.chars().count(),
            current_tail_chars = request.current_tail.chars().count(),
            frozen_prefix = %short_log_text(&request.frozen_prefix, 120),
            current_tail = %short_log_text(&request.current_tail, 120),
            "streaming AI rewrite request started"
        );

        let response = match self.api_mode {
            AiRewriteApiMode::OpenAiCompat => {
                let payload = ChatCompletionsRequest {
                    model: self.model.clone(),
                    messages: vec![
                        ChatMessage {
                            role: "system".to_string(),
                            content: system_prompt.clone(),
                        },
                        ChatMessage {
                            role: "user".to_string(),
                            content: user_prompt.clone(),
                        },
                    ],
                    temperature: 0.2,
                    top_p: 0.9,
                    max_tokens: Some(self.max_output_chars.clamp(24, 192) as u32),
                    reasoning_effort: openai_reasoning_effort(&self.model),
                    stream: false,
                };

                let mut http = self.client.post(&self.endpoint_url).json(&payload);
                if let Some(token) = &self.bearer_token {
                    http = http.bearer_auth(token);
                }
                http.send()
            }
            AiRewriteApiMode::OllamaNative => {
                let payload = OllamaChatRequest {
                    model: self.model.clone(),
                    messages: vec![
                        ChatMessage {
                            role: "system".to_string(),
                            content: system_prompt,
                        },
                        ChatMessage {
                            role: "user".to_string(),
                            content: user_prompt,
                        },
                    ],
                    stream: false,
                    think: false,
                    options: OllamaChatOptions {
                        temperature: 0.1,
                        top_p: 0.9,
                        num_predict: self.max_output_chars.clamp(16, 128) as u32,
                    },
                };
                self.client.post(&self.endpoint_url).json(&payload).send()
            }
        };

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                self.arm_backoff()?;
                let error_message = format!("{error:#}");
                tracing::warn!(
                    endpoint_url = %self.endpoint_url,
                    model = %self.model,
                    api_mode = self.api_mode.as_str(),
                    error = %error_message,
                    "streaming AI rewrite request failed before response"
                );
                return Err(anyhow!("call local AI rewrite server: {error_message}"));
            }
        };

        match self.api_mode {
            AiRewriteApiMode::OpenAiCompat => self.handle_openai_response(response),
            AiRewriteApiMode::OllamaNative => self.handle_ollama_response(response),
        }
    }

    fn handle_openai_response(
        &self,
        response: reqwest::blocking::Response,
    ) -> Result<Option<AiRewriteResponse>> {
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            self.arm_backoff()?;
            tracing::warn!(
                endpoint_url = %self.endpoint_url,
                model = %self.model,
                api_mode = self.api_mode.as_str(),
                status = %status,
                body = %short_log_text(body.trim(), 200),
                "streaming AI rewrite server returned non-success status"
            );
            return Err(anyhow!(
                "local AI rewrite server returned {status}: {}",
                body.trim()
            ));
        }

        let payload: ChatCompletionsResponse = match response.json() {
            Ok(payload) => payload,
            Err(error) => {
                self.arm_backoff()?;
                let error_message = format!("{error:#}");
                tracing::warn!(
                    endpoint_url = %self.endpoint_url,
                    model = %self.model,
                    api_mode = self.api_mode.as_str(),
                    error = %error_message,
                    "streaming AI rewrite response decode failed"
                );
                return Err(anyhow!("decode local AI rewrite response: {error_message}"));
            }
        };

        self.clear_backoff()?;

        let rewritten_tail = payload
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .map(|content| normalize_model_output(&content))
            .unwrap_or_default();
        if rewritten_tail.is_empty() {
            tracing::info!(
                endpoint_url = %self.endpoint_url,
                model = %self.model,
                api_mode = self.api_mode.as_str(),
                "streaming AI rewrite returned empty content"
            );
            return Ok(None);
        }

        tracing::info!(
            endpoint_url = %self.endpoint_url,
            model = %self.model,
            api_mode = self.api_mode.as_str(),
            rewritten_tail = %short_log_text(&rewritten_tail, 120),
            rewritten_tail_chars = rewritten_tail.chars().count(),
            "streaming AI rewrite response accepted"
        );
        Ok(Some(AiRewriteResponse { rewritten_tail }))
    }

    fn handle_ollama_response(
        &self,
        response: reqwest::blocking::Response,
    ) -> Result<Option<AiRewriteResponse>> {
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            self.arm_backoff()?;
            tracing::warn!(
                endpoint_url = %self.endpoint_url,
                model = %self.model,
                api_mode = self.api_mode.as_str(),
                status = %status,
                body = %short_log_text(body.trim(), 200),
                "streaming AI rewrite server returned non-success status"
            );
            return Err(anyhow!(
                "local AI rewrite server returned {status}: {}",
                body.trim()
            ));
        }

        let payload: OllamaChatResponse = match response.json() {
            Ok(payload) => payload,
            Err(error) => {
                self.arm_backoff()?;
                let error_message = format!("{error:#}");
                tracing::warn!(
                    endpoint_url = %self.endpoint_url,
                    model = %self.model,
                    api_mode = self.api_mode.as_str(),
                    error = %error_message,
                    "streaming AI rewrite response decode failed"
                );
                return Err(anyhow!("decode local AI rewrite response: {error_message}"));
            }
        };

        self.clear_backoff()?;

        let rewritten_tail = payload
            .message
            .map(|message| normalize_model_output(&message.content))
            .unwrap_or_default();
        if rewritten_tail.is_empty() {
            tracing::info!(
                endpoint_url = %self.endpoint_url,
                model = %self.model,
                api_mode = self.api_mode.as_str(),
                done_reason = payload.done_reason.unwrap_or_default(),
                "streaming AI rewrite returned empty content"
            );
            return Ok(None);
        }

        tracing::info!(
            endpoint_url = %self.endpoint_url,
            model = %self.model,
            api_mode = self.api_mode.as_str(),
            rewritten_tail = %short_log_text(&rewritten_tail, 120),
            rewritten_tail_chars = rewritten_tail.chars().count(),
            done_reason = payload.done_reason.unwrap_or_default(),
            "streaming AI rewrite response accepted"
        );
        Ok(Some(AiRewriteResponse { rewritten_tail }))
    }

    fn is_in_backoff(&self) -> Result<bool> {
        let guard = self
            .failure_backoff_until
            .lock()
            .map_err(|_| anyhow!("ai rewrite backoff lock poisoned"))?;
        Ok(guard.is_some_and(|deadline| Instant::now() < deadline))
    }

    fn arm_backoff(&self) -> Result<()> {
        let mut guard = self
            .failure_backoff_until
            .lock()
            .map_err(|_| anyhow!("ai rewrite backoff lock poisoned"))?;
        *guard = Some(Instant::now() + AI_REWRITE_FAILURE_BACKOFF);
        Ok(())
    }

    fn clear_backoff(&self) -> Result<()> {
        let mut guard = self
            .failure_backoff_until
            .lock()
            .map_err(|_| anyhow!("ai rewrite backoff lock poisoned"))?;
        *guard = None;
        Ok(())
    }
}

impl AiRewriteApiMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompat => "openai_compat",
            Self::OllamaNative => "ollama_native",
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatCompletionsRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    top_p: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionsResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, serde::Deserialize)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    think: bool,
    options: OllamaChatOptions,
}

#[derive(Debug, Serialize)]
struct OllamaChatOptions {
    temperature: f32,
    top_p: f32,
    num_predict: u32,
}

#[derive(Debug, serde::Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaChatMessage>,
    done_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OllamaChatMessage {
    content: String,
}

fn build_system_prompt(max_output_chars: usize) -> String {
    format!(
        "你是中文语音输入法 HUD 的实时 AI 改写器。你的输入来自语音识别，当前尾巴里经常有同音错字、近音错字、漏字、专有名词误识别和中英混说误识别。\n你的任务是输出用户最可能真正想输入的最终尾巴。\n硬规则：\n1. 只改写“当前尾巴”，绝不能重复“冻结前缀”。\n2. 优先修正语音识别错词，而不是机械照抄原字面。\n3. 可以按语义修正常见同音错字、近音错字、专有名词，例如：浏览七->浏览器，已识别->已实现，口待克斯->Codex。\n4. 保留用户真实说出的中英文混合、产品名、代码名和 API 名称。\n5. 不确定时返回原文，不要凭空扩写。\n6. 只输出 JSON，不要解释，不要 Markdown，不要代码块。\n7. JSON 格式必须是 {{\"tail\":\"改写后的当前尾巴\"}}。\n8. tail 长度不要超过 {} 个可见字符。",
        max_output_chars
    )
}

fn build_user_prompt(request: &AiRewriteRequest, max_context_chars: usize) -> String {
    let process_name = request.context.process_name.as_deref().unwrap_or("unknown");
    let window_title = request.context.window_title.as_deref().unwrap_or("unknown");
    let selected_text =
        context_text_or_empty(request.context.selected_text.as_deref(), max_context_chars);
    let text_before_cursor = context_text_or_empty(
        request.context.text_before_cursor.as_deref(),
        max_context_chars,
    );
    let text_after_cursor = context_text_or_empty(
        request.context.text_after_cursor.as_deref(),
        max_context_chars,
    );
    let frozen_prefix = if request.frozen_prefix.trim().is_empty() {
        "(空)".to_string()
    } else {
        take_last_chars(request.frozen_prefix.trim(), max_context_chars)
    };

    format!(
        "下面是一次实时语音输入改写请求。\n请结合应用场景、窗口标题、光标附近文本和冻结前文，纠正“当前尾巴”里的语音识别错误。\n当前应用: {}\n窗口标题: {}\n光标环境: {}\n选中文本: {}\n光标前文本: {}\n光标后文本: {}\n冻结前缀(只做参考，绝不能输出):\n{}\n当前尾巴(只改这里):\n{}\n\n请只返回 JSON：{{\"tail\":\"纠正后的当前尾巴\"}}。",
        process_name,
        window_title,
        describe_output_context_kind(request.context.kind),
        selected_text,
        text_before_cursor,
        text_after_cursor,
        frozen_prefix,
        request.current_tail.trim()
    )
}

fn context_text_or_empty(value: Option<&str>, max_context_chars: usize) -> String {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| take_last_chars(text, max_context_chars))
        .unwrap_or_else(|| "(空)".to_string())
}

fn describe_output_context_kind(kind: OutputContextKind) -> &'static str {
    match kind {
        OutputContextKind::EditableWithContentOnRight => "可编辑，但光标右侧仍有内容",
        OutputContextKind::EditableAtEnd => "可编辑，光标在末尾",
        OutputContextKind::Unknown => "未知",
    }
}

fn normalize_model_output(raw: &str) -> String {
    let mut cleaned = raw.trim();
    if cleaned.starts_with("```") && cleaned.ends_with("```") {
        cleaned = cleaned.trim_matches('`').trim();
        if let Some(stripped) = cleaned.strip_prefix("json") {
            cleaned = stripped.trim();
        }
    }
    if let Some(parsed) = parse_json_tail(cleaned) {
        return parsed;
    }
    let cleaned = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .last()
        .unwrap_or(cleaned);
    if let Some(parsed) = parse_json_tail(cleaned) {
        return parsed;
    }
    cleaned
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '“' | '”' | '‘' | '’'))
        .trim()
        .to_string()
}

#[derive(Debug, Deserialize)]
struct TailRewriteJson {
    #[serde(default)]
    tail: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

fn parse_json_tail(raw: &str) -> Option<String> {
    let parsed = serde_json::from_str::<TailRewriteJson>(raw).ok()?;
    parsed
        .tail
        .or(parsed.text)
        .map(|text| {
            text.trim()
                .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '“' | '”' | '‘' | '’'))
                .trim()
                .to_string()
        })
        .filter(|text| !text.is_empty())
}

fn take_last_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }

    text.chars().skip(char_count - max_chars).collect()
}

fn openai_reasoning_effort(model: &str) -> Option<String> {
    if model.contains("gpt-oss") {
        return Some("low".to_string());
    }

    None
}

fn normalize_endpoint_mode(endpoint_url: &str) -> Result<(String, AiRewriteApiMode)> {
    let url = Url::parse(endpoint_url).with_context(|| {
        format!("parse voice.streaming.ai_rewrite.endpoint_url: {endpoint_url}")
    })?;
    let host = url.host_str().unwrap_or_default();
    let is_local_ollama =
        matches!(host, "127.0.0.1" | "localhost") && url.port_or_known_default() == Some(11434);

    if is_local_ollama {
        let mut native = url;
        native.set_path("/api/chat");
        native.set_query(None);
        return Ok((native.to_string(), AiRewriteApiMode::OllamaNative));
    }

    Ok((endpoint_url.to_string(), AiRewriteApiMode::OpenAiCompat))
}

fn short_log_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut shortened = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars {
        shortened.push_str("...");
    }
    shortened
}

#[cfg(test)]
mod tests {
    use super::{
        AiRewriteRequest, OutputContextKind, OutputContextSnapshot, build_user_prompt,
        normalize_model_output, openai_reasoning_effort, take_last_chars,
    };

    #[test]
    fn take_last_chars_keeps_suffix() {
        assert_eq!(
            take_last_chars("第一句已经稳定。第二句继续改写", 7),
            "第二句继续改写"
        );
        assert_eq!(take_last_chars("短句", 6), "短句");
    }

    #[test]
    fn build_user_prompt_contains_context() {
        let prompt = build_user_prompt(
            &AiRewriteRequest {
                frozen_prefix: "第一句已经稳定。".to_string(),
                current_tail: "第二句先是错字".to_string(),
                context: OutputContextSnapshot {
                    process_name: Some("Code.exe".to_string()),
                    window_title: Some("main.rs - Visual Studio Code".to_string()),
                    kind: OutputContextKind::EditableAtEnd,
                    selected_text: Some("旧文本".to_string()),
                    text_before_cursor: Some("fn main() {".to_string()),
                    text_after_cursor: Some("}".to_string()),
                },
            },
            12,
        );

        assert!(prompt.contains("当前应用: Code.exe"));
        assert!(prompt.contains("窗口标题: main.rs - Visual Studio Code"));
        assert!(prompt.contains("光标环境: 可编辑，光标在末尾"));
        assert!(prompt.contains("选中文本: 旧文本"));
        assert!(prompt.contains("第一句已经稳定。"));
        assert!(prompt.contains("第二句先是错字"));
        assert!(prompt.contains("{\"tail\":\"纠正后的当前尾巴\"}"));
    }

    #[test]
    fn normalize_model_output_prefers_json_tail() {
        assert_eq!(
            normalize_model_output(r#"{"tail":"我觉得这个功能不太对"}"#),
            "我觉得这个功能不太对"
        );
        assert_eq!(
            normalize_model_output("```json\n{\"text\":\"这个 HUD 应该逐渐改写\"}\n```"),
            "这个 HUD 应该逐渐改写"
        );
    }

    #[test]
    fn gpt_oss_requests_low_reasoning_effort() {
        assert_eq!(
            openai_reasoning_effort("openai/gpt-oss-20b"),
            Some("low".to_string())
        );
        assert_eq!(openai_reasoning_effort("qwen/qwen3"), None);
    }
}
