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
    let mut loaded = airpcez::config::apply_cli_overrides(
        Config::load(config_path),
        &std::env::args().collect::<Vec<_>>(),
    );
    if worker_mode {
        loaded.role = Role::Worker;
    }

    let bound_ui_port = loaded.ui_port;
    let rpc_port = loaded.rpc_port;
    let rpc_bin = loaded.rpc_binary_path();
    let config = Arc::new(Mutex::new(loaded));
    let provider = Arc::new(LocalStats { config: config.clone() });
    let supervisor: Arc<dyn ProcessBackend> = Arc::new(TokioSupervisor::new());
    let state = AppState {
        provider,
        supervisor: supervisor.clone(),
        config: config.clone(),
        http: reqwest::Client::new(),
        config_path: config_path.to_path_buf(),
        bound_ui_port,
    };

    if worker_mode {
        let spec = airpcez_core::flags::rpc_server_spec(&rpc_bin, "0.0.0.0", rpc_port, None);
        match supervisor.start(spec) {
            Ok(()) => eprintln!("[airpcez] --worker: started rpc-server `{rpc_bin}` on 0.0.0.0:{rpc_port}"),
            Err(e) => {
                eprintln!("[airpcez] --worker: FAILED to start rpc-server `{rpc_bin}`: {e}");
                eprintln!("[airpcez]   set `rpc_binary = \"/abs/path/to/rpc-server\"` (or `llama_dir`) in ./airpcez.toml, run from that dir");
            }
        }
    }

    eprintln!(
        "[airpcez] listening on http://0.0.0.0:{}{}",
        bound_ui_port,
        if worker_mode { "  (worker: rpc-server autostarted)" } else { "" }
    );
    airpcez::server::run_server(bound_ui_port, state).await;
}
