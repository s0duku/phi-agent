#![cfg(unix)]

use std::time::Duration;

use crate::container::job::{JobAccess, JobHandle, JobStatus};
use crate::container::local::rpc::{self, Response, Status};
use crate::tests::container::support::{connect, job_exec, job_interact};

const EXPIRATION: Duration = Duration::from_secs(5);

#[test]
fn repeated_serial_interactions_keep_one_container_consistent() {
    let (handle, info) = job_exec("cat", Duration::ZERO, EXPIRATION).unwrap();
    assert!(matches!(info.status(), JobStatus::Running));
    let handle = handle.unwrap();
    assert!(info.outputs().is_empty());

    for index in 0..64 {
        let interaction = job_interact(
            JobHandle(handle.0.clone()),
            &format!("serial-{index}\n"),
            Duration::from_millis(2),
        )
        .unwrap();
        assert!(matches!(interaction.status(), JobStatus::Running));
        assert!(interaction.outputs().contains(&format!("serial-{index}")));
    }

    let final_info =
        job_interact(JobHandle(handle.0.clone()), "\u{4}", Duration::from_secs(2)).unwrap();
    assert!(matches!(final_info.status(), JobStatus::Exited(0)));
    assert!(
        !final_info.outputs().contains("serial-"),
        "EOF response repeated consumed output: {:?}",
        final_info.outputs()
    );

    let missing = job_interact(handle, "", Duration::ZERO).unwrap();
    assert!(matches!(missing.status(), JobStatus::NoExist));
}

#[test]
fn initial_response_preserves_output_beyond_the_visible_terminal_page() {
    let (_, info) = job_exec(
        "for i in $(seq 1 100); do printf 'line-%s\\n' \"$i\"; done",
        Duration::from_secs(2),
        EXPIRATION,
    )
    .unwrap();

    assert!(matches!(info.status(), JobStatus::Exited(0)));
    assert!(info.outputs().contains("line-1\n"));
    assert!(info.outputs().contains("line-100"));
    assert_eq!(info.outputs().lines().count(), 100);
    assert!(!info.terminal().truncated());
}

#[test]
fn maximum_wait_value_still_returns_when_the_shell_exits() {
    let (handle, info) = job_exec("sleep 2; exit 17", Duration::ZERO, EXPIRATION).unwrap();
    assert!(matches!(info.status(), JobStatus::Running));
    let handle = handle.unwrap();

    let request = rpc::Request::Access(JobAccess::Interact {
        data: String::new(),
        wait: Duration::MAX,
    });
    let mut stream = connect(&handle.0).unwrap();
    rpc::write_frame(&mut stream, &request).unwrap();
    let response: Response = rpc::read_frame(&mut stream).unwrap();

    let Response::Terminal {
        status: Status::Exited(17),
        waited_ms,
        ..
    } = response
    else {
        panic!("interact did not return the expected exit status");
    };
    assert!(waited_ms >= 1_500);
}
