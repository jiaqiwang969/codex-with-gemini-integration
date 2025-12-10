use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use app_test_support::McpProcess;
use app_test_support::to_response;
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

    // The model list may grow over time; ensure key presets are present with
    // the expected metadata instead of asserting exact equality.
    let find = |id: &str| {
        items
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("expected model `{id}` in list"))
    };

    let gpt_5_1_codex_max = find("gpt-5.1-codex-max")?;
    assert_eq!(
        gpt_5_1_codex_max.description,
        "Latest Codex-optimized flagship for deep and fast reasoning."
    );
    assert_eq!(
        gpt_5_1_codex_max.default_reasoning_effort,
        ReasoningEffort::Medium
    );

    let gpt_5_1_codex = find("gpt-5.1-codex")?;
    assert_eq!(gpt_5_1_codex.description, "Optimized for codex.");

    let gpt_5_1_codex_mini = find("gpt-5.1-codex-mini")?;
    assert_eq!(
        gpt_5_1_codex_mini.description,
        "Optimized for codex. Cheaper, faster, but less capable."
    );

    let gpt_5_1 = find("gpt-5.1")?;
    assert_eq!(
        gpt_5_1.description,
        "Broad world knowledge with strong general reasoning."
    );

    let gemini_3_pro_preview = find("gemini-3-pro-preview")?;
    assert_eq!(
        gemini_3_pro_preview.description,
        "Google Gemini 3 Pro preview."
    );

    let gemini_3_pro_image_preview = find("gemini-3-pro-image-preview")?;
    assert_eq!(
        gemini_3_pro_image_preview.description,
        "Gemini 3 Pro image preview for text, image understanding, and image generation."
    );

    assert!(next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_models_pagination_works() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let first_request = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(1),
            cursor: None,
        })
        .await?;

    let first_response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(first_request)),
    )
    .await??;

    let ModelListResponse {
        data: first_items,
        next_cursor: first_cursor,
    } = to_response::<ModelListResponse>(first_response)?;

    // Collect the full list with a large limit to compare against.
    let request_all = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
        })
        .await?;

    let all_response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_all)),
    )
    .await??;

    let ModelListResponse {
        data: all_items,
        next_cursor: all_cursor,
    } = to_response::<ModelListResponse>(all_response)?;
    assert!(all_cursor.is_none());

    let all_ids: Vec<String> = all_items.into_iter().map(|m| m.id).collect();

    // Now walk the paginated endpoint and ensure we see the same sequence of ids.
    assert_eq!(first_items.len(), 1);
    let mut paged_ids = vec![first_items[0].id.clone()];
    let mut cursor = first_cursor;

    while let Some(c) = cursor {
        let request = mcp
            .send_list_models_request(ModelListParams {
                limit: Some(1),
                cursor: Some(c.clone()),
            })
            .await?;

        let response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request)),
        )
        .await??;

        let ModelListResponse { data, next_cursor } = to_response::<ModelListResponse>(response)?;

        assert_eq!(data.len(), 1);
        paged_ids.push(data[0].id.clone());
        cursor = next_cursor;
    }

    assert_eq!(paged_ids, all_ids);
    Ok(())
}

#[tokio::test]
async fn list_models_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
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
