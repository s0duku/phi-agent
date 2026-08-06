use std::io::{BufRead, BufReader};
#[cfg(not(test))]
use std::process::Output;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::headlessterm::job::{HeadlessTermError, JobHandle, TerminalCommand, WorkerLaunchStage};

use super::startup::WorkerLaunchReport;

const RPC_READY_WAIT: Duration = Duration::from_secs(5);

pub(super) fn spawn_worker(
    handle: &JobHandle,
    command: TerminalCommand,
    expiration: Duration,
) -> Result<(), HeadlessTermError> {
    #[cfg(test)]
    {
        let handle = handle.0.clone();
        let worker = super::server::PreparedWorker::new(&handle, command, expiration)
            .map_err(launch_report_error)?;
        std::thread::Builder::new()
            .name(format!("phi-headlessterm-{handle}"))
            .spawn(move || {
                if let Err(error) = worker.run() {
                    eprintln!("headlessterm worker failed: {error}");
                }
            })
            .map_err(|error| {
                HeadlessTermError::launch(WorkerLaunchStage::SpawnWorker, error.to_string())
            })?;
        Ok(())
    }
    #[cfg(not(test))]
    {
        let executable = std::env::current_exe().map_err(|error| {
            HeadlessTermError::launch(WorkerLaunchStage::SpawnWorker, error.to_string())
        })?;
        let output = Command::new(executable)
            .args([
                "headlessterm",
                "launch-local",
                &handle.0,
                &duration_millis(expiration).to_string(),
                &serde_json::to_string(&command)
                    .map_err(|error| HeadlessTermError::protocol(error.to_string()))?,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| {
                HeadlessTermError::launch(
                    WorkerLaunchStage::SpawnWorker,
                    format!("unable to start phi headlessterm worker: {error}"),
                )
            })?;
        parse_launch_output(&output)?.into_result()
    }
}

pub(super) fn launch_worker(
    handle: &str,
    command: TerminalCommand,
    expiration: Duration,
) -> WorkerLaunchReport {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            return WorkerLaunchReport::failed(WorkerLaunchStage::SpawnWorker, error.to_string());
        }
    };
    let mut worker = Command::new(executable);
    let command = match serde_json::to_string(&command) {
        Ok(command) => command,
        Err(error) => {
            return WorkerLaunchReport::failed(WorkerLaunchStage::SpawnWorker, error.to_string());
        }
    };
    worker
        .args([
            "headlessterm",
            "local",
            handle,
            &duration_millis(expiration).to_string(),
            &command,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    if let Err(error) = prevent_standard_handle_inheritance() {
        return WorkerLaunchReport::failed(WorkerLaunchStage::SpawnWorker, error);
    }
    configure_detached_worker(&mut worker);
    let mut worker = match worker.spawn() {
        Ok(worker) => worker,
        Err(error) => {
            return WorkerLaunchReport::failed(
                WorkerLaunchStage::SpawnWorker,
                format!("unable to launch detached phi headlessterm worker: {error}"),
            );
        }
    };
    let Some(stdout) = worker.stdout.take() else {
        return WorkerLaunchReport::failed(
            WorkerLaunchStage::AwaitWorker,
            "detached worker did not expose its launch report",
        );
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout)
            .read_line(&mut line)
            .map(|_| line)
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(RPC_READY_WAIT) {
        Ok(Ok(line)) => serde_json::from_str(&line).unwrap_or_else(|error| {
            WorkerLaunchReport::failed(
                WorkerLaunchStage::AwaitWorker,
                format!("invalid worker launch report: {error}"),
            )
        }),
        Ok(Err(error)) => WorkerLaunchReport::failed(WorkerLaunchStage::AwaitWorker, error),
        Err(_) => WorkerLaunchReport::failed(
            WorkerLaunchStage::AwaitWorker,
            "timed out waiting for worker launch report",
        ),
    }
}

#[cfg(windows)]
fn prevent_standard_handle_inheritance() -> Result<(), String> {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    for standard_handle in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(standard_handle) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(format!(
                "unable to detach inherited standard handle: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn configure_detached_worker(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
fn configure_detached_worker(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(not(test))]
fn parse_launch_output(output: &Output) -> Result<WorkerLaunchReport, HeadlessTermError> {
    match serde_json::from_slice(&output.stdout) {
        Ok(report) => Ok(report),
        Err(error) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            if detail.is_empty() {
                Err(HeadlessTermError::protocol(format!(
                    "invalid headlessterm launch report (status {}): {error}",
                    output.status
                )))
            } else {
                Err(HeadlessTermError::protocol(format!(
                    "invalid headlessterm launch report (status {}): {error}: {detail}",
                    output.status
                )))
            }
        }
    }
}

#[cfg(test)]
fn launch_report_error(report: WorkerLaunchReport) -> HeadlessTermError {
    report
        .into_result()
        .expect_err("failed worker launch report should contain an error")
}
