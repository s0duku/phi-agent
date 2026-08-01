use serde::{Serialize, de::DeserializeOwned};

mod command;
mod step;

pub use command::{
    DoctorCommand, HistoryCommand, PhiAgentCommand, ProbeCommand, ProbeCommandArgs, RunCommand,
    RunCommandArgs, RunCommandInput, StepCommand, StepCommandArgs, StepCommandInput,
    YoloCommandInput,
};
#[allow(unused_imports)]
pub(crate) use step::{
    RuntimeFailureStep, StepBounce, StepCont, StepInterveneError, StepInterveneNext,
    StepInterveneResult,
};

use std::{collections::BTreeSet, sync::Arc};

pub(crate) use crate::expr::{DeltaLookup, PhiExprDelta, PhiStepExpr};
#[cfg(test)]
use crate::module::PhiModule;
use crate::{
    config::{ModelRequestDefaults, PhiConfig, PhiRuntimeSetup, ProviderConfig},
    error::PhiAgentRuntimeError,
    executor::{PhiExecutor, PhiToolDefinition},
    features::{build_default_modules, build_init_modules, build_runtime_modules},
    home::{PhiHome, PhiHomeDoctorReport},
    message::{PhiHistory, PhiMessage},
    module::{PhiAgentCommitEvent, PhiModuleChain, PhiModuleLayout},
    render,
    session::{PhiAgentStep, PhiReActStep, Session},
};

#[derive(serde::Serialize)]
pub struct DoctorReport {
    pub python_plugin: crate::features::PluginRuntimeStatus,
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
    pub(crate) home: Option<Arc<dyn PhiHome>>,
}

impl PhiAgentBuildContext {
    pub(crate) fn new(session: Session, command: PhiAgentCommand) -> Self {
        Self {
            session,
            command,
            config: PhiConfig::default(),
            home: None,
        }
    }

    pub(crate) fn bind_home(
        &mut self,
        home: Arc<dyn PhiHome>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.config = home.config()?;
        self.home = Some(home);
        Ok(())
    }

    pub fn config(&self) -> &PhiConfig {
        &self.config
    }

    pub fn command(&self) -> &PhiAgentCommand {
        &self.command
    }

    pub fn home(&self) -> Option<&Arc<dyn PhiHome>> {
        self.home.as_ref()
    }
}

pub struct AgentStepRunOutcome {
    pub session: Session,
    pub error: Option<PhiAgentRuntimeError>,
}

pub struct PhiAgent {
    runtime: Option<PhiAgentRuntime>,
}

pub(crate) struct PhiAgentRuntime {
    base: PhiStepExpr,
    delta: PhiExprDelta,
    executor: PhiExecutor,
    configured_tools: Vec<PhiToolDefinition>,
    modules: PhiModuleChain,
    render: render::PhiRender,
    model_defaults: ModelRequestDefaults,
    render_template: Option<String>,
    home: Arc<dyn PhiHome>,
    config: PhiRuntimeSetup,
}

pub struct PhiAgentBuilder {
    context: PhiAgentBuildContext,
    modules: PhiModuleLayout,
    render: Option<render::PhiRender>,
    model_defaults: Option<ModelRequestDefaults>,
    home: Option<Arc<dyn PhiHome>>,
}

pub(crate) struct PreparedPhiAgentBuilder {
    builder: PhiAgentBuilder,
}

impl PhiAgentBuilder {
    fn from_context(context: PhiAgentBuildContext) -> Self {
        Self {
            context,
            modules: PhiModuleLayout::default(),
            render: None,
            model_defaults: None,
            home: None,
        }
    }

    pub fn context(&self) -> &PhiAgentBuildContext {
        &self.context
    }

    pub(crate) fn prepare(mut self) -> Result<PreparedPhiAgentBuilder, Box<dyn std::error::Error>> {
        // PhiHome is treated as an external runtime component now: callers
        // must resolve and instantiate it before agent build so the builder
        // consumes a concrete home instance instead of performing hidden
        // discovery on its own.
        let home = self.home.take().ok_or_else(|| {
            "PhiAgentBuilder requires a PhiHome instance before prepare/build".to_string()
        })?;

        // Home config is part of the agent's durable runtime environment, so
        // it must be visible before we derive any modules from the build
        // context. Otherwise build-time module selection would see only
        // process env while later runtime/reporting paths see merged settings.
        self.context.bind_home(home.clone())?;
        self.home = Some(home);
        Ok(PreparedPhiAgentBuilder { builder: self })
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

    pub fn build(self) -> Result<PhiAgent, Box<dyn std::error::Error>> {
        self.prepare()?.build()
    }
}

impl PreparedPhiAgentBuilder {
    pub fn context(&self) -> &PhiAgentBuildContext {
        &self.builder.context
    }

