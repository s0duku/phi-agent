use crate::message::PhiMessage;

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
    pub max_model_request_retries: Option<usize>,
    pub template: Option<String>,
    pub plugin_args: Vec<String>,
    pub container: Option<String>,
    pub quiet: bool,
    pub input_messages: Vec<PhiMessage>,
}

#[derive(Clone)]
pub struct StepCommand {
    pub max_model_request_retries: Option<usize>,
    pub template: Option<String>,
    pub plugin_args: Vec<String>,
    pub container: Option<String>,
    pub quiet: bool,
    pub input_messages: Vec<PhiMessage>,
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
    pub quiet: bool,
    pub max_steps: Option<usize>,
    pub max_model_request_retries: Option<usize>,
    pub template: Option<String>,
    pub plugin_args: Vec<String>,
    pub container: Option<String>,
}

pub struct StepCommandArgs {
    pub quiet: bool,
    pub max_model_request_retries: Option<usize>,
    pub template: Option<String>,
    pub plugin_args: Vec<String>,
    pub container: Option<String>,
}

pub struct ProbeCommandArgs {
    pub max_model_request_retries: Option<usize>,
}

pub struct RunCommandInput<T> {
    pub args: T,
    pub forced_max_steps: Option<usize>,
    pub input_messages: Vec<PhiMessage>,
}

pub struct StepCommandInput<T> {
    pub args: T,
    pub input_messages: Vec<PhiMessage>,
}

impl PhiAgentCommand {
    pub fn run() -> RunCommand {
        RunCommand {
            max_steps: None,
            max_model_request_retries: Some(3),
            template: None,
            plugin_args: Vec::new(),
            container: None,
            quiet: false,
            input_messages: Vec::new(),
        }
    }

    pub fn step() -> StepCommand {
        StepCommand {
            max_model_request_retries: Some(3),
            template: None,
            plugin_args: Vec::new(),
            container: None,
            quiet: false,
            input_messages: Vec::new(),
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
        input_messages: Vec<PhiMessage>,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<RunCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Run(
            Self::run()
                .with_max_steps(max_steps.or(args.max_steps))
                .with_max_model_request_retries(args.max_model_request_retries)
                .with_template(args.template)
                .with_plugin_args(args.plugin_args)
                .with_container(args.container)
                .with_quiet(args.quiet)
                .with_input_messages(input_messages),
        ))
    }

    pub fn from_yolo_args<T>(
        args: T,
        max_steps: Option<usize>,
        input_messages: Vec<PhiMessage>,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<RunCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Yolo(
            Self::yolo()
                .with_max_steps(max_steps.or(args.max_steps))
                .with_max_model_request_retries(args.max_model_request_retries)
                .with_template(args.template)
                .with_plugin_args(args.plugin_args)
                .with_container(args.container)
                .with_quiet(args.quiet)
                .with_input_messages(input_messages),
        ))
    }

    pub fn from_step_args<T>(
        args: T,
        input_messages: Vec<PhiMessage>,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Into<StepCommandArgs>,
    {
        let args = args.into();
        Ok(Self::Step(
            Self::step()
                .with_max_model_request_retries(args.max_model_request_retries)
                .with_template(args.template)
                .with_plugin_args(args.plugin_args)
                .with_container(args.container)
                .with_quiet(args.quiet)
                .with_input_messages(input_messages),
        ))
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
        match self {
            PhiAgentCommand::Run(command) => &command.plugin_args,
            PhiAgentCommand::Yolo(command) => &command.plugin_args,
            PhiAgentCommand::Step(command) => &command.plugin_args,
            _ => &[],
        }
    }

    pub fn container(&self) -> Option<&str> {
        match self {
            PhiAgentCommand::Run(command) | PhiAgentCommand::Yolo(command) => {
                command.container.as_deref()
            }
            PhiAgentCommand::Step(command) => command.container.as_deref(),
            _ => None,
        }
    }

    pub fn template(&self) -> Option<&str> {
        match self {
            PhiAgentCommand::Run(command) => command.template.as_deref(),
            PhiAgentCommand::Yolo(command) => command.template.as_deref(),
            PhiAgentCommand::Step(command) => command.template.as_deref(),
            _ => None,
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
        self.max_model_request_retries = max_model_request_retries;
        self
    }

    pub fn with_template(mut self, template: Option<String>) -> Self {
        self.template = template.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    pub fn with_plugin_args(mut self, plugin_args: Vec<String>) -> Self {
        self.plugin_args = plugin_args;
        self
    }

    pub fn with_container(mut self, container: Option<String>) -> Self {
        self.container = container.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn with_input_messages(mut self, input_messages: Vec<PhiMessage>) -> Self {
        self.input_messages = input_messages;
        self
    }
}

impl StepCommand {
    pub fn with_max_model_request_retries(
        mut self,
        max_model_request_retries: Option<usize>,
    ) -> Self {
        self.max_model_request_retries = max_model_request_retries;
        self
    }

    pub fn with_template(mut self, template: Option<String>) -> Self {
        self.template = template.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    pub fn with_plugin_args(mut self, plugin_args: Vec<String>) -> Self {
        self.plugin_args = plugin_args;
        self
    }

    pub fn with_container(mut self, container: Option<String>) -> Self {
        self.container = container.filter(|value| !value.trim().is_empty());
        self
    }

    pub fn with_input_messages(mut self, input_messages: Vec<PhiMessage>) -> Self {
        self.input_messages = input_messages;
        self
    }
}

impl<T> TryFrom<RunCommandInput<T>> for PhiAgentCommand
where
    T: Into<RunCommandArgs>,
{
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: RunCommandInput<T>) -> Result<Self, Self::Error> {
        Self::from_run_args(value.args, value.forced_max_steps, value.input_messages)
    }
}

pub struct YoloCommandInput<T> {
    pub args: T,
    pub forced_max_steps: Option<usize>,
    pub input_messages: Vec<PhiMessage>,
}

impl<T> TryFrom<YoloCommandInput<T>> for PhiAgentCommand
where
    T: Into<RunCommandArgs>,
{
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: YoloCommandInput<T>) -> Result<Self, Self::Error> {
        Self::from_yolo_args(value.args, value.forced_max_steps, value.input_messages)
    }
}

impl<T> TryFrom<StepCommandInput<T>> for PhiAgentCommand
where
    T: Into<StepCommandArgs>,
{
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: StepCommandInput<T>) -> Result<Self, Self::Error> {
        Self::from_step_args(value.args, value.input_messages)
    }
}
