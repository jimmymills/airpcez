use airpcez_core::stats::StatsProvider;
use axum::{extract::State, routing::get, Json, Router};
use std::sync::Arc;

type Provider = Arc<dyn StatsProvider>;

pub async fn run_server(port: u16, provider: Provider) {
    let app = Router::new()
        .route("/stats", get(stats_handler))
        .with_state(provider);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn stats_handler(State(p): State<Provider>) -> Json<airpcez_core::model::NodeStats> {
    Json(p.sample())
}
