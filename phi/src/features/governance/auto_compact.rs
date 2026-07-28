use crate::{
    agent::{
        PhiAgentRuntime, StepCont, StepInterveneError, StepInterveneNext, StepInterveneResult,
    },
    module::PhiModule,
    render::approx_history_token_count,
    session::PhiAgentStep,
};

pub struct AutoCompactPolicy {
    threshold_tokens: usize,
}

impl AutoCompactPolicy {
    pub const fn new(context_tokens: usize) -> Self {
        Self {
            threshold_tokens: context_tokens.saturating_mul(9) / 10,
        }
    }

    #[cfg(test)]
    pub const fn with_threshold(threshold_tokens: usize) -> Self {
        Self { threshold_tokens }
    }
}

impl PhiModule for AutoCompactPolicy {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> StepInterveneResult {
        let expr = runtime.base_expr();
        let PhiAgentStep::RequestComplete { .. } = expr.step() else {
            return next.call(runtime, cont);
        };

        if expr.model_retry_state().is_some()
            || expr
                .loop_guard_rejected_attempts()
                .is_some_and(|attempts| attempts != 0)
        {
            return next.call(runtime, cont);
        }

        let history = runtime.history();
        let rough_tokens = approx_history_token_count(&history);
        if rough_tokens < self.precheck_threshold_tokens() {
            return next.call(runtime, cont);
        }

        let request = runtime
            .base_step()
            .request_complete_call()
            .cloned()
            .expect("request complete should produce a provider call");
        let token_count = match runtime.provider_history_token_count(&request, &history) {
            Ok(token_count) => token_count,
            Err(error) => return Err(StepInterveneError::new(runtime, error)),
        };
        if token_count < self.threshold_tokens {
            return next.call(runtime, cont);
        }

        let delta = runtime.cur_delta().clone();
        Ok(crate::agent::StepBounce::CreateNextStep(
            runtime,
            PhiAgentStep::request_compact(),
            delta,
        ))
    }
}

impl AutoCompactPolicy {
    const fn precheck_threshold_tokens(&self) -> usize {
        self.threshold_tokens.saturating_mul(3) / 4
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        agent::{PhiAgent, PhiAgentCommand},
        message::PhiMessage,
        session::{PhiAgentStep, Session},
        tests::support::stub_client,
        utils::approx_token_count,
    };

    use super::AutoCompactPolicy;

    #[test]
    fn uses_ninety_percent_of_context_window() {
        assert_eq!(AutoCompactPolicy::new(100).threshold_tokens, 90);
        assert_eq!(AutoCompactPolicy::new(100).precheck_threshold_tokens(), 67);
    }

    #[test]
    fn rough_precheck_leaves_obviously_small_histories_alone() {
        let policy = AutoCompactPolicy::with_threshold(100);
        assert!(approx_token_count("hello") < policy.precheck_threshold_tokens());
    }

    #[tokio::test]
    async fn clean_request_complete_is_replaced_by_request_compact() {
        let defaults = crate::tests::support::test_model_defaults();
        let original = PhiAgentStep::request_complete("custom request", &defaults);
        let session = Session::from_root(original.clone(), vec![PhiMessage::user("hello")]);
        let outcome = PhiAgent::builder(session, PhiAgentCommand::Step(PhiAgentCommand::step()))
            .with_home(Arc::new(crate::home::LocalPhiHome::new(
                crate::tests::support::unique_test_home(),
            )))
            .with_client(stub_client(Vec::new()))
            .with_module(AutoCompactPolicy::with_threshold(1))
            .build()
            .expect("agent should build")
            .run_single_step()
            .await;

        let PhiAgentStep::RequestCompact = outcome.session.step() else {
            panic!("clean request should enter request compact");
        };
        let expr = outcome.session.clone().into_expr();
        let resume = expr
            .expr()
            .expect("request compact should retain the resumable parent expr");
        assert_eq!(resume.step().detail(), original.detail());
        assert_eq!(
            resume
                .step()
                .request_complete_call()
                .map(|call| call.model.as_str()),
            original
                .request_complete_call()
                .map(|call| call.model.as_str()),
        );
        assert!(
            resume
                .step()
                .request_complete_call()
                .is_some_and(|call| call.tools.is_empty()),
            "resume step should stay tool-free",
        );
    }

    #[tokio::test]
    async fn retrying_request_complete_is_not_compacted() {
        let defaults = crate::tests::support::test_model_defaults();
        let session = Session::from_expr(
            crate::expr::PhiStepExpr::new(
                PhiAgentStep::request_complete("retrying", &defaults),
                vec![PhiMessage::user("hello")],
            )
            .with_model_retry_state(crate::session::PhiModelRetryState { attempt: 1 }),
        );
        let outcome = crate::agent::PhiAgent::builder(
            session,
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        )
        .with_home(Arc::new(crate::home::LocalPhiHome::new(
            crate::tests::support::unique_test_home(),
        )))
        .with_client(stub_client(vec![PhiMessage::assistant("done")]))
        .with_module(AutoCompactPolicy::with_threshold(1))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

        assert!(!matches!(
            outcome.session.step(),
            PhiAgentStep::RequestCompact
        ));
    }

    #[tokio::test]
    async fn loop_guard_request_complete_is_not_compacted() {
        let defaults = crate::tests::support::test_model_defaults();
        let session = Session::from_expr(
            crate::expr::PhiStepExpr::new(
                PhiAgentStep::request_complete("loop retry", &defaults),
                vec![PhiMessage::user("hello")],
            )
            .with_loop_guard_rejected_attempts(1),
        );
        let outcome = crate::agent::PhiAgent::builder(
            session,
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        )
        .with_home(Arc::new(crate::home::LocalPhiHome::new(
            crate::tests::support::unique_test_home(),
        )))
        .with_client(stub_client(vec![PhiMessage::assistant("done")]))
        .with_module(AutoCompactPolicy::with_threshold(1))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

        assert!(!matches!(
            outcome.session.step(),
            PhiAgentStep::RequestCompact
        ));
    }
}
