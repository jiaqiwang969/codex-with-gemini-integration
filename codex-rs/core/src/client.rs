use std::sync::Arc;

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
use codex_otel::otel_event_manager::OtelEventManager;
use codex_protocol::ConversationId;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::models::ContentItem;
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
use crate::flags::CODEX_RS_SSE_FIXTURE;
use crate::model_provider_info::ModelProviderInfo;
use crate::model_provider_info::WireApi;
use crate::openai_models::model_family::ModelFamily;
use crate::protocol::TokenUsage;
use crate::tools::spec::create_tools_json_for_chat_completions_api;
use crate::tools::spec::create_tools_json_for_responses_api;

#[derive(Debug, Clone)]
pub struct ModelClient {
    config: Arc<Config>,
    auth_manager: Option<Arc<AuthManager>>,
    model_family: ModelFamily,
    otel_event_manager: OtelEventManager,
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
        otel_event_manager: OtelEventManager,
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
            otel_event_manager,
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
                        self.otel_event_manager.clone(),
                    ))
                } else {
                    Ok(map_response_stream(
                        api_stream.aggregate(),
                        self.otel_event_manager.clone(),
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
                    &self.config.model,
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

        let api_model = {
            let mut api_model = self.config.model.as_str();
            if let Some(stripped) = api_model.strip_suffix("-codex") {
                api_model = stripped;
            }
            if let Some(stripped) = api_model.strip_suffix("-germini") {
                api_model = stripped;
            }
            api_model
        };

        // Use streamGenerateContent endpoint with alt=sse for streaming
        let url = format!(
            "{}/models/{api_model}:streamGenerateContent?alt=sse",
            base_url.trim_end_matches('/'),
        );

        let model_family = self.get_model_family();
        let instructions = prompt.get_full_instructions(&model_family).into_owned();
        let contents =
            build_gemini_contents(&prompt.get_formatted_input(), &prompt.reference_images);
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

        // Ensure the active loop has thought signatures on function calls so
        // preview models accept the request without 400/429 errors.
        let contents = ensure_active_loop_has_thought_signatures(&contents);

        let thinking_config = Self::build_gemini_thinking_config(api_model);

        // Build generationConfig with thinkingConfig nested properly.
        // Gemini defaults to temperature=1.0; Codex uses a slightly
        // lower temperature (0.8) to encourage more stable, less
        // speculative reasoning while still allowing exploration.
        // Gemini now enforces that only one of `thinkingLevel` or
        // `thinkingBudget` may be set. We pick the level for thinking
        // variants (to request high-quality thoughts) and budget for
        // non-thinking text models (to keep longer tool loops), while
        // omitting the field entirely for image models that reject it.
        let generation_config = Some(GeminiGenerationConfig {
            temperature: Some(0.8),
            top_k: Some(64),
            top_p: Some(0.95),
            max_output_tokens: None, // Let the model decide
            thinking_config,
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
            generation_config,
            safety_settings,
        };

        // Optional debug hook to inspect the exact Gemini request payload.
        if std::env::var("CODEX_DEBUG_GEMINI_REQUEST").is_ok()
            && let Ok(json) = serde_json::to_string_pretty(&request)
        {
            eprintln!("DEBUG GEMINI REQUEST:\n{json}");
        }

        // Build request with Gemini-specific auth handling
        let client = build_reqwest_client();
        let mut req_builder = client.post(&url);
        // Always apply provider-level headers so env_http_headers like
        // GEMINI_COOKIE are respected even when we inject the API key
        // directly below.
        req_builder = self.provider.apply_http_headers(req_builder);

        // Prefer GEMINI_API_KEY from the environment, then fall back to auth.json.
        // This matches the documented behaviour where a dedicated Gemini key
        // in the env takes precedence over the shared key stored in auth.json.
        let gemini_api_key = crate::auth::read_gemini_api_key_from_env().or_else(|| {
            crate::auth::read_gemini_api_key_from_auth_json(
                &self.config.codex_home,
                self.config.cli_auth_credentials_store_mode,
            )
        });

        if let Some(api_key) = gemini_api_key {
            // Override any existing X-Goog-Api-Key header so we can prefer
            // a dedicated Gemini key or the shared OPENAI_API_KEY from
            // auth.json when present.
            req_builder = req_builder.header("x-goog-api-key", api_key);
        }

        // Retry configuration: max 3 attempts with exponential backoff
        const MAX_ATTEMPTS: u64 = 3;
        const INITIAL_DELAY_MS: u64 = 5000;
        const MAX_DELAY_MS: u64 = 30000;

        let mut attempt: u64 = 0;
        let mut current_delay = INITIAL_DELAY_MS;

        let response = loop {
            attempt += 1;

            let result = self
                .otel_event_manager
                .log_request(attempt, || {
                    req_builder.try_clone().unwrap().json(&request).send()
                })
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

    fn build_gemini_thinking_config(api_model: &str) -> Option<GeminiThinkingConfig> {
        let is_thinking_model = api_model.contains("thinking");

        if api_model.contains("image") {
            return None;
        }

        if is_thinking_model {
            return Some(GeminiThinkingConfig {
                thinking_level: Some("high".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None,
            });
        }

        Some(GeminiThinkingConfig {
            thinking_level: None,
            include_thoughts: None,
            thinking_budget: Some(32768),
        })
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
            return Ok(map_response_stream(stream, self.otel_event_manager.clone()));
        }

        let auth_manager = self.auth_manager.clone();
        let model_family = self.get_model_family();
        let instructions = prompt.get_full_instructions(&model_family).into_owned();
        let tools_json: Vec<Value> = create_tools_json_for_responses_api(&prompt.tools)?;

        let reasoning = if model_family.supports_reasoning_summaries {
            Some(Reasoning {
                effort: self.effort.or(model_family.default_reasoning_effort),
                summary: Some(self.summary),
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
            };

            let stream_result = client
                .stream_prompt(&self.config.model, &api_prompt, options)
                .await;

            match stream_result {
                Ok(stream) => {
                    return Ok(map_response_stream(stream, self.otel_event_manager.clone()));
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

    pub fn get_otel_event_manager(&self) -> OtelEventManager {
        self.otel_event_manager.clone()
    }

    pub fn get_session_source(&self) -> SessionSource {
        self.session_source.clone()
    }

    /// Returns the currently configured model slug.
    pub fn get_model(&self) -> String {
        self.config.model.clone()
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
            model: &self.config.model,
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
    let mut function_call: Option<(String, String, Option<String>)> = None; // (name, args, thought_signature)
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

                        // Handle text content
                        if let Some(text) = part.text
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
                            let args = if call.args.is_null() {
                                "{}".to_string()
                            } else {
                                call.args.to_string()
                            };
                            function_call = Some((
                                call.name,
                                args,
                                part.thought_signature.or(last_thought_signature.clone()),
                            ));
                        }
                    }
                }
            }
        }
    }

    // Emit final items
    if let Some((name, arguments, thought_signature)) = function_call {
        // If there was a function call, emit it
        let item = ResponseItem::FunctionCall {
            id: None,
            name,
            arguments,
            call_id: "gemini-function-call".to_string(),
            thought_signature,
        };
        let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
    } else if assistant_item_sent || last_inline_image.is_some() {
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
                thought_signature: last_thought_signature,
            };
            let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
        }
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
) -> Vec<GeminiContentRequest> {
    let mut contents = Vec::new();
    // Track the last function call so we can pair it with the response and
    // propagate the Gemini 3 thought_signature back on the function response.
    let mut last_function_call_name: Option<String> = None;
    let mut last_function_call_thought_signature: Option<String> = None;

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
            ResponseItem::FunctionCall {
                name,
                arguments,
                thought_signature,
                ..
            } => {
                last_function_call_name = Some(name.clone());
                last_function_call_thought_signature = thought_signature.clone();
                let args: serde_json::Value = serde_json::from_str(arguments)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
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
                        // Pass through the thought signature exactly as received.
                        thought_signature: part_thought_signature.clone(),
                        compat_thought_signature: part_thought_signature,
                    }],
                });
            }
            // Handle FunctionCallOutput - send back to model with role "function"
            ResponseItem::FunctionCallOutput { output, .. } => {
                let function_name = last_function_call_name
                    .take()
                    .unwrap_or_else(|| "unknown_function".to_string());
                let thought_signature = last_function_call_thought_signature.take();

                // Build the response object with the output content
                let response_value = serde_json::json!({
                    "output": output.content.clone(),
                    "success": output.success.unwrap_or(true)
                });
                let part_thought_signature = thought_signature.clone();

                contents.push(GeminiContentRequest {
                    role: Some("function".to_string()),
                    parts: vec![GeminiPartRequest {
                        text: None,
                        inline_data: None,
                        function_call: None,
                        function_response: Some(GeminiFunctionResponsePart {
                            name: function_name,
                            response: response_value,
                        }),
                        thought_signature: part_thought_signature.clone(),
                        compat_thought_signature: part_thought_signature,
                    }],
                });
            }
            _ => {}
        }
    }

    append_reference_images_to_contents(&mut contents, reference_images);

    contents
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
    const SYNTHETIC_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

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
                    part.thought_signature = Some(SYNTHETIC_THOUGHT_SIGNATURE.to_string());
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
        let telemetry = Arc::new(ApiTelemetry::new(self.otel_event_manager.clone()));
        let request_telemetry: Arc<dyn RequestTelemetry> = telemetry.clone();
        let sse_telemetry: Arc<dyn SseTelemetry> = telemetry;
        (request_telemetry, sse_telemetry)
    }

    /// Builds request telemetry for unary API calls (e.g., Compact endpoint).
    fn build_request_telemetry(&self) -> Arc<dyn RequestTelemetry> {
        let telemetry = Arc::new(ApiTelemetry::new(self.otel_event_manager.clone()));
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
    generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    safety_settings: Option<Vec<GeminiSafetySetting>>,
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
    /// Codex caps this at 32768 tokens to better support long, multi-step
    /// reasoning while still bounding worst-case cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GeminiHarmCategory {
    HarmCategoryHarassment,
    HarmCategoryHateSpeech,
    HarmCategorySexuallyExplicit,
    HarmCategoryDangerousContent,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[allow(dead_code)]
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

#[derive(Debug, Serialize, Clone)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: serde_json::Value,
}

