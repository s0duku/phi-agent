//! Phi library entrypoint.
//!
//! The core Phi concept is that a full agent session can flow through
//! stdin/stdout as JSON through an explicit `-` target. That makes `phi run` usable both as an
//! interactive command and as a process in a session pipeline:
//!
//! - stdin: prior session JSON when SESSION is `-`
//! - stdout: updated session JSON
//!
//! This allows callers to chain steps together without introducing a separate
//! session service or hidden local state.

pub mod agent;
mod banner;
mod cli;
pub mod config;
pub mod error;
pub mod executor;
pub(crate) mod expr;
pub mod features;
pub mod headlessterm;
pub mod home;
pub mod message;
pub mod module;
pub mod render;
pub mod session;
pub mod utils;

#[cfg(test)]
mod tests;

use std::{
    any::Any,
    future::{Future, poll_fn},
    io,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    path::Path,
    path::PathBuf,
    task::Poll,
};

use clap::{
    Arg, ArgAction, ArgMatches, Args, Command as ClapCommand, Error as ClapError, FromArgMatches,
    Parser, Subcommand,
};

use agent::{PhiAgent, PhiAgentCommand, build_agent_with_config};
use config::PhiConfig;
use features::{pretty_info, pretty_warning};
use home::{command::HomeArgs, load_home};
use message::PhiMessage;
use session::{PhiAgentStep, PhiReActStep, Session, SessionTarget};

enum CliAgentExit {
    Completed,
    Interrupted,
    InterruptedAfterStep,
    Panicked(Box<dyn Any + Send>),
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
    error: Option<crate::error::PhiAgentRuntimeError>,
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
        Command::HeadlessTerminal(args) => headlessterm::run(args).await.map_err(Into::into),
        Command::Run(args) => run_agent(cli.home.as_deref(), args).await,
        Command::Yolo(args) => yolo_agent(cli.home.as_deref(), args).await,
        Command::Step(args) => step_agent(cli.home.as_deref(), args).await,
        Command::Doctor(args) => doctor_runtime(cli.home.as_deref(), args),
        Command::Session(args) => session::command::run(cli.home.as_deref(), args).await,
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
    let config_path = args.base.config.as_deref();
    let session = args.base.session_target.load()?;
    let input_messages = args.base.messages.messages();
    let home = load_home(home_spec)?;
    let config = load_config(home.as_ref(), config_path)?;
    run_step_with_input(args, input_messages, session, home, config).await
}

async fn run_step_with_input(
    args: StepArgs,
    input_messages: Vec<PhiMessage>,
    session: Session,
    home: std::sync::Arc<dyn home::PhiHome>,
    config: PhiConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    emit_existing_session_notice(&args.base.session_target, args.base.quiet);
    emit_input_messages(&input_messages, args.base.quiet);
    let session = session.append_messages(input_messages);
    let (session, exit) = run_cli_agent(
        session,
        PhiAgentCommand::try_from(agent::StepCommandInput {
            args: agent::StepCommandArgs::from(&args.base),
        })?,
        home,
        config,
        |session| args.base.session_target.checkpoint(session),
    )
    .await?;
    persist_cli_agent_session(session, exit, &args.base.session_target, args.base.quiet)
}

async fn run_agent_with_step_limit(
    home_spec: Option<&str>,
    args: RunArgs,
    forced_max_steps: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = args.base.config.as_deref();
    let session = args.base.session_target.load()?;
    let input_messages = args.base.messages.messages();
    let home = load_home(home_spec)?;
    let config = load_config(home.as_ref(), config_path)?;
    run_with_input(
        args,
        forced_max_steps,
        input_messages,
        session,
        home,
        config,
    )
    .await
}

