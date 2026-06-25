use crate::model::*;

pub trait StatsProvider: Send + Sync {
    fn sample(&self) -> NodeStats;
}

pub struct MockStatsProvider {
    pub stats: NodeStats,
}

impl StatsProvider for MockStatsProvider {
    fn sample(&self) -> NodeStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mock_provider_returns_its_stats() {
        let stats = NodeStats {
            name: "n".into(), role: Role::Worker, ram_total_mib: 8, ram_free_mib: 4,
            cpu_logical: 4, devices: vec![], rpc_endpoint: None, binary_version: None,
            running: false, sampled_at_unix: 0,
        };
        let p = MockStatsProvider { stats: stats.clone() };
        assert_eq!(p.sample(), stats);
    }
}
