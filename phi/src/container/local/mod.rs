pub(crate) mod platform;
mod pty;
pub(crate) mod rpc;
mod runtime;
pub(crate) mod terminal;
pub(crate) mod wait;

use crate::container::job::{JobContainer, JobHandle, JobInfo};

pub struct LocalShellJobContainer;

pub(crate) fn container_entry(
    handle: &str,
    expiration: std::time::Duration,
    command: &str,
) -> Result<(), String> {
    runtime::run_container(handle, command, expiration)
}

#[async_trait::async_trait]
impl JobContainer for LocalShellJobContainer {
    async fn job_exec(
        cmd: &str,
        timeout: std::time::Duration,
        expiration: std::time::Duration,
    ) -> Result<(Option<JobHandle>, JobInfo), String> {
        runtime::job_exec(cmd, timeout, expiration)
    }

    async fn job_write(
        handle: JobHandle,
        data: &str,
        timeout: std::time::Duration,
    ) -> Result<JobInfo, String> {
        runtime::job_write(handle, data, timeout)
    }

    async fn job_close(handle: JobHandle) -> Result<JobInfo, String> {
        runtime::job_close(handle)
    }
}
