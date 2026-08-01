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
    pub template: Option<String>,
    pub plugin_args: Vec<String>,
    pub quiet: bool,
    executor: ExecutorOptions,
}

#[derive(Clone)]
enum ExecutorOptions {
    Enabled { container: Option<String> },
    Disabled,
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

pub struct AgentCommandArgs {
    pub quiet: bool,
    pub no_exec: bool,
    pub max_model_request_retries: Option<usize>,
    pub template: Option<String>,
    pub plugin_args: Vec<String>,
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
            options: AgentCommandOptions::default(),
        }
    }

    pub fn step() -> StepCommand {
        StepCommand {
            options: AgentCommandOptions::default(),
        }
    }

    pub fn probe() -> ProbeCommand {
        ProbeCommand {
            max_model_request_retries: Some(3),
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

    pub fn plugin_args(&self) -> &[String] {
        self.options()
            .map(|options| options.plugin_args.as_slice())
            .unwrap_or_default()
    }

    pub fn container(&self) -> Option<&str> {
        self.options().and_then(AgentCommandOptions::container)
    }

    pub fn template(&self) -> Option<&str> {
        self.options()
            .and_then(|options| options.template.as_deref())
    }

    pub(crate) fn no_exec(&self) -> bool {
        self.options()
            .is_some_and(|options| matches!(options.executor, ExecutorOptions::Disabled))
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

impl Default for AgentCommandOptions {
    fn default() -> Self {
        Self {
            max_model_request_retries: Some(3),
            template: None,
            plugin_args: Vec::new(),
            quiet: false,
            executor: ExecutorOptions::Enabled { container: None },
        }
    }
}

impl From<AgentCommandArgs> for AgentCommandOptions {
    fn from(args: AgentCommandArgs) -> Self {
        let container = args.container.filter(|value| !value.trim().is_empty());
        Self {
            max_model_request_retries: args.max_model_request_retries,
            template: args.template.filter(|value| !value.trim().is_empty()),
            plugin_args: args.plugin_args,
            quiet: args.quiet,
            executor: if args.no_exec {
                ExecutorOptions::Disabled
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
            ExecutorOptions::Disabled => None,
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

    pub fn with_template(mut self, template: Option<String>) -> Self {
        self.options.template = template.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.options.quiet = quiet;
        self
    }

    pub fn with_no_exec(mut self, no_exec: bool) -> Self {
        if no_exec {
            self.options.executor = ExecutorOptions::Disabled;
        } else if matches!(self.options.executor, ExecutorOptions::Disabled) {
            self.options.executor = ExecutorOptions::Enabled { container: None };
        }
        self
    }

    pub fn with_plugin_args(mut self, plugin_args: Vec<String>) -> Self {
        self.options.plugin_args = plugin_args;
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

    pub fn with_template(mut self, template: Option<String>) -> Self {
        self.options.template = template.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.options.quiet = quiet;
        self
    }

    pub fn with_no_exec(mut self, no_exec: bool) -> Self {
        if no_exec {
            self.options.executor = ExecutorOptions::Disabled;
        } else if matches!(self.options.executor, ExecutorOptions::Disabled) {
            self.options.executor = ExecutorOptions::Enabled { container: None };
        }
        self
    }

    pub fn with_plugin_args(mut self, plugin_args: Vec<String>) -> Self {
        self.options.plugin_args = plugin_args;
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
    fn no_exec_discards_container_when_command_options_are_built() {
        let command = PhiAgentCommand::from_step_args(AgentCommandArgs {
            quiet: false,
            no_exec: true,
            max_model_request_retries: Some(3),
            template: None,
            plugin_args: Vec::new(),
            container: Some("unused-container".to_string()),
        })
        .expect("step command should build");

        assert!(command.no_exec());
        assert_eq!(command.container(), None);
    }
}
