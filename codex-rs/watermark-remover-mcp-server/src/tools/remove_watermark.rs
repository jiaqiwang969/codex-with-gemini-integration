//! Remove Watermark tool - removes watermarks from images

use anyhow::Context;
use anyhow::Result;
use mcp_types::CallToolResult;
use mcp_types::ContentBlock;
use mcp_types::TextContent;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

#[derive(Deserialize)]
struct RemoveWatermarkArgs {
    image_path: Option<String>,
    image_dir: Option<String>,
    output_dir: Option<String>,
}

pub async fn handle_remove_watermark(args: serde_json::Value) -> Result<CallToolResult> {
    let args: RemoveWatermarkArgs = serde_json::from_value(args)?;

    // Validate arguments
    if args.image_path.is_none() && args.image_dir.is_none() {
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: "Error: Either image_path or image_dir must be provided".to_string(),
                annotations: None,
            })],
            is_error: Some(true),
            structured_content: None,
        });
    }

    let scripts_dir = get_scripts_dir()?;
    let script_path = scripts_dir.join("remove_watermark.py");

    let mut cmd = Command::new("python3");
    cmd.arg(&script_path);

    if let Some(image_path) = &args.image_path {
        let path = PathBuf::from(image_path);
        if !path.exists() {
            return Ok(CallToolResult {
                content: vec![ContentBlock::TextContent(TextContent {
                    r#type: "text".to_string(),
                    text: format!("Error: Image file not found: {image_path}"),
                    annotations: None,
                })],
                is_error: Some(true),
                structured_content: None,
            });
        }
        cmd.arg("--image").arg(image_path);
        info!("Removing watermark from image: {}", image_path);
    } else if let Some(image_dir) = &args.image_dir {
        let path = PathBuf::from(image_dir);
        if !path.exists() || !path.is_dir() {
            return Ok(CallToolResult {
                content: vec![ContentBlock::TextContent(TextContent {
                    r#type: "text".to_string(),
                    text: format!("Error: Directory not found: {image_dir}"),
                    annotations: None,
                })],
                is_error: Some(true),
                structured_content: None,
            });
        }
        cmd.arg("--dir").arg(image_dir);
        info!("Removing watermarks from directory: {}", image_dir);
    }

    if let Some(output_dir) = &args.output_dir {
        tokio::fs::create_dir_all(output_dir).await?;
        cmd.arg("--output").arg(output_dir);
    }

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute remove_watermark.py")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: format!("Error running remove_watermark.py: {stderr}"),
                annotations: None,
            })],
            is_error: Some(true),
            structured_content: None,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    Ok(CallToolResult {
        content: vec![ContentBlock::TextContent(TextContent {
            r#type: "text".to_string(),
            text: format!("Successfully removed watermarks.\n{stdout}"),
            annotations: None,
        })],
        is_error: Some(false),
        structured_content: None,
    })
}

fn get_scripts_dir() -> Result<PathBuf> {
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(parent) = exe_path.parent()
    {
        let possible_paths = vec![
            parent.join("../../../watermark-remover-mcp-server/scripts"),
            parent.join("../../watermark-remover-mcp-server/scripts"),
            parent.join("scripts"),
        ];

        for path in possible_paths {
            if path.exists() {
                return Ok(path.canonicalize()?);
            }
        }
    }

    if let Ok(scripts_dir) = std::env::var("WATERMARK_SCRIPTS_DIR") {
        return Ok(PathBuf::from(scripts_dir));
    }

    let cwd = std::env::current_dir()?;
    Ok(cwd.join("scripts"))
}
