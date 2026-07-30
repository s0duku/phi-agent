use crate::tests::container::support::exec_job;

#[test]
fn short_job_returns_output_and_no_live_handle() {
    let (handle, info) = exec_job("printf completed", Duration::from_secs(2), EXPIRATION).unwrap();
    assert!(handle.is_none());
    assert!(matches!(info.status(), JobStatus::Exited(0)));
    assert_eq!(info.outputs(), "completed");
}

#[test]
fn exit_status_is_preserved() {
    let (handle, info) = exec_job("exit 23", Duration::from_secs(2), EXPIRATION).unwrap();
    assert!(handle.is_none());
    assert!(matches!(info.status(), JobStatus::Exited(23)));
}
