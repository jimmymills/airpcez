use crate::process::ProcSpec;

pub enum ModelRef {
    Hf(String),
    Local(String),
}

pub enum CpuMoe {
    Off,
    All,
    NLayers(u32),
}

pub struct LlamaServerOpts<'a> {
    pub binary: &'a str,
    pub model: &'a ModelRef,
    pub rpc_endpoints: &'a [String],
    pub ngl: Option<u32>,
    pub tensor_split: Option<&'a str>,
    pub main_gpu: Option<u32>,
    pub device: Option<&'a str>,
    pub cpu_moe: &'a CpuMoe,
    pub ctx: Option<u32>,
    pub host: &'a str,
    pub port: u16,
}

pub fn llama_server_spec(opts: &LlamaServerOpts) -> ProcSpec {
    let mut args: Vec<String> = Vec::new();
    match opts.model {
        ModelRef::Hf(v) => {
            args.push("-hf".into());
            args.push(v.clone());
        }
        ModelRef::Local(p) => {
            args.push("-m".into());
            args.push(p.clone());
        }
    }
    if !opts.rpc_endpoints.is_empty() {
        args.push("--rpc".into());
        args.push(opts.rpc_endpoints.join(","));
    }
    if let Some(n) = opts.ngl {
        args.push("-ngl".into());
        args.push(n.to_string());
    }
    if let Some(ts) = opts.tensor_split {
        args.push("--tensor-split".into());
        args.push(ts.into());
    }
    if let Some(mg) = opts.main_gpu {
        args.push("--main-gpu".into());
        args.push(mg.to_string());
    }
    if let Some(d) = opts.device {
        args.push("--device".into());
        args.push(d.into());
    }
    match opts.cpu_moe {
        CpuMoe::Off => {}
        CpuMoe::All => args.push("--cpu-moe".into()),
        CpuMoe::NLayers(n) => {
            args.push("--n-cpu-moe".into());
            args.push(n.to_string());
        }
    }
    if let Some(c) = opts.ctx {
        args.push("-c".into());
        args.push(c.to_string());
    }
    args.push("--host".into());
    args.push(opts.host.into());
    args.push("--port".into());
    args.push(opts.port.to_string());
    ProcSpec {
        program: opts.binary.into(),
        args,
    }
}

pub fn rpc_server_spec(binary: &str, host: &str, port: u16, device: Option<&str>) -> ProcSpec {
    let mut args = vec!["-H".into(), host.into(), "-p".into(), port.to_string()];
    if let Some(d) = device {
        args.push("-d".into());
        args.push(d.into());
    }
    ProcSpec { program: binary.into(), args }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_rpc_server_argv_with_device_pin() {
        let spec = rpc_server_spec("/opt/llama/rpc-server", "0.0.0.0", 50052, Some("MTL0"));
        assert_eq!(spec.program, "/opt/llama/rpc-server");
        assert_eq!(spec.args, vec!["-H", "0.0.0.0", "-p", "50052", "-d", "MTL0"]);
    }

    #[test]
    fn omits_device_flag_when_none() {
        let spec = rpc_server_spec("rpc-server", "0.0.0.0", 50052, None);
        assert_eq!(spec.args, vec!["-H", "0.0.0.0", "-p", "50052"]);
    }

    #[test]
    fn builds_moe_solo_argv() {
        let model = ModelRef::Hf("unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M".into());
        let opts = LlamaServerOpts {
            binary: "/llama/llama-server",
            model: &model,
            rpc_endpoints: &[],
            ngl: Some(99),
            tensor_split: None,
            main_gpu: None,
            device: None,
            cpu_moe: &CpuMoe::All,
            ctx: Some(8192),
            host: "0.0.0.0",
            port: 8080,
        };
        let spec = llama_server_spec(&opts);
        assert_eq!(spec.program, "/llama/llama-server");
        assert_eq!(
            spec.args,
            vec![
                "-hf",
                "unsloth/Qwen3.6-35B-A3B-GGUF:Q4_K_M",
                "-ngl",
                "99",
                "--cpu-moe",
                "-c",
                "8192",
                "--host",
                "0.0.0.0",
                "--port",
                "8080",
            ]
        );
    }

    #[test]
    fn builds_cluster_dense_argv() {
        let model = ModelRef::Local("/mnt/ssd/m.gguf".into());
        let eps = vec![
            "192.168.0.125:50052".to_string(),
            "192.168.0.83:50052".to_string(),
        ];
        let opts = LlamaServerOpts {
            binary: "llama-server",
            model: &model,
            rpc_endpoints: &eps,
            ngl: Some(40),
            tensor_split: Some("0,12,11"),
            main_gpu: Some(1),
            device: Some("RPC0,RPC1"),
            cpu_moe: &CpuMoe::Off,
            ctx: Some(4096),
            host: "0.0.0.0",
            port: 8080,
        };
        let spec = llama_server_spec(&opts);
        assert_eq!(
            spec.args,
            vec![
                "-m",
                "/mnt/ssd/m.gguf",
                "--rpc",
                "192.168.0.125:50052,192.168.0.83:50052",
                "-ngl",
                "40",
                "--tensor-split",
                "0,12,11",
                "--main-gpu",
                "1",
                "--device",
                "RPC0,RPC1",
                "-c",
                "4096",
                "--host",
                "0.0.0.0",
                "--port",
                "8080",
            ]
        );
    }

    #[test]
    fn builds_n_cpu_moe_argv() {
        let model = ModelRef::Hf("repo:Q4_K_M".into());
        let opts = LlamaServerOpts {
            binary: "llama-server",
            model: &model,
            rpc_endpoints: &[],
            ngl: Some(99),
            tensor_split: None,
            main_gpu: None,
            device: None,
            cpu_moe: &CpuMoe::NLayers(32),
            ctx: None,
            host: "127.0.0.1",
            port: 8080,
        };
        let spec = llama_server_spec(&opts);
        assert_eq!(
            spec.args,
            vec![
                "-hf",
                "repo:Q4_K_M",
                "-ngl",
                "99",
                "--n-cpu-moe",
                "32",
                "--host",
                "127.0.0.1",
                "--port",
                "8080",
            ]
        );
    }
}
