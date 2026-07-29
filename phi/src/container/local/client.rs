use std::time::Duration;

use crate::container::job::{JobAccess, JobAccessResult, JobHandle, JobInfo, JobStatus};

use super::launcher;
use super::protocol::{Request, Response, Status};
use super::rpc;

pub(super) fn job_exec(
    command: &str,
    wait: Duration,
    expiration: Duration,
) -> Result<(Option<JobHandle>, JobInfo), String> {
    let handle = JobHandle::random()?;
    launcher::spawn_container(&handle, command, expiration)?;
    let result = job_access(
        JobHandle(handle.0.clone()),
        JobAccess::Interact {
            data: String::new(),
            wait,
        },
    )?;
    let JobAccessResult::Interacted(info) = result else {
        return Err("job access returned a write acknowledgment for interact request".into());
    };
    let live_handle = matches!(info.status(), JobStatus::Running).then_some(handle);
    Ok((live_handle, info))
}

pub(super) fn job_access(handle: JobHandle, access: JobAccess) -> Result<JobAccessResult, String> {
    let interacts = matches!(access, JobAccess::Interact { .. });
    let response = send_request(&handle.0, Request::Access(access))?;
    if interacts {
        response_into_job_info(response).map(JobAccessResult::Interacted)
    } else {
        response_into_status(response).map(JobAccessResult::Written)
    }
}

pub(super) fn job_close(handle: JobHandle) -> Result<JobInfo, String> {
    request(&handle.0, Request::Close)
}

fn request(handle: &str, request: Request) -> Result<JobInfo, String> {
    response_into_job_info(send_request(handle, request)?)
}

fn response_into_job_info(response: Option<Response>) -> Result<JobInfo, String> {
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
            output_truncated,
            waited_ms,
        }) => Ok(JobInfo::new(
            job_status(status),
            output,
            output_truncated,
            Duration::from_millis(waited_ms),
        )),
        Some(Response::Failed { error, .. }) => Err(error),
        Some(Response::Written { .. }) => {
            Err("job protocol returned write acknowledgment for terminal request".to_owned())
        }
    }
}

fn response_into_status(response: Option<Response>) -> Result<JobStatus, String> {
    match response {
        None => Ok(JobStatus::NoExist),
        Some(Response::Written { status }) => Ok(job_status(status)),
        Some(Response::Failed { error, .. }) => Err(error),
        Some(Response::Terminal { .. }) => {
            Err("job protocol returned terminal snapshot for write request".to_owned())
        }
    }
}

fn job_status(status: Status) -> JobStatus {
    match status {
        Status::Running => JobStatus::Running,
        Status::Exited(code) => JobStatus::Exited(code),
    }
}

fn send_request(handle: &str, request: Request) -> Result<Option<Response>, String> {
    let mut stream = match rpc::connect(handle) {
        Ok(stream) => stream,
        Err(error) if is_missing_endpoint(&error) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if let Err(error) = rpc::write_frame(&mut stream, &request) {
        if is_disconnected_endpoint(&error) {
            return Ok(None);
        }
        return Err(error.to_string());
    }
    match rpc::read_frame(&mut stream) {
        Ok(response) => Ok(Some(response)),
        Err(error) if is_disconnected_endpoint(&error) => Ok(None),
        Err(error) => Err(error.to_string()),
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
