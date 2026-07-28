//! Phi library entrypoint.
//!
//! The core Phi concept is that a full agent session can flow through
//! stdin/stdout as JSON. That makes `phi run` usable both as an
//! interactive command and as a process in a session pipeline:
//!
//! - stdin: optional prior session JSON
//! - stdout: updated session JSON
//!
//! This allows callers to chain steps together without introducing a separate
//! session service or hidden local state.

pub mod agent;
mod banner;
pub mod config;
pub mod container;
pub mod error;
pub mod executor;
pub(crate) mod expr;
pub mod features;
pub mod home;
pub mod message;
pub mod module;
pub mod probe;
pub mod render;
pub mod session;
pub mod utils;

#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Read},
    path::Path,
    path::PathBuf,
};

use clap::{
    Arg, ArgAction, ArgMatches, Args, Command as ClapCommand, Error as ClapError, FromArgMatches,
    Parser, Subcommand,
};

use agent::{
    PhiAgentCommand, build_agent, run_agent_steps, run_single_agent_step, yolo_agent_steps,
};
use features::pretty_info;
use home::{command::HomeArgs, load_home};
use message::PhiMessage;
use session::Session;

enum SessionInput {
    Pipeline(Session),
    FileBacked {
        path: PathBuf,
        session: Session,
        stdin_user_message: Option<String>,
        loaded_existing: bool,
    },
}

#[derive(Debug)]
pub struct ReportedCliError {
    source: Box<dyn std::error::Error>,
}

impl ReportedCliError {
    fn new(source: Box<dyn std::error::Error>) -> Self {
        Self { source }
    }
}

impl std::fmt::Display for ReportedCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for ReportedCliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn finalize_agent_outcome(
    error: Option<crate::error::PhiRuntimeError>,
    quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match error {
        Some(error) if quiet => Err(Box::new(error)),
        Some(error) => Err(Box::new(ReportedCliError::new(Box::new(error)))),
        None => Ok(()),
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())?;

    Ok(())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Container(args) => container::run(args).await.map_err(Into::into),
        Command::Run(args) => run_agent(cli.home.as_deref(), args).await,
        Command::Yolo(args) => yolo_agent(cli.home.as_deref(), args).await,
        Command::Step(args) => step_agent(cli.home.as_deref(), args).await,
        Command::Doctor(args) => doctor_runtime(cli.home.as_deref(), args),
        Command::Session(args) => session::command::run(args).await,
        Command::Probe(args) => probe_session_command(cli.home.as_deref(), args),
        Command::Home(args) => home::command::run(args),
    }
}

async fn run_agent(
    home_spec: Option<&str>,
    args: RunArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    run_agent_with_step_limit(home_spec, args, None).await
}

async fn yolo_agent(
    home_spec: Option<&str>,
    args: RunArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    yolo_agent_with_step_limit(home_spec, args, None).await
}

async fn step_agent(
    home_spec: Option<&str>,
    args: StepArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let home = load_home(home_spec)?;
    let session_input = read_session_input(args.base.session_path.as_deref())?;
    emit_existing_session_notice(&session_input, args.base.quiet);
    let session = session_input.session().clone();
    let input_messages = collect_effective_input_messages(&args.base, &session_input);

    if input_messages.is_empty() {
        let outcome = run_single_agent_step(
            session,
            PhiAgentCommand::try_from(agent::StepCommandInput {
                args: agent::StepCommandArgs::from(&args),
                input_messages: Vec::new(),
            })?,
            home.clone(),
        )
        .await?;
        persist_outcome_session(&outcome.session, &session_input)?;
        return finalize_agent_outcome(outcome.error, args.base.quiet);
    }
    let outcome = run_single_agent_step(
        session,
        PhiAgentCommand::try_from(agent::StepCommandInput {
            args: agent::StepCommandArgs::from(&args),
            input_messages,
        })?,
        home,
    )
    .await?;
    persist_outcome_session(&outcome.session, &session_input)?;
    finalize_agent_outcome(outcome.error, args.base.quiet)
}

