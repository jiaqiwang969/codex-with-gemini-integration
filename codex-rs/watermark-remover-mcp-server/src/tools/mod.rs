//! Tool implementations for Watermark Remover

mod images_to_pdf;
mod pdf_to_images;
mod process_pdf;
mod remove_watermark;

use anyhow::Result;
use mcp_types::CallToolRequestParams;
use mcp_types::CallToolResult;
use mcp_types::Tool;
use mcp_types::ToolInputSchema;
use serde_json::json;

pub use images_to_pdf::handle_images_to_pdf;
pub use pdf_to_images::handle_pdf_to_images;
pub use process_pdf::handle_process_pdf;
pub use remove_watermark::handle_remove_watermark;

/// Get tool definitions for MCP
pub fn get_tool_definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "pdf_to_images".to_string(),
            title: None,
            description: Some("将PDF文件转换为PNG图片。每页转换为一张图片。".to_string()),
            annotations: None,
            output_schema: None,
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(json!({
                    "pdf_path": {
                        "type": "string",
                        "description": "PDF文件的绝对路径"
                    },
                    "output_dir": {
                        "type": "string",
                        "description": "输出目录路径（可选，默认在PDF同目录下创建临时目录）"
                    },
                    "dpi": {
                        "type": "integer",
                        "default": 200,
                        "description": "输出图片的DPI（默认200）"
                    }
                })),
                required: Some(vec!["pdf_path".to_string()]),
            },
        },
        Tool {
            name: "remove_watermark".to_string(),
            title: None,
            description: Some(
                "去除图片右下角的水印（如NotebookLM水印）。支持单张图片或整个目录。".to_string(),
            ),
            annotations: None,
            output_schema: None,
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(json!({
                    "image_path": {
                        "type": "string",
                        "description": "单张图片的路径（与image_dir二选一）"
                    },
                    "image_dir": {
                        "type": "string",
                        "description": "图片目录路径（与image_path二选一）"
                    },
                    "output_dir": {
                        "type": "string",
                        "description": "输出目录路径（可选，默认覆盖原图或输出到同目录）"
                    }
                })),
                required: Some(vec![]),
            },
        },
        Tool {
            name: "images_to_pdf".to_string(),
            title: None,
            description: Some("将目录中的图片合并为一个PDF文件。图片按文件名排序。".to_string()),
            annotations: None,
            output_schema: None,
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(json!({
                    "image_dir": {
                        "type": "string",
                        "description": "包含图片的目录路径"
                    },
                    "output_path": {
                        "type": "string",
                        "description": "输出PDF文件路径"
                    },
                    "pattern": {
                        "type": "string",
                        "default": "*_processed.png",
                        "description": "图片文件匹配模式（默认 *_processed.png）"
                    }
                })),
                required: Some(vec!["image_dir".to_string(), "output_path".to_string()]),
            },
        },
        Tool {
            name: "process_pdf".to_string(),
            title: None,
            description: Some("一键处理PDF：转换为图片 → 去除水印 → 合并回PDF。".to_string()),
            annotations: None,
            output_schema: None,
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(json!({
                    "pdf_path": {
                        "type": "string",
                        "description": "输入PDF文件路径"
                    },
                    "output_path": {
                        "type": "string",
                        "description": "输出PDF文件路径（可选，默认为 原文件名_nowatermark.pdf）"
                    },
                    "dpi": {
                        "type": "integer",
                        "default": 200,
                        "description": "处理图片的DPI（默认200）"
                    }
                })),
                required: Some(vec!["pdf_path".to_string()]),
            },
        },
    ]
}

/// Handle tool call requests
pub async fn handle_tool_call(request: CallToolRequestParams) -> Result<CallToolResult> {
    let arguments = request
        .arguments
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    match request.name.as_str() {
        "pdf_to_images" => handle_pdf_to_images(arguments).await,
        "remove_watermark" => handle_remove_watermark(arguments).await,
        "images_to_pdf" => handle_images_to_pdf(arguments).await,
        "process_pdf" => handle_process_pdf(arguments).await,
        _ => Err(anyhow::anyhow!("Unknown tool: {}", request.name)),
    }
}
