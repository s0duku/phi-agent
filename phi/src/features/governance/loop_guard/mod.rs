mod reasoning;
mod similarity;

use crate::{
    agent::{PhiAgentRuntime, StepCont, StepInterveneNext},
    error::{PhiErrorKind, PhiRuntimeError, PhiRuntimeResult},
    expr::{DeltaLookup, PhiExprDelta, PhiStepExpr},
    message::{PhiHistory, PhiMessage},
    module::{PhiAgentCommitEvent, PhiAgentStepEvent, PhiModule},
    session::PhiAgentStep,
};
use serde::Serialize;

pub use reasoning::{ReasoningSimilarityConfig, ReasoningSimilarityDetector};

const DEFAULT_LOOPGUARD_WINDOW: usize = 5;
const DEFAULT_LOOPGUARD_MAX_RETRIES: usize = 5;
const DEFAULT_LOOPGUARD_REASONING_NGRAM_SIZE: usize = 4;
const DEFAULT_LOOPGUARD_REASONING_SIMILARITY_THRESHOLD: f64 = 0.90;
const DEFAULT_LOOPGUARD_REASONING_MIN_CHARS: usize = 300;
const LOOP_GUARD_REJECTED_ATTEMPTS_STORE_KEY: &str = "phi_loop_guard_rejected_attempts";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LoopGuardProbe {
    pub active: bool,
    pub rejected_attempts: usize,
    pub max_retries: usize,
    pub boundary_matches: bool,
    pub will_intervene: bool,
    pub exhausted: bool,
    pub next_rejected_attempts: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct LoopGuardConfig {
    pub window: usize,
    pub max_retries: usize,
    pub reasoning: Option<ReasoningSimilarityConfig>,
}

#[derive(Clone, Debug)]
pub struct LoopDetection {
    pub detector: &'static str,
    pub detail: String,
}

pub trait LoopDetector: Send + Sync {
    fn initialize(&mut self, messages: &PhiHistory, window: usize);
    fn inspect_candidate(&mut self, message: &PhiMessage, window: usize) -> Option<LoopDetection>;
    fn commit(&mut self, message: &PhiMessage, window: usize);
}

pub struct LoopGuardPolicy {
    window: usize,
    max_retries: usize,
    detectors: Vec<Box<dyn LoopDetector>>,
}

pub const fn default_loopguard_window() -> usize {
    DEFAULT_LOOPGUARD_WINDOW
}

pub const fn default_loopguard_max_retries() -> usize {
    DEFAULT_LOOPGUARD_MAX_RETRIES
}

pub const fn default_loopguard_reasoning_ngram_size() -> usize {
    DEFAULT_LOOPGUARD_REASONING_NGRAM_SIZE
}

pub const fn default_loopguard_reasoning_similarity_threshold() -> f64 {
    DEFAULT_LOOPGUARD_REASONING_SIMILARITY_THRESHOLD
}

pub const fn default_loopguard_reasoning_min_chars() -> usize {
    DEFAULT_LOOPGUARD_REASONING_MIN_CHARS
}

impl LoopGuardPolicy {
    pub fn new(config: LoopGuardConfig) -> Self {
        let mut detectors: Vec<Box<dyn LoopDetector>> = Vec::new();
        if let Some(reasoning) = config.reasoning {
            detectors.push(Box::new(ReasoningSimilarityDetector::new(reasoning)));
        }

        Self {
            window: config.window,
            max_retries: config.max_retries,
            detectors,
        }
    }

    fn inspect_candidate(&mut self, message: &PhiMessage) -> Result<(), PhiRuntimeError> {
        let detection = self
            .detectors
            .iter_mut()
            .find_map(|detector| detector.inspect_candidate(message, self.window));

        let Some(detection) = detection else {
            return Ok(());
        };

        Err(PhiRuntimeError::model_candidate_rejected(format!(
            "loop detected by {}: {}",
            detection.detector, detection.detail
        ))
        .with_source_step("request_complete"))
    }
}

fn request_complete_ancestor(expr: &PhiStepExpr) -> Option<&PhiStepExpr> {
    expr.find_ancestor(|step| matches!(step, PhiAgentStep::RequestComplete { .. }))
}

fn is_loop_guard_failed_step(step: &PhiAgentStep) -> bool {
    let PhiAgentStep::Failed { error } = step else {
        return false;
    };

    error.kind() == PhiErrorKind::ModelCandidateRejected
        && error.source_step() == Some("request_complete")
}

impl PhiModule for LoopGuardPolicy {
    type ProbInfo = LoopGuardProbe;

    fn probe_name(&self) -> &'static str {
        "loop_guard"
    }

    fn probe(&self, runtime: &PhiAgentRuntime) -> Option<Self::ProbInfo> {
        let ancestor = request_complete_ancestor(runtime.base_expr());
        let rejected_attempts = ancestor
            .and_then(PhiStepExpr::loop_guard_rejected_attempts)
            .unwrap_or(0);
        let boundary_matches = is_loop_guard_failed_step(runtime.base_step()) && ancestor.is_some();
        let next_rejected_attempts = boundary_matches.then_some(rejected_attempts + 1);
        let exhausted = boundary_matches && rejected_attempts >= self.max_retries;

        Some(LoopGuardProbe {
            active: rejected_attempts > 0,
            rejected_attempts,
            max_retries: self.max_retries,
            boundary_matches,
            will_intervene: boundary_matches && !exhausted,
            exhausted,
            next_rejected_attempts,
        })
    }

    fn intervene(
        &mut self,
        mut runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        // Loop-guard retries are decided at the next step boundary on purpose:
        // the candidate response should fail cleanly first, then a later
        // boundary policy chooses whether that failure resumes or remains
        // terminal. This preserves the "prepare first, commit last" step
        // semantics instead of letting event handlers directly control commit.
        let probe = self
            .probe(&runtime)
            .expect("loop guard probe should always be available");
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
                "loop guard budget exhausted after {} rejections; resuming with a clean completion request",
                self.max_retries
            ));
            delta.unbind_loop_guard_rejected_attempts();
            let step = runtime.request_complete_step("resuming after exhausted loop guard budget");
            return Ok(crate::agent::StepBounce::ReplaceBaseStep(
                runtime, step, delta,
            ));
        }

        let next_rejected_attempts = probe
            .next_rejected_attempts
            .expect("loop guard intervention should compute next rejected attempts");
        delta.bind_loop_guard_rejected_attempts(next_rejected_attempts);
        let step = runtime.request_complete_step("retrying completion after loop-guard rejection");
        let _ = next;
        let _ = cont;
        Ok(crate::agent::StepBounce::ReplaceBaseStep(
            runtime, step, delta,
        ))
    }

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        match event {
            PhiAgentStepEvent::BeforeModelRequest {
                step: _step,
                expr,
                request,
                history,
            } => {
                for detector in &mut self.detectors {
                    detector.initialize(history, self.window);
                }

                if expr
                    .loop_guard_rejected_attempts()
                    .is_some_and(|attempts| attempts > 0)
                {
                    request.temperature = request
                        .temperature
                        .map(|temperature| (temperature * 1.2).clamp(0.0, 2.0));
                }
            }
            PhiAgentStepEvent::BeforeCreateNextStep {
                base_expr,
                step,
                delta,
            }
            | PhiAgentStepEvent::BeforeReplaceBaseStep {
                base_expr,
                step,
                delta,
            } => {
                if !matches!(step, PhiAgentStep::RequestComplete { .. }) {
                    return Ok(());
                }

                match delta.loop_guard_rejected_attempts_binding() {
                    DeltaLookup::Value(_) | DeltaLookup::Unset => return Ok(()),
                    DeltaLookup::Missing => {}
                }

                let Some(previous_attempts) = base_expr.loop_guard_rejected_attempts() else {
                    return Ok(());
                };

                if delta.has_model_retry_state_binding() {
                    delta.bind_loop_guard_rejected_attempts(previous_attempts);
                } else {
                    delta.unbind_loop_guard_rejected_attempts();
                }
            }
            PhiAgentStepEvent::AfterModelResponse { message, .. } => {
                self.inspect_candidate(message)?
            }
            _ => {}
        }
        Ok(())
    }

    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        if let PhiAgentCommitEvent::ModelResponseCommitted { message } = event {
            for detector in &mut self.detectors {
                detector.commit(message, self.window);
            }
        }
    }
}

