use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};

use clap::{Arg, ArgMatches, Args, Command, Error, FromArgMatches, Subcommand};

use super::{PhiReActStep, Session, SessionStepKind, SessionTarget};
use crate::{
    banner,
    cli::MessageArgs,
    config::ModelRequestDefaults,
    features::pretty_history,
    headlessterm::{HeadlessTerminal, JobHandle},
    message::PhiMessage,
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
        about = "Store a JSON value in the outermost session delta",
        before_help = banner::startup_banner()
    )]
    Store(SessionStoreArgs),
    #[command(
        about = "Remove a key from the outermost session delta",
        before_help = banner::startup_banner()
    )]
    Remove(SessionRemoveArgs),
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
        about = "Explain a session's current state as structured JSON",
        before_help = banner::startup_banner()
    )]
    State(SessionStateArgs),
    #[command(
        about = "Remove the outermost session frame",
        before_help = banner::startup_banner()
    )]
    Rollback(SessionRollbackArgs),
    #[command(
        about = "Print a session's committed history as JSON or an echo-style transcript",
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
    #[command(flatten)]
    config: crate::ConfigArgs,
    #[arg(value_name = "SESSION", help = "Session file, or - for stdout")]
    target: SessionTarget,
}

#[derive(Args)]
pub struct SessionHistoryArgs {
    #[arg(
        long,
        help = "Render history as an echo-style transcript instead of JSON"
    )]
    pub view: bool,
    #[arg(value_name = "SESSION", help = "Session file, or - for stdin")]
    target: SessionTarget,
}

pub struct SessionAppendArgs {
    target: SessionTarget,
    messages: MessageArgs,
}

#[derive(Args)]
#[command(group(
    clap::ArgGroup::new("value")
        .required(true)
        .multiple(false)
        .args(["json", "text", "json_file", "text_file"])
))]
pub struct SessionStoreArgs {
    #[arg(long, value_name = "KEY", required = true)]
    pub key: String,
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub text: Option<String>,
    #[arg(long = "json-file", value_name = "FILE")]
    pub json_file: Option<PathBuf>,
    #[arg(long = "text-file", value_name = "FILE")]
    pub text_file: Option<PathBuf>,
    #[arg(value_name = "SESSION", help = "Session file, or - for stdin/stdout")]
    target: SessionTarget,
}

#[derive(Args)]
pub struct SessionRemoveArgs {
    #[arg(long, value_name = "KEY", required = true)]
    pub key: String,
    #[arg(value_name = "SESSION", help = "Session file, or - for stdin/stdout")]
    target: SessionTarget,
}

#[derive(Args)]
pub struct SessionRollbackArgs {
    #[arg(long, value_name = "STEP", help = "Rollback to the nearest frame of this step kind")]
    pub to: Option<SessionStepKind>,
    #[arg(value_name = "SESSION", help = "Session file, or - for stdin/stdout")]
    target: SessionTarget,
}

#[derive(Args)]
pub struct SessionStepTransformArgs {
    #[command(flatten)]
    config: crate::ConfigArgs,
    #[arg(value_name = "SESSION", help = "Session file, or - for stdin/stdout")]
    target: SessionTarget,
    #[arg(long, required = true)]
    pub provider: bool,
}

#[derive(Args)]
#[command(group(
    clap::ArgGroup::new("result")
        .required(true)
        .multiple(false)
        .args(["json", "text", "json_file", "text_file"])
))]
pub struct SessionToolResultArgs {
    #[command(flatten)]
    config: crate::ConfigArgs,
    #[arg(value_name = "SESSION", help = "Session file, or - for stdin/stdout")]
    target: SessionTarget,
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    #[arg(long, value_name = "TEXT")]
    pub text: Option<String>,
    #[arg(long = "json-file", value_name = "FILE")]
    pub json_file: Option<PathBuf>,
    #[arg(long = "text-file", value_name = "FILE")]
    pub text_file: Option<PathBuf>,
    #[arg(long = "no-sanitize")]
    pub no_sanitize: bool,
}

