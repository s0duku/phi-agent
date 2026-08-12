use crate::{
    agent::{PhiAgentRuntime, StepCont, StepInterveneNext},
    error::PhiAgentRuntimeError,
    expr::{PhiExprDelta, PhiStepExpr, PhiVariable},
    module::{PhiAgentStepEvent, PhiModule},
    session::{PhiAgentStep, PhiModelRetryState, PhiReActStep},
};
use serde::Serialize;

pub(super) const MODEL_RETRY_STATE_VARIABLE: PhiVariable<PhiModelRetryState> =
    PhiVariable::new("phi_model_retry_state");

pub(super) fn model_retry_state(expr: &PhiStepExpr) -> Option<PhiModelRetryState> {
    expr.lookup(MODEL_RETRY_STATE_VARIABLE)
}

fn store_model_retry_state(delta: &mut PhiExprDelta, retry: PhiModelRetryState) {
    delta.store(MODEL_RETRY_STATE_VARIABLE, retry);
}

fn remove_model_retry_state(delta: &mut PhiExprDelta) {
    delta.remove(MODEL_RETRY_STATE_VARIABLE);
}

pub(super) fn affects_model_retry_state(delta: &PhiExprDelta) -> bool {
    delta.affects(MODEL_RETRY_STATE_VARIABLE)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelRetryProbe {
    pub active: bool,
    pub attempt: Option<usize>,
    pub max_retries: usize,
    pub boundary_matches: bool,
    pub will_intervene: bool,
    pub exhausted: bool,
    pub next_attempt: Option<usize>,
}

pub struct ModelRetryPolicy {
    max_retries: usize,
}

impl ModelRetryPolicy {
    pub const fn new(max_retries: usize) -> Self {
        Self { max_retries }
    }

    fn retry_state(&self, runtime: &PhiAgentRuntime) -> ModelRetryProbe {
        let ancestor = request_provider_ancestor(runtime.base_expr());
        let attempt = ancestor
            .and_then(model_retry_state)
            .map(|retry| retry.attempt);
        let boundary_matches =
            is_model_retry_failed_step(runtime.base_step()) && ancestor.is_some();
        let next_attempt =
            boundary_matches.then(|| attempt.map(|attempt| attempt + 1).unwrap_or(1));
        let exhausted = next_attempt.is_some_and(|attempt| attempt > self.max_retries);

        ModelRetryProbe {
            active: attempt.is_some(),
            attempt,
            max_retries: self.max_retries,
            boundary_matches,
            will_intervene: boundary_matches && !exhausted,
            exhausted,
            next_attempt,
        }
    }
}

fn request_provider_ancestor(expr: &PhiStepExpr) -> Option<&PhiStepExpr> {
    expr.find_ancestor(|step| {
        matches!(
            step,
            PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. })
        )
    })
}

fn is_model_retry_failed_step(step: &PhiAgentStep) -> bool {
    let PhiAgentStep::Failed(failed) = step else {
        return false;
    };
    let error = failed.error();

    matches!(
        error,
        PhiAgentRuntimeError::ProviderRequest { .. }
            | PhiAgentRuntimeError::ProviderResponse { .. }
    )
}

impl PhiModule for ModelRetryPolicy {
    fn intervene(
        &mut self,
        mut runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        let probe = self.retry_state(&runtime);
        if !probe.boundary_matches {
            return next.call(runtime, cont);
        }

        if probe.exhausted {
            runtime.emit_warning(&format!(
                "model retry budget exhausted after {} attempts; resuming with a clean completion request",
                self.max_retries
            ));
            remove_model_retry_state(runtime.cur_delta_mut());
            let step = runtime.request_provider_step("resuming after exhausted model retry budget");
            return Ok(crate::agent::StepBounce::ReplaceBaseStep(runtime, step));
        }

        let next_attempt = probe
            .next_attempt
            .expect("model retry intervention should compute a next attempt");
        store_model_retry_state(
            runtime.cur_delta_mut(),
            PhiModelRetryState {
                attempt: next_attempt,
            },
        );
        let step = runtime.request_provider_step(format!(
            "retrying model request ({next_attempt}/{})",
            self.max_retries
        ));
        let _ = next;
        let _ = cont;
        Ok(crate::agent::StepBounce::ReplaceBaseStep(runtime, step))
    }

