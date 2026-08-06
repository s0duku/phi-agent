use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use interprocess::local_socket::tokio::{Listener, Stream, prelude::*};

use crate::headlessterm::job::{HeadlessTermError, JobAccess, TerminalCommand, WorkerLaunchStage};

use super::lease::ActivityExpiration;
use super::process::RunningJob;
use super::protocol::{ProcessStatus, Request, Response, Status};
use super::startup::WorkerLaunchReport;
use super::state::PendingTerminalResponse;
use super::{interaction, rpc};

const CLIENT_IO_GRACE: Duration = Duration::from_secs(5);

pub(super) async fn run_worker(
    handle: &str,
    command: TerminalCommand,
    expiration: Duration,
) -> Result<(), String> {
    run_worker_async(handle, command, expiration, true).await
}

#[cfg(test)]
pub(super) fn run_test_worker(
    handle: &str,
    command: TerminalCommand,
    expiration: Duration,
) -> Result<(), String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?
        .block_on(run_worker_async(handle, command, expiration, false))
}

async fn run_worker_async(
    handle: &str,
    command: TerminalCommand,
    expiration: Duration,
    report_launch: bool,
) -> Result<(), String> {
    let worker = match PreparedWorker::new(handle, command, expiration).await {
        Ok(worker) => worker,
        Err(report) => {
            if report_launch {
                write_launch_report(&report)?;
            }
            return report.into_result().map_err(|error| error.to_string());
        }
    };
    if report_launch {
        write_launch_report(&WorkerLaunchReport::ready(handle))?;
    }
    worker.run().await
}

struct PreparedWorker {
    listener: Listener,
    job: RunningJob,
    expiration: Duration,
}

impl PreparedWorker {
    async fn new(
        handle: &str,
        command: TerminalCommand,
        expiration: Duration,
    ) -> Result<Self, WorkerLaunchReport> {
        let listener = rpc::bind_async(handle).await.map_err(|error| {
            WorkerLaunchReport::failed(WorkerLaunchStage::BindRpc, error.to_string())
        })?;
        let job = tokio::task::spawn_blocking(move || RunningJob::spawn(command))
            .await
            .map_err(|error| {
                WorkerLaunchReport::failed(WorkerLaunchStage::SpawnCommand, error.to_string())
            })?
            .map_err(|error| WorkerLaunchReport::failed(WorkerLaunchStage::SpawnCommand, error))?;
        Ok(Self {
            listener,
            job,
            expiration,
        })
    }

    async fn run(self) -> Result<(), String> {
        let listener = self.listener;
        let mut job = self.job;
        let mut activity = ActivityExpiration::new(self.expiration, CLIENT_IO_GRACE);
        let mut terminal_flushed = false;
        let mut pending_request = None;

        loop {
            let was_exited = job.has_exited();
            job = refresh_job(job).await?;
            if !was_exited && job.has_exited() {
                activity.observe_exit();
            }
            if terminal_flushed && job.reached_eof() {
                return Ok(());
            }

            let accepted = match pending_request.take() {
                Some(request) => Some(request),
                None => match tokio::time::timeout(interaction::POLL_INTERVAL, listener.accept())
                    .await
                {
                    Ok(Ok(stream)) => Some((stream, None)),
                    Ok(Err(error)) => return Err(error.to_string()),
                    Err(_) => None,
                },
            };
            if let Some((stream, request)) = accepted {
                let (next_job, outcome, next_request) =
                    serve(&listener, stream, request, job).await?;
                job = next_job;
                pending_request = next_request;
                if outcome.handled {
                    activity.observe_interaction();
                }
                terminal_flushed |= outcome.terminal_flushed;
                if outcome.should_exit {
                    return Ok(());
                }
            }

            if activity.elapsed() {
                tokio::task::spawn_blocking(move || job.expire())
                    .await
                    .map_err(|error| error.to_string())??;
                return Ok(());
            }
        }
    }
}

