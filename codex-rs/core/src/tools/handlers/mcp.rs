use async_trait::async_trait;
use codex_protocol::models::ContentItem;

use crate::codex::Session;
use crate::function_tool::FunctionCallError;
use crate::mcp_tool_call::handle_mcp_tool_call;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct McpHandler;

/// 从会话中提取最近的剪贴板图片 data URL
/// Codex 已经将图片转换为 data URL，我们直接使用它
async fn extract_recent_image_from_session(session: &Session) -> Option<String> {
    let mut history = session.clone_history().await;
    let items = history.get_history();

    // 从后向前遍历，找最近的图片
    for item in items.iter().rev() {
        if let codex_protocol::models::ResponseItem::Message { content, .. } = item {
            for content_item in content {
                if let ContentItem::InputImage { image_url } = content_item
                    && image_url.starts_with("data:image/")
                {
                    tracing::info!("✅ 从会话中提取到图片 data URL (长度: {})", image_url.len());
                    return Some(image_url.clone());
                }
            }
        }
    }

    tracing::warn!("⚠️ 会话中未找到图片");
    None
}

#[async_trait]
impl ToolHandler for McpHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Mcp
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;

        let (server, tool, raw_arguments) = match payload {
            ToolPayload::Mcp {
                server,
                tool,
                raw_arguments,
            } => (server, tool, raw_arguments),
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "mcp handler received unsupported payload".to_string(),
                ));
            }
        };

        let mut arguments_str = raw_arguments;

        // 添加调试日志（如果参数很长，简化显示）
        if tool == "hunyuan_generate_3d" {
            tracing::info!("🔍 MCP Tool Called - hunyuan_generate_3d");
            // 不显示具体参数，避免长 data URL
        } else {
            tracing::info!("🔍 MCP Tool Called - Server: {}, Tool: {}", server, tool);
            if arguments_str.len() < 500 {
                tracing::info!("📝 Arguments: {}", arguments_str);
            }
        }

        // 特殊处理 hunyuan_generate_3d 工具：自动注入剪贴板图片
        if tool == "hunyuan_generate_3d"
            && server == "hunyuan-3d"
            && let Ok(mut args) = serde_json::from_str::<serde_json::Value>(&arguments_str)
            && let Some(obj) = args.as_object_mut()
        {
            // 检查是否有无效的 image_url（如 "[剪贴板图片]" 或其他无效值）
            let has_invalid_image_url = if let Some(url_value) = obj.get("image_url") {
                if let Some(url_str) = url_value.as_str() {
                    // 这些都是无效的 image_url，需要替换
                    url_str == "[剪贴板图片]"
                        || url_str.is_empty()
                        || url_str.contains("codex-clipboard")
                        || (!url_str.starts_with("data:")
                            && !url_str.starts_with("http://")
                            && !url_str.starts_with("https://")
                            && !std::path::Path::new(url_str).exists())
                } else {
                    true
                }
            } else {
                false
            };

            // 如果用户粘贴了图片，总是尝试从会话提取并替换
            if let Some(data_url) = extract_recent_image_from_session(session.as_ref()).await {
                // 移除任何现有的 image_url（避免与自动注入的冲突）
                if obj.contains_key("image_url") {
                    tracing::info!("⚠️ 移除传入的 image_url 参数，使用会话中的剪贴板图片");
                    obj.remove("image_url");
                }

                // 重要：图片模式下不能有 prompt！
                if obj.contains_key("prompt") {
                    tracing::info!("⚠️ 图片模式：移除 prompt 参数（API 限制）");
                    obj.remove("prompt");
                }

                // 注入正确的 data URL
                obj.insert("image_url".to_string(), serde_json::Value::String(data_url));
                arguments_str = serde_json::to_string(&args).unwrap_or(arguments_str);
                tracing::info!("✅ 自动注入剪贴板图片（data URL）");
            } else if has_invalid_image_url {
                // 如果有无效的 image_url 且没有找到会话图片，移除它
                tracing::info!("⚠️ 移除无效的 image_url 参数");
                obj.remove("image_url");
                arguments_str = serde_json::to_string(&args).unwrap_or(arguments_str);
            }
        }

        let response = handle_mcp_tool_call(
            session.as_ref(),
            turn.as_ref(),
            call_id.clone(),
            server,
            tool,
            arguments_str,
        )
        .await;

        match response {
            codex_protocol::models::ResponseInputItem::McpToolCallOutput { result, .. } => {
                Ok(ToolOutput::Mcp { result })
            }
            codex_protocol::models::ResponseInputItem::FunctionCallOutput { output, .. } => {
                let codex_protocol::models::FunctionCallOutputPayload {
                    content,
                    content_items,
                    success,
                } = output;
                Ok(ToolOutput::Function {
                    content,
                    content_items,
                    success,
                })
            }
            _ => Err(FunctionCallError::RespondToModel(
                "mcp handler received unexpected response variant".to_string(),
            )),
        }
    }
}
