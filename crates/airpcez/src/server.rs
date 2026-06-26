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
    pub config: Arc<std::sync::Mutex<crate::config::Config>>,
    pub http: reqwest::Client,
    pub config_path: std::path::PathBuf,
    pub bound_ui_port: u16,
}

impl AppState {
    pub fn for_test(provider: Arc<dyn StatsProvider>) -> AppState {
        AppState {
            provider,
            supervisor: Arc::new(crate::supervisor::TokioSupervisor::new()),
            config: Arc::new(std::sync::Mutex::new(crate::config::Config::default())),
            http: reqwest::Client::new(),
            config_path: std::env::temp_dir().join("airpcez-test-config.toml"),
            bound_ui_port: 8675,
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
        .route("/config", get(get_config).post(post_config))
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
    let nodes = { s.config.lock().unwrap().nodes.clone() };
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

/// The default airpcez UI port; a worker node is polled at `http://<addr>/stats`.
const DEFAULT_UI_PORT: u16 = 8675;

/// Append the default UI port to a bare host/IP so it's pollable. "192.168.0.83" ->
/// "192.168.0.83:8675"; an addr that already carries a `:port` is left as typed.
fn normalize_node_addr(addr: &str) -> String {
    let addr = addr.trim();
    if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_UI_PORT}")
    }
}

async fn add_node(State(s): State<AppState>, Json(entry): Json<NodeEntry>)
    -> Json<Vec<NodeEntry>> {
    let entry = NodeEntry { name: entry.name, addr: normalize_node_addr(&entry.addr) };
    let mut g = s.config.lock().unwrap();
    if !g.nodes.iter().any(|n| n.addr == entry.addr) {
        g.nodes.push(entry);
    }
    Json(g.nodes.clone())
}

#[derive(serde::Deserialize)]
struct RemoveNode {
    addr: String,
}

async fn remove_node(State(s): State<AppState>, Json(req): Json<RemoveNode>)
    -> Json<Vec<NodeEntry>> {
    let addr = normalize_node_addr(&req.addr);
    let mut g = s.config.lock().unwrap();
    g.nodes.retain(|n| n.addr != addr);
    Json(g.nodes.clone())
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
    hf_cache_dir: Option<String>,
}

/// "unsloth/Qwen3.6-35B-A3B-GGUF" -> "models--unsloth--Qwen3.6-35B-A3B-GGUF" (HF hub layout).
fn hf_cache_dirname(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}

/// Pick the GGUF from a directory listing: filter by `quant` substring when given, then
/// prefer the first shard of a sharded model (`*-00001-of-*.gguf`), else the sole `.gguf`.
fn pick_gguf(names: Vec<String>, quant: Option<&str>) -> Option<String> {
    let mut ggufs: Vec<String> = names.into_iter()
        .filter(|n| n.to_lowercase().ends_with(".gguf"))
        .collect();
    if let Some(q) = quant {
        let ql = q.to_lowercase();
        let matched: Vec<String> = ggufs.iter().filter(|n| n.to_lowercase().contains(&ql)).cloned().collect();
        if !matched.is_empty() {
            ggufs = matched;
        }
    }
    if let Some(shard) = ggufs.iter().find(|n| n.contains("-00001-of-")) {
        return Some(shard.clone());
    }
    if ggufs.len() == 1 {
        return ggufs.into_iter().next();
    }
    None
}

/// Find a .gguf inside a directory, descending into a HuggingFace `snapshots/<hash>/`
/// cache layout (newest snapshot) when present. `quant` narrows multi-quant dirs.
fn find_gguf_in_dir(dir: &std::path::Path, quant: Option<&str>) -> Option<String> {
    let snapshots = dir.join("snapshots");
    let search = if snapshots.is_dir() {
        std::fs::read_dir(&snapshots).ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|d| d.is_dir())
            .max_by_key(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok())?
    } else {
        dir.to_path_buf()
    };
    let names: Vec<String> = std::fs::read_dir(&search).ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    pick_gguf(names, quant).map(|f| search.join(f).to_string_lossy().into_owned())
}

