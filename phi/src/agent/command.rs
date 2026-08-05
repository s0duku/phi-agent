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
    Enabled { container: Option<String> },
    Null,
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
            options: AgentCommandArgs::default().into(),
        }
    }

    pub fn step() -> StepCommand {
        StepCommand {
            options: AgentCommandArgs::default().into(),
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
            options: AgentCommandOptions::from(args.options),
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
            options: AgentCommandOptions::from(args.options),
        }))
    }

    pub fn from_step_args<T>(args: T) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<StepCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Step(StepCommand {
            options: AgentCommandOptions::from(args),
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

impl From<AgentCommandArgs> for AgentCommandOptions {
    fn from(args: AgentCommandArgs) -> Self {
        let container = args.container.filter(|value| !value.trim().is_empty());
        Self {
            max_model_request_retries: args.max_model_request_retries,
            quiet: args.quiet,
            executor: if args.null_executor {
                ExecutorOptions::Null
            } else {
                ExecutorOptions::Enabled { container }
            },
        }
    }
}

impl AgentCommandOptions {
    fn container(&self) -> Option<&str> {
        match &self.executor {
            ExecutorOptions::Enabled { container } => container.as_deref(),
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
            self.options.executor = ExecutorOptions::Enabled { container: None };
        }
        self
    }

    pub fn with_container(mut self, container: Option<String>) -> Self {
        self.options.executor = ExecutorOptions::Enabled {
            container: container.filter(|value| !value.trim().is_empty()),
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
            self.options.executor = ExecutorOptions::Enabled { container: None };
        }
        self
    }

    pub fn with_container(mut self, container: Option<String>) -> Self {
        self.options.executor = ExecutorOptions::Enabled {
            container: container.filter(|value| !value.trim().is_empty()),
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
        })
        .expect("step command should build");

        assert!(command.null_executor());
        assert_eq!(command.container(), None);
    }
}
