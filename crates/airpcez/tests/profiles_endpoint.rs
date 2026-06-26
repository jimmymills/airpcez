use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use std::sync::Arc;

fn stats() -> NodeStats {
    NodeStats {
        name: "h".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    }
}

#[tokio::test]
async fn profiles_crud_roundtrip() {
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats: stats() }));
    let pp = state.profiles_path();
    let _ = std::fs::remove_file(&pp); // clean slate
    tokio::spawn(airpcez::server::run_server(19301, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let c = reqwest::Client::new();

    // POST a profile (no id -> derived from name)
    let saved: serde_json::Value = c.post("http://127.0.0.1:19301/profiles")
        .json(&serde_json::json!({ "name": "Best Networked", "model": "repo:Q4_K_M", "ngl": 99 }))
        .send().await.unwrap().json().await.unwrap();
    assert!(saved.as_array().unwrap().iter().any(|p| p["id"] == "best-networked"));

    // GET filtered by model returns it; a different model filter does not
    let got: serde_json::Value = c.get("http://127.0.0.1:19301/profiles?model=repo:Q4_K_M")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(got.as_array().unwrap().len(), 1);
    let none: serde_json::Value = c.get("http://127.0.0.1:19301/profiles?model=other")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(none.as_array().unwrap().len(), 0);

    // DELETE removes it
    let after: serde_json::Value = c.request(reqwest::Method::DELETE, "http://127.0.0.1:19301/profiles")
        .json(&serde_json::json!({ "id": "best-networked" }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(after.as_array().unwrap().len(), 0);

    let _ = std::fs::remove_file(&pp);
}

#[tokio::test]
async fn apply_reconciles_nodes_and_launch_404s_on_unknown() {
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats: stats() }));
    let pp = state.profiles_path();
    let _ = std::fs::remove_file(&pp);
    tokio::spawn(airpcez::server::run_server(19302, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let c = reqwest::Client::new();

    // Create a networked profile with one node
    c.post("http://127.0.0.1:19302/profiles")
        .json(&serde_json::json!({
            "name": "net", "model": "repo:Q4_K_M",
            "nodes": [{ "name": "m2", "addr": "192.168.0.125:8675" }]
        })).send().await.unwrap();

    // apply reconciles the host's node list
    let applied = c.post("http://127.0.0.1:19302/profiles/net/apply").send().await.unwrap();
    assert_eq!(applied.status(), 200);
    let cfg: serde_json::Value = c.get("http://127.0.0.1:19302/config").send().await.unwrap().json().await.unwrap();
    assert_eq!(cfg["nodes"][0]["addr"], "192.168.0.125:8675");

    // launch on an unknown id -> 404
    let unknown = c.post("http://127.0.0.1:19302/profiles/nope/launch").send().await.unwrap();
    assert_eq!(unknown.status(), 404);

    let _ = std::fs::remove_file(&pp);
}