/// Used in request parts to represent a function call from the model (for history replay).
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCallPart {
    name: String,
    args: serde_json::Value,
}

/// Used in request parts to represent a function response back to the model.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionResponsePart {
    name: String,
    response: serde_json::Value,
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

fn map_response_stream<S>(api_stream: S, otel_event_manager: OtelEventManager) -> ResponseStream
where
    S: futures::Stream<Item = std::result::Result<ResponseEvent, ApiError>>
        + Unpin
        + Send
        + 'static,
{
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent>>(1600);
    let manager = otel_event_manager;

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
                        manager.sse_event_completed(
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
                        manager.see_event_completed_failed(&mapped);
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
    otel_event_manager: OtelEventManager,
}

impl ApiTelemetry {
    fn new(otel_event_manager: OtelEventManager) -> Self {
        Self { otel_event_manager }
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
        self.otel_event_manager.record_api_request(
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
        self.otel_event_manager.log_sse_event(result, duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

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

        // Turn 2 Model (Index 3) - Should be fixed
        assert_eq!(processed[3].role.as_deref(), Some("model"));
        assert_eq!(
            processed[3].parts[0].thought_signature.as_deref(),
            Some("skip_thought_signature_validator"),
            "Latest model turn in active loop should have thought signature"
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
    fn build_gemini_thinking_config_sets_high_level_for_thinking_models() {
        let config = ModelClient::build_gemini_thinking_config("gemini-3-pro-preview-thinking");

        assert_eq!(
            config,
            Some(GeminiThinkingConfig {
                thinking_level: Some("high".to_string()),
                include_thoughts: Some(true),
                thinking_budget: None,
            })
        );
    }

    #[test]
    fn build_gemini_thinking_config_uses_budget_for_text_models() {
        let config = ModelClient::build_gemini_thinking_config("gemini-3-pro-preview");

        assert_eq!(
            config,
            Some(GeminiThinkingConfig {
                thinking_level: None,
                include_thoughts: None,
                thinking_budget: Some(32768),
            })
        );
    }

    #[test]
    fn build_gemini_thinking_config_skips_image_models() {
        let config = ModelClient::build_gemini_thinking_config("gemini-3-pro-image-preview");

        assert_eq!(config, None);
    }
}