async fn run_with_input(
    args: RunArgs,
    forced_max_steps: Option<usize>,
    input_messages: Vec<PhiMessage>,
    session: Session,
    home: std::sync::Arc<dyn home::PhiHome>,
    config: PhiConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    emit_existing_session_notice(&args.base.session_target, args.base.quiet);
    emit_input_messages(&input_messages, args.base.quiet);
    let session = session.append_messages(input_messages);
    let max_steps = forced_max_steps.or(args.max_steps);
    let (session, exit) = run_cli_agent(
        session,
        PhiAgentCommand::try_from(agent::RunCommandInput {
            args: agent::RunCommandArgs::from(&args),
            forced_max_steps: max_steps,
        })?,
        home,
        config,
        |session| args.base.session_target.checkpoint(session),
    )
    .await?;
    persist_cli_agent_session(session, exit, &args.base.session_target, args.base.quiet)
}

async fn yolo_agent_with_step_limit(
    home_spec: Option<&str>,
    args: RunArgs,
    forced_max_steps: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = args.base.config.as_deref();
    let session = args.base.session_target.load()?;
    let input_messages = args.base.messages.messages();
    let home = load_home(home_spec)?;
    let config = load_config(home.as_ref(), config_path)?;
    yolo_with_input(
        args,
        forced_max_steps,
        input_messages,
        session,
        home,
        config,
    )
    .await
}

async fn yolo_with_input(
    args: RunArgs,
    forced_max_steps: Option<usize>,
    input_messages: Vec<PhiMessage>,
    session: Session,
    home: std::sync::Arc<dyn home::PhiHome>,
    config: PhiConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    emit_existing_session_notice(&args.base.session_target, args.base.quiet);
    emit_input_messages(&input_messages, args.base.quiet);
    let session = session.append_messages(input_messages);
    let max_steps = forced_max_steps.or(args.max_steps);
    let (session, exit) = run_cli_agent(
        session,
        PhiAgentCommand::try_from(agent::YoloCommandInput {
            args: agent::RunCommandArgs::from(&args),
            forced_max_steps: max_steps,
        })?,
        home,
        config,
        |session| args.base.session_target.checkpoint(session),
    )
    .await?;
    persist_cli_agent_session(session, exit, &args.base.session_target, args.base.quiet)
}

async fn run_cli_agent(
    session: Session,
    command: PhiAgentCommand,
    home: std::sync::Arc<dyn home::PhiHome>,
    config: PhiConfig,
    persist_checkpoint: impl FnMut(&Session) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(Session, CliAgentExit), Box<dyn std::error::Error>> {
    let step_once = matches!(&command, PhiAgentCommand::Step(_));
    let yolo = matches!(&command, PhiAgentCommand::Yolo(_));
    let agent = build_agent_with_config(session, command, home, config)?;
    run_built_cli_agent(agent, step_once, yolo, persist_checkpoint).await
}

async fn run_built_cli_agent(
    mut agent: PhiAgent,
    step_once: bool,
    yolo: bool,
    mut persist_checkpoint: impl FnMut(&Session) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(Session, CliAgentExit), Box<dyn std::error::Error>> {
    persist_checkpoint(&agent.session())?;
    let mut previous_was_failed = false;
    loop {
        let checkpoint = agent.session();
        match step_or_ctrl_c(&mut agent).await? {
            CliAgentExit::Completed => persist_checkpoint(&agent.session())?,
            CliAgentExit::InterruptedAfterStep => {
                persist_checkpoint(&agent.session())?;
                return Ok((agent.into_session(), CliAgentExit::InterruptedAfterStep));
            }
            exit => return Ok((checkpoint, exit)),
        }
        let session = agent.session();
        let completed = if step_once {
            true
        } else if yolo {
            match session.step() {
                PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. }) => true,
                PhiAgentStep::Failed(_) if previous_was_failed => true,
                PhiAgentStep::Failed(_) => {
                    previous_was_failed = true;
                    false
                }
                _ => {
                    previous_was_failed = false;
                    false
                }
            }
        } else {
            session.step().is_terminal()
                || matches!(
                    session.step(),
                    PhiAgentStep::ReAct(PhiReActStep::RequestCompact { .. })
                )
        };
        if completed {
            return Ok((agent.into_session(), CliAgentExit::Completed));
        }
    }
}

async fn step_or_ctrl_c(agent: &mut PhiAgent) -> Result<CliAgentExit, std::io::Error> {
    step_or_interrupt(agent, tokio::signal::ctrl_c()).await
}

