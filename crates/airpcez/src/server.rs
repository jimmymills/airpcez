use airpcez_core::stats::StatsProvider;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
    routing::get,
    Json, Router,
};
use std::{sync::Arc, time::Duration};

type Provider = Arc<dyn StatsProvider>;

pub async fn run_server(port: u16, provider: Provider) {
    let app = Router::new()
        .route("/stats", get(stats_handler))
        .route("/ws", get(ws_handler))
        .with_state(provider);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn stats_handler(State(p): State<Provider>) -> Json<airpcez_core::model::NodeStats> {
    Json(p.sample())
}

async fn ws_handler(ws: WebSocketUpgrade, State(p): State<Provider>) -> Response {
    ws.on_upgrade(move |socket| ws_loop(socket, p))
}

async fn ws_loop(mut socket: WebSocket, p: Provider) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let json = serde_json::to_string(&p.sample()).unwrap();
        if socket.send(Message::Text(json)).await.is_err() {
            break;
        }
    }
}
