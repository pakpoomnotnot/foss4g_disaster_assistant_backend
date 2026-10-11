use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// OpenAI-compatible base URL, including the version segment (`.../v1`).
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    /// Sends OpenRouter's `reasoning: {enabled: true}`. Off by default: it
    /// slows every extraction down and the query object needs no deliberation.
    pub llm_reasoning: bool,
    pub gistda_gateway_host: String,
    /// Long-lived key. Preferred: it never expires, so there is nothing to refresh.
    pub gistda_api_key: String,
    /// Short-lived JWT (~1 hour, measured). Only used when no api_key is set.
    pub gistda_access_token: String,
    pub gazetteer_path: String,
    pub port: u16,
}

/// Which credential the gateway is being called with. Both are passed as a
/// query parameter and the gateway propagates whichever one was used into the
/// `asset.data` hrefs it hands back, so the second hop needs no extra work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GistdaAuth {
    ApiKey,
    Token,
}

impl GistdaAuth {
    pub fn param(self) -> &'static str {
        match self {
            GistdaAuth::ApiKey => "api_key",
            GistdaAuth::Token => "token",
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            llm_base_url: env::var("LLM_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".to_string()),
            // OPENROUTER_API_KEY is accepted so a key exported for OpenRouter's own
            // tooling works as is; an explicit LLM_API_KEY still wins.
            llm_api_key: env::var("LLM_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
                .or_else(|| env::var("OPENROUTER_API_KEY").ok())
                .unwrap_or_default(),
            llm_model: env::var("LLM_MODEL").unwrap_or_else(|_| "tekton-code".to_string()),
            llm_reasoning: env::var("LLM_REASONING")
                .is_ok_and(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes")),
            gistda_gateway_host: env::var("GISTDA_GATEWAY_HOST")
                .unwrap_or_else(|_| "https://api-gateway.gistda.or.th/api/2.0".to_string()),
            gistda_api_key: env::var("GISTDA_API_KEY").unwrap_or_default(),
            gistda_access_token: env::var("GISTDA_ACCESS_TOKEN").unwrap_or_default(),
            gazetteer_path: env::var("GAZETTEER_PATH")
                .unwrap_or_else(|_| "../src/lib/gazetteer_merged.json".to_string()),
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8080),
        })
    }

    /// api_key wins whenever it is present — the token path stays only as a
    /// fallback for a key-less setup.
    pub fn gistda_auth(&self) -> (GistdaAuth, &str) {
        if !self.gistda_api_key.is_empty() {
            (GistdaAuth::ApiKey, self.gistda_api_key.as_str())
        } else {
            (GistdaAuth::Token, self.gistda_access_token.as_str())
        }
    }

    /// True when the configured credential is usable right now. An api_key has
    /// no expiry, so it only has to be non-empty; a JWT also has to be unexpired.
    pub fn gistda_credential_valid(&self) -> bool {
        let (kind, value) = self.gistda_auth();
        !value.is_empty()
            && (kind == GistdaAuth::ApiKey || !crate::gistda::auth::is_token_expired(value))
    }
}
