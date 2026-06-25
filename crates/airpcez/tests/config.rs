use airpcez::config::Config;

#[test]
fn defaults_when_missing_then_roundtrips() {
    let dir = std::env::temp_dir().join(format!("airpcez-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let c = Config::load(&path);
    assert_eq!(c.ui_port, 8675);
    assert_eq!(c.rpc_port, 50052);
    assert_eq!(c.llama_port, 8080);
    c.save(&path).unwrap();
    let c2 = Config::load(&path);
    assert_eq!(c.ui_port, c2.ui_port);
}