async fn step_or_interrupt<I>(
    agent: &mut PhiAgent,
    interrupt: I,
) -> Result<CliAgentExit, std::io::Error>
where
    I: std::future::Future<Output = Result<(), std::io::Error>>,
{
    let cancellation = agent.cancellation();
    let mut step = Box::pin(catch_future_unwind(agent.step()));
    let outcome = tokio::select! {
        biased;
        signal = interrupt => {
            signal?;
            cancellation.cancel();
            if !cancellation.should_commit_current_step() {
                return Ok(CliAgentExit::Interrupted);
            }
            step.await.map(|()| CliAgentExit::InterruptedAfterStep)
        }
        outcome = &mut step => outcome.map(|()| CliAgentExit::Completed),
    };
    Ok(outcome.unwrap_or_else(CliAgentExit::Panicked))
}

async fn catch_future_unwind<F>(future: F) -> Result<F::Output, Box<dyn Any + Send>>
where
    F: Future,
{
    let mut future = Box::pin(future);
    poll_fn(
        move |context| match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Err(payload) => Poll::Ready(Err(payload)),
        },
    )
    .await
}

fn resume_panic_after_persist_failure(
    payload: Box<dyn Any + Send>,
    error: Box<dyn std::error::Error>,
) -> ! {
    eprintln!("failed to persist the last committed session after panic: {error}");
    resume_unwind(payload)
}

fn persist_panicked_session(
    session: &Session,
    session_target: &SessionTarget,
    quiet: bool,
    payload: Box<dyn Any + Send>,
) -> ! {
    if let Err(error) = session_target.persist(session) {
        resume_panic_after_persist_failure(payload, error);
    }
    if !quiet {
        eprintln!(
            "{}",
            pretty_warning("panic; persisted the last committed session state")
        );
    }
    resume_unwind(payload)
}

fn persist_cli_agent_session(
    session: Session,
    exit: CliAgentExit,
    session_target: &SessionTarget,
    quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let CliAgentExit::Panicked(payload) = exit {
        persist_panicked_session(&session, session_target, quiet, payload);
    }
    session_target.persist(&session)?;
    if matches!(
        exit,
        CliAgentExit::Interrupted | CliAgentExit::InterruptedAfterStep
    ) {
        if !quiet {
            eprintln!(
                "{}",
                pretty_warning("interrupted; persisted the last committed session state")
            );
        }
        return Ok(());
    }
    finalize_agent_outcome(session.step().error().cloned(), quiet)
}

fn doctor_runtime(
    home_spec: Option<&str>,
    args: DoctorArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let home = load_home(home_spec)?;
    let config = load_config(home.as_ref(), args.config.config.as_deref())?;
    let agent = build_agent_with_config(
        Session::empty(),
        PhiAgentCommand::Doctor(PhiAgentCommand::doctor()),
        home,
        config,
    )?;
    let report = agent.doctor_report();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &report)?;
    use std::io::Write;
    handle.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn new_session(home: &dyn home::PhiHome) -> Result<Session, Box<dyn std::error::Error>> {
    let config = load_config(home, None)?;
    new_session_with_config(&config)
}

pub(crate) fn load_config(
    home: &dyn home::PhiHome,
    explicit_path: Option<&Path>,
) -> Result<PhiConfig, Box<dyn std::error::Error>> {
    let config = if let Some(path) = explicit_path {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("failed to read phi config {}: {error}", path.display()))?;
        PhiConfig::from_yaml(&bytes)?
    } else {
        match home.read_file(&home.config()) {
            Ok(bytes) => PhiConfig::from_yaml(&bytes)?,
            Err(error) if error.is_not_found() => PhiConfig::default(),
            Err(error) => return Err(Box::new(error)),
        }
    };
    config.apply_process_env()
}

