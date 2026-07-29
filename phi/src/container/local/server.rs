use std::io;
use std::time::Duration;

use interprocess::local_socket::traits::Listener as _;

use crate::container::job::JobAccess;

use super::lease::ActivityExpiration;
use super::process::RunningJob;
use super::protocol::{Request, Response, Status};
use super::terminal::PendingTerminalResponse;
use super::{interaction, rpc};

const CLIENT_IO_GRACE: Duration = Duration::from_secs(5);

pub(super) fn run_container(
    handle: &str,
    command: &str,
    expiration: Duration,
) -> Result<(), String> {
    let listener = rpc::bind(handle).map_err(|error| error.to_string())?;
    let mut job = RunningJob::spawn(command)?;
    let mut activity = ActivityExpiration::new(expiration, CLIENT_IO_GRACE);
    let mut terminal_flushed = false;

    loop {
        job.observe_terminal()?;
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
        std::thread::sleep(interaction::POLL_INTERVAL);
    }
}

struct ServeOutcome {
    handled: bool,
    should_exit: bool,
    terminal_flushed: bool,
}

enum OperationResult {
    Written(Status),
    Terminal {
        status: Status,
        waited: Duration,
        pending: PendingTerminalResponse,
    },
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
    let result = match request {
        Request::Access(JobAccess::Write { data }) => {
            job.write(data.as_bytes()).and_then(|status| {
                job.observe_terminal()?;
                Ok(OperationResult::Written(status))
            })
        }
        Request::Access(JobAccess::Interact { data, wait }) => {
            job.interact(data.as_bytes(), wait).map(|interaction| {
                let (status, waited, pending) = interaction.into_parts();
                OperationResult::Terminal {
                    status,
                    waited,
                    pending,
                }
            })
        }
        Request::Close => job.close().and_then(|status| {
            job.observe_terminal()?;
            Ok(OperationResult::Terminal {
                status,
                waited: Duration::ZERO,
                pending: job.pending_response(),
            })
        }),
    };
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            let status = job
                .refresh_status()?
                .map_or(Status::Running, Status::Exited);
            let response = Response::Failed { status, error };
            return write_response(stream, response, None, close_requested, false, job);
        }
    };
    let (response, delivery, status, terminal_response) = match result {
        OperationResult::Written(status) => (Response::Written { status }, None, status, false),
        OperationResult::Terminal {
            status,
            waited,
            pending,
        } => {
            let (output, output_truncated, delivery) = pending.into_parts();
            (
                Response::Terminal {
                    status,
                    output,
                    output_truncated,
                    waited_ms: duration_millis(waited),
                },
                Some(delivery),
                status,
                true,
            )
        }
    };
    write_response(
        stream,
        response,
        delivery,
        close_requested,
        terminal_response && matches!(status, Status::Exited(_)) && job.reached_eof(),
        job,
    )
}

fn write_response(
    stream: &mut impl ReadWrite,
    response: Response,
    delivery: Option<super::terminal::TerminalDelivery>,
    close_requested: bool,
    terminal_finished: bool,
    job: &mut RunningJob,
) -> Result<ServeOutcome, String> {
    let terminal_flushed = delivery.is_some();
    if rpc::write_frame(stream, &response).is_err() {
        return Ok(ServeOutcome {
            handled: true,
            should_exit: close_requested,
            terminal_flushed: false,
        });
    }
    if let Some(delivery) = delivery {
        job.acknowledge(delivery);
    }
    Ok(ServeOutcome {
        handled: true,
        should_exit: close_requested || terminal_finished,
        terminal_flushed,
    })
}

trait ReadWrite: std::io::Read + std::io::Write {}
impl<T: std::io::Read + std::io::Write> ReadWrite for T {}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
