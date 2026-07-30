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
mod supervisor;
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
    async fn exec_job(
        cmd: &str,
        try_wait: std::time::Duration,
        expiration: std::time::Duration,
    ) -> Result<(Option<JobHandle>, JobInfo), String> {
        client::exec_job(cmd, try_wait, expiration)
    }

    async fn access_job(handle: JobHandle, access: JobAccess) -> Result<JobAccessResult, String> {
        client::access_job(handle, access)
    }

    async fn close_job(handle: JobHandle) -> Result<JobInfo, String> {
        client::close_job(handle)
    }
}