pub(crate) fn new_session_with_config(
    config: &PhiConfig,
) -> Result<Session, Box<dyn std::error::Error>> {
    let defaults = config::ModelRequestDefaults::from(config);
    let history = features::configured_system_prompt_from_config(config)
        .map(PhiMessage::system)
        .into_iter()
        .collect::<Vec<_>>();
    Ok(Session::from_root(
        PhiAgentStep::request_provider("ready", &defaults),
        history,
    ))
}

fn emit_input_messages(messages: &[PhiMessage], quiet: bool) {
    if quiet {
        return;
    }
    for message in messages {
        eprintln!("{}", features::pretty_message(message));
    }
}

fn existing_session_notice(session_target: &SessionTarget) -> Option<String> {
    session_target
        .file()
        .map(|path| format!("resuming from existing session file: {}", path.display()))
}

fn emit_existing_session_notice(session_target: &SessionTarget, quiet: bool) {
    if quiet {
        return;
    }

    if let Some(message) = existing_session_notice(session_target) {
        eprintln!("{}", pretty_info(&message));
    }
}

#[derive(Parser)]
#[command(
    name = "phi",
    version = env!("CARGO_PKG_VERSION"),
    about = "CLI-oriented Agent Runtime",
    before_help = banner::startup_banner(),
    after_help = concat!("Version: ", env!("CARGO_PKG_VERSION"))
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
        about = "Repeat the primitive `step` evaluation with more autonomy, continuing through recoverable failures until TurnEnd",
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
    #[command(name = "headlessterm")]
    HeadlessTerminal(headlessterm::HeadlessTerminalArgs),
}

#[derive(Args, Default)]
struct ConfigArgs {
    /// Use this YAML file instead of the config location provided by PhiHome.
    #[arg(long = "config", global = true, value_name = "FILE")]
    config: Option<PathBuf>,
}

#[derive(Args, Default)]
struct DoctorArgs {
    #[command(flatten)]
    config: ConfigArgs,
}

struct AgentCliArgs {
    session_target: SessionTarget,
    config: Option<PathBuf>,
    quiet: bool,
    null_executor: bool,
    max_model_request_retries: Option<usize>,
    container: Option<String>,
    runner: Option<String>,
    runner_args: Vec<String>,
    messages: cli::MessageArgs,
}

struct RunArgs {
    base: AgentCliArgs,
    max_steps: Option<usize>,
}

struct StepArgs {
    base: AgentCliArgs,
}

impl From<&RunArgs> for agent::RunCommandArgs {
    fn from(value: &RunArgs) -> Self {
        Self {
            options: agent::AgentCommandArgs::from(&value.base),
            max_steps: value.max_steps,
        }
    }
}

impl From<&AgentCliArgs> for agent::AgentCommandArgs {
    fn from(value: &AgentCliArgs) -> Self {
        Self {
            quiet: value.quiet,
            null_executor: value.null_executor,
            max_model_request_retries: value.max_model_request_retries,
            container: value.container.clone(),
            runner: value.runner.clone(),
            runner_args: value.runner_args.clone(),
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
            base: parse_agent_cli_args(matches)?,
            max_steps: matches.remove_one::<usize>("max_steps"),
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), ClapError> {
        let mut matches = matches.clone();
        self.update_from_arg_matches_mut(&mut matches)
    }

    fn update_from_arg_matches_mut(&mut self, matches: &mut ArgMatches) -> Result<(), ClapError> {
        update_agent_cli_args(&mut self.base, matches)?;
        if let Some(max_steps) = matches.remove_one::<usize>("max_steps") {
            self.max_steps = Some(max_steps);
        }
        Ok(())
    }
}

impl Args for RunArgs {
    fn augment_args(cmd: ClapCommand) -> ClapCommand {
        add_agent_cli_args(
            cmd.arg(
                Arg::new("max_steps")
                    .long("max-steps")
                    .value_name("N")
                    .value_parser(clap::value_parser!(usize)),
            ),
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
            base: parse_agent_cli_args(matches)?,
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), ClapError> {
        let mut matches = matches.clone();
        self.update_from_arg_matches_mut(&mut matches)
    }

    fn update_from_arg_matches_mut(&mut self, matches: &mut ArgMatches) -> Result<(), ClapError> {
        update_agent_cli_args(&mut self.base, matches)?;
        Ok(())
    }
}

impl Args for StepArgs {
    fn augment_args(cmd: ClapCommand) -> ClapCommand {
        add_agent_cli_args(cmd)
    }

