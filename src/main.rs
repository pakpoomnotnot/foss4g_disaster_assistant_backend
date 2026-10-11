mod analysis;
mod config;
mod error;
mod gazetteer;
mod gistda;
mod handlers;
mod lang;
mod llm;
mod models;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::Config>,
    pub gazetteer: Arc<Vec<models::GazetteerEntry>>,
    pub http: reqwest::Client,
    pub llm: Arc<llm::openai::LlmClient>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = config::Config::from_env()?;

    tracing::info!(path = %config.gazetteer_path, "loading gazetteer");
    let gazetteer = gazetteer::loader::load(&config.gazetteer_path)?;
    tracing::info!(entries = gazetteer.len(), "gazetteer loaded");

    let http = reqwest::Client::new();
    let llm = llm::openai::LlmClient::new(
        config.llm_base_url.clone(),
        config.llm_api_key.clone(),
        config.llm_model.clone(),
        config.llm_reasoning,
    );
    tracing::info!(
        base = %config.llm_base_url,
        model = %config.llm_model,
        reasoning = config.llm_reasoning,
        "llm configured"
    );
    let port = config.port;

    let state = AppState {
        config: Arc::new(config),
        gazetteer: Arc::new(gazetteer),
        http,
        llm: Arc::new(llm),
    };

    // Permissive for now (v1/demo, matches the rest of this backend's Phase 1 scope) —
    // the Next.js frontend runs on a different origin/port, so without this every
    // browser request would be blocked before it reaches a handler. Lock this down
    // to the real frontend origin before production.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/query", post(handlers::query::handle_query))
        .route("/api/health", get(handlers::health::health))
        .with_state(state)
        .layer(cors);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "askme-disaster backend listening");
    axum::serve(listener, app).await?;

    Ok(())
}
