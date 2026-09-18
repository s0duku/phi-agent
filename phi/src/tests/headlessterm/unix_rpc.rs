#![cfg(unix)]

use std::thread;
use std::time::{Duration, Instant};

use crate::headlessterm::job::{JobAccess, JobHandle, JobStatus, ReturnWhen};
use crate::headlessterm::worker::protocol::{Request, Response, Status};
use crate::headlessterm::worker::rpc;
use crate::tests::headlessterm::support::{connect, exec_job, job_interact};

const EXPIRATION: Duration = Duration::from_secs(5);

#[test]
fn repeated_serial_interactions_keep_one_container_consistent() {
    let (handle, info) = exec_job("cat", Duration::ZERO, EXPIRATION).unwrap();
    assert!(matches!(info.status(), JobStatus::RunningWaitElapsed));
    let handle = handle.unwrap();
    assert!(info.outputs().is_empty());

    for index in 0..64 {
        let expected = format!("serial-{index}");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut input = format!("{expected}\n");
        let mut output = String::new();

        loop {
            let interaction = job_interact(
                JobHandle(handle.0.clone()),
                &input,
                Duration::from_millis(2),
            )
            .unwrap();
            assert!(interaction.is_running());
            output.push_str(interaction.outputs());
            if output.contains(&expected) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {expected:?}; received {output:?}"
            );
            input.clear();
        }
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
    let (_, info) = exec_job(
        "for i in $(seq 1 100); do printf 'line-%s\\n' \"$i\"; done",
        Duration::from_secs(2),
        EXPIRATION,
    )
    .unwrap();

    assert!(matches!(info.status(), JobStatus::Exited(0)));
    assert!(info.outputs().contains("line-1\n"));
    assert!(info.outputs().contains("line-100"));
    assert_eq!(info.outputs().lines().count(), 100);
    assert!(!info.truncated());
}

#[test]
fn maximum_wait_value_still_returns_when_the_shell_exits() {
    let (handle, info) = exec_job("sleep 2; exit 17", Duration::ZERO, EXPIRATION).unwrap();
    assert!(matches!(info.status(), JobStatus::RunningWaitElapsed));
    let handle = handle.unwrap();

    let request = Request::Access(JobAccess::Interact {
        data: String::new(),
        return_when: ReturnWhen::output_settled(Duration::MAX),
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

#[test]
fn settled_output_keeps_worker_alive_until_late_exit_output_is_read() {
    let (handle, initial) = exec_job(
        "printf 'first\\n'; sleep 3.2; printf 'second\\n'; exit 17",
        Duration::from_secs(5),
        EXPIRATION,
    )
    .unwrap();
    assert!(matches!(initial.status(), JobStatus::RunningOutputSettled));
    assert!(initial.outputs().contains("first"));

    // Let the process exit after the settled snapshot was delivered. The
    // worker must retain the handle so the next interaction can drain the
    // bytes written just before exit and report the final status.
    thread::sleep(Duration::from_millis(500));
    let final_info = job_interact(handle.unwrap(), "", Duration::from_secs(2)).unwrap();
    assert!(matches!(final_info.status(), JobStatus::Exited(17)));
    assert!(final_info.outputs().contains("second"));
}

#[test]
fn continuously_changing_output_reports_wait_elapsed() {
    let (handle, initial) = exec_job(
        "i=0; while :; do printf 'tick-%s\\n' \"$i\"; i=$((i + 1)); sleep 0.02; done",
        Duration::from_millis(250),
        EXPIRATION,
    )
    .unwrap();

    assert!(matches!(initial.status(), JobStatus::RunningWaitElapsed));
    assert!(!initial.outputs().is_empty());

    let closed = crate::tests::headlessterm::support::block_on(
        crate::headlessterm::worker::client::close_job(handle.unwrap()),
    )
    .unwrap();
    assert!(matches!(closed.status(), JobStatus::Closed(_)));
}
