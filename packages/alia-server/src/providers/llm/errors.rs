use std::fmt;

use reqwest::StatusCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum LlmProviderErrorKind {
    Auth,
    RateLimited,
    Overloaded,
    BadRequest,
    ContextLength,
    ContentFilter,
    TransientNetwork,
    Provider,
}

#[derive(Debug)]
pub struct LlmProviderError {
    pub provider: &'static str,
    pub kind: LlmProviderErrorKind,
    pub status: Option<StatusCode>,
    pub message: String,
}

impl fmt::Display for LlmProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(status) = self.status {
            write!(
                f,
                "{} LLM error kind={:?} status={}: {}",
                self.provider, self.kind, status, self.message
            )
        } else {
            write!(
                f,
                "{} LLM error kind={:?}: {}",
                self.provider, self.kind, self.message
            )
        }
    }
}

impl std::error::Error for LlmProviderError {}

pub fn classify_http_error(
    provider: &'static str,
    status: StatusCode,
    body: String,
) -> anyhow::Error {
    let lower = body.to_ascii_lowercase();
    let kind = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        LlmProviderErrorKind::Auth
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        LlmProviderErrorKind::RateLimited
    } else if matches!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    ) {
        LlmProviderErrorKind::Overloaded
    } else if lower.contains("context_length")
        || lower.contains("context length")
        || lower.contains("maximum context")
        || lower.contains("token limit")
        || lower.contains("too many tokens")
    {
        LlmProviderErrorKind::ContextLength
    } else if lower.contains("content_filter")
        || lower.contains("content filter")
        || lower.contains("safety")
        || lower.contains("blocked")
    {
        LlmProviderErrorKind::ContentFilter
    } else if status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY {
        LlmProviderErrorKind::BadRequest
    } else {
        LlmProviderErrorKind::Provider
    };
    LlmProviderError {
        provider,
        kind,
        status: Some(status),
        message: body,
    }
    .into()
}

pub fn classify_transport_error(provider: &'static str, error: reqwest::Error) -> anyhow::Error {
    let kind = if error.is_timeout() || error.is_connect() || error.is_request() {
        LlmProviderErrorKind::TransientNetwork
    } else {
        LlmProviderErrorKind::Provider
    };
    LlmProviderError {
        provider,
        kind,
        status: error.status(),
        message: error.to_string(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_http_errors() {
        let err = classify_http_error("test", StatusCode::UNAUTHORIZED, "bad key".to_string());
        assert_eq!(
            err.downcast_ref::<LlmProviderError>().unwrap().kind,
            LlmProviderErrorKind::Auth
        );

        let err = classify_http_error(
            "test",
            StatusCode::TOO_MANY_REQUESTS,
            "slow down".to_string(),
        );
        assert_eq!(
            err.downcast_ref::<LlmProviderError>().unwrap().kind,
            LlmProviderErrorKind::RateLimited
        );

        let err = classify_http_error(
            "test",
            StatusCode::BAD_REQUEST,
            "context length exceeded".to_string(),
        );
        assert_eq!(
            err.downcast_ref::<LlmProviderError>().unwrap().kind,
            LlmProviderErrorKind::ContextLength
        );
    }
}
