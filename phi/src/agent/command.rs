use crate::headlessterm::TerminalCommand;

#[derive(Clone)]
pub enum PhiAgentCommand {
    Run(RunCommand),
    Yolo(RunCommand),
    Step(StepCommand),
    Probe(ProbeCommand),
    Doctor(DoctorCommand),
    History(HistoryCommand),
}

#[derive(Clone)]
pub struct RunCommand {
    pub max_steps: Option<usize>,
    options: AgentCommandOptions,
}

#[derive(Clone)]
pub struct StepCommand {
    options: AgentCommandOptions,
}

#[derive(Clone)]
struct AgentCommandOptions {
    pub max_model_request_retries: Option<usize>,
    pub quiet: bool,
    executor: ExecutorOptions,
}

#[derive(Clone)]
enum ExecutorOptions {
    Enabled { target: ExecutionTarget },
    Null,
}

#[derive(Clone)]
enum ExecutionTarget {
    LocalShell,
    Container(String),
    CustomRunner { program: String, args: Vec<String> },
}

#[derive(Clone)]
pub struct ProbeCommand {
    pub max_model_request_retries: Option<usize>,
}

#[derive(Clone)]
pub struct DoctorCommand;

#[derive(Clone)]
pub struct HistoryCommand;

pub struct RunCommandArgs {
    pub options: AgentCommandArgs,
    pub max_steps: Option<usize>,
}

#[derive(Default)]
pub struct AgentCommandArgs {
    pub quiet: bool,
    pub null_executor: bool,
    pub max_model_request_retries: Option<usize>,
    pub container: Option<String>,
    pub runner: Option<String>,
    pub runner_args: Vec<String>,
}

pub type StepCommandArgs = AgentCommandArgs;

pub struct ProbeCommandArgs {
    pub max_model_request_retries: Option<usize>,
}

pub struct RunCommandInput<T> {
    pub args: T,
    pub forced_max_steps: Option<usize>,
}

pub struct StepCommandInput<T> {
    pub args: T,
}

impl PhiAgentCommand {
    pub fn run() -> RunCommand {
        RunCommand {
            max_steps: None,
            options: AgentCommandOptions::enabled_local(),
        }
    }

    pub fn step() -> StepCommand {
        StepCommand {
            options: AgentCommandOptions::enabled_local(),
        }
    }

    pub fn probe() -> ProbeCommand {
        ProbeCommand {
            max_model_request_retries: None,
        }
    }

    pub fn doctor() -> DoctorCommand {
        DoctorCommand
    }

    pub fn history() -> HistoryCommand {
        HistoryCommand
    }

    pub fn yolo() -> RunCommand {
        Self::run()
    }

    pub fn from_run_args<T>(
        args: T,
        max_steps: Option<usize>,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<RunCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Run(RunCommand {
            max_steps: max_steps.or(args.max_steps),
            options: AgentCommandOptions::try_from(args.options)?,
        }))
    }

    pub fn from_yolo_args<T>(
        args: T,
        max_steps: Option<usize>,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<RunCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Yolo(RunCommand {
            max_steps: max_steps.or(args.max_steps),
            options: AgentCommandOptions::try_from(args.options)?,
        }))
    }

    pub fn from_step_args<T>(args: T) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<StepCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Step(StepCommand {
            options: AgentCommandOptions::try_from(args)?,
        }))
    }

    pub fn from_probe_args<T>(args: T) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<ProbeCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Probe(Self::probe().with_max_model_request_retries(
            args.max_model_request_retries,
        )))
    }

    pub fn container(&self) -> Option<&str> {
        self.options().and_then(AgentCommandOptions::container)
    }

    pub(crate) fn terminal_command(&self, command: String) -> Option<TerminalCommand> {
        self.options()
            .and_then(|options| options.terminal_command(command))
    }

    pub(crate) fn null_executor(&self) -> bool {
        self.options()
            .is_some_and(|options| matches!(options.executor, ExecutorOptions::Null))
    }

    pub(crate) fn max_steps(&self) -> Option<usize> {
        match self {
            Self::Run(command) | Self::Yolo(command) => command.max_steps,
            _ => None,
        }
    }

    pub(crate) fn max_model_request_retries(&self) -> Option<usize> {
        match self {
            Self::Probe(command) => command.max_model_request_retries,
            _ => self
                .options()
                .and_then(|options| options.max_model_request_retries),
        }
    }

    pub(crate) fn verbose(&self) -> bool {
        self.options().is_some_and(|options| !options.quiet)
    }

    fn options(&self) -> Option<&AgentCommandOptions> {
        match self {
            Self::Run(command) | Self::Yolo(command) => Some(&command.options),
            Self::Step(command) => Some(&command.options),
            _ => None,
        }
    }
}

