#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{build_shell_command, disable_pty_echo, kill_process, poll_process_status};
#[cfg(windows)]
pub(crate) use windows::{
    build_shell_command, disable_pty_echo, kill_process, poll_process_status,
};
