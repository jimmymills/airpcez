use airpcez::server::AppState;
use airpcez::supervisor::TokioSupervisor;
use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::process::{ProcessBackend, ProcStatus};
use std::sync::Arc;

#[tokio::test]
async fn worker_start_and_stop_endpoints() {
    let stats = NodeStats {
        name: "worker-test".into(), role: Role::Worker, ram_total_mib: 8, ram_free_mib: 4,
        cpu_logical: 2, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let supervisor = Arc::new(TokioSupervisor::new());
    let state = AppState {
        provider: Arc::new(MockStatsProvider { stats }),
        supervisor: supervisor.clone(),
    };
    tokio::spawn(airpcez::server::run_server(18677, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();

    // POST /worker/start with echo (exits 0 immediately)
    let resp = client
        .post("http://127.0.0.1:18677/worker/start")
        .json(&serde_json::json!({
            "binary": "echo",
            "rpc_port": 50052,
            "device": "MTL0"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Give echo time to start and exit
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let status = supervisor.status();
    assert!(
        matches!(status, ProcStatus::Running | ProcStatus::Exited(_)),
        "unexpected status: {:?}", status
    );

    // POST /worker/stop
    let resp = client
        .post("http://127.0.0.1:18677/worker/stop")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
