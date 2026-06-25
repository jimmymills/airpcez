use airpcez::server::AppState;
use airpcez::stats_provider::LocalStats;
use airpcez::supervisor::TokioSupervisor;
use airpcez_core::model::Role;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = Arc::new(LocalStats {
        name: sysinfo::System::host_name().unwrap_or_else(|| "airpcez-node".to_string()),
        role: Role::Worker,
    });
    let state = AppState {
        provider,
        supervisor: Arc::new(TokioSupervisor::new()),
    };
    airpcez::server::run_server(8675, state).await;
}
