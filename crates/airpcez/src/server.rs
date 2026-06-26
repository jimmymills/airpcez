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
    pub llama_dir: Option<String>,
    pub llama_port: u16,
}

impl AppState {
    pub fn for_test(provider: Arc<dyn StatsProvider>) -> AppState {
        AppState {
            provider,
            supervisor: Arc::new(crate::supervisor::TokioSupervisor::new()),
            nodes: Arc::new(std::sync::Mutex::new(Vec::new())),
            http: reqwest::Client::new(),
            llama_dir: None,
            llama_port: 8080,
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
        .route("/host/launch", post(host_launch))
        .route("/host/stop", post(host_stop))
        .route("/host/health", get(host_health))
        .route("/host/logs", get(host_logs))
        .route("/catalog", get(catalog_handler))
        .route("/suggest", post(suggest_handler))
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

#[derive(serde::Deserialize)]
struct LaunchRequest {
    model_hf: Option<String>,
    model_path: Option<String>,
    ngl: Option<u32>,
    tensor_split: Option<String>,
    main_gpu: Option<u32>,
    device: Option<String>,
    cpu_moe: Option<String>,
    ctx: Option<u32>,
}

async fn host_launch(State(s): State<AppState>, Json(req): Json<LaunchRequest>) -> impl IntoResponse {
    use airpcez_core::flags::*;
    let model = match (req.model_hf, req.model_path) {
        (Some(hf), _) => ModelRef::Hf(hf),
        (None, Some(p)) => ModelRef::Local(p),
        (None, None) => return (StatusCode::BAD_REQUEST, "model_hf or model_path required".to_string()).into_response(),
    };
    let cpu_moe = match req.cpu_moe.as_deref() {
        Some("all") => CpuMoe::All,
        Some("off") | None => CpuMoe::Off,
        Some(n) => match n.parse::<u32>() { Ok(v) => CpuMoe::NLayers(v), Err(_) => CpuMoe::Off },
    };
    let nodes = { s.nodes.lock().unwrap().clone() };
    let cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    let eps: Vec<String> = cluster.nodes.iter()
        .filter_map(|n| n.stats.as_ref().and_then(|st| st.rpc_endpoint.clone()))
        .collect();
    let binary = match &s.llama_dir {
        Some(d) => format!("{d}/llama-server"),
        None => "llama-server".to_string(),
    };
    let opts = LlamaServerOpts {
        binary: &binary, model: &model, rpc_endpoints: &eps,
        ngl: req.ngl, tensor_split: req.tensor_split.as_deref(), main_gpu: req.main_gpu,
        device: req.device.as_deref(), cpu_moe: &cpu_moe, ctx: req.ctx,
        host: "0.0.0.0", port: s.llama_port,
    };
    match s.supervisor.start(llama_server_spec(&opts)) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({
            "openai_url": format!("http://localhost:{}/v1", s.llama_port)
        }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(serde_json::json!({"error": e}))).into_response(),
    }
}

async fn host_stop(State(s): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "stopped": s.supervisor.stop() })))
}

/// Server-side readiness probe: a launched `llama-server` returns 200 on /health only
/// once the model is loaded (503 while loading). The cockpit polls this (same-origin,
/// no CORS) so it shows "loading… → ready" instead of a premature "launched".
async fn host_health(State(s): State<AppState>) -> Json<serde_json::Value> {
    let url = format!("http://localhost:{}/health", s.llama_port);
    match s.http.get(&url).timeout(Duration::from_secs(2)).send().await {
        Ok(r) if r.status().is_success() => {
            Json(serde_json::json!({ "ready": true, "detail": "model ready" }))
        }
        Ok(r) => Json(serde_json::json!({
            "ready": false, "detail": format!("loading model (HTTP {})", r.status().as_u16())
        })),
        Err(_) => Json(serde_json::json!({
            "ready": false, "detail": "starting — llama-server not responding yet"
        })),
    }
}

/// The supervised process (llama-server / rpc-server) has its stdout+stderr captured
/// into a rolling buffer. Expose it so launch failures are diagnosable from the cockpit
/// instead of vanishing into a piped child process.
async fn host_logs(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": format!("{:?}", s.supervisor.status()),
        "lines": s.supervisor.recent_logs(),
    }))
}

async fn catalog_handler() -> Json<Vec<crate::catalog::CatalogEntry>> {
    Json(crate::catalog::model_catalog())
}

#[derive(serde::Deserialize)]
struct SuggestRequest { meta: airpcez_core::planner::ModelMeta, ctx: u32 }

async fn suggest_handler(State(s): State<AppState>, Json(req): Json<SuggestRequest>)
    -> Json<airpcez_core::planner::Plan> {
    use airpcez_core::cluster::*;
    let self_stats = s.provider.sample();
    let self_version = self_stats.binary_version.clone();
    let self_snap = NodeSnapshot {
        entry: NodeEntry { name: self_stats.name.clone(), addr: "self".into() },
        stats: Some(self_stats), reachable: true, error: None,
    };
    let nodes = { s.nodes.lock().unwrap().clone() };
    let mut cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    let warnings = airpcez_core::cluster::version_warnings(self_version.as_deref(), &cluster.nodes);
    cluster.nodes.insert(0, self_snap);
    let mut plan = airpcez_core::planner::suggest_plan(&cluster, &req.meta, req.ctx);
    plan.warnings = warnings;
    Json(plan)
}