async fn run_agent_with_step_limit(
    home_spec: Option<&str>,
    args: RunArgs,
    forced_max_steps: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let home = load_home(home_spec)?;
    let session_input = read_session_input(args.base.session_path.as_deref())?;
    emit_existing_session_notice(&session_input, args.base.quiet);
    let session = session_input.session().clone();
    let max_steps = forced_max_steps.or(args.max_steps);
    let input_messages = collect_effective_input_messages(&args.base, &session_input);

    if input_messages.is_empty() {
        let outcome = run_agent_steps(
            session,
            PhiAgentCommand::try_from(agent::RunCommandInput {
                args: agent::RunCommandArgs::from(&args),
                forced_max_steps: max_steps,
                input_messages: Vec::new(),
            })?,
            home.clone(),
        )
        .await?;
        persist_outcome_session(&outcome.session, &session_input)?;
        return finalize_agent_outcome(outcome.error, args.base.quiet);
    }
    let outcome = run_agent_steps(
        session,
        PhiAgentCommand::try_from(agent::RunCommandInput {
            args: agent::RunCommandArgs::from(&args),
            forced_max_steps: max_steps,
            input_messages,
        })?,
        home,
    )
    .await?;
    persist_outcome_session(&outcome.session, &session_input)?;
    finalize_agent_outcome(outcome.error, args.base.quiet)
}

async fn yolo_agent_with_step_limit(
    home_spec: Option<&str>,
    args: RunArgs,
    forced_max_steps: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let home = load_home(home_spec)?;
    let session_input = read_session_input(args.base.session_path.as_deref())?;
    emit_existing_session_notice(&session_input, args.base.quiet);
    let session = session_input.session().clone();
    let max_steps = forced_max_steps.or(args.max_steps);
    let input_messages = collect_effective_input_messages(&args.base, &session_input);

    if input_messages.is_empty() {
        let outcome = yolo_agent_steps(
            session,
            PhiAgentCommand::try_from(agent::YoloCommandInput {
                args: agent::RunCommandArgs::from(&args),
                forced_max_steps: max_steps,
                input_messages: Vec::new(),
            })?,
            home.clone(),
        )
        .await?;
        persist_outcome_session(&outcome.session, &session_input)?;
        return finalize_agent_outcome(outcome.error, args.base.quiet);
    }
    let outcome = yolo_agent_steps(
        session,
        PhiAgentCommand::try_from(agent::YoloCommandInput {
            args: agent::RunCommandArgs::from(&args),
            forced_max_steps: max_steps,
            input_messages,
        })?,
        home,
    )
    .await?;
    persist_outcome_session(&outcome.session, &session_input)?;
    finalize_agent_outcome(outcome.error, args.base.quiet)
}

fn probe_session_command(
    home_spec: Option<&str>,
    args: ProbeArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let home = load_home(home_spec)?;
    let session = read_probe_session(args.session_path.as_deref())?;
    let agent = build_agent(session, PhiAgentCommand::from_probe_args(&args)?, home)?;
    let probe = agent.probe_report();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &probe)?;
    use std::io::Write;
    handle.write_all(b"\n")?;
    Ok(())
}

fn doctor_runtime(
    home_spec: Option<&str>,
    _args: DoctorArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let home = load_home(home_spec)?;
    let agent = build_agent(
        Session::empty(),
        PhiAgentCommand::Doctor(PhiAgentCommand::doctor()),
        home,
    )?;
    let report = agent.doctor_report();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &report)?;
    use std::io::Write;
    handle.write_all(b"\n")?;
    Ok(())
}

impl SessionInput {
    fn session(&self) -> &Session {
        match self {
            Self::Pipeline(session) => session,
            Self::FileBacked { session, .. } => session,
        }
    }

    fn existing_session_path(&self) -> Option<&Path> {
        match self {
            Self::Pipeline(_) => None,
            Self::FileBacked {
                path,
                loaded_existing: true,
                ..
            } => Some(path.as_path()),
            Self::FileBacked { .. } => None,
        }
    }
}

