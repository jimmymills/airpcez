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
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
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

#[tokio::test]
async fn rewrites_rpc_endpoint_host() {
    // Stand up a server whose MockStatsProvider reports rpc_endpoint with 0.0.0.0.
    // The poller should rewrite the host part with the node's reachable IP (127.0.0.1).
    let stats = NodeStats {
        name: "rpc-node".into(), role: Role::Worker,
        ram_total_mib: 16, ram_free_mib: 8, cpu_logical: 4, devices: vec![],
        rpc_endpoint: Some("0.0.0.0:50052".into()),
        binary_version: None, running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19102, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let nodes = vec![NodeEntry { name: "rpc-node".into(), addr: "127.0.0.1:19102".into() }];
    let cs = poll_nodes(&reqwest::Client::new(), &nodes).await;
    assert_eq!(cs.nodes.len(), 1);
    let snap = &cs.nodes[0];
    assert!(snap.reachable);
    let ep = snap.stats.as_ref().unwrap().rpc_endpoint.as_deref().unwrap();
    assert_eq!(ep, "127.0.0.1:50052", "poller must rewrite 0.0.0.0 with node's reachable IP");
}