    fn augment_args_for_update(cmd: ClapCommand) -> ClapCommand {
        Self::augment_args(cmd)
    }
}

fn parse_agent_cli_args(matches: &mut ArgMatches) -> Result<AgentCliArgs, ClapError> {
    Ok(AgentCliArgs {
        session_target: matches
            .remove_one::<SessionTarget>("session_target")
            .expect("clap requires a session target"),
        config: matches.remove_one::<PathBuf>("config"),
        quiet: matches.get_flag("quiet"),
        null_executor: matches.get_flag("null_executor"),
        max_model_request_retries: matches.remove_one::<usize>("max_model_request_retries"),
        container: matches.remove_one::<String>("container"),
        runner: matches.remove_one::<String>("runner"),
        runner_args: matches
            .remove_many::<String>("runner_arg")
            .map(Iterator::collect)
            .unwrap_or_default(),
        messages: cli::MessageArgs::parse(matches)
            .map_err(|error| ClapError::raw(clap::error::ErrorKind::InvalidValue, error))?,
    })
}

fn update_agent_cli_args(
    target: &mut AgentCliArgs,
    matches: &mut ArgMatches,
) -> Result<(), ClapError> {
    if let Some(session_target) = matches.remove_one::<SessionTarget>("session_target") {
        target.session_target = session_target;
    }
    if let Some(config) = matches.remove_one::<PathBuf>("config") {
        target.config = Some(config);
    }
    target.quiet |= matches.get_flag("quiet");
    target.null_executor |= matches.get_flag("null_executor");
    if let Some(max_model_request_retries) =
        matches.remove_one::<usize>("max_model_request_retries")
    {
        target.max_model_request_retries = Some(max_model_request_retries);
    }
    if let Some(container) = matches.remove_one::<String>("container") {
        target.container = Some(container);
    }
    if let Some(runner) = matches.remove_one::<String>("runner") {
        target.runner = Some(runner);
    }
    target.runner_args.extend(
        matches
            .remove_many::<String>("runner_arg")
            .map(Iterator::collect::<Vec<_>>)
            .unwrap_or_default(),
    );
    target
        .messages
        .extend_from_matches(matches)
        .map_err(|error| ClapError::raw(clap::error::ErrorKind::InvalidValue, error))?;
    Ok(())
}

fn add_agent_cli_args(cmd: ClapCommand) -> ClapCommand {
    cli::MessageArgs::augment(
        cmd.arg(
            Arg::new("session_target")
                .value_name("SESSION")
                .help("Session file, or - for stdin/stdout")
                .required(true)
                .value_parser(clap::value_parser!(SessionTarget)),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("FILE")
                .value_parser(clap::value_parser!(PathBuf))
                .help("Use this YAML file instead of the config location provided by PhiHome"),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .action(ArgAction::SetTrue)
                .help("Disable human-readable stderr logs"),
        )
        .arg(
            Arg::new("null_executor")
                .long("null-executor")
                .action(ArgAction::SetTrue)
                .help("Use an executor initialized without built-in or module-provided tools"),
        )
        .arg(
            Arg::new("max_model_request_retries")
                .long("max-model-request-retries")
                .value_name("N")
                .value_parser(clap::value_parser!(usize)),
        )
        .arg(
            Arg::new("container")
                .long("container")
                .value_name("NAME")
                .conflicts_with("runner")
                .help("Execute shell jobs inside an already-running Docker container"),
        )
        .arg(
            Arg::new("runner")
                .long("runner")
                .value_name("PROGRAM")
                .conflicts_with("container")
                .help("Pass shell job commands to a custom runner program"),
        )
        .arg(
            Arg::new("runner_arg")
                .long("runner-arg")
                .value_name("ARG")
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .requires("runner")
                .help("Append a fixed runner argument before each shell command"),
        ),
    )
}
