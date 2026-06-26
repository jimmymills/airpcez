use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use std::sync::Arc;

// With no llama-server listening on the configured llama_port, /host/health must
// report not-ready (rather than erroring) so the cockpit shows "loading…".
#[tokio::test]
async fn host_health_reports_not_ready_without_llama_server() {
    let stats = NodeStats {
        name: "host".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    state.config.lock().unwrap().llama_port = 1; // nothing is listening here → connection refused
    tokio::spawn(airpcez::server::run_server(19305, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let v: serde_json::Value = reqwest::Client::new()
        .get("http://127.0.0.1:19305/host/health")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(v["ready"], false);
    assert!(v["detail"].as_str().unwrap().contains("starting"));
}
