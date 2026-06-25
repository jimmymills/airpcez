/// Parse `llama-server --version` output → "b<N>" (e.g. "b9789").
pub fn parse_llama_version(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let l = line.trim();
        for prefix in ["version:", "build:"] {
            if let Some(rest) = l.strip_prefix(prefix) {
                let num: String = rest.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
                if !num.is_empty() { return Some(format!("b{num}")); }
            }
        }
    }
    None
}

pub fn detect_binary_version(llama_dir: Option<&str>) -> Option<String> {
    let dir = llama_dir?;
    let bin = std::path::Path::new(dir).join("llama-server");
    let out = std::process::Command::new(bin).arg("--version").output().ok()?;
    // llama.cpp prints --version to stderr on some builds; check both.
    let text = if !out.stdout.is_empty() { String::from_utf8_lossy(&out.stdout).into_owned() }
               else { String::from_utf8_lossy(&out.stderr).into_owned() };
    parse_llama_version(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_build_number() {
        assert_eq!(parse_llama_version("version: 9789 (abc1234)\nbuilt with ..."), Some("b9789".into()));
        assert_eq!(parse_llama_version("build: 9789 (abc)"), Some("b9789".into()));
        assert_eq!(parse_llama_version("garbage"), None);
    }
}
