//! Query task status tool implementation

use anyhow::Context;
use anyhow::Result;
use mcp_types::CallToolResult;
use mcp_types::ContentBlock;
use mcp_types::TextContent;
use serde::Deserialize;
use serde_json::json;
use tracing::info;

use crate::models::ApiVersion;
use crate::tencent_cloud::TencentCloudClient;

#[derive(Debug, Deserialize)]
struct QueryParams {
    job_id: String,
    api_version: Option<String>,
}

pub async fn handle_query(
    arguments: serde_json::Value,
    secret_id: String,
    secret_key: String,
) -> Result<CallToolResult> {
    let params: QueryParams =
        serde_json::from_value(arguments).context("Failed to parse query parameters")?;

    // Parse API version
    let api_version = match params.api_version.as_deref() {
        Some("rapid") => ApiVersion::Rapid,
        _ => ApiVersion::Pro,
    };

    // Create client and query job
    let client = TencentCloudClient::new(secret_id, secret_key)?;
    let status = client.query_job(&params.job_id, api_version).await?;

    info!("Queried job {}: {}", params.job_id, status.status);

    // Format response
    let mut response_text = format!(
        "📊 Job Status Query\n\n\
        **Job ID**: {}\n\
        **Status**: {}\n",
        params.job_id, status.status
    );

    let status_lower = status.status.to_lowercase();

    if status_lower == "success" || status_lower == "completed" || status_lower == "finish" || status_lower == "done" {
        response_text.push_str("\n✅ Job completed successfully!\n");

        if let Some(preview_url) = &status.preview_url {
            response_text.push_str(&format!("\n**Preview**: {}\n", preview_url));
        }

        if let Some(result_urls) = &status.result_urls {
            response_text.push_str("\n**Result Files**:\n");
            for url in result_urls {
                response_text.push_str(&format!("  - {}\n", url));
            }
        }

        if let Some(files) = &status.result_file3_d_s {
            response_text.push_str("\n**3D Files**:\n");
            for file in files {
                response_text.push_str(&format!("  - {} format: {}\n", file.file_type, file.url));
                if let Some(preview) = &file.preview_image_url {
                    response_text.push_str(&format!("    Preview: {}\n", preview));
                }
            }
        }

        response_text.push_str("\n💡 Use hunyuan_download_results to download the files.\n");
    } else if status_lower == "failed" || status_lower == "error" || status_lower == "timeout" {
        let error_msg = status
            .error_msg
            .or(status.error_message)
            .unwrap_or_else(|| "Unknown error".to_string());
        response_text.push_str(&format!("\n❌ Job failed: {}\n", error_msg));
    } else if status_lower == "pending" || status_lower == "processing" || status_lower == "running" {
        response_text.push_str("\n⏳ Job is still processing. Please check again later.\n");
    }

    Ok(CallToolResult {
        content: vec![ContentBlock::TextContent(TextContent {
            r#type: "text".to_string(),
            text: response_text,
            annotations: None,
        })],
        is_error: None,
        structured_content: None,
    })
}
