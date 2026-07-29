use crate::{
    agent::{PhiAgentRuntime, StepCont, StepInterveneNext},
    error::PhiErrorKind,
    expr::{DeltaLookup, PhiExprDelta, PhiStepExpr},
    module::{PhiAgentStepEvent, PhiModule},
    session::{PhiAgentStep, PhiModelRetryState},
};
use serde::Serialize;

const MODEL_RETRY_STATE_STORE_KEY: &str = "phi_model_retry_state";

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
}

fn request_complete_ancestor(expr: &PhiStepExpr) -> Option<&PhiStepExpr> {
    expr.find_ancestor(|step| matches!(step, PhiAgentStep::RequestComplete { .. }))
}

fn is_model_retry_failed_step(step: &PhiAgentStep) -> bool {
    let PhiAgentStep::Failed { error } = step else {
        return false;
    };

    matches!(
        error.kind(),
        PhiErrorKind::ProviderRequest | PhiErrorKind::ProviderResponse
    ) && error.source_step() == Some("request_complete")
}

impl PhiModule for ModelRetryPolicy {
    type ProbInfo = ModelRetryProbe;

    fn probe_name(&self) -> &'static str {
        "model_retry"
    }

    fn probe(&self, runtime: &PhiAgentRuntime) -> Option<Self::ProbInfo> {
        let ancestor = request_complete_ancestor(runtime.base_expr());
        let attempt = ancestor
            .and_then(PhiStepExpr::model_retry_state)
            .map(|retry| retry.attempt);
        let boundary_matches =
            is_model_retry_failed_step(runtime.base_step()) && ancestor.is_some();
        let next_attempt =
            boundary_matches.then(|| attempt.map(|attempt| attempt + 1).unwrap_or(1));
        let exhausted = next_attempt.is_some_and(|attempt| attempt > self.max_retries);

        Some(ModelRetryProbe {
            active: attempt.is_some(),
            attempt,
            max_retries: self.max_retries,
            boundary_matches,
            will_intervene: boundary_matches && !exhausted,
            exhausted,
            next_attempt,
        })
    }

    fn intervene(
        &mut self,
        mut runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        let probe = self
            .probe(&runtime)
            .expect("model retry probe should always be available");
        if !probe.boundary_matches {
            return next.call(runtime, cont);
        }

        let mut delta = if runtime.cur_delta().is_empty() {
            runtime.base_delta().clone()
        } else {
            runtime.cur_delta().clone()
        };
        if probe.exhausted {
            runtime.emit_warning(&format!(
                "model retry budget exhausted after {} attempts; resuming with a clean completion request",
                self.max_retries
            ));
            delta.unbind_model_retry_state();
            let step = runtime.request_complete_step("resuming after exhausted model retry budget");
            return Ok(crate::agent::StepBounce::ReplaceBaseStep(
                runtime, step, delta,
            ));
        }

        let next_attempt = probe
            .next_attempt
            .expect("model retry intervention should compute a next attempt");
        delta.bind_model_retry_state(PhiModelRetryState {
            attempt: next_attempt,
        });
        let step = runtime.request_complete_step(format!(
            "retrying model request ({next_attempt}/{})",
            self.max_retries
        ));
        let _ = next;
        let _ = cont;
        Ok(crate::agent::StepBounce::ReplaceBaseStep(
            runtime, step, delta,
        ))
    }

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> crate::error::PhiRuntimeResult<()> {
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

        if !matches!(step, PhiAgentStep::RequestComplete { .. }) {
            return Ok(());
        }

        match delta.model_retry_state_binding() {
            DeltaLookup::Value(_) | DeltaLookup::Unset => return Ok(()),
            DeltaLookup::Missing => {}
        }

        let Some(previous_retry_state) = base_expr.model_retry_state() else {
            return Ok(());
        };

        if delta.has_loop_guard_rejected_attempts_binding() {
            delta.bind_model_retry_state(previous_retry_state);
        } else {
            delta.unbind_model_retry_state();
        }

        Ok(())
    }
}

