//! Data models for Hunyuan AI3D API

use serde::Deserialize;
use serde::Serialize;

/// API version to use
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ApiVersion {
    #[default]
    Pro,
    Rapid,
    Standard, // 通用版本 SubmitHunyuanTo3DJob
}

/// Generate type for professional version
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GenerateType {
    #[default]
    Normal,
    LowPoly,
    Geometry,
    Sketch,
}

/// Polygon type for LowPoly mode
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolygonType {
    Triangle,
    Quadrilateral,
}

/// Multi-view image
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ViewImage {
    pub view_type: String, // "left", "right", "back"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_image_base64: Option<String>,
}

/// Unified request for 3D generation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Generate3DRequest {
    // Text input (mutually exclusive with image except in Sketch mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    // Image inputs (mutually exclusive with prompt except in Sketch mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,

    // Multi-view images for professional version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_view_images: Option<Vec<ViewImage>>,

    // Output format (glb, fbx, obj, usdz)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>, // Pro: OutputFormat, Rapid: OutputType

    // Professional parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_pbr: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_count: Option<i32>, // 40000-1500000
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_type: Option<GenerateType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polygon_type: Option<PolygonType>, // Only for LowPoly mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i32>,
}

/// API request for professional version
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProApiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_view_images: Option<Vec<ViewImage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_p_b_r: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polygon_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i32>,
}

/// API request for rapid version
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RapidApiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>, // Base64
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_type: Option<String>, // Note: different from Pro's OutputFormat
}

/// Response from submit API
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SubmitResponse {
    pub job_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Task status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Processing,
    Success,
    Completed,
    Failed,
    Error,
    Timeout,
}

/// Response from query API
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResponse {
    // Common fields
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    // Error fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,

    // Result fields (Professional API)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_urls: Option<Vec<String>>,

    // Result fields (Rapid API)
    #[serde(rename = "ResultFile3Ds", skip_serializing_if = "Option::is_none")]
    pub result_file3_d_s: Option<Vec<ResultFile>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResultFile {
    #[serde(rename = "Type")]
    pub file_type: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_image_url: Option<String>,
}