async fn refresh_job(mut job: RunningJob) -> Result<RunningJob, String> {
    tokio::task::spawn_blocking(move || {
        job.observe_terminal()?;
        job.refresh_status()?;
        Ok::<_, String>(job)
    })
    .await
    .map_err(|error| error.to_string())?
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

async fn serve(
    listener: &Listener,
    mut stream: Stream,
    request: Option<Request>,
    job: RunningJob,
) -> Result<(RunningJob, ServeOutcome, Option<(Stream, Option<Request>)>), String> {
    let request: Request = match request.map(Ok).unwrap_or_else(|| Err(())) {
        Ok(request) => request,
        Err(()) => match rpc::read_frame_async(&mut stream).await {
            Ok(request) => request,
            Err(_) => {
                return Ok((
                    job,
                    ServeOutcome {
                        handled: false,
                        should_exit: false,
                        terminal_flushed: false,
                    },
                    None,
                ));
            }
        },
    };
    let close_requested = matches!(request, Request::Close);
    let acknowledgement = match &request {
        Request::Interact { request_id, .. } => Some(*request_id),
        _ => None,
    };

    let (mut job, result) = match request {
        Request::Access(JobAccess::Write { data }) => {
            run_job(job, move |job| {
                job.write(data.as_bytes()).and_then(|status| {
                    job.observe_terminal()?;
                    Ok(OperationResult::Written(status))
                })
            })
            .await?
        }
        Request::Access(JobAccess::Interact { data, return_when }) => {
            let (job, outcome) = run_interaction(listener, job, 0, data, return_when).await?;
            match outcome {
                InteractionOutcome::Completed(result) => (job, Ok(result)),
                InteractionOutcome::Cancelled { pending } => {
                    return Ok((
                        job,
                        ServeOutcome {
                            handled: true,
                            should_exit: false,
                            terminal_flushed: false,
                        },
                        pending.map(|(stream, request)| (stream, Some(request))),
                    ));
                }
            }
        }
        Request::Interact {
            request_id,
            data,
            return_when,
        } => {
            let (job, outcome) =
                run_interaction(listener, job, request_id, data, return_when).await?;
            match outcome {
                InteractionOutcome::Completed(result) => (job, Ok(result)),
                InteractionOutcome::Cancelled { pending } => {
                    return Ok((
                        job,
                        ServeOutcome {
                            handled: true,
                            should_exit: false,
                            terminal_flushed: false,
                        },
                        pending.map(|(stream, request)| (stream, Some(request))),
                    ));
                }
            }
        }
        Request::Cancel { .. } | Request::Acknowledge { .. } => {
            return Ok((
                job,
                ServeOutcome {
                    handled: false,
                    should_exit: false,
                    terminal_flushed: false,
                },
                None,
            ));
        }
        Request::Close => {
            run_job(job, |job| {
                job.close().and_then(|status| {
                    job.observe_terminal()?;
                    Ok(OperationResult::Terminal {
                        status,
                        waited: Duration::ZERO,
                        pending: job.pending_response(),
                    })
                })
            })
            .await?
        }
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
            return write_response(stream, response, None, None, close_requested, false, job).await;
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
        acknowledgement,
        close_requested,
        terminal_finished,
        job,
    )
    .await
}

async fn run_interaction(
    listener: &Listener,
    job: RunningJob,
    request_id: u64,
    data: String,
    return_when: crate::headlessterm::job::ReturnWhen,
) -> Result<(RunningJob, InteractionOutcome), String> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let mut task = tokio::task::spawn_blocking(move || {
        let mut job = job;
        let result = job
            .interact(data.as_bytes(), return_when, || {
                !worker_cancelled.load(Ordering::Acquire)
            })
            .map(|interaction| {
                interaction.map(|interaction| {
                    let (status, waited, pending) = interaction.into_parts();
                    OperationResult::Terminal {
                        status,
                        waited,
                        pending,
                    }
                })
            });
        (job, result)
    });

    loop {
        tokio::select! {
            biased;
            joined = &mut task => {
                let (job, result) = joined.map_err(|error| error.to_string())?;
                return Ok((
                    job,
                    InteractionOutcome::Completed(result?.expect(
                        "an attached interaction cannot cancel itself",
                    )),
                ));
            }
            accepted = listener.accept() => {
                let mut cancel_stream = accepted.map_err(|error| error.to_string())?;
                let request = rpc::read_frame_async(&mut cancel_stream).await;
                if matches!(request, Ok(Request::Cancel { request_id: cancelled_id }) if cancelled_id == request_id) {
                    cancelled.store(true, Ordering::Release);
                    let (job, result) = task.await.map_err(|error| error.to_string())?;
                    result?;
                    return Ok((job, InteractionOutcome::Cancelled { pending: None }));
                }
                if let Ok(request) = request
                    && !matches!(request, Request::Cancel { .. } | Request::Acknowledge { .. })
                {
                    cancelled.store(true, Ordering::Release);
                    let (job, result) = task.await.map_err(|error| error.to_string())?;
                    result?;
                    return Ok((
                        job,
                        InteractionOutcome::Cancelled {
                            pending: Some((cancel_stream, request)),
                        },
                    ));
                }
            }
        }
    }
}

enum InteractionOutcome {
    Completed(OperationResult),
    Cancelled { pending: Option<(Stream, Request)> },
}

async fn run_job<F>(
    mut job: RunningJob,
    operation: F,
) -> Result<(RunningJob, Result<OperationResult, String>), String>
where
    F: FnOnce(&mut RunningJob) -> Result<OperationResult, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let result = operation(&mut job);
        (job, result)
    })
    .await
    .map_err(|error| error.to_string())
}

async fn write_response(
    mut stream: Stream,
    response: Response,
    delivery: Option<super::state::TerminalDelivery>,
    acknowledgement: Option<u64>,
    close_requested: bool,
    terminal_finished: bool,
    mut job: RunningJob,
) -> Result<(RunningJob, ServeOutcome, Option<(Stream, Option<Request>)>), String> {
    let terminal_flushed = delivery.is_some();
    if rpc::write_frame_async(&mut stream, &response)
        .await
        .is_err()
    {
        return Ok((
            job,
            ServeOutcome {
                handled: true,
                should_exit: close_requested,
                terminal_flushed: false,
            },
            None,
        ));
    }
    if let Some(delivery) = delivery {
        let acknowledged = match acknowledgement {
            Some(request_id) => matches!(
                rpc::read_frame_async(&mut stream).await,
                Ok(Request::Acknowledge { request_id: acknowledged_id })
                    if acknowledged_id == request_id
            ),
            None => true,
        };
        if acknowledged {
            job.acknowledge(delivery);
        }
    }
    Ok((
        job,
        ServeOutcome {
            handled: true,
            should_exit: close_requested || terminal_finished,
            terminal_flushed,
        },
        None,
    ))
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