impl PhiStepExpr {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn model_retry_state(&self) -> Option<PhiModelRetryState> {
        self.lookup(MODEL_RETRY_STATE_STORE_KEY)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_model_retry_state(mut self, retry: PhiModelRetryState) -> Self {
        self.delta.bind_model_retry_state(retry);
        self
    }
}

impl PhiExprDelta {
    pub(crate) fn model_retry_state_binding(&self) -> DeltaLookup<PhiModelRetryState> {
        self.lookup(MODEL_RETRY_STATE_STORE_KEY)
    }

    pub(crate) fn bind_model_retry_state(&mut self, retry: PhiModelRetryState) {
        self.unbind_model_retry_state();
        self.bind(MODEL_RETRY_STATE_STORE_KEY, retry);
    }

    pub(crate) fn unbind_model_retry_state(&mut self) {
        self.unbind(MODEL_RETRY_STATE_STORE_KEY);
    }

    pub(crate) fn has_model_retry_state_binding(&self) -> bool {
        !matches!(self.model_retry_state_binding(), DeltaLookup::Missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::PhiRuntimeError,
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
        ) -> crate::error::PhiRuntimeResult<PhiModelResponse> {
            let mut attempts = self.attempts.lock().expect("attempt lock should succeed");
            *attempts += 1;
            if *attempts == 1 {
                Err(
                    PhiRuntimeError::provider_request("model request failed: timeout")
                        .with_source_step("request_complete"),
                )
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
            PhiAgentStep::request_complete("requesting completion", &test_model_defaults()),
            Vec::<PhiMessage>::new(),
        );
        let session = Session::from_expr(
            base.branch_failed(
                PhiRuntimeError::provider_request("model request failed: timeout")
                    .with_source_step("request_complete"),
            ),
        );

        let outcome = crate::tests::support::step_agent_builder(session)
            .with_client(crate::tests::support::stub_client(Vec::new()))
            .with_module(ModelRetryPolicy::new(3))
            .build()
            .expect("agent should build")
            .run_single_step()
            .await;

        assert!(matches!(
            outcome.session.step(),
            PhiAgentStep::RequestComplete { detail, .. }
                if detail == "retrying model request (1/3)"
        ));
        let expr = outcome.session.clone().into_expr();
        assert_eq!(
            expr.model_retry_state(),
            Some(PhiModelRetryState { attempt: 1 })
        );
    }

    #[tokio::test]
    async fn provider_retry_state_flows_without_store() {
        let session = Session::from_root(
            PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
        assert!(matches!(
            agent.session().step(),
            PhiAgentStep::Failed { .. }
        ));
        agent.step().await;
        assert!(matches!(
            agent.session().step(),
            PhiAgentStep::RequestComplete { detail, .. }
                if detail == "retrying model request (1/3)"
        ));
        agent.step().await;
        let outcome = agent.into_session();

        assert!(matches!(outcome.step(), PhiAgentStep::Completed { .. }));
        assert_eq!(*attempts.lock().expect("attempt lock should succeed"), 2);
        assert_eq!(
            outcome.history().to_messages(),
            vec![PhiMessage::assistant("ok")]
        );
    }

    #[tokio::test]
    async fn exhausted_model_retry_budget_resets_to_clean_request_complete_step() {
        let retrying_step = PhiStepExpr::new(
            PhiAgentStep::request_complete("retrying model request (3/3)", &test_model_defaults()),
            Vec::<PhiMessage>::new(),
        )
        .with_model_retry_state(PhiModelRetryState { attempt: 3 });
        let session = Session::from_expr(
            retrying_step.branch_failed(
                PhiRuntimeError::provider_request("model request failed: timeout")
                    .with_source_step("request_complete"),
            ),
        );

        let outcome = crate::tests::support::step_agent_builder(session)
            .with_client(crate::tests::support::stub_client(Vec::new()))
            .with_module(ModelRetryPolicy::new(3))
            .build()
            .expect("agent should build")
            .run_single_step()
            .await;

        match outcome.session.step() {
            PhiAgentStep::RequestComplete { detail, .. } => {
                assert_eq!(detail, "resuming after exhausted model retry budget");
            }
            step => panic!("expected clean request-complete step, got {step:?}"),
        }
        assert_eq!(
            outcome.session.clone().into_expr().model_retry_state(),
            None
        );
    }
}
