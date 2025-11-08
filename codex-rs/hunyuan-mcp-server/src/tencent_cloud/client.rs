//! Tencent Cloud API client

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use chrono::Utc;
use reqwest::Client;
use reqwest::Response;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::warn;
use serde_json::json;
use std::time::Duration;
use tracing::debug;
use tracing::error;
use tracing::info;

use crate::models::ApiVersion;
use crate::models::Generate3DRequest;
use crate::models::GenerateType;
use crate::models::ProApiRequest;
use crate::models::QueryResponse;
use crate::models::RapidApiRequest;
use crate::models::SubmitResponse;
use crate::models::ViewImage;
use crate::tencent_cloud::auth::TencentAuth;

const API_ENDPOINT: &str = "https://ai3d.tencentcloudapi.com";
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(2);

pub struct TencentCloudClient {
    client: Client,
    auth: TencentAuth,
    endpoint: String,
}

impl TencentCloudClient {
    pub fn new(secret_id: String, secret_key: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("Failed to create HTTP client")?;

        let auth = TencentAuth::new(secret_id, secret_key);

        Ok(Self {
            client,
            auth,
            endpoint: API_ENDPOINT.to_string(),
        })
    }

    /// Submit a 3D generation job
    pub async fn submit_job(
        &self,
        request: Generate3DRequest,
        api_version: ApiVersion,
    ) -> Result<SubmitResponse> {
        let action = match api_version {
            ApiVersion::Pro => "SubmitHunyuanTo3DProJob",
            ApiVersion::Rapid => "SubmitHunyuanTo3DRapidJob",
            ApiVersion::Standard => "SubmitHunyuanTo3DJob",
        };

        let body = self.prepare_request_body(request, api_version)?;
        let response: serde_json::Value = self.call_api(action, &body).await?;
        
        // Log the full response for debugging
        info!("Submit job response: {}", serde_json::to_string_pretty(&response)?);

        // Extract job ID from various possible response formats
        let job_id = response
            .get("JobId")
            .or_else(|| response.get("TaskId"))
            .or_else(|| response.get("Data").and_then(|d| d.get("JobId")))
            .or_else(|| response.get("Result").and_then(|r| r.get("JobId")))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Failed to extract JobId from response. Response: {}", 
                serde_json::to_string(&response).unwrap_or_else(|_| "unable to serialize".to_string())))?;

        Ok(SubmitResponse {
            job_id: job_id.to_string(),
            request_id: response
                .get("RequestId")
                .and_then(|v| v.as_str())
                .map(String::from),
        })
    }

    /// Query job status
    pub async fn query_job(&self, job_id: &str, api_version: ApiVersion) -> Result<QueryResponse> {
        let action = match api_version {
            ApiVersion::Pro => "QueryHunyuanTo3DProJob",
            ApiVersion::Rapid => "QueryHunyuanTo3DRapidJob",
            ApiVersion::Standard => "QueryHunyuanTo3DJob",
        };

        let body = json!({
            "JobId": job_id
        });

        // Get raw response first for debugging
        let response: serde_json::Value = self.call_api(action, &body).await?;
        info!("Query job response: {}", serde_json::to_string_pretty(&response)?);
        
        // Try to parse into QueryResponse
        serde_json::from_value(response.clone())
            .with_context(|| format!("Failed to parse query response: {}", 
                serde_json::to_string(&response).unwrap_or_else(|_| "unable to serialize".to_string())))
    }

    /// Prepare request body based on API version
    fn prepare_request_body(
        &self,
        request: Generate3DRequest,
        api_version: ApiVersion,
    ) -> Result<serde_json::Value> {
        match api_version {
            ApiVersion::Pro => {
                let mut pro_request = json!({});

                // Handle text input
                if let Some(ref prompt) = request.prompt {
                    pro_request["Prompt"] = json!(prompt);
                }

                // Handle image input
                if let Some(ref base64) = request.image_base64 {
                    pro_request["ImageBase64"] = json!(base64);
                } else if let Some(ref url) = request.image_url {
                    pro_request["ImageUrl"] = json!(url);
                }

                // Handle multi-view images
                if let Some(views) = request.multi_view_images {
                    pro_request["MultiViewImages"] = json!(views);
                }

                // Note: Professional API may not support OutputFormat parameter
                // Based on API response: "The parameter `OutputFormat` is not recognized"
                // Commenting out for now - the API will use its default format
                // if let Some(format) = request.output_format {
                //     pro_request["OutputFormat"] = json!(format);
                // }

                // Professional parameters
                if let Some(enable_pbr) = request.enable_pbr {
                    pro_request["EnablePBR"] = json!(enable_pbr);
                }

                if let Some(face_count) = request.face_count {
                    if face_count < 40000 || face_count > 1500000 {
                        return Err(anyhow!("FaceCount must be between 40000 and 1500000"));
                    }
                    pro_request["FaceCount"] = json!(face_count);
                }

                if let Some(ref generate_type) = request.generate_type {
                    let type_str = match generate_type {
                        GenerateType::Normal => "Normal",
                        GenerateType::LowPoly => "LowPoly",
                        GenerateType::Geometry => "Geometry",
                        GenerateType::Sketch => "Sketch",
                    };
                    pro_request["GenerateType"] = json!(type_str);

                    // Validate Sketch mode allows both text and image
                    if *generate_type != GenerateType::Sketch {
                        let has_text = request.prompt.is_some();
                        let has_image =
                            request.image_base64.is_some() || request.image_url.is_some();
                        if has_text && has_image {
                            return Err(anyhow!(
                                "Text and image cannot be used together except in Sketch mode"
                            ));
                        }
                    }
                }

                if let Some(ref polygon_type) = request.polygon_type {
                    // Only valid for LowPoly mode
                    if request.generate_type.as_ref() != Some(&GenerateType::LowPoly) {
                        return Err(anyhow!("PolygonType is only valid for LowPoly mode"));
                    }
                    let type_str = match polygon_type {
                        crate::models::PolygonType::Triangle => "triangle",
                        crate::models::PolygonType::Quadrilateral => "quadrilateral",
                    };
                    pro_request["PolygonType"] = json!(type_str);
                }

                // Note: Professional API does not support NegativePrompt and Seed parameters
                // These parameters are only available in other API versions
                // Commenting out to avoid "UnknownParameter" errors
                // if let Some(negative) = request.negative_prompt {
                //     pro_request["NegativePrompt"] = json!(negative);
                // }
                // if let Some(seed) = request.seed {
                //     pro_request["Seed"] = json!(seed);
                // }

                Ok(pro_request)
            }
            ApiVersion::Rapid => {
                let mut rapid_request = json!({});

                // Handle text input (Rapid限制200字符，Pro限制1024字符)
                if let Some(ref prompt) = request.prompt {
                    if prompt.len() > 200 {
                        warn!("Rapid API限制prompt最多200个字符，当前{}个字符", prompt.len());
                    }
                    rapid_request["Prompt"] = json!(prompt);
                }

                // Handle image input (Rapid uses same field names as Pro)
                if let Some(ref base64) = request.image_base64 {
                    rapid_request["ImageBase64"] = json!(base64);
                } else if let Some(ref url) = request.image_url {
                    rapid_request["ImageUrl"] = json!(url);
                }

                // Rapid API supports ResultFormat parameter
                if let Some(ref format) = request.output_format {
                    // Convert to uppercase as API expects OBJ, GLB, STL, USDZ, FBX, MP4
                    rapid_request["ResultFormat"] = json!(format.to_uppercase());
                } else {
                    // Default to OBJ
                    rapid_request["ResultFormat"] = json!("OBJ");
                }

                // Rapid API also supports EnablePBR
                if let Some(enable_pbr) = request.enable_pbr {
                    rapid_request["EnablePBR"] = json!(enable_pbr);
                }

                Ok(rapid_request)
            }
            ApiVersion::Standard => {
                // Standard API - 通用版本，可能支持更多参数
                let mut standard_request = json!({});

                // Handle text input
                if let Some(ref prompt) = request.prompt {
                    standard_request["Prompt"] = json!(prompt);
                }

                // Handle image input
                if let Some(ref base64) = request.image_base64 {
                    standard_request["ImageBase64"] = json!(base64);
                } else if let Some(ref url) = request.image_url {
                    standard_request["ImageUrl"] = json!(url);
                }

                // Standard API可能支持的参数（需要根据实际API文档确认）
                if let Some(enable_pbr) = request.enable_pbr {
                    standard_request["EnablePBR"] = json!(enable_pbr);
                }

                if let Some(face_count) = request.face_count {
                    standard_request["FaceCount"] = json!(face_count);
                }

                if let Some(ref generate_type) = request.generate_type {
                    let type_str = match generate_type {
                        GenerateType::Normal => "Normal",
                        GenerateType::LowPoly => "LowPoly", 
                        GenerateType::Geometry => "Geometry",
                        GenerateType::Sketch => "Sketch",
                    };
                    standard_request["GenerateType"] = json!(type_str);
                }

                Ok(standard_request)
            }
        }
    }

    /// Call Tencent Cloud API with retry logic
    async fn call_api<T: DeserializeOwned>(
        &self,
        action: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let mut retries = 0;

        loop {
            let timestamp = Utc::now().timestamp();
            let payload = serde_json::to_string(body)?;

            // Generate authentication headers
            let mut headers = self.auth.sign_request(
                "POST",
                "ai3d.tencentcloudapi.com",
                "/",
                "",
                &payload,
                timestamp,
            )?;
            headers.insert("X-TC-Action".to_string(), action.to_string());

            // Build request
            let mut request = self.client.post(&self.endpoint).body(payload);
            for (key, value) in headers {
                request = request.header(key, value);
            }

            // Send request
            let response = request.send().await?;
            let status = response.status();
            let text = response.text().await?;

            debug!("API Response: {}", text);

            // Parse response
            let json_response: serde_json::Value = serde_json::from_str(&text)
                .with_context(|| format!("Failed to parse API response: {}", text))?;

            // Check for errors
            if let Some(error) = json_response.get("Error") {
                let code = error
                    .get("Code")
                    .and_then(|c| c.as_str())
                    .unwrap_or("Unknown");
                let message = error
                    .get("Message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");

                // Retry on transient errors
                if retries < MAX_RETRIES
                    && (code.contains("RequestLimitExceeded")
                        || code.contains("InternalError")
                        || code.contains("ServiceUnavailable"))
                {
                    retries += 1;
                    error!(
                        "API error (retry {}/{}): {} - {}",
                        retries, MAX_RETRIES, code, message
                    );
                    tokio::time::sleep(RETRY_DELAY * retries).await;
                    continue;
                }

                return Err(anyhow!("API error: {} - {}", code, message));
            }

            // Success - parse response
            if let Some(response_data) = json_response.get("Response") {
                return serde_json::from_value(response_data.clone())
                    .context("Failed to deserialize API response");
            } else {
                return serde_json::from_value(json_response)
                    .context("Failed to deserialize API response");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prepare_pro_request() {
        let client = TencentCloudClient::new("test".to_string(), "test".to_string()).unwrap();

        let request = Generate3DRequest {
            prompt: Some("a cute cat".to_string()),
            image_base64: None,
            image_url: None,
            multi_view_images: None,
            output_format: Some("glb".to_string()),
            enable_pbr: Some(true),
            face_count: Some(50000),
            generate_type: Some(GenerateType::Normal),
            polygon_type: None,
            negative_prompt: None,
            seed: Some(42),
        };

        let body = client
            .prepare_request_body(request, ApiVersion::Pro)
            .unwrap();

        assert_eq!(body["Prompt"], "a cute cat");
        assert_eq!(body["OutputFormat"], "glb");
        assert_eq!(body["EnablePBR"], true);
        assert_eq!(body["FaceCount"], 50000);
        assert_eq!(body["GenerateType"], "Normal");
        assert_eq!(body["Seed"], 42);
    }

    #[test]
    fn test_prepare_rapid_request() {
        let client = TencentCloudClient::new("test".to_string(), "test".to_string()).unwrap();

        let request = Generate3DRequest {
            prompt: Some("a cute dog".to_string()),
            image_base64: None,
            image_url: None,
            multi_view_images: None,
            output_format: Some("glb".to_string()),
            enable_pbr: None,
            face_count: None,
            generate_type: None,
            polygon_type: None,
            negative_prompt: None,
            seed: None,
        };

        let body = client
            .prepare_request_body(request, ApiVersion::Rapid)
            .unwrap();

        assert_eq!(body["Prompt"], "a cute dog");
        assert_eq!(body["OutputType"], "GLB");
    }

    #[test]
    fn test_validate_face_count() {
        let client = TencentCloudClient::new("test".to_string(), "test".to_string()).unwrap();

        let request = Generate3DRequest {
            prompt: Some("test".to_string()),
            face_count: Some(10000), // Too low
            ..Default::default()
        };

        let result = client.prepare_request_body(request, ApiVersion::Pro);
        assert!(result.is_err());
    }
}
