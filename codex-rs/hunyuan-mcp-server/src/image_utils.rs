//! Image processing utilities

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use base64::Engine;
use image::GenericImageView;
use std::path::Path;
use tokio::fs;

const MAX_IMAGE_SIZE: usize = 6 * 1024 * 1024; // 6MB
const MIN_IMAGE_DIMENSION: u32 = 128;
const MAX_IMAGE_DIMENSION: u32 = 5000;

/// Extract base64 data from a data URL
/// Format: data:image/png;base64,<base64_data>
pub fn extract_base64_from_data_url(data_url: &str) -> Option<String> {
    if !data_url.starts_with("data:") {
        return None;
    }

    // Find the comma that separates metadata from data
    let comma_pos = data_url.find(',')?;
    let base64_data = &data_url[comma_pos + 1..];

    // Verify it's base64 encoded
    let metadata = &data_url[5..comma_pos]; // Skip "data:"
    if metadata.contains("base64") {
        Some(base64_data.to_string())
    } else {
        None
    }
}

/// Detect the source type of an image input
pub enum ImageSource {
    DataUrl(String),      // data:image/png;base64,...
    LocalPath(String),    // /path/to/image.png
    RemoteUrl(String),    // http://example.com/image.png
    Base64String(String), // Direct base64 string
}

impl ImageSource {
    pub fn detect(input: &str) -> ImageSource {
        if input.starts_with("data:") {
            ImageSource::DataUrl(input.to_string())
        } else if input.starts_with("http://") || input.starts_with("https://") {
            ImageSource::RemoteUrl(input.to_string())
        } else if Path::new(input).exists() {
            ImageSource::LocalPath(input.to_string())
        } else {
            // Assume it's a base64 string if it's not a URL or file path
            ImageSource::Base64String(input.to_string())
        }
    }
}

/// Convert various image sources to base64
pub async fn to_base64(source: ImageSource) -> Result<String> {
    match source {
        ImageSource::DataUrl(url) => {
            extract_base64_from_data_url(&url).ok_or_else(|| anyhow!("Invalid data URL format"))
        }
        ImageSource::LocalPath(path) => {
            let data = fs::read(&path)
                .await
                .with_context(|| format!("Failed to read image file: {}", path))?;
            validate_image_bytes(&data)?;
            Ok(base64::engine::general_purpose::STANDARD.encode(data))
        }
        ImageSource::RemoteUrl(url) => download_and_encode(&url).await,
        ImageSource::Base64String(s) => {
            // Validate that it's actually valid base64
            base64::engine::general_purpose::STANDARD
                .decode(&s)
                .context("Invalid base64 string")?;
            Ok(s)
        }
    }
}

/// Download an image from URL and encode to base64
async fn download_and_encode(url: &str) -> Result<String> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("Failed to download image from: {}", url))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to download image: HTTP {}",
            response.status()
        ));
    }

    let bytes = response
        .bytes()
        .await
        .context("Failed to read image response body")?;

    validate_image_bytes(&bytes)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Validate image bytes for size and dimensions
fn validate_image_bytes(data: &[u8]) -> Result<()> {
    // Check file size
    if data.len() > MAX_IMAGE_SIZE {
        return Err(anyhow!(
            "Image size ({} bytes) exceeds maximum allowed size ({} bytes)",
            data.len(),
            MAX_IMAGE_SIZE
        ));
    }

    // Load image to check dimensions
    let img = image::load_from_memory(data).context("Failed to decode image")?;

    let (width, height) = img.dimensions();

    if width < MIN_IMAGE_DIMENSION || height < MIN_IMAGE_DIMENSION {
        return Err(anyhow!(
            "Image dimensions ({}x{}) are below minimum required ({}x{})",
            width,
            height,
            MIN_IMAGE_DIMENSION,
            MIN_IMAGE_DIMENSION
        ));
    }

    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(anyhow!(
            "Image dimensions ({}x{}) exceed maximum allowed ({}x{})",
            width,
            height,
            MAX_IMAGE_DIMENSION,
            MAX_IMAGE_DIMENSION
        ));
    }

    Ok(())
}

/// Get MIME type from image format
pub fn get_mime_type(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase());

    match ext.as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/png", // Default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_base64_from_data_url() {
        let data_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAUA";
        let result = extract_base64_from_data_url(data_url);
        assert_eq!(result, Some("iVBORw0KGgoAAAANSUhEUgAAAAUA".to_string()));

        let data_url = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQEA";
        let result = extract_base64_from_data_url(data_url);
        assert_eq!(result, Some("/9j/4AAQSkZJRgABAQEA".to_string()));

        let invalid = "http://example.com/image.png";
        let result = extract_base64_from_data_url(invalid);
        assert_eq!(result, None);
    }

    #[test]
    fn test_image_source_detection() {
        match ImageSource::detect("data:image/png;base64,abc") {
            ImageSource::DataUrl(_) => {}
            _ => panic!("Should detect data URL"),
        }

        match ImageSource::detect("http://example.com/image.png") {
            ImageSource::RemoteUrl(_) => {}
            _ => panic!("Should detect remote URL"),
        }

        match ImageSource::detect("https://example.com/image.png") {
            ImageSource::RemoteUrl(_) => {}
            _ => panic!("Should detect HTTPS URL"),
        }

        // Note: Local path detection requires the file to exist
        // Base64 string is the fallback for anything else
        match ImageSource::detect("iVBORw0KGgoAAAANSUhEUgAAAAUA") {
            ImageSource::Base64String(_) => {}
            _ => panic!("Should detect base64 string as fallback"),
        }
    }
}