    fn handle(
        &mut self,
        event: &mut PhiAgentStepEvent<'_>,
    ) -> crate::error::PhiAgentRuntimeResult<()> {
        let (base_expr, step, delta) = match event {
            PhiAgentStepEvent::BeforeCreateNextStep {
                base_expr,
                step,
                delta,
            }
            | PhiAgentStepEvent::BeforeReplaceBaseStep {
                base_expr,
                step,
                delta,
            } => (base_expr, step, delta),
            _ => return Ok(()),
        };

        if !matches!(step, PhiReActStep::RequestProvider { .. }) {
            return Ok(());
        }

        if affects_model_retry_state(delta) {
            return Ok(());
        }

        let Some(previous_retry_state) = model_retry_state(base_expr) else {
            return Ok(());
        };

        if super::loop_guard::affects_loop_guard_rejected_attempts(delta) {
            store_model_retry_state(delta, previous_retry_state);
        } else {
            remove_model_retry_state(delta);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::PhiAgentRuntimeError,
        expr::PhiStepExpr,
        message::{PhiHistory, PhiMessage},
        render::{PhiModelResponse, PhiProviderCall, TestClient},
        session::Session,
        tests::support::test_model_defaults,
    };
    use std::sync::{Arc, Mutex};

    struct FlakyProvider {
        attempts: Arc<Mutex<usize>>,
    }

    #[async_trait::async_trait]
    impl TestClient for FlakyProvider {
        async fn complete(
            &self,
            _request: &PhiProviderCall,
            _messages: &PhiHistory,
        ) -> crate::error::PhiAgentRuntimeResult<PhiModelResponse> {
            let mut attempts = self.attempts.lock().expect("attempt lock should succeed");
            *attempts += 1;
            if *attempts == 1 {
                Err(PhiAgentRuntimeError::provider_request(
                    "model request failed: timeout",
                ))
            } else {
                Ok(PhiModelResponse::unspecified(vec![PhiMessage::assistant(
                    "ok",
                )]))
            }
        }
    }

    #[tokio::test]
    async fn failed_model_request_resumes_previous_step_until_budget_exhausted() {
        let base = PhiStepExpr::new(
            PhiAgentStep::request_provider("requesting completion", &test_model_defaults()),
            Vec::<PhiMessage>::new(),
        );
        let session = Session::from_expr(base.branch_failed(
            PhiAgentRuntimeError::provider_request("model request failed: timeout"),
        ));

        let outcome = crate::tests::support::step_agent_builder(session)
            .with_client(crate::tests::support::stub_client(Vec::new()))
            .with_module(ModelRetryPolicy::new(3))
            .build()
            .expect("agent should build")
            .run_single_step()
            .await;

        assert!(matches!(
            outcome.session.step(),
            PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
                if detail == "retrying model request (1/3)"
        ));
        let expr = outcome.session.clone().into_expr();
        assert_eq!(
            model_retry_state(&expr),
            Some(PhiModelRetryState { attempt: 1 })
        );
    }

    #[tokio::test]
    async fn provider_retry_state_flows_across_steps() {
        let session = Session::from_root(
            PhiAgentStep::request_provider("ready", &test_model_defaults()),
            Vec::new(),
        );
        let attempts = Arc::new(Mutex::new(0));
        let mut agent = crate::tests::support::step_agent_builder(session)
            .with_client(Arc::new(FlakyProvider {
                attempts: attempts.clone(),
            }))
            .with_module(ModelRetryPolicy::new(3))
            .build()
            .expect("agent should build");
        agent.step().await;
        assert!(matches!(agent.session().step(), PhiAgentStep::Failed(_)));
        agent.step().await;
        assert!(matches!(
            agent.session().step(),
            PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
                if detail == "retrying model request (1/3)"
        ));
        agent.step().await;
        let outcome = agent.into_session();

        assert!(matches!(
            outcome.step(),
            PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
        ));
        assert_eq!(*attempts.lock().expect("attempt lock should succeed"), 2);
        assert_eq!(
            outcome.history().to_messages(),
            vec![PhiMessage::assistant("ok")]
        );
    }

    #[tokio::test]
    async fn exhausted_model_retry_budget_resets_to_clean_request_provider_step() {
        let retrying_step = PhiStepExpr::new(
            PhiAgentStep::request_provider("retrying model request (3/3)", &test_model_defaults()),
            Vec::<PhiMessage>::new(),
        )
        .store(
            MODEL_RETRY_STATE_VARIABLE,
            PhiModelRetryState { attempt: 3 },
        );
        let session = Session::from_expr(retrying_step.branch_failed(
            PhiAgentRuntimeError::provider_request("model request failed: timeout"),
        ));

        let outcome = crate::tests::support::step_agent_builder(session)
            .with_client(crate::tests::support::stub_client(Vec::new()))
            .with_module(ModelRetryPolicy::new(3))
            .build()
            .expect("agent should build")
            .run_single_step()
            .await;

        match outcome.session.step() {
            PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. }) => {
                assert_eq!(detail, "resuming after exhausted model retry budget");
            }
            step => panic!("expected clean request-complete step, got {step:?}"),
        }
        assert_eq!(
            model_retry_state(&outcome.session.clone().into_expr()),
            None
        );
    }
}
