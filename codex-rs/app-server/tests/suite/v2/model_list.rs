use std::time::Duration;

use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::to_response;
use app_test_support::write_models_cache;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ReasoningEffortOption;
use codex_app_server_protocol::RequestId;
use codex_protocol::openai_models::ReasoningEffort;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

#[tokio::test]
async fn list_models_returns_all_models_with_large_limit() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    let ModelListResponse {
        data: items,
        next_cursor,
    } = to_response::<ModelListResponse>(response)?;

    let expected_models = vec![
        Model {
            id: "gpt-5.2".to_string(),
            model: "gpt-5.2".to_string(),
            display_name: "gpt-5.2".to_string(),
            description:
                "Latest frontier model with improvements across knowledge, reasoning and coding"
                    .to_string(),
            supported_reasoning_efforts: vec![
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Low,
                    description: "Balances speed with some reasoning; useful for straightforward \
                                   queries and short explanations"
                        .to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Medium,
                    description: "Provides a solid balance of reasoning depth and latency for \
                         general-purpose tasks"
                        .to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems"
                        .to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning for complex problems".to_string(),
                },
            ],
            default_reasoning_effort: ReasoningEffort::Medium,
            is_default: true,
        },
        Model {
            id: "gpt-5.1-codex-mini".to_string(),
            model: "gpt-5.1-codex-mini".to_string(),
            display_name: "gpt-5.1-codex-mini".to_string(),
            description: "Optimized for codex. Cheaper, faster, but less capable.".to_string(),
            supported_reasoning_efforts: vec![
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Medium,
                    description: "Dynamically adjusts reasoning based on the task".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::High,
                    description: "Maximizes reasoning depth for complex or ambiguous problems"
                        .to_string(),
                },
            ],
            default_reasoning_effort: ReasoningEffort::Medium,
            is_default: false,
        },
        Model {
            id: "gpt-5.1-codex-max".to_string(),
            model: "gpt-5.1-codex-max".to_string(),
            display_name: "gpt-5.1-codex-max".to_string(),
            description: "Codex-optimized flagship for deep and fast reasoning.".to_string(),
            supported_reasoning_efforts: vec![
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Low,
                    description: "Fast responses with lighter reasoning".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Medium,
                    description: "Balances speed and reasoning depth for everyday tasks"
                        .to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::High,
                    description: "Greater reasoning depth for complex problems".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning depth for complex problems".to_string(),
                },
            ],
            default_reasoning_effort: ReasoningEffort::Medium,
            is_default: false,
        },
        Model {
            id: "gpt-5.2-codex".to_string(),
            model: "gpt-5.2-codex".to_string(),
            display_name: "gpt-5.2-codex".to_string(),
            description: "Latest frontier agentic coding model.".to_string(),
            supported_reasoning_efforts: vec![
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Low,
                    description: "Fast responses with lighter reasoning".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Medium,
                    description: "Balances speed and reasoning depth for everyday tasks"
                        .to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::High,
                    description: "Greater reasoning depth for complex problems".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::XHigh,
                    description: "Extra high reasoning depth for complex problems".to_string(),
                },
            ],
            default_reasoning_effort: ReasoningEffort::Medium,
            is_default: false,
        },
        Model {
            id: "gemini-3-flash-preview-gemini".to_string(),
            model: "gemini-3-flash-preview-gemini".to_string(),
            display_name: "gemini-3-flash-preview-gemini".to_string(),
            description: "Google Gemini 3 Flash preview.".to_string(),
            supported_reasoning_efforts: vec![
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Minimal,
                    description: "Fastest responses with minimal reasoning (Flash-exclusive)"
                        .to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Low,
                    description: "Lower-cost Gemini thinking".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Medium,
                    description: "Balanced reasoning depth for general tasks".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::High,
                    description: "Higher-quality Gemini thinking for complex problems".to_string(),
                },
            ],
            default_reasoning_effort: ReasoningEffort::High,
            is_default: false,
        },
        Model {
            id: "gemini-3-pro-preview-codex".to_string(),
            model: "gemini-3-pro-preview-codex".to_string(),
            display_name: "gemini-3-pro-preview-codex".to_string(),
            description: "Gemini 3 Pro preview with Germini-style system prompt and Codex tooling."
                .to_string(),
            supported_reasoning_efforts: vec![
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Low,
                    description: "Lower-cost Gemini thinking".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::Medium,
                    description: "Balanced reasoning depth for general tasks".to_string(),
                },
                ReasoningEffortOption {
                    reasoning_effort: ReasoningEffort::High,
                    description: "Higher-quality Gemini thinking for complex problems".to_string(),
                },
            ],
            default_reasoning_effort: ReasoningEffort::High,
            is_default: false,
        },
        Model {
            id: "gemini-3-pro-image-preview".to_string(),
            model: "gemini-3-pro-image-preview".to_string(),
            display_name: "gemini-3-pro-image-preview".to_string(),
            description:
                "Gemini 3 Pro image preview for text, image understanding, and image generation."
                    .to_string(),
            supported_reasoning_efforts: vec![ReasoningEffortOption {
                reasoning_effort: ReasoningEffort::Medium,
                description: "Default Gemini reasoning behaviour for image workflows.".to_string(),
            }],
            default_reasoning_effort: ReasoningEffort::Medium,
            is_default: false,
        },
    ];

    assert_eq!(items, expected_models);
    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_pagination_works() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let all_request = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
        })
        .await?;

    let all_response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(all_request)),
    )
    .await??;

    let ModelListResponse {
        data: all_items,
        next_cursor: all_cursor,
    } = to_response::<ModelListResponse>(all_response)?;
    assert!(all_cursor.is_none());

    let expected_ids: Vec<String> = all_items.into_iter().map(|model| model.id).collect();
    assert!(!expected_ids.is_empty());

    let mut cursor = None;
    let mut ids = Vec::new();
    for _ in 0..expected_ids.len() {
        let request_id = mcp
            .send_list_models_request(ModelListParams {
                limit: Some(1),
                cursor: cursor.clone(),
            })
            .await?;

        let response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;

        let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;
        assert_eq!(data.len(), 1);
        ids.push(data[0].id.clone());
        cursor = next_cursor;

        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(ids, expected_ids);
    assert!(cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_models_cache(codex_home.path())?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: None,
            cursor: Some("invalid".to_string()),
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, "invalid cursor: invalid");
    Ok(())
}
