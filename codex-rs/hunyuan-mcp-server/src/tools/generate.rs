//! Generate 3D model tool implementation

use anyhow::Context;
use anyhow::Result;
use mcp_types::CallToolResult;
use mcp_types::ContentBlock;
use mcp_types::TextContent;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::image_utils::ImageSource;
use crate::image_utils::{self};
use crate::models::ApiVersion;
use crate::models::Generate3DRequest;
use crate::models::GenerateType;
use crate::models::PolygonType;
use crate::models::ViewImage;
use crate::tencent_cloud::TencentCloudClient;
use crate::tools::download::download_results;

#[derive(Debug, Deserialize)]
struct GenerateParams {
    prompt: Option<String>,
    image_url: Option<String>,
    image_base64: Option<String>,
    multi_view_images: Option<Vec<ViewImageParam>>,
    output_format: Option<String>,
    api_version: Option<String>,
    generate_type: Option<String>,
    enable_pbr: Option<bool>,
    face_count: Option<i32>,
    polygon_type: Option<String>,
    negative_prompt: Option<String>,
    seed: Option<i32>,
    wait_for_completion: Option<bool>,
    output_dir: Option<String>,
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
        Some("standard") => ApiVersion::Standard,
        _ => ApiVersion::Pro, // Default to Pro (more features and parameters)
    };

    // Build request
    let mut request = Generate3DRequest {
        prompt: params.prompt.clone(),
        image_base64: None,
        image_url: None,
        multi_view_images: None,
        output_format: params.output_format.or_else(|| Some("obj".to_string())), // Default to OBJ
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

    // Wait for completion if requested
    let wait_for_completion = params.wait_for_completion.unwrap_or(true);
    if wait_for_completion {
        response_text.push_str("\n⏳ Waiting for job to complete...\n");

        let max_wait = Duration::from_secs(600); // 10 minutes
        let poll_interval = Duration::from_secs(10);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > max_wait {
                response_text.push_str("\n⚠️ Job timed out after 10 minutes. You can check status later with hunyuan_query_task.\n");
                break;
            }

            sleep(poll_interval).await;

            match client.query_job(&job_id, api_version).await {
                Ok(status) => {
                    debug!("Job {} status: {}", job_id, status.status);

                    let status_lower = status.status.to_lowercase();
                    if status_lower == "success"
                        || status_lower == "completed"
                        || status_lower == "finish"
                    {
                        response_text.push_str(&format!("\n✅ Job completed successfully!\n"));

                        // Auto-download results
                        if let Some(output_dir) = params.output_dir.as_ref() {
                            match download_results(&job_id, api_version, output_dir, &client).await
                            {
                                Ok(files) => {
                                    response_text.push_str(&format!(
                                        "\n📦 Downloaded {} files to {}/\n",
                                        files.len(),
                                        output_dir
                                    ));
                                    for file in files {
                                        response_text.push_str(&format!("  - {}\n", file));
                                    }
                                }
                                Err(e) => {
                                    response_text.push_str(&format!(
                                        "\n⚠️ Failed to download files: {}\n",
                                        e
                                    ));
                                }
                            }
                        }
                        break;
                    } else if status_lower == "failed"
                        || status_lower == "error"
                        || status_lower == "timeout"
                    {
                        let error_msg = status
                            .error_msg
                            .unwrap_or_else(|| "Unknown error".to_string());
                        response_text.push_str(&format!("\n❌ Job failed: {}\n", error_msg));
                        break;
                    }
                }
                Err(e) => {
                    debug!("Query error (will retry): {}", e);
                }
            }
        }
    } else {
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

                    if status_lower == "done"
                        || status_lower == "success"
                        || status_lower == "completed"
                        || status_lower == "finish"
                    {
                        final_status = Some(status);
                        break;
                    } else if status_lower == "failed"
                        || status_lower == "error"
                        || status_lower == "timeout"
                    {
                        let error_msg = status
                            .error_msg
                            .or(status.error_message)
                            .unwrap_or_else(|| "Unknown error".to_string());
                        response_text = format!("❌ 3D生成失败\n\n**错误信息**: {}", error_msg);

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

            // 创建输出目录 - 使用时间戳和描述创建唯一目录
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let base_dir = params
                .output_dir
                .clone()
                .unwrap_or_else(|| "/tmp/hunyuan-3d".to_string());

            // 从prompt中提取简短描述作为目录名的一部分
            let desc = if let Some(prompt) = &params.prompt {
                // 取前20个字符，移除特殊字符
                let clean: String = prompt
                    .chars()
                    .take(20)
                    .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
                    .collect::<String>()
                    .trim()
                    .replace(' ', "_");
                if !clean.is_empty() {
                    format!("_{}", clean)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            let output_dir = format!(
                "{}/{}_{}{}",
                base_dir,
                timestamp,
                &job_id[..8], // 使用JobID的前8位
                desc
            );
            let output_path = std::path::PathBuf::from(&output_dir);
            tokio::fs::create_dir_all(&output_path).await?;

            let mut downloaded_files = Vec::new();

            // 下载3D文件
            if let Some(files) = status.result_file3_d_s {
                for file in files {
                    // 下载预览图
                    if let Some(preview) = &file.preview_image_url {
                        response_text
                            .push_str(&format!("\n🖼️ **预览图**: [查看预览]({})\n", preview));
                    }

                    // 下载模型文件
                    let ext = if file.url.contains(".zip") {
                        "zip"
                    } else {
                        &file.file_type.to_lowercase()
                    };
                    let filename = format!("{}_{}.{}", job_id, file.file_type.to_lowercase(), ext);

                    match crate::tools::download::download_file(&file.url, &output_path, &filename)
                        .await
                    {
                        Ok(downloaded_path) => {
                            downloaded_files.push(downloaded_path.clone());

                            // 如果是ZIP文件，解压它
                            if downloaded_path.ends_with(".zip") {
                                if let Ok(extracted) = crate::tools::download::extract_zip(
                                    &downloaded_path,
                                    &output_path,
                                )
                                .await
                                {
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
                response_text.push_str(&format!(
                    "\n📁 **下载的文件** (保存在 `{}`目录):\n",
                    output_dir
                ));
                for file in &downloaded_files {
                    if let Some(filename) = std::path::Path::new(&file).file_name() {
                        response_text.push_str(&format!("  - {}\n", filename.to_string_lossy()));
                    }
                }

                // 特别标注主要的3D文件
                for file in &downloaded_files {
                    if file.ends_with(".obj")
                        || file.ends_with(".glb")
                        || file.ends_with(".fbx")
                        || file.ends_with(".usdz")
                    {
                        response_text.push_str(&format!("\n🎯 **3D模型文件**: `{}`\n", file));
                        break;
                    }
                }
            }

            response_text.push_str(&format!(
                "\n⏱️ **生成用时**: 约{}秒",
                start_time.elapsed().as_secs()
            ));
        } else {
            response_text.push_str(&format!(
                "\n⏱️ 任务处理超时（已等待{}秒）\n",
                max_wait_time.as_secs()
            ));
            response_text.push_str(&format!("您可以稍后使用Job ID查询: {}", job_id));
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
