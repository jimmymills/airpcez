use airpcez_core::cluster::NodeEntry;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn add_then_remove_node() {
    let stats = NodeStats { name: "h".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None, running: false, sampled_at_unix: 0 };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19103, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let c = reqwest::Client::new();
    let added: Vec<NodeEntry> = c.post("http://127.0.0.1:19103/nodes")
        .json(&NodeEntry { name: "w".into(), addr: "192.168.0.9:8675".into() })
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(added.len(), 1);
    let after: Vec<NodeEntry> = c.delete("http://127.0.0.1:19103/nodes")
        .json(&serde_json::json!({"addr":"192.168.0.9:8675"}))
        .send().await.unwrap().json().await.unwrap();
    assert!(after.is_empty());
}
