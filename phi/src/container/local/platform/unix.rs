use std::io;

use portable_pty::CommandBuilder;

pub(crate) fn build_shell_command(command: &str) -> CommandBuilder {
    let shell = choose_shell();
    let mut builder = CommandBuilder::new(shell);
    builder.arg("-c");
    builder.arg(command);
    builder
}

pub(crate) fn poll_process_status(process_id: Option<u32>) -> io::Result<Option<i8>> {
    let Some(process_id) = process_id else {
        return Ok(None);
    };
    let mut status = 0;
    let result = unsafe { libc::waitpid(process_id as libc::pid_t, &mut status, libc::WNOHANG) };
    if result == 0 {
        return Ok(None);
    }
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) {
            return Ok(None);
        }
        return Err(error);
    }
    Ok(Some(normalize_wait_status(status)))
}

pub(crate) fn kill_process(group_leader: Option<libc::pid_t>, force: bool) -> io::Result<()> {
    let Some(group_leader) = group_leader else {
        return Ok(());
    };
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    if unsafe { libc::kill(-group_leader, signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn normalize_wait_status(status: libc::c_int) -> i8 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as u8 as i8
    } else if libc::WIFSIGNALED(status) {
        -(libc::WTERMSIG(status).min(i8::MAX as i32) as i8)
    } else {
        -1
    }
}

fn choose_shell() -> String {
    std::path::Path::new("/bin/bash")
        .is_file()
        .then(|| "/bin/bash".to_owned())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("bash"))
                    .find(|candidate| candidate.is_file())
                    .map(|path| path.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_else(|| "/bin/sh".to_owned())
}