// Without [SESSION], Phi reads/writes session JSON through stdin/stdout so
// sessions can be piped directly across process boundaries. With [SESSION], Phi
// treats the path as the persistent backing store and stdin, when present, is
// interpreted as a plain user message instead.
fn read_session_input(path: Option<&Path>) -> Result<SessionInput, Box<dyn std::error::Error>> {
    match path {
        Some(path) => {
            let loaded_existing = path.exists();
            let session = if loaded_existing {
                Session::load(path)?
            } else {
                Session::empty()
            };
            let stdin_user_message = read_stdin_user_message()?;
            Ok(SessionInput::FileBacked {
                path: path.to_path_buf(),
                session,
                stdin_user_message,
                loaded_existing,
            })
        }
        None if io::stdin().is_terminal() => Ok(SessionInput::Pipeline(Session::empty())),
        None => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input)?;

            if input.iter().all(|byte| byte.is_ascii_whitespace()) {
                Ok(SessionInput::Pipeline(Session::empty()))
            } else {
                Ok(SessionInput::Pipeline(Session::load_bytes(&input)?))
            }
        }
    }
}

fn read_probe_session(path: Option<&Path>) -> Result<Session, Box<dyn std::error::Error>> {
    match path {
        Some(path) => Session::load(path),
        None if io::stdin().is_terminal() => Ok(Session::empty()),
        None => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input)?;
            if input.iter().all(|byte| byte.is_ascii_whitespace()) {
                Ok(Session::empty())
            } else {
                Session::load_bytes(&input)
            }
        }
    }
}

fn existing_session_notice(session_input: &SessionInput) -> Option<String> {
    session_input
        .existing_session_path()
        .map(|path| format!("resuming from existing session file: {}", path.display()))
}

fn emit_existing_session_notice(session_input: &SessionInput, quiet: bool) {
    if quiet {
        return;
    }

    if let Some(message) = existing_session_notice(session_input) {
        eprintln!("{}", pretty_info(&message));
    }
}

fn read_stdin_user_message() -> Result<Option<String>, Box<dyn std::error::Error>> {
    if io::stdin().is_terminal() {
        return Ok(None);
    }

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let stripped = input.trim();
    if stripped.is_empty() {
        Ok(None)
    } else {
        Ok(Some(stripped.to_string()))
    }
}

fn collect_effective_input_messages(
    args: &AgentCliArgs,
    session_input: &SessionInput,
) -> Vec<PhiMessage> {
    let mut messages = Vec::new();

    if let SessionInput::FileBacked {
        stdin_user_message: Some(text),
        ..
    } = session_input
    {
        messages.push(PhiMessage::user(text.clone()));
    }

    messages.extend(
        args.input_messages
            .iter()
            .cloned()
            .map(InputMessage::into_message),
    );
    messages
}

fn persist_outcome_session(
    session: &Session,
    session_input: &SessionInput,
) -> Result<(), Box<dyn std::error::Error>> {
    match session_input {
        SessionInput::Pipeline(_) => session.write_stdout(),
        SessionInput::FileBacked { path, .. } => session.save(path),
    }
}

#[derive(Parser)]
#[command(
    name = "phi",
    about = "Literally A CLI Agent",
    before_help = banner::startup_banner()
)]
struct Cli {
    // Home loading is a CLI concern rather than an agent-builder side effect.
    // The selected spec is resolved once into a concrete PhiHome instance and
    // then handed to every command that needs an initialized agent.
    #[arg(long = "home", global = true, value_name = "HOME")]
    home: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Repeat the primitive `step` evaluation with more autonomy, continuing through recoverable failures until Completed",
        before_help = banner::startup_banner()
    )]
    Yolo(RunArgs),
    #[command(
        about = "Repeat the primitive `step` evaluation until the session first reaches a terminal boundary",
        before_help = banner::startup_banner()
    )]
    Run(RunArgs),
    #[command(
        about = "Advance the session by exactly one atomic step; this is the core evaluation unit behind `run` and `yolo`",
        before_help = banner::startup_banner()
    )]
    Step(StepArgs),
    #[command(
        about = "Inspect a session's current eval-state and governance status as JSON",
        before_help = banner::startup_banner()
    )]
    Probe(ProbeArgs),
    #[command(
        about = "Manage session-backed history and transcript views",
        before_help = banner::startup_banner()
    )]
    Session(session::command::SessionArgs),
    #[command(
        about = "Manage local or packed Phi home directories",
        before_help = banner::startup_banner()
    )]
    Home(HomeArgs),
    #[command(
        about = "Report initialized runtime status, tools, and system prompt",
        before_help = banner::startup_banner()
    )]
    Doctor(DoctorArgs),
    Container(container::ContainerArgs),
}

