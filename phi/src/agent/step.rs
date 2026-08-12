use std::{future::Future, pin::Pin};

use crate::{
    agent::PhiAgentRuntime,
    expr::{PhiExprDelta, PhiStepExpr},
    message::{PhiHistory, PhiMessage, PhiToolResultMessage},
    module::PhiAgentStepEvent,
    render::{PhiModelResponse, PhiModelTurnState, PhiProviderCall},
    session::{PhiAgentStep, PhiReActStep},
};

// StepCont is the internal continuation type used by Phi's step interpreter
// to finish the current step after async work returns.
pub(crate) struct StepCont(
    Box<dyn FnOnce(PhiAgentRuntime) -> Pin<Box<dyn Future<Output = StepBounce> + Send>> + Send>,
);

impl StepCont {
    pub(crate) fn new<F>(cont: F) -> Self
    where
        F: FnOnce(PhiAgentRuntime) -> Pin<Box<dyn Future<Output = StepBounce> + Send>>
            + Send
            + 'static,
    {
        Self(Box::new(cont))
    }

    pub(crate) async fn call(self, runtime: PhiAgentRuntime) -> StepBounce {
        (self.0)(runtime).await
    }
}

pub(crate) struct StepInterveneNext(
    Box<dyn FnOnce(PhiAgentRuntime, StepCont) -> StepInterveneResult + Send>,
);

impl StepInterveneNext {
    pub(crate) fn new<F>(next: F) -> Self
    where
        F: FnOnce(PhiAgentRuntime, StepCont) -> StepInterveneResult + Send + 'static,
    {
        Self(Box::new(next))
    }

    pub(crate) fn call(self, runtime: PhiAgentRuntime, cont: StepCont) -> StepInterveneResult {
        (self.0)(runtime, cont)
    }
}

pub(crate) enum StepBounce {
    ContEval(PhiAgentRuntime, StepCont),
    CreateNextStep(PhiAgentRuntime, PhiReActStep),
    ReplaceBaseStep(PhiAgentRuntime, PhiReActStep),
    RuntimeFailed(PhiAgentRuntime, crate::error::PhiAgentRuntimeError),
    RollbackStep(PhiAgentRuntime),
    KeepBaseStep(PhiAgentRuntime),
}

pub(crate) struct RuntimeFailureStep(crate::error::PhiAgentRuntimeError);

impl RuntimeFailureStep {
    pub(crate) fn into_error(self) -> crate::error::PhiAgentRuntimeError {
        self.0
    }
}

pub(crate) struct StepInterveneError {
    runtime: PhiAgentRuntime,
    error: crate::error::PhiAgentRuntimeError,
}

impl StepInterveneError {
    #[cfg(test)]
    pub(crate) fn new(runtime: PhiAgentRuntime, error: crate::error::PhiAgentRuntimeError) -> Self {
        Self { runtime, error }
    }
}

impl std::fmt::Debug for StepInterveneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepInterveneError")
            .field("error", &self.error)
            .finish()
    }
}

pub(crate) type StepInterveneResult = Result<StepBounce, StepInterveneError>;

fn restore_module(
    mut bounce: StepBounce,
    index: usize,
    module: Box<dyn crate::module::DynPhiModule>,
) -> StepBounce {
    match &mut bounce {
        StepBounce::ContEval(runtime, ..)
        | StepBounce::CreateNextStep(runtime, ..)
        | StepBounce::ReplaceBaseStep(runtime, ..)
        | StepBounce::RuntimeFailed(runtime, ..)
        | StepBounce::RollbackStep(runtime)
        | StepBounce::KeepBaseStep(runtime) => {
            runtime.modules.restore_module(index, module);
        }
    }
    bounce
}

fn appended_history_tail(base: &PhiHistory, updated: &PhiHistory) -> Vec<PhiMessage> {
    let base_messages = base.to_messages();
    let updated_messages = updated.to_messages();
    if updated_messages.len() < base_messages.len()
        || updated_messages[..base_messages.len()] != base_messages[..]
    {
        return Vec::new();
    }

    updated_messages
        .into_iter()
        .skip(base_messages.len())
        .collect()
}

