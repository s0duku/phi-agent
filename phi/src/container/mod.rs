pub mod job;
pub mod local;

use std::io::Write;
use std::time::Duration;

use clap::{Args, Subcommand};

pub use job::{
    JobAccess, JobAccessResult, JobContainer, JobHandle, JobInfo, JobStatus, TerminalSnapshot,
};
pub use local::LocalShellJobContainer;

#[derive(Args)]
#[command(about = "Provide the local container runtime used by phi persistent shell jobs")]
pub struct ContainerArgs {
    #[command(subcommand)]
    command: ContainerCommand,
}

#[derive(Subcommand)]
enum ContainerCommand {
    #[command(
        about = "Start a shell job through the container backend and print its initial output"
    )]
    Exec {
        #[arg(
            long,
            default_value_t = 1000,
            help = "Maximum initial wait for output activity or process exit, in milliseconds"
        )]
        wait_ms: u64,
        #[arg(
            long,
            default_value_t = 300_000,
            help = "Container inactivity lifetime before automatic termination and cleanup, in milliseconds"
        )]
        expiration_ms: u64,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    #[command(
        about = "Send input to a running shell job and read its current terminal snapshot from the container"
    )]
    Write {
        handle: String,
        data: Option<String>,
        #[arg(
            long,
            default_value_t = 1000,
            help = "Maximum wait for output activity or process exit, in milliseconds"
        )]
        wait_ms: u64,
    },
    #[command(about = "Close a running shell job and release its container resources")]
    Close { handle: String },
    #[command(
        name = "local",
        about = "Run the local shell container process used by phi job tools"
    )]
    Local {
        handle: String,
        #[arg(
            help = "Container inactivity lifetime before automatic termination and cleanup, in milliseconds"
        )]
        expiration_ms: u64,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

pub async fn run(args: ContainerArgs) -> Result<(), String> {
    match args.command {
        ContainerCommand::Exec {
            wait_ms,
            expiration_ms,
            command,
        } => {
            let command = command.join(" ");
            let (handle, info) = <LocalShellJobContainer as JobContainer>::job_exec(
                &command,
                Duration::from_millis(wait_ms),
                Duration::from_millis(expiration_ms),
            )
            .await?;
            render(info, handle.as_ref())?;
        }
        ContainerCommand::Write {
            handle,
            data,
            wait_ms,
        } => {
            let result = <LocalShellJobContainer as JobContainer>::job_access(
                JobHandle(handle),
                JobAccess::Interact {
                    data: data.unwrap_or_default(),
                    wait: Duration::from_millis(wait_ms),
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
        ContainerCommand::Close { handle } => {
            let info =
                <LocalShellJobContainer as JobContainer>::job_close(JobHandle(handle)).await?;
            render(info, None)?;
        }
        ContainerCommand::Local {
            handle,
            expiration_ms,
            command,
        } => {
            let command = command.join(" ");
            local::container_entry(&handle, Duration::from_millis(expiration_ms), &command)?;
        }
    }
    Ok(())
}

fn render(info: JobInfo, handle: Option<&JobHandle>) -> Result<(), String> {
    std::io::stdout()
        .write_all(info.terminal().text().as_bytes())
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
