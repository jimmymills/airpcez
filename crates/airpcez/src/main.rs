use airpcez::stats_provider::LocalStats;
use airpcez_core::model::Role;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = Arc::new(LocalStats {
        name: hostname(),
        role: Role::Worker,
    });
    airpcez::server::run_server(8675, provider).await;
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
