use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::AppState;

/// README §6 — checks LLM reachability, gazetteer load, and the GISTDA
/// credential (an api_key just has to exist; a token also has to be unexpired).
pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let gazetteer_ok = !state.gazetteer.is_empty();

    let (auth_kind, _) = state.config.gistda_auth();
    let token_ok = state.config.gistda_credential_valid();

    let llm_ok = state
        .llm
        .health_request()
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let healthy = gazetteer_ok && token_ok && llm_ok;
    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(json!({
            "status": if healthy { "ok" } else { "degraded" },
            "gazetteer_loaded": gazetteer_ok,
            "gazetteer_entries": state.gazetteer.len(),
            "gistda_token_valid": token_ok,
            "gistda_auth": auth_kind.param(),
            "llm_reachable": llm_ok,
            "llm_model": state.config.llm_model,
        })),
    )
}