#[derive(Args, Default)]
struct ProbeArgs {
    #[arg(value_name = "SESSION")]
    session_path: Option<PathBuf>,
    #[arg(long = "max-model-request-retries", value_name = "N")]
    max_model_request_retries: Option<usize>,
}

#[derive(Args, Default)]
struct DoctorArgs {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InputMessage {
    role: InputRole,
    text: String,
}

impl InputMessage {
    fn into_message(self) -> PhiMessage {
        match self.role {
            InputRole::User => PhiMessage::user(self.text),
            InputRole::Assistant => PhiMessage::assistant(self.text),
        }
    }
}

#[derive(Default)]
struct AgentCliArgs {
    session_path: Option<PathBuf>,
    quiet: bool,
    plugin_args: Vec<String>,
    input_messages: Vec<InputMessage>,
}

#[derive(Default)]
struct RunArgs {
    base: AgentCliArgs,
    max_steps: Option<usize>,
    max_model_request_retries: Option<usize>,
    template: Option<String>,
}

#[derive(Default)]
struct StepArgs {
    base: AgentCliArgs,
    max_model_request_retries: Option<usize>,
    template: Option<String>,
}

impl From<&RunArgs> for agent::RunCommandArgs {
    fn from(value: &RunArgs) -> Self {
        Self {
            quiet: value.base.quiet,
            max_steps: value.max_steps,
            max_model_request_retries: value.max_model_request_retries,
            template: value.template.clone(),
            plugin_args: value.base.plugin_args.clone(),
        }
    }
}

impl From<&StepArgs> for agent::StepCommandArgs {
    fn from(value: &StepArgs) -> Self {
        Self {
            quiet: value.base.quiet,
            max_model_request_retries: value.max_model_request_retries,
            template: value.template.clone(),
            plugin_args: value.base.plugin_args.clone(),
        }
    }
}

impl From<&ProbeArgs> for agent::ProbeCommandArgs {
    fn from(value: &ProbeArgs) -> Self {
        Self {
            max_model_request_retries: value
                .max_model_request_retries
                .or(PhiAgentCommand::probe().max_model_request_retries),
        }
    }
}

impl FromArgMatches for RunArgs {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, ClapError> {
        let mut matches = matches.clone();
        Self::from_arg_matches_mut(&mut matches)
    }

