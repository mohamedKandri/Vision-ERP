use serde::Serialize;

/// Error returned to the frontend. `code` is stable and translated in the UI;
/// `message` is a developer-facing detail.
#[derive(Debug, Serialize, thiserror::Error, PartialEq)]
#[error("{code}: {message}")]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
}

impl ApiError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), retry_after_secs: None }
    }
    pub fn unauthorized() -> Self {
        Self::new("unauthorized", "login required")
    }
    pub fn forbidden(perm: &str) -> Self {
        Self::new("forbidden", format!("missing permission {perm}"))
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new("invalid", msg)
    }
    pub fn not_found(what: &str) -> Self {
        Self::new("not_found", what.to_string())
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("internal", format!("db: {e}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self::invalid(format!("bad params: {e}"))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
