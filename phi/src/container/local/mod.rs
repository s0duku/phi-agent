pub(crate) mod platform;
mod pty;
pub(crate) mod rpc;
mod runtime;
pub(crate) mod terminal;
pub(crate) mod wait;

use crate::container::job::{JobAccess, JobAccessResult, JobContainer, JobHandle, JobInfo};

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
        wait: std::time::Duration,
        expiration: std::time::Duration,
    ) -> Result<(Option<JobHandle>, JobInfo), String> {
        runtime::job_exec(cmd, wait, expiration)
    }

    async fn job_access(handle: JobHandle, access: JobAccess) -> Result<JobAccessResult, String> {
        runtime::job_access(handle, access)
    }

    async fn job_close(handle: JobHandle) -> Result<JobInfo, String> {
        runtime::job_close(handle)
    }
}
