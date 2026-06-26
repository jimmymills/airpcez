use airpcez_core::model::Role;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub ui_port: u16,
    pub rpc_port: u16,
    pub llama_port: u16,
    pub role: Role,
    pub llama_dir: Option<String>,
    pub hf_cache_dir: Option<String>,
    pub rpc_binary: Option<String>,
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

    pub fn load(path: &Path) -> Config {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, content).map_err(|e| e.to_string())
    }
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
}
