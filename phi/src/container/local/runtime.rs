use std::io;
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Listener as _;

use crate::container::job::{JobHandle, JobInfo, JobStatus, TerminalSnapshot};

use super::pty::PtySession;
use super::rpc::{self, Request, Response, Status};
use super::wait::{ActivityExpiration, WaitPolicy};

#[cfg(not(test))]
use std::process::{Command, Stdio};

const CLIENT_IO_GRACE: Duration = Duration::from_secs(5);
const IDLE_POLL: Duration = super::wait::PROBE_INTERVAL;
const CLOSE_GRACE: Duration = Duration::from_millis(250);
const EXIT_OUTPUT_GRACE: Duration = Duration::from_millis(250);

pub(crate) fn job_exec(
    command: &str,
    timeout: Duration,
    expiration: Duration,
) -> Result<(Option<JobHandle>, JobInfo), String> {
    let handle = JobHandle::random()?;
    spawn_container(&handle, command, expiration)?;
    let info = job_write(JobHandle(handle.0.clone()), "", timeout)?;
    let live_handle = matches!(info.status(), JobStatus::Running).then_some(handle);
    Ok((live_handle, info))
}

pub(crate) fn job_write(
    handle: JobHandle,
    data: &str,
    timeout: Duration,
) -> Result<JobInfo, String> {
    request(
        &handle.0,
        Request::Interact {
            data: data.to_owned(),
            wait_millis: rpc::duration_millis(timeout),
        },
    )
}

pub(crate) fn job_send(handle: JobHandle, data: &str) -> Result<JobStatus, String> {
    request_status(
        &handle.0,
        Request::Write {
            data: data.to_owned(),
        },
    )
}

pub(crate) fn job_close(handle: JobHandle) -> Result<JobInfo, String> {
    request(&handle.0, Request::Close)
}