    fn from_arg_matches_mut(matches: &mut ArgMatches) -> Result<Self, ClapError> {
        Ok(Self {
            base: parse_agent_cli_args(matches),
            max_steps: matches.remove_one::<usize>("max_steps"),
            max_model_request_retries: matches.remove_one::<usize>("max_model_request_retries"),
            template: matches.remove_one::<String>("template"),
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), ClapError> {
        let mut matches = matches.clone();
        self.update_from_arg_matches_mut(&mut matches)
    }

    fn update_from_arg_matches_mut(&mut self, matches: &mut ArgMatches) -> Result<(), ClapError> {
        update_agent_cli_args(&mut self.base, matches);
        if let Some(max_steps) = matches.remove_one::<usize>("max_steps") {
            self.max_steps = Some(max_steps);
        }
        if let Some(max_model_request_retries) =
            matches.remove_one::<usize>("max_model_request_retries")
        {
            self.max_model_request_retries = Some(max_model_request_retries);
        }
        if let Some(template) = matches.remove_one::<String>("template") {
            self.template = Some(template);
        }
        Ok(())
    }
}

impl Args for RunArgs {
    fn augment_args(cmd: ClapCommand) -> ClapCommand {
        add_agent_cli_args(cmd)
            .arg(
                Arg::new("max_steps")
                    .long("max-steps")
                    .value_name("N")
                    .value_parser(clap::value_parser!(usize)),
            )
            .arg(
                Arg::new("max_model_request_retries")
                    .long("max-model-request-retries")
                    .value_name("N")
                    .value_parser(clap::value_parser!(usize)),
            )
            .arg(Arg::new("template").long("template").value_name("NAME"))
            .arg(
                Arg::new("plugin_args")
                    .raw(true)
                    .num_args(0..)
                    .value_name("PLUGIN_ARGS"),
            )
    }

    fn augment_args_for_update(cmd: ClapCommand) -> ClapCommand {
        Self::augment_args(cmd)
    }
}

impl FromArgMatches for StepArgs {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, ClapError> {
        let mut matches = matches.clone();
        Self::from_arg_matches_mut(&mut matches)
    }

    fn from_arg_matches_mut(matches: &mut ArgMatches) -> Result<Self, ClapError> {
        Ok(Self {
            base: parse_agent_cli_args(matches),
            max_model_request_retries: matches.remove_one::<usize>("max_model_request_retries"),
            template: matches.remove_one::<String>("template"),
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), ClapError> {
        let mut matches = matches.clone();
        self.update_from_arg_matches_mut(&mut matches)
    }

    fn update_from_arg_matches_mut(&mut self, matches: &mut ArgMatches) -> Result<(), ClapError> {
        update_agent_cli_args(&mut self.base, matches);
        if let Some(max_model_request_retries) =
            matches.remove_one::<usize>("max_model_request_retries")
        {
            self.max_model_request_retries = Some(max_model_request_retries);
        }
        if let Some(template) = matches.remove_one::<String>("template") {
            self.template = Some(template);
        }
        Ok(())
    }
}

impl Args for StepArgs {
    fn augment_args(cmd: ClapCommand) -> ClapCommand {
        add_agent_cli_args(cmd)
            .arg(
                Arg::new("max_model_request_retries")
                    .long("max-model-request-retries")
                    .value_name("N")
                    .value_parser(clap::value_parser!(usize)),
            )
            .arg(Arg::new("template").long("template").value_name("NAME"))
            .arg(
                Arg::new("plugin_args")
                    .raw(true)
                    .num_args(0..)
                    .value_name("PLUGIN_ARGS"),
            )
    }

    fn augment_args_for_update(cmd: ClapCommand) -> ClapCommand {
        Self::augment_args(cmd)
    }
}

fn parse_input_messages(matches: &ArgMatches) -> Vec<InputMessage> {
    let mut ordered = BTreeMap::new();
    collect_messages(matches, "user", InputRole::User, &mut ordered);
    collect_messages(matches, "assistant", InputRole::Assistant, &mut ordered);
    ordered.into_values().collect()
}

fn parse_agent_cli_args(matches: &mut ArgMatches) -> AgentCliArgs {
    AgentCliArgs {
        session_path: matches.remove_one::<PathBuf>("session_path"),
        quiet: matches.get_flag("quiet"),
        plugin_args: matches
            .remove_many::<String>("plugin_args")
            .map(|values| values.collect())
            .unwrap_or_default(),
        input_messages: parse_input_messages(matches),
    }
}

fn update_agent_cli_args(target: &mut AgentCliArgs, matches: &mut ArgMatches) {
    if let Some(session_path) = matches.remove_one::<PathBuf>("session_path") {
        target.session_path = Some(session_path);
    }
    target.quiet |= matches.get_flag("quiet");
    target.plugin_args.extend(
        matches
            .remove_many::<String>("plugin_args")
            .map(|values| values.collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    target.input_messages.extend(parse_input_messages(matches));
}

fn add_agent_cli_args(cmd: ClapCommand) -> ClapCommand {
    cmd.arg(
        Arg::new("session_path")
            .value_name("SESSION")
            .value_parser(clap::value_parser!(PathBuf)),
    )
    .arg(
        Arg::new("quiet")
            .long("quiet")
            .action(ArgAction::SetTrue)
            .help("Disable human-readable stderr logs"),
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

fn collect_messages(
    matches: &ArgMatches,
    id: &str,
    role: InputRole,
    ordered: &mut BTreeMap<usize, InputMessage>,
) {
    let Some(values) = matches.get_many::<String>(id) else {
        return;
    };
    let indices = matches
        .indices_of(id)
        .expect("values are present, so indices should exist");

    for (text, index) in values.zip(indices) {
        ordered.insert(
            index,
            InputMessage {
                role,
                text: text.clone(),
            },
        );
    }
}
