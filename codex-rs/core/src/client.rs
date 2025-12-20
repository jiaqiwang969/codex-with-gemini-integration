use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use crate::api_bridge::auth_provider_from_auth;
use crate::api_bridge::map_api_error;
use bytes::Bytes;
use codex_api::AggregateStreamExt;
use codex_api::ChatClient as ApiChatClient;
use codex_api::CompactClient as ApiCompactClient;
use codex_api::CompactionInput as ApiCompactionInput;
use codex_api::Prompt as ApiPrompt;
use codex_api::RequestTelemetry;
use codex_api::ReqwestTransport;
use codex_api::ResponseStream as ApiResponseStream;
use codex_api::ResponsesClient as ApiResponsesClient;
use codex_api::ResponsesOptions as ApiResponsesOptions;
use codex_api::SseTelemetry;
use codex_api::TransportError;
use codex_api::common::Reasoning;
use codex_api::create_text_param_for_request;
use codex_api::error::ApiError;
use codex_app_server_protocol::AuthMode;
use codex_otel::otel_manager::OtelManager;
use codex_protocol::ConversationId;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::SessionSource;
use eventsource_stream::Event;
use eventsource_stream::EventStreamError;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use futures::TryStreamExt;
use http::HeaderMap as ApiHeaderMap;
use http::HeaderValue;
use http::StatusCode as HttpStatusCode;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::debug;
use tracing::warn;

use crate::AuthManager;
use crate::auth::RefreshTokenError;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::client_common::ResponseStream;
use crate::client_common::tools::ResponsesApiTool;
use crate::client_common::tools::ToolSpec;
use crate::config::Config;
use crate::default_client::build_reqwest_client;
use crate::error::CodexErr;
use crate::error::ResponseStreamFailed;
use crate::error::Result;
use crate::error::UnexpectedResponseError;
use crate::features::FEATURES;
use crate::flags::CODEX_RS_SSE_FIXTURE;
use crate::model_provider_info::ModelProviderInfo;
use crate::model_provider_info::WireApi;
use crate::openai_models::model_family::ModelFamily;
use crate::protocol::TokenUsage;
use crate::tools::spec::create_tools_json_for_chat_completions_api;
use crate::tools::spec::create_tools_json_for_responses_api;

static GEMINI_CALL_ID_COUNTER: AtomicI64 = AtomicI64::new(0);

fn next_gemini_call_id() -> String {
    let id = GEMINI_CALL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("gemini-function-call-{id}")
}

const GEMINI_READ_ONLY_TOOL_NAMES: [&str; 9] = [
    "grep_files",
    "list_dir",
    "read_file",
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
    "view_image",
    "shell",
    "shell_command",
];
const DEFAULT_GEMINI_THINKING_BUDGET: i32 = 8192;

fn parse_bool_env(key: &str) -> Option<bool> {
    std::env::var(key).ok().and_then(|value| {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    })
}

/// Checks if the given text is meaningful for display as reasoning content.
/// Filters out garbage data like repeated characters (e.g., "000000...") that
/// Gemini sometimes outputs when processing images.
fn is_meaningful_thought_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    // If text is very long (>100 chars) and consists mostly of the same character,
    // it's likely garbage data from image processing
    if trimmed.len() > 100 {
        let first_char = trimmed.chars().next().unwrap();
        let same_char_count = trimmed.chars().filter(|&c| c == first_char).count();
        let ratio = same_char_count as f64 / trimmed.len() as f64;
        if ratio > 0.9 {
            return false;
        }
    }

    // Check if text is just repeated digits (common garbage pattern)
    if trimmed.len() > 50 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    true
}

fn last_user_message_text(input: &[ResponseItem]) -> Option<String> {
    let Some(ResponseItem::Message { role, content, .. }) = input.last() else {
        return None;
    };
    if role != "user" {
        return None;
    }

    let mut out = String::new();
    for item in content {
        let ContentItem::InputText { text } = item else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(text);
    }
    (!out.is_empty()).then_some(out)
}

fn should_force_gemini_read_tools_first_turn_with_override(
    input: &[ResponseItem],
    force_override: Option<bool>,
) -> bool {
    if last_user_message_text(input).is_none() {
        return false;
    };

    match force_override {
        Some(true) => true,
        Some(false) => false,
        None => true,
    }
}

fn gemini_read_only_allowed_function_names(tools: &[ToolSpec]) -> Vec<String> {
    let mut allowed = Vec::new();
    for name in GEMINI_READ_ONLY_TOOL_NAMES {
        if tools
            .iter()
            .any(|tool| matches!(tool, ToolSpec::Function(tool) if tool.name == name))
        {
            allowed.push(name.to_string());
        }
    }
    allowed
}

fn build_gemini_tool_config_with_override(
    tools: &[ToolSpec],
    input: &[ResponseItem],
    force_override: Option<bool>,
    api_model: &str,
) -> GeminiFunctionCallingConfig {
    let mut function_calling_config = GeminiFunctionCallingConfig {
        mode: GeminiFunctionCallingMode::Auto,
        allowed_function_names: None,
        // Enable streaming function call arguments for Gemini 3 models.
        // This reduces perceived latency when the model calls functions.
        stream_function_call_arguments: is_gemini_3_model(api_model).then_some(true),
    };

    if should_force_gemini_read_tools_first_turn_with_override(input, force_override) {
        let allowed_function_names = gemini_read_only_allowed_function_names(tools);
        if !allowed_function_names.is_empty() {
            function_calling_config.mode = GeminiFunctionCallingMode::Any;
            function_calling_config.allowed_function_names = Some(allowed_function_names);
        }
    }

    function_calling_config
}

fn build_gemini_tool_config(
    tools: &[ToolSpec],
    input: &[ResponseItem],
    api_model: &str,
) -> GeminiFunctionCallingConfig {
    build_gemini_tool_config_with_override(
        tools,
        input,
        parse_bool_env("CODEX_GEMINI_FORCE_READ_TOOLS_FIRST_TURN"),
        api_model,
    )
}

#[derive(Debug, Clone)]
pub struct ModelClient {
    config: Arc<Config>,
    auth_manager: Option<Arc<AuthManager>>,
    model_family: ModelFamily,
    otel_manager: OtelManager,
    provider: ModelProviderInfo,
    conversation_id: ConversationId,
    effort: Option<ReasoningEffortConfig>,
    summary: ReasoningSummaryConfig,
    session_source: SessionSource,
}

#[allow(clippy::too_many_arguments)]
impl ModelClient {
    pub fn new(
        config: Arc<Config>,
        auth_manager: Option<Arc<AuthManager>>,
        model_family: ModelFamily,
        otel_manager: OtelManager,
        provider: ModelProviderInfo,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        conversation_id: ConversationId,
        session_source: SessionSource,
    ) -> Self {
        Self {
            config,
            auth_manager,
            model_family,
            otel_manager,
            provider,
            conversation_id,
            effort,
            summary,
            session_source,
        }
    }

    pub fn get_model_context_window(&self) -> Option<i64> {
        let model_family = self.get_model_family();
        let effective_context_window_percent = model_family.effective_context_window_percent;
        model_family
            .context_window
            .map(|w| w.saturating_mul(effective_context_window_percent) / 100)
    }

    pub fn config(&self) -> Arc<Config> {
        Arc::clone(&self.config)
    }

    pub fn provider(&self) -> &ModelProviderInfo {
        &self.provider
    }

    /// Streams a single model turn using either the Responses or Chat
    /// Completions wire API, depending on the configured provider.
    ///
    /// For Chat providers, the underlying stream is optionally aggregated
    /// based on the `show_raw_agent_reasoning` flag in the config.
    pub async fn stream(&self, prompt: &Prompt) -> Result<ResponseStream> {
        match self.provider.wire_api {
            WireApi::Responses => self.stream_responses_api(prompt).await,
            WireApi::Chat => {
                let api_stream = self.stream_chat_completions(prompt).await?;

                if self.config.show_raw_agent_reasoning {
                    Ok(map_response_stream(
                        api_stream.streaming_mode(),
                        self.otel_manager.clone(),
                    ))
                } else {
                    Ok(map_response_stream(
                        api_stream.aggregate(),
                        self.otel_manager.clone(),
                    ))
                }
            }
            WireApi::Gemini => self.stream_gemini(prompt).await,
        }
    }

    /// Streams a turn via the OpenAI Chat Completions API.
    ///
    /// This path is only used when the provider is configured with
    /// `WireApi::Chat`; it does not support `output_schema` today.
    async fn stream_chat_completions(&self, prompt: &Prompt) -> Result<ApiResponseStream> {
        if prompt.output_schema.is_some() {
            return Err(CodexErr::UnsupportedOperation(
                "output_schema is not supported for Chat Completions API".to_string(),
            ));
        }

        let auth_manager = self.auth_manager.clone();
        let model_family = self.get_model_family();
        let instructions = prompt.get_full_instructions(&model_family).into_owned();
        let tools_json = create_tools_json_for_chat_completions_api(&prompt.tools)?;
        let api_prompt = build_api_prompt(prompt, instructions, tools_json);
        let conversation_id = self.conversation_id.to_string();
        let session_source = self.session_source.clone();

        let mut refreshed = false;
        loop {
            let auth = auth_manager.as_ref().and_then(|m| m.auth());
            let api_provider = self
                .provider
                .to_api_provider(auth.as_ref().map(|a| a.mode))?;
            let api_auth = auth_provider_from_auth(auth.clone(), &self.provider).await?;
            let transport = ReqwestTransport::new(build_reqwest_client());
            let (request_telemetry, sse_telemetry) = self.build_streaming_telemetry();
            let client = ApiChatClient::new(transport, api_provider, api_auth)
                .with_telemetry(Some(request_telemetry), Some(sse_telemetry));

            let stream_result = client
                .stream_prompt(
                    &self.get_model(),
                    &api_prompt,
                    Some(conversation_id.clone()),
                    Some(session_source.clone()),
                )
                .await;

            match stream_result {
                Ok(stream) => return Ok(stream),
                Err(ApiError::Transport(TransportError::Http { status, .. }))
                    if status == StatusCode::UNAUTHORIZED =>
                {
                    handle_unauthorized(status, &mut refreshed, &auth_manager, &auth).await?;
                    continue;
                }
                Err(err) => return Err(map_api_error(err)),
            }
        }
    }

