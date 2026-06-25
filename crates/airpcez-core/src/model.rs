use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Worker,
    Host,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Cuda,
    Metal,
    Cpu,
    Other,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct DeviceStats {
    pub name: String,
    pub kind: DeviceKind,
    pub vram_total_mib: u64,
    pub vram_free_mib: u64,
    pub reliable: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct NodeStats {
    pub name: String,
    pub role: Role,
    pub ram_total_mib: u64,
    pub ram_free_mib: u64,
    pub cpu_logical: u32,
    pub devices: Vec<DeviceStats>,
    pub rpc_endpoint: Option<String>,
    pub binary_version: Option<String>,
    pub running: bool,
    pub sampled_at_unix: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_stats_json_roundtrips() {
        let s = NodeStats {
            name: "mac-host".into(),
            role: Role::Worker,
            ram_total_mib: 16384,
            ram_free_mib: 10240,
            cpu_logical: 12,
            devices: vec![DeviceStats {
                name: "MTL0".into(),
                kind: DeviceKind::Metal,
                vram_total_mib: 12288,
                vram_free_mib: 11000,
                reliable: true,
            }],
            rpc_endpoint: Some("192.168.0.125:50052".into()),
            binary_version: Some("b9789".into()),
            running: false,
            sampled_at_unix: 1782415690,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: NodeStats = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
