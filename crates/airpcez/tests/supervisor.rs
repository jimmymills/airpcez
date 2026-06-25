use airpcez_core::process::*;
use airpcez::supervisor::TokioSupervisor;

#[tokio::test]
async fn runs_and_captures_output() {
    let sup = TokioSupervisor::new();
    sup.start(ProcSpec { program: "echo".into(), args: vec!["hello-airpcez".into()] }).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(matches!(sup.status(), ProcStatus::Exited(0)));
    assert!(sup.recent_logs().iter().any(|l| l.contains("hello-airpcez")));
}

#[tokio::test]
async fn stop_terminates_running_child() {
    let sup = TokioSupervisor::new();
    sup.start(ProcSpec { program: "sleep".into(), args: vec!["30".into()] }).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(matches!(sup.status(), ProcStatus::Running));
    sup.stop();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(matches!(sup.status(), ProcStatus::Stopped));
}
