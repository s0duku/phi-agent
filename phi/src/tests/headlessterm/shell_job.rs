use std::time::Duration;

use crate::executor::PhiTool;
use crate::executor::tools::shell::job::{
    InteractArgs, InteractiveInput, ShellJobExecTool, ShellJobInteractTool, interactive_input,
};
use crate::headlessterm::{
    HeadlessTerminal, JobAccess, JobAccessResult, JobHandle, JobInfo, JobProcessStatus, JobStatus,
    TerminalCommand,
};

async fn access_interact(handle: JobHandle, data: &str, try_wait: Duration) -> JobInfo {
    let result = HeadlessTerminal::new()
        .access_job(
            handle,
            JobAccess::Interact {
                data: data.to_owned(),
                return_when: try_wait.into(),
            },
        )
        .await
        .unwrap();
    match result {
        JobAccessResult::Interacted(info) => info,
        JobAccessResult::Written(_) => panic!("interact returned write acknowledgment"),
    }
}

#[test]
fn shell_job_definition_is_platform_specific() {
    let tool = ShellJobExecTool;
    assert_eq!(
        tool.name(),
        if cfg!(windows) {
            "powershell_job"
        } else {
            "bash_job"
        }
    );
    assert_eq!(tool.parameters()["required"], serde_json::json!(["cmd"]));
}

#[test]
fn interact_defaults_to_read_with_a_sixty_second_wait() {
    let args: InteractArgs = serde_json::from_value(serde_json::json!({
        "handle": "mira-kest"
    }))
    .unwrap();
    assert!(args.input.is_none());
    assert_eq!(args.wait_ms, 60_000);

    let tool = ShellJobInteractTool;
    assert_eq!(
        tool.parameters()["properties"]["wait_ms"]["default"],
        serde_json::json!(60_000)
    );
    assert!(tool.parameters()["properties"].get("timeout").is_none());
    assert!(
        serde_json::from_value::<InteractArgs>(serde_json::json!({
            "handle": "mira-kest",
            "timeout": 1
        }))
        .is_err()
    );
}

#[test]
fn interact_completes_only_unterminated_nonempty_input() {
    assert_eq!(interactive_input(None), InteractiveInput::Direct("".into()));
    assert_eq!(
        interactive_input(Some(String::new())),
        InteractiveInput::Direct("\r".into())
    );
    assert_eq!(
        interactive_input(Some("hello".into())),
        InteractiveInput::Submit("hello".into())
    );
    assert_eq!(
        interactive_input(Some("hello\n".into())),
        InteractiveInput::Direct(if cfg!(windows) {
            "hello\r".into()
        } else {
            "hello\n".into()
        })
    );
    assert_eq!(
        interactive_input(Some("hello\r\n".into())),
        InteractiveInput::Direct(if cfg!(windows) {
            "hello\r".into()
        } else {
            "hello\r\n".into()
        })
    );

    let legacy = serde_json::from_value::<InteractArgs>(serde_json::json!({
        "handle": "mira-kest",
        "data": ""
    }));
    assert!(legacy.is_err(), "legacy interact field should be rejected");

    let missing_shell = serde_json::from_value::<TerminalCommand>(serde_json::json!({
        "DockerExec": {
            "container": "phi",
            "command": "echo hi"
        }
    }));
    assert!(
        missing_shell.is_err(),
        "headlessterm commands must use the complete current wire shape"
    );

    let tool = ShellJobInteractTool;
    assert!(tool.description().contains("pressing Enter once"));
    assert!(
        tool.description()
            .contains("Omit input to only read output")
    );
    assert!(
        tool.parameters()["properties"]["input"]["description"]
            .as_str()
            .unwrap()
            .contains("Provide an empty string to press Enter once")
    );
}

