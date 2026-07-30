#![cfg(unix)]

use std::io::{Read, Result as IoResult, Write};
use std::time::Duration;

use crate::headlessterm::job::{JobAccess, JobAccessResult, JobHandle, JobInfo};
use crate::headlessterm::worker::rpc;
use crate::headlessterm::{HeadlessTermError, HeadlessTerminal};

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

pub(crate) fn exec_job(
    cmd: &str,
    try_wait: Duration,
    expiration: Duration,
) -> Result<(Option<JobHandle>, JobInfo), HeadlessTermError> {
    block_on(HeadlessTerminal::new().exec_job(cmd, try_wait, expiration))
}

pub(crate) fn job_interact(
    handle: JobHandle,
    data: &str,
    try_wait: Duration,
) -> Result<JobInfo, HeadlessTermError> {
    let result = block_on(HeadlessTerminal::new().access_job(
        handle,
        JobAccess::Interact {
            data: data.to_owned(),
            return_when: try_wait.into(),
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
