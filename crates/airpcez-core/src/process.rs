#[derive(Clone, Debug, PartialEq)]
pub struct ProcSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ProcStatus {
    #[default]
    Stopped,
    Running,
    Exited(i32),
    Crashed(String),
}

pub trait ProcessBackend: Send + Sync {
    fn start(&self, spec: ProcSpec) -> Result<(), String>;
    fn stop(&self) -> bool;
    fn status(&self) -> ProcStatus;
    fn recent_logs(&self) -> Vec<String>;
}
