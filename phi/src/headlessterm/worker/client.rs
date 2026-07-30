use std::time::Duration;

use crate::headlessterm::job::{
    HeadlessTermError, JobAccess, JobAccessResult, JobHandle, JobInfo, JobStatus, ReturnWhen,
    TerminalCommand,
};

use super::launcher;
use super::protocol::{Request, Response, Status};
use super::rpc;

pub(crate) fn exec_job(
    command: TerminalCommand,
    return_when: ReturnWhen,
    expiration: Duration,
) -> Result<(Option<JobHandle>, JobInfo), HeadlessTermError> {
    let handle = JobHandle::random().map_err(HeadlessTermError::operation)?;
    launcher::spawn_worker(&handle, command, expiration)?;
    let result = access_job(
        JobHandle(handle.0.clone()),
        JobAccess::Interact {
            data: String::new(),
            return_when,
        },
    )?;
    let JobAccessResult::Interacted(info) = result else {
        return Err(HeadlessTermError::protocol(
            "job access returned a write acknowledgment for interact request",
        ));
    };
    let live_handle = matches!(info.status(), JobStatus::Running).then_some(handle);
    Ok((live_handle, info))
}

pub(crate) fn access_job(
    handle: JobHandle,
    access: JobAccess,
) -> Result<JobAccessResult, HeadlessTermError> {
    let interacts = matches!(access, JobAccess::Interact { .. });
    let response = send_request(&handle.0, Request::Access(access))?;
    if interacts {
        response_into_job_info(response).map(JobAccessResult::Interacted)
    } else {
        response_into_status(response).map(JobAccessResult::Written)
    }
}

pub(crate) fn close_job(handle: JobHandle) -> Result<JobInfo, HeadlessTermError> {
    request(&handle.0, Request::Close)
}

fn request(handle: &str, request: Request) -> Result<JobInfo, HeadlessTermError> {
    response_into_job_info(send_request(handle, request)?)
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

fn response_into_status(response: Option<Response>) -> Result<JobStatus, HeadlessTermError> {
    match response {
        None => Ok(JobStatus::NoExist),
        Some(Response::Written { status }) => Ok(job_status(status)),
        Some(Response::Failed { error, .. }) => Err(error),
        Some(Response::Terminal { .. }) => Err(HeadlessTermError::protocol(
            "job protocol returned terminal snapshot for write request",
        )),
    }
}

fn job_status(status: Status) -> JobStatus {
    match status {
        Status::Running => JobStatus::Running,
        Status::Exited(code) => JobStatus::Exited(code),
    }
}

fn send_request(handle: &str, request: Request) -> Result<Option<Response>, HeadlessTermError> {
    let mut stream = match rpc::connect(handle) {
        Ok(stream) => stream,
        Err(error) if is_missing_endpoint(&error) => return Ok(None),
        Err(error) => return Err(HeadlessTermError::transport("connect", error.to_string())),
    };
    if let Err(error) = rpc::write_frame(&mut stream, &request) {
        if is_disconnected_endpoint(&error) {
            return Ok(None);
        }
        return Err(HeadlessTermError::transport("write", error.to_string()));
    }
    match rpc::read_frame(&mut stream) {
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
