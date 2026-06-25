use airpcez::poller::poll_nodes;
use airpcez_core::cluster::NodeEntry;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn polls_reachable_and_unreachable() {
    // Stand up a real airpcez server serving a mock /stats on one port.
    let stats = NodeStats { name: "up".into(), role: Role::Worker, ram_total_mib: 8,
        ram_free_mib: 4, cpu_logical: 4, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState {
        provider: Arc::new(MockStatsProvider { stats }),
        supervisor: Arc::new(airpcez::supervisor::TokioSupervisor::new()),
    };
    tokio::spawn(airpcez::server::run_server(19101, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let nodes = vec![
        NodeEntry { name: "up".into(),   addr: "127.0.0.1:19101".into() },
        NodeEntry { name: "down".into(), addr: "127.0.0.1:1".into() },     // nothing listening
    ];
    let cs = poll_nodes(&reqwest::Client::new(), &nodes).await;
    assert_eq!(cs.nodes.len(), 2);
    assert!(cs.nodes[0].reachable && cs.nodes[0].stats.as_ref().unwrap().name == "up");
    assert!(!cs.nodes[1].reachable && cs.nodes[1].stats.is_none());
}
