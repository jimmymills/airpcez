use airpcez_core::model::*;

// The iPad worker's /stats payload MUST deserialize into NodeStats unchanged.
// Capture is taken verbatim from `curl http://IPAD_IP:8675/stats` (Task 5).
#[test]
fn ipad_stats_payload_deserializes_into_node_stats() {
    let json = include_str!("fixtures/ipad_stats.json");
    let stats: NodeStats = serde_json::from_str(json).expect("iPad /stats must parse as NodeStats");
    assert_eq!(stats.role, Role::Worker);
    assert_eq!(stats.binary_version.as_deref(), Some("b9789"));
    assert_eq!(stats.rpc_endpoint.as_deref(), Some("0.0.0.0:50052"));
    assert_eq!(stats.devices.len(), 1);
    let d = &stats.devices[0];
    assert_eq!(d.kind, DeviceKind::Metal);
    // Donation budget is the single source of truth: free never exceeds total.
    assert!(d.vram_free_mib <= d.vram_total_mib);
    assert!(d.reliable);
}