impl PhiStepExpr {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn loop_guard_rejected_attempts(&self) -> Option<usize> {
        self.lookup(LOOP_GUARD_REJECTED_ATTEMPTS_STORE_KEY)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_loop_guard_rejected_attempts(self, rejected_attempts: usize) -> Self {
        self.with_store(LOOP_GUARD_REJECTED_ATTEMPTS_STORE_KEY, rejected_attempts)
    }
}

impl PhiExprDelta {
    pub(crate) fn loop_guard_rejected_attempts_binding(&self) -> DeltaLookup<usize> {
        self.lookup(LOOP_GUARD_REJECTED_ATTEMPTS_STORE_KEY)
    }

    pub(crate) fn bind_loop_guard_rejected_attempts(&mut self, rejected_attempts: usize) {
        self.unbind_loop_guard_rejected_attempts();
        self.bind(LOOP_GUARD_REJECTED_ATTEMPTS_STORE_KEY, rejected_attempts);
    }

    pub(crate) fn unbind_loop_guard_rejected_attempts(&mut self) {
        self.unbind(LOOP_GUARD_REJECTED_ATTEMPTS_STORE_KEY);
    }

    pub(crate) fn has_loop_guard_rejected_attempts_binding(&self) -> bool {
        !matches!(
            self.loop_guard_rejected_attempts_binding(),
            DeltaLookup::Missing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::ReasoningEffort, error::PhiRuntimeError, render::PhiProviderCall, session::Session,
    };

    struct NoopDetector;

    impl LoopDetector for NoopDetector {
        fn initialize(&mut self, _messages: &PhiHistory, _window: usize) {}

        fn inspect_candidate(
            &mut self,
            _message: &PhiMessage,
            _window: usize,
        ) -> Option<LoopDetection> {
            None
        }

        fn commit(&mut self, _message: &PhiMessage, _window: usize) {}
    }

    fn test_request() -> PhiProviderCall {
        PhiProviderCall {
            model: "test-model".to_string(),
            tools: Vec::new(),
            temperature: Some(0.5),
            max_tokens: 1024,
            enable_reasoning: true,
            thinking_token_budget: 128,
            reasoning_effort: ReasoningEffort::Medium,
        }
    }

    #[test]
    fn loop_guard_retry_raises_temperature_for_retry_request() {
        let session = Session::from_expr(
            crate::expr::PhiStepExpr::new(
                PhiAgentStep::request_complete(
                    "requesting completion",
                    &crate::tests::support::test_model_defaults(),
                ),
                vec![PhiMessage::user("hello")],
            )
            .with_loop_guard_rejected_attempts(1),
        );

        let mut policy = LoopGuardPolicy {
            window: 4,
            max_retries: 3,
            detectors: vec![Box::new(NoopDetector)],
        };

        let mut request = test_request();
        let expr = session.clone().into_expr();
        let mut history = expr.history();
        let mut event = PhiAgentStepEvent::BeforeModelRequest {
            step: expr.step(),
            expr: &expr,
            history: &mut history,
            request: &mut request,
        };
        policy
            .handle(&mut event)
            .expect("before-model hook should succeed");

        assert_eq!(request.temperature, Some(0.6));
    }

    #[test]
    fn loop_guard_retry_state_clears_after_leaving_failed_retry_path() {
        let session = Session::from_root(
            PhiAgentStep::completed("done"),
            vec![PhiMessage::user("hello")],
        );
        let expr = session.into_expr();
        assert_eq!(expr.loop_guard_rejected_attempts(), None);

        let mut request = test_request();
        let mut policy = LoopGuardPolicy {
            window: 4,
            max_retries: 3,
            detectors: vec![Box::new(NoopDetector)],
        };
        let expr = Session::from_root(
            PhiAgentStep::completed("done"),
            vec![PhiMessage::user("hello")],
        )
        .into_expr();
        let mut history = expr.history();
        let mut event = PhiAgentStepEvent::BeforeModelRequest {
            step: expr.step(),
            expr: &expr,
            history: &mut history,
            request: &mut request,
        };
        policy
            .handle(&mut event)
            .expect("before-model hook should succeed");

        assert_eq!(request.temperature, Some(0.5));
    }

    #[tokio::test]
    async fn exhausted_loop_guard_budget_resets_to_clean_request_complete_step() {
        let retrying_step = crate::expr::PhiStepExpr::new(
            PhiAgentStep::request_complete(
                "requesting completion",
                &crate::tests::support::test_model_defaults(),
            ),
            Vec::<PhiMessage>::new(),
        )
        .with_loop_guard_rejected_attempts(3);
        let session = Session::from_expr(
            retrying_step.branch_failed(
                PhiRuntimeError::model_candidate_rejected("loop detected")
                    .with_source_step("request_complete"),
            ),
        );

        let outcome = crate::tests::support::step_agent_builder(session)
            .with_client(crate::tests::support::stub_client(Vec::new()))
            .with_module(LoopGuardPolicy {
                window: 4,
                max_retries: 3,
                detectors: vec![Box::new(NoopDetector)],
            })
            .build()
            .expect("agent should build")
            .run_single_step()
            .await;

        match outcome.session.step() {
            PhiAgentStep::RequestComplete { detail, .. } => {
                assert_eq!(detail, "resuming after exhausted loop guard budget");
            }
            step => panic!("expected clean request-complete step, got {step:?}"),
        }
        assert_eq!(
            outcome
                .session
                .clone()
                .into_expr()
                .loop_guard_rejected_attempts(),
            None
        );
    }
}
