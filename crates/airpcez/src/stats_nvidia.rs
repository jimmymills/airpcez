use airpcez_core::model::{DeviceKind, DeviceStats, vram_reliable};

pub fn parse_nvidia_smi(csv: &str) -> Vec<DeviceStats> {
    csv.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let cols: Vec<&str> = line.split(',').map(str::trim).collect();
            if cols.len() < 3 { return None; }
            let total: u64 = cols[1].parse().ok()?;
            let free: u64 = cols[2].parse().ok()?;
            Some(DeviceStats {
                name: cols[0].to_string(),
                kind: DeviceKind::Cuda,
                vram_total_mib: total,
                vram_free_mib: free,
                reliable: vram_reliable(total, free),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_one_gpu() {
        let csv = include_str!("../tests/fixtures/nvidia-smi.csv");
        let devs = parse_nvidia_smi(csv);
        assert_eq!(devs.len(), 1);
        let d = &devs[0];
        assert_eq!(d.name, "NVIDIA GeForce RTX 2080 SUPER");
        assert_eq!(d.kind, DeviceKind::Cuda);
        assert_eq!(d.vram_total_mib, 8192);
        assert_eq!(d.vram_free_mib, 7700);
        assert!(d.reliable);
    }
}
