use std::{collections::BTreeSet, io, path::PathBuf};

use clap::{Args, Subcommand};

use super::Session;
use crate::{
    banner,
    features::pretty_history,
    headlessterm::{HeadlessTerminal, JobHandle},
    message::{PhiMessage, PhiToolMessage},
};

#[derive(Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: SessionCommand,
}

#[derive(Subcommand)]
pub enum SessionCommand {
    #[command(
        about = "Create a new empty session file without overwriting an existing path",
        before_help = banner::startup_banner()
    )]
    New(SessionNewArgs),
    #[command(
        about = "Print a session's committed history as an echo-style transcript",
        before_help = banner::startup_banner()
    )]
    History(SessionHistoryArgs),
    #[command(
        about = "Close jobs referenced by a session and delete its session file",
        before_help = banner::startup_banner()
    )]
    Delete(SessionDeleteArgs),
}

#[derive(Args)]
pub struct SessionNewArgs {
    #[arg(value_name = "SESSION")]
    pub file: PathBuf,
}

#[derive(Args)]
pub struct SessionHistoryArgs {
    pub file: PathBuf,
}

#[derive(Args)]
pub struct SessionDeleteArgs {
    #[arg(value_name = "SESSION")]
    pub file: PathBuf,
}

pub async fn run(args: SessionArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        SessionCommand::New(args) => new(args),
        SessionCommand::History(args) => history(args),
        SessionCommand::Delete(args) => delete(args).await,
    }
}

fn new(args: SessionNewArgs) -> Result<(), Box<dyn std::error::Error>> {
    Session::empty().create(args.file)
}

fn history(args: SessionHistoryArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = Session::load(&args.file)?;
    let history = render_history(&session);
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    use std::io::Write;
    if !history.is_empty() {
        handle.write_all(history.as_bytes())?;
        handle.write_all(b"\n")?;
    }
    Ok(())
}

async fn delete(args: SessionDeleteArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = Session::load(&args.file)?;
    let handles = running_job_handles(&session);
    let mut close_errors = Vec::new();

    for handle in handles {
        if let Err(error) = HeadlessTerminal::new()
            .close_job(JobHandle(handle.clone()))
            .await
        {
            close_errors.push(format!("{handle}: {error}"));
        }
    }

    std::fs::remove_file(&args.file).map_err(|error| {
        format!(
            "failed to delete session file {}: {error}",
            args.file.display()
        )
    })?;

    if close_errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "session deleted, but failed to close jobs: {}",
            close_errors.join(", ")
        )
        .into())
    }
}

fn running_job_handles(session: &Session) -> Vec<String> {
    let mut handles = BTreeSet::new();
    for message in session.history().iter() {
        let PhiMessage::Tool(PhiToolMessage::ToolResult { result, .. }) = message else {
            continue;
        };
        if result["value"]["status"] != "running" {
            continue;
        }
        let Some(handle) = result["value"]["handle"].as_str() else {
            continue;
        };
        if JobHandle::is_valid(handle) {
            handles.insert(handle.to_owned());
        }
    }
    handles.into_iter().collect()
}

fn render_history(session: &Session) -> String {
    pretty_history(&session.history())
}

#[cfg(test)]
mod tests {
    use super::{render_history, running_job_handles};

    use crate::{
        message::PhiMessage,
        session::{PhiAgentStep, Session},
    };

    #[test]
    fn finds_unique_running_job_handles_only() {
        let session = Session::from_root(
            PhiAgentStep::turn_end("done"),
            vec![
                PhiMessage::tool_result(
                    Some("one".to_owned()),
                    Some("bash_job".to_owned()),
                    serde_json::json!({
                        "value": { "status": "running", "handle": "mira-kest" }
                    }),
                ),
                PhiMessage::tool_result(
                    Some("two".to_owned()),
                    Some("bash_job".to_owned()),
                    serde_json::json!({
                        "value": { "status": "running", "handle": "mira-kest" }
                    }),
                ),
                PhiMessage::tool_result(
                    Some("three".to_owned()),
                    Some("bash_job".to_owned()),
                    serde_json::json!({
                        "value": { "status": "exited", "handle": null }
                    }),
                ),
            ],
        );

        assert_eq!(running_job_handles(&session), ["mira-kest"]);
    }

    #[test]
    fn history_command_renders_non_empty_session() {
        let path = std::env::temp_dir().join(format!(
            "phi-session-history-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session = Session::from_root(
            PhiAgentStep::turn_end("done"),
            vec![PhiMessage::user("hello")],
        );
        session.save(&path).unwrap();

        let rendered = render_history(&Session::load(&path).unwrap());
        assert!(!rendered.is_empty());

        std::fs::remove_file(path).unwrap();
    }
}
