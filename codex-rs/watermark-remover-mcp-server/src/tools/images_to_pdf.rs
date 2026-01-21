//! Images to PDF tool - merges images into a PDF

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
struct ImagesToPdfArgs {
    image_dir: String,
    output_path: String,
    pattern: Option<String>,
}

pub async fn handle_images_to_pdf(args: serde_json::Value) -> Result<CallToolResult> {
    let args: ImagesToPdfArgs = serde_json::from_value(args)?;

    let image_dir = PathBuf::from(&args.image_dir);
    if !image_dir.exists() || !image_dir.is_dir() {
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: format!("Error: Directory not found: {}", args.image_dir),
                annotations: None,
            })],
            is_error: Some(true),
            structured_content: None,
        });
    }

    let pattern = args.pattern.unwrap_or_else(|| "*.png".to_string());

    info!(
        "Merging images to PDF: {} -> {}",
        args.image_dir, args.output_path
    );

    let scripts_dir = get_scripts_dir()?;
    let script_path = scripts_dir.join("images_to_pdf.py");

    let output = Command::new("python3")
        .arg(&script_path)
        .arg(&args.image_dir)
        .arg(&args.output_path)
        .arg(&pattern)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute images_to_pdf.py")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: format!("Error running images_to_pdf.py: {stderr}"),
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
            text: format!("Successfully created PDF: {}\n{}", args.output_path, stdout),
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
