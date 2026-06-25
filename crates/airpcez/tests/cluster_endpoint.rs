use airpcez_core::cluster::ClusterStatus;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn cluster_endpoint_includes_self() {
    let stats = NodeStats { name: "host".into(), role: Role::Host, ram_total_mib: 16,
        ram_free_mib: 8, cpu_logical: 8, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19102, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let cs: ClusterStatus = reqwest::get("http://127.0.0.1:19102/cluster")
        .await.unwrap().json().await.unwrap();
    assert_eq!(cs.nodes.len(), 1); // just self (no workers configured)
    assert_eq!(cs.nodes[0].entry.name, "host");
    assert!(cs.nodes[0].reachable && cs.nodes[0].stats.is_some());
}
