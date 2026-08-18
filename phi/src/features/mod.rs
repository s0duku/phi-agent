pub mod governance;
mod observers;

use std::io::IsTerminal;

use crate::{
    agent::{
        PhiAgentBuildContext, PhiAgentCommand, PhiAgentRuntime, StepBounce, StepCont,
        StepInterveneNext,
    },
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    executor::ToolCallOutput,
    message::{PhiMessage, PhiToolResultMessage},
    module::{PhiModule, PhiModuleLayout},
    session::{PhiAgentStep, PhiReActStep},
};
use governance::auto_compact::AutoCompactPolicy;
#[allow(unused_imports)]
pub(crate) use governance::model_retry::ModelRetryPolicy;
pub use observers::echo::pretty_history;
pub(crate) use observers::echo::{pretty_message, pretty_warning};

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/system.txt");

pub(crate) fn build_default_modules(context: &PhiAgentBuildContext) -> PhiModuleLayout {
    let max_steps =
        command_max_steps(&context.command).unwrap_or_else(|| context.config().runtime().max_steps);
    let mut modules = PhiModuleLayout::default();
    modules.push_governance(Box::new(governance::step_budget::StepBudgetPolicy::new(
        max_steps,
    )));
    if let Some(max_model_request_retries) = command_max_model_request_retries(&context.command) {
        modules.push_governance(Box::new(governance::model_retry::ModelRetryPolicy::new(
            max_model_request_retries,
        )));
    }

    let context_tokens = context.config().runtime().context_tokens;
    modules.push_governance(Box::new(AutoCompactPolicy::new(context_tokens)));

    modules.push_recovery(Box::new(DefaultFailedRecoveryModule));

    modules
}

pub(crate) fn build_init_modules(context: &PhiAgentBuildContext) -> PhiModuleLayout {
    if context.session.history().is_empty() && command_requires_user_input(&context.command) {
        let mut modules = PhiModuleLayout::default();
        modules.push_init(Box::new(EmptySessionGuardModule));
        return modules;
    }
    PhiModuleLayout::default()
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
    modules
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
    match settings.runtime().system.as_deref() {
        // An explicitly configured empty prompt remains a real system message.
        Some(system) => Some(system.trim().to_string()),
        None => {
            let prompt = DEFAULT_SYSTEM_PROMPT.trim();
            (!prompt.is_empty()).then(|| prompt.to_string())
        }
    }
}

struct EmptySessionGuardModule;
struct DefaultFailedRecoveryModule;

impl PhiModule for EmptySessionGuardModule {
    fn init_context(&mut self, _context: &mut PhiAgentBuildContext) -> PhiAgentRuntimeResult<()> {
        Err(crate::error::PhiAgentRuntimeError::session(
            "session is empty; provide --user/--assistant",
        ))
    }
}

impl PhiModule for DefaultFailedRecoveryModule {
    fn intervene(
        &mut self,
        mut runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        if let PhiAgentStep::Failed(failed) = runtime.base_step()
            && matches!(
                failed.error(),
                PhiAgentRuntimeError::ContextExceededLimit { .. }
            )
            && let Some(parent) = runtime.base_expr().expr()
        {
            let next_rate = match parent.step() {
                PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. }) => 0.1,
                PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate }) => {
                    *retain_rate + 0.05
                }
                _ => return Ok(StepBounce::KeepBaseStep(runtime)),
            };
            if next_rate > 0.5 {
                runtime
                    .emit_warning("compact retained-history limit reached; preserving failed step");
                return Ok(StepBounce::KeepBaseStep(runtime));
            }
            if matches!(
                parent.step(),
                PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. })
            ) {
                runtime.emit_warning(
                    "provider request exceeded the context limit; recovering through compact with 10% retained history",
                );
            } else {
                let retained_percent = (next_rate * 100.0).round() as u32;
                runtime.emit_warning(&format!(
                    "compact request exceeded the provider context limit; retrying with {retained_percent}% retained history"
                ));
            }
            return Ok(StepBounce::ReplaceBaseStep(
                runtime,
                PhiReActStep::request_compact_with_retain_rate(next_rate),
            ));
        }

        if let PhiAgentStep::Failed(failed) = runtime.base_step().clone()
            && let error = failed.error()
            && matches!(
                error,
                PhiAgentRuntimeError::ToolNotFound { .. } | PhiAgentRuntimeError::ToolError { .. }
            )
            && let Some(tool_request) = error.tool_request().cloned()
        {
            let output =
                ToolCallOutput::new(error.tool_error_detail().cloned().unwrap_or_else(|| {
                    serde_json::json!({
                        "error": error.detail(),
                        "kind": "tool_not_found",
                        "tool_name": tool_request.name.clone(),
                    })
                }));
            let rendered_output = serde_json::to_string(&output)
                .expect("tool-not-found recovery output should serialize");
            runtime.emit_warning(&format!(
                "recovered failed tool call by committing a structured result for {}: {}",
                tool_request.name, rendered_output
            ));
            let pending_messages = error.pending_messages().unwrap_or_default().to_vec();
            for message in pending_messages {
                runtime.commit_message(message);
            }
            let mut assistant = error
                .assistant()
                .cloned()
                .expect("persisted tool failures must carry their assistant turn");
            let completed = error
                .pending_results()
                .expect("persisted tool failures must carry completed results")
                .to_vec();
            let keep_calls = completed.len().saturating_add(1);
            assistant.tool_calls.truncate(keep_calls);
            if assistant.tool_calls.len() <= completed.len() {
                assistant.tool_calls.push(tool_request.clone());
            } else {
                assistant.tool_calls[completed.len()] = tool_request.clone();
            }
            runtime.commit_model_response(assistant);
            for result in completed {
                runtime.commit_tool_result(PhiMessage::ToolResult(result));
            }
            let message = PhiMessage::ToolResult(PhiToolResultMessage {
                id: tool_request
                    .call_id
                    .clone()
                    .or(Some(tool_request.id.clone())),
                name: Some(tool_request.name.clone()),
                result: serde_json::to_value(&output)
                    .expect("tool-not-found recovery output should serialize"),
            });
            runtime.commit_tool_result(message);
            let step =
                runtime.request_provider_step("tool result committed; model response is pending");
            return Ok(StepBounce::ReplaceBaseStep(runtime, step));
        }

        if matches!(runtime.base_step(), PhiAgentStep::Failed(_))
            && runtime.base_expr().expr().is_some()
        {
            runtime.emit_warning("rolling back failed step");
            return Ok(StepBounce::RollbackStep(runtime));
        }
        next.call(runtime, cont)
    }
}

