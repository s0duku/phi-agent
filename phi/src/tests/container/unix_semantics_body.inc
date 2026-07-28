use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::time::Duration;

use crate::container::job::JobStatus;
use crate::tests::container::support::job_exec;

const WAIT: Duration = Duration::from_secs(2);
const EXPIRATION: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
struct ShellResult {
    status: i8,
    output: String,
}

#[test]
fn shell_exit_status_matches_bash_c() {
    for command in [
        "true",
        "false",
        "exit 23",
        "true; false",
        "false || exit 7",
        "exit 201",
        "exec bash -c 'exit 12'",
    ] {
        assert_matches_bash(command);
    }
}

fn assert_matches_bash(command: &str) {
    assert_eq!(run_container(command), run_bash(command), "command: {command}");
}

fn run_bash(command: &str) -> ShellResult {
    let output = Command::new("/bin/bash")
        .args(["-c", command])
        .output()
        .expect("semantic tests require /bin/bash");
    ShellResult {
        status: status_code(&output.status),
        output: String::from_utf8_lossy(&output.stdout).into_owned(),
    }
}

fn run_container(command: &str) -> ShellResult {
    let (handle, info) = job_exec(command, WAIT, EXPIRATION).unwrap();
    assert!(handle.is_none(), "shell did not finish: {command}");
    let status = match info.status() {
        JobStatus::Exited(code) => *code,
        JobStatus::Running => panic!("shell was still running: {command}"),
        JobStatus::NoExist => panic!("container disappeared: {command}"),
    };
    ShellResult {
        status,
        output: info.outputs().replace("\r\n", "\n"),
    }
}

fn status_code(status: &std::process::ExitStatus) -> i8 {
    match status.code() {
        Some(code) => code as u8 as i8,
        None => -(status.signal().unwrap_or(1).min(i8::MAX as i32) as i8),
    }
}
