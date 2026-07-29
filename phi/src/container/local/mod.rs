mod client;
pub(crate) mod interaction;
mod launcher;
pub(crate) mod lease;
pub(crate) mod platform;
mod process;
pub(crate) mod protocol;
mod pty;
pub(crate) mod rpc;
mod server;
pub(crate) mod terminal;

use crate::container::job::{JobAccess, JobAccessResult, JobContainer, JobHandle, JobInfo};

pub struct LocalShellJobContainer;

pub(crate) fn container_entry(
    handle: &str,
    expiration: std::time::Duration,
    command: &str,
) -> Result<(), String> {
    server::run_container(handle, command, expiration)
}

pub(crate) fn launch_container(
    handle: &str,
    expiration: std::time::Duration,
    command: &str,
) -> Result<(), String> {
    launcher::launch_container(handle, command, expiration)
}

#[async_trait::async_trait]
impl JobContainer for LocalShellJobContainer {
    async fn job_exec(
        cmd: &str,
        wait: std::time::Duration,
        expiration: std::time::Duration,
    ) -> Result<(Option<JobHandle>, JobInfo), String> {
        client::job_exec(cmd, wait, expiration)
    }

    async fn job_access(handle: JobHandle, access: JobAccess) -> Result<JobAccessResult, String> {
        client::job_access(handle, access)
    }

    async fn job_close(handle: JobHandle) -> Result<JobInfo, String> {
        client::job_close(handle)
    }
}
