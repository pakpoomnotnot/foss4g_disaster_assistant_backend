use axum::{http::StatusCode, response::IntoResponse, response::Response, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("llm returned invalid schema: {0}")]
    Llm(String),
    #[error("llm unavailable")]
    LlmUnavailable,
    #[error("gistda credential missing or expired")]
    TokenExpired,
    #[error("gistda upstream error: {0}")]
    Gistda(String),
    #[error("internal error: {0}")]
    #[allow(dead_code)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::LlmUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM unavailable or timed out".to_string(),
            ),
            AppError::Llm(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to parse LLM response".to_string(),
            ),
            AppError::TokenExpired => (
                StatusCode::SERVICE_UNAVAILABLE,
                "GISTDA credential missing or expired — set GISTDA_API_KEY in .env".to_string(),
            ),
            AppError::Gistda(_) => (
                StatusCode::BAD_GATEWAY,
                "upstream GISTDA/Vallaris error".to_string(),
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
            ),
        };
        tracing::error!(error = %self, "request failed");
        (status, Json(json!({ "error": message }))).into_response()
    }
}
