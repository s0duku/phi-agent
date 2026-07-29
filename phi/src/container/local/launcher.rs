use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::container::job::JobHandle;

use super::rpc;

const RPC_READY_WAIT: Duration = Duration::from_secs(5);

pub(super) fn spawn_container(
    handle: &JobHandle,
    command: &str,
    expiration: Duration,
) -> Result<(), String> {
    #[cfg(test)]
    {
        let handle = handle.0.clone();
        let ready_handle = handle.clone();
        let command = command.to_owned();
        std::thread::Builder::new()
            .name(format!("phi-container-{handle}"))
            .spawn(move || {
                let _ = super::server::run_container(&handle, &command, expiration);
            })
            .map_err(|error| error.to_string())?;
        wait_until_ready(&ready_handle, RPC_READY_WAIT)
    }
    #[cfg(not(test))]
    {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let status = Command::new(executable)
            .args([
                "container",
                "launch-local",
                &handle.0,
                &duration_millis(expiration).to_string(),
                command,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("unable to start phi local container: {error}"))?;
        if !status.success() {
            return Err(format!(
                "phi container launcher exited with status {status}"
            ));
        }
        wait_until_ready(&handle.0, RPC_READY_WAIT)
    }
}

pub(super) fn launch_container(
    handle: &str,
    command: &str,
    expiration: Duration,
) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut worker = Command::new(executable);
    worker
        .args([
            "container",
            "local",
            handle,
            &duration_millis(expiration).to_string(),
            command,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detached_worker(&mut worker);
    worker
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("unable to launch detached phi container: {error}"))
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