#[cfg(windows)]
#[test]
fn windows_interact_submits_each_multiline_input_line_with_enter() {
    assert_eq!(
        interactive_input(Some("x = 10\ny = 20\nprint(x + y)".into())),
        InteractiveInput::Submit("x = 10\ry = 20\rprint(x + y)".into())
    );
    assert_eq!(
        interactive_input(Some("first\r\nsecond\n".into())),
        InteractiveInput::Direct("first\rsecond\r".into())
    );
    assert_eq!(
        interactive_input(Some("already\rready\r".into())),
        InteractiveInput::Direct("already\rready\r".into())
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_linear_output_beyond_one_screen_remains_complete() {
    let (_, info) = HeadlessTerminal::new()
        .exec_job(
            "1..100 | ForEach-Object { Write-Output \"line-$_\" }",
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

    assert!(
        matches!(info.status(), JobStatus::Exited(0)),
        "Windows command did not exit before its output boundary: {info:#?}"
    );
    assert_eq!(
        info.outputs().lines().count(),
        100,
        "Windows output was incomplete: {info:#?}"
    );
    assert!(
        info.outputs().contains("line-1\n"),
        "Windows output lost its first line: {info:#?}"
    );
    assert!(
        info.outputs().contains("line-100"),
        "Windows output lost its last line: {info:#?}"
    );
}

#[tokio::test]
async fn running_job_expires_without_interaction() {
    let (handle, initial) = HeadlessTerminal::new()
        .exec_job(
            long_running_command(),
            Duration::ZERO,
            Duration::from_millis(300),
        )
        .await
        .unwrap();
    assert!(initial.is_running());
    let handle = handle.unwrap();

    tokio::time::sleep(Duration::from_millis(700)).await;
    let expired = access_interact(handle, "", Duration::ZERO).await;
    assert!(matches!(expired.status(), JobStatus::NoExist));
}

#[tokio::test]
async fn interaction_restarts_running_job_expiration() {
    let expiration = Duration::from_millis(500);
    let (handle, initial) = HeadlessTerminal::new()
        .exec_job(long_running_command(), Duration::ZERO, expiration)
        .await
        .unwrap();
    assert!(initial.is_running());
    let handle = handle.unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    let refreshed = access_interact(JobHandle(handle.0.clone()), "", Duration::ZERO).await;
    assert!(refreshed.is_running());

    tokio::time::sleep(Duration::from_millis(300)).await;
    let still_running = access_interact(JobHandle(handle.0.clone()), "", Duration::ZERO).await;
    assert!(still_running.is_running());

    tokio::time::sleep(Duration::from_millis(800)).await;
    let expired = access_interact(handle, "", Duration::ZERO).await;
    assert!(matches!(expired.status(), JobStatus::NoExist));
}

#[tokio::test]
async fn cancelled_interaction_releases_worker_without_consuming_output() {
    let command = if cfg!(windows) {
        "Start-Sleep -Milliseconds 300; Write-Output after-cancel; Start-Sleep -Seconds 30"
    } else {
        "sleep 0.3; printf after-cancel; sleep 30"
    };
    let (handle, initial) = HeadlessTerminal::new()
        .exec_job(command, Duration::ZERO, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(initial.is_running());
    let handle = handle.unwrap();

    let pending = access_interact(JobHandle(handle.0.clone()), "", Duration::from_secs(30));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), pending)
            .await
            .is_err()
    );

    tokio::time::sleep(Duration::from_millis(500)).await;
    let read = tokio::time::timeout(
        Duration::from_secs(1),
        access_interact(JobHandle(handle.0.clone()), "", Duration::ZERO),
    )
    .await
    .expect("cancelled interaction should release the worker promptly");
    assert!(read.outputs().contains("after-cancel"));

    let _ = HeadlessTerminal::new().close_job(handle).await;
}

fn long_running_command() -> &'static str {
    if cfg!(windows) {
        "Start-Sleep -Seconds 30"
    } else {
        "sleep 30"
    }
}

