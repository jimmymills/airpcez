use airpcez_core::planner::{Plan, ModelMeta};
use airpcez_core::stats::MockStatsProvider;
use airpcez_core::model::*;
use std::sync::Arc;

#[tokio::test]
async fn suggest_returns_a_plan_for_self() {
    // Host self has one reliable 12 GB GPU; ask for a small model.
    let stats = NodeStats {
        name: "host".into(), role: Role::Host, ram_total_mib: 16000, ram_free_mib: 9000,
        cpu_logical: 8,
        devices: vec![DeviceStats { name: "MTL0".into(), kind: DeviceKind::Metal,
            vram_total_mib: 12000, vram_free_mib: 11000, reliable: true }],
        rpc_endpoint: None, binary_version: None, running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19201, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let plan: Plan = reqwest::Client::new().post("http://127.0.0.1:19201/suggest")
        .json(&serde_json::json!({ "meta": ModelMeta { total_mib: 6000, n_layers: 32, is_moe: false }, "ctx": 4096 }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(plan.ngl, 32);
    assert!(plan.gpu_pool_mib > 0);
}