    async fn stream_gemini(&self, prompt: &Prompt) -> Result<ResponseStream> {
        let base_url = self.provider.base_url.as_ref().ok_or_else(|| {
            CodexErr::UnsupportedOperation("Gemini providers must define a base_url".to_string())
        })?;
        let base_url = Self::normalize_gemini_base_url(base_url);

        let model = self.get_model();
        let api_model = model.strip_suffix("-codex").unwrap_or(&model);
        let api_model = api_model.strip_suffix("-germini").unwrap_or(api_model);
        let api_model = api_model.strip_suffix("-gemini").unwrap_or(api_model);

        // Use streamGenerateContent endpoint with alt=sse for streaming
        let url = format!(
            "{}/models/{api_model}:streamGenerateContent?alt=sse",
            base_url.as_ref().trim_end_matches('/'),
        );

        let model_family = self.get_model_family();
        let instructions = prompt.get_full_instructions(&model_family).into_owned();
        let formatted_input = prompt.get_formatted_input();
        let contents = build_gemini_contents(&formatted_input, &prompt.reference_images, api_model);
        if contents.is_empty() {
            return Err(CodexErr::UnsupportedOperation(
                "Gemini requests require at least one message".to_string(),
            ));
        }

        let system_instruction = (!instructions.trim().is_empty()).then(|| GeminiContentRequest {
            role: None,
            parts: vec![GeminiPartRequest {
                text: Some(instructions),
                inline_data: None,
                function_call: None,
                function_response: None,
                thought_signature: None,
                compat_thought_signature: None,
            }],
        });

        let tools = build_gemini_tools(&prompt.tools);
        let tool_config = tools.as_ref().map(|_| GeminiToolConfig {
            function_calling_config: build_gemini_tool_config(&prompt.tools, &formatted_input, api_model),
        });

        // Ensure the active loop has thought signatures on function calls so
        // preview models accept the request without 400/429 errors.
        let contents = ensure_active_loop_has_thought_signatures(&contents);

        let reasoning_effort = self.effort.or(model_family.default_reasoning_effort);
        let thinking_config = Self::build_gemini_thinking_config(api_model, reasoning_effort);

        // Build generationConfig with thinkingConfig nested properly.
        // Per Gemini 3 documentation: "We strongly recommend keeping the
        // temperature parameter at its default value of 1.0. Lowering it
        // may cause looping or degraded performance on reasoning tasks."
        // Gemini now enforces that only one of `thinkingLevel` or
        // `thinkingBudget` may be set. We pick the level for thinking
        // variants (to request high-quality thoughts) and budget for
        // non-thinking text models (to keep longer tool loops), while
        // omitting the field entirely for image models that reject it.
        let generation_config = Some(GeminiGenerationConfig {
            temperature: Some(1.0), // Gemini 3 recommended default
            top_k: Some(64),
            top_p: Some(0.95),
            max_output_tokens: None, // Let the model decide
            thinking_config,
            // TODO: Consider allowing user to specify media_resolution via MCP mechanism
            // when they mention specific quality requirements (e.g., "high quality image analysis").
            // Valid options: media_resolution_low (280 tokens), media_resolution_medium (560),
            // media_resolution_high (1120), media_resolution_ultra_high (2240, per-part only).
            media_resolution: None, // Let Gemini auto-select based on media type
        });

        // Default safety settings to allow code-related content
        let safety_settings = Some(vec![
            GeminiSafetySetting {
                category: GeminiHarmCategory::HarmCategoryHarassment,
                threshold: GeminiHarmBlockThreshold::BlockOnlyHigh,
            },
            GeminiSafetySetting {
                category: GeminiHarmCategory::HarmCategoryHateSpeech,
                threshold: GeminiHarmBlockThreshold::BlockOnlyHigh,
            },
            GeminiSafetySetting {
                category: GeminiHarmCategory::HarmCategorySexuallyExplicit,
                threshold: GeminiHarmBlockThreshold::BlockOnlyHigh,
            },
            GeminiSafetySetting {
                category: GeminiHarmCategory::HarmCategoryDangerousContent,
                threshold: GeminiHarmBlockThreshold::BlockOnlyHigh,
            },
        ]);

        let request = GeminiRequest {
            system_instruction,
            contents,
            tools,
            tool_config,
            generation_config,
            safety_settings,
        };

        // Optional debug hook to inspect the exact Gemini request payload.
        if std::env::var("CODEX_DEBUG_GEMINI_REQUEST").is_ok()
            && let Ok(json) = serde_json::to_string_pretty(&request)
        {
            debug!("DEBUG GEMINI REQUEST:\n{json}");
        }

        // Build request with Gemini-specific auth handling
        let client = build_reqwest_client();

        // Prefer GEMINI_API_KEY from the environment, then fall back to auth.json.
        // This matches the documented behaviour where a dedicated Gemini key
        // in the env takes precedence over the shared key stored in auth.json.
        let gemini_api_key = crate::auth::read_gemini_api_key_from_env().or_else(|| {
            crate::auth::read_gemini_api_key_from_auth_json(
                &self.config.codex_home,
                self.config.cli_auth_credentials_store_mode,
            )
        });

        let make_request_builder = || {
            let mut req_builder = client.post(&url);
            // Always apply provider-level headers so env_http_headers like
            // GEMINI_COOKIE are respected even when we inject the API key
            // directly below.
            req_builder = self.provider.apply_http_headers(req_builder);
            if let Some(api_key) = gemini_api_key.as_deref() {
                // Override any existing X-Goog-Api-Key header so we can prefer
                // a dedicated Gemini key or the shared OPENAI_API_KEY from
                // auth.json when present.
                req_builder = req_builder.header("x-goog-api-key", api_key);
            }
            req_builder
        };

        // Retry configuration: max 3 attempts with exponential backoff
        const MAX_ATTEMPTS: u64 = 3;
        const INITIAL_DELAY_MS: u64 = 5000;
        const MAX_DELAY_MS: u64 = 30000;

        let mut attempt: u64 = 0;
        let mut current_delay = INITIAL_DELAY_MS;

        let response = loop {
            attempt += 1;

            let result = self
                .otel_manager
                .log_request(attempt, || make_request_builder().json(&request).send())
                .await;

            match result {
                Ok(resp) => break resp,
                Err(err) => {
                    // Check if we should retry
                    let should_retry = if let Some(status) = err.status() {
                        // Retry on 429 (Too Many Requests) or 5xx server errors
                        status == StatusCode::TOO_MANY_REQUESTS
                            || (status.as_u16() >= 500 && status.as_u16() < 600)
                    } else {
                        // Network errors - retry
                        err.is_connect() || err.is_timeout()
                    };

                    if should_retry && attempt < MAX_ATTEMPTS {
                        // Exponential backoff with jitter
                        let jitter =
                            (current_delay as f64 * 0.3 * (rand::random::<f64>() * 2.0 - 1.0))
                                as u64;
                        let delay_with_jitter = current_delay.saturating_add(jitter);
                        debug!(
                            "Gemini request attempt {} failed, retrying after {}ms: {}",
                            attempt, delay_with_jitter, err
                        );
                        tokio::time::sleep(Duration::from_millis(delay_with_jitter)).await;
                        current_delay = std::cmp::min(MAX_DELAY_MS, current_delay * 2);
                        continue;
                    }

                    return Err(CodexErr::ResponseStreamFailed(ResponseStreamFailed {
                        source: err,
                        request_id: None,
                    }));
                }
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            // Gemini preview models may reject tool calls when they believe a
            // function call is missing a thought_signature. When this happens,
            // degrade gracefully by surfacing a plain assistant message rather
            // than hard‑failing the turn. The upstream proxy may return either
            // 400 (Bad Request) or 429 (Too Many Requests) depending on the
            // validation layer that catches the issue.
            if (status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::BAD_REQUEST)
                && body.contains("missing a `thought_signature`")
            {
                let mut message =
                    "Gemini backend rejected this tool call because it expects a thought_signature \
on shell_command. This Codex build already attempted to provide one, but the upstream \
proxy still returned a validation error.\n\n\
As a workaround, please run shell commands using the `codex` profile \
instead (for example: `codex -p codex`), or execute the command manually in your terminal."
                        .to_string();

                // Include a trimmed copy of the original error for debugging.
                if !body.trim().is_empty() {
                    message.push_str("\n\nUpstream error:\n");
                    let snippet = body.chars().take(2000).collect::<String>();
                    message.push_str(&snippet);
                }

                let item = ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText { text: message }],
                    thought_signature: None,
                };

                return Ok(spawn_gemini_response_stream(
                    Some(item),
                    "gemini-error-thought-signature".to_string(),
                    None,
                ));
            }

            return Err(CodexErr::UnexpectedStatus(UnexpectedResponseError {
                status,
                body,
                request_id: None,
            }));
        }

        // Stream the SSE response
        let idle_timeout = self.provider.stream_idle_timeout();
        let byte_stream = response.bytes_stream();