/// Resolve a user-supplied `-m` path: a file is used as-is; a directory (incl. an HF
/// `models--org--repo/` cache dir) is resolved to its .gguf.
fn resolve_model_path(path: &str) -> Result<String, String> {
    let p = std::path::Path::new(path);
    if p.is_file() {
        return Ok(path.to_string());
    }
    if !p.exists() {
        return Err(format!("model path does not exist: {path}"));
    }
    find_gguf_in_dir(p, None)
        .ok_or_else(|| format!("no single .gguf in {path} — point at the exact .gguf file (none, or several quants)"))
}

/// Try to resolve an `org/repo:quant` selection to a locally-cached .gguf under an HF hub
/// cache dir, so a cached model loads via `-m` without any download. None → not cached.
fn resolve_hf_in_cache(cache_dir: &str, hf: &str) -> Option<String> {
    let (repo, quant) = match hf.rsplit_once(':') {
        Some((r, q)) => (r, Some(q)),
        None => (hf, None),
    };
    let repo_dir = std::path::Path::new(cache_dir).join(hf_cache_dirname(repo));
    if !repo_dir.is_dir() {
        return None;
    }
    find_gguf_in_dir(&repo_dir, quant)
}

async fn host_launch(State(s): State<AppState>, Json(req): Json<LaunchRequest>) -> impl IntoResponse {
    use airpcez_core::flags::*;
    // Extract config values before any await — never hold the Mutex across an await.
    let (llama_dir, llama_port, hf_cache_dir_cfg) = {
        let c = s.config.lock().unwrap();
        (c.llama_dir.clone(), c.llama_port, c.hf_cache_dir.clone())
    };
    // Effective HF cache dir: per-launch field overrides the host config default.
    let cache_dir = req.hf_cache_dir.as_deref().map(str::trim).filter(|s| !s.is_empty())
        .or(hf_cache_dir_cfg.as_deref());
    let model = match (req.model_hf, req.model_path) {
        (Some(hf), _) if !hf.trim().is_empty() => {
            let hf = hf.trim();
            // Prefer a locally-cached copy (load via -m, no download) when a cache dir is set.
            match cache_dir.and_then(|c| resolve_hf_in_cache(c, hf)) {
                Some(local) => ModelRef::Local(local),
                None => ModelRef::Hf(hf.to_string()),
            }
        }
        (_, Some(p)) if !p.trim().is_empty() => match resolve_model_path(p.trim()) {
            Ok(file) => ModelRef::Local(file),
            Err(e) => return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({ "error": e }))).into_response(),
        },
        _ => return (StatusCode::BAD_REQUEST, "model_hf or model_path required".to_string()).into_response(),
    };
    let cpu_moe = match req.cpu_moe.as_deref() {
        Some("all") => CpuMoe::All,
        Some("off") | None => CpuMoe::Off,
        Some(n) => match n.parse::<u32>() { Ok(v) => CpuMoe::NLayers(v), Err(_) => CpuMoe::Off },
    };
    let nodes = { s.config.lock().unwrap().nodes.clone() };
    let cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    let eps: Vec<String> = cluster.nodes.iter()
        .filter_map(|n| n.stats.as_ref().and_then(|st| st.rpc_endpoint.clone()))
        .collect();
    let binary = match &llama_dir {
        Some(d) => format!("{d}/llama-server"),
        None => "llama-server".to_string(),
    };
    let opts = LlamaServerOpts {
        binary: &binary, model: &model, rpc_endpoints: &eps,
        ngl: req.ngl, tensor_split: req.tensor_split.as_deref(), main_gpu: req.main_gpu,
        device: req.device.as_deref(), cpu_moe: &cpu_moe, ctx: req.ctx,
        host: "0.0.0.0", port: llama_port,
    };
    match s.supervisor.start(llama_server_spec(&opts)) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({
            "openai_url": format!("http://localhost:{}/v1", llama_port)
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
    let llama_port = s.config.lock().unwrap().llama_port;
    let url = format!("http://localhost:{}/health", llama_port);
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
    let nodes = { s.config.lock().unwrap().nodes.clone() };
    let mut cluster = crate::poller::poll_nodes(&s.http, &nodes).await;
    let warnings = airpcez_core::cluster::version_warnings(self_version.as_deref(), &cluster.nodes);
    cluster.nodes.insert(0, self_snap);
    let mut plan = airpcez_core::planner::suggest_plan(&cluster, &req.meta, req.ctx);
    plan.warnings = warnings;
    Json(plan)
}

async fn get_config(State(s): State<AppState>) -> Json<crate::config::Config> {
    Json(s.config.lock().unwrap().clone())
}

async fn post_config(State(s): State<AppState>, Json(new): Json<crate::config::Config>) -> impl IntoResponse {
    let restart_required = new.ui_port != s.bound_ui_port;
    let save = { let mut c = s.config.lock().unwrap(); *c = new; c.save(&s.config_path) };
    match save {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "saved": true, "restart_required": restart_required }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "saved": false, "error": e }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::{hf_cache_dirname, pick_gguf};
    #[test]
    fn pick_gguf_prefers_first_shard_then_sole_file() {
        assert_eq!(pick_gguf(vec!["a.txt".into(), "model.gguf".into()], None).as_deref(), Some("model.gguf"));
        assert_eq!(
            pick_gguf(vec![
                "m-00002-of-00003.gguf".into(),
                "m-00001-of-00003.gguf".into(),
                "m-00003-of-00003.gguf".into(),
            ], None).as_deref(),
            Some("m-00001-of-00003.gguf")
        );
        assert_eq!(pick_gguf(vec!["q4.gguf".into(), "q8.gguf".into()], None), None); // ambiguous quants
        assert_eq!(pick_gguf(vec!["readme.md".into()], None), None); // no gguf present
    }
    #[test]
    fn pick_gguf_narrows_by_quant() {
        let files = vec![
            "Qwen3.6-35B-A3B-Q4_K_M.gguf".to_string(),
            "Qwen3.6-35B-A3B-Q8_0.gguf".to_string(),
        ];
        // Without a quant the two are ambiguous; with one it resolves.
        assert_eq!(pick_gguf(files.clone(), None), None);
        assert_eq!(pick_gguf(files, Some("Q4_K_M")).as_deref(), Some("Qwen3.6-35B-A3B-Q4_K_M.gguf"));
    }
    #[test]
    fn hf_repo_maps_to_cache_dirname() {
        assert_eq!(hf_cache_dirname("unsloth/Qwen3.6-35B-A3B-GGUF"), "models--unsloth--Qwen3.6-35B-A3B-GGUF");
    }
    #[test]
    fn normalize_node_addr_appends_default_port() {
        assert_eq!(super::normalize_node_addr("192.168.0.83"), "192.168.0.83:8675");
        assert_eq!(super::normalize_node_addr("  192.168.0.83 "), "192.168.0.83:8675"); // trims
        assert_eq!(super::normalize_node_addr("192.168.0.83:9000"), "192.168.0.83:9000"); // keeps port
        assert_eq!(super::normalize_node_addr("worker.local"), "worker.local:8675");
    }
    #[test]
    fn resolve_hf_in_cache_finds_local_gguf_by_quant() {
        use std::fs;
        let base = std::env::temp_dir().join("airpcez_hf_cache_test");
        let _ = fs::remove_dir_all(&base);
        let snap = base.join("models--unsloth--Foo-GGUF").join("snapshots").join("abc123");
        fs::create_dir_all(&snap).unwrap();
        fs::write(snap.join("Foo-Q4_K_M.gguf"), b"x").unwrap();
        fs::write(snap.join("Foo-Q8_0.gguf"), b"x").unwrap();

        let cache = base.to_str().unwrap();
        assert_eq!(
            super::resolve_hf_in_cache(cache, "unsloth/Foo-GGUF:Q4_K_M").as_deref(),
            Some(snap.join("Foo-Q4_K_M.gguf").to_str().unwrap())
        );
        // Not in the cache → None (caller then falls back to a real -hf download).
        assert_eq!(super::resolve_hf_in_cache(cache, "unsloth/Bar-GGUF:Q4_K_M"), None);
        let _ = fs::remove_dir_all(&base);
    }
}
