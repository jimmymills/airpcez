use crate::process::ProcSpec;

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
}
