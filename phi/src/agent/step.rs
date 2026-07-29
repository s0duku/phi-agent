use std::{future::Future, pin::Pin};

use crate::{
    agent::PhiAgentRuntime,
    executor::ToolCallRequest,
    expr::{PhiExprDelta, PhiStepExpr},
    message::{PhiHistory, PhiMessage, PhiToolMessage},
    module::PhiAgentStepEvent,
    render::{PhiModelResponse, PhiModelTurnState, PhiProviderCall},
    session::PhiAgentStep,
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
    CreateNextStep(PhiAgentRuntime, PhiAgentStep, PhiExprDelta),
    ReplaceBaseStep(PhiAgentRuntime, PhiAgentStep, PhiExprDelta),
}

pub(crate) struct StepInterveneError {
    runtime: PhiAgentRuntime,
    error: crate::error::PhiRuntimeError,
}

impl StepInterveneError {
    pub(crate) fn new(runtime: PhiAgentRuntime, error: crate::error::PhiRuntimeError) -> Self {
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
        | StepBounce::ReplaceBaseStep(runtime, ..) => {
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
        step: &mut PhiAgentStep,
        delta: &mut PhiExprDelta,
        replace_base: bool,
    ) -> crate::error::PhiRuntimeResult<()> {
        if matches!(step, PhiAgentStep::Failed { .. }) {
            return Ok(());
        }

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

    fn compact_resume_step(&self) -> PhiAgentStep {
        self.find_ancestor(|step| matches!(step, PhiAgentStep::RequestComplete { .. }))
            .map(|ancestor| ancestor.step().clone())
            .unwrap_or_else(|| self.request_complete_step("ready"))
    }

    fn continue_failed(
        self,
        error: crate::error::PhiRuntimeError,
        source_step: &'static str,
    ) -> StepBounce {
        let delta = self.cur_delta().clone();
        StepBounce::CreateNextStep(
            self,
            PhiAgentStep::failed(error.with_source_step(source_step)),
            delta,
        )
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
                StepBounce::CreateNextStep(mut runtime, mut step, mut delta) => {
                    if let Err(error) =
                        runtime.handle_bounce_transition(&mut step, &mut delta, false)
                    {
                        bounce = runtime.continue_failed(error, "bounce_transition");
                        continue;
                    }
                    let base = std::mem::replace(&mut runtime.base, PhiStepExpr::empty_root());
                    runtime.base = PhiStepExpr::branch(base, step, delta);
                    runtime.delta = PhiExprDelta::default();
                    if let Some(error) = runtime.base.step().error() {
                        let event = crate::module::PhiAgentCommitEvent::StepFailed { error };
                        runtime.modules.observe(&event);
                    }
                    return runtime;
                }
                StepBounce::ReplaceBaseStep(mut runtime, mut step, mut delta) => {
                    if let Err(error) =
                        runtime.handle_bounce_transition(&mut step, &mut delta, true)
                    {
                        bounce = runtime.continue_failed(error, "bounce_transition");
                        continue;
                    }
                    let parent = runtime.base.expr.clone();
                    runtime.base = PhiStepExpr {
                        step,
                        delta,
                        expr: parent,
                    };
                    runtime.delta = PhiExprDelta::default();
                    if let Some(error) = runtime.base.step().error() {
                        let event = crate::module::PhiAgentCommitEvent::StepFailed { error };
                        runtime.modules.observe(&event);
                    }
                    return runtime;
                }
            }
        }
    }

