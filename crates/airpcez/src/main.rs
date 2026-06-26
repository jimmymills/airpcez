use airpcez::config::Config;
use airpcez::server::AppState;
use airpcez::stats_provider::LocalStats;
use airpcez::supervisor::TokioSupervisor;
use airpcez_core::model::Role;
use airpcez_core::process::ProcessBackend;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() {
    // `--worker` forces the worker role and auto-starts rpc-server on boot, so a worker
    // box is fully ready as soon as airpcez launches (no manual "Start worker" step).
    let worker_mode = std::env::args().any(|a| a == "--worker");

    // Config is loaded from ./airpcez.toml (next to the binary / cwd).
    // If the file is absent or unparseable the compiled-in defaults are used.
    let config_path = Path::new("airpcez.toml");
    let mut config = Config::load(config_path);
    if worker_mode {
        config.role = Role::Worker;
    }

    let provider = Arc::new(LocalStats {
        name: config.node_name.clone(),
        role: config.role,
        llama_dir: config.llama_dir.clone(),
        rpc_port: config.rpc_port,
    });
    let supervisor: Arc<dyn ProcessBackend> = Arc::new(TokioSupervisor::new());
    let state = AppState {
        provider,
        supervisor: supervisor.clone(),
        nodes: Arc::new(Mutex::new(config.nodes.clone())),
        http: reqwest::Client::new(),
        llama_dir: config.llama_dir.clone(),
        llama_port: config.llama_port,
        hf_cache_dir: config.hf_cache_dir.clone(),
    };

    if worker_mode {
        let bin = config.rpc_binary_path();
        let spec = airpcez_core::flags::rpc_server_spec(&bin, "0.0.0.0", config.rpc_port, None);
        match supervisor.start(spec) {
            Ok(()) => eprintln!("[airpcez] --worker: started rpc-server `{bin}` on 0.0.0.0:{}", config.rpc_port),
            Err(e) => eprintln!("[airpcez] --worker: FAILED to start rpc-server `{bin}`: {e}"),
        }
    }

    eprintln!(
        "[airpcez] listening on http://0.0.0.0:{}{}",
        config.ui_port,
        if worker_mode { "  (worker: rpc-server autostarted)" } else { "" }
    );
    airpcez::server::run_server(config.ui_port, state).await;
}
