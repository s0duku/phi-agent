use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::headlessterm::job::{
    HeadlessTermError, JobAccess, JobAccessResult, JobHandle, JobInfo, JobProcessStatus, JobStatus,
    ReturnWhen, TerminalCommand,
};

use super::launcher;
use super::protocol::{ProcessStatus, Request, Response, Status};
use super::rpc;

const WORKER_CONNECT_WAIT: Duration = Duration::from_secs(1);
const WORKER_CONNECT_RETRY: Duration = Duration::from_millis(10);

#[derive(Clone, Copy)]
enum MissingEndpoint {
    Return,
    RetryUntil(std::time::Instant),
}

pub(crate) async fn exec_job(
    command: TerminalCommand,
    return_when: ReturnWhen,
    expiration: Duration,
) -> Result<(Option<JobHandle>, JobInfo), HeadlessTermError> {
    let handle = JobHandle::random().map_err(HeadlessTermError::operation)?;
    launcher::spawn_worker(&handle, command, expiration)?;
    let response = send_interaction(
        &handle.0,
        String::new(),
        return_when,
        MissingEndpoint::RetryUntil(std::time::Instant::now() + WORKER_CONNECT_WAIT),
    )
    .await?;
    let result = response_into_job_info(response).map(JobAccessResult::Interacted)?;
    let JobAccessResult::Interacted(info) = result else {
        return Err(HeadlessTermError::protocol(
            "job access returned a write acknowledgment for interact request",
        ));
    };
    let live_handle = info.is_running().then_some(handle);
    Ok((live_handle, info))
}

pub(crate) async fn access_job(
    handle: JobHandle,
    access: JobAccess,
) -> Result<JobAccessResult, HeadlessTermError> {
    if let JobAccess::Interact { data, return_when } = access {
        let response =
            send_interaction(&handle.0, data, return_when, MissingEndpoint::Return).await?;
        return response_into_job_info(response).map(JobAccessResult::Interacted);
    }
    let response =
        send_request(&handle.0, Request::Access(access), MissingEndpoint::Return).await?;
    response_into_status(response).map(JobAccessResult::Written)
}

struct CancelOnDrop {
    handle: String,
    request_id: u64,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let handle = self.handle.clone();
        let request_id = self.request_id;
        tokio::spawn(async move {
            if let Ok(mut stream) = rpc::connect_async(&handle).await {
                let _ = rpc::write_frame_async(&mut stream, &Request::Cancel { request_id }).await;
            }
        });
    }
}

async fn send_interaction(
    handle: &str,
    data: String,
    return_when: ReturnWhen,
    missing_endpoint: MissingEndpoint,
) -> Result<Option<Response>, HeadlessTermError> {
    static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let mut stream = connect(handle, missing_endpoint).await?;
    let Some(stream) = stream.as_mut() else {
        return Ok(None);
    };
    rpc::write_frame_async(
        stream,
        &Request::Interact {
            request_id,
            data,
            return_when,
        },
    )
    .await
    .map_err(|error| HeadlessTermError::transport("write", error.to_string()))?;
    let mut cancel = CancelOnDrop {
        handle: handle.to_owned(),
        request_id,
        armed: true,
    };
    let response = rpc::read_frame_async(stream).await;
    match response {
        Ok(response) => {
            rpc::write_frame_async(stream, &Request::Acknowledge { request_id })
                .await
                .map_err(|error| HeadlessTermError::transport("acknowledge", error.to_string()))?;
            cancel.armed = false;
            Ok(Some(response))
        }
        Err(error) if is_disconnected_endpoint(&error) => Ok(None),
        Err(error) => Err(HeadlessTermError::transport("read", error.to_string())),
    }
}

async fn connect(
    handle: &str,
    missing_endpoint: MissingEndpoint,
) -> Result<Option<interprocess::local_socket::tokio::Stream>, HeadlessTermError> {
    loop {
        match rpc::connect_async(handle).await {
            Ok(stream) => return Ok(Some(stream)),
            Err(error) if is_missing_endpoint(&error) => match missing_endpoint {
                MissingEndpoint::RetryUntil(deadline) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(WORKER_CONNECT_RETRY).await
                }
                MissingEndpoint::Return | MissingEndpoint::RetryUntil(_) => return Ok(None),
            },
            Err(error) => return Err(HeadlessTermError::transport("connect", error.to_string())),
        }
    }
}

pub(crate) async fn close_job(handle: JobHandle) -> Result<JobInfo, HeadlessTermError> {
    request(&handle.0, Request::Close).await
}

async fn request(handle: &str, request: Request) -> Result<JobInfo, HeadlessTermError> {
    response_into_job_info(send_request(handle, request, MissingEndpoint::Return).await?)
}

fn response_into_job_info(response: Option<Response>) -> Result<JobInfo, HeadlessTermError> {
    match response {
        None => Ok(JobInfo::new(
            JobStatus::NoExist,
            String::new(),
            false,
            Duration::ZERO,
        )),
        Some(Response::Terminal {
            status,
            output,
            truncated,
            waited_ms,
        }) => Ok(JobInfo::new(
            job_status(status),
            output,
            truncated,
            Duration::from_millis(waited_ms),
        )),
        Some(Response::Failed { error, .. }) => Err(error),
        Some(Response::Written { .. }) => Err(HeadlessTermError::protocol(
            "job protocol returned write acknowledgment for terminal request",
        )),
    }
}

fn response_into_status(response: Option<Response>) -> Result<JobProcessStatus, HeadlessTermError> {
    match response {
        None => Ok(JobProcessStatus::NoExist),
        Some(Response::Written { status }) => Ok(process_status(status)),
        Some(Response::Failed { error, .. }) => Err(error),
        Some(Response::Terminal { .. }) => Err(HeadlessTermError::protocol(
            "job protocol returned terminal snapshot for write request",
        )),
    }
}

fn job_status(status: Status) -> JobStatus {
    match status {
        Status::RunningOutputSettled => JobStatus::RunningOutputSettled,
        Status::RunningScreenSampled => JobStatus::RunningScreenSampled,
        Status::RunningWaitElapsed => JobStatus::RunningWaitElapsed,
        Status::Exited(code) => JobStatus::Exited(code),
        Status::Closed(code) => JobStatus::Closed(code),
    }
}

fn process_status(status: ProcessStatus) -> JobProcessStatus {
    match status {
        ProcessStatus::Running => JobProcessStatus::Running,
        ProcessStatus::Exited(code) => JobProcessStatus::Exited(code),
    }
}

async fn send_request(
    handle: &str,
    request: Request,
    missing_endpoint: MissingEndpoint,
) -> Result<Option<Response>, HeadlessTermError> {
    let Some(mut stream) = connect(handle, missing_endpoint).await? else {
        return Ok(None);
    };
    if let Err(error) = rpc::write_frame_async(&mut stream, &request).await {
        if is_disconnected_endpoint(&error) {
            return Ok(None);
        }
        return Err(HeadlessTermError::transport("write", error.to_string()));
    }
    match rpc::read_frame_async(&mut stream).await {
        Ok(response) => Ok(Some(response)),
        Err(error) if is_disconnected_endpoint(&error) => Ok(None),
        Err(error) => Err(HeadlessTermError::transport("read", error.to_string())),
    }
}

fn is_missing_endpoint(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

fn is_disconnected_endpoint(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    )
}
