//! Tencent Cloud TC3-HMAC-SHA256 authentication

use anyhow::Context;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use hmac::Hmac;
use hmac::Mac;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

/// Tencent Cloud authentication handler
pub struct TencentAuth {
    secret_id: String,
    secret_key: String,
}

impl TencentAuth {
    pub fn new(secret_id: String, secret_key: String) -> Self {
        Self {
            secret_id,
            secret_key,
        }
    }

    /// Generate TC3-HMAC-SHA256 signature
    pub fn sign_request(
        &self,
        method: &str,
        host: &str,
        uri: &str,
        params: &str,
        payload: &str,
        timestamp: i64,
    ) -> Result<HashMap<String, String>> {
        let service = "ai3d";
        let algorithm = "TC3-HMAC-SHA256";
        let date = DateTime::<Utc>::from_timestamp(timestamp, 0).context("Invalid timestamp")?;
        let date_str = date.format("%Y-%m-%d").to_string();

        // Step 1: Build canonical request
        let hashed_payload = hex::encode(Sha256::digest(payload.as_bytes()));
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            uri,
            params,
            format!("content-type:application/json\nhost:{}\n", host),
            "content-type;host",
            hashed_payload
        );

        // Step 2: Build string to sign
        let credential_scope = format!("{date_str}/{service}/tc3_request");
        let hashed_canonical_request = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let string_to_sign =
            format!("{algorithm}\n{timestamp}\n{credential_scope}\n{hashed_canonical_request}");

        // Step 3: Calculate signature
        let secret_date = self.hmac_sha256(
            format!("TC3{}", self.secret_key).as_bytes(),
            date_str.as_bytes(),
        )?;
        let secret_service = self.hmac_sha256(&secret_date, service.as_bytes())?;
        let secret_signing = self.hmac_sha256(&secret_service, b"tc3_request")?;
        let signature = hex::encode(self.hmac_sha256(&secret_signing, string_to_sign.as_bytes())?);

        // Step 4: Build authorization header
        let authorization = format!(
            "{} Credential={}/{}, SignedHeaders=content-type;host, Signature={}",
            algorithm, self.secret_id, credential_scope, signature
        );

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), authorization);
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("Host".to_string(), host.to_string());
        headers.insert("X-TC-Action".to_string(), "".to_string()); // Will be set by caller
        headers.insert("X-TC-Version".to_string(), "2025-05-13".to_string());
        headers.insert("X-TC-Timestamp".to_string(), timestamp.to_string());
        headers.insert("X-TC-Region".to_string(), "ap-guangzhou".to_string());

        Ok(headers)
    }

    fn hmac_sha256(&self, key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let mut mac = HmacSha256::new_from_slice(key).context("Invalid key length for HMAC")?;
        mac.update(data);
        Ok(mac.finalize().into_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmac_sha256() {
        let auth = TencentAuth::new("test_id".to_string(), "test_key".to_string());
        let result = auth.hmac_sha256(b"key", b"data").unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_sign_request() {
        let auth = TencentAuth::new("test_id".to_string(), "test_key".to_string());
        let timestamp = 1609459200; // 2021-01-01 00:00:00 UTC

        let headers = auth
            .sign_request(
                "POST",
                "ai3d.tencentcloudapi.com",
                "/",
                "",
                r#"{"Prompt":"test"}"#,
                timestamp,
            )
            .unwrap();

        assert!(headers.contains_key("Authorization"));
        assert!(headers.contains_key("X-TC-Timestamp"));
        assert_eq!(headers.get("X-TC-Version").unwrap(), "2025-05-13");
    }
}