impl TryFrom<AgentCommandArgs> for AgentCommandOptions {
    type Error = Box<dyn std::error::Error>;

    fn try_from(args: AgentCommandArgs) -> Result<Self, Self::Error> {
        if args.null_executor {
            return Ok(Self {
                max_model_request_retries: args.max_model_request_retries,
                quiet: args.quiet,
                executor: ExecutorOptions::Null,
            });
        }
        let container = args.container.filter(|value| !value.trim().is_empty());
        let runner = match args.runner {
            Some(program) if program.trim().is_empty() => {
                return Err("--runner requires a non-empty program".into());
            }
            runner => runner,
        };
        if container.is_some() && runner.is_some() {
            return Err("--container and --runner are mutually exclusive".into());
        }
        if runner.is_none() && !args.runner_args.is_empty() {
            return Err("runner arguments require a runner program".into());
        }
        let target = match (container, runner) {
            (Some(container), None) => ExecutionTarget::Container(container),
            (None, Some(program)) => ExecutionTarget::CustomRunner {
                program,
                args: args.runner_args,
            },
            (None, None) => ExecutionTarget::LocalShell,
            (Some(_), Some(_)) => unreachable!("conflicting execution targets were rejected"),
        };
        Ok(Self {
            max_model_request_retries: args.max_model_request_retries,
            quiet: args.quiet,
            executor: ExecutorOptions::Enabled { target },
        })
    }
}

impl AgentCommandOptions {
    fn enabled_local() -> Self {
        Self {
            max_model_request_retries: None,
            quiet: false,
            executor: ExecutorOptions::Enabled {
                target: ExecutionTarget::LocalShell,
            },
        }
    }

    fn container(&self) -> Option<&str> {
        match &self.executor {
            ExecutorOptions::Enabled {
                target: ExecutionTarget::Container(container),
            } => Some(container),
            ExecutorOptions::Enabled { .. } | ExecutorOptions::Null => None,
        }
    }

    fn terminal_command(&self, command: String) -> Option<TerminalCommand> {
        match &self.executor {
            ExecutorOptions::Enabled {
                target: ExecutionTarget::LocalShell,
            } => Some(TerminalCommand::shell(command)),
            ExecutorOptions::Enabled {
                target: ExecutionTarget::Container(container),
            } => Some(TerminalCommand::docker_exec(container.clone(), command)),
            ExecutorOptions::Enabled {
                target: ExecutionTarget::CustomRunner { program, args },
            } => Some(TerminalCommand::custom_runner(
                program.clone(),
                args.clone(),
                command,
            )),
            ExecutorOptions::Null => None,
        }
    }
}

impl ProbeCommand {
    pub fn with_max_model_request_retries(
        mut self,
        max_model_request_retries: Option<usize>,
    ) -> Self {
        self.max_model_request_retries = max_model_request_retries;
        self
    }
}

impl RunCommand {
    pub fn with_max_steps(mut self, max_steps: Option<usize>) -> Self {
        self.max_steps = max_steps;
        self
    }

    pub fn with_max_model_request_retries(
        mut self,
        max_model_request_retries: Option<usize>,
    ) -> Self {
        self.options.max_model_request_retries = max_model_request_retries;
        self
    }

    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.options.quiet = quiet;
        self
    }

    pub fn with_null_executor(mut self, null_executor: bool) -> Self {
        if null_executor {
            self.options.executor = ExecutorOptions::Null;
        } else if matches!(self.options.executor, ExecutorOptions::Null) {
            self.options.executor = ExecutorOptions::Enabled {
                target: ExecutionTarget::LocalShell,
            };
        }
        self
    }

    pub fn with_container(mut self, container: Option<String>) -> Self {
        self.options.executor = ExecutorOptions::Enabled {
            target: container
                .filter(|value| !value.trim().is_empty())
                .map(ExecutionTarget::Container)
                .unwrap_or(ExecutionTarget::LocalShell),
        };
        self
    }
}

