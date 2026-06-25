use airpcez::server::AppState;
use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio_tungstenite::connect_async;

#[tokio::test]
async fn ws_pushes_stats_frames() {
    let stats = NodeStats { name: "ws".into(), role: Role::Worker, ram_total_mib: 1,
        ram_free_mib: 1, cpu_logical: 1, devices: vec![], rpc_endpoint: None,
        binary_version: None, running: false, sampled_at_unix: 0 };
    let state = AppState::for_test(Arc::new(MockStatsProvider { stats: stats.clone() }));
    tokio::spawn(airpcez::server::run_server(18676, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (mut ws, _) = connect_async("ws://127.0.0.1:18676/ws").await.unwrap();
    let msg = ws.next().await.unwrap().unwrap();
    let got: NodeStats = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(got.name, "ws");
}
