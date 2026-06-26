use airpcez_core::model::Role;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)] // any omitted field falls back to Config::default() — partial tomls are fine
pub struct Config {
    pub ui_port: u16,
    pub rpc_port: u16,
    pub llama_port: u16,
    pub role: Role,
    pub llama_dir: Option<String>,
    pub hf_cache_dir: Option<String>,
    pub rpc_binary: Option<String>,
    pub rpc_device: Option<String>,
    pub node_name: String,
    #[serde(default)]
    pub nodes: Vec<airpcez_core::cluster::NodeEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ui_port: 8675,
            rpc_port: 50052,
            llama_port: 8080,
            role: Role::Worker,
            llama_dir: None,
            hf_cache_dir: None,
            rpc_binary: None,
            rpc_device: None,
            node_name: sysinfo::System::host_name()
                .unwrap_or_else(|| "airpcez-node".to_string()),
            nodes: Vec::new(),
        }
    }
}

impl Config {
    /// Resolve the rpc-server binary for `--worker` autostart: explicit `rpc_binary`,
    /// else `<llama_dir>/rpc-server`, else bare `rpc-server` (PATH lookup).
    pub fn rpc_binary_path(&self) -> String {
        self.rpc_binary
            .clone()
            .or_else(|| self.llama_dir.as_ref().map(|d| format!("{}/rpc-server", d.trim_end_matches('/'))))
            .unwrap_or_else(|| "rpc-server".to_string())
    }

    /// The rpc-server `-d` device filter for this worker: an explicit `rpc_device`,
    /// else — ONLY on Apple Silicon (macos+aarch64), where RAM and VRAM are unified —
    /// the GPU device `MTL0`, so rpc-server doesn't also serve the redundant 0-MiB
    /// BLAS/Accelerate device. On every other platform, None (serve all devices).
    pub fn rpc_device_filter(&self) -> Option<String> {
        if let Some(d) = &self.rpc_device {
            return Some(d.clone());
        }
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some("MTL0".to_string())
        } else {
            None
        }
    }

    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            // A malformed file used to silently fall back to defaults; warn loudly instead.
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!(
                    "[airpcez] WARNING: {} failed to parse ({e}) — using defaults; your settings are NOT applied",
                    path.display()
                );
                Config::default()
            }),
            Err(_) => Config::default(), // no config file present → compiled-in defaults
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }
}

/// Apply `--<flag> <value>` pairs from `args` onto `config`, returning the mutated config.
/// Unknown flags and flags whose value is missing or unparseable are silently ignored.
pub fn apply_cli_overrides(mut config: Config, args: &[String]) -> Config {
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        // Grab the value token that follows, if present.
        let value = if i + 1 < args.len() { Some(args[i + 1].as_str()) } else { None };
        match flag.as_str() {
            "--ui-port" => {
                if let Some(v) = value {
                    if let Ok(p) = v.parse::<u16>() {
                        config.ui_port = p;
                        i += 1;
                    }
                }
            }
            "--rpc-port" => {
                if let Some(v) = value {
                    if let Ok(p) = v.parse::<u16>() {
                        config.rpc_port = p;
                        i += 1;
                    }
                }
            }
            "--llama-port" => {
                if let Some(v) = value {
                    if let Ok(p) = v.parse::<u16>() {
                        config.llama_port = p;
                        i += 1;
                    }
                }
            }
            "--role" => {
                if let Some(v) = value {
                    config.role = if v == "host" { Role::Host } else { Role::Worker };
                    i += 1;
                }
            }
            "--llama-dir" => {
                if let Some(v) = value {
                    config.llama_dir = Some(v.to_string());
                    i += 1;
                }
            }
            "--hf-cache-dir" => {
                if let Some(v) = value {
                    config.hf_cache_dir = Some(v.to_string());
                    i += 1;
                }
            }
            "--rpc-binary" => {
                if let Some(v) = value {
                    config.rpc_binary = Some(v.to_string());
                    i += 1;
                }
            }
            "--rpc-device" => {
                if let Some(v) = value {
                    config.rpc_device = Some(v.to_string());
                    i += 1;
                }
            }
            "--node-name" => {
                if let Some(v) = value {
                    config.node_name = v.to_string();
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_binary_path_resolution() {
        let mut c = Config::default();
        c.rpc_binary = None;
        c.llama_dir = None;
        assert_eq!(c.rpc_binary_path(), "rpc-server");
        c.llama_dir = Some("/llama/build/bin/".to_string()); // trailing slash trimmed
        assert_eq!(c.rpc_binary_path(), "/llama/build/bin/rpc-server");
        c.rpc_binary = Some("/custom/rpc-server".to_string());
        assert_eq!(c.rpc_binary_path(), "/custom/rpc-server"); // explicit wins over llama_dir
    }

    #[test]
    fn partial_toml_fills_missing_fields_from_defaults() {
        // A worker only needs to set rpc_binary — everything else should default, not fail.
        let c: Config = toml::from_str("rpc_binary = \"/x/rpc-server\"\nnode_name = \"m2-pro\"").unwrap();
        assert_eq!(c.rpc_binary.as_deref(), Some("/x/rpc-server"));
        assert_eq!(c.node_name, "m2-pro");
        assert_eq!(c.ui_port, 8675); // defaulted
        assert_eq!(c.rpc_port, 50052); // defaulted
        assert!(matches!(c.role, Role::Worker)); // defaulted
    }

    #[test]
    fn apply_cli_overrides_sets_known_fields() {
        let args: Vec<String> = ["--role", "host", "--ui-port", "9000", "--llama-dir", "/x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = apply_cli_overrides(Config::default(), &args);
        assert_eq!(c.role, Role::Host);
        assert_eq!(c.ui_port, 9000);
        assert_eq!(c.llama_dir, Some("/x".into()));
    }

    #[test]
    fn apply_cli_overrides_invalid_port_leaves_default() {
        let args: Vec<String> = ["--ui-port", "abc"].iter().map(|s| s.to_string()).collect();
        let c = apply_cli_overrides(Config::default(), &args);
        assert_eq!(c.ui_port, 8675); // default unchanged
    }

    #[test]
    fn apply_cli_overrides_trailing_flag_no_panic() {
        // --node-name at the end with no following value must not panic.
        let args: Vec<String> = ["--node-name"].iter().map(|s| s.to_string()).collect();
        let c = apply_cli_overrides(Config::default(), &args);
        // node_name stays as whatever the default hostname is — just assert no panic
        let _ = c.node_name;
    }

    #[test]
    fn rpc_device_filter_explicit_override_wins() {
        let mut c = Config::default();
        c.rpc_device = Some("CUDA0".into());
        assert_eq!(c.rpc_device_filter(), Some("CUDA0".to_string()));
    }

    #[test]
    fn rpc_device_filter_default_platform_behavior() {
        assert_eq!(
            Config::default().rpc_device_filter(),
            if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                Some("MTL0".to_string())
            } else {
                None
            }
        );
    }

    #[test]
    fn apply_cli_overrides_rpc_device() {
        let args: Vec<String> = ["--rpc-device", "CUDA0"].iter().map(|s| s.to_string()).collect();
        let c = apply_cli_overrides(Config::default(), &args);
        assert_eq!(c.rpc_device, Some("CUDA0".into()));
    }
}
