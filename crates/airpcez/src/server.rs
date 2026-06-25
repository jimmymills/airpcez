use airpcez_core::{cluster::NodeEntry, process::ProcessBackend, stats::StatsProvider};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct AppState {
    pub provider: Arc<dyn StatsProvider>,
    pub supervisor: Arc<dyn ProcessBackend>,
    pub nodes: Arc<std::sync::Mutex<Vec<NodeEntry>>>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn for_test(provider: Arc<dyn StatsProvider>) -> AppState {
        AppState {
            provider,
            supervisor: Arc::new(crate::supervisor::TokioSupervisor::new()),
            nodes: Arc::new(std::sync::Mutex::new(Vec::new())),
            http: reqwest::Client::new(),
        }
    }
}

pub async fn run_server(port: u16, state: AppState) {
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/stats", get(stats_handler))
        .route("/ws", get(ws_handler))
        .route("/cluster", get(cluster_handler))
        .route("/worker/start", post(worker_start_handler))
        .route("/worker/stop", post(worker_stop_handler))
        .route("/nodes", post(add_node).delete(remove_node))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn stats_handler(State(s): State<AppState>) -> Json<airpcez_core::model::NodeStats> {
    Json(s.provider.sample())
}

async fn ws_handler(ws: WebSocketUpgrade, State(s): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| ws_loop(socket, s.provider))
}

async fn ws_loop(mut socket: WebSocket, p: Arc<dyn StatsProvider>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let json = serde_json::to_string(&p.sample()).unwrap();
        if socket.send(Message::Text(json)).await.is_err() {
            break;
        }
    }
}

async fn cluster_handler(State(s): State<AppState>) -> Json<airpcez_core::cluster::ClusterStatus> {
    use airpcez_core::cluster::*;
    let self_stats = s.provider.sample();
    let self_version = self_stats.binary_version.clone();
    let self_snap = NodeSnapshot {
        entry: NodeEntry { name: self_stats.name.clone(), addr: "self".into() },
        stats: Some(self_stats), reachable: true, error: None,
    };
    let nodes = { s.nodes.lock().unwrap().clone() };
    let mut cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    cluster.warnings = version_warnings(self_version.as_deref(), &cluster.nodes);
    cluster.nodes.insert(0, self_snap);
    Json(cluster)
}

#[derive(serde::Deserialize)]
struct WorkerStartRequest {
    binary: String,
    rpc_port: u16,
    device: Option<String>,
}

async fn worker_start_handler(
    State(s): State<AppState>,
    Json(req): Json<WorkerStartRequest>,
) -> impl IntoResponse {
    let spec = airpcez_core::flags::rpc_server_spec(
        &req.binary,
        "0.0.0.0",
        req.rpc_port,
        req.device.as_deref(),
    );
    match s.supervisor.start(spec) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn worker_stop_handler(State(s): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, axum::Json(serde_json::json!({ "stopped": s.supervisor.stop() })))
}

async fn add_node(State(s): State<AppState>, Json(entry): Json<NodeEntry>)
    -> Json<Vec<NodeEntry>> {
    let mut g = s.nodes.lock().unwrap();
    if !g.iter().any(|n| n.addr == entry.addr) {
        g.push(entry);
    }
    Json(g.clone())
}

#[derive(serde::Deserialize)]
struct RemoveNode {
    addr: String,
}

async fn remove_node(State(s): State<AppState>, Json(req): Json<RemoveNode>)
    -> Json<Vec<NodeEntry>> {
    let mut g = s.nodes.lock().unwrap();
    g.retain(|n| n.addr != req.addr);
    Json(g.clone())
}

async fn serve_index() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../assets/index.html"))
}
