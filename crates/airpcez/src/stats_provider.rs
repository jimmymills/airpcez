use airpcez_core::model::*;
use airpcez_core::stats::StatsProvider;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct LocalStats { pub name: String, pub role: Role }

#[cfg(target_os = "macos")]
fn real_free_ram_mib(fallback_mib: u64) -> u64 {
    let pagesize: u64 = std::process::Command::new("sysctl").args(["-n", "hw.pagesize"]).output().ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(16384);
    match std::process::Command::new("vm_stat").output() {
        Ok(o) if o.status.success() =>
            crate::stats_macos::parse_vm_stat_free_mib(&String::from_utf8_lossy(&o.stdout), pagesize),
        _ => fallback_mib,
    }
}
#[cfg(not(target_os = "macos"))]
fn real_free_ram_mib(fallback_mib: u64) -> u64 { fallback_mib }

impl StatsProvider for LocalStats {
    fn sample(&self) -> NodeStats {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let ram_total_mib = sys.total_memory() / (1024 * 1024);
        let ram_free_mib = real_free_ram_mib(sys.available_memory() / (1024 * 1024));
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
    // Apple unified memory: approximate VRAM free from real-free RAM (vm_stat path).
    let free = real_free_ram_mib(0);
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
