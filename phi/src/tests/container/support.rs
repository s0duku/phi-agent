#![cfg(unix)]

use std::io::{Read, Result as IoResult, Write};
use std::time::Duration;

use crate::container::job::{JobHandle, JobInfo};
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
    timeout: Duration,
    expiration: Duration,
) -> Result<(Option<JobHandle>, JobInfo), String> {
    block_on(<LocalShellJobContainer as JobContainer>::job_exec(
        cmd, timeout, expiration,
    ))
}

pub(crate) fn job_interact(
    handle: JobHandle,
    data: &str,
    timeout: Duration,
) -> Result<JobInfo, String> {
    block_on(<LocalShellJobContainer as JobContainer>::job_write(
        handle, data, timeout,
    ))
}

pub(crate) fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}
