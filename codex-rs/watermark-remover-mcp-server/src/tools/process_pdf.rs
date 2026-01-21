//! Process PDF tool - convert to images and remove watermarks

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
struct ProcessPdfArgs {
    pdf_path: String,
    images_output_dir: String,
    dpi: Option<u32>,
}

pub async fn handle_process_pdf(args: serde_json::Value) -> Result<CallToolResult> {
    let args: ProcessPdfArgs = serde_json::from_value(args)?;

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

    let output_dir = PathBuf::from(&args.images_output_dir);
    let dpi = args.dpi.unwrap_or(200);

    info!(
        "Processing PDF: {} -> images in {}",
        args.pdf_path,
        output_dir.display()
    );

    // Create output directory
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: format!("Error creating output directory: {e}"),
                annotations: None,
            })],
            is_error: Some(true),
            structured_content: None,
        });
    }

    let scripts_dir = get_scripts_dir()?;
    let script_path = scripts_dir.join("process_pdf_to_images.py");

    let output = Command::new("python3")
        .arg(&script_path)
        .arg(&args.pdf_path)
        .arg(output_dir.to_string_lossy().to_string())
        .arg(dpi.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute process_pdf_to_images.py")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(CallToolResult {
            content: vec![ContentBlock::TextContent(TextContent {
                r#type: "text".to_string(),
                text: format!("Error running process_pdf_to_images.py: {stderr}"),
                annotations: None,
            })],
            is_error: Some(true),
            structured_content: None,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Count output images
    let image_count = std::fs::read_dir(&output_dir)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "png")
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);

    Ok(CallToolResult {
        content: vec![ContentBlock::TextContent(TextContent {
            r#type: "text".to_string(),
            text: format!(
                "Successfully processed PDF and removed watermarks!\n\nImages output directory: {}\nTotal images: {}\n\n{}",
                output_dir.display(),
                image_count,
                stdout
            ),
            annotations: None,
        })],
        is_error: Some(false),
        structured_content: None,
    })
}

fn get_scripts_dir() -> Result<PathBuf> {
    // First check environment variable
    if let Ok(scripts_dir) = std::env::var("WATERMARK_SCRIPTS_DIR") {
        let path = PathBuf::from(&scripts_dir);
        if path.exists() {
            return Ok(path);
        }
    }

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

    let cwd = std::env::current_dir()?;
    Ok(cwd.join("scripts"))
}
