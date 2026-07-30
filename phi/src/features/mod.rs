pub mod governance;
mod observers;
pub(crate) mod plugin;

use std::io::IsTerminal;

use crate::{
    agent::{
        PhiAgentBuildContext, PhiAgentCommand, PhiAgentRuntime, StepBounce, StepCont,
        StepInterveneNext,
    },
    config::{default_max_steps, optional_public_usize_from_config},
    error::PhiErrorKind,
    error::PhiRuntimeResult,
    executor::ToolCallOutput,
    message::PhiMessage,
    module::{PhiModule, PhiModuleLayout},
    session::PhiAgentStep,
};
use governance::auto_compact::AutoCompactPolicy;
use governance::loop_guard::{
    LoopGuardConfig, ReasoningSimilarityConfig, default_loopguard_max_retries,
    default_loopguard_reasoning_min_chars, default_loopguard_reasoning_ngram_size,
    default_loopguard_reasoning_similarity_threshold, default_loopguard_window,
};
#[allow(unused_imports)]
pub(crate) use governance::model_retry::ModelRetryPolicy;
pub use observers::echo::pretty_history;
pub(crate) use observers::echo::{pretty_message, pretty_warning};
pub use plugin::PluginRuntimeStatus;

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/system.txt");

pub(crate) fn build_default_modules(context: &PhiAgentBuildContext) -> PhiModuleLayout {
    let max_steps = command_max_steps(&context.command)
        .or(
            optional_public_usize_from_config(context.config(), "PHI_MAX_STEPS")
                .ok()
                .flatten(),
        )
        .unwrap_or(default_max_steps());
    let mut modules = PhiModuleLayout::default();
    modules.push_governance(Box::new(governance::step_budget::StepBudgetPolicy::new(
        max_steps,
    )));
    if let Some(max_model_request_retries) = command_max_model_request_retries(&context.command) {
        modules.push_governance(Box::new(governance::model_retry::ModelRetryPolicy::new(
            max_model_request_retries,
        )));
    }

    // Loop detection is a built-in governance policy, not a user-tunable
    // runtime switch. Keep it always mounted from source-level defaults.
    modules.push_governance(Box::new(governance::loop_guard::LoopGuardPolicy::new(
        loop_guard_config(),
    )));
    let context_tokens = optional_public_usize_from_config(context.config(), "PHI_CONTEXT_TOKENS")
        .ok()
        .flatten()
        .unwrap_or(crate::config::default_context_tokens());
    modules.push_governance(Box::new(AutoCompactPolicy::new(context_tokens)));

    modules.push_recovery(Box::new(DefaultFailedRecoveryModule));

    modules
}

pub(crate) fn build_init_modules(context: &PhiAgentBuildContext) -> PhiModuleLayout {
    let messages = bootstrap_messages(context);
    if messages.is_empty() {
        if context.session.history().is_empty() && command_requires_user_input(&context.command) {
            let mut modules = PhiModuleLayout::default();
            modules.push_init(Box::new(EmptySessionGuardModule));
            return modules;
        }
        return PhiModuleLayout::default();
    }

    let mut modules = PhiModuleLayout::default();
    modules.push_init(Box::new(CommandInputModule {
        messages,
        verbose: command_verbose(&context.command),
    }));
    modules
}

pub(crate) fn build_runtime_modules(context: &PhiAgentBuildContext) -> PhiModuleLayout {
    let mut modules = PhiModuleLayout::default();
    if command_verbose(&context.command) {
        if std::io::stderr().is_terminal() {
            modules.push_observer(Box::new(observers::spinner::SpinnerModule::new()));
        }
        modules.push_observer(Box::new(observers::echo::EchoModule::new()));
    }
    if let Some(sender) = command_message_sender(&context.command) {
        modules.push_observer(Box::new(observers::channel::ChannelModule::new(sender)));
    }
    if let Some(plugin_module) = plugin::build_plugin_module(context) {
        // Plugins are mounted after Phi's built-in runtime observers so the
        // default UX stays stable, but still before recovery modules so
        // plugins may override Phi's fallback failed-session behavior.
        modules.push_extension(plugin_module);
    }
    modules
}

pub fn plugin_runtime_status() -> PluginRuntimeStatus {
    plugin::python_plugin_status()
}

pub(crate) fn pretty_info(message: &str) -> String {
    observers::echo::pretty_info(message)
}

pub fn configured_system_prompt(context: &PhiAgentBuildContext) -> Option<String> {
    configured_system_prompt_from_config(context.config())
}

pub(crate) fn configured_system_prompt_from_config(
    settings: &crate::config::PhiConfig,
) -> Option<String> {
    settings
        .get("PHI_SYSTEM")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let prompt = DEFAULT_SYSTEM_PROMPT.trim();
            (!prompt.is_empty()).then(|| prompt.to_string())
        })
}

struct CommandInputModule {
    messages: Vec<PhiMessage>,
    verbose: bool,
}

struct EmptySessionGuardModule;
struct DefaultFailedRecoveryModule;

impl PhiModule for CommandInputModule {
    type ProbInfo = ();

    fn init_context(&mut self, context: &mut PhiAgentBuildContext) -> PhiRuntimeResult<()> {
        context.bootstrap_messages(self.messages.drain(..), self.verbose)
    }
}

impl PhiModule for EmptySessionGuardModule {
    type ProbInfo = ();

    fn init_context(&mut self, _context: &mut PhiAgentBuildContext) -> PhiRuntimeResult<()> {
        Err(crate::error::PhiRuntimeError::session(
            "session is empty; provide --user/--assistant",
        ))
    }
}

