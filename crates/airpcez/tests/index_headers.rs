use airpcez_core::model::*;
use airpcez_core::stats::MockStatsProvider;
use std::sync::Arc;

// The cockpit is a single-page app; without a Cache-Control header an open tab serves
// stale JS after a deploy. Assert the index is served with `no-cache` so a normal reload
// always revalidates and picks up fresh assets.
#[tokio::test]
async fn index_sets_no_cache_header() {
    let stats = NodeStats {
        name: "h".into(), role: Role::Host, ram_total_mib: 1, ram_free_mib: 1,
        cpu_logical: 1, devices: vec![], rpc_endpoint: None, binary_version: None,
        running: false, sampled_at_unix: 0,
    };
    let state = airpcez::server::AppState::for_test(Arc::new(MockStatsProvider { stats }));
    tokio::spawn(airpcez::server::run_server(19401, state));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let resp = reqwest::Client::new().get("http://127.0.0.1:19401/").send().await.unwrap();
    assert_eq!(
        resp.headers().get("cache-control").and_then(|v| v.to_str().ok()),
        Some("no-cache")
    );
}
