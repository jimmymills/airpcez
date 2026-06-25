pub mod server;
pub mod stats_provider;
pub mod stats_nvidia;

#[cfg(target_os = "macos")]
pub mod stats_macos;