impl PhiModule for DefaultFailedRecoveryModule {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        mut runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        if let PhiAgentStep::Failed { error } = runtime.base_step().clone()
            && error.kind() == PhiErrorKind::ToolNotFound
            && error.source_step() == Some("step_tool")
            && let Some(tool_request) = error.tool_request().cloned()
        {
            let output = ToolCallOutput::failure(
                error.detail(),
                serde_json::json!({
                    "kind": "tool_not_found",
                    "tool_name": tool_request.name.clone(),
                }),
            );
            let rendered_output = serde_json::to_string(&output)
                .expect("tool-not-found recovery output should serialize");
            runtime.emit_warning(&format!(
                "recovered failed tool call by committing a structured tool_not_found result for {}: {}",
                tool_request.name, rendered_output
            ));
            let pending_messages = error.pending_messages().unwrap_or_default().to_vec();
            for message in pending_messages {
                runtime.commit_message(message);
            }
            runtime.commit_message(PhiMessage::tool_call(
                tool_request
                    .call_id
                    .clone()
                    .or(Some(tool_request.id.clone())),
                tool_request.name.clone(),
                tool_request.arguments.clone(),
            ));
            let message = PhiMessage::tool_result(
                tool_request
                    .call_id
                    .clone()
                    .or(Some(tool_request.id.clone())),
                Some(tool_request.name.clone()),
                serde_json::to_value(&output)
                    .expect("tool-not-found recovery output should serialize"),
            );
            runtime.commit_tool_result(message);
            let delta = if runtime.cur_delta().is_empty() {
                runtime.base_delta().clone()
            } else {
                runtime.cur_delta().clone()
            };
            let step =
                runtime.request_provider_step("tool result committed; model response is pending");
            return Ok(StepBounce::ReplaceBaseStep(runtime, step, delta));
        }

        if let PhiAgentStep::Failed { .. } = runtime.base_step().clone() {
            runtime.emit_warning("resuming from failed step");
            let delta = if runtime.cur_delta().is_empty() {
                runtime.base_delta().clone()
            } else {
                runtime.cur_delta().clone()
            };
            let step = runtime.request_provider_step("resuming from failed step");
            return Ok(StepBounce::ReplaceBaseStep(runtime, step, delta));
        }
        next.call(runtime, cont)
    }
}

fn bootstrap_messages(context: &PhiAgentBuildContext) -> Vec<PhiMessage> {
    if matches!(context.command, PhiAgentCommand::Probe(_)) {
        return Vec::new();
    }

    let mut messages = Vec::new();
    if context.session.history().is_empty()
        && let Some(system_prompt) = configured_system_prompt(context)
    {
        messages.push(PhiMessage::system(system_prompt));
    }
    messages.extend(command_input_messages(&context.command));
    messages
}

fn command_requires_user_input(command: &PhiAgentCommand) -> bool {
    matches!(
        command,
        PhiAgentCommand::Run(_) | PhiAgentCommand::Yolo(_) | PhiAgentCommand::Step(_)
    )
}

fn loop_guard_config() -> LoopGuardConfig {
    let config = LoopGuardConfig {
        window: default_loopguard_window(),
        max_retries: default_loopguard_max_retries(),
        reasoning: Some(ReasoningSimilarityConfig {
            ngram_size: default_loopguard_reasoning_ngram_size(),
            similarity_threshold: default_loopguard_reasoning_similarity_threshold(),
            min_chars: default_loopguard_reasoning_min_chars(),
        }),
    };

    debug_assert!(config.window > 0, "loop guard window must stay non-zero");
    if let Some(reasoning) = &config.reasoning {
        debug_assert!(
            reasoning.ngram_size > 0,
            "loop guard reasoning ngram size must stay non-zero"
        );
        debug_assert!(
            (0.0..=1.0).contains(&reasoning.similarity_threshold),
            "loop guard reasoning similarity threshold must stay within [0, 1]"
        );
    }

    config
}

fn command_max_steps(command: &PhiAgentCommand) -> Option<usize> {
    match command {
        PhiAgentCommand::Run(command) | PhiAgentCommand::Yolo(command) => command.max_steps,
        _ => None,
    }
}

fn command_verbose(command: &PhiAgentCommand) -> bool {
    match command {
        PhiAgentCommand::Run(command) | PhiAgentCommand::Yolo(command) => !command.quiet,
        PhiAgentCommand::Step(command) => !command.quiet,
        _ => false,
    }
}

fn command_message_sender(
    _command: &PhiAgentCommand,
) -> Option<tokio::sync::mpsc::UnboundedSender<crate::message::PhiMessage>> {
    None
}

fn command_max_model_request_retries(command: &PhiAgentCommand) -> Option<usize> {
    match command {
        PhiAgentCommand::Run(command) | PhiAgentCommand::Yolo(command) => {
            command.max_model_request_retries
        }
        PhiAgentCommand::Step(command) => command.max_model_request_retries,
        PhiAgentCommand::Probe(command) => command.max_model_request_retries,
        _ => None,
    }
}

fn command_input_messages(command: &PhiAgentCommand) -> Vec<PhiMessage> {
    match command {
        PhiAgentCommand::Run(command) | PhiAgentCommand::Yolo(command) => {
            command.input_messages.clone()
        }
        PhiAgentCommand::Step(command) => command.input_messages.clone(),
        _ => Vec::new(),
    }
}
