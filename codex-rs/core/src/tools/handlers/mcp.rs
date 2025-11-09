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
                if let ContentItem::InputImage { image_url } = content_item {
                    if image_url.starts_with("data:image/") {
                        tracing::info!("✅ 从会话中提取到图片 data URL (长度: {})", image_url.len());
                        return Some(image_url.clone());
                    }
                }
            }
        }
    }
    
    tracing::warn!("⚠️ 会话中未找到图片");
    None
}

/// 自动定位 codex-clipboard 临时文件（备用方案）
/// 由于文件总是在系统临时目录，我们可以自动补全路径
fn auto_resolve_clipboard_path(input: &str) -> Option<String> {
    // 只处理 codex-clipboard 文件
    if !input.contains("codex-clipboard") {
        return None;
    }

    // 如果已经是有效路径，直接使用
    if std::path::Path::new(input).exists() {
        return Some(input.to_string());
    }

    // 提取文件名（支持各种输入格式）
    let file_name = if input.contains('/') || input.contains('\\') {
        // 从路径中提取文件名
        std::path::Path::new(input)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(input)
    } else {
        // 已经是文件名
        input
    };

    // 系统临时目录是确定的位置
    let temp_path = std::env::temp_dir().join(file_name);
    if temp_path.exists() {
        let resolved = temp_path.to_string_lossy().to_string();
        tracing::info!("✅ 自动定位临时文件: {} -> {}", input, resolved);
        return Some(resolved);
    }

    // 备用位置（某些系统可能不同）
    for fallback in &["/tmp", "/private/tmp"] {
        let path = std::path::Path::new(fallback).join(file_name);
        if path.exists() {
            let resolved = path.to_string_lossy().to_string();
            tracing::info!("✅ 在备用位置找到: {} -> {}", input, resolved);
            return Some(resolved);
        }
    }

    tracing::warn!("⚠️ 未找到临时文件: {}", file_name);
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

        // 特殊处理 hunyuan_generate_3d 工具：自动注入剪贴板图片
        if tool == "hunyuan_generate_3d" && server == "hunyuan-3d" {
            if let Ok(mut args) = serde_json::from_str::<serde_json::Value>(&arguments_str) {
                if let Some(obj) = args.as_object_mut() {
                    // 判断是否需要注入图片
                    let needs_image = if let Some(url_value) = obj.get("image_url") {
                        if let Some(url_str) = url_value.as_str() {
                            // 如果传递的是文件名或无效路径（不是 data URL）
                            !url_str.starts_with("data:") && 
                            (url_str.contains("codex-clipboard") || url_str.is_empty())
                        } else {
                            false
                        }
                    } else {
                        // 没有 image_url 参数，需要自动注入
                        true
                    };
                    
                    if needs_image {
                        tracing::info!("🎯 检测到需要剪贴板图片，从会话提取...");
                        // 优先方案：从会话中获取 Codex 已处理的 data URL
                        if let Some(data_url) = extract_recent_image_from_session(session.as_ref()).await {
                            obj.insert("image_url".to_string(), serde_json::Value::String(data_url));
                            arguments_str = serde_json::to_string(&args).unwrap_or(arguments_str);
                            tracing::info!("✅ 成功从会话注入图片 data URL");
                        } else if let Some(url_value) = obj.get("image_url") {
                            // 备用方案：尝试查找本地文件
                            if let Some(url_str) = url_value.as_str() {
                                if let Some(real_path) = auto_resolve_clipboard_path(url_str) {
                                    tracing::info!("✅ 备用方案：找到文件 {}", real_path);
                                    obj.insert("image_url".to_string(), serde_json::Value::String(real_path));
                                    arguments_str = serde_json::to_string(&args).unwrap_or(arguments_str);
                                }
                            }
                        }
                    }
                }
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
