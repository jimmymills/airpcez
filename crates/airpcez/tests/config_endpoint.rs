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
