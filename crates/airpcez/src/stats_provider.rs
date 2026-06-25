use airpcez_core::model::*;
use airpcez_core::stats::StatsProvider;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct LocalStats { pub name: String, pub role: Role }

impl StatsProvider for LocalStats {
    fn sample(&self) -> NodeStats {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let ram_total_mib = sys.total_memory() / (1024 * 1024);
        let ram_free_mib = sys.available_memory() / (1024 * 1024);
        let cpu_logical = num_cpus_logical();
        let devices = gather_devices();
        NodeStats {
            name: self.name.clone(),
            role: self.role,
            ram_total_mib,
            ram_free_mib,
            cpu_logical,
            devices,
            rpc_endpoint: None,
            binary_version: None,
            running: false,
            sampled_at_unix: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        }
    }
}

fn num_cpus_logical() -> u32 {
    std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1)
}

#[cfg(target_os = "linux")]
fn gather_devices() -> Vec<DeviceStats> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total,memory.free", "--format=csv,noheader,nounits"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            crate::stats_nvidia::parse_nvidia_smi(&String::from_utf8_lossy(&o.stdout))
        }
        _ => vec![],
    }
}

#[cfg(target_os = "macos")]
fn gather_devices() -> Vec<DeviceStats> {
    let sp = Command::new("system_profiler").arg("SPDisplaysDataType").output();
    let total = sp.ok()
        .filter(|o| o.status.success())
        .and_then(|o| crate::stats_macos::parse_metal_vram_mib(&String::from_utf8_lossy(&o.stdout)));
    // Apple unified memory: approximate VRAM free from system RAM real-free.
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let free = sys.available_memory() / (1024 * 1024);
    match total {
        Some(t) => vec![DeviceStats {
            name: "MTL0".into(), kind: DeviceKind::Metal,
            vram_total_mib: t, vram_free_mib: free.min(t),
            reliable: vram_reliable(t, free.min(t)),
        }],
        None => vec![],
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn gather_devices() -> Vec<DeviceStats> { vec![] }
