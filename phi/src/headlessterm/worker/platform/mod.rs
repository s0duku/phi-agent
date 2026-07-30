#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{disable_pty_echo, kill_process, poll_process_status};
#[cfg(windows)]
pub(crate) use windows::{disable_pty_echo, kill_process, poll_process_status};
