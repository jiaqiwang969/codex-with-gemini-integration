//! Tool implementations for Hunyuan AI3D

mod download;
mod generate;
mod query;

use anyhow::Result;
use mcp_types::CallToolRequestParams;
use mcp_types::CallToolResult;
use mcp_types::Tool;
use mcp_types::ToolInputSchema;
use serde_json::json;

pub use download::handle_download;
pub use generate::handle_generate;
pub use query::handle_query;

/// Get tool definitions for MCP
pub fn get_tool_definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "hunyuan_generate_3d".to_string(),
            title: None,
            description: Some("生成3D模型。重要：用户粘贴图片后，不要传递image_url参数，也不要传递'[剪贴板图片]'等占位符！系统会自动从会话中提取图片。".to_string()),
            annotations: None,
            output_schema: None,
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(json!({
                    "prompt": {
                        "type": "string",
                        "description": "文本描述（仅用于纯文本生成）- 重要：有图片输入时不要传递此参数！prompt与image_url/image_base64互斥，不能同时存在！"
                    },
                    "image_url": {
                        "type": "string",
                        "description": "图片URL - 警告：1) 用户粘贴图片时不要传递此参数！2) 不要传递'[剪贴板图片]'！3) image_url和image_base64不能同时存在！系统会自动处理剪贴板图片。"
                    },
                    "image_base64": {
                        "type": "string",
                        "description": "Base64编码图片 - 直接提供图片的base64数据。注意：image_base64 和 image_url 不能同时存在，只能选择其一！"
                    },
                    "multi_view_images": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "view_type": {
                                    "type": "string",
                                    "enum": ["left", "right", "back"],
                                    "description": "视角类型 - left:左视图, right:右视图, back:后视图"
                                },
                                "view_image_url": {
                                    "type": "string",
                                    "description": "该视角的图片URL"
                                },
                                "view_image_base64": {
                                    "type": "string",
                                    "description": "该视角的Base64图片数据"
                                }
                            }
                        },
                        "description": "多视角图片 - Pro版本支持提供多个角度的参考图以生成更精确的模型"
                    },
                    "api_version": {
                        "type": "string",
                        "enum": ["pro", "rapid", "standard"],
                        "default": "pro",
                        "description": "API版本 - Pro:专业版3并发; Rapid:极速版1并发; Standard:通用版"
                    },
                    "generate_type": {
                        "type": "string",
                        "enum": ["Normal", "LowPoly", "Geometry", "Sketch"],
                        "description": "生成模式 - Normal:标准带纹理; LowPoly:低多边形风格; Geometry:白模无纹理; Sketch:草图模式(支持文字+图片)"
                    },
                    "enable_pbr": {
                        "type": "boolean",
                        "description": "是否启用PBR材质 - 生成更真实的金属、粗糙度、法线贴图等(Pro和Rapid都支持)"
                    },
                    "face_count": {
                        "type": "integer",
                        "minimum": 40000,
                        "maximum": 1500000,
                        "description": "模型面数限制 - 40K-80K:低模游戏资产; 80K-300K:中等细节; 300K-1.5M:高精度模型"
                    },
                    "polygon_type": {
                        "type": "string",
                        "enum": ["triangle", "quadrilateral"],
                        "description": "多边形类型 - triangle:三角面(通用); quadrilateral:四边面(更整洁) - 仅LowPoly模式有效"
                    },
                    "negative_prompt": {
                        "type": "string",
                        "description": "负面提示词 - 描述不想要的特征(注意：仅Rapid API支持，Pro版本不支持)"
                    },
                    "seed": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "随机种子 - 用于复现结果(注意：仅Rapid API支持，Pro版本不支持)"
                    },
                    "wait_for_completion": {
                        "type": "boolean",
                        "default": true,
                        "description": "是否等待任务完成并自动下载文件 - true:一站式完成; false:仅提交任务"
                    }
                })),
                required: Some(vec![]),
            },
        },
        Tool {
            name: "hunyuan_query_task".to_string(),
            title: None,
            description: Some("Query the status of a 3D generation task".to_string()),
            annotations: None,
            output_schema: None,
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(json!({
                    "job_id": {
                        "type": "string",
                        "description": "The job ID returned from hunyuan_generate_3d"
                    },
                    "api_version": {
                        "type": "string",
                        "enum": ["pro", "rapid"],
                        "default": "pro",
                        "description": "API version used for the job"
                    }
                })),
                required: Some(vec!["job_id".to_string()]),
            },
        },
        Tool {
            name: "hunyuan_download_results".to_string(),
            title: None,
            description: Some("下载生成的3D模型文件。注意：文件会自动保存到 /tmp/hunyuan-3d/ 目录，不要传递 output_dir 参数！".to_string()),
            annotations: None,
            output_schema: None,
            input_schema: ToolInputSchema {
                r#type: "object".to_string(),
                properties: Some(json!({
                    "job_id": {
                        "type": "string",
                        "description": "The job ID to download results for"
                    },
                    "api_version": {
                        "type": "string",
                        "enum": ["pro", "rapid"],
                        "default": "pro",
                        "description": "API version used for the job"
                    },
                })),
                required: Some(vec!["job_id".to_string()]),
            },
        },
    ]
}

/// Handle tool call requests
pub async fn handle_tool_call(
    request: CallToolRequestParams,
    secret_id: String,
    secret_key: String,
) -> Result<CallToolResult> {
    let arguments = request
        .arguments
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    match request.name.as_str() {
        "hunyuan_generate_3d" => handle_generate(arguments, secret_id, secret_key).await,
        "hunyuan_query_task" => handle_query(arguments, secret_id, secret_key).await,
        "hunyuan_download_results" => handle_download(arguments, secret_id, secret_key).await,
        _ => Err(anyhow::anyhow!("Unknown tool: {}", request.name)),
    }
}
