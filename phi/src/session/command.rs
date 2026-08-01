use std::{collections::BTreeMap, collections::BTreeSet, io, path::PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Args, Command, Error, FromArgMatches, Subcommand};

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
        about = "Create a new initialized session file without overwriting an existing path",
        before_help = banner::startup_banner()
    )]
    New(SessionNewArgs),
    #[command(
        about = "Append user or assistant messages to the outermost session delta",
        before_help = banner::startup_banner()
    )]
    Append(SessionAppendArgs),
    #[command(
        about = "Remove the outermost session frame",
        before_help = banner::startup_banner()
    )]
    Rollback(SessionRollbackArgs),
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

#[derive(Default)]
pub struct SessionAppendArgs {
    pub file: Option<PathBuf>,
    messages: Vec<PhiMessage>,
}

#[derive(Args)]
pub struct SessionRollbackArgs {
    #[arg(value_name = "SESSION")]
    pub file: Option<PathBuf>,
}

#[derive(Args)]
pub struct SessionDeleteArgs {
    #[arg(value_name = "SESSION")]
    pub file: PathBuf,
}

pub async fn run(
    home_spec: Option<&str>,
    args: SessionArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        SessionCommand::New(args) => new(home_spec, args),
        SessionCommand::Append(args) => append(args),
        SessionCommand::Rollback(args) => rollback(args),
        SessionCommand::History(args) => history(args),
        SessionCommand::Delete(args) => delete(args).await,
    }
}

fn append(args: SessionAppendArgs) -> Result<(), Box<dyn std::error::Error>> {
    transform(args.file, |session| session.append_messages(args.messages))
}

fn rollback(args: SessionRollbackArgs) -> Result<(), Box<dyn std::error::Error>> {
    transform(args.file, Session::rollback)
}

fn transform(
    file: Option<PathBuf>,
    operation: impl FnOnce(Session) -> Session,
) -> Result<(), Box<dyn std::error::Error>> {
    let session = match &file {
        Some(path) => Session::load(path)?,
        None => {
            use std::io::Read;
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input)?;
            Session::load_bytes(&input)?
        }
    };
    let session = operation(session);
    match file {
        Some(path) => session.save(path),
        None => session.write_stdout(),
    }
}

impl FromArgMatches for SessionAppendArgs {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let mut matches = matches.clone();
        Self::from_arg_matches_mut(&mut matches)
    }

    fn from_arg_matches_mut(matches: &mut ArgMatches) -> Result<Self, Error> {
        let mut ordered = BTreeMap::new();
        collect_messages(matches, "user", PhiMessage::user, &mut ordered);
        collect_messages(matches, "assistant", PhiMessage::assistant, &mut ordered);
        Ok(Self {
            file: matches.remove_one("file"),
            messages: ordered.into_values().collect(),
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }

    fn update_from_arg_matches_mut(&mut self, matches: &mut ArgMatches) -> Result<(), Error> {
        *self = Self::from_arg_matches_mut(matches)?;
        Ok(())
    }
}

impl Args for SessionAppendArgs {
    fn augment_args(command: Command) -> Command {
        command
            .arg(
                Arg::new("file")
                    .value_name("SESSION")
                    .value_parser(clap::value_parser!(PathBuf)),
            )
            .arg(
                Arg::new("user")
                    .long("user")
                    .value_name("TEXT")
                    .action(ArgAction::Append),
            )
            .arg(
                Arg::new("assistant")
                    .long("assistant")
                    .value_name("TEXT")
                    .action(ArgAction::Append),
            )
    }

    fn augment_args_for_update(command: Command) -> Command {
        Self::augment_args(command)
    }
}

fn collect_messages(
    matches: &ArgMatches,
    id: &str,
    construct: impl Fn(String) -> PhiMessage,
    ordered: &mut BTreeMap<usize, PhiMessage>,
) {
    let Some(values) = matches.get_many::<String>(id) else {
        return;
    };
    let indices = matches
        .indices_of(id)
        .expect("message values must retain their argument positions");
    for (text, index) in values.zip(indices) {
        ordered.insert(index, construct(text.clone()));
    }
}

fn new(home_spec: Option<&str>, args: SessionNewArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = crate::home::load_home(home_spec)?;
    crate::new_session(home.as_ref())?.create(args.file)
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
