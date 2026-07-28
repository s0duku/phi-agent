use std::time::Duration;

use crate::container::{JobContainer, JobHandle, JobStatus, LocalShellJobContainer};
use crate::executor::PhiTool;
use crate::executor::tools::shell::job::{
    InteractArgs, ShellJobExecTool, ShellJobInteractTool, interactive_input,
};

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
    assert_eq!(args.timeout, 60_000);
}

#[test]
fn interact_completes_only_unterminated_nonempty_input() {
    assert_eq!(interactive_input(None), "");
    assert_eq!(interactive_input(Some(String::new())), "\r");
    assert_eq!(interactive_input(Some("hello".into())), "hello\r");
    assert_eq!(
        interactive_input(Some("hello\n".into())),
        if cfg!(windows) { "hello\r" } else { "hello\n" }
    );
    assert_eq!(
        interactive_input(Some("hello\r\n".into())),
        if cfg!(windows) {
            "hello\r"
        } else {
            "hello\r\n"
        }
    );

    let legacy: InteractArgs = serde_json::from_value(serde_json::json!({
        "handle": "mira-kest",
        "data": ""
    }))
    .unwrap();
    assert_eq!(legacy.input.as_deref(), Some(""));

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
        "x = 10\ry = 20\rprint(x + y)\r"
    );
    assert_eq!(
        interactive_input(Some("first\r\nsecond\n".into())),
        "first\rsecond\r"
    );
    assert_eq!(
        interactive_input(Some("already\rready\r".into())),
        "already\rready\r"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn windows_linear_output_beyond_one_screen_remains_complete() {
    let (_, info) = <LocalShellJobContainer as JobContainer>::job_exec(
        "1..100 | ForEach-Object { Write-Output \"line-$_\" }",
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert!(matches!(info.status(), JobStatus::Exited(0)));
    assert_eq!(info.outputs().lines().count(), 100);
    assert!(info.outputs().contains("line-1\n"));
    assert!(info.outputs().contains("line-100"));
}

#[tokio::test]
async fn running_job_expires_without_interaction() {
    let (handle, initial) = <LocalShellJobContainer as JobContainer>::job_exec(
        long_running_command(),
        Duration::ZERO,
        Duration::from_millis(300),
    )
    .await
    .unwrap();
    assert!(matches!(initial.status(), JobStatus::Running));
    let handle = handle.unwrap();

    tokio::time::sleep(Duration::from_millis(700)).await;
    let expired = <LocalShellJobContainer as JobContainer>::job_write(handle, "", Duration::ZERO)
        .await
        .unwrap();
    assert!(matches!(expired.status(), JobStatus::NoExist));
}

#[tokio::test]
async fn interaction_restarts_running_job_expiration() {
    let expiration = Duration::from_millis(500);
    let (handle, initial) = <LocalShellJobContainer as JobContainer>::job_exec(
        long_running_command(),
        Duration::ZERO,
        expiration,
    )
    .await
    .unwrap();
    assert!(matches!(initial.status(), JobStatus::Running));
    let handle = handle.unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    let refreshed = <LocalShellJobContainer as JobContainer>::job_write(
        JobHandle(handle.0.clone()),
        "",
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert!(matches!(refreshed.status(), JobStatus::Running));

    tokio::time::sleep(Duration::from_millis(300)).await;
    let still_running = <LocalShellJobContainer as JobContainer>::job_write(
        JobHandle(handle.0.clone()),
        "",
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert!(matches!(still_running.status(), JobStatus::Running));

    tokio::time::sleep(Duration::from_millis(800)).await;
    let expired = <LocalShellJobContainer as JobContainer>::job_write(handle, "", Duration::ZERO)
        .await
        .unwrap();
    assert!(matches!(expired.status(), JobStatus::NoExist));
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
async fn interact_submits_unterminated_input_to_a_line_buffered_command() {
    let (handle, initial) = <LocalShellJobContainer as JobContainer>::job_exec(
        "IFS= read -r value; if IFS= read -r -t 0.2 extra; then printf 'extra:%s' \"$extra\"; else printf 'received:%s' \"$value\"; fi",
        Duration::from_millis(20),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(initial.status(), JobStatus::Running));

    let info = <LocalShellJobContainer as JobContainer>::job_write(
        handle.unwrap(),
        &interactive_input(Some("hello".into())),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert!(matches!(info.status(), JobStatus::Exited(0)));
    assert!(info.outputs().contains("received:hello"));
    assert!(!info.outputs().contains("extra:"));
}

#[cfg(unix)]
#[tokio::test]
async fn empty_input_submits_a_single_enter_while_omitting_input_only_reads() {
    let (handle, initial) = <LocalShellJobContainer as JobContainer>::job_exec(
        "IFS= read -r value; printf 'value:<%s>' \"$value\"",
        Duration::from_millis(20),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(initial.status(), JobStatus::Running));
    let handle = handle.unwrap();

    let read_only = <LocalShellJobContainer as JobContainer>::job_write(
        JobHandle(handle.0.clone()),
        &interactive_input(None),
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert!(matches!(read_only.status(), JobStatus::Running));
    assert!(read_only.outputs().is_empty());

    let entered = <LocalShellJobContainer as JobContainer>::job_write(
        handle,
        &interactive_input(Some(String::new())),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert!(matches!(entered.status(), JobStatus::Exited(0)));
    assert!(entered.outputs().contains("value:<>"));
}

#[cfg(unix)]
#[tokio::test]
async fn persistent_job_can_be_read_and_closed() {
    let (handle, initial) = <LocalShellJobContainer as JobContainer>::job_exec(
        "printf ready; sleep 10",
        Duration::from_secs(1),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(initial.status(), JobStatus::Running));
    assert_eq!(initial.outputs(), "ready");
    let handle = handle.unwrap();

    let read = <LocalShellJobContainer as JobContainer>::job_write(
        JobHandle(handle.0.clone()),
        "",
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert!(matches!(read.status(), JobStatus::Running));
    assert!(read.outputs().is_empty());

    let closed = <LocalShellJobContainer as JobContainer>::job_close(JobHandle(handle.0.clone()))
        .await
        .unwrap();
    assert!(matches!(closed.status(), JobStatus::Exited(_)));
    let missing = <LocalShellJobContainer as JobContainer>::job_write(handle, "", Duration::ZERO)
        .await
        .unwrap();
    assert!(matches!(missing.status(), JobStatus::NoExist));
}