    #[cfg(test)]
    pub(crate) fn with_module(mut self, module: impl PhiModule + 'static) -> Self {
        self.builder.modules.push_extension(Box::new(module));
        self
    }

    pub(crate) fn with_module_layout(mut self, layout: PhiModuleLayout) -> Self {
        self.builder.modules.extend(layout);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_client(mut self, client: Arc<dyn crate::render::TestClient>) -> Self {
        self.builder.render = Some(render::PhiRender::from_test_client(client));
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
        let home = self
            .builder
            .home
            .take()
            .expect("PreparedPhiAgentBuilder should retain home after prepare()");
        let mut modules = self.builder.modules.into_modules();
        crate::module::init_context_modules(&mut modules, &mut self.builder.context)?;

        let mut tools = crate::executor::builtins::default_tools();
        crate::module::module_tools(&mut modules, &self.builder.context, &mut tools);
        let output_limits =
            crate::config::tool_output_limits_from_config(&self.builder.context.config);
        let executor = PhiExecutor::from_tools(tools, output_limits)?;

        let model_defaults = if let Some(model_defaults) = self.builder.model_defaults.take() {
            model_defaults
        } else {
            ModelRequestDefaults::from_config(&self.builder.context.config)?
        };
        let configured_tools = crate::config::phi_tools_from_config(&self.builder.context.config)?;
        if model_defaults.model.trim().is_empty() {
            eprintln!(
                "phi provider: PHI_MODEL is not configured; model requests will likely fail until a model name is set"
            );
        }
        let provider_config = ProviderConfig::from_config(&self.builder.context.config)?;
        let render = if let Some(render) = self.builder.render.take() {
            render
        } else {
            render::build(provider_config)?
        };
        let render_template = self
            .builder
            .context
            .command
            .template()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.builder
                    .context
                    .config
                    .get("PHI_TEMPLATE")
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });

        // This is the single state ownership crossing into evaluation: Session is consumed
        // here, and the runtime retains only the functional expression plus transient delta.
        Ok(PhiAgent {
            runtime: Some(PhiAgentRuntime {
                base: self.builder.context.session.into_expr(),
                delta: PhiExprDelta::default(),
                executor,
                configured_tools,
                modules: PhiModuleChain::new(modules),
                render,
                model_defaults,
                render_template,
                home,
                config: PhiRuntimeSetup::from_command(
                    self.builder.context.config.clone(),
                    &self.builder.context.command,
                ),
            }),
        })
    }
}

impl PhiAgent {
    pub fn builder(session: Session, command: PhiAgentCommand) -> PhiAgentBuilder {
        PhiAgentBuilder::from_context(PhiAgentBuildContext::new(session, command))
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

    pub fn probe_report(&self) -> crate::probe::PhiSessionProbe {
        let runtime = self
            .runtime
            .as_ref()
            .expect("PhiAgent runtime should exist while probing");
        let modules = runtime.modules.probe_json(runtime);
        crate::probe::probe_session(&self.session(), modules)
    }

    pub fn doctor_report(&self) -> DoctorReport {
        let runtime = self
            .runtime
            .as_ref()
            .expect("PhiAgent runtime should exist while building doctor report");
        let system_prompt =
            crate::features::configured_system_prompt_from_config(runtime.config().config());
        DoctorReport {
            python_plugin: crate::features::plugin_runtime_status(),
            home: runtime.home().doctor_report(),
            system: DoctorSystemPrompt {
                enabled: system_prompt.is_some(),
                content: system_prompt,
            },
            tools: runtime.tool_definitions(),
        }
    }

    fn into_runtime(self) -> PhiAgentRuntime {
        self.runtime
            .expect("PhiAgent runtime should exist when splitting agent")
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

    pub async fn run_to_completion(mut self) -> AgentStepRunOutcome {
        self.run().await;
        AgentStepRunOutcome {
            error: self.session().step().error().cloned(),
            session: self.into_session(),
        }
    }

    pub async fn run_to_completed(mut self) -> AgentStepRunOutcome {
        self.yolo().await;
        AgentStepRunOutcome {
            error: self.session().step().error().cloned(),
            session: self.into_session(),
        }
    }

    pub fn run_python_code(self, code: &str) -> Result<String, Box<dyn std::error::Error>> {
        let mut runtime = self.into_runtime();
        runtime.run_python_code(code)
    }

    pub async fn step(&mut self) {
        let runtime = self
            .runtime
            .take()
            .expect("PhiAgent runtime should exist during step evaluation");
        self.runtime = Some(runtime.run_step().await);
    }

    pub async fn run(&mut self) {
        loop {
            self.step().await;
            let step = self
                .runtime
                .as_ref()
                .expect("PhiAgent runtime should exist while running")
                .base_step();
            if step.is_terminal()
                || matches!(step, PhiAgentStep::ReAct(PhiReActStep::RequestCompact))
            {
                return;
            }
        }
    }

    pub async fn yolo(&mut self) {
        let mut previous_was_failed = false;
        loop {
            self.step().await;
            match self
                .runtime
                .as_ref()
                .expect("PhiAgent runtime should exist while running")
                .base_step()
            {
                PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. }) => return,
                PhiAgentStep::Failed(_) if previous_was_failed => return,
                PhiAgentStep::Failed(_) => previous_was_failed = true,
                _ => previous_was_failed = false,
            }
        }
    }
}

