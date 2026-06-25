use airpcez::stats_provider::LocalStats;
use airpcez_core::model::Role;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = Arc::new(LocalStats {
        name: sysinfo::System::host_name().unwrap_or_else(|| "airpcez-node".to_string()),
        role: Role::Worker,
    });
    airpcez::server::run_server(8675, provider).await;
}
