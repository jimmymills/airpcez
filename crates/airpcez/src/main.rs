use airpcez::config::Config;
use airpcez::server::AppState;
use airpcez::stats_provider::LocalStats;
use airpcez::supervisor::TokioSupervisor;
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Config is loaded from ./airpcez.toml (next to the binary / cwd).
    // If the file is absent or unparseable the compiled-in defaults are used.
    let config_path = Path::new("airpcez.toml");
    let config = Config::load(config_path);

    let provider = Arc::new(LocalStats {
        name: config.node_name.clone(),
        role: config.role,
    });
    let state = AppState {
        provider,
        supervisor: Arc::new(TokioSupervisor::new()),
    };
    airpcez::server::run_server(config.ui_port, state).await;
}
