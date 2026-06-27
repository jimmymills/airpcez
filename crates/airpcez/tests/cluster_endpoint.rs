use airpcez_core::cluster::ClusterResponse;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn cluster_endpoint_includes_self_and_totals() {
    let stats = NodeStats { name: "host".into(), role: Role::Host, ram_total_mib: 16,
        ram_free_mib: 8, cpu_logical: 8, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19102, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let cs: ClusterResponse = reqwest::get("http://127.0.0.1:19102/cluster")
        .await.unwrap().json().await.unwrap();
    assert_eq!(cs.status.nodes.len(), 1); // just self (no workers configured)
    assert_eq!(cs.status.nodes[0].entry.name, "host");
    assert!(cs.status.nodes[0].reachable && cs.status.nodes[0].stats.is_some());
    // self has 16 MiB RAM and no GPU -> pool == ram, vram == 0
    assert_eq!(cs.totals.ram_total_mib, 16);
    assert_eq!(cs.totals.ram_free_mib, 8);
    assert_eq!(cs.totals.vram_total_mib, 0);
    assert_eq!(cs.totals.pool_total_mib, 16);
    assert_eq!(cs.totals.pool_free_mib, 8);
}
