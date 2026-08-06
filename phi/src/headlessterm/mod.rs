pub mod job;
pub(crate) mod worker;

use std::time::Duration;

use clap::{Args, Subcommand};

pub use job::{
    DEFAULT_TRY_WAIT, HeadlessTermError, JobAccess, JobAccessResult, JobHandle, JobInfo,
    JobProcessStatus, JobStatus, ReturnWhen, TerminalCommand, WorkerLaunchStage,
};

/// Client for Phi's persistent headlessterm worker.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeadlessTerminal;

impl HeadlessTerminal {
    pub const fn new() -> Self {
        Self
    }

    pub async fn exec_job<C, W>(
        &self,
        command: C,
        return_when: W,
        expiration: Duration,
    ) -> Result<(Option<JobHandle>, JobInfo), HeadlessTermError>
    where
        C: Into<TerminalCommand>,
        W: Into<ReturnWhen>,
    {
        worker::client::exec_job(command.into(), return_when.into(), expiration).await
    }

    pub async fn access_job(
        &self,
        handle: JobHandle,
        access: JobAccess,
    ) -> Result<JobAccessResult, HeadlessTermError> {
        worker::client::access_job(handle, access).await
    }

    pub async fn close_job(&self, handle: JobHandle) -> Result<JobInfo, HeadlessTermError> {
        worker::client::close_job(handle).await
    }
}

#[derive(Args)]
#[command(about = "Provide Phi's persistent headlessterm runtime")]
pub struct HeadlessTerminalArgs {
    #[command(subcommand)]
    command: HeadlessTerminalCommand,
}

#[derive(Subcommand)]
enum HeadlessTerminalCommand {
    Exec {
        #[arg(
            long,
            help = "Execute the command inside an already-running Docker container"
        )]
        container: Option<String>,
        #[arg(long, default_value_t = 60_000)]
        wait_ms: u64,
        #[arg(long, default_value_t = 300_000)]
        expiration_ms: u64,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    Access {
        handle: String,
        #[arg(long)]
        data: Option<String>,
        #[arg(long, default_value_t = 60_000)]
        wait_ms: u64,
        #[arg(long)]
        write_only: bool,
    },
    Close {
        handle: String,
    },
    #[command(
        name = "launch-local",
        about = "Launch a detached local headlessterm worker"
    )]
    LaunchLocal {
        handle: String,
        expiration_ms: u64,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    #[command(name = "local", about = "Run the local headlessterm worker")]
    Local {
        handle: String,
        expiration_ms: u64,
        command: String,
    },
}

pub async fn run(args: HeadlessTerminalArgs) -> Result<(), String> {
    let terminal = HeadlessTerminal::new();
    match args.command {
        HeadlessTerminalCommand::Exec {
            container,
            wait_ms,
            expiration_ms,
            command,
        } => {
            let info = terminal
                .exec_job(
                    match container {
                        Some(container) => {
                            TerminalCommand::docker_exec(container, command.join(" "))
                        }
                        None => TerminalCommand::shell(command.join(" ")),
                    },
                    ReturnWhen::output_settled(Duration::from_millis(wait_ms)),
                    Duration::from_millis(expiration_ms),
                )
                .await
                .map_err(|error| error.to_string())?;
            render(&info)?;
        }
        HeadlessTerminalCommand::Access {
            handle,
            data,
            wait_ms,
            write_only,
        } => {
            let result = terminal
                .access_job(
                    JobHandle(handle),
                    if write_only {
                        JobAccess::Write {
                            data: data.unwrap_or_default(),
                        }
                    } else {
                        JobAccess::Interact {
                            data: data.unwrap_or_default(),
                            return_when: ReturnWhen::output_settled(Duration::from_millis(wait_ms)),
                        }
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            render(&result)?;
        }
        HeadlessTerminalCommand::Close { handle } => {
            let info = terminal
                .close_job(JobHandle(handle))
                .await
                .map_err(|error| error.to_string())?;
            render(&info)?;
        }
        HeadlessTerminalCommand::LaunchLocal {
            handle,
            expiration_ms,
            command,
        } => {
            let command = command.join(" ");
            let report = match parse_terminal_command(&command) {
                Ok(command) => {
                    worker::launch_worker(&handle, Duration::from_millis(expiration_ms), command)
                }
                Err(error) => worker::WorkerLaunchReport::failed(
                    worker::WorkerLaunchStage::DecodeCommand,
                    error.to_string(),
                ),
            };
            println!(
                "{}",
                serde_json::to_string(&report).map_err(|error| error.to_string())?
            );
            report.into_result().map_err(|error| error.to_string())?;
        }
        HeadlessTerminalCommand::Local {
            handle,
            expiration_ms,
            command,
        } => {
            let command = serde_json::from_str(&command).map_err(|error| error.to_string())?;
            worker::worker_entry(&handle, Duration::from_millis(expiration_ms), command).await?;
        }
    }
    Ok(())
}

fn parse_terminal_command(command: &str) -> Result<TerminalCommand, serde_json::Error> {
    serde_json::from_str(command).or_else(|error| {
        if command.contains("\\\"") {
            serde_json::from_str(&command.replace("\\\"", "\""))
        } else {
            Err(error)
        }
    })
}

fn render(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| error.to_string())?
    );
    Ok(())
}
