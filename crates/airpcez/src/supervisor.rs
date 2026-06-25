use airpcez_core::process::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

struct Inner {
    status: ProcStatus,
    logs: Vec<String>,
    kill_tx: Option<oneshot::Sender<()>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            status: ProcStatus::Stopped,
            logs: Vec::new(),
            kill_tx: None,
        }
    }
}

pub struct TokioSupervisor {
    inner: Arc<Mutex<Inner>>,
}

impl TokioSupervisor {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }
}

impl Default for TokioSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessBackend for TokioSupervisor {
    fn start(&self, spec: ProcSpec) -> Result<(), String> {
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let (kill_tx, kill_rx) = oneshot::channel::<()>();

        {
            let mut g = self.inner.lock().unwrap();
            g.status = ProcStatus::Running;
            g.kill_tx = Some(kill_tx);
        }

        // Spawn stdout reader task
        let li = self.inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut g = li.lock().unwrap();
                g.logs.push(l);
                if g.logs.len() > 500 {
                    g.logs.remove(0);
                }
            }
        });

        // Spawn stderr reader task
        let le = self.inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut g = le.lock().unwrap();
                g.logs.push(l);
                if g.logs.len() > 500 {
                    g.logs.remove(0);
                }
            }
        });

        // Spawn monitor task: select between child exit and kill signal
        let lw = self.inner.clone();
        tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    let mut g = lw.lock().unwrap();
                    // Clear kill_tx since process already exited
                    g.kill_tx = None;
                    g.status = match status {
                        Ok(s) if s.success() => ProcStatus::Exited(0),
                        Ok(s) => ProcStatus::Exited(s.code().unwrap_or(-1)),
                        Err(e) => ProcStatus::Crashed(e.to_string()),
                    };
                }
                _ = kill_rx => {
                    // Kill the child and set status to Stopped
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let mut g = lw.lock().unwrap();
                    g.kill_tx = None;
                    g.status = ProcStatus::Stopped;
                }
            }
        });

        Ok(())
    }

    fn stop(&self) {
        let kill_tx = {
            let mut g = self.inner.lock().unwrap();
            g.kill_tx.take()
        };
        if let Some(tx) = kill_tx {
            let _ = tx.send(());
        }
    }

    fn status(&self) -> ProcStatus {
        self.inner.lock().unwrap().status.clone()
    }

    fn recent_logs(&self) -> Vec<String> {
        self.inner.lock().unwrap().logs.clone()
    }
}