impl PhiAgentRuntime {
    fn handle_bounce_transition(
        &mut self,
        step: &PhiReActStep,
        delta: &mut PhiExprDelta,
        replace_base: bool,
    ) -> crate::error::PhiAgentRuntimeResult<()> {
        let base_expr = self.base.clone();
        let mut event = if replace_base {
            PhiAgentStepEvent::BeforeReplaceBaseStep {
                base_expr: &base_expr,
                step,
                delta,
            }
        } else {
            PhiAgentStepEvent::BeforeCreateNextStep {
                base_expr: &base_expr,
                step,
                delta,
            }
        };
        self.modules.handle(&mut event)
    }

    fn compact_resume_step(&self) -> PhiReActStep {
        self.find_ancestor(|step| {
            matches!(
                step,
                PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. })
            )
        })
        .and_then(|ancestor| ancestor.step().react().cloned())
        .unwrap_or_else(|| self.request_provider_step("ready"))
    }

    fn continue_failed(self, error: crate::error::PhiAgentRuntimeError) -> StepBounce {
        StepBounce::RuntimeFailed(self, error)
    }

    pub(super) async fn run_step(self) -> Self {
        let mut runtime = self;
        runtime.delta = PhiExprDelta::default();
        let bounce = runtime.eval_step_with_modules(terminal_cont(), 0);
        Self::bounce(bounce).await
    }

    async fn bounce(mut bounce: StepBounce) -> Self {
        loop {
            match bounce {
                StepBounce::ContEval(runtime, cont) => bounce = cont.call(runtime).await,
                StepBounce::CreateNextStep(mut runtime, step) => {
                    let base_expr = runtime.base.clone();
                    let mut event = PhiAgentStepEvent::BeforeCreateNextStep {
                        base_expr: &base_expr,
                        step: &step,
                        delta: &mut runtime.delta,
                    };
                    if let Err(error) = runtime.modules.handle(&mut event) {
                        bounce = runtime.continue_failed(error);
                        continue;
                    }
                    if let Err(error) = crate::session::validate_react_step(&step) {
                        bounce = runtime.continue_failed(error);
                        continue;
                    }
                    let delta = std::mem::take(&mut runtime.delta);
                    let base = std::mem::replace(&mut runtime.base, PhiStepExpr::empty_root());
                    runtime.base = base.create_next_step(PhiAgentStep::ReAct(step), delta);
                    if let Some(error) = runtime.base.step().error() {
                        let event = crate::module::PhiAgentCommitEvent::StepFailed { error };
                        runtime.modules.observe(&event);
                    }
                    return runtime;
                }
                StepBounce::ReplaceBaseStep(mut runtime, step) => {
                    let current_delta = std::mem::take(&mut runtime.delta);
                    let mut delta = runtime.base.delta().clone().then(current_delta);
                    if let Err(error) = runtime.handle_bounce_transition(&step, &mut delta, true) {
                        bounce = runtime.continue_failed(error);
                        continue;
                    }
                    if let Err(error) = crate::session::validate_react_step(&step) {
                        bounce = runtime.continue_failed(error);
                        continue;
                    }
                    if matches!(step, PhiReActStep::Compacted) && runtime.base.expr().is_none() {
                        bounce =
                            runtime.continue_failed(crate::error::PhiAgentRuntimeError::session(
                                "compacted frame must preserve a parent expr",
                            ));
                        continue;
                    }
                    let base = std::mem::replace(&mut runtime.base, PhiStepExpr::empty_root());
                    runtime.base =
                        base.replace_base_step_with_delta(PhiAgentStep::ReAct(step), delta);
                    if let Some(error) = runtime.base.step().error() {
                        let event = crate::module::PhiAgentCommitEvent::StepFailed { error };
                        runtime.modules.observe(&event);
                    }
                    return runtime;
                }
                StepBounce::RuntimeFailed(mut runtime, error) => {
                    runtime.delta = PhiExprDelta::default();
                    let base = std::mem::replace(&mut runtime.base, PhiStepExpr::empty_root());
                    runtime.base = PhiStepExpr::branch(
                        base,
                        PhiAgentStep::runtime_failed(RuntimeFailureStep(error)),
                        PhiExprDelta::default(),
                    );
                    let event = crate::module::PhiAgentCommitEvent::StepFailed {
                        error: runtime
                            .base
                            .step()
                            .error()
                            .expect("runtime-failed transition must create a failed step"),
                    };
                    runtime.modules.observe(&event);
                    return runtime;
                }
                StepBounce::RollbackStep(mut runtime) => {
                    if let Some(parent) = runtime.base.expr().cloned() {
                        runtime.base = parent;
                    }
                    runtime.delta = PhiExprDelta::default();
                    return runtime;
                }
                StepBounce::KeepBaseStep(mut runtime) => {
                    runtime.delta = PhiExprDelta::default();
                    return runtime;
                }
            }
        }
    }

    pub(crate) fn eval_step_with_modules(mut self, cont: StepCont, index: usize) -> StepBounce {
        if index >= self.modules.len() {
            return match self.base_step().clone() {
                PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate }) => {
                    self.step_request_compact(cont, retain_rate)
                }
                PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. }) => {
                    self.request_provider(cont)
                }
                PhiAgentStep::ReAct(PhiReActStep::RequestExecutor {
                    pending_messages,
                    assistant,
                    pending_results,
                    ..
                }) => self.step_tool(pending_messages, assistant, pending_results, cont),
                // TurnEnd is a pure step-level transition: once a turn has
                // finished cleanly, the next explicit step should mechanically
                // resume from a fresh completion request.
                PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. }) => {
                    let step = self.request_provider_step("resuming from turn-end step");
                    StepBounce::CreateNextStep(self, step)
                }

                // Failed intentionally does nothing at the core step level.
                // Any "resume failed session" behavior must come from a
                // module so callers still have a chance to inspect or rewrite
                // the failed session before Phi falls back.
                // do nothing
                PhiAgentStep::Failed(_) => StepBounce::KeepBaseStep(self),

                PhiAgentStep::ReAct(PhiReActStep::Compacted) => {
                    let step = self.compact_resume_step();
                    StepBounce::CreateNextStep(self, step)
                }
            };
        }

        let module = self.modules.take_module(index);
        let next = StepInterveneNext::new(move |runtime, cont| {
            Ok(StepBounce::ContEval(
                runtime,
                StepCont::new(move |runtime| {
                    Box::pin(async move { runtime.eval_step_with_modules(cont, index + 1) })
                }),
            ))
        });

        let mut module = module;
        match module.intervene(self, cont, next) {
            Ok(bounce) => restore_module(bounce, index, module),
            Err(StepInterveneError { runtime, error }) => {
                let bounce = runtime.continue_failed(error);
                restore_module(bounce, index, module)
            }
        }
    }

    fn step_request_compact(self, _cont: StepCont, retain_rate: f32) -> StepBounce {
        StepBounce::ContEval(
            self,
            StepCont::new(move |runtime| {
                Box::pin(async move {
                    let mut runtime = runtime;
                    let mut request =
                        PhiProviderCall::from_parts(&runtime.model_defaults, Vec::new());
                    let mut history = runtime.history();
                    let step = runtime.base_step().clone();
                    let expr = runtime.base_expr().clone();
                    let mut before_event = PhiAgentStepEvent::BeforeCompactRequest {
                        step: &step,
                        expr: &expr,
                        history: &mut history,
                        request: &mut request,
                    };

                    if let Err(error) = runtime.modules.handle(&mut before_event) {
                        return runtime.continue_failed(error);
                    }

                    match runtime.render.compact(request, history, retain_rate).await {
                        Ok(mut history) => {
                            let mut after_event = PhiAgentStepEvent::AfterCompactResponse {
                                history: &mut history,
                            };
                            if let Err(error) = runtime.modules.handle(&mut after_event) {
                                return runtime.continue_failed(error);
                            }
                            runtime.delta = history.into();
                            StepBounce::CreateNextStep(runtime, PhiReActStep::Compacted)
                        }
                        Err(crate::error::PhiAgentRuntimeError::ContextExceededLimit {
                            detail,
                        }) => runtime.continue_failed(
                            crate::error::PhiAgentRuntimeError::context_exceeded_limit(detail),
                        ),
                        Err(error) => runtime.continue_failed(
                            crate::error::PhiAgentRuntimeError::request_compact(error.detail()),
                        ),
                    }
                })
            }),
        )
    }

    fn request_provider(mut self, _cont: StepCont) -> StepBounce {
        let original_history = self.history();
        let mut request = self.request_provider_request(
            self.base_step()
                .request_provider_call()
                .cloned()
                .expect("request_provider requires a request_provider step"),
        );
        let mut request_history = original_history.clone();
        let step = self.base_step().clone();
        let expr = self.base_expr().clone();
        let mut before_event = PhiAgentStepEvent::BeforeModelRequest {
            step: &step,
            expr: &expr,
            history: &mut request_history,
            request: &mut request,
        };
        if let Err(failure) = self.modules.handle(&mut before_event) {
            return self.continue_failed(failure);
        }
        let request_history_tail = appended_history_tail(&original_history, &request_history);

        StepBounce::ContEval(
            self,
            StepCont::new(move |mut runtime| {
                Box::pin(async move {
                    let PhiModelResponse {
                        mut assistant,
                        turn_state,
                    } = match runtime.render.complete(&request, &request_history).await {
                        Ok(response_messages) => response_messages,
                        Err(failure) => {
                            return runtime.continue_failed(failure);
                        }
                    };

                    if let Some(assistant) = &mut assistant {
                        let mut after_event = PhiAgentStepEvent::AfterModelResponse { assistant };
                        if let Err(failure) = runtime.modules.handle(&mut after_event) {
                            return runtime.continue_failed(failure);
                        }
                    }

                    let mut parsed_messages = Vec::new();
                    if let Some(assistant) = &assistant {
                        parsed_messages.push(PhiMessage::Assistant(assistant.clone()));
                    }
                    let mut after_response_event = PhiAgentStepEvent::AfterModelResponseParsed {
                        messages: &parsed_messages,
                    };
                    if let Err(failure) = runtime.modules.handle(&mut after_response_event) {
                        return runtime.continue_failed(failure);
                    }

                    if assistant
                        .as_ref()
                        .is_some_and(|message| !message.tool_calls.is_empty())
                    {
                        let step = match PhiReActStep::request_executor_turn(
                            "tool execution is pending",
                            request_history_tail.clone(),
                            assistant.expect("tool calls require an assistant response"),
                            Vec::new(),
                        ) {
                            Ok(step) => step,
                            Err(error) => return runtime.continue_failed(error),
                        };
                        return StepBounce::CreateNextStep(runtime, step);
                    }

                    for message in request_history_tail.clone() {
                        runtime.commit_message(message);
                    }
                    if let Some(assistant) = assistant {
                        runtime.commit_model_response(assistant);
                    }
                    if turn_state == PhiModelTurnState::Continue {
                        let step = runtime.request_provider_step(
                            "provider response requires another model request",
                        );
                        return StepBounce::CreateNextStep(runtime, step);
                    }
                    StepBounce::CreateNextStep(
                        runtime,
                        PhiReActStep::turn_end(
                            "model response committed; no tool execution is pending",
                        ),
                    )
                })
            }),
        )
    }

    fn step_tool(
        mut self,
        pending_messages: Vec<PhiMessage>,
        mut assistant: crate::message::PhiAssistantMessage,
        mut pending_results: Vec<PhiToolResultMessage>,
        _cont: StepCont,
    ) -> StepBounce {
        let Some(mut request) = assistant.tool_calls.get(pending_results.len()).cloned() else {
            let step = self.request_provider_step("no tool execution is pending");
            return StepBounce::ReplaceBaseStep(self, step);
        };
        let step = self.base_step().clone();
        let expr = self.base_expr().clone();
        let mut before_event = PhiAgentStepEvent::BeforeToolCall {
            step: &step,
            expr: &expr,
            request: &mut request,
        };
        if let Err(failure) = self.modules.handle(&mut before_event) {
            return self.continue_failed(failure);
        }

        StepBounce::ContEval(
            self,
            StepCont::new(move |mut runtime| {
                Box::pin(async move {
                    let (request, mut response) =
                        match runtime.executor.call_tool(request.clone(), &runtime).await {
                            Ok(outcome) => outcome,
                            Err(failure) => {
                                let (detail, request, not_found) = match failure {
                                    crate::executor::PhiToolExecutionError::NotFound {
                                        detail,
                                        request,
                                    } => (serde_json::Value::String(detail), request, true),
                                    crate::executor::PhiToolExecutionError::Failed {
                                        detail,
                                        request,
                                    } => (detail, request, false),
                                };
                                let mut failed_assistant = assistant.clone();
                                let Some(failed_request) =
                                    failed_assistant.tool_calls.get_mut(pending_results.len())
                                else {
                                    return runtime.continue_failed(
                                        crate::error::PhiAgentRuntimeError::session(
                                            "failed tool turn has no current request",
                                        ),
                                    );
                                };
                                *failed_request = request;
                                let turn = match crate::error::PhiFailedToolTurn::new(
                                    pending_messages.clone(),
                                    failed_assistant,
                                    pending_results.clone(),
                                ) {
                                    Ok(turn) => turn,
                                    Err(error) => return runtime.continue_failed(error),
                                };
                                let failure = if not_found {
                                    crate::error::PhiAgentRuntimeError::tool_not_found(
                                        detail.as_str().unwrap_or("unknown tool"),
                                        turn,
                                    )
                                } else {
                                    crate::error::PhiAgentRuntimeError::tool_error(detail, turn)
                                };
                                return runtime.continue_failed(failure);
                            }
                        };

                    let mut after_event = PhiAgentStepEvent::AfterToolCall {
                        request: &request,
                        result: &mut response,
                    };
                    if let Err(failure) = runtime.modules.handle(&mut after_event) {
                        return runtime.continue_failed(failure);
                    }

                    let resume_call = runtime.request_provider_call();
                    let Some(current_request) = assistant.tool_calls.get_mut(pending_results.len())
                    else {
                        return runtime.continue_failed(
                            crate::error::PhiAgentRuntimeError::session(
                                "completed tool turn has no current request",
                            ),
                        );
                    };
                    *current_request = request.clone();
                    pending_results.push(PhiToolResultMessage {
                        id: request.call_id.clone().or(Some(request.id.clone())),
                        name: Some(request.name.clone()),
                        result: serde_json::to_value(&response.output)
                            .expect("tool output should serialize into history"),
                    });
                    let resolution = match crate::session::resolve_tool_result(
                        pending_messages,
                        assistant,
                        pending_results,
                        resume_call,
                    ) {
                        Ok(resolved) => resolved,
                        Err(error) => return runtime.continue_failed(error),
                    };
                    match resolution {
                        crate::session::ToolResultResolution::Pending(step) => {
                            StepBounce::ReplaceBaseStep(runtime, step)
                        }
                        crate::session::ToolResultResolution::Complete { messages, step } => {
                            for message in messages {
                                match message {
                                    PhiMessage::Assistant(assistant) => {
                                        runtime.commit_model_response(assistant)
                                    }
                                    PhiMessage::ToolResult(_) => {
                                        runtime.commit_tool_result(message)
                                    }
                                    _ => runtime.commit_message(message),
                                }
                            }
                            StepBounce::CreateNextStep(runtime, step)
                        }
                    }
                })
            }),
        )
    }
}

fn terminal_cont() -> StepCont {
    StepCont::new(|_runtime| {
        Box::pin(async move {
            unreachable!("terminal continuation should never be polled after step finalization")
        })
    })
}
