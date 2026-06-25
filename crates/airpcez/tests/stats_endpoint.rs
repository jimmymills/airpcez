use airpcez::server::AppState;
use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use std::sync::Arc;

#[tokio::test]
async fn stats_endpoint_returns_node_stats() {
    let stats = NodeStats {
        name: "test".into(), role: Role::Worker, ram_total_mib: 16, ram_free_mib: 8,
        cpu_logical: 4, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let state = AppState::for_test(Arc::new(MockStatsProvider { stats: stats.clone() }));
    tokio::spawn(airpcez::server::run_server(18675, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let got: NodeStats = reqwest::get("http://127.0.0.1:18675/stats")
        .await.unwrap().json().await.unwrap();
    assert_eq!(got, stats);
}
