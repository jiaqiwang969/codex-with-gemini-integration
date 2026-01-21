//! PDF to Images tool - converts PDF pages to PNG images

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
struct PdfToImagesArgs {
    pdf_path: String,
    output_dir: Option<String>,
    dpi: Option<u32>,
}

pub async fn handle_pdf_to_images(args: serde_json::Value) -> Result<CallToolResult> {
    let args: PdfToImagesArgs = serde_json::from_value(args)?;

    let pdf_path = PathBuf::from(&args.pdf_path);
    if !pdf_path.exists() {
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: format!("Error: PDF file not found: {}", args.pdf_path),
                annotations: None,
            })],
            is_error: Some(true),
            structured_content: None,
        });
    }

    let dpi = args.dpi.unwrap_or(200);

    // Determine output directory
    let output_dir = if let Some(dir) = args.output_dir {
        PathBuf::from(dir)
    } else {
        let stem = pdf_path.file_stem().unwrap_or_default().to_string_lossy();
        pdf_path
            .parent()
            .unwrap_or(&pdf_path)
            .join(format!("{stem}_pages"))
    };

    // Create output directory
    tokio::fs::create_dir_all(&output_dir).await?;

    info!(
        "Converting PDF to images: {} -> {:?}",
        args.pdf_path, output_dir
    );

    // Get the scripts directory (relative to the binary)
    let scripts_dir = get_scripts_dir()?;
    let script_path = scripts_dir.join("pdf_to_images.py");

    // Run Python script
    let output = Command::new("python3")
        .arg(&script_path)
        .arg(&args.pdf_path)
        .arg(output_dir.to_string_lossy().to_string())
        .arg(dpi.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute pdf_to_images.py")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: format!("Error running pdf_to_images.py: {stderr}"),
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
            text: format!(
                "Successfully converted PDF to images.\nOutput directory: {}\n{}",
                output_dir.display(),
                stdout
            ),
            annotations: None,
        })],
        is_error: Some(false),
        structured_content: None,
    })
}

fn get_scripts_dir() -> Result<PathBuf> {
    // Try to find scripts directory relative to the executable
    if let Ok(exe_path) = std::env::current_exe() {
        // In development: executable is in target/debug or target/release
        // Scripts are in watermark-remover-mcp-server/scripts
        if let Some(parent) = exe_path.parent() {
            // Check if we're in target directory
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
    }

    // Fallback: check environment variable
    if let Ok(scripts_dir) = std::env::var("WATERMARK_SCRIPTS_DIR") {
        return Ok(PathBuf::from(scripts_dir));
    }

    // Last resort: current directory
    let cwd = std::env::current_dir()?;
    Ok(cwd.join("scripts"))
}
