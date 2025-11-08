//! Generate 3D model tool implementation

use anyhow::Context;
use anyhow::Result;
use mcp_types::CallToolResult;
use mcp_types::ContentBlock;
use mcp_types::TextContent;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::image_utils::ImageSource;
use crate::image_utils::{self};
use crate::models::ApiVersion;
use crate::models::Generate3DRequest;
use crate::models::GenerateType;
use crate::models::PolygonType;
use crate::models::ViewImage;
use crate::tencent_cloud::TencentCloudClient;

async fn append_jsonl_event(base_dir: &str, job_id: &str, event: serde_json::Value) -> anyhow::Result<()> {
    use tokio::fs::OpenOptions;
    use tokio::io::AsyncWriteExt;
    let log_root = std::path::Path::new(base_dir).join("logs");
    tokio::fs::create_dir_all(&log_root).await?;
    let path = log_root.join(format!("{}.jsonl", job_id));
    let mut f = OpenOptions::new().create(true).append(true).open(path).await?;
    let mut line = serde_json::to_string(&event)?;
    line.push('\n');
    f.write_all(line.as_bytes()).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GenerateParams {
    prompt: Option<String>,
    image_url: Option<String>,
    image_base64: Option<String>,
    multi_view_images: Option<Vec<ViewImageParam>>,
    api_version: Option<String>,
    generate_type: Option<String>,
    enable_pbr: Option<bool>,
    face_count: Option<i32>,
    polygon_type: Option<String>,
    negative_prompt: Option<String>,
    seed: Option<i32>,
    wait_for_completion: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ViewImageParam {
    view_type: String,
    view_image_url: Option<String>,
    view_image_base64: Option<String>,
}

pub async fn handle_generate(
    arguments: serde_json::Value,
    secret_id: String,
    secret_key: String,
) -> Result<CallToolResult> {
    let params: GenerateParams =
        serde_json::from_value(arguments).context("Failed to parse generate parameters")?;

    // Parse API version
    let api_version = match params.api_version.as_deref() {
        Some("rapid") => ApiVersion::Rapid,
        Some("pro") => ApiVersion::Pro,
        _ => ApiVersion::Rapid, // Default to Rapid (supports OutputFormat)
    };

    // Build request
    let mut request = Generate3DRequest {
        prompt: params.prompt.clone(),
        image_base64: None,
        image_url: None,
        multi_view_images: None,
        // 仅允许 OBJ，强制为 OBJ（返回官方 ZIP 打包直链）
        output_format: Some("OBJ".to_string()),
        enable_pbr: params.enable_pbr,
        face_count: params.face_count,
        generate_type: None,
        polygon_type: None,
        negative_prompt: params.negative_prompt,
        seed: params.seed,
    };

    // Handle image inputs
    if let Some(url) = params.image_url {
        // Check if it's a data URL or needs conversion
        let source = ImageSource::detect(&url);
        match source {
            ImageSource::DataUrl(_) => {
                // Extract base64 from data URL
                if let Some(base64) = image_utils::extract_base64_from_data_url(&url) {
                    request.image_base64 = Some(base64);
                } else {
                    request.image_url = Some(url);
                }
            }
            _ => {
                // Convert to base64
                let base64 = image_utils::to_base64(source).await?;
                request.image_base64 = Some(base64);
            }
        }
    } else if let Some(base64) = params.image_base64 {
        request.image_base64 = Some(base64);
    }

    // Handle multi-view images
    if let Some(views) = params.multi_view_images {
        let mut converted_views = Vec::new();
        for view in views {
            let mut converted = ViewImage {
                view_type: view.view_type,
                view_image_url: None,
                view_image_base64: None,
            };

            if let Some(url) = view.view_image_url {
                let source = ImageSource::detect(&url);
                let base64 = image_utils::to_base64(source).await?;
                converted.view_image_base64 = Some(base64);
            } else if let Some(base64) = view.view_image_base64 {
                converted.view_image_base64 = Some(base64);
            }

            converted_views.push(converted);
        }
        request.multi_view_images = Some(converted_views);
    }

    // Parse generate type
    if let Some(gen_type) = params.generate_type {
        request.generate_type = match gen_type.as_str() {
            "LowPoly" => Some(GenerateType::LowPoly),
            "Geometry" => Some(GenerateType::Geometry),
            "Sketch" => Some(GenerateType::Sketch),
            _ => Some(GenerateType::Normal),
        };
    }

    // Parse polygon type
    if let Some(poly_type) = params.polygon_type {
        request.polygon_type = match poly_type.as_str() {
            "quadrilateral" => Some(PolygonType::Quadrilateral),
            _ => Some(PolygonType::Triangle),
        };
    }

    // Auto-detect mode
    let has_text = request.prompt.is_some();
    let has_image = request.image_base64.is_some() || request.image_url.is_some();

    if has_text && has_image && request.generate_type.is_none() {
        // Auto-use Sketch mode for combined input
        request.generate_type = Some(GenerateType::Sketch);
        info!("Auto-detected combined text+image input, using Sketch mode");
    }

    // Create client and submit job
    let client = TencentCloudClient::new(secret_id, secret_key)?;
    let submit_response = client.submit_job(request, api_version).await?;
    let job_id = submit_response.job_id.clone();

    info!("Submitted job: {}", job_id);
    // 强制使用 /tmp/hunyuan-3d 作为输出目录（不允许 MCP 自定义）
    let base_output_dir = "/tmp/hunyuan-3d".to_string();
    let _ = append_jsonl_event(
        &base_output_dir,
        &job_id,
        json!({
            "event":"submitted",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "job_id": job_id,
            "api_version": match api_version { ApiVersion::Rapid=>"rapid", ApiVersion::Pro=>"pro", ApiVersion::Standard=>"standard" },
            "prompt": params.prompt,
            "output_format": "OBJ",
            "output_dir": "/tmp/hunyuan-3d"
        })
    ).await;

    // Prepare initial response
    let mut response_text = format!(
        "✅ Successfully submitted 3D generation job\n\n\
        **Job ID**: {}\n\
        **API Version**: {:?}\n",
        job_id, api_version
    );

    if has_text {
        response_text.push_str(&format!(
            "**Prompt**: {}\n",
            params.prompt.as_ref().unwrap()
        ));
    }
    if has_image {
        response_text.push_str("**Image**: Provided\n");
    }
    // 显式告知输出格式为 OBJ
    response_text.push_str("**输出格式**: OBJ\n");

    // 若不等待，直接返回
    if !params.wait_for_completion.unwrap_or(true) {
        response_text.push_str("\n💡 Job submitted. Use hunyuan_query_task to check status.\n");
    }

    // 如果 wait_for_completion 参数为 true，自动轮询并下载
    if params.wait_for_completion.unwrap_or(true) {
        info!("Auto-polling enabled, waiting for job completion...");
        
        // 轮询任务状态
        let max_wait_time = std::time::Duration::from_secs(300); // 最多等待5分钟
        let poll_interval = std::time::Duration::from_secs(5); // 每5秒查询一次
        let start_time = std::time::Instant::now();
        
        let mut final_status = None;
        
        while start_time.elapsed() < max_wait_time {
            tokio::time::sleep(poll_interval).await;
            
            match client.query_job(&job_id, api_version).await {
                Ok(status) => {
                    let status_lower = status.status.to_lowercase();
                    info!("Job {} status: {}", job_id, status.status);
                    let _ = append_jsonl_event(
                        &base_output_dir,
                        &job_id,
                        json!({
                            "event":"status",
                            "timestamp": chrono::Utc::now().to_rfc3339(),
                            "job_id": job_id,
                            "status": status.status
                        })
                    ).await;
                    
                    if status_lower == "done" || status_lower == "succ" || status_lower == "success" || status_lower == "completed" || status_lower == "finish" {
                        final_status = Some(status);
                        break;
                    } else if status_lower == "failed" || status_lower == "error" || status_lower == "timeout" {
                        let error_msg = status.error_msg
                            .or(status.error_message)
                            .unwrap_or_else(|| "Unknown error".to_string());
                        response_text = format!("❌ 3D生成失败\n\n**错误信息**: {}", error_msg);
                        let _ = append_jsonl_event(
                            &base_output_dir,
                            &job_id,
                            json!({"event":"failed","timestamp": chrono::Utc::now().to_rfc3339(),"job_id": job_id,"error": error_msg})
                        ).await;
                        
                        return Ok(CallToolResult {
                            content: vec![ContentBlock::TextContent(TextContent {
                                r#type: "text".to_string(),
                                text: response_text,
                                annotations: None,
                            })],
                            is_error: Some(true),
                            structured_content: None,
                        });
                    }
                    // 继续等待 processing/pending 状态
                }
                Err(e) => {
                    warn!("Failed to query job status: {}", e);
                    // 继续重试
                }
            }
        }
        
        // 如果任务完成，自动下载文件
        if let Some(status) = final_status {
            response_text = format!("✅ 3D模型生成成功！\n\n");
            
            if let Some(prompt) = &params.prompt {
                response_text.push_str(&format!("**描述**: {}\n", prompt));
            }
            let _ = append_jsonl_event(
                &base_output_dir,
                &job_id,
                json!({"event":"completed","timestamp": chrono::Utc::now().to_rfc3339(),"job_id": job_id})
            ).await;
            
            // 创建输出目录
            let output_dir = base_output_dir.clone();
            let output_path = std::path::PathBuf::from(&output_dir);
            tokio::fs::create_dir_all(&output_path).await?;
            
            let mut downloaded_files = Vec::new();
            
            // 下载3D文件
            if let Some(files) = status.result_file3_d_s {
                for file in files {
                    // 下载预览图
                    if let Some(preview) = &file.preview_image_url {
                        response_text.push_str(&format!("\n🖼️ **预览图**: [查看预览]({})\n", preview));
                    }
                    
                    // 下载模型文件
                    let ext = if file.url.contains(".zip") { "zip" } else { &file.file_type.to_lowercase() };
                    let filename = format!("{}_{}.{}", job_id, file.file_type.to_lowercase(), ext);
                    
                    match crate::tools::download::download_file(&file.url, &output_path, &filename).await {
                        Ok(downloaded_path) => {
                            downloaded_files.push(downloaded_path.clone());
                            
                            // 如果是ZIP文件，解压它
                            if downloaded_path.ends_with(".zip") {
                                if let Ok(extracted) = crate::tools::download::extract_zip(&downloaded_path, &output_path).await {
                                    downloaded_files.extend(extracted);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to download file: {}", e);
                        }
                    }
                }
            }
            
            // 显示下载的文件
            if !downloaded_files.is_empty() {
                response_text.push_str(&format!("\n📁 **下载的文件** (保存在 `{}`目录):\n", output_dir));
                for file in &downloaded_files {
                    if let Some(filename) = std::path::Path::new(&file).file_name() {
                        response_text.push_str(&format!("  - {}\n", filename.to_string_lossy()));
                    }
                }
                let _ = append_jsonl_event(
                    &base_output_dir,
                    &job_id,
                    json!({"event":"downloaded","timestamp": chrono::Utc::now().to_rfc3339(),"job_id": job_id,"files": downloaded_files})
                ).await;
                
                // 特别标注主要的3D文件
                for file in &downloaded_files {
                    if file.ends_with(".obj") || file.ends_with(".glb") || file.ends_with(".fbx") || file.ends_with(".usdz") {
                        response_text.push_str(&format!("\n🎯 **3D模型文件**: `{}`\n", file));
                        break;
                    }
                }
            }
            
            response_text.push_str(&format!("\n⏱️ **生成用时**: 约{}秒", start_time.elapsed().as_secs()));
        } else {
            response_text.push_str(&format!("\n⏱️ 任务处理超时（已等待{}秒）\n", max_wait_time.as_secs()));
            response_text.push_str(&format!("您可以稍后使用Job ID查询: {}", job_id));
            let _ = append_jsonl_event(
                &base_output_dir,
                &job_id,
                json!({"event":"timeout","timestamp": chrono::Utc::now().to_rfc3339(),"job_id": job_id,"max_wait_secs": max_wait_time.as_secs()})
            ).await;
        }
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