impl StepCommand {
    pub fn with_max_model_request_retries(
        mut self,
        max_model_request_retries: Option<usize>,
    ) -> Self {
        self.options.max_model_request_retries = max_model_request_retries;
        self
    }

    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.options.quiet = quiet;
        self
    }

    pub fn with_null_executor(mut self, null_executor: bool) -> Self {
        if null_executor {
            self.options.executor = ExecutorOptions::Null;
        } else if matches!(self.options.executor, ExecutorOptions::Null) {
            self.options.executor = ExecutorOptions::Enabled {
                target: ExecutionTarget::LocalShell,
            };
        }
        self
    }

    pub fn with_container(mut self, container: Option<String>) -> Self {
        self.options.executor = ExecutorOptions::Enabled {
            target: container
                .filter(|value| !value.trim().is_empty())
                .map(ExecutionTarget::Container)
                .unwrap_or(ExecutionTarget::LocalShell),
        };
        self
    }
}

impl<T> TryFrom<RunCommandInput<T>> for PhiAgentCommand
where
    T: Into<RunCommandArgs>,
{
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: RunCommandInput<T>) -> Result<Self, Self::Error> {
        Self::from_run_args(value.args, value.forced_max_steps)
    }
}

pub struct YoloCommandInput<T> {
    pub args: T,
    pub forced_max_steps: Option<usize>,
}

impl<T> TryFrom<YoloCommandInput<T>> for PhiAgentCommand
where
    T: Into<RunCommandArgs>,
{
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: YoloCommandInput<T>) -> Result<Self, Self::Error> {
        Self::from_yolo_args(value.args, value.forced_max_steps)
    }
}

impl<T> TryFrom<StepCommandInput<T>> for PhiAgentCommand
where
    T: Into<StepCommandArgs>,
{
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: StepCommandInput<T>) -> Result<Self, Self::Error> {
        Self::from_step_args(value.args)
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentCommandArgs, PhiAgentCommand};

    #[test]
    fn library_command_defaults_do_not_enable_model_retry() {
        for command in [
            PhiAgentCommand::Run(PhiAgentCommand::run()),
            PhiAgentCommand::Yolo(PhiAgentCommand::yolo()),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
            PhiAgentCommand::Probe(PhiAgentCommand::probe()),
        ] {
            assert_eq!(command.max_model_request_retries(), None);
        }
    }

    #[test]
    fn normalized_command_options_preserve_explicit_model_retry_budget() {
        let command = PhiAgentCommand::from_step_args(AgentCommandArgs {
            max_model_request_retries: Some(5),
            ..AgentCommandArgs::default()
        })
        .expect("step command should build");

        assert_eq!(command.max_model_request_retries(), Some(5));
    }

    #[test]
    fn null_executor_discards_container_when_command_options_are_built() {
        let command = PhiAgentCommand::from_step_args(AgentCommandArgs {
            quiet: false,
            null_executor: true,
            max_model_request_retries: Some(3),
            container: Some("unused-container".to_string()),
            ..AgentCommandArgs::default()
        })
        .expect("step command should build");

        assert!(command.null_executor());
        assert_eq!(command.container(), None);
    }

    #[test]
    fn runner_builds_a_custom_terminal_command() {
        let command = PhiAgentCommand::from_step_args(AgentCommandArgs {
            runner: Some("bash".to_owned()),
            runner_args: vec!["-c".to_owned()],
            ..AgentCommandArgs::default()
        })
        .expect("step command should accept a custom runner");

        assert!(matches!(
            command.terminal_command("printf ready".to_owned()),
            Some(crate::headlessterm::TerminalCommand::CustomRunner {
                program,
                args,
                command,
            }) if program == "bash" && args == ["-c"] && command == "printf ready"
        ));
    }

    #[test]
    fn runner_arguments_require_a_runner_program() {
        let result = PhiAgentCommand::from_step_args(AgentCommandArgs {
            runner_args: vec!["-c".to_owned()],
            ..AgentCommandArgs::default()
        });
        let Err(error) = result else {
            panic!("runner arguments without a runner should fail")
        };
        assert!(error.to_string().contains("require a runner program"));
    }

    #[test]
    fn runner_program_cannot_be_empty() {
        let result = PhiAgentCommand::from_step_args(AgentCommandArgs {
            runner: Some("  ".to_owned()),
            ..AgentCommandArgs::default()
        });
        let Err(error) = result else {
            panic!("an empty runner program should fail")
        };
        assert!(error.to_string().contains("non-empty program"));
    }
}