    pub(crate) fn eval_step_with_modules(mut self, cont: StepCont, index: usize) -> StepBounce {
        if index >= self.modules.len() {
            return match self.base_step().clone() {
                PhiAgentStep::RequestCompact => self.step_request_compact(cont),
                PhiAgentStep::RequestComplete { .. } => self.request_complete(cont),
                PhiAgentStep::RequestExecutor {
                    pending_messages,
                    tool_calls,
                    ..
                } => self.step_tool(pending_messages, tool_calls, cont),
                // Completed is a pure step-level transition: once a turn has
                // finished cleanly, the next explicit step should mechanically
                // resume from a fresh completion request.
                PhiAgentStep::Completed { .. } => {
                    let step = self.request_complete_step("resuming from completed step");
                    let delta = self.cur_delta().clone();
                    StepBounce::CreateNextStep(self, step, delta)
                }

                // Failed intentionally does nothing at the core step level.
                // Any "resume failed session" behavior must come from a
                // module so callers still have a chance to inspect or rewrite
                // the failed session before Phi falls back.
                // do nothing
                PhiAgentStep::Failed { .. } => {
                    let step = self.base_step().clone();
                    let delta = self.base_delta().clone();
                    StepBounce::ReplaceBaseStep(self, step, delta)
                }

                PhiAgentStep::Compacted => {
                    let step = self.compact_resume_step();
                    let delta = if self.cur_delta().is_empty() {
                        self.base_delta().clone()
                    } else {
                        self.cur_delta().clone()
                    };
                    StepBounce::CreateNextStep(self, step, delta)
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
                let bounce = runtime.continue_failed(error, "intervene");
                restore_module(bounce, index, module)
            }
        }
    }

    fn step_request_compact(self, _cont: StepCont) -> StepBounce {
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
                        return runtime.continue_failed(error, "step_request_compact");
                    }

                    match runtime.render.compact(request, history).await {
                        Ok(mut history) => {
                            let mut after_event = PhiAgentStepEvent::AfterCompactResponse {
                                history: &mut history,
                            };
                            if let Err(error) = runtime.modules.handle(&mut after_event) {
                                return runtime.continue_failed(error, "step_request_compact");
                            }
                            runtime.delta = history.into();
                            let delta = runtime.cur_delta().clone();
                            StepBounce::CreateNextStep(runtime, PhiAgentStep::Compacted, delta)
                        }
                        Err(error) => runtime.continue_failed(
                            crate::error::PhiRuntimeError::request_compact(error.detail()),
                            "step_request_compact",
                        ),
                    }
                })
            }),
        )
    }

    fn request_complete(mut self, _cont: StepCont) -> StepBounce {
        let original_history = self.history();
        let mut request = self.request_complete_request(
            self.base_step()
                .request_complete_call()
                .cloned()
                .expect("request_complete requires a request_complete step"),
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
            return self.continue_failed(failure, "request_complete");
        }
        let request_history_tail = appended_history_tail(&original_history, &request_history);

        StepBounce::ContEval(
            self,
            StepCont::new(move |mut runtime| {
                Box::pin(async move {
                    let PhiModelResponse {
                        messages: mut response_messages,
                        turn_state,
                    } = match runtime
                        .render
                        .complete(
                            runtime.home.as_ref(),
                            runtime.render_template.as_deref(),
                            &request,
                            &request_history,
                        )
                        .await
                    {
                        Ok(response_messages) => response_messages,
                        Err(failure) => {
                            return runtime.continue_failed(failure, "request_complete");
                        }
                    };

                    for assistant in response_messages
                        .iter_mut()
                        .filter(|message| matches!(message, PhiMessage::Assistant(_)))
                    {
                        let mut after_event =
                            PhiAgentStepEvent::AfterModelResponse { message: assistant };
                        if let Err(failure) = runtime.modules.handle(&mut after_event) {
                            return runtime.continue_failed(failure, "request_complete");
                        }
                    }

                    let mut tool_calls = Vec::new();
                    let mut committed_assistants = Vec::new();
                    let mut response_messages_without_tool_call = Vec::new();
                    for message in response_messages {
                        match message {
                            PhiMessage::Tool(PhiToolMessage::ToolCall {
                                id,
                                name,
                                arguments,
                            }) => {
                                tool_calls.push(ToolCallRequest {
                                    id: id.clone().unwrap_or_else(|| name.clone()),
                                    call_id: id,
                                    name,
                                    arguments,
                                });
                            }
                            other => {
                                if matches!(other, PhiMessage::Assistant(_)) {
                                    committed_assistants.push(other.clone());
                                }
                                response_messages_without_tool_call.push(other);
                            }
                        }
                    }

                    let mut after_response_event = PhiAgentStepEvent::AfterModelResponseParsed {
                        messages: &response_messages_without_tool_call,
                    };
                    if let Err(failure) = runtime.modules.handle(&mut after_response_event) {
                        return runtime.continue_failed(failure, "request_complete");
                    }

                    if !tool_calls.is_empty() {
                        let mut pending_messages = request_history_tail.clone();
                        pending_messages.extend(response_messages_without_tool_call);
                        let delta = runtime.cur_delta().clone();
                        return StepBounce::CreateNextStep(
                            runtime,
                            PhiAgentStep::request_executor(
                                "tool execution is pending",
                                pending_messages,
                                tool_calls,
                            ),
                            delta,
                        );
                    }

                    for message in request_history_tail.clone() {
                        runtime.commit_message(message);
                    }
                    for message in response_messages_without_tool_call {
                        runtime.commit_message(message);
                    }
                    for assistant in &committed_assistants {
                        let committed_event =
                            crate::module::PhiAgentCommitEvent::ModelResponseCommitted {
                                message: assistant,
                            };
                        runtime.modules.observe(&committed_event);
                    }
                    let delta = runtime.cur_delta().clone();
                    if turn_state == PhiModelTurnState::Continue {
                        let step = runtime.request_complete_step(
                            "provider response requires another model request",
                        );
                        return StepBounce::CreateNextStep(runtime, step, delta);
                    }
                    StepBounce::CreateNextStep(
                        runtime,
                        PhiAgentStep::completed(
                            "model response committed; no tool execution is pending",
                        ),
                        delta,
                    )
                })
            }),
        )
    }

    fn step_tool(
        mut self,
        pending_messages: Vec<PhiMessage>,
        tool_calls: Vec<ToolCallRequest>,
        _cont: StepCont,
    ) -> StepBounce {
        let mut tool_calls = tool_calls.into_iter();
        let Some(mut request) = tool_calls.next() else {
            let step = self.request_complete_step("no tool execution is pending");
            let delta = if self.cur_delta().is_empty() {
                self.base_delta().clone()
            } else {
                self.cur_delta().clone()
            };
            return StepBounce::ReplaceBaseStep(self, step, delta);
        };
        let remaining_tool_calls = tool_calls.collect::<Vec<_>>();
        let step = self.base_step().clone();
        let expr = self.base_expr().clone();
        let mut before_event = PhiAgentStepEvent::BeforeToolCall {
            step: &step,
            expr: &expr,
            request: &mut request,
        };
        if let Err(failure) = self.modules.handle(&mut before_event) {
            return self.continue_failed(failure, "step_tool");
        }

        StepBounce::ContEval(
            self,
            StepCont::new(move |mut runtime| {
                Box::pin(async move {
                    let (request, mut response) =
                        match runtime.executor.call_tool(request.clone(), &runtime).await {
                            Ok(outcome) => outcome,
                            Err(failure) => {
                                let failure = if matches!(
                                    failure.kind(),
                                    crate::error::PhiErrorKind::ToolNotFound
                                ) {
                                    failure
                                        .with_pending_messages(pending_messages.clone())
                                        .with_remaining_tool_requests(remaining_tool_calls.clone())
                                } else {
                                    failure
                                };
                                return runtime.continue_failed(failure, "step_tool");
                            }
                        };

                    let mut after_event = PhiAgentStepEvent::AfterToolCall {
                        request: &request,
                        result: &mut response,
                    };
                    if let Err(failure) = runtime.modules.handle(&mut after_event) {
                        return runtime.continue_failed(failure, "step_tool");
                    }

                    let tool_call_message = PhiMessage::tool_call(
                        request.call_id.clone().or(Some(request.id.clone())),
                        request.name.clone(),
                        request.arguments.clone(),
                    );
                    let tool_result_message = PhiMessage::tool_result(
                        response.call_id.clone().or(Some(response.id.clone())),
                        Some(response.name.clone()),
                        serde_json::to_value(&response.output)
                            .expect("tool output should serialize into history"),
                    );

                    let committed_assistants = pending_messages
                        .iter()
                        .filter(|message| matches!(message, PhiMessage::Assistant(_)))
                        .cloned()
                        .collect::<Vec<_>>();
                    for message in pending_messages {
                        runtime.commit_message(message);
                    }
                    for assistant in &committed_assistants {
                        let committed_event =
                            crate::module::PhiAgentCommitEvent::ModelResponseCommitted {
                                message: assistant,
                            };
                        runtime.modules.observe(&committed_event);
                    }
                    runtime.commit_message(tool_call_message);
                    runtime.commit_tool_result(tool_result_message);
                    let step = if remaining_tool_calls.is_empty() {
                        runtime.request_complete_step(
                            "tool result committed; model response is pending",
                        )
                    } else {
                        PhiAgentStep::request_executor(
                            "additional tool execution is pending",
                            Vec::new(),
                            remaining_tool_calls,
                        )
                    };
                    let delta = runtime.cur_delta().clone();
                    StepBounce::CreateNextStep(runtime, step, delta)
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
