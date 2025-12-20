use async_trait::async_trait;
use base64::Engine;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use tokio::fs;

use crate::function_tool::FunctionCallError;
use crate::protocol::EventMsg;
use crate::protocol::ViewImageToolCallEvent;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ViewImageHandler;

#[derive(Deserialize)]
struct ViewImageArgs {
    path: String,
}

/// Supported image MIME types for multimodal function responses.
fn get_mime_type(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "heic" => Some("image/heic"),
        "heif" => Some("image/heif"),
        _ => None,
    }
}

#[async_trait]
impl ToolHandler for ViewImageHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "view_image handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ViewImageArgs = serde_json::from_str(&arguments).map_err(|e| {
            FunctionCallError::RespondToModel(format!("failed to parse function arguments: {e:?}"))
        })?;

        let abs_path = turn.resolve_path(Some(args.path));

        let metadata = fs::metadata(&abs_path).await.map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "unable to locate image at `{}`: {error}",
                abs_path.display()
            ))
        })?;

        if !metadata.is_file() {
            return Err(FunctionCallError::RespondToModel(format!(
                "image path `{}` is not a file",
                abs_path.display()
            )));
        }

        // Get MIME type for the image
        let mime_type = get_mime_type(&abs_path);

        // Read the image file
        let image_data = fs::read(&abs_path).await.map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "failed to read image at `{}`: {error}",
                abs_path.display()
            ))
        })?;

        let file_size_kb = image_data.len() / 1024;

        // Build content_items for multimodal function response (Gemini 3)
        let content_items = if let Some(mime) = mime_type {
            let base64_data = base64::engine::general_purpose::STANDARD.encode(&image_data);
            let data_url = format!("data:{};base64,{}", mime, base64_data);
            Some(vec![
                FunctionCallOutputContentItem::InputText {
                    text: format!("Image file: {} ({} KB)", abs_path.display(), file_size_kb),
                },
                FunctionCallOutputContentItem::InputImage { image_url: data_url },
            ])
        } else {
            None
        };

        // Also inject image via user input for compatibility with GPT and older Gemini models
        // This ensures the image is available in the conversation context regardless of model
        let _ = session
            .inject_input(vec![UserInput::LocalImage {
                path: abs_path.clone(),
            }])
            .await;

        // Send event for UI display
        session
            .send_event(
                turn.as_ref(),
                EventMsg::ViewImageToolCall(ViewImageToolCallEvent {
                    call_id,
                    path: abs_path.clone(),
                }),
            )
            .await;

        // Return response with both text content and optional multimodal items
        let content_text = if mime_type.is_some() {
            format!(
                "Viewed image: {} ({} KB, {})",
                abs_path.display(),
                file_size_kb,
                mime_type.unwrap_or("unknown")
            )
        } else {
            format!(
                "Viewed image: {} ({} KB) - unsupported format for inline display",
                abs_path.display(),
                file_size_kb
            )
        };

        Ok(ToolOutput::Function {
            content: content_text,
            content_items,
            success: Some(true),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex::make_session_and_context;
    use crate::tools::context::ToolInvocation;
    use crate::turn_diff_tracker::TurnDiffTracker;
    use codex_protocol::models::FunctionCallOutputContentItem;
    use pretty_assertions::assert_eq;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use tokio::sync::Mutex;

    /// Helper to create a test PNG file (minimal valid PNG)
    fn create_test_png() -> anyhow::Result<NamedTempFile> {
        let mut temp = NamedTempFile::with_suffix(".png")?;
        // Minimal valid PNG: 8-byte signature + IHDR chunk + IEND chunk
        let png_data: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // IHDR length
            0x49, 0x48, 0x44, 0x52, // IHDR type
            0x00, 0x00, 0x00, 0x01, // width: 1
            0x00, 0x00, 0x00, 0x01, // height: 1
            0x08, 0x02, // bit depth: 8, color type: 2 (RGB)
            0x00, 0x00, 0x00, // compression, filter, interlace
            0x90, 0x77, 0x53, 0xDE, // CRC
            0x00, 0x00, 0x00, 0x00, // IEND length
            0x49, 0x45, 0x4E, 0x44, // IEND type
            0xAE, 0x42, 0x60, 0x82, // CRC
        ];
        temp.write_all(&png_data)?;
        temp.flush()?;
        Ok(temp)
    }

    /// Helper to create a test JPEG file (minimal valid JPEG)
    fn create_test_jpeg() -> anyhow::Result<NamedTempFile> {
        let mut temp = NamedTempFile::with_suffix(".jpg")?;
        // Minimal valid JPEG: SOI + APP0 + minimal data + EOI
        let jpeg_data: Vec<u8> = vec![
            0xFF, 0xD8, // SOI (Start of Image)
            0xFF, 0xE0, // APP0 marker
            0x00, 0x10, // APP0 length
            0x4A, 0x46, 0x49, 0x46, 0x00, // "JFIF\0"
            0x01, 0x01, // version
            0x00, // aspect ratio units
            0x00, 0x01, // X density
            0x00, 0x01, // Y density
            0x00, 0x00, // thumbnail dimensions
            0xFF, 0xD9, // EOI (End of Image)
        ];
        temp.write_all(&jpeg_data)?;
        temp.flush()?;
        Ok(temp)
    }

    #[test]
    fn test_get_mime_type_png() {
        let path = std::path::Path::new("test.png");
        assert_eq!(get_mime_type(path), Some("image/png"));
    }

    #[test]
    fn test_get_mime_type_jpeg() {
        let path = std::path::Path::new("test.jpg");
        assert_eq!(get_mime_type(path), Some("image/jpeg"));

        let path = std::path::Path::new("test.jpeg");
        assert_eq!(get_mime_type(path), Some("image/jpeg"));
    }

    #[test]
    fn test_get_mime_type_webp() {
        let path = std::path::Path::new("test.webp");
        assert_eq!(get_mime_type(path), Some("image/webp"));
    }

    #[test]
    fn test_get_mime_type_unsupported() {
        let path = std::path::Path::new("test.bmp");
        assert_eq!(get_mime_type(path), None);

        let path = std::path::Path::new("test.txt");
        assert_eq!(get_mime_type(path), None);
    }

    #[tokio::test]
    async fn test_view_image_returns_multimodal_content_items_for_png() -> anyhow::Result<()> {
        let temp_png = create_test_png()?;
        let (session, turn_context) = make_session_and_context().await;

        let invocation = ToolInvocation {
            session: Arc::new(session),
            turn: Arc::new(turn_context),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::default())),
            call_id: "test-call-1".to_string(),
            tool_name: "view_image".to_string(),
            payload: ToolPayload::Function {
                arguments: serde_json::json!({
                    "path": temp_png.path().to_str().unwrap()
                })
                .to_string(),
            },
        };

        let handler = ViewImageHandler;
        let result = handler.handle(invocation).await?;

        match result {
            ToolOutput::Function {
                content,
                content_items,
                success,
            } => {
                // Verify success
                assert_eq!(success, Some(true));

                // Verify content text contains path and mime type
                assert!(content.contains("image/png"));

                // Verify content_items contains both text and image
                let items = content_items.expect("should have content_items for PNG");
                assert_eq!(items.len(), 2);

                // First item should be text
                match &items[0] {
                    FunctionCallOutputContentItem::InputText { text } => {
                        assert!(text.contains("Image file:"));
                    }
                    _ => panic!("Expected InputText as first item"),
                }

                // Second item should be image with data URL
                match &items[1] {
                    FunctionCallOutputContentItem::InputImage { image_url } => {
                        assert!(image_url.starts_with("data:image/png;base64,"));
                    }
                    _ => panic!("Expected InputImage as second item"),
                }
            }
            _ => panic!("Expected Function output"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_view_image_returns_multimodal_content_items_for_jpeg() -> anyhow::Result<()> {
        let temp_jpeg = create_test_jpeg()?;
        let (session, turn_context) = make_session_and_context().await;

        let invocation = ToolInvocation {
            session: Arc::new(session),
            turn: Arc::new(turn_context),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::default())),
            call_id: "test-call-2".to_string(),
            tool_name: "view_image".to_string(),
            payload: ToolPayload::Function {
                arguments: serde_json::json!({
                    "path": temp_jpeg.path().to_str().unwrap()
                })
                .to_string(),
            },
        };

        let handler = ViewImageHandler;
        let result = handler.handle(invocation).await?;

        match result {
            ToolOutput::Function {
                content_items,
                success,
                ..
            } => {
                assert_eq!(success, Some(true));

                let items = content_items.expect("should have content_items for JPEG");
                assert_eq!(items.len(), 2);

                // Second item should be image with JPEG data URL
                match &items[1] {
                    FunctionCallOutputContentItem::InputImage { image_url } => {
                        assert!(image_url.starts_with("data:image/jpeg;base64,"));
                    }
                    _ => panic!("Expected InputImage as second item"),
                }
            }
            _ => panic!("Expected Function output"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_view_image_unsupported_format_returns_none_content_items() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::with_suffix(".bmp")?;
        temp.write_all(b"fake bmp data")?;
        temp.flush()?;

        let (session, turn_context) = make_session_and_context().await;

        let invocation = ToolInvocation {
            session: Arc::new(session),
            turn: Arc::new(turn_context),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::default())),
            call_id: "test-call-3".to_string(),
            tool_name: "view_image".to_string(),
            payload: ToolPayload::Function {
                arguments: serde_json::json!({
                    "path": temp.path().to_str().unwrap()
                })
                .to_string(),
            },
        };

        let handler = ViewImageHandler;
        let result = handler.handle(invocation).await?;

        match result {
            ToolOutput::Function {
                content,
                content_items,
                success,
            } => {
                assert_eq!(success, Some(true));
                // Unsupported format should not have content_items
                assert!(content_items.is_none());
                // But should still have content text
                assert!(content.contains("unsupported format"));
            }
            _ => panic!("Expected Function output"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_view_image_file_not_found() -> anyhow::Result<()> {
        let (session, turn_context) = make_session_and_context().await;

        let invocation = ToolInvocation {
            session: Arc::new(session),
            turn: Arc::new(turn_context),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::default())),
            call_id: "test-call-4".to_string(),
            tool_name: "view_image".to_string(),
            payload: ToolPayload::Function {
                arguments: serde_json::json!({
                    "path": "/nonexistent/path/to/image.png"
                })
                .to_string(),
            },
        };

        let handler = ViewImageHandler;
        let result = handler.handle(invocation).await;

        match result {
            Err(FunctionCallError::RespondToModel(msg)) => {
                assert!(msg.contains("unable to locate image"));
            }
            Ok(_) => panic!("Expected error for nonexistent file"),
            Err(other) => panic!("Expected RespondToModel error, got {:?}", other),
        }

        Ok(())
    }
}
