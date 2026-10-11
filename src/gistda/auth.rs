use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use serde::Deserialize;

#[derive(Deserialize)]
struct JwtClaims {
    exp: i64,
}

/// Phase 1 (README §4.3): no auto-refresh, just decode the JWT `exp` claim
/// (without verifying the signature — we don't hold GISTDA's key) so we can
/// fail fast with a clear message instead of letting GISTDA return a bare 401.
pub fn is_token_expired(token: &str) -> bool {
    let Some(payload_b64) = token.split('.').nth(1) else {
        return true;
    };
    let Ok(payload) = URL_SAFE_NO_PAD.decode(payload_b64) else {
        return true;
    };
    let Ok(claims) = serde_json::from_slice::<JwtClaims>(&payload) else {
        return true;
    };
    Utc::now().timestamp() >= claims.exp
}
