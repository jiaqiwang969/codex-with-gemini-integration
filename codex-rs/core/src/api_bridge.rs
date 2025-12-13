use chrono::DateTime;
use chrono::Utc;
use codex_api::AuthProvider as ApiAuthProvider;
use codex_api::TransportError;
use codex_api::error::ApiError;
use codex_api::rate_limits::parse_rate_limit;
use http::HeaderMap;
use serde::Deserialize;
use std::time::Duration;

use crate::auth::CodexAuth;
use crate::error::CodexErr;
use crate::error::RetryLimitReachedError;
use crate::error::UnexpectedResponseError;
use crate::error::UsageLimitReachedError;
use crate::model_provider_info::ModelProviderInfo;
use crate::token_data::PlanType;
use crate::util::try_parse_error_message;
use crate::util::try_parse_retry_after_from_rate_limit_message;

pub(crate) fn map_api_error(err: ApiError) -> CodexErr {
    match err {
        ApiError::ContextWindowExceeded => CodexErr::ContextWindowExceeded,
        ApiError::QuotaExceeded => CodexErr::QuotaExceeded,
        ApiError::UsageNotIncluded => CodexErr::UsageNotIncluded,
        ApiError::Retryable { message, delay } => CodexErr::Stream(message, delay),
        ApiError::Stream(msg) => CodexErr::Stream(msg, None),
        ApiError::Api { status, message } => CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status,
            body: message,
            request_id: None,
        }),
        ApiError::Transport(transport) => match transport {
            TransportError::Http {
                status,
                headers,
                body,
            } => {
                let body_text = body.unwrap_or_default();

                if status == http::StatusCode::BAD_REQUEST {
                    if body_text
                        .contains("The image data you provided does not represent a valid image")
                    {
                        CodexErr::InvalidImageRequest()
                    } else {
                        CodexErr::InvalidRequest(body_text)
                    }
                } else if status == http::StatusCode::INTERNAL_SERVER_ERROR {
                    CodexErr::InternalServerError
                } else if status == http::StatusCode::TOO_MANY_REQUESTS {
                    if let Ok(err) = serde_json::from_str::<UsageErrorResponse>(&body_text) {
                        if err.error.error_type.as_deref() == Some("usage_limit_reached") {
                            let rate_limits = headers.as_ref().and_then(parse_rate_limit);
                            let resets_at = err
                                .error
                                .resets_at
                                .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0));
                            return CodexErr::UsageLimitReached(UsageLimitReachedError {
                                plan_type: err.error.plan_type,
                                resets_at,
                                rate_limits,
                            });
                        } else if err.error.error_type.as_deref() == Some("usage_not_included") {
                            return CodexErr::UsageNotIncluded;
                        }
                    }

                    let message = try_parse_error_message(&body_text);
                    let retry_after = retry_after_from_headers(headers.as_ref())
                        .or_else(|| try_parse_retry_after_from_rate_limit_message(&message));
                    CodexErr::RetryLimit(RetryLimitReachedError {
                        status,
                        request_id: extract_request_id(headers.as_ref()),
                        message: Some(message),
                        retry_after,
                    })
                } else {
                    CodexErr::UnexpectedStatus(UnexpectedResponseError {
                        status,
                        body: body_text,
                        request_id: extract_request_id(headers.as_ref()),
                    })
                }
            }
            TransportError::RetryLimit => CodexErr::RetryLimit(RetryLimitReachedError {
                status: http::StatusCode::INTERNAL_SERVER_ERROR,
                request_id: None,
                message: None,
                retry_after: None,
            }),
            TransportError::Timeout => CodexErr::Timeout,
            TransportError::Network(msg) | TransportError::Build(msg) => {
                CodexErr::Stream(msg, None)
            }
        },
        ApiError::RateLimit(msg) => CodexErr::Stream(msg, None),
    }
}

fn extract_request_id(headers: Option<&HeaderMap>) -> Option<String> {
    headers.and_then(|map| {
        ["cf-ray", "x-request-id", "x-oai-request-id"]
            .iter()
            .find_map(|name| {
                map.get(*name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            })
    })
}

fn retry_after_from_headers(headers: Option<&HeaderMap>) -> Option<Duration> {
    let headers = headers?;
    let retry_after = headers.get(http::header::RETRY_AFTER)?;
    let raw = retry_after.to_str().ok()?;
    let seconds: f64 = raw.trim().parse().ok()?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_api::error::ApiError;
    use http::HeaderValue;
    use pretty_assertions::assert_eq;

    #[test]
    fn map_api_error_includes_rate_limit_message_and_retry_after_delay() {
        let body = r#"{
  "error": {
    "message": "Rate limit reached for gpt-test. Please try again in 11.054s.",
    "type": "rate_limit_exceeded",
    "param": null,
    "code": "rate_limit_exceeded"
  }
}"#;

        let mapped = map_api_error(ApiError::Transport(TransportError::Http {
            status: http::StatusCode::TOO_MANY_REQUESTS,
            headers: None,
            body: Some(body.to_string()),
        }));

        match mapped {
            CodexErr::RetryLimit(err) => {
                assert_eq!(err.status, http::StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(
                    err.message,
                    Some(
                        "Rate limit reached for gpt-test. Please try again in 11.054s.".to_string()
                    )
                );
                assert_eq!(err.retry_after, Some(Duration::from_millis(11_054)));
            }
            other => panic!("expected RetryLimit error, got {other:?}"),
        }
    }

    #[test]
    fn map_api_error_prefers_retry_after_header_over_message() {
        let body = r#"{
  "error": {
    "message": "Too many requests.",
    "type": "rate_limit_exceeded"
  }
}"#;

        let mut headers = HeaderMap::new();
        headers.insert(http::header::RETRY_AFTER, HeaderValue::from_static("12"));

        let mapped = map_api_error(ApiError::Transport(TransportError::Http {
            status: http::StatusCode::TOO_MANY_REQUESTS,
            headers: Some(headers),
            body: Some(body.to_string()),
        }));

        match mapped {
            CodexErr::RetryLimit(err) => {
                assert_eq!(err.status, http::StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(err.retry_after, Some(Duration::from_secs(12)));
            }
            other => panic!("expected RetryLimit error, got {other:?}"),
        }
    }
}

pub(crate) async fn auth_provider_from_auth(
    auth: Option<CodexAuth>,
    provider: &ModelProviderInfo,
) -> crate::error::Result<CoreAuthProvider> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(CoreAuthProvider {
            token: Some(api_key),
            account_id: None,
        });
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(CoreAuthProvider {
            token: Some(token),
            account_id: None,
        });
    }

    if let Some(auth) = auth {
        let token = auth.get_token().await?;
        Ok(CoreAuthProvider {
            token: Some(token),
            account_id: auth.get_account_id(),
        })
    } else {
        Ok(CoreAuthProvider {
            token: None,
            account_id: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct UsageErrorResponse {
    error: UsageErrorBody,
}

#[derive(Debug, Deserialize)]
struct UsageErrorBody {
    #[serde(rename = "type")]
    error_type: Option<String>,
    plan_type: Option<PlanType>,
    resets_at: Option<i64>,
}

#[derive(Clone, Default)]
pub(crate) struct CoreAuthProvider {
    token: Option<String>,
    account_id: Option<String>,
}

impl ApiAuthProvider for CoreAuthProvider {
    fn bearer_token(&self) -> Option<String> {
        self.token.clone()
    }

    fn account_id(&self) -> Option<String> {
        self.account_id.clone()
    }
}
