mod command;
mod step;

pub use command::{
    AgentCommandArgs, DoctorCommand, HistoryCommand, PhiAgentCommand, RunCommand, RunCommandArgs,
    RunCommandInput, StepCommand, StepCommandArgs, StepCommandInput, YoloCommandInput,
};
#[allow(unused_imports)]
pub(crate) use step::{
    RuntimeFailureStep, StepBounce, StepCont, StepInterveneError, StepInterveneNext,
    StepInterveneResult,
};

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) use crate::expr::{PhiExprDelta, PhiStepExpr};
#[cfg(test)]
use crate::module::PhiModule;
use crate::{
    config::{ModelRequestDefaults, PhiConfig},
    error::PhiAgentRuntimeError,
    executor::{PhiExecutor, PhiToolDefinition, ToolOutputLimits},
    features::{build_default_modules, build_init_modules, build_runtime_modules},
    home::{PhiHome, PhiHomeDoctorReport},
    message::{PhiHistory, PhiMessage},
    module::{PhiAgentCommitEvent, PhiModuleChain, PhiModuleLayout},
    render,
    session::{PhiAgentStep, PhiReActStep, Session},
};

#[derive(serde::Serialize)]
pub struct DoctorReport {
    pub home: PhiHomeDoctorReport,
    pub system: DoctorSystemPrompt,
    pub tools: Vec<crate::executor::PhiToolDefinition>,
}

#[derive(serde::Serialize)]
pub struct DoctorSystemPrompt {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

pub struct PhiAgentBuildContext {
    pub(crate) session: Session,
    pub(crate) command: PhiAgentCommand,
    pub(crate) config: PhiConfig,
}

impl PhiAgentBuildContext {
    fn new(session: Session, command: PhiAgentCommand, config: PhiConfig) -> Self {
        Self {
            session,
            command,
            config,
        }
    }

    pub fn config(&self) -> &PhiConfig {
        &self.config
    }

    pub fn command(&self) -> &PhiAgentCommand {
        &self.command
    }
}

pub struct AgentStepRunOutcome {
    pub session: Session,
    pub error: Option<PhiAgentRuntimeError>,
}

pub struct PhiAgent {
    runtime: Option<PhiAgentRuntime>,
}

#[derive(Clone, Default)]
pub(crate) struct PhiCancellation {
    inner: Arc<PhiCancellationState>,
}

#[derive(Default)]
struct PhiCancellationState {
    cancelled: AtomicBool,
    commit_on_cancel: AtomicBool,
    notify: tokio::sync::Notify,
}

#[derive(Clone)]
pub(crate) struct PhiRuntimeSetup {
    config: PhiConfig,
    command: PhiAgentCommand,
    home: Arc<dyn PhiHome>,
    cancellation: PhiCancellation,
}

pub(crate) struct PhiAgentRuntime {
    base: PhiStepExpr,
    delta: PhiExprDelta,
    executor: PhiExecutor,
    configured_tools: Vec<PhiToolDefinition>,
    modules: PhiModuleChain,
    render: render::PhiRender,
    model_defaults: ModelRequestDefaults,
    setup: PhiRuntimeSetup,
}

pub struct PhiAgentBuilder {
    session: Session,
    command: PhiAgentCommand,
    modules: PhiModuleLayout,
    render: Option<render::PhiRender>,
    model_defaults: Option<ModelRequestDefaults>,
    home: Option<Arc<dyn PhiHome>>,
    config: Option<PhiConfig>,
}

pub(crate) struct PreparedPhiAgentBuilder {
    context: PhiAgentBuildContext,
    modules: PhiModuleLayout,
    render: Option<render::PhiRender>,
    model_defaults: Option<ModelRequestDefaults>,
    home: Arc<dyn PhiHome>,
}

impl PhiAgentBuilder {
    fn new(session: Session, command: PhiAgentCommand) -> Self {
        Self {
            session,
            command,
            modules: PhiModuleLayout::default(),
            render: None,
            model_defaults: None,
            home: None,
            config: None,
        }
    }