impl PhiAgentRuntime {
    pub(crate) fn config(&self) -> &PhiRuntimeSetup {
        &self.config
    }

    pub(crate) fn base_expr(&self) -> &PhiStepExpr {
        &self.base
    }

    pub(crate) fn base_step(&self) -> &PhiAgentStep {
        self.base.step()
    }

    pub(crate) fn history(&self) -> PhiHistory {
        let mut messages = self.base.history().into_messages();
        messages.extend(self.delta.history().clone().into_messages());
        PhiHistory::from_messages(messages)
    }

    pub(crate) fn home(&self) -> &dyn PhiHome {
        self.home.as_ref()
    }

    #[allow(dead_code)]
    pub(crate) fn lookup<T>(&self, name: &str) -> Option<T>
    where
        T: DeserializeOwned,
    {
        match self.delta.lookup(name) {
            DeltaLookup::Value(value) => Some(value),
            DeltaLookup::Unset => None,
            DeltaLookup::Missing => self.base.lookup(name),
        }
    }

    pub(crate) fn cur_delta_mut(&mut self) -> &mut PhiExprDelta {
        &mut self.delta
    }

    pub(crate) fn base_delta(&self) -> &PhiExprDelta {
        self.base.delta()
    }

    #[allow(dead_code)]
    pub(crate) fn store<T>(&mut self, name: &str, value: T)
    where
        T: Serialize,
    {
        self.delta.bind(name, value);
    }

    #[allow(dead_code)]
    pub(crate) fn unstore(&mut self, name: &str) {
        self.delta.unbind(name);
    }

    pub(crate) fn find_ancestor(
        &self,
        predicate: impl Fn(&PhiAgentStep) -> bool,
    ) -> Option<&PhiStepExpr> {
        self.base.find_ancestor(predicate)
    }

    pub(crate) fn provider_history_token_count(
        &self,
        request: &render::PhiProviderCall,
        history: &PhiHistory,
    ) -> crate::error::PhiAgentRuntimeResult<usize> {
        self.render.provider_history_token_count(
            self.home(),
            self.render_template.as_deref(),
            request,
            history,
        )
    }

    pub(crate) fn request_provider_step(&self, detail: impl Into<String>) -> PhiReActStep {
        PhiReActStep::request_provider_with_call(
            detail,
            render::PhiProviderCall::from_parts(&self.model_defaults, self.tool_definitions()),
        )
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

    // Display rendering belongs to the step events before this point.
    pub(crate) fn commit_tool_result(&mut self, message: PhiMessage) {
        self.delta.push_message(message);
    }

    pub(crate) fn emit_warning(&mut self, message: &str) {
        let event = PhiAgentCommitEvent::WarningEmitted { message };
        self.modules.observe(&event);
    }

    fn run_python_code(&mut self, code: &str) -> Result<String, Box<dyn std::error::Error>> {
        self.modules
            .run_python_code(code)?
            .ok_or_else(|| "python runtime is not mounted on this agent".into())
    }
}

pub async fn run_agent_steps(
    session: Session,
    command: PhiAgentCommand,
    home: Arc<dyn PhiHome>,
) -> Result<AgentStepRunOutcome, Box<dyn std::error::Error>> {
    let agent = build_agent(session, command, home)?;
    Ok(agent.run_to_completion().await)
}

pub async fn yolo_agent_steps(
    session: Session,
    command: PhiAgentCommand,
    home: Arc<dyn PhiHome>,
) -> Result<AgentStepRunOutcome, Box<dyn std::error::Error>> {
    let agent = build_agent(session, command, home)?;
    Ok(agent.run_to_completed().await)
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
    let mut builder = PhiAgent::builder(session, command)
        .with_home(home)
        .prepare()?;
    let init_modules = build_init_modules(builder.context());
    let default_modules = build_default_modules(builder.context());
    let runtime_modules = build_runtime_modules(builder.context());
    builder = builder.with_module_layout(init_modules);
    builder = builder.with_module_layout(default_modules);
    builder = builder.with_module_layout(runtime_modules);

    builder.build()
}
