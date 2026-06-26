use airpcez_core::cluster::NodeEntry;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

fn mock_stats() -> NodeStats {
    NodeStats { name: "h".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None, running: false, sampled_at_unix: 0 }
}

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

#[tokio::test]
async fn add_then_remove_node_persists_to_config_file() {
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats: mock_stats() }));
    let path = state.config_path.clone();
    let _ = std::fs::remove_file(&path);
    tokio::spawn(airpcez::server::run_server(19104, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let c = reqwest::Client::new();

    // POST /nodes — node should be written to the toml file
    let _added: Vec<NodeEntry> = c.post("http://127.0.0.1:19104/nodes")
        .json(&NodeEntry { name: "w1".into(), addr: "10.9.9.9:8675".into() })
        .send().await.unwrap().json().await.unwrap();
    let contents = std::fs::read_to_string(&path).expect("config file should exist after POST /nodes");
    assert!(contents.contains("10.9.9.9:8675"), "addr should be in toml after add; got: {contents}");

    // DELETE /nodes — node should be removed from the toml file
    let _after: Vec<NodeEntry> = c.delete("http://127.0.0.1:19104/nodes")
        .json(&serde_json::json!({"addr":"10.9.9.9:8675"}))
        .send().await.unwrap().json().await.unwrap();
    let contents = std::fs::read_to_string(&path).expect("config file should exist after DELETE /nodes");
    assert!(!contents.contains("10.9.9.9"), "addr should be absent from toml after remove; got: {contents}");
}
