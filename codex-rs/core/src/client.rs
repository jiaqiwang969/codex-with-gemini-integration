use std::sync::Arc;

use crate::api_bridge::auth_provider_from_auth;
use crate::api_bridge::map_api_error;
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
use codex_protocol::config_types::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SessionSource;
use eventsource_stream::Event;
use eventsource_stream::EventStreamError;
use futures::StreamExt;
use http::HeaderMap as ApiHeaderMap;
use http::HeaderValue;
use http::StatusCode as HttpStatusCode;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
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
use crate::model_family::ModelFamily;
use crate::model_provider_info::ModelProviderInfo;
use crate::model_provider_info::WireApi;
use crate::openai_model_info::get_model_info;
use crate::protocol::TokenUsage;
use crate::tools::spec::create_tools_json_for_chat_completions_api;
use crate::tools::spec::create_tools_json_for_responses_api;

#[derive(Debug, Clone)]
pub struct ModelClient {
    config: Arc<Config>,
    auth_manager: Option<Arc<AuthManager>>,
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
            otel_event_manager,
            provider,
            conversation_id,
            effort,
            summary,
            session_source,
        }
    }

    pub fn get_model_context_window(&self) -> Option<i64> {
        let pct = self.config.model_family.effective_context_window_percent;
        self.config
            .model_context_window
            .or_else(|| get_model_info(&self.config.model_family).map(|info| info.context_window))
            .map(|w| w.saturating_mul(pct) / 100)
    }

    pub fn get_auto_compact_token_limit(&self) -> Option<i64> {
        self.config.model_auto_compact_token_limit.or_else(|| {
            get_model_info(&self.config.model_family).and_then(|info| info.auto_compact_token_limit)
        })
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
        let instructions = prompt
            .get_full_instructions(&self.config.model_family)
            .into_owned();
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

        let url = format!(
            "{}/models/{}:generateContent",
            base_url.trim_end_matches('/'),
            self.config.model
        );

        let instructions = prompt
            .get_full_instructions(&self.config.model_family)
            .into_owned();
        let mut contents = build_gemini_contents(&prompt.get_formatted_input());
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
            }],
        });

        let tools = build_gemini_tools(&prompt.tools);

        let request = GeminiRequest {
            system_instruction,
            contents: std::mem::take(&mut contents),
            tools,
        };

        // Build request with Gemini-specific auth handling
        let client = build_reqwest_client();
        let mut req_builder = client.post(&url);
        // Always apply provider-level headers so env_http_headers like
        // GEMINI_COOKIE are respected even when we inject the API key
        // directly below.
        req_builder = self.provider.apply_http_headers(req_builder);

        // Try to get GEMINI_API_KEY from auth.json first, then fall back to env var
        let gemini_api_key = crate::auth::read_gemini_api_key_from_auth_json(
            &self.config.codex_home,
            self.config.cli_auth_credentials_store_mode,
        )
        .or_else(crate::auth::read_gemini_api_key_from_env);

        if let Some(api_key) = gemini_api_key {
            // Override any existing X-Goog-Api-Key header so we can prefer
            // a dedicated Gemini key or the shared OPENAI_API_KEY from
            // auth.json when present.
            req_builder = req_builder.header("x-goog-api-key", api_key);
        }

        let response = self
            .otel_event_manager
            .log_request(1, || req_builder.json(&request).send())
            .await
            .map_err(|err| {
                CodexErr::ResponseStreamFailed(ResponseStreamFailed {
                    source: err,
                    request_id: None,
                })
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CodexErr::UnexpectedStatus(UnexpectedResponseError {
                status,
                body,
                request_id: None,
            }));
        }

        let body: GeminiResponse = response.json().await.map_err(|err| {
            CodexErr::ResponseStreamFailed(ResponseStreamFailed {
                source: err,
                request_id: None,
            })
        })?;
        let parsed = parse_gemini_response(body)?;

        Ok(spawn_gemini_response_stream(
            parsed.response_item,
            parsed.response_id,
            parsed.token_usage,
        ))
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
        let instructions = prompt
            .get_full_instructions(&self.config.model_family)
            .into_owned();
        let tools_json: Vec<Value> = create_tools_json_for_responses_api(&prompt.tools)?;

        let reasoning = if self.config.model_family.supports_reasoning_summaries {
            Some(Reasoning {
                effort: self
                    .effort
                    .or(self.config.model_family.default_reasoning_effort),
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

        let verbosity = if self.config.model_family.support_verbosity {
            self.config
                .model_verbosity
                .or(self.config.model_family.default_verbosity)
        } else {
            if self.config.model_verbosity.is_some() {
                warn!(
                    "model_verbosity is set but ignored as the model does not support verbosity: {}",
                    self.config.model_family.family
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
        self.config.model_family.clone()
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
            .get_full_instructions(&self.config.model_family)
            .into_owned();
        let payload = ApiCompactionInput {
            model: &self.config.model,
            input: &prompt.input,
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

struct GeminiParsedResponse {
    response_item: Option<ResponseItem>,
    response_id: String,
    token_usage: Option<TokenUsage>,
}

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

fn candidate_to_response_item(candidate: GeminiCandidate) -> Option<ResponseItem> {
    let content = candidate.content?;
    let parts = content.parts.unwrap_or_default();
    let mut response_parts = Vec::new();
    let mut function_call: Option<GeminiFunctionCall> = None;

    for part in parts {
        if function_call.is_none()
            && let Some(call) = part.function_call
        {
            function_call = Some(call);
        }

        if let Some(text) = part.text {
            if text.trim().is_empty() {
                continue;
            }
            response_parts.push(ContentItem::OutputText { text });
        }
    }

    if let Some(call) = function_call {
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
        });
    }

    if response_parts.is_empty() {
        None
    } else {
        Some(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: response_parts,
        })
    }
}

fn build_gemini_contents(items: &[ResponseItem]) -> Vec<GeminiContentRequest> {
    let mut contents = Vec::new();
    for item in items {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let parts = content_to_gemini_parts(content);
                if parts.is_empty() {
                    continue;
                }
                contents.push(GeminiContentRequest {
                    role: Some(map_gemini_role(role)),
                    parts,
                });
            }
            ResponseItem::FunctionCallOutput { output, .. } => {
                let text = output.content.clone();
                if text.trim().is_empty() {
                    continue;
                }
                contents.push(GeminiContentRequest {
                    role: Some("user".to_string()),
                    parts: vec![GeminiPartRequest {
                        text: Some(text),
                        inline_data: None,
                    }],
                });
            }
            _ => {}
        }
    }
    contents
}

fn map_gemini_role(role: &str) -> String {
    if role.eq_ignore_ascii_case("assistant") {
        "model".to_string()
    } else {
        "user".to_string()
    }
}

fn content_to_gemini_parts(content: &[ContentItem]) -> Vec<GeminiPartRequest> {
    let mut parts = Vec::new();
    for entry in content {
        match entry {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                if text.trim().is_empty() {
                    continue;
                }
                parts.push(GeminiPartRequest {
                    text: Some(text.clone()),
                    inline_data: None,
                });
            }
            ContentItem::InputImage { image_url } => {
                if let Some((mime, data)) = parse_data_url(image_url) {
                    if data.trim().is_empty() {
                        continue;
                    }
                    parts.push(GeminiPartRequest {
                        text: None,
                        inline_data: Some(GeminiInlineData {
                            mime_type: mime,
                            data,
                        }),
                    });
                } else if !image_url.trim().is_empty() {
                    // Fallback: preserve the URL as plain text hint when we cannot
                    // parse a data URL. This keeps non-data URLs usable even if the
                    // Gemini endpoint only understands inline data/file references.
                    parts.push(GeminiPartRequest {
                        text: Some(format!("Image reference: {image_url}")),
                        inline_data: None,
                    });
                }
            }
        }
    }
    parts
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
            let params = serde_json::to_value(parameters).ok();
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

/// Adapts the core `Prompt` type into the `codex-api` payload shape.
fn build_api_prompt(prompt: &Prompt, instructions: String, tools_json: Vec<Value>) -> ApiPrompt {
    ApiPrompt {
        instructions,
        input: prompt.get_formatted_input(),
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPartRequest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPartRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<GeminiInlineData>,
}

#[derive(Debug, Serialize)]
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

    #[serde(rename = "functionCall", default, alias = "function_call")]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: serde_json::Value,
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
