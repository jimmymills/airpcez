fn pages_named(vm_stat: &str, label: &str) -> u64 {
    vm_stat.lines()
        .find(|l| l.trim_start().starts_with(label))
        .and_then(|l| l.rsplit([' ', ':'])
            .map(|t| t.trim().trim_end_matches('.'))
            .find(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Reclaimable RAM = free + inactive + speculative pages. Excludes wired
/// (e.g. Metal GPU buffers) so we report what a launch can actually use.
pub fn parse_vm_stat_free_mib(vm_stat: &str, page_size_bytes: u64) -> u64 {
    let pages = pages_named(vm_stat, "Pages free")
        + pages_named(vm_stat, "Pages inactive")
        + pages_named(vm_stat, "Pages speculative");
    pages * page_size_bytes / (1024 * 1024)
}

pub fn parse_metal_vram_mib(system_profiler: &str) -> Option<u64> {
    // Look for a "VRAM (Total): N GB" or "VRAM (Dynamic, Max): N MB" line.
    for line in system_profiler.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("VRAM").and_then(|r| r.split_once(':').map(|x| x.1)) {
            let rest = rest.trim();
            let (num, unit) = rest.split_once(' ')?;
            let n: f64 = num.trim().parse().ok()?;
            let mib = match unit.trim() {
                "MB" => n,
                "GB" => n * 1024.0,
                _ => return None,
            };
            return Some(mib as u64);
        }
    }
    None
}

/// Apple Silicon has unified memory and no `system_profiler` VRAM line, so the
/// Metal working set must be estimated: the `iogpu.wired_limit_mb` sysctl when the
/// user has raised it, otherwise ~75% of physical RAM (the recommended-max default).
pub fn apple_silicon_working_set_mib(ram_total_mib: u64, iogpu_wired_limit_mb: u64) -> u64 {
    if iogpu_wired_limit_mb > 0 { iogpu_wired_limit_mb } else { ram_total_mib * 3 / 4 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn apple_silicon_working_set_defaults_to_three_quarters() {
        assert_eq!(apple_silicon_working_set_mib(16384, 0), 12288); // 75% of 16 GiB
        assert_eq!(apple_silicon_working_set_mib(16384, 14000), 14000); // raised wired limit wins
        assert_eq!(apple_silicon_working_set_mib(65536, 0), 49152); // 75% of 64 GiB
    }

    #[test]
    fn computes_real_free_mib_from_vm_stat() {
        let txt = include_str!("../tests/fixtures/vm_stat.txt");
        // free(50000) + inactive(120000) + speculative(10000) = 180000 pages
        // * 16384 bytes / 1MiB = 2812.5 MiB -> 2812
        let mib = parse_vm_stat_free_mib(txt, 16384);
        assert_eq!(mib, 2812);
    }

    #[test]
    fn parses_metal_vram_units() {
        assert_eq!(parse_metal_vram_mib("      VRAM (Total): 4 GB"), Some(4096));
        assert_eq!(parse_metal_vram_mib("      VRAM (Dynamic, Max): 1536 MB"), Some(1536));
        assert_eq!(parse_metal_vram_mib("      VRAM (Total): 4 TB"), None);
        assert_eq!(parse_metal_vram_mib("no vram line here"), None);
    }
}
