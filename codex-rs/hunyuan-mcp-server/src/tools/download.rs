//! Download results tool implementation

use anyhow::Context;
use anyhow::Result;
use mcp_types::CallToolResult;
use mcp_types::ContentBlock;
use mcp_types::TextContent;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use chrono::Local;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::debug;
use tracing::info;

use crate::models::ApiVersion;
use crate::tencent_cloud::TencentCloudClient;

#[derive(Debug, Deserialize)]
struct DownloadParams {
    job_id: String,
    api_version: Option<String>,
    output_dir: Option<String>,
}

pub async fn handle_download(
    arguments: serde_json::Value,
    secret_id: String,
    secret_key: String,
) -> Result<CallToolResult> {
    let params: DownloadParams =
        serde_json::from_value(arguments).context("Failed to parse download parameters")?;

    // Parse API version
    let api_version = match params.api_version.as_deref() {
        Some("rapid") => ApiVersion::Rapid,
        _ => ApiVersion::Pro,
    };

    // 创建带时间戳的输出目录
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let base_dir = params
        .output_dir
        .unwrap_or_else(|| "/tmp/hunyuan-3d".to_string());
    let output_dir = format!("{}/{}_{}_download", 
        base_dir, 
        timestamp,
        &params.job_id[..8.min(params.job_id.len())]
    );

    // Create client and download
    let client = TencentCloudClient::new(secret_id, secret_key)?;
    let files = download_results(&params.job_id, api_version, &output_dir, &client).await?;

    // Format response
    let response_text = if files.is_empty() {
        format!(
            "⚠️ No files found for job {}\n\n\
            The job may still be processing or may have failed.",
            params.job_id
        )
    } else {
        let mut text = format!(
            "✅ Successfully downloaded files for job {}\n\n\
            **Output Directory**: {}\n\
            **Files Downloaded**: {}\n\n",
            params.job_id,
            output_dir,
            files.len()
        );

        for file in &files {
            text.push_str(&format!("  - {}\n", file));
        }

        text
    };

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

/// Download results from a completed job
pub async fn download_results(
    job_id: &str,
    api_version: ApiVersion,
    output_dir: &str,
    client: &TencentCloudClient,
) -> Result<Vec<String>> {
    // Query job to get URLs
    let status = client.query_job(job_id, api_version).await?;

    let status_lower = status.status.to_lowercase();
    if !(status_lower == "success" || status_lower == "completed" || status_lower == "finish" || status_lower == "done") {
        info!("Job {} is not complete yet, status: {}", job_id, status.status);
        return Ok(Vec::new());
    }

    // Create output directory
    let output_path = PathBuf::from(output_dir);
    fs::create_dir_all(&output_path)
        .await
        .context("Failed to create output directory")?;

    let mut downloaded_files = Vec::new();

    // Download preview if available
    if let Some(preview_url) = status.preview_url {
        if let Ok(file) = download_file(
            &preview_url,
            &output_path,
            &format!("{}_preview.png", job_id),
        )
        .await
        {
            downloaded_files.push(file);
        }
    }

    // Download result files
    if let Some(result_urls) = status.result_urls {
        for (i, url) in result_urls.iter().enumerate() {
            let ext = detect_extension(url);
            let filename = format!("{}_result_{}.{}", job_id, i, ext);
            if let Ok(file) = download_file(url, &output_path, &filename).await {
                downloaded_files.push(file);
            }
        }
    }

    // Download 3D files
    if let Some(files) = status.result_file3_d_s {
        for file in files {
            // Detect the actual file extension from URL
            let ext = if file.url.contains(".zip") {
                "zip"
            } else {
                &file.file_type.to_lowercase()
            };
            
            let filename = format!(
                "{}_{}.{}",
                job_id,
                file.file_type.to_lowercase(),
                ext
            );
            
            info!("Downloading {} file from {}", file.file_type, file.url);
            if let Ok(downloaded) = download_file(&file.url, &output_path, &filename).await {
                downloaded_files.push(downloaded);
            }
            
            // Also download preview if available
            if let Some(preview) = &file.preview_image_url {
                let preview_filename = format!("{}_{}_preview.png", job_id, file.file_type.to_lowercase());
                if let Ok(downloaded) = download_file(preview, &output_path, &preview_filename).await {
                    downloaded_files.push(downloaded);
                }
            }
        }
    }

    // If we got a ZIP file, extract it
    for file in downloaded_files.clone() {
        if file.ends_with(".zip") {
            if let Ok(extracted) = extract_zip(&file, &output_path).await {
                info!("Extracted {} files from {}", extracted.len(), file);
                downloaded_files.extend(extracted);
            }
        }
    }

    Ok(downloaded_files)
}

pub async fn download_file(url: &str, output_dir: &Path, filename: &str) -> Result<String> {
    debug!("Downloading {} to {}", url, filename);

    let response = reqwest::get(url).await.context("Failed to download file")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to download: HTTP {}",
            response.status()
        ));
    }

    let bytes = response.bytes().await?;
    let file_path = output_dir.join(filename);

    let mut file = fs::File::create(&file_path).await?;
    file.write_all(&bytes).await?;

    Ok(file_path.to_string_lossy().to_string())
}

pub async fn extract_zip(zip_path: &str, output_dir: &Path) -> Result<Vec<String>> {
    // Run the zip extraction in a blocking task to avoid Send issues
    let zip_path = zip_path.to_string();
    let output_dir = output_dir.to_path_buf();
    
    tokio::task::spawn_blocking(move || {
        use std::io::Cursor;
        use std::io::Read;
        
        let zip_data = std::fs::read(&zip_path)?;
        let cursor = Cursor::new(zip_data);
        
        let mut archive = zip::ZipArchive::new(cursor)?;
        let mut extracted_files = Vec::new();
        
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let file_path = output_dir.join(file.name());
            
            if file.is_dir() {
                std::fs::create_dir_all(&file_path)?;
            } else {
                if let Some(parent) = file_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                
                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)?;
                std::fs::write(&file_path, buffer)?;
                
                extracted_files.push(file_path.to_string_lossy().to_string());
            }
        }
        
        Ok::<Vec<String>, anyhow::Error>(extracted_files)
    })
    .await?
}

fn detect_extension(url: &str) -> &str {
    if url.contains(".zip") {
        "zip"
    } else if url.contains(".glb") {
        "glb"
    } else if url.contains(".fbx") {
        "fbx"
    } else if url.contains(".obj") {
        "obj"
    } else if url.contains(".usdz") {
        "usdz"
    } else if url.contains(".png") {
        "png"
    } else if url.contains(".jpg") || url.contains(".jpeg") {
        "jpg"
    } else {
        "bin"
    }
}
