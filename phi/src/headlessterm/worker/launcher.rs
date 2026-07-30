use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::headlessterm::job::{JobHandle, TerminalCommand};

use super::rpc;

const RPC_READY_WAIT: Duration = Duration::from_secs(5);

pub(super) fn spawn_worker(
    handle: &JobHandle,
    command: TerminalCommand,
    expiration: Duration,
) -> Result<(), String> {
    #[cfg(test)]
    {
        let handle = handle.0.clone();
        let ready_handle = handle.clone();
        let command = command.clone();
        std::thread::Builder::new()
            .name(format!("phi-headlessterm-{handle}"))
            .spawn(move || {
                let _ = super::server::run_worker(&handle, command, expiration);
            })
            .map_err(|error| error.to_string())?;
        wait_until_ready(&ready_handle, RPC_READY_WAIT)
    }
    #[cfg(not(test))]
    {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let status = Command::new(executable)
            .args([
                "headlessterm",
                "launch-local",
                &handle.0,
                &duration_millis(expiration).to_string(),
                &serde_json::to_string(&command).map_err(|error| error.to_string())?,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("unable to start phi headlessterm worker: {error}"))?;
        if !status.success() {
            return Err(format!(
                "phi headlessterm launcher exited with status {status}"
            ));
        }
        wait_until_ready(&handle.0, RPC_READY_WAIT)
    }
}

pub(super) fn launch_worker(
    handle: &str,
    command: TerminalCommand,
    expiration: Duration,
) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut worker = Command::new(executable);
    let command = serde_json::to_string(&command).map_err(|error| error.to_string())?;
    worker
        .args([
            "headlessterm",
            "local",
            handle,
            &duration_millis(expiration).to_string(),
            &command,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    prevent_standard_handle_inheritance()?;
    configure_detached_worker(&mut worker);
    worker
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("unable to launch detached phi headlessterm worker: {error}"))
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

fn wait_until_ready(handle: &str, wait: Duration) -> Result<(), String> {
    let deadline = Instant::now() + wait;
    loop {
        match rpc::connect(handle) {
            Ok(_) => return Ok(()),
            Err(error) if is_missing_endpoint(&error) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if is_missing_endpoint(&error) => {
                return Err("job daemon did not become ready".to_owned());
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn is_missing_endpoint(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}
