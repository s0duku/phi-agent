use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::{
    agent::{PhiAgentRuntime, StepBounce, StepCont, StepInterveneNext},
    message::PhiMessage,
    module::PhiModule,
    session::{PhiAgentStep, Session},
};

use super::support::{step_agent_builder, stub_client, test_model_defaults};

struct RewriteInterveneModule;
struct StopInterveneModule;
struct CountingInterveneModule {
    calls: Arc<AtomicUsize>,
}

impl PhiModule for RewriteInterveneModule {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        let _ = cont;
        let _ = next;
        let delta = runtime.base_delta().clone();
        Ok(StepBounce::ReplaceBaseStep(
            runtime,
            PhiAgentStep::turn_end("rewritten by intervene"),
            delta,
        ))
    }
}

impl PhiModule for StopInterveneModule {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        _cont: StepCont,
        _next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        let delta = runtime.cur_delta().clone();
        Ok(StepBounce::CreateNextStep(
            runtime,
            PhiAgentStep::turn_end("stopped by intervene"),
            delta,
        ))
    }
}

impl PhiModule for CountingInterveneModule {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        self.calls.fetch_add(1, Ordering::Relaxed);
        next.call(runtime, cont)
    }
}

#[tokio::test]
async fn intervene_rewrites_step_before_default_eval() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(Vec::new()))
        .with_module(RewriteInterveneModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::TurnEnd { detail } if detail == "rewritten by intervene"
    ));
}

#[tokio::test]
async fn intervene_may_stop_before_later_modules_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );

    let _outcome = step_agent_builder(session)
        .with_client(stub_client(Vec::new()))
        .with_module(StopInterveneModule)
        .with_module(CountingInterveneModule {
            calls: calls.clone(),
        })
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(calls.load(Ordering::Relaxed), 0);
}