fn command_requires_user_input(command: &PhiAgentCommand) -> bool {
    matches!(
        command,
        PhiAgentCommand::Run(_) | PhiAgentCommand::Yolo(_) | PhiAgentCommand::Step(_)
    )
}

fn command_max_steps(command: &PhiAgentCommand) -> Option<usize> {
    command.max_steps()
}

fn command_verbose(command: &PhiAgentCommand) -> bool {
    command.verbose()
}

fn command_message_sender(
    _command: &PhiAgentCommand,
) -> Option<tokio::sync::mpsc::UnboundedSender<crate::message::PhiMessage>> {
    None
}

fn command_max_model_request_retries(command: &PhiAgentCommand) -> Option<usize> {
    command.max_model_request_retries()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::{
        error::PhiAgentRuntimeError,
        expr::PhiStepExpr,
        message::PhiHistory,
        render::{PhiModelResponse, PhiProviderCall, TestClient},
        session::{PhiAgentStep, PhiReActStep, Session},
        tests::support::{default_step_agent_builder, test_model_defaults},
    };

    struct ContextExceededProvider;

    #[async_trait]
    impl TestClient for ContextExceededProvider {
        async fn complete(
            &self,
            _request: &PhiProviderCall,
            _messages: &PhiHistory,
        ) -> crate::error::PhiAgentRuntimeResult<PhiModelResponse> {
            Err(PhiAgentRuntimeError::context_exceeded_limit(
                "provider context overflow",
            ))
        }
    }

    #[tokio::test]
    async fn provider_context_overflow_enters_compact_instead_of_rollback_loop() {
        let session = Session::from_root(
            PhiAgentStep::request_provider("ready", &test_model_defaults()),
            vec![crate::message::PhiMessage::user("large history")],
        );
        let mut agent = default_step_agent_builder(session)
            .with_client(Arc::new(ContextExceededProvider))
            .build()
            .unwrap();

        agent.step().await;
        assert!(matches!(
            agent.session().step(),
            PhiAgentStep::Failed(failed)
                if matches!(failed.error(), PhiAgentRuntimeError::ContextExceededLimit { .. })
        ));

        agent.step().await;
        let expr = agent.into_session().into_expr();
        assert!(matches!(
            expr.step(),
            PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate: 0.1 })
        ));
        assert!(matches!(
            expr.expr().map(PhiStepExpr::step),
            Some(PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. }))
        ));
    }

    #[tokio::test]
    async fn provider_context_overflow_replaces_failed_with_initial_compact_step() {
        let provider = PhiStepExpr::new(
            PhiAgentStep::request_provider("ready", &test_model_defaults()),
            Vec::new(),
        );
        let session = Session::from_expr(provider.branch_failed(
            PhiAgentRuntimeError::context_exceeded_limit("context overflow"),
        ));

        let outcome = default_step_agent_builder(session)
            .build()
            .unwrap()
            .run_single_step()
            .await
            .session;
        let expr = outcome.into_expr();

        assert!(matches!(
            expr.step(),
            PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate: 0.1 })
        ));
        assert!(matches!(
            expr.expr().map(PhiStepExpr::step),
            Some(PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. }))
        ));
        assert!(expr.expr().and_then(PhiStepExpr::expr).is_none());
    }

    #[tokio::test]
    async fn compact_context_overflow_replaces_failed_with_higher_retain_rate() {
        let provider = PhiStepExpr::new(
            PhiAgentStep::request_provider("ready", &test_model_defaults()),
            Vec::new(),
        );
        let compact = PhiStepExpr::branch(
            provider,
            PhiAgentStep::request_compact(),
            crate::expr::PhiExprDelta::default(),
        );
        let session = Session::from_expr(compact.branch_failed(
            PhiAgentRuntimeError::context_exceeded_limit("context overflow"),
        ));

        let outcome = default_step_agent_builder(session)
            .build()
            .unwrap()
            .run_single_step()
            .await
            .session;
        let expr = outcome.into_expr();

        assert!(matches!(
            expr.step(),
            PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate })
                if (*retain_rate - 0.15).abs() < f32::EPSILON
        ));
        assert!(matches!(
            expr.expr().map(PhiStepExpr::step),
            Some(PhiAgentStep::ReAct(PhiReActStep::RequestCompact {
                retain_rate: 0.1
            }))
        ));
        assert!(matches!(
            expr.expr()
                .and_then(PhiStepExpr::expr)
                .map(PhiStepExpr::step),
            Some(PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. }))
        ));
        assert!(
            expr.expr()
                .and_then(PhiStepExpr::expr)
                .and_then(PhiStepExpr::expr)
                .is_none()
        );
    }
}