#[derive(Args)]
pub struct SessionStateArgs {
    #[arg(value_name = "SESSION", help = "Session file, or - for stdin")]
    target: SessionTarget,
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
        SessionCommand::Store(args) => store(args),
        SessionCommand::Remove(args) => remove(args),
        SessionCommand::Next(args) => next(home_spec, args),
        SessionCommand::Replace(args) => replace(home_spec, args),
        SessionCommand::ToolResult(args) => tool_result(home_spec, args),
        SessionCommand::State(args) => state(args),
        SessionCommand::Rollback(args) => rollback(args),
        SessionCommand::History(args) => history(args),
        SessionCommand::Delete(args) => delete(args).await,
    }
}

fn state(args: SessionStateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = args.target.load()?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &session.state()?)?;
    use std::io::Write;
    handle.write_all(b"\n")?;
    Ok(())
}

fn append(args: SessionAppendArgs) -> Result<(), Box<dyn std::error::Error>> {
    transform(args.target, |session| {
        Ok(session.append_messages(args.messages.messages()))
    })
}

fn store(args: SessionStoreArgs) -> Result<(), Box<dyn std::error::Error>> {
    let value = parse_stored_value(args.json, args.text, args.json_file, args.text_file)?;
    transform(args.target, |session| {
        session.store_json(args.key, value).map_err(Into::into)
    })
}

fn remove(args: SessionRemoveArgs) -> Result<(), Box<dyn std::error::Error>> {
    transform(args.target, |session| {
        session.remove_key(args.key).map_err(Into::into)
    })
}

fn parse_stored_value(
    json: Option<String>,
    text: Option<String>,
    json_file: Option<PathBuf>,
    text_file: Option<PathBuf>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    match (json, text, json_file, text_file) {
        (Some(json), None, None, None) => serde_json::from_str(&json)
            .map_err(|error| format!("invalid --json value: {error}").into()),
        (None, Some(text), None, None) => Ok(serde_json::Value::String(text)),
        (None, None, Some(path), None) => {
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("failed to read --json-file {}: {error}", path.display()))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid JSON in --json-file {}: {error}", path.display()).into())
        }
        (None, None, None, Some(path)) => Ok(serde_json::Value::String(
            std::fs::read_to_string(&path)
                .map_err(|error| format!("failed to read --text-file {}: {error}", path.display()))?,
        )),
        _ => unreachable!("clap requires exactly one stored value input"),
    }
}

fn next(
    home_spec: Option<&str>,
    args: SessionStepTransformArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let step = provider_step(home_spec, args.config.config.as_deref(), args.provider)?;
    transform(args.target, |session| {
        session.next(step).map_err(Into::into)
    })
}

fn replace(
    home_spec: Option<&str>,
    args: SessionStepTransformArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let step = provider_step(home_spec, args.config.config.as_deref(), args.provider)?;
    transform(args.target, |session| {
        session.replace(step).map_err(Into::into)
    })
}

fn tool_result(
    home_spec: Option<&str>,
    args: SessionToolResultArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = args.config.config.as_deref();
    let home = crate::home::load_home(home_spec)?;
    let config = crate::load_config(home.as_ref(), config_path)?;
    let result = match (args.json, args.text, args.json_file, args.text_file) {
        (Some(json), None, None, None) => serde_json::from_str(&json)
            .map_err(|error| format!("invalid --json tool result: {error}"))?,
        (None, Some(text), None, None) => serde_json::Value::String(text),
        (None, None, Some(path), None) => {
            let bytes = std::fs::read(&path).map_err(|error| {
                format!("failed to read --json-file {}: {error}", path.display())
            })?;
            serde_json::from_slice(&bytes).map_err(|error| {
                format!("invalid JSON in --json-file {}: {error}", path.display())
            })?
        }
        (None, None, None, Some(path)) => {
            let text = std::fs::read_to_string(&path).map_err(|error| {
                format!("failed to read --text-file {}: {error}", path.display())
            })?;
            serde_json::Value::String(text)
        }
        _ => unreachable!("clap requires exactly one tool result input"),
    };
    let result = if args.no_sanitize {
        result
    } else {
        crate::executor::sanitizer::sanitize_json_string_leaves(
            result,
            config.executor().tool_threshold_tokens,
            config.executor().tool_preview_bytes,
        )
    };
    let resume_call = provider_call(home_spec, config_path)?;
    transform(args.target, |session| {
        session
            .insert_tool_result(result, resume_call)
            .map_err(Into::into)
    })
}