#[cfg(unix)]
#[tokio::test]
async fn nohup_process_survives_shell_exit_and_container_expiration() {
    let marker = background_marker("nohup");
    let command = format!(
        "nohup sh -c 'sleep 0.4; printf survived > \"{}\"' >/dev/null 2>&1 &",
        marker.display()
    );

    let (handle, info) = HeadlessTerminal::new()
        .exec_job(&command, Duration::from_secs(2), Duration::from_millis(50))
        .await
        .unwrap();
    assert!(handle.is_none());
    assert!(matches!(info.status(), JobStatus::Exited(0)));

    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "survived");
    std::fs::remove_file(marker).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn attached_background_process_ends_with_terminal_session() {
    let marker = background_marker("attached");
    let command = format!(
        "sh -c 'sleep 0.4; printf escaped > \"{}\"' &",
        marker.display()
    );

    let (handle, info) = HeadlessTerminal::new()
        .exec_job(&command, Duration::from_secs(2), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(handle.is_none());
    assert!(matches!(info.status(), JobStatus::Exited(0)));

    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(!marker.exists());
}

#[cfg(unix)]
fn background_marker(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "phi-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[cfg(unix)]
#[tokio::test]
async fn interact_submits_unterminated_input_to_a_line_buffered_command() {
    let (handle, initial) = HeadlessTerminal::new().exec_job(
        "IFS= read -r value; if IFS= read -r -t 0.2 extra; then printf 'extra:%s' \"$extra\"; else printf 'received:%s' \"$value\"; fi",
        Duration::from_millis(20),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(initial.is_running());

    let info = access_interact(handle.unwrap(), "hello\r", Duration::from_secs(2)).await;
    assert!(matches!(info.status(), JobStatus::Exited(0)));
    assert!(info.outputs().contains("received:hello"));
    assert!(!info.outputs().contains("extra:"));
}

#[cfg(unix)]
#[tokio::test]
async fn pty_line_discipline_does_not_echo_submitted_input() {
    let (handle, initial) = HeadlessTerminal::new()
        .exec_job(
            "IFS= read -r value; printf done",
            Duration::from_millis(20),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(initial.is_running());

    let info = access_interact(handle.unwrap(), "private-input\r", Duration::from_secs(2)).await;
    assert!(matches!(info.status(), JobStatus::Exited(0)));
    assert_eq!(info.outputs(), "done");
}

#[cfg(unix)]
#[tokio::test]
async fn empty_input_submits_a_single_enter_while_omitting_input_only_reads() {
    let (handle, initial) = HeadlessTerminal::new()
        .exec_job(
            "IFS= read -r value; printf 'value:<%s>' \"$value\"",
            Duration::from_millis(20),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(initial.is_running());
    let handle = handle.unwrap();

    let read_only = access_interact(JobHandle(handle.0.clone()), "", Duration::ZERO).await;
    assert!(read_only.is_running());
    assert!(read_only.outputs().is_empty());

    let entered = access_interact(handle, "\r", Duration::from_secs(2)).await;
    assert!(matches!(entered.status(), JobStatus::Exited(0)));
    assert!(entered.outputs().contains("value:<>"));
}

#[cfg(unix)]
#[tokio::test]
async fn persistent_job_can_be_read_and_closed() {
    let (handle, initial) = HeadlessTerminal::new()
        .exec_job(
            "printf ready; sleep 10",
            Duration::from_secs(1),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(initial.is_running());
    assert_eq!(initial.outputs(), "ready");
    let handle = handle.unwrap();

    let read = access_interact(JobHandle(handle.0.clone()), "", Duration::ZERO).await;
    assert!(read.is_running());
    assert!(read.outputs().is_empty());

    let closed = HeadlessTerminal::new()
        .close_job(JobHandle(handle.0.clone()))
        .await
        .unwrap();
    assert!(matches!(closed.status(), JobStatus::Closed(_)));
    let missing = access_interact(handle, "", Duration::ZERO).await;
    assert!(matches!(missing.status(), JobStatus::NoExist));
}

#[cfg(unix)]
#[tokio::test]
async fn write_preserves_output_for_the_next_interaction_delta() {
    let (handle, initial) = HeadlessTerminal::new()
        .exec_job(line_input_command(), Duration::ZERO, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(initial.is_running());
    let handle = handle.unwrap();

    let result = HeadlessTerminal::new()
        .access_job(
            JobHandle(handle.0.clone()),
            JobAccess::Write {
                data: "preserved".into(),
            },
        )
        .await
        .unwrap();
    let JobAccessResult::Written(status) = result else {
        panic!("write returned terminal snapshot");
    };
    assert!(matches!(status, JobProcessStatus::Running));

    tokio::time::sleep(Duration::from_millis(20)).await;
    let read = access_interact(JobHandle(handle.0.clone()), "\r", Duration::from_secs(2)).await;
    assert!(read.outputs().contains("before"));
    assert!(read.outputs().contains("received:preserved"));

    let _ = HeadlessTerminal::new().close_job(handle).await;
}

#[cfg(unix)]
fn line_input_command() -> &'static str {
    "stty raw -echo; value=$(dd bs=1 count=9 2>/dev/null); printf before; dd bs=1 count=1 of=/dev/null 2>/dev/null; printf 'received:%s' \"$value\""
}