pub(crate) fn run_container(
    handle: &str,
    command: &str,
    expiration: Duration,
) -> Result<(), String> {
    let listener = rpc::bind(handle).map_err(|error| error.to_string())?;
    let mut job = RunningJob::spawn(command)?;
    let mut activity = ActivityExpiration::new(expiration, CLIENT_IO_GRACE);
    let mut terminal_flushed = false;

    loop {
        job.capture()?;
        let was_exited = job.has_exited();
        job.refresh_status()?;
        if !was_exited && job.has_exited() {
            activity.observe_exit();
        }
        if terminal_flushed && job.reached_eof() {
            return Ok(());
        }

        loop {
            match listener.accept() {
                Ok(mut stream) => {
                    let outcome = serve(&mut stream, &mut job)?;
                    if outcome.handled {
                        activity.observe_interaction();
                    }
                    terminal_flushed |= outcome.terminal_flushed;
                    if outcome.should_exit {
                        return Ok(());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.to_string()),
            }
        }

        if activity.elapsed() {
            job.expire()?;
            return Ok(());
        }
        std::thread::sleep(IDLE_POLL);
    }
}

struct RunningJob {
    pty: PtySession,
    exited_at: Option<Instant>,
}

impl RunningJob {
    fn spawn(command: &str) -> Result<Self, String> {
        Ok(Self {
            pty: PtySession::spawn(command)?,
            exited_at: None,
        })
    }

    fn capture(&mut self) -> Result<bool, String> {
        self.pty.capture()
    }

    fn refresh_status(&mut self) -> Result<Option<i8>, String> {
        let status = self.pty.refresh_status()?;
        if status.is_some() && self.exited_at.is_none() {
            self.exited_at = Some(Instant::now());
        }
        Ok(status)
    }

    fn has_exited(&self) -> bool {
        self.exited_at.is_some()
    }

    fn expire(&mut self) -> Result<(), String> {
        if !self.pty.reached_eof() {
            self.pty.terminate(true)?;
        }
        Ok(())
    }

    fn interact(&mut self, input: &[u8], timeout: Duration) -> Result<Status, String> {
        if !input.is_empty() {
            self.pty.write_all(input)?;
        }
        let mut wait = WaitPolicy::new(timeout, self.pty.pending_output_at());
        loop {
            wait.observe_output(self.capture()?);
            if let Some(code) = self.refresh_status()? {
                self.capture_after_exit()?;
                return Ok(Status::Exited(code));
            }
            let Some(remaining) = wait.remaining() else {
                return Ok(Status::Running);
            };
            std::thread::sleep(remaining.min(IDLE_POLL));
        }
    }

    fn write(&mut self, input: &[u8]) -> Result<Status, String> {
        if !input.is_empty() {
            self.pty.write_all(input)?;
        }
        self.refresh_status()?
            .map(Status::Exited)
            .map_or(Ok(Status::Running), Ok)
    }

    fn close(&mut self) -> Result<Status, String> {
        self.capture()?;
        if let Some(code) = self.refresh_status()? {
            self.capture_after_exit()?;
            return Ok(Status::Exited(code));
        }

        self.pty.terminate(false)?;
        if let Some(code) = self.wait_for_exit(CLOSE_GRACE)? {
            return Ok(Status::Exited(code));
        }

        self.pty.terminate(true)?;
        self.wait_for_exit(CLIENT_IO_GRACE)?
            .map(Status::Exited)
            .ok_or_else(|| "job did not stop".to_owned())
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<i8>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.capture()?;
            if let Some(code) = self.refresh_status()? {
                self.capture_after_exit()?;
                return Ok(Some(code));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(IDLE_POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    fn reached_eof(&self) -> bool {
        self.pty.reached_eof()
    }

    fn capture_after_exit(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + EXIT_OUTPUT_GRACE;
        while !self.pty.reached_eof() && Instant::now() < deadline {
            self.capture()?;
            if !self.pty.reached_eof() {
                std::thread::sleep(
                    IDLE_POLL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
        self.capture()?;
        Ok(())
    }
}

struct ServeOutcome {
    handled: bool,
    should_exit: bool,
    terminal_flushed: bool,
}

fn serve(stream: &mut impl ReadWrite, job: &mut RunningJob) -> Result<ServeOutcome, String> {
    let request: Request = match rpc::read_frame(stream) {
        Ok(request) => request,
        Err(_) => {
            return Ok(ServeOutcome {
                handled: false,
                should_exit: false,
                terminal_flushed: false,
            });
        }
    };
    let close_requested = matches!(request, Request::Close);
    let writes_only = matches!(request, Request::Write { .. });
    let result = match request {
        Request::Write { data } => job.write(data.as_bytes()),
        Request::Interact { data, wait_millis } => {
            job.interact(data.as_bytes(), Duration::from_millis(wait_millis))
        }
        Request::Close => job.close(),
    };
    let (status, error) = match result {
        Ok(status) => (status, None),
        Err(error) => {
            let fallback = job
                .refresh_status()?
                .map_or(Status::Running, Status::Exited);
            (fallback, Some(error))
        }
    };
    job.capture()?;
    let exited = matches!(status, Status::Exited(_));
    let (response, commit, terminal_response) = match error {
        Some(error) => (Response::Failed { status, error }, None, false),
        None if writes_only => (Response::Written { status }, None, false),
        None => {
            let prepared = job.pty.prepare_snapshot();
            let (terminal, commit) = prepared.into_parts();
            (Response::Terminal { status, terminal }, Some(commit), true)
        }
    };
    let should_exit = close_requested || (terminal_response && exited && job.reached_eof());
    if rpc::write_frame(stream, &response).is_err() {
        return Ok(ServeOutcome {
            handled: true,
            should_exit: close_requested,
            terminal_flushed: false,
        });
    }
    if let Some(commit) = commit {
        job.pty.commit_snapshot(commit);
    }
    Ok(ServeOutcome {
        handled: true,
        should_exit,
        terminal_flushed: terminal_response,
    })
}

trait ReadWrite: std::io::Read + std::io::Write {}
impl<T: std::io::Read + std::io::Write> ReadWrite for T {}

fn request(handle: &str, request: Request) -> Result<JobInfo, String> {
    send_request(handle, request)?.map_or_else(
        || {
            Ok(JobInfo::new(
                JobStatus::NoExist,
                TerminalSnapshot::default(),
            ))
        },
        response_into_job_info,
    )
}

fn request_status(handle: &str, request: Request) -> Result<JobStatus, String> {
    let Some(response) = send_request(handle, request)? else {
        return Ok(JobStatus::NoExist);
    };
    response_into_status(response)
}

fn response_into_job_info(response: Response) -> Result<JobInfo, String> {
    match response {
        Response::Terminal { status, terminal } => Ok(JobInfo::new(job_status(status), terminal)),
        Response::Failed { error, .. } => Err(error),
        Response::Written { .. } => {
            Err("job protocol returned write acknowledgment for terminal request".to_owned())
        }
    }
}

fn response_into_status(response: Response) -> Result<JobStatus, String> {
    match response {
        Response::Written { status } => Ok(job_status(status)),
        Response::Failed { error, .. } => Err(error),
        Response::Terminal { .. } => {
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

fn spawn_container(handle: &JobHandle, command: &str, expiration: Duration) -> Result<(), String> {
    #[cfg(test)]
    {
        let handle = handle.0.clone();
        let ready_handle = handle.clone();
        let command = command.to_owned();
        std::thread::Builder::new()
            .name(format!("phi-container-{}", handle))
            .spawn(move || {
                let _ = run_container(&handle, &command, expiration);
            })
            .map_err(|error| error.to_string())?;
        wait_until_ready(&ready_handle, CLIENT_IO_GRACE)
    }
    #[cfg(not(test))]
    {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        Command::new(executable)
            .args([
                "container",
                "local",
                &handle.0,
                &rpc::duration_millis(expiration).to_string(),
                command,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("unable to start phi local container: {error}"))?;
        wait_until_ready(&handle.0, CLIENT_IO_GRACE)
    }
}

fn wait_until_ready(handle: &str, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match rpc::connect(handle) {
            Ok(_) => return Ok(()),
            Err(error) if is_missing_endpoint(&error) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if is_missing_endpoint(&error) => {
                return Err("job daemon did not become ready".to_owned());
            }
            Err(error) => return Err(error.to_string()),
        }
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
