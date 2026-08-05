use std::io;
use std::time::Duration;

use interprocess::local_socket::{Listener, Stream, traits::Listener as _, traits::Stream as _};

use crate::headlessterm::job::{HeadlessTermError, JobAccess, TerminalCommand, WorkerLaunchStage};

use super::lease::ActivityExpiration;
use super::process::RunningJob;
use super::protocol::{ProcessStatus, Request, Response, Status};
use super::startup::WorkerLaunchReport;
use super::state::PendingTerminalResponse;
use super::{interaction, rpc};

const CLIENT_IO_GRACE: Duration = Duration::from_secs(5);

pub(super) fn run_worker(
    handle: &str,
    command: TerminalCommand,
    expiration: Duration,
) -> Result<(), String> {
    let worker = match PreparedWorker::new(handle, command, expiration) {
        Ok(worker) => worker,
        Err(report) => {
            write_launch_report(&report)?;
            return report.into_result().map_err(|error| error.to_string());
        }
    };
    write_launch_report(&WorkerLaunchReport::ready(handle))?;
    worker.run()
}

pub(super) struct PreparedWorker {
    listener: Listener,
    job: RunningJob,
    expiration: Duration,
}

impl PreparedWorker {
    pub(super) fn new(
        handle: &str,
        command: TerminalCommand,
        expiration: Duration,
    ) -> Result<Self, WorkerLaunchReport> {
        let listener = rpc::bind(handle).map_err(|error| {
            WorkerLaunchReport::failed(WorkerLaunchStage::BindRpc, error.to_string())
        })?;
        let job = RunningJob::spawn(command)
            .map_err(|error| WorkerLaunchReport::failed(WorkerLaunchStage::SpawnCommand, error))?;
        Ok(Self {
            listener,
            job,
            expiration,
        })
    }

    pub(super) fn run(self) -> Result<(), String> {
        let listener = self.listener;
        let mut job = self.job;
        let expiration = self.expiration;
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
}

fn write_launch_report(report: &WorkerLaunchReport) -> Result<(), String> {
    use std::io::Write as _;

    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, report).map_err(|error| error.to_string())?;
    stdout.write_all(b"\n").map_err(|error| error.to_string())?;
    stdout.flush().map_err(|error| error.to_string())
}

struct ServeOutcome {
    handled: bool,
    should_exit: bool,
    terminal_flushed: bool,
}

enum OperationResult {
    Written(ProcessStatus),
    Terminal {
        status: Status,
        waited: Duration,
        pending: PendingTerminalResponse,
    },
}

fn serve(stream: &mut Stream, job: &mut RunningJob) -> Result<ServeOutcome, String> {
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
        Request::Access(JobAccess::Interact { data, return_when }) => {
            stream
                .set_nonblocking(true)
                .map_err(|error| error.to_string())?;
            let interaction =
                job.interact(data.as_bytes(), return_when, || client_connected(stream));
            let Some(interaction) = interaction? else {
                return Ok(ServeOutcome {
                    handled: true,
                    should_exit: false,
                    terminal_flushed: false,
                });
            };
            stream
                .set_nonblocking(false)
                .map_err(|error| error.to_string())?;
            Ok({
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
                .map_or(ProcessStatus::Running, ProcessStatus::Exited);
            let response = Response::Failed {
                status,
                error: HeadlessTermError::operation(error),
            };
            return write_response(stream, response, None, close_requested, false, job);
        }
    };
    let (response, delivery, terminal_finished) = match result {
        OperationResult::Written(status) => (Response::Written { status }, None, false),
        OperationResult::Terminal {
            status,
            waited,
            pending,
        } => {
            let (output, truncated, delivery) = pending.into_parts();
            (
                Response::Terminal {
                    status,
                    output,
                    truncated,
                    waited_ms: duration_millis(waited),
                },
                Some(delivery),
                matches!(status, Status::Exited(_) | Status::Closed(_)) && job.reached_eof(),
            )
        }
    };
    write_response(
        stream,
        response,
        delivery,
        close_requested,
        terminal_finished,
        job,
    )
}

fn client_connected(stream: &mut Stream) -> bool {
    let mut byte = [0_u8; 1];
    matches!(
        std::io::Read::read(stream, &mut byte),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            )
    )
}

fn write_response(
    stream: &mut Stream,
    response: Response,
    delivery: Option<super::state::TerminalDelivery>,
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

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
