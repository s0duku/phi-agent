pub(crate) mod client;
mod command;
pub(crate) mod interaction;
mod launcher;
pub(crate) mod lease;
pub(crate) mod platform;
mod process;
pub(crate) mod protocol;
mod pty;
pub(crate) mod rpc;
mod server;
mod startup;
pub(crate) mod state;

use crate::headlessterm::job::TerminalCommand;
pub(crate) use crate::headlessterm::job::WorkerLaunchStage;
pub(crate) use startup::WorkerLaunchReport;

pub(crate) fn worker_entry(
    handle: &str,
    expiration: std::time::Duration,
    command: TerminalCommand,
) -> Result<(), String> {
    server::run_worker(handle, command, expiration)
}

pub(crate) fn launch_worker(
    handle: &str,
    expiration: std::time::Duration,
    command: TerminalCommand,
) -> WorkerLaunchReport {
    launcher::launch_worker(handle, command, expiration)
}
