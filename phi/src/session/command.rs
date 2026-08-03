use std::{collections::BTreeSet, io, io::IsTerminal, path::PathBuf};

use clap::{Arg, ArgMatches, Args, Command, Error, FromArgMatches, Subcommand};

use super::{PhiReActStep, Session};
use crate::{
    agent::PhiAgentCommand,
    banner,
    cli::MessageArgs,
    config::ModelRequestDefaults,
    features::pretty_history,
    headlessterm::{HeadlessTerminal, JobHandle},
    message::{PhiMessage, PhiToolMessage},
    render::PhiProviderCall,
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
        about = "Add a new outer session frame",
        before_help = banner::startup_banner()
    )]
    Next(SessionStepTransformArgs),
    #[command(
        about = "Replace the outermost session step while preserving its delta",
        before_help = banner::startup_banner()
    )]
    Replace(SessionStepTransformArgs),
    #[command(
        about = "Resolve the first call in the current RequestExecutor step",
        before_help = banner::startup_banner()
    )]
    ToolResult(SessionToolResultArgs),
    #[command(
        about = "Inspect a session's current eval-state and governance status as JSON",
        before_help = banner::startup_banner()
    )]
    Peek(SessionPeekArgs),
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
    messages: MessageArgs,
}

#[derive(Args)]
pub struct SessionRollbackArgs {
    #[arg(value_name = "SESSION")]
    pub file: Option<PathBuf>,
}

#[derive(Args)]
pub struct SessionStepTransformArgs {
    #[arg(value_name = "SESSION")]
    pub file: Option<PathBuf>,
    #[arg(long, required = true)]
    pub provider: bool,
}

#[derive(Args)]
#[command(group(
    clap::ArgGroup::new("result")
        .required(true)
        .multiple(false)
        .args(["json", "text"])
))]
pub struct SessionToolResultArgs {
    #[arg(value_name = "SESSION")]
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub text: Option<String>,
}

#[derive(Args)]
pub struct SessionPeekArgs {
    #[arg(value_name = "SESSION")]
    pub file: Option<PathBuf>,
    #[arg(long = "max-model-request-retries", value_name = "N")]
    pub max_model_request_retries: Option<usize>,
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
        SessionCommand::Next(args) => next(home_spec, args),
        SessionCommand::Replace(args) => replace(home_spec, args),
        SessionCommand::ToolResult(args) => tool_result(home_spec, args),
        SessionCommand::Peek(args) => peek(home_spec, args),
        SessionCommand::Rollback(args) => rollback(args),
        SessionCommand::History(args) => history(args),
        SessionCommand::Delete(args) => delete(args).await,
    }
}

fn peek(home_spec: Option<&str>, args: SessionPeekArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = match args.file.as_deref() {
        Some(path) => Session::load(path)?,
        None if io::stdin().is_terminal() => Session::empty(),
        None => {
            use std::io::Read;
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input)?;
            if input.iter().all(|byte| byte.is_ascii_whitespace()) {
                Session::empty()
            } else {
                Session::load_bytes(&input)?
            }
        }
    };
    let home = crate::home::load_home(home_spec)?;
    let retries = args
        .max_model_request_retries
        .or(PhiAgentCommand::probe().max_model_request_retries);
    let agent = crate::agent::build_agent(
        session,
        PhiAgentCommand::Probe(PhiAgentCommand::probe().with_max_model_request_retries(retries)),
        home,
    )?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &agent.probe_report())?;
    use std::io::Write;
    handle.write_all(b"\n")?;
    Ok(())
}

fn append(args: SessionAppendArgs) -> Result<(), Box<dyn std::error::Error>> {
    transform(args.file, |session| {
        Ok(session.append_messages(args.messages.messages()))
    })
}

fn next(
    home_spec: Option<&str>,
    args: SessionStepTransformArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let step = provider_step(home_spec, args.provider)?;
    transform(args.file, |session| Ok(session.next(step)))
}

fn replace(
    home_spec: Option<&str>,
    args: SessionStepTransformArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let step = provider_step(home_spec, args.provider)?;
    transform(args.file, |session| Ok(session.replace(step)))
}

fn tool_result(
    home_spec: Option<&str>,
    args: SessionToolResultArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = match (args.json, args.text) {
        (Some(json), None) => serde_json::from_str(&json)
            .map_err(|error| format!("invalid --json tool result: {error}"))?,
        (None, Some(text)) => serde_json::Value::String(text),
        _ => unreachable!("clap requires exactly one tool result input"),
    };
    let resume_call = provider_call(home_spec)?;
    transform(args.file, |session| {
        session
            .insert_tool_result(result, resume_call)
            .map_err(Into::into)
    })
}

fn provider_step(
    home_spec: Option<&str>,
    provider: bool,
) -> Result<PhiReActStep, Box<dyn std::error::Error>> {
    if !provider {
        return Err("a session step kind is required".into());
    }
    Ok(PhiReActStep::request_provider_with_call(
        "ready",
        provider_call(home_spec)?,
    ))
}

fn provider_call(home_spec: Option<&str>) -> Result<PhiProviderCall, Box<dyn std::error::Error>> {
    let home = crate::home::load_home(home_spec)?;
    let defaults = ModelRequestDefaults::from_config(&home.config()?)?;
    Ok(PhiProviderCall::from_parts(&defaults, Vec::new()))
}

fn rollback(args: SessionRollbackArgs) -> Result<(), Box<dyn std::error::Error>> {
    transform(args.file, |session| Ok(Session::rollback(session)))
}

fn transform(
    file: Option<PathBuf>,
    operation: impl FnOnce(Session) -> Result<Session, Box<dyn std::error::Error>>,
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
    let session = operation(session)?;
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
        Ok(Self {
            file: matches.remove_one("file"),
            messages: MessageArgs::parse(matches),
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
        MessageArgs::augment(
            command.arg(
                Arg::new("file")
                    .value_name("SESSION")
                    .value_parser(clap::value_parser!(PathBuf)),
            ),
        )
    }

    fn augment_args_for_update(command: Command) -> Command {
        Self::augment_args(command)
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
        if !matches!(
            result["value"]["status"].as_str(),
            Some(
                "running"
                    | "running_output_settled"
                    | "running_screen_sampled"
                    | "running_wait_elapsed"
            )
        ) {
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
