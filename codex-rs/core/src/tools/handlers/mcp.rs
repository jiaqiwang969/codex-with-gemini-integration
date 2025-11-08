use async_trait::async_trait;

use crate::function_tool::FunctionCallError;
use crate::mcp_tool_call::handle_mcp_tool_call;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct McpHandler;

/// 自动定位 codex-clipboard 临时文件
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

        // 特殊处理 hunyuan_generate_3d 工具的图片路径
        if tool == "hunyuan_generate_3d"
            && let Ok(mut args) = serde_json::from_str::<serde_json::Value>(&arguments_str)
            && let Some(obj) = args.as_object_mut()
        {
            // 检查并修正 image_url 参数
            if let Some(url_value) = obj.get("image_url")
                && let Some(url_str) = url_value.as_str()
            {
                // 自动解析临时文件路径
                if let Some(real_path) = auto_resolve_clipboard_path(url_str) {
                    obj.insert(
                        "image_url".to_string(),
                        serde_json::Value::String(real_path),
                    );
                    arguments_str = serde_json::to_string(&args).unwrap_or(arguments_str);
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
