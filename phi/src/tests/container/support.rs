#![cfg(unix)]

use std::io::{Read, Result as IoResult, Write};
use std::time::Duration;

use crate::container::job::{JobAccess, JobAccessResult, JobHandle, JobInfo};
use crate::container::local::rpc;
use crate::container::{JobContainer, LocalShellJobContainer};

pub(crate) struct EndpointStream(pub(crate) interprocess::local_socket::Stream);

impl Read for EndpointStream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.0.read(buf)
    }
}

impl Write for EndpointStream {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> IoResult<()> {
        self.0.flush()
    }
}

pub(crate) fn connect(handle: &str) -> std::io::Result<EndpointStream> {
    rpc::connect(handle).map(EndpointStream)
}

pub(crate) fn job_exec(
    cmd: &str,
    wait: Duration,
    expiration: Duration,
) -> Result<(Option<JobHandle>, JobInfo), String> {
    block_on(<LocalShellJobContainer as JobContainer>::job_exec(
        cmd, wait, expiration,
    ))
}

pub(crate) fn job_interact(
    handle: JobHandle,
    data: &str,
    wait: Duration,
) -> Result<JobInfo, String> {
    let result = block_on(<LocalShellJobContainer as JobContainer>::job_access(
        handle,
        JobAccess::Interact {
            data: data.to_owned(),
            wait,
        },
    ))?;
    match result {
        JobAccessResult::Interacted(info) => Ok(info),
        JobAccessResult::Written(_) => panic!("interact returned write acknowledgment"),
    }
}

pub(crate) fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}
