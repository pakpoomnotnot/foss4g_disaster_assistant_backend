use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::models::ExtractedQuery;

use super::prompt::system_prompt;

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

/// OpenRouter's unified reasoning switch. Other OpenAI-compatible servers do
/// not know it, so it is only serialized when turned on.
#[derive(Serialize)]
struct Reasoning {
    enabled: bool,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    /// Server-side JSON mode. This is what replaces Ollama's `format: "json"` —
    /// without it the model is free to wrap the object in prose or fences.
    response_format: ResponseFormat,
    stream: bool,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Reasoning>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    /// Nullable: OpenRouter returns `null` here when a reasoning model spent its
    /// whole token budget thinking (the thoughts go to a separate `reasoning`
    /// field, which is deliberately not read).
    content: Option<String>,
}

/// Removes a reasoning model's private scratchpad from the reply.
///
/// `tekton-code` is a reasoning model: it emits its deliberation inside
/// `<think>…</think>` in the SAME `message.content` as the answer. Rendered
/// verbatim, the analysis panel showed the model talking to itself in English
/// ("Let's draft the response now. </think> น้ำท่วมใน…") above the real answer.
///
/// Keeps everything after the last closing tag; if the block was opened and
/// never closed (truncated by a token limit) there is no answer to show, so the
/// caller gets nothing rather than a paragraph of internal monologue.
fn strip_reasoning(raw: &str) -> String {
    const CLOSE: &str = "</think>";
    if let Some(idx) = raw.rfind(CLOSE) {
        return raw[idx + CLOSE.len()..].trim().to_string();
    }
    if raw.trim_start().starts_with("<think>") {
        return String::new();
    }
    raw.trim().to_string()
}

/// Cuts the JSON object out of a reply that should have been only JSON.
///
/// `response_format: json_object` is a request, not a guarantee: OpenRouter
/// drops parameters a provider does not support, and free Gemma endpoints then
/// answer with a ```json fence or a sentence around the object. Taking the
/// outermost braces covers both without a second model call.
fn extract_json(raw: &str) -> &str {
    match (raw.find('{'), raw.rfind('}')) {
        (Some(start), Some(end)) if start < end => &raw[start..=end],
        _ => raw,
    }
}

/// Talks to any OpenAI-compatible `/chat/completions` endpoint.
pub struct LlmClient {
    client: reqwest::Client,
    /// Base including the version segment, e.g. `http://host:port/v1`.
    base_url: String,
    api_key: String,
    model: String,
    reasoning: bool,
}

impl LlmClient {
    pub fn new(base_url: String, api_key: String, model: String, reasoning: bool) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
            reasoning,
        }
    }

    fn reasoning(&self) -> Option<Reasoning> {
        self.reasoning.then_some(Reasoning { enabled: true })
    }

    pub fn models_url(&self) -> String {
        format!("{}/models", self.base_url)
    }

    fn auth<'a>(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.api_key.is_empty() {
            rb
        } else {
            rb.bearer_auth(&self.api_key)
        }
    }

    pub fn health_request(&self) -> reqwest::RequestBuilder {
        self.auth(self.client.get(self.models_url()))
    }

    async fn call_once(&self, text: &str) -> Result<ExtractedQuery, AppError> {
        // Rebuilt every call so a long-running server keeps resolving "today"
        // against the real date rather than the date it booted on.
        let prompt = system_prompt();
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                Message { role: "system", content: &prompt },
                Message { role: "user", content: text },
            ],
            response_format: ResponseFormat { kind: "json_object" },
            stream: false,
            // Deterministic extraction: we want the same sentence to map to the
            // same query object every time, not creative paraphrasing.
            temperature: 0.0,
            reasoning: self.reasoning(),
        };

        let resp = self
            .auth(self.client.post(format!("{}/chat/completions", self.base_url)))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "llm request failed");
                AppError::LlmUnavailable
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            tracing::warn!(%status, detail = %detail, "llm returned non-success");
            return Err(AppError::LlmUnavailable);
        }

        let parsed: ChatResponse = resp.json().await.map_err(|e| {
            tracing::warn!(error = %e, "failed to decode chat completion envelope");
            AppError::LlmUnavailable
        })?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| AppError::Llm("no content returned".to_string()))?;

        let answer = strip_reasoning(&content);
        serde_json::from_str::<ExtractedQuery>(extract_json(&answer))
            .map_err(|e| AppError::Llm(format!("{e}: {content}")))
    }

    /// Free-text completion used for the statistics briefing. Returns plain
    /// prose, so no `response_format` and no schema retry.
    ///
    /// Never fails the request: the map, the table and the chart are the
    /// answer, and a missing paragraph of commentary is not worth turning a
    /// good result into an error page.
    pub async fn narrate(&self, system: &str, user: &str) -> Option<String> {
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                Message { role: "system", content: system },
                Message { role: "user", content: user },
            ],
            response_format: ResponseFormat { kind: "text" },
            stream: false,
            // A touch above zero: this is prose, and 0.0 makes it read like a
            // form letter across similar queries.
            temperature: 0.3,
            reasoning: self.reasoning(),
        };

        let resp = self
            .auth(self.client.post(format!("{}/chat/completions", self.base_url)))
            .json(&body)
            .send()
            .await
            .inspect_err(|e| tracing::warn!(error = %e, "analysis request failed"))
            .ok()?;

        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "analysis returned non-success");
            return None;
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .inspect_err(|e| tracing::warn!(error = %e, "failed to decode analysis response"))
            .ok()?;

        let text = parsed.choices.into_iter().next()?.message.content?;
        let text = strip_reasoning(&text);
        (!text.is_empty()).then(|| text.to_string())
    }

    /// Calls the model and parses `ExtractedQuery`, retrying once on schema
    /// mismatch per README §10 (never retries a plain connectivity failure).
    pub async fn extract(&self, text: &str) -> Result<ExtractedQuery, AppError> {
        match self.call_once(text).await {
            Ok(q) => Ok(q),
            Err(AppError::LlmUnavailable) => Err(AppError::LlmUnavailable),
            Err(e) => {
                tracing::warn!(error = %e, "llm returned invalid schema, retrying once");
                self.call_once(text).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_json, strip_reasoning};

    #[test]
    fn unwraps_a_fenced_json_reply() {
        let raw = "```json\n{\"hazard\": \"flood\"}\n```";
        assert_eq!(extract_json(raw), "{\"hazard\": \"flood\"}");
    }

    #[test]
    fn leaves_bare_json_alone() {
        assert_eq!(extract_json("{\"a\":{\"b\":1}}"), "{\"a\":{\"b\":1}}");
    }

    #[test]
    fn removes_the_think_block() {
        let raw = "<think>Let me plan. The user wants Thai.</think>\nน้ำท่วมในอยุธยา พบ 100 จุด";
        assert_eq!(strip_reasoning(raw), "น้ำท่วมในอยุธยา พบ 100 จุด");
    }

    #[test]
    fn keeps_text_after_the_last_close_tag() {
        // Some replies re-open the block; only what follows the final close is
        // the answer.
        let raw = "<think>a</think>draft<think>b</think>final answer";
        assert_eq!(strip_reasoning(raw), "final answer");
    }

    #[test]
    fn unterminated_reasoning_yields_nothing() {
        // Truncated mid-thought: there is no answer, and showing the monologue
        // is worse than showing no commentary at all.
        assert_eq!(strip_reasoning("<think>still thinking and cut off"), "");
    }

    #[test]
    fn plain_replies_pass_through() {
        assert_eq!(strip_reasoning("  a normal answer  "), "a normal answer");
    }
}
