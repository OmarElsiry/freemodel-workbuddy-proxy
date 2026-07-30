use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error, Clone)]
#[error("{message}")]
pub struct AcpError {
    pub message: String,
    pub category: &'static str,
    pub retryable: bool,
    pub status_code: Option<StatusCode>,
}

impl AcpError {
    pub fn new(message: impl Into<String>, category: &'static str) -> Self {
        Self {
            message: message.into(),
            category,
            retryable: false,
            status_code: None,
        }
    }

    pub fn retryable(mut self, value: bool) -> Self {
        self.retryable = value;
        self
    }

    pub fn status(mut self, status: StatusCode) -> Self {
        self.status_code = Some(status);
        self
    }

    pub fn from_http_status(operation: &str, status: StatusCode) -> Self {
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Self::new(
                format!(
                    "WorkBuddy ACP {operation} authentication failed ({})",
                    status.as_u16()
                ),
                "authentication",
            )
            .status(status);
        }
        let retryable =
            matches!(status.as_u16(), 408 | 409 | 425 | 429) || status.is_server_error();
        let category = if matches!(
            status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        ) {
            "capacity"
        } else {
            "upstream"
        };
        Self::new(
            format!("WorkBuddy ACP {operation} failed ({})", status.as_u16()),
            category,
        )
        .retryable(retryable)
        .status(status)
    }
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Permission(String),
    #[error("{0}")]
    Upstream(String),
    #[error(transparent)]
    Acp(#[from] AcpError),
    #[error("{0}")]
    Internal(String),
}

impl ProxyError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Permission(_) => StatusCode::FORBIDDEN,
            Self::Acp(error) => error.status_code.unwrap_or(StatusCode::BAD_GATEWAY),
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_request_error",
            Self::NotFound(_) => "not_found_error",
            Self::Conflict(_) => "invalid_request_error",
            Self::Permission(_) => "permission_error",
            Self::Acp(_) => "workbuddy_acp_error",
            Self::Upstream(_) | Self::Internal(_) => "upstream_error",
        }
    }

    pub fn envelope(&self) -> Value {
        let status = self.status();
        json!({
            "error": {
                "message": self.to_string(),
                "type": self.kind(),
                "code": status.as_u16()
            }
        })
    }
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> axum::response::Response {
        (self.status(), Json(self.envelope())).into_response()
    }
}