fn provider_step(
    home_spec: Option<&str>,
    config_path: Option<&Path>,
    provider: bool,
) -> Result<PhiReActStep, Box<dyn std::error::Error>> {
    if !provider {
        return Err("a session step kind is required".into());
    }
    Ok(PhiReActStep::request_provider_with_call(
        "ready",
        provider_call(home_spec, config_path)?,
    ))
}

fn provider_call(
    home_spec: Option<&str>,
    config_path: Option<&Path>,
) -> Result<PhiProviderCall, Box<dyn std::error::Error>> {
    let home = crate::home::load_home(home_spec)?;
    let config = crate::load_config(home.as_ref(), config_path)?;
    let defaults = ModelRequestDefaults::from(&config);
    Ok(PhiProviderCall::from_parts(&defaults, Vec::new()))
}

fn rollback(args: SessionRollbackArgs) -> Result<(), Box<dyn std::error::Error>> {
    transform(args.target, |session| match args.to {
        Some(kind) => Session::rollback_to(session, kind).map_err(Into::into),
        None => Ok(Session::rollback(session)),
    })
}

fn transform(
    target: SessionTarget,
    operation: impl FnOnce(Session) -> Result<Session, Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let session = target.load()?;
    let session = operation(session)?;
    target.persist(&session)
}

impl FromArgMatches for SessionAppendArgs {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let mut matches = matches.clone();
        Self::from_arg_matches_mut(&mut matches)
    }

    fn from_arg_matches_mut(matches: &mut ArgMatches) -> Result<Self, Error> {
        Ok(Self {
            target: matches
                .remove_one("target")
                .expect("clap requires a session target"),
            messages: MessageArgs::parse(matches)
                .map_err(|error| Error::raw(clap::error::ErrorKind::InvalidValue, error))?,
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
                Arg::new("target")
                    .value_name("SESSION")
                    .help("Session file, or - for stdin/stdout")
                    .required(true)
                    .value_parser(clap::value_parser!(SessionTarget)),
            ),
        )
    }

    fn augment_args_for_update(command: Command) -> Command {
        Self::augment_args(command)
    }
}

fn new(home_spec: Option<&str>, args: SessionNewArgs) -> Result<(), Box<dyn std::error::Error>> {
    let home = crate::home::load_home(home_spec)?;
    let config = crate::load_config(home.as_ref(), args.config.config.as_deref())?;
    let session = crate::new_session_with_config(&config)?;
    args.target.create(&session)
}

fn history(args: SessionHistoryArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = args.target.load()?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    use std::io::Write;
    if args.view {
        let history = render_history(&session);
        if !history.is_empty() {
            handle.write_all(history.as_bytes())?;
        }
    } else {
        serde_json::to_writer(&mut handle, &session.history())?;
    }
    handle.write_all(b"\n")?;
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
        let PhiMessage::ToolResult(result) = message else {
            continue;
        };
        if !matches!(
            result.result["value"]["status"].as_str(),
            Some(
                "running"
                    | "running_output_settled"
                    | "running_screen_sampled"
                    | "running_wait_elapsed"
            )
        ) {
            continue;
        }
        let Some(handle) = result.result["value"]["handle"].as_str() else {
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

    #[test]
    fn history_serializes_the_committed_phi_history() {
        let session = Session::from_root(
            PhiAgentStep::turn_end("done"),
            vec![PhiMessage::user("hello")],
        );
        assert_eq!(
            serde_json::to_value(session.history()).unwrap(),
            serde_json::json!([{ "role": "user", "content": "hello" }])
        );
    }
}