    pub(crate) fn prepare(mut self) -> Result<PreparedPhiAgentBuilder, Box<dyn std::error::Error>> {
        // PhiHome is treated as an external runtime component now: callers
        // must resolve and instantiate it before agent build so the builder
        // consumes a concrete home instance instead of performing hidden
        // discovery on its own.
        let home = self.home.take().ok_or_else(|| {
            "PhiAgentBuilder requires a PhiHome instance before prepare/build".to_string()
        })?;

        // Resolve the immutable setup before deriving modules so every build
        // stage and the resulting runtime observe the same config snapshot.
        let config = self
            .config
            .take()
            .map(Ok)
            .unwrap_or_else(|| crate::load_config(home.as_ref(), None))?;
        Ok(PreparedPhiAgentBuilder {
            context: PhiAgentBuildContext::new(self.session, self.command, config),
            modules: self.modules,
            render: self.render,
            model_defaults: self.model_defaults,
            home,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_module(mut self, module: impl PhiModule + 'static) -> Self {
        self.modules.push_extension(Box::new(module));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_client(mut self, client: Arc<dyn crate::render::TestClient>) -> Self {
        self.render = Some(render::PhiRender::from_test_client(client));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_render(mut self, render: render::PhiRender) -> Self {
        self.render = Some(render);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_model_defaults(mut self, model_defaults: ModelRequestDefaults) -> Self {
        self.model_defaults = Some(model_defaults);
        self
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_home(mut self, home: Arc<dyn PhiHome>) -> Self {
        self.home = Some(home);
        self
    }

    pub(crate) fn with_config(mut self, config: PhiConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn build(self) -> Result<PhiAgent, Box<dyn std::error::Error>> {
        self.prepare()?.build()
    }
}

impl PreparedPhiAgentBuilder {
    pub fn context(&self) -> &PhiAgentBuildContext {
        &self.context
    }

    #[cfg(test)]
    pub(crate) fn with_module(mut self, module: impl PhiModule + 'static) -> Self {
        self.modules.push_extension(Box::new(module));
        self
    }

    pub(crate) fn with_module_layout(mut self, layout: PhiModuleLayout) -> Self {
        self.modules.extend(layout);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_client(mut self, client: Arc<dyn crate::render::TestClient>) -> Self {
        self.render = Some(render::PhiRender::from_test_client(client));
        self
    }

    pub fn build(mut self) -> Result<PhiAgent, Box<dyn std::error::Error>> {
        // Agent construction is intentionally staged here so every caller goes
        // through the same lifecycle:
        // 1. prepare() resolves home + merged config
        // 2. module may rewrite the build context
        // 3. executor is derived, then module may refine it
        // 4. the resulting session/executor/provider are frozen into PhiAgent
        //
        // The prepared builder exists so any module derivation uses the
        // exact same build context that final build() will consume.
        self.context.session.validate()?;
        let mut modules = self.modules.into_modules();
        crate::module::init_context_modules(&mut modules, &mut self.context)?;
        self.context.session.validate()?;

        let mut tools = Vec::new();
        if !self.context.command.null_executor() {
            tools = crate::executor::builtins::default_tools();
            crate::module::module_tools(&mut modules, &self.context, &mut tools);
        }
        let output_limits = ToolOutputLimits::new(
            self.context.config.executor().tool_threshold_tokens,
            self.context.config.executor().tool_preview_bytes,
        );
        let executor = PhiExecutor::from_tools(tools, output_limits)?;

        let model_defaults = if let Some(model_defaults) = self.model_defaults.take() {
            model_defaults
        } else {
            ModelRequestDefaults::from(&self.context.config)
        };
        let configured_tools = self.context.config.tools().to_vec();
        if model_defaults.model.trim().is_empty() {
            eprintln!(
                "phi provider: PHI_MODEL is not configured; model requests will likely fail until a model name is set"
            );
        }
        let provider_config = self.context.config.provider().clone();
        let render = if let Some(render) = self.render.take() {
            render
        } else {
            render::build(provider_config)?
        };
        // This is the single state ownership crossing into evaluation: Session is consumed
        // here, and the runtime retains only the functional expression plus transient delta.
        Ok(PhiAgent {
            runtime: Some(PhiAgentRuntime {
                base: self.context.session.into_expr(),
                delta: PhiExprDelta::default(),
                executor,
                configured_tools,
                modules: PhiModuleChain::new(modules),
                render,
                model_defaults,
                setup: PhiRuntimeSetup {
                    config: self.context.config,
                    command: self.context.command,
                    home: self.home,
                    cancellation: PhiCancellation::default(),
                },
            }),
        })
    }
}

impl PhiAgent {
    pub fn builder(session: Session, command: PhiAgentCommand) -> PhiAgentBuilder {
        PhiAgentBuilder::new(session, command)
    }

    pub fn into_session(self) -> Session {
        // Session is reconstructed only at the agent boundary; runtime evaluation never uses it.
        Session::from_expr(
            self.runtime
                .expect("PhiAgent runtime should exist when consuming session")
                .base,
        )
    }

    pub fn session(&self) -> Session {
        // Checkpoints are immutable Session snapshots of the last committed runtime base.
        Session::from_expr(
            self.runtime
                .as_ref()
                .expect("PhiAgent runtime should exist while viewing session")
                .base
                .clone(),
        )
    }

    pub fn doctor_report(&self) -> DoctorReport {
        let runtime = self
            .runtime
            .as_ref()
            .expect("PhiAgent runtime should exist while building doctor report");
        let system_prompt =
            crate::features::configured_system_prompt_from_config(runtime.setup().config());
        DoctorReport {
            home: runtime.home().doctor_report(),
            system: DoctorSystemPrompt {
                enabled: system_prompt.is_some(),
                content: system_prompt,
            },
            tools: runtime.tool_definitions(),
        }
    }

    #[cfg(test)]
    pub(crate) fn runtime(&self) -> &PhiAgentRuntime {
        self.runtime
            .as_ref()
            .expect("PhiAgent runtime should exist while borrowing runtime")
    }

    pub async fn run_single_step(mut self) -> AgentStepRunOutcome {
        self.step().await;
        AgentStepRunOutcome {
            error: self.session().step().error().cloned(),
            session: self.into_session(),
        }
    }

    pub async fn step(&mut self) {
        let runtime = self
            .runtime
            .take()
            .expect("PhiAgent runtime should exist during step evaluation");
        self.runtime = Some(runtime.run_step().await);
    }

    pub(crate) fn cancellation(&self) -> PhiCancellation {
        self.runtime
            .as_ref()
            .expect("PhiAgent runtime should exist while borrowing cancellation")
            .setup
            .cancellation
            .clone()
    }
}

impl PhiCancellation {
    pub(crate) fn commit_current_step_on_cancel(&self) {
        self.inner.commit_on_cancel.store(true, Ordering::Release);
    }

    pub(crate) fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.notify.notify_waiters();
        }
    }

    pub(crate) fn should_commit_current_step(&self) -> bool {
        self.inner.commit_on_cancel.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        if self.inner.cancelled.load(Ordering::Acquire) {
            return;
        }
        let notified = self.inner.notify.notified();
        if self.inner.cancelled.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

impl PhiAgentRuntime {
    pub(crate) fn setup(&self) -> &PhiRuntimeSetup {
        &self.setup
    }
}

impl PhiRuntimeSetup {
    pub(crate) fn config(&self) -> &PhiConfig {
        &self.config
    }

    pub(crate) fn command(&self) -> &PhiAgentCommand {
        &self.command
    }

    pub(crate) fn home(&self) -> &dyn PhiHome {
        self.home.as_ref()
    }

    pub(crate) fn cancellation(&self) -> &PhiCancellation {
        &self.cancellation
    }
}

impl PhiAgentRuntime {
    pub(crate) fn base_expr(&self) -> &PhiStepExpr {
        &self.base
    }

    pub(crate) fn base_step(&self) -> &PhiAgentStep {
        self.base.step()
    }

    #[cfg(test)]
    pub(crate) fn module_count(&self) -> usize {
        self.modules.len()
    }

    pub(crate) fn history(&self) -> PhiHistory {
        let mut messages = self.base.history().into_messages();
        messages.extend(self.delta.history().clone().into_messages());
        PhiHistory::from_messages(messages)
    }

    pub(crate) fn home(&self) -> &dyn PhiHome {
        self.setup.home()
    }

    pub(crate) fn cur_delta_mut(&mut self) -> &mut PhiExprDelta {
        &mut self.delta
    }

    pub(crate) fn find_ancestor(
        &self,
        predicate: impl Fn(&PhiAgentStep) -> bool,
    ) -> Option<&PhiStepExpr> {
        self.base.find_ancestor(predicate)
    }

    pub(crate) fn provider_history_token_count(&self, history: &PhiHistory) -> usize {
        self.render.provider_history_token_count(history)
    }

    pub(crate) fn request_provider_step(&self, detail: impl Into<String>) -> PhiReActStep {
        PhiReActStep::request_provider_with_call(detail, self.request_provider_call())
    }

    pub(crate) fn request_provider_call(&self) -> render::PhiProviderCall {
        render::PhiProviderCall::from_parts(&self.model_defaults, self.tool_definitions())
    }

    pub(crate) fn request_provider_request(
        &self,
        request: render::PhiProviderCall,
    ) -> render::PhiProviderCall {
        let mut request = request;
        let mut seen = request
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        for tool in self.tool_definitions() {
            if seen.insert(tool.name.clone()) {
                request.tools.push(tool);
            }
        }
        request
    }

    pub(crate) fn tool_definitions(&self) -> Vec<PhiToolDefinition> {
        // Relative tool order is part of the provider-call surface: executor
        // definitions must stay first and preserve executor registration order,
        // then configured tools follow in their external order. Keeping this
        // sequence stable helps preserve provider KV-cache reuse.
        let mut tools = self.executor.definitions();
        let mut seen = tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        for tool in &self.configured_tools {
            if seen.insert(tool.name.clone()) {
                tools.push(tool.clone());
            }
        }
        tools
    }

    // Commit events are post-commit observation hooks. Once the step decides to
    // publish history, observation must not be able to re-open the error path.
    // That invariant is enforced by using the infallible observe(...) channel
    // instead of the fallible handle(...) path used during payload shaping.
    pub(crate) fn commit_message(&mut self, message: PhiMessage) {
        self.delta.push_message(message.clone());
        let event = PhiAgentCommitEvent::MessageCommitted { message: &message };
        self.modules.observe(&event);
    }

    pub(crate) fn commit_model_response(&mut self, assistant: crate::message::PhiAssistantMessage) {
        let message = PhiMessage::Assistant(assistant);
        self.delta.push_message(message.clone());
        self.modules
            .observe(&PhiAgentCommitEvent::MessageCommitted { message: &message });
        self.modules
            .observe(&PhiAgentCommitEvent::ModelResponseCommitted { message: &message });
    }

    // Display rendering belongs to the step events before this point.
    pub(crate) fn commit_tool_result(&mut self, message: PhiMessage) {
        self.commit_message(message);
    }

    pub(crate) fn emit_warning(&mut self, message: &str) {
        let event = PhiAgentCommitEvent::WarningEmitted { message };
        self.modules.observe(&event);
    }
}

pub async fn run_single_agent_step(
    session: Session,
    command: PhiAgentCommand,
    home: Arc<dyn PhiHome>,
) -> Result<AgentStepRunOutcome, Box<dyn std::error::Error>> {
    let agent = build_agent(session, command, home)?;
    Ok(agent.run_single_step().await)
}

pub(crate) fn build_agent(
    session: Session,
    command: PhiAgentCommand,
    home: Arc<dyn PhiHome>,
) -> Result<PhiAgent, Box<dyn std::error::Error>> {
    build_prepared_agent(
        PhiAgent::builder(session, command)
            .with_home(home)
            .prepare()?,
    )
}

pub(crate) fn build_agent_with_config(
    session: Session,
    command: PhiAgentCommand,
    home: Arc<dyn PhiHome>,
    config: PhiConfig,
) -> Result<PhiAgent, Box<dyn std::error::Error>> {
    build_prepared_agent(
        PhiAgent::builder(session, command)
            .with_home(home)
            .with_config(config)
            .prepare()?,
    )
}

fn build_prepared_agent(
    mut builder: PreparedPhiAgentBuilder,
) -> Result<PhiAgent, Box<dyn std::error::Error>> {
    let init_modules = build_init_modules(builder.context());
    let default_modules = build_default_modules(builder.context());
    let runtime_modules = build_runtime_modules(builder.context());
    builder = builder.with_module_layout(init_modules);
    builder = builder.with_module_layout(default_modules);
    builder = builder.with_module_layout(runtime_modules);
    builder.build()
}
