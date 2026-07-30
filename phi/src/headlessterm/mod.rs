pub mod job;
pub(crate) mod worker;

use std::io::Write;
use std::time::Duration;

use clap::{Args, Subcommand};

pub use job::{
    JobAccess, JobAccessResult, JobHandle, JobInfo, JobStatus, ReturnWhen, TerminalCommand,
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
    ) -> Result<(Option<JobHandle>, JobInfo), String>
    where
        C: Into<TerminalCommand>,
        W: Into<ReturnWhen>,
    {
        worker::client::exec_job(command.into(), return_when.into(), expiration)
            .map_err(|error| error.to_string())
    }

    pub async fn access_job(
        &self,
        handle: JobHandle,
        access: JobAccess,
    ) -> Result<JobAccessResult, String> {
        worker::client::access_job(handle, access).map_err(|error| error.to_string())
    }

    pub async fn close_job(&self, handle: JobHandle) -> Result<JobInfo, String> {
        worker::client::close_job(handle).map_err(|error| error.to_string())
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
        #[arg(long, default_value_t = 1000)]
        wait_ms: u64,
        #[arg(long, default_value_t = 300_000)]
        expiration_ms: u64,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    Write {
        handle: String,
        data: Option<String>,
        #[arg(long, default_value_t = 1000)]
        wait_ms: u64,
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
            wait_ms,
            expiration_ms,
            command,
        } => {
            let info = terminal
                .exec_job(
                    TerminalCommand::shell(command.join(" ")),
                    ReturnWhen::output_settled(Duration::from_millis(wait_ms)),
                    Duration::from_millis(expiration_ms),
                )
                .await?;
            render(info.1, info.0.as_ref())?;
        }
        HeadlessTerminalCommand::Write {
            handle,
            data,
            wait_ms,
        } => {
            let result = terminal
                .access_job(
                    JobHandle(handle),
                    JobAccess::Interact {
                        data: data.unwrap_or_default(),
                        return_when: ReturnWhen::output_settled(Duration::from_millis(wait_ms)),
                    },
                )
                .await?;
            let JobAccessResult::Interacted(info) = result else {
                return Err(
                    "job access returned a write acknowledgment for interact request".into(),
                );
            };
            render(info, None)?;
        }
        HeadlessTerminalCommand::Close { handle } => {
            render(terminal.close_job(JobHandle(handle)).await?, None)?;
        }
        HeadlessTerminalCommand::LaunchLocal {
            handle,
            expiration_ms,
            command,
        } => {
            let command = command.join(" ");
            let command =
                serde_json::from_str(&command).unwrap_or_else(|_| TerminalCommand::shell(command));
            worker::launch_worker(&handle, Duration::from_millis(expiration_ms), command)?;
        }
        HeadlessTerminalCommand::Local {
            handle,
            expiration_ms,
            command,
        } => {
            let command = serde_json::from_str(&command).map_err(|error| error.to_string())?;
            worker::worker_entry(&handle, Duration::from_millis(expiration_ms), command)?;
        }
    }
    Ok(())
}

fn render(info: JobInfo, handle: Option<&JobHandle>) -> Result<(), String> {
    std::io::stdout()
        .write_all(info.outputs().as_bytes())
        .map_err(|error| error.to_string())?;
    let status = match info.status() {
        JobStatus::Running => "running".to_owned(),
        JobStatus::Exited(code) => format!("exited:{code}"),
        JobStatus::NoExist => "not-found".to_owned(),
    };
    match handle {
        Some(handle) => eprintln!("status={status} handle={}", handle.0),
        None => eprintln!("status={status}"),
    }
    Ok(())
}
