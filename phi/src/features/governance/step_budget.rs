use crate::session::PhiAgentStep;
use crate::{
    agent::{PhiAgentRuntime, StepCont, StepInterveneNext},
    module::PhiModule,
};

pub struct StepBudgetPolicy {
    max_steps: usize,
}

impl StepBudgetPolicy {
    pub const fn new(max_steps: usize) -> Self {
        Self { max_steps }
    }
}

impl PhiModule for StepBudgetPolicy {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        if !matches!(runtime.base_step(), PhiAgentStep::RequestComplete { .. }) {
            return next.call(runtime, cont);
        }

        if runtime.history().len() >= self.max_steps {
            let delta = runtime.cur_delta().clone();
            return Ok(crate::agent::StepBounce::CreateNextStep(
                runtime,
                PhiAgentStep::completed(format!("max steps reached: {}", self.max_steps)),
                delta,
            ));
        }

        next.call(runtime, cont)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{PhiAgentStep, Session};
    use crate::tests::support::test_model_defaults;

    #[test]
    fn step_budget_completes_at_step_boundary_beyond_limit() {
        let session = Session::from_root(
            PhiAgentStep::request_complete("ready", &test_model_defaults()),
            vec![
                crate::message::PhiMessage::user("one"),
                crate::message::PhiMessage::assistant("two"),
            ],
        );

        let outcome = crate::tests::support::step_agent_builder(session)
            .with_client(crate::tests::support::stub_client(Vec::new()))
            .with_module(StepBudgetPolicy::new(2))
            .build()
            .expect("agent should build")
            .run_single_step();
        let outcome = tokio::runtime::Runtime::new()
            .expect("tokio runtime should build")
            .block_on(outcome);

        assert!(matches!(
            outcome.session.step(),
            PhiAgentStep::Completed { detail }
            if detail == "max steps reached: 2"
        ));
    }
}