        Ok(spawn_gemini_sse_stream(byte_stream, idle_timeout))
    }

    fn build_gemini_thinking_config(
        api_model: &str,
        reasoning_effort: Option<ReasoningEffortConfig>,
    ) -> Option<GeminiThinkingConfig> {
        if api_model.contains("image") {
            return None;
        }

        if is_gemini_3_model(api_model) {
            // For Gemini 3 models, use only thinkingLevel (not thinkingBudget).
            // Per Gemini docs: thinkingLevel is the recommended approach for Gemini 3.
            // thinkingBudget is for Gemini 2.5 series only.
            //
            // Gemini 3 Flash supports additional levels: minimal, low, medium, high
            // Gemini 3 Pro supports: low, medium, high
            // Default to "high" for best quality, but respect user's reasoning effort setting.
            let thinking_level = match reasoning_effort {
                Some(ReasoningEffortConfig::XHigh) => "high",
                Some(ReasoningEffortConfig::High) => "high",
                Some(ReasoningEffortConfig::Medium) => {
                    // Flash supports "medium", Pro uses "medium" too
                    if api_model.contains("flash") {
                        "medium"
                    } else {
                        "medium"
                    }
                }
                Some(ReasoningEffortConfig::Low) => {
                    // Flash supports "minimal" for lowest, Pro uses "low"
                    if api_model.contains("flash") {
                        "minimal"
                    } else {
                        "low"
                    }
                }
                Some(ReasoningEffortConfig::Minimal) => {
                    // Minimal is Flash-exclusive, Pro falls back to "low"
                    if api_model.contains("flash") {
                        "minimal"
                    } else {
                        "low"
                    }
                }
                Some(ReasoningEffortConfig::None) => {
                    // No reasoning - use lowest available level
                    if api_model.contains("flash") {
                        "minimal"
                    } else {
                        "low"
                    }
                }
                None => "high", // Default to high for best quality
            };

            return Some(GeminiThinkingConfig {
                thinking_level: Some(thinking_level.to_string()),
                include_thoughts: Some(true),
                thinking_budget: None, // Do not mix with thinkingLevel for Gemini 3
            });
        }

        // For Gemini 2.5 and other models, use thinkingBudget
        Some(GeminiThinkingConfig {
            thinking_level: None,
            include_thoughts: matches!(
                reasoning_effort,
                Some(ReasoningEffortConfig::High | ReasoningEffortConfig::XHigh)
            )
            .then_some(true),
            thinking_budget: Some(DEFAULT_GEMINI_THINKING_BUDGET),
        })
    }

    fn normalize_gemini_base_url(base_url: &str) -> Cow<'_, str> {
        let trimmed = base_url.trim_end_matches('/');
        if let Some(prefix) = trimmed.strip_suffix("/v1") {
            Cow::Owned(format!("{prefix}/v1beta"))
        } else {
            Cow::Borrowed(trimmed)
        }
    }

    /// Streams a turn via the OpenAI Responses API.
    ///
    /// Handles SSE fixtures, reasoning summaries, verbosity, and the
    /// `text` controls used for output schemas.
    async fn stream_responses_api(&self, prompt: &Prompt) -> Result<ResponseStream> {
        if let Some(path) = &*CODEX_RS_SSE_FIXTURE {
            warn!(path, "Streaming from fixture");
            let stream = codex_api::stream_from_fixture(path, self.provider.stream_idle_timeout())
                .map_err(map_api_error)?;
            return Ok(map_response_stream(stream, self.otel_manager.clone()));
        }

        let auth_manager = self.auth_manager.clone();
        let model_family = self.get_model_family();
        let instructions = prompt.get_full_instructions(&model_family).into_owned();
        let tools_json: Vec<Value> = create_tools_json_for_responses_api(&prompt.tools)?;

        let reasoning = if model_family.supports_reasoning_summaries {
            Some(Reasoning {
                effort: self.effort.or(model_family.default_reasoning_effort),
                summary: if self.summary == ReasoningSummaryConfig::None {
                    None
                } else {
                    Some(self.summary)
                },
            })
        } else {
            None
        };

        let include: Vec<String> = if reasoning.is_some() {
            vec!["reasoning.encrypted_content".to_string()]
        } else {
            vec![]
        };

        let verbosity = if model_family.support_verbosity {
            self.config
                .model_verbosity
                .or(model_family.default_verbosity)
        } else {
            if self.config.model_verbosity.is_some() {
                warn!(
                    "model_verbosity is set but ignored as the model does not support verbosity: {}",
                    model_family.family
                );
            }
            None
        };

        let text = create_text_param_for_request(verbosity, &prompt.output_schema);
        let api_prompt = build_api_prompt(prompt, instructions.clone(), tools_json);
        let conversation_id = self.conversation_id.to_string();
        let session_source = self.session_source.clone();

        let mut refreshed = false;
        loop {
            let auth = auth_manager.as_ref().and_then(|m| m.auth());
            let api_provider = self
                .provider
                .to_api_provider(auth.as_ref().map(|a| a.mode))?;
            let api_auth = auth_provider_from_auth(auth.clone(), &self.provider).await?;
            let transport = ReqwestTransport::new(build_reqwest_client());
            let (request_telemetry, sse_telemetry) = self.build_streaming_telemetry();
            let client = ApiResponsesClient::new(transport, api_provider, api_auth)
                .with_telemetry(Some(request_telemetry), Some(sse_telemetry));

            let options = ApiResponsesOptions {
                reasoning: reasoning.clone(),
                include: include.clone(),
                prompt_cache_key: Some(conversation_id.clone()),
                text: text.clone(),
                store_override: None,
                conversation_id: Some(conversation_id.clone()),
                session_source: Some(session_source.clone()),
                extra_headers: beta_feature_headers(&self.config),
            };

            let stream_result = client
                .stream_prompt(&self.get_model(), &api_prompt, options)
                .await;

            match stream_result {
                Ok(stream) => {
                    return Ok(map_response_stream(stream, self.otel_manager.clone()));
                }
                Err(ApiError::Transport(TransportError::Http { status, .. }))
                    if status == StatusCode::UNAUTHORIZED =>
                {
                    handle_unauthorized(status, &mut refreshed, &auth_manager, &auth).await?;
                    continue;
                }
                Err(err) => return Err(map_api_error(err)),
            }
        }
    }

    pub fn get_provider(&self) -> ModelProviderInfo {
        self.provider.clone()
    }

    pub fn get_otel_manager(&self) -> OtelManager {
        self.otel_manager.clone()
    }

    pub fn get_session_source(&self) -> SessionSource {
        self.session_source.clone()
    }

    /// Returns the currently configured model slug.
    pub fn get_model(&self) -> String {
        self.get_model_family().get_model_slug().to_string()
    }

    /// Returns the currently configured model family.
    pub fn get_model_family(&self) -> ModelFamily {
        self.model_family.clone()
    }

    /// Returns the current reasoning effort setting.
    pub fn get_reasoning_effort(&self) -> Option<ReasoningEffortConfig> {
        self.effort
    }

    /// Returns the current reasoning summary setting.
    pub fn get_reasoning_summary(&self) -> ReasoningSummaryConfig {
        self.summary
    }

    pub fn get_auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.auth_manager.clone()
    }

    /// Compacts the current conversation history using the Compact endpoint.
    ///
    /// This is a unary call (no streaming) that returns a new list of
    /// `ResponseItem`s representing the compacted transcript.
    pub async fn compact_conversation_history(&self, prompt: &Prompt) -> Result<Vec<ResponseItem>> {
        if prompt.input.is_empty() {
            return Ok(Vec::new());
        }
        let auth_manager = self.auth_manager.clone();
        let auth = auth_manager.as_ref().and_then(|m| m.auth());
        let api_provider = self
            .provider
            .to_api_provider(auth.as_ref().map(|a| a.mode))?;
        let api_auth = auth_provider_from_auth(auth.clone(), &self.provider).await?;
        let transport = ReqwestTransport::new(build_reqwest_client());
        let request_telemetry = self.build_request_telemetry();
        let client = ApiCompactClient::new(transport, api_provider, api_auth)
            .with_telemetry(Some(request_telemetry));

        let instructions = prompt
            .get_full_instructions(&self.get_model_family())
            .into_owned();
        let sanitized_input = strip_thought_signatures_from_input(&prompt.input);
        let payload = ApiCompactionInput {
            model: &self.get_model(),
            input: &sanitized_input,
            instructions: &instructions,
        };

        let mut extra_headers = ApiHeaderMap::new();
        if let SessionSource::SubAgent(sub) = &self.session_source {
            let subagent = if let crate::protocol::SubAgentSource::Other(label) = sub {
                label.clone()
            } else {
                serde_json::to_value(sub)
                    .ok()
                    .and_then(|v| v.as_str().map(std::string::ToString::to_string))
                    .unwrap_or_else(|| "other".to_string())
            };
            if let Ok(val) = HeaderValue::from_str(&subagent) {
                extra_headers.insert("x-openai-subagent", val);
            }
        }

        client
            .compact_input(&payload, extra_headers)
            .await
            .map_err(map_api_error)
    }
}

fn spawn_gemini_response_stream(
    response_item: Option<ResponseItem>,
    response_id: String,
    token_usage: Option<TokenUsage>,
) -> ResponseStream {
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent>>(8);
    tokio::spawn(async move {
        if tx_event.send(Ok(ResponseEvent::Created)).await.is_err() {
            return;
        }
        if let Some(item) = response_item {
            if tx_event
                .send(Ok(ResponseEvent::OutputItemAdded(item.clone())))
                .await
                .is_err()
            {
                return;
            }
            if tx_event
                .send(Ok(ResponseEvent::OutputItemDone(item)))
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id,
                token_usage,
            }))
            .await;
    });
    ResponseStream { rx_event }
}

/// Spawns a task that processes Gemini SSE stream and converts it to ResponseStream.
fn spawn_gemini_sse_stream<S>(byte_stream: S, idle_timeout: Duration) -> ResponseStream
where
    S: futures::Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin + Send + 'static,
{
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent>>(1600);
    tokio::spawn(async move {
        process_gemini_sse(byte_stream, tx_event, idle_timeout).await;
    });
    ResponseStream { rx_event }
}

