use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn get_config_returns_default_ui_port() {
    let stats = NodeStats {
        name: "host".into(), role: Role::Host, ram_total_mib: 8000, ram_free_mib: 4000,
        cpu_logical: 4, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19301, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let resp = client.get("http://127.0.0.1:19301/config")
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ui_port"], 8675);
}

#[tokio::test]
async fn post_config_updates_and_persists() {
    let stats = NodeStats {
        name: "host".into(), role: Role::Host, ram_total_mib: 8000, ram_free_mib: 4000,
        cpu_logical: 4, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19302, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();

    // Fetch current config and modify llama_port
    let mut cfg: serde_json::Value = client.get("http://127.0.0.1:19302/config")
        .send().await.unwrap().json().await.unwrap();
    cfg["llama_port"] = serde_json::json!(9999);

    let post_resp = client.post("http://127.0.0.1:19302/config")
        .json(&cfg)
        .send().await.unwrap();
    assert_eq!(post_resp.status(), 200);
    let post_body: serde_json::Value = post_resp.json().await.unwrap();
    assert_eq!(post_body["saved"], true);

    // Verify GET reflects the update
    let get_resp: serde_json::Value = client.get("http://127.0.0.1:19302/config")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(get_resp["llama_port"], 9999);
}

#[tokio::test]
async fn post_config_preserves_nodes_and_node_name() {
    let stats = NodeStats {
        name: "host".into(), role: Role::Host, ram_total_mib: 8000, ram_free_mib: 4000,
        cpu_logical: 4, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19303, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();

    // Add a worker node via /nodes
    client.post("http://127.0.0.1:19303/nodes")
        .json(&serde_json::json!({ "name": "w1", "addr": "10.0.0.5:8675" }))
        .send().await.unwrap();

    // Fetch current config, clobber nodes and node_name, then POST back
    let mut cfg: serde_json::Value = client.get("http://127.0.0.1:19303/config")
        .send().await.unwrap().json().await.unwrap();
    cfg["nodes"] = serde_json::json!([]);
    cfg["node_name"] = serde_json::json!("");

    let post_resp = client.post("http://127.0.0.1:19303/config")
        .json(&cfg)
        .send().await.unwrap();
    assert_eq!(post_resp.status(), 200);
    let post_body: serde_json::Value = post_resp.json().await.unwrap();
    assert_eq!(post_body["saved"], true);

    // GET config — nodes must still contain w1, node_name must not be empty
    let get_resp: serde_json::Value = client.get("http://127.0.0.1:19303/config")
        .send().await.unwrap().json().await.unwrap();
    let nodes = get_resp["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1, "nodes must not be clobbered by settings save");
    assert_eq!(nodes[0]["name"], "w1");
    assert!(
        !get_resp["node_name"].as_str().unwrap_or("").is_empty(),
        "node_name must not be wiped by an empty settings save"
    );
}