/// Processes Gemini SSE stream and emits ResponseEvents.
async fn process_gemini_sse<S>(
    stream: S,
    tx_event: mpsc::Sender<Result<ResponseEvent>>,
    idle_timeout: Duration,
) where
    S: futures::Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin,
{
    // Send Created event first
    if tx_event.send(Ok(ResponseEvent::Created)).await.is_err() {
        return;
    }

    let mut stream = stream
        .map_ok(|b| b)
        .map_err(|e| std::io::Error::other(e.to_string()))
        .eventsource();

    // State for accumulating response
    let mut accumulated_text = String::new();
    let mut assistant_item_sent = false;
    let mut reasoning_item_sent = false;
    let mut function_calls: Vec<(String, String, Option<String>, String)> = Vec::new(); // (name, args, thought_signature, call_id)
    let mut last_response_id = "gemini-stream".to_string();
    let mut last_token_usage: Option<TokenUsage> = None;
    let mut last_thought_signature: Option<String> = None;
    let mut last_inline_image: Option<(String, String)> = None; // (mime_type, data_base64)

    loop {
        let response = timeout(idle_timeout, stream.next()).await;

        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("Gemini SSE stream error: {}", e);
                // Don't send error, just break and emit what we have
                break;
            }
            Ok(None) => {
                // Stream ended - emit final items
                break;
            }
            Err(_) => {
                debug!("Gemini SSE idle timeout");
                // On timeout, emit what we have accumulated
                break;
            }
        };

        // Skip empty data
        if sse.data.trim().is_empty() {
            continue;
        }

        // Parse the JSON chunk
        let chunk: GeminiResponse = match serde_json::from_str(&sse.data) {
            Ok(val) => val,
            Err(err) => {
                debug!(
                    "Failed to parse Gemini SSE event: {err}, data: {}",
                    &sse.data
                );
                continue;
            }
        };

        // Update response ID and token usage if present
        if let Some(id) = chunk.response_id {
            last_response_id = id;
        }
        if let Some(usage) = chunk.usage_metadata {
            last_token_usage = Some(usage.into());
        }

        // Process candidates
        if let Some(candidates) = chunk.candidates {
            for candidate in candidates {
                if let Some(content) = candidate.content
                    && let Some(parts) = content.parts
                {
                    for part in parts {
                        // Track thought signature
                        if let Some(sig) = &part.thought_signature {
                            last_thought_signature = Some(sig.clone());
                        }
                        let is_thought = part.thought.is_some();

                        // Handle thought content - emit as ReasoningContentDelta
                        // This allows users to see what Gemini is thinking about
                        // Filter out garbage data that Gemini sometimes outputs when processing images
                        if is_thought
                            && let Some(text) = &part.text
                            && !text.is_empty()
                            && is_meaningful_thought_text(text)
                        {
                            // Send reasoning item notification on first thought
                            if !reasoning_item_sent {
                                // Emit a reasoning item added notification
                                let item = ResponseItem::Reasoning {
                                    id: format!("gemini-thought-{}", last_response_id),
                                    summary: vec![],
                                    content: None,
                                    encrypted_content: None,
                                };
                                if tx_event
                                    .send(Ok(ResponseEvent::OutputItemAdded(item)))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                                reasoning_item_sent = true;
                            }

                            // Send thought content as reasoning delta
                            if tx_event
                                .send(Ok(ResponseEvent::ReasoningContentDelta {
                                    delta: text.clone(),
                                    content_index: 0,
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            continue;
                        }

                        // Skip garbage thought content without emitting
                        if is_thought {
                            continue;
                        }

                        // Handle text content
                        if let Some(text) = part.text
                            && !is_thought
                            && !text.is_empty()
                        {
                            // Send OutputItemAdded on first text
                            if !assistant_item_sent {
                                let item = ResponseItem::Message {
                                    id: None,
                                    role: "assistant".to_string(),
                                    content: vec![],
                                    thought_signature: None,
                                };
                                if tx_event
                                    .send(Ok(ResponseEvent::OutputItemAdded(item)))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                                assistant_item_sent = true;
                            }

                            // Send text delta
                            if tx_event
                                .send(Ok(ResponseEvent::OutputTextDelta(text.clone())))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            accumulated_text.push_str(&text);
                        }

                        // Handle image content from image-capable Gemini models.
                        if let Some(inline_data) = part.inline_data
                            && !inline_data.data.trim().is_empty()
                            && !inline_data.mime_type.is_empty()
                        {
                            last_inline_image = Some((inline_data.mime_type, inline_data.data));
                        }

                        // Handle function call
                        if let Some(call) = part.function_call {
                            let name = call.name;
                            let args = if call.args.is_null() {
                                "{}".to_string()
                            } else {
                                call.args.to_string()
                            };
                            let thought_signature =
                                part.thought_signature.or(last_thought_signature.clone());
                            if let Some(last) = function_calls.last_mut()
                                && last.0 == name
                                && last.1 == args
                            {
                                last.2 = thought_signature;
                            } else {
                                function_calls.push((
                                    name,
                                    args,
                                    thought_signature,
                                    next_gemini_call_id(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    // Emit final items
    if assistant_item_sent || last_inline_image.is_some() {
        // Emit the complete message, which may include text, an image, or both.
        let mut content = Vec::new();
        if !accumulated_text.is_empty() {
            content.push(ContentItem::OutputText {
                text: accumulated_text,
            });
        }
        if let Some((mime_type, data)) = last_inline_image
            && !mime_type.is_empty()
            && !data.trim().is_empty()
        {
            let image_url = format!("data:{mime_type};base64,{data}");
            content.push(ContentItem::InputImage { image_url });
        }

        if !content.is_empty() {
            let item = ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content,
                thought_signature: last_thought_signature.clone(),
            };
            let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
        }
    }

    for (name, arguments, thought_signature, call_id) in function_calls {
        let item = ResponseItem::FunctionCall {
            id: None,
            name,
            arguments,
            call_id,
            thought_signature,
        };
        let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
    }

    // Send completed event
    let _ = tx_event
        .send(Ok(ResponseEvent::Completed {
            response_id: last_response_id,
            token_usage: last_token_usage,
        }))
        .await;
}

// The following functions are kept for potential fallback to non-streaming mode
#[allow(dead_code)]
struct GeminiParsedResponse {
    response_item: Option<ResponseItem>,
    response_id: String,
    token_usage: Option<TokenUsage>,
}

#[allow(dead_code)]
fn parse_gemini_response(body: GeminiResponse) -> Result<GeminiParsedResponse> {
    let response_id = body
        .response_id
        .unwrap_or_else(|| "gemini-response".to_string());
    let response_item = body
        .candidates
        .and_then(|candidates| candidates.into_iter().find_map(candidate_to_response_item));

    let token_usage = body.usage_metadata.map(Into::into);

    Ok(GeminiParsedResponse {
        response_item,
        response_id,
        token_usage,
    })
}

#[allow(dead_code)]
fn candidate_to_response_item(candidate: GeminiCandidate) -> Option<ResponseItem> {
    let content = candidate.content?;
    let parts = content.parts.unwrap_or_default();
    let mut response_parts = Vec::new();
    let mut function_call: Option<GeminiFunctionCall> = None;
    let mut function_call_thought_signature: Option<String> = None;
    let mut last_part_thought_signature: Option<String> = None;

    for part in parts {
        if let Some(sig) = &part.thought_signature {
            last_part_thought_signature = Some(sig.clone());
        }
        let is_thought = part.thought.is_some();

        if function_call.is_none()
            && let Some(call) = part.function_call
        {
            function_call = Some(call);
            // Capture the thought signature from the same part as the function call.
            function_call_thought_signature = part.thought_signature.clone();
        }

        if let Some(text) = part.text {
            if text.trim().is_empty() {
                continue;
            }
            if is_thought {
                continue;
            }
            response_parts.push(ContentItem::OutputText { text });
        }
    }

    if let Some(call) = function_call {
        // Prefer a thought signature that was attached to the same part as the
        // function call. If the provider instead attached the thoughtSignature
        // to a different part (for example, a trailing text part that closes
        // the step), fall back to the last observed signature for this
        // message so we do not drop it when replaying the call.
        let thought_signature = function_call_thought_signature.or(last_part_thought_signature);

        let args = if call.args.is_null() {
            "{}".to_string()
        } else {
            call.args.to_string()
        };

        return Some(ResponseItem::FunctionCall {
            id: None,
            name: call.name,
            arguments: args,
            call_id: "gemini-function-call".to_string(),
            thought_signature,
        });
    }

    if response_parts.is_empty() {
        None
    } else {
        Some(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: response_parts,
            thought_signature: last_part_thought_signature,
        })
    }
}

fn build_gemini_contents(
    items: &[ResponseItem],
    reference_images: &[String],
    api_model: &str,
) -> Vec<GeminiContentRequest> {
    let mut contents = Vec::new();
    // Record function calls emitted by the model so we can pair subsequent
    // FunctionCallOutput items with the correct function name and
    // thought_signature, even when multiple tool calls happen in the same turn.
    let mut function_calls_by_id: HashMap<String, (String, Option<String>)> = HashMap::new();

    for item in items {
        match item {
            ResponseItem::Message {
                role,
                content,
                thought_signature,
                ..
            } => {
                let parts = content_to_gemini_parts(content, thought_signature.as_deref());
                if parts.is_empty() {
                    continue;
                }

                contents.push(GeminiContentRequest {
                    role: Some(map_gemini_role(role)),
                    parts,
                });
            }
            // Handle FunctionCall from the model - add to history with role "model"
            // Per Gemini 3 spec: parallel function calls should be in the same content,
            // with only the FIRST part containing the thoughtSignature.
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                thought_signature,
                ..
            } => {
                function_calls_by_id
                    .insert(call_id.clone(), (name.clone(), thought_signature.clone()));
                let args: serde_json::Value = serde_json::from_str(arguments)
                    .unwrap_or(serde_json::Value::Object(Default::default()));

                // Check if the last content is a "model" role with function calls
                // If so, append to it (parallel function calls); otherwise create new
                let should_merge = contents
                    .last()
                    .map(|c| {
                        c.role.as_deref() == Some("model")
                            && c.parts.iter().all(|p| p.function_call.is_some())
                    })
                    .unwrap_or(false);

                if should_merge {
                    // Parallel function call - append to existing model content
                    // Per Gemini 3 spec: only the first functionCall has thoughtSignature
                    debug!(
                        "Gemini: merging parallel function call '{}' into existing model content (no thoughtSignature)",
                        name
                    );
                    let last = contents.last_mut().unwrap();
                    last.parts.push(GeminiPartRequest {
                        text: None,
                        inline_data: None,
                        function_call: Some(GeminiFunctionCallPart {
                            name: name.clone(),
                            args,
                        }),
                        function_response: None,
                        // Subsequent parallel calls should NOT have thoughtSignature
                        thought_signature: None,
                        compat_thought_signature: None,
                    });
                } else {
                    // First function call or after non-function-call content
                    debug!(
                        "Gemini: creating new model content for function call '{}' with thoughtSignature: {:?}",
                        name,
                        thought_signature.as_ref().map(|s| &s[..s.len().min(20)])
                    );
                    let part_thought_signature = thought_signature.clone();
                    contents.push(GeminiContentRequest {
                        role: Some("model".to_string()),
                        parts: vec![GeminiPartRequest {
                            text: None,
                            inline_data: None,
                            function_call: Some(GeminiFunctionCallPart {
                                name: name.clone(),
                                args,
                            }),
                            function_response: None,
                            // First function call has the thoughtSignature
                            thought_signature: part_thought_signature.clone(),
                            compat_thought_signature: part_thought_signature,
                        }],
                    });
                }
            }
            // Handle FunctionCallOutput - send back to model with role "user"
            // Per Gemini 3 spec:
            // 1. Function responses use role "user", not "function"
            // 2. thoughtSignature is ONLY on functionCall parts, NOT on functionResponse
            // 3. Parallel function responses should be grouped together
            ResponseItem::FunctionCallOutput { call_id, output } => {
                let (function_name, _thought_signature) = function_calls_by_id
                    .get(call_id)
                    .map(|(name, sig)| (name.clone(), sig.clone()))
                    .unwrap_or_else(|| ("unknown_function".to_string(), None));

                let (output_text, mut inline_parts) =
                    build_gemini_function_response_payload(output);
                let response_value = serde_json::json!({
                    "output": output_text,
                    "success": output.success.unwrap_or(true)
                });
                let supports_multimodal = is_gemini_3_model(api_model);
                let nested_parts = if supports_multimodal && !inline_parts.is_empty() {
                    Some(std::mem::take(&mut inline_parts))
                } else {
                    None
                };

                // Check if the last content is a "user" role with function responses (parallel responses)
                let should_merge = contents
                    .last()
                    .map(|c| {
                        c.role.as_deref() == Some("user")
                            && c.parts
                                .iter()
                                .all(|p| p.function_response.is_some() || p.inline_data.is_some())
                    })
                    .unwrap_or(false);

                // Per Gemini 3 spec: functionResponse parts should NOT have thoughtSignature
                let response_part = GeminiPartRequest {
                    text: None,
                    inline_data: None,
                    function_call: None,
                    function_response: Some(GeminiFunctionResponsePart {
                        id: Some(call_id.clone()),
                        name: function_name,
                        response: response_value,
                        parts: nested_parts,
                    }),
                    // Per Gemini 3 spec: NO thoughtSignature on functionResponse parts
                    thought_signature: None,
                    compat_thought_signature: None,
                };

                if should_merge {
                    // Parallel function response - append to existing user content
                    let last = contents.last_mut().unwrap();
                    last.parts.push(response_part);
                    if !supports_multimodal {
                        last.parts.append(&mut inline_parts);
                    }
                } else {
                    // First function response or after non-function-response content
                    let mut parts = vec![response_part];
                    if !supports_multimodal {
                        parts.append(&mut inline_parts);
                    }
                    // Per Gemini 3 spec: function responses use role "user"
                    contents.push(GeminiContentRequest {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
            }
            _ => {}
        }
    }

    append_reference_images_to_contents(&mut contents, reference_images);

    // Log summary of built contents for debugging
    if tracing::enabled!(tracing::Level::DEBUG) {
        let mut func_call_count = 0;
        let mut func_resp_count = 0;
        for content in &contents {
            for part in &content.parts {
                if part.function_call.is_some() {
                    func_call_count += 1;
                }
                if part.function_response.is_some() {
                    func_resp_count += 1;
                }
            }
        }
        debug!(
            "Gemini: built {} contents with {} function calls and {} function responses",
            contents.len(),
            func_call_count,
            func_resp_count
        );
    }

    contents
}

fn is_gemini_3_model(api_model: &str) -> bool {
    api_model.starts_with("gemini-3")
}

fn gemini_inline_data_part(mime_type: String, data: String) -> GeminiPartRequest {
    GeminiPartRequest {
        text: None,
        inline_data: Some(GeminiInlineData { mime_type, data }),
        function_call: None,
        function_response: None,
        thought_signature: None,
        compat_thought_signature: None,
    }
}

fn split_function_output_content(
    items: &[FunctionCallOutputContentItem],
) -> (Vec<String>, Vec<GeminiPartRequest>) {
    let mut text_parts = Vec::new();
    let mut inline_parts = Vec::new();

    for item in items {
        match item {
            FunctionCallOutputContentItem::InputText { text } => {
                if !text.trim().is_empty() {
                    text_parts.push(text.clone());
                }
            }
            FunctionCallOutputContentItem::InputImage { image_url } => {
                if let Some((mime, data)) = parse_data_url(image_url) {
                    inline_parts.push(gemini_inline_data_part(mime, data));
                } else if !image_url.trim().is_empty() {
                    text_parts.push(format!("Image reference: {image_url}"));
                }
            }
        }
    }

    (text_parts, inline_parts)
}

fn build_gemini_function_response_payload(
    output: &FunctionCallOutputPayload,
) -> (String, Vec<GeminiPartRequest>) {
    let (text_parts, inline_parts) = if let Some(items) = output
        .content_items
        .as_ref()
        .filter(|items| !items.is_empty())
    {
        split_function_output_content(items)
    } else {
        let mut text_parts = Vec::new();
        if !output.content.trim().is_empty() {
            text_parts.push(output.content.clone());
        }
        (text_parts, Vec::new())
    };

    let mut output_text = if text_parts.is_empty() {
        String::new()
    } else {
        text_parts.join("\n")
    };
    if output_text.is_empty() && !inline_parts.is_empty() {
        output_text = format!("Binary content provided ({} item(s)).", inline_parts.len());
    }

    (output_text, inline_parts)
}

fn map_gemini_role(role: &str) -> String {
    if role.eq_ignore_ascii_case("assistant") {
        "model".to_string()
    } else {
        "user".to_string()
    }
}

fn content_to_gemini_parts(
    content: &[ContentItem],
    message_thought_signature: Option<&str>,
) -> Vec<GeminiPartRequest> {
    let mut parts = Vec::new();
    for entry in content {
        if let ContentItem::InputText { text } | ContentItem::OutputText { text } = entry {
            if text.trim().is_empty() {
                continue;
            }
            parts.push(GeminiPartRequest {
                text: Some(text.clone()),
                inline_data: None,
                function_call: None,
                function_response: None,
                thought_signature: None,
                compat_thought_signature: None,
            });
        }
    }
    if let Some(sig) = message_thought_signature
        && !parts.is_empty()
        && let Some(last) = parts.last_mut()
        && last.thought_signature.is_none()
    {
        last.thought_signature = Some(sig.to_string());
        last.compat_thought_signature = Some(sig.to_string());
    }
    parts
}

fn ensure_active_loop_has_thought_signatures(
    contents: &[GeminiContentRequest],
) -> Vec<GeminiContentRequest> {
    /// Official Gemini 3 thought signature bypass string.
    /// Per Gemini 3 documentation, this special value instructs the API
    /// to skip thought signature validation for injected history.
    const SYNTHETIC_THOUGHT_SIGNATURE: &str = "context_engineering_is_the_way_to_go";

    let mut new_contents = contents.to_vec();
    // Find the start of the "active loop" as the last `user` turn that
    // contains a non‑empty text part. Gemini only validates thought signatures
    // for the current turn, so we avoid mutating earlier history.
    let mut last_user_with_text: Option<usize> = None;
    for (idx, content) in new_contents.iter().enumerate() {
        if !content
            .role
            .as_deref()
            .is_some_and(|role| role.eq_ignore_ascii_case("user"))
        {
            continue;
        }

        if content
            .parts
            .iter()
            .any(|part| part.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
        {
            last_user_with_text = Some(idx);
        }
    }

    let Some(start) = last_user_with_text.and_then(|idx| idx.checked_add(1)) else {
        return new_contents;
    };
    if start >= new_contents.len() {
        return new_contents;
    }

    // For every subsequent `model` turn in the active loop, ensure the first
    // `functionCall` part has a `thoughtSignature`. If the model did not
    // produce one (for example when history was injected), synthesize the
    // recommended dummy signature so Gemini accepts the request.
    for content in &mut new_contents[start..] {
        if !content
            .role
            .as_deref()
            .is_some_and(|role| role.eq_ignore_ascii_case("model"))
        {
            continue;
        }

        let mut patched_first_call = false;
        for part in &mut content.parts {
            if part.function_call.is_some() && !patched_first_call {
                patched_first_call = true;
                if part.thought_signature.is_none() {
                    let signature = part
                        .compat_thought_signature
                        .clone()
                        .unwrap_or_else(|| SYNTHETIC_THOUGHT_SIGNATURE.to_string());
                    part.thought_signature = Some(signature.clone());
                    if part.compat_thought_signature.is_none() {
                        part.compat_thought_signature = Some(signature);
                    }
                } else if part.compat_thought_signature.is_none() {
                    part.compat_thought_signature = part.thought_signature.clone();
                }
            }
        }
    }

    new_contents
}

fn append_reference_images_to_contents(
    contents: &mut Vec<GeminiContentRequest>,
    reference_images: &[String],
) {
    if reference_images.is_empty() {
        return;
    }

    // Enforce a soft cap on the number of inlineData image parts we send
    // back to Gemini preview image models. Cursor's Nano Banana Pro docs
    // recommend at most 14 reference images; once we exceed this budget we
    // drop extras while keeping text and tool call content intact.
    const MAX_INLINE_IMAGES: usize = 14;
    let limit = reference_images.len().min(MAX_INLINE_IMAGES);

    let user_index = contents.iter().rposition(|content| {
        content
            .role
            .as_deref()
            .is_some_and(|role| role.eq_ignore_ascii_case("user"))
    });

    let index = if let Some(i) = user_index {
        i
    } else {
        contents.push(GeminiContentRequest {
            role: Some("user".to_string()),
            parts: Vec::new(),
        });
        contents.len().saturating_sub(1)
    };

    for image_url in reference_images.iter().take(limit) {
        if let Some((mime, data)) = parse_data_url(image_url) {
            if mime.is_empty() || data.trim().is_empty() {
                continue;
            }
            contents[index].parts.push(GeminiPartRequest {
                text: None,
                inline_data: Some(GeminiInlineData {
                    mime_type: mime,
                    data,
                }),
                function_call: None,
                function_response: None,
                thought_signature: None,
                compat_thought_signature: None,
            });
        } else if !image_url.trim().is_empty() {
            // Fallback: preserve the URL as plain text hint when we cannot
            // parse a data URL. This keeps non-data URLs usable even if the
            // Gemini endpoint only understands inline data/file references.
            contents[index].parts.push(GeminiPartRequest {
                text: Some(format!("Image reference: {image_url}")),
                inline_data: None,
                function_call: None,
                function_response: None,
                thought_signature: None,
                compat_thought_signature: None,
            });
        }
    }
}

fn parse_data_url(url: &str) -> Option<(String, String)> {
    // Expected shape: data:<mime>;base64,<data>
    let without_prefix = url.strip_prefix("data:")?;
    let (meta, data) = without_prefix.split_once(',')?;
    let (mime, encoding) = meta.split_once(';')?;
    if !encoding.eq_ignore_ascii_case("base64") {
        return None;
    }
    Some((mime.to_string(), data.to_string()))
}

fn strip_additional_properties(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // Gemini function declaration schemas do not recognize
            // `additionalProperties`; drop it and recurse into all
            // nested values so schemas remain broadly compatible.
            map.remove("additionalProperties");
            for v in map.values_mut() {
                strip_additional_properties(v);
            }
        }
        Value::Array(items) => {
            for v in items {
                strip_additional_properties(v);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn build_gemini_tools(tools: &[ToolSpec]) -> Option<Vec<GeminiTool>> {
    let mut functions = Vec::new();

    for tool in tools {
        if let ToolSpec::Function(ResponsesApiTool {
            name,
            description,
            parameters,
            ..
        }) = tool
        {
            let params = serde_json::to_value(parameters).ok().map(|mut v| {
                strip_additional_properties(&mut v);
                v
            });
            functions.push(GeminiFunctionDeclaration {
                name: name.clone(),
                description: Some(description.clone()),
                parameters: params,
            });
        }
    }

    if functions.is_empty() {
        None
    } else {
        Some(vec![GeminiTool {
            function_declarations: Some(functions),
        }])
    }
}

impl ModelClient {
    /// Builds request and SSE telemetry for streaming API calls (Chat/Responses).
    fn build_streaming_telemetry(&self) -> (Arc<dyn RequestTelemetry>, Arc<dyn SseTelemetry>) {
        let telemetry = Arc::new(ApiTelemetry::new(self.otel_manager.clone()));
        let request_telemetry: Arc<dyn RequestTelemetry> = telemetry.clone();
        let sse_telemetry: Arc<dyn SseTelemetry> = telemetry;
        (request_telemetry, sse_telemetry)
    }

    /// Builds request telemetry for unary API calls (e.g., Compact endpoint).
    fn build_request_telemetry(&self) -> Arc<dyn RequestTelemetry> {
        let telemetry = Arc::new(ApiTelemetry::new(self.otel_manager.clone()));
        let request_telemetry: Arc<dyn RequestTelemetry> = telemetry;
        request_telemetry
    }
}

/// Produces a sanitized copy of the input transcript where any Gemini‑specific
/// `thought_signature` metadata attached to function calls is stripped.
///
/// This keeps internal Gemini state available inside `ResponseItem`s for
/// Gemini requests while ensuring we do not send unknown fields such as
/// `input[*].thought_signature` to non‑Gemini providers (for example the
/// OpenAI Responses API).
fn strip_thought_signatures_from_input(input: &[ResponseItem]) -> Vec<ResponseItem> {
    input
        .iter()
        .cloned()
        .map(|mut item| {
            if let ResponseItem::FunctionCall {
                thought_signature, ..
            } = &mut item
            {
                *thought_signature = None;
            }
            item
        })
        .collect()
}

/// Adapts the core `Prompt` type into the `codex-api` payload shape.
fn build_api_prompt(prompt: &Prompt, instructions: String, tools_json: Vec<Value>) -> ApiPrompt {
    let input = strip_thought_signatures_from_input(&prompt.get_formatted_input());
    ApiPrompt {
        instructions,
        input,
        tools: tools_json,
        parallel_tool_calls: prompt.parallel_tool_calls,
        output_schema: prompt.output_schema.clone(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContentRequest>,
    contents: Vec<GeminiContentRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<GeminiToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    safety_settings: Option<Vec<GeminiSafetySetting>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolConfig {
    function_calling_config: GeminiFunctionCallingConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCallingConfig {
    mode: GeminiFunctionCallingMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_function_names: Option<Vec<String>>,
    /// Gemini 3 Pro+ feature: stream function call arguments as they are generated.
    /// This reduces perceived latency when functions need to be called.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_function_call_arguments: Option<bool>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum GeminiFunctionCallingMode {
    #[allow(dead_code)]
    None,
    Auto,
    Any,
}

/// Media resolution for image/PDF processing.
/// Per Gemini 3 docs: media_resolution_low=280 tokens, media_resolution_medium=560,
/// media_resolution_high=1120, media_resolution_ultra_high=2240.
#[derive(Debug, Serialize, Clone, Copy)]
#[allow(dead_code)]
enum GeminiMediaResolution {
    /// Low resolution (280 tokens per image)
    #[serde(rename = "media_resolution_low")]
    Low,
    /// Medium resolution (560 tokens per image)
    #[serde(rename = "media_resolution_medium")]
    Medium,
    /// High resolution (1120 tokens per image) - recommended for image analysis
    #[serde(rename = "media_resolution_high")]
    High,
    /// Ultra high resolution (2240 tokens per image) - maximum quality
    /// Note: Cannot be set globally via generation_config, only per media part
    #[serde(rename = "media_resolution_ultra_high")]
    UltraHigh,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<GeminiThinkingConfig>,
    /// Media resolution for image/PDF processing.
    /// Higher resolution provides better detail but uses more tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    media_resolution: Option<GeminiMediaResolution>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct GeminiThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<String>,
    /// Whether to include model's thoughts in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    include_thoughts: Option<bool>,
    /// Token budget for thinking. Use -1 for no limit, 0 to disable.
    /// Codex caps this at 8192 tokens to keep Gemini 2.x loops bounded while
    /// still allowing multi-step reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[allow(clippy::enum_variant_names)]
enum GeminiHarmCategory {
    HarmCategoryHarassment,
    HarmCategoryHateSpeech,
    HarmCategorySexuallyExplicit,
    HarmCategoryDangerousContent,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[allow(dead_code, clippy::enum_variant_names)]
enum GeminiHarmBlockThreshold {
    BlockNone,
    BlockOnlyHigh,
    BlockMediumAndAbove,
    BlockLowAndAbove,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiSafetySetting {
    category: GeminiHarmCategory,
    threshold: GeminiHarmBlockThreshold,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiContentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPartRequest>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct GeminiPartRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<GeminiInlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCallPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponsePart>,
    /// Gemini 3 thought signature - must be returned exactly as received.
    /// Serialized as `thoughtSignature` for the Gemini API.
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
    /// Compatibility alias for providers that expect `thought_signature`
    /// on function call parts (for example, upstream proxies that validate
    /// thought signatures using snake_case field names). This is always
    /// serialized with the same value as `thoughtSignature` when present.
    #[serde(skip_serializing_if = "Option::is_none", rename = "thought_signature")]
    compat_thought_signature: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct GeminiInlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    response_id: Option<String>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContentResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContentResponse {
    parts: Option<Vec<GeminiPartResponse>>,
}

#[derive(Debug, Deserialize)]
struct GeminiPartResponse {
    #[serde(default)]
    text: Option<String>,

    /// Image payloads returned by image-capable Gemini models (e.g. inlineData).
    #[serde(rename = "inlineData", default)]
    inline_data: Option<GeminiInlineData>,

    #[serde(rename = "functionCall", default, alias = "function_call")]
    function_call: Option<GeminiFunctionCall>,

    /// Gemini 3 thought signature - must be preserved and returned in subsequent requests
    #[serde(rename = "thoughtSignature", default)]
    thought_signature: Option<String>,

    #[serde(default)]
    thought: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: serde_json::Value,
}

/// Used in request parts to represent a function call from the model (for history replay).
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCallPart {
    name: String,
    args: serde_json::Value,
}

/// Used in request parts to represent a function response back to the model.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionResponsePart {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    response: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    parts: Option<Vec<GeminiPartRequest>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    #[serde(skip_serializing_if = "Option::is_none")]
    function_declarations: Option<Vec<GeminiFunctionDeclaration>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionDeclaration {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<i64>,
    candidates_token_count: Option<i64>,
    total_token_count: Option<i64>,
    thoughts_token_count: Option<i64>,
}

impl From<GeminiUsageMetadata> for TokenUsage {
    fn from(meta: GeminiUsageMetadata) -> Self {
        let input = meta.prompt_token_count.unwrap_or_default();
        let output = meta.candidates_token_count.unwrap_or_default();
        let reasoning = meta.thoughts_token_count.unwrap_or_default();
        let total = meta.total_token_count.unwrap_or(input + output + reasoning);
        TokenUsage {
            input_tokens: input,
            cached_input_tokens: 0,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            total_tokens: total,
        }
    }
}

fn beta_feature_headers(config: &Config) -> ApiHeaderMap {
    let enabled = FEATURES
        .iter()
        .filter_map(|spec| {
            if spec.stage.beta_menu_description().is_some() && config.features.enabled(spec.id) {
                Some(spec.key)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let value = enabled.join(",");
    let mut headers = ApiHeaderMap::new();
    if !value.is_empty()
        && let Ok(header_value) = HeaderValue::from_str(value.as_str())
    {
        headers.insert("x-codex-beta-features", header_value);
    }
    headers
}

fn map_response_stream<S>(api_stream: S, otel_manager: OtelManager) -> ResponseStream
where
    S: futures::Stream<Item = std::result::Result<ResponseEvent, ApiError>>
        + Unpin
        + Send
        + 'static,
{
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent>>(1600);

    tokio::spawn(async move {
        let mut logged_error = false;
        let mut api_stream = api_stream;
        while let Some(event) = api_stream.next().await {
            match event {
                Ok(ResponseEvent::Completed {
                    response_id,
                    token_usage,
                }) => {
                    if let Some(usage) = &token_usage {
                        otel_manager.sse_event_completed(
                            usage.input_tokens,
                            usage.output_tokens,
                            Some(usage.cached_input_tokens),
                            Some(usage.reasoning_output_tokens),
                            usage.total_tokens,
                        );
                    }
                    if tx_event
                        .send(Ok(ResponseEvent::Completed {
                            response_id,
                            token_usage,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(event) => {
                    if tx_event.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                Err(err) => {
                    let mapped = map_api_error(err);
                    if !logged_error {
                        otel_manager.see_event_completed_failed(&mapped);
                        logged_error = true;
                    }
                    if tx_event.send(Err(mapped)).await.is_err() {
                        return;
                    }
                }
            }
        }
    });

    ResponseStream { rx_event }
}

/// Handles a 401 response by optionally refreshing ChatGPT tokens once.
///
/// When refresh succeeds, the caller should retry the API call; otherwise
/// the mapped `CodexErr` is returned to the caller.
async fn handle_unauthorized(
    status: StatusCode,
    refreshed: &mut bool,
    auth_manager: &Option<Arc<AuthManager>>,
    auth: &Option<crate::auth::CodexAuth>,
) -> Result<()> {
    if *refreshed {
        return Err(map_unauthorized_status(status));
    }

    if let Some(manager) = auth_manager.as_ref()
        && let Some(auth) = auth.as_ref()
        && auth.mode == AuthMode::ChatGPT
    {
        match manager.refresh_token().await {
            Ok(_) => {
                *refreshed = true;
                Ok(())
            }
            Err(RefreshTokenError::Permanent(failed)) => Err(CodexErr::RefreshTokenFailed(failed)),
            Err(RefreshTokenError::Transient(other)) => Err(CodexErr::Io(other)),
        }
    } else {
        Err(map_unauthorized_status(status))
    }
}

fn map_unauthorized_status(status: StatusCode) -> CodexErr {
    map_api_error(ApiError::Transport(TransportError::Http {
        status,
        headers: None,
        body: None,
    }))
}

struct ApiTelemetry {
    otel_manager: OtelManager,
}

impl ApiTelemetry {
    fn new(otel_manager: OtelManager) -> Self {
        Self { otel_manager }
    }
}

impl RequestTelemetry for ApiTelemetry {
    fn on_request(
        &self,
        attempt: u64,
        status: Option<HttpStatusCode>,
        error: Option<&TransportError>,
        duration: Duration,
    ) {
        let error_message = error.map(std::string::ToString::to_string);
        self.otel_manager.record_api_request(
            attempt,
            status.map(|s| s.as_u16()),
            error_message.as_deref(),
            duration,
        );
    }
}

impl SseTelemetry for ApiTelemetry {
    fn on_sse_poll(
        &self,
        result: &std::result::Result<
            Option<std::result::Result<Event, EventStreamError<TransportError>>>,
            tokio::time::error::Elapsed,
        >,
        duration: Duration,
    ) {
        self.otel_manager.log_sse_event(result, duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::FunctionCallOutputContentItem;
    use codex_protocol::models::FunctionCallOutputPayload;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn test_is_meaningful_thought_text_filters_garbage() {
        // Empty text should be filtered
        assert!(!is_meaningful_thought_text(""));
        assert!(!is_meaningful_thought_text("   "));

        // Normal text should pass
        assert!(is_meaningful_thought_text("Let me think about this..."));
        assert!(is_meaningful_thought_text("I need to analyze the image."));

        // Repeated zeros (garbage from image processing) should be filtered
        let zeros = "0".repeat(200);
        assert!(!is_meaningful_thought_text(&zeros));

        // Repeated digits should be filtered
        let digits = "1234567890".repeat(20);
        assert!(!is_meaningful_thought_text(&digits));

        // Text with 90%+ same character should be filtered
        let mostly_zeros = format!("{}abc", "0".repeat(150));
        assert!(!is_meaningful_thought_text(&mostly_zeros));

        // Short repeated text is OK (under threshold)
        assert!(is_meaningful_thought_text("000"));
        assert!(is_meaningful_thought_text("12345"));

        // Mixed content should pass
        assert!(is_meaningful_thought_text("The image shows 3 LEGO blocks arranged in a cross pattern."));
    }

    #[test]
    fn test_ensure_active_loop_fixes_all_turns() {
        let contents = vec![
            GeminiContentRequest {
                role: Some("user".to_string()),
                parts: vec![GeminiPartRequest {
                    text: Some("turn 1".to_string()),
                    inline_data: None,
                    function_call: None,
                    function_response: None,
                    thought_signature: None,
                    compat_thought_signature: None,
                }],
            },
            GeminiContentRequest {
                role: Some("model".to_string()),
                parts: vec![GeminiPartRequest {
                    text: None,
                    inline_data: None,
                    function_call: Some(GeminiFunctionCallPart {
                        name: "func1".to_string(),
                        args: json!({}),
                    }),
                    function_response: None,
                    thought_signature: None,
                    compat_thought_signature: None,
                }],
            },
            GeminiContentRequest {
                role: Some("user".to_string()),
                parts: vec![GeminiPartRequest {
                    text: Some("turn 2".to_string()),
                    inline_data: None,
                    function_call: None,
                    function_response: None,
                    thought_signature: None,
                    compat_thought_signature: None,
                }],
            },
            GeminiContentRequest {
                role: Some("model".to_string()),
                parts: vec![GeminiPartRequest {
                    text: None,
                    inline_data: None,
                    function_call: Some(GeminiFunctionCallPart {
                        name: "func2".to_string(),
                        args: json!({}),
                    }),
                    function_response: None,
                    thought_signature: None,
                    compat_thought_signature: None,
                }],
            },
        ];

        let processed = ensure_active_loop_has_thought_signatures(&contents);

        assert_eq!(processed.len(), 4);
        // Only the active loop (after the last user-with-text turn) should be patched.
        assert_eq!(processed[1].role.as_deref(), Some("model"));
        assert!(
            processed[1].parts[0].thought_signature.is_none(),
            "Earlier model turn outside active loop should remain unchanged"
        );
        assert!(
            processed[1].parts[0].compat_thought_signature.is_none(),
            "Earlier model turn outside active loop should remain unchanged"
        );

        // Turn 2 Model (Index 3) - Should be fixed
        assert_eq!(processed[3].role.as_deref(), Some("model"));
        assert_eq!(
            processed[3].parts[0].thought_signature.as_deref(),
            Some("context_engineering_is_the_way_to_go"),
            "Latest model turn in active loop should have thought signature"
        );
        assert_eq!(
            processed[3].parts[0].compat_thought_signature.as_deref(),
            Some("context_engineering_is_the_way_to_go"),
            "Latest model turn in active loop should have thought signature"
        );
    }

    #[test]
    fn normalize_gemini_base_url_promotes_v1_to_v1beta() {
        assert_eq!(
            ModelClient::normalize_gemini_base_url("https://api.ppaicode.com/v1").as_ref(),
            "https://api.ppaicode.com/v1beta"
        );
        assert_eq!(
            ModelClient::normalize_gemini_base_url("https://generativelanguage.googleapis.com/v1/")
                .as_ref(),
            "https://generativelanguage.googleapis.com/v1beta"
        );
        assert_eq!(
            ModelClient::normalize_gemini_base_url("https://api.ppchat.vip/v1beta").as_ref(),
            "https://api.ppchat.vip/v1beta"
        );
    }

    #[test]
    fn candidate_to_response_item_captures_function_call_thought_signature() {
        let body: GeminiResponse = serde_json::from_value(json!({
            "responseId": "resp-1",
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "shell_command",
                                    "args": { "command": "ls" }
                                },
                                "thoughtSignature": "sig-func-part"
                            }
                        ]
                    }
                }
            ]
        }))
        .unwrap();

        let parsed = parse_gemini_response(body).unwrap();
        let item = parsed.response_item.expect("expected response item");
        match item {
            ResponseItem::FunctionCall {
                name,
                thought_signature,
                ..
            } => {
                assert_eq!(name, "shell_command");
                assert_eq!(thought_signature.as_deref(), Some("sig-func-part"));
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn candidate_to_response_item_uses_last_part_thought_signature_for_function_call() {
        let body: GeminiResponse = serde_json::from_value(json!({
            "responseId": "resp-1",
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "shell_command",
                                    "args": { "command": "ls" }
                                }
                            },
                            {
                                "text": "running command",
                                "thoughtSignature": "sig-last-part"
                            }
                        ]
                    }
                }
            ]
        }))
        .unwrap();

        let parsed = parse_gemini_response(body).unwrap();
        let item = parsed.response_item.expect("expected response item");
        match item {
            ResponseItem::FunctionCall {
                name,
                thought_signature,
                ..
            } => {
                assert_eq!(name, "shell_command");
                assert_eq!(thought_signature.as_deref(), Some("sig-last-part"));
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn build_gemini_contents_pairs_function_call_outputs_by_call_id() {
        // Test parallel function calls - per Gemini 3 spec, consecutive function calls
        // should be merged into the same content, with only the first having thoughtSignature
        let items = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hi".to_string(),
                }],
                thought_signature: None,
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "shell_command".to_string(),
                arguments: serde_json::to_string(&json!({ "command": "ls" })).unwrap(),
                call_id: "call-1".to_string(),
                thought_signature: Some("sig-1".to_string()),
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "read_file".to_string(),
                arguments: serde_json::to_string(&json!({ "path": "README.md" })).unwrap(),
                call_id: "call-2".to_string(),
                thought_signature: Some("sig-2".to_string()),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output: FunctionCallOutputPayload {
                    content: "out-1".to_string(),
                    success: Some(true),
                    ..Default::default()
                },
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-2".to_string(),
                output: FunctionCallOutputPayload {
                    content: "out-2".to_string(),
                    success: Some(false),
                    ..Default::default()
                },
            },
        ];

        let contents = build_gemini_contents(&items, &[], "gemini-3-pro-preview");

        // Per Gemini 3 spec: parallel calls are merged
        // contents[0] = user message
        // contents[1] = model with 2 function calls (merged)
        // contents[2] = function with 2 responses (merged)
        assert_eq!(contents.len(), 3);

        // Verify parallel function calls are in same model content
        assert_eq!(contents[1].role.as_deref(), Some("model"));
        assert_eq!(contents[1].parts.len(), 2);

        // First function call has thoughtSignature
        assert!(contents[1].parts[0].function_call.is_some());
        assert_eq!(
            contents[1].parts[0]
                .function_call
                .as_ref()
                .unwrap()
                .name,
            "shell_command"
        );
        assert_eq!(
            contents[1].parts[0].thought_signature.as_deref(),
            Some("sig-1")
        );

        // Second function call should NOT have thoughtSignature (per Gemini 3 spec)
        assert!(contents[1].parts[1].function_call.is_some());
        assert_eq!(
            contents[1].parts[1]
                .function_call
                .as_ref()
                .unwrap()
                .name,
            "read_file"
        );
        assert!(
            contents[1].parts[1].thought_signature.is_none(),
            "Parallel function calls after the first should not have thoughtSignature"
        );

        // Verify parallel function responses are in same user content (per Gemini 3 spec)
        assert_eq!(contents[2].role.as_deref(), Some("user"));
        assert_eq!(contents[2].parts.len(), 2);

        // Per Gemini 3 spec: functionResponse parts should NOT have thoughtSignature
        let response1 = contents[2].parts[0]
            .function_response
            .as_ref()
            .expect("expected function response");
        assert_eq!(response1.name, "shell_command");
        assert!(
            contents[2].parts[0].thought_signature.is_none(),
            "functionResponse parts should NOT have thoughtSignature per Gemini 3 spec"
        );

        // Second response also has no thoughtSignature
        let response2 = contents[2].parts[1]
            .function_response
            .as_ref()
            .expect("expected function response");
        assert_eq!(response2.name, "read_file");
        assert!(
            contents[2].parts[1].thought_signature.is_none(),
            "functionResponse parts should NOT have thoughtSignature per Gemini 3 spec"
        );
    }

    #[test]
    fn build_gemini_contents_nests_inline_data_for_gemini_3_function_responses() {
        let items = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hi".to_string(),
                }],
                thought_signature: None,
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "view_image".to_string(),
                arguments: serde_json::to_string(&json!({ "path": "image.png" })).unwrap(),
                call_id: "call-1".to_string(),
                thought_signature: Some("sig-1".to_string()),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output: FunctionCallOutputPayload {
                    content_items: Some(vec![FunctionCallOutputContentItem::InputImage {
                        image_url: "data:image/png;base64,AAAA".to_string(),
                    }]),
                    ..Default::default()
                },
            },
        ];

        let contents = build_gemini_contents(&items, &[], "gemini-3-pro-preview");

        let expected_inline = GeminiPartRequest {
            text: None,
            inline_data: Some(GeminiInlineData {
                mime_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            }),
            function_call: None,
            function_response: None,
            thought_signature: None,
            compat_thought_signature: None,
        };

        // Per Gemini 3 spec: functionResponse parts do NOT have thoughtSignature
        let expected_parts = vec![GeminiPartRequest {
            text: None,
            inline_data: None,
            function_call: None,
            function_response: Some(GeminiFunctionResponsePart {
                id: Some("call-1".to_string()),
                name: "view_image".to_string(),
                response: json!({
                    "output": "Binary content provided (1 item(s)).",
                    "success": true
                }),
                parts: Some(vec![expected_inline]),
            }),
            thought_signature: None,
            compat_thought_signature: None,
        }];

        // Function response should have role "user" per Gemini 3 spec
        assert_eq!(contents[2].role.as_deref(), Some("user"));
        assert_eq!(contents[2].parts, expected_parts);
    }

    #[test]
    fn build_gemini_contents_sends_inline_data_as_siblings_for_non_gemini_3() {
        let items = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hi".to_string(),
                }],
                thought_signature: None,
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "view_image".to_string(),
                arguments: serde_json::to_string(&json!({ "path": "image.png" })).unwrap(),
                call_id: "call-1".to_string(),
                thought_signature: Some("sig-1".to_string()),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output: FunctionCallOutputPayload {
                    content_items: Some(vec![FunctionCallOutputContentItem::InputImage {
                        image_url: "data:image/png;base64,AAAA".to_string(),
                    }]),
                    ..Default::default()
                },
            },
        ];

        let contents = build_gemini_contents(&items, &[], "gemini-2.5-pro");

        let expected_inline = GeminiPartRequest {
            text: None,
            inline_data: Some(GeminiInlineData {
                mime_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            }),
            function_call: None,
            function_response: None,
            thought_signature: None,
            compat_thought_signature: None,
        };

        // Per Gemini spec: functionResponse parts do NOT have thoughtSignature
        let expected_parts = vec![
            GeminiPartRequest {
                text: None,
                inline_data: None,
                function_call: None,
                function_response: Some(GeminiFunctionResponsePart {
                    id: Some("call-1".to_string()),
                    name: "view_image".to_string(),
                    response: json!({
                        "output": "Binary content provided (1 item(s)).",
                        "success": true
                    }),
                    parts: None,
                }),
                thought_signature: None,
                compat_thought_signature: None,
            },
            expected_inline,
        ];

        // Function response should have role "user" per Gemini spec
        assert_eq!(contents[2].role.as_deref(), Some("user"));
        assert_eq!(contents[2].parts, expected_parts);
    }

    fn make_function_tool(name: &str) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: name.to_string(),
            description: String::new(),
            strict: false,
            parameters: crate::tools::spec::JsonSchema::Object {
                properties: BTreeMap::new(),
                required: None,
                additional_properties: Some(false.into()),
            },
        })
    }

    #[test]
    fn gemini_read_tools_first_turn_uses_any_mode_for_repo_analysis_requests() {
        let tools = vec![
            make_function_tool("apply_patch"),
            make_function_tool("grep_files"),
            make_function_tool("read_file"),
            make_function_tool("list_dir"),
        ];

        let input = vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "帮我分析这个项目，并阅读相关代码".to_string(),
            }],
            thought_signature: None,
        }];

        let config = build_gemini_tool_config_with_override(&tools, &input, None, "gemini-2.5-pro");

        assert_eq!(config.mode, GeminiFunctionCallingMode::Any);
        assert_eq!(
            config.allowed_function_names,
            Some(vec![
                "grep_files".to_string(),
                "list_dir".to_string(),
                "read_file".to_string()
            ])
        );
        // Non-Gemini 3 model should not have stream_function_call_arguments
        assert_eq!(config.stream_function_call_arguments, None);
    }

    #[test]
    fn gemini_read_tools_first_turn_falls_back_to_auto_when_not_first_turn() {
        let tools = vec![make_function_tool("grep_files")];
        let input = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "帮我分析这个项目".to_string(),
                }],
                thought_signature: None,
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output: FunctionCallOutputPayload {
                    content: "done".to_string(),
                    success: Some(true),
                    ..Default::default()
                },
            },
        ];

        let config = build_gemini_tool_config_with_override(&tools, &input, Some(true), "gemini-2.5-pro");

        assert_eq!(config.mode, GeminiFunctionCallingMode::Auto);
        assert_eq!(config.allowed_function_names, None);
    }

    #[test]
    fn gemini_read_tools_first_turn_respects_force_override() {
        let tools = vec![make_function_tool("grep_files")];
        let input = vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "hello".to_string(),
            }],
            thought_signature: None,
        }];

        // Test with Gemini 3 model - should have stream_function_call_arguments enabled
        let config = build_gemini_tool_config_with_override(&tools, &input, Some(true), "gemini-3-pro-preview");
        assert_eq!(config.mode, GeminiFunctionCallingMode::Any);
        assert_eq!(
            config.allowed_function_names,
            Some(vec!["grep_files".to_string()])
        );
        assert_eq!(config.stream_function_call_arguments, Some(true));

        // Test with Gemini 2.5 model - should not have stream_function_call_arguments
        let config = build_gemini_tool_config_with_override(&tools, &input, Some(false), "gemini-2.5-pro");
        assert_eq!(config.mode, GeminiFunctionCallingMode::Auto);
        assert_eq!(config.allowed_function_names, None);
        assert_eq!(config.stream_function_call_arguments, None);
    }

    #[test]
    fn build_gemini_thinking_config_sets_high_level_for_thinking_models() {
        let config = ModelClient::build_gemini_thinking_config(
            "gemini-3-pro-preview-codex",
            Some(ReasoningEffortConfig::High),
        );

        assert_eq!(
            config,
            Some(GeminiThinkingConfig {
                thinking_level: Some("high".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None, // Gemini 3 uses thinkingLevel only, not thinkingBudget
            })
        );
    }

    #[test]
    fn build_gemini_thinking_config_defaults_to_high_for_gemini_3() {
        let config = ModelClient::build_gemini_thinking_config("gemini-3-flash-preview", None);

        assert_eq!(
            config,
            Some(GeminiThinkingConfig {
                thinking_level: Some("high".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None, // Gemini 3 uses thinkingLevel only, not thinkingBudget
            })
        );
    }

    #[test]
    fn build_gemini_thinking_config_uses_budget_for_text_models() {
        let config = ModelClient::build_gemini_thinking_config("gemini-2.5-pro", None);

        assert_eq!(
            config,
            Some(GeminiThinkingConfig {
                thinking_level: None,
                include_thoughts: None,
                thinking_budget: Some(DEFAULT_GEMINI_THINKING_BUDGET),
            })
        );
    }

    #[test]
    fn build_gemini_thinking_config_skips_image_models() {
        let config = ModelClient::build_gemini_thinking_config(
            "gemini-3-pro-image-preview",
            Some(ReasoningEffortConfig::High),
        );

        assert_eq!(config, None);
    }

    #[test]
    fn build_gemini_thinking_config_flash_uses_minimal_for_low_effort() {
        let config = ModelClient::build_gemini_thinking_config(
            "gemini-3-flash-preview",
            Some(ReasoningEffortConfig::Low),
        );

        assert_eq!(
            config,
            Some(GeminiThinkingConfig {
                thinking_level: Some("minimal".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None,
            })
        );
    }

    #[test]
    fn build_gemini_thinking_config_pro_uses_low_for_low_effort() {
        let config = ModelClient::build_gemini_thinking_config(
            "gemini-3-pro-preview",
            Some(ReasoningEffortConfig::Low),
        );

        assert_eq!(
            config,
            Some(GeminiThinkingConfig {
                thinking_level: Some("low".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None,
            })
        );
    }

    #[test]
    fn build_gemini_thinking_config_medium_effort() {
        // Both Flash and Pro should use "medium" for medium effort
        let flash_config = ModelClient::build_gemini_thinking_config(
            "gemini-3-flash-preview",
            Some(ReasoningEffortConfig::Medium),
        );

        assert_eq!(
            flash_config,
            Some(GeminiThinkingConfig {
                thinking_level: Some("medium".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None,
            })
        );

        let pro_config = ModelClient::build_gemini_thinking_config(
            "gemini-3-pro-preview",
            Some(ReasoningEffortConfig::Medium),
        );

        assert_eq!(
            pro_config,
            Some(GeminiThinkingConfig {
                thinking_level: Some("medium".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None,
            })
        );
    }
}
