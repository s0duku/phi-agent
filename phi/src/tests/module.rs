use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::{
    agent::{PhiAgentRuntime, StepBounce, StepCont, StepInterveneError, StepInterveneNext},
    error::PhiAgentRuntimeError,
    message::PhiMessage,
    module::PhiModule,
    session::{PhiAgentStep, PhiReActStep, Session},
};

use super::support::{step_agent_builder, stub_client, test_model_defaults};

struct RewriteInterveneModule;
struct AppendAndRewriteInterveneModule;
struct AppendThenFailInterveneModule;
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
        Ok(StepBounce::ReplaceBaseStep(
            runtime,
            PhiReActStep::turn_end("rewritten by intervene"),
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
        Ok(StepBounce::CreateNextStep(
            runtime,
            PhiReActStep::turn_end("stopped by intervene"),
        ))
    }
}

impl PhiModule for AppendAndRewriteInterveneModule {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        mut runtime: PhiAgentRuntime,
        _cont: StepCont,
        _next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        runtime.commit_message(PhiMessage::assistant("current"));
        Ok(StepBounce::ReplaceBaseStep(
            runtime,
            PhiReActStep::turn_end("merged replacement"),
        ))
    }
}

impl PhiModule for AppendThenFailInterveneModule {
    type ProbInfo = ();

    fn intervene(
        &mut self,
        mut runtime: PhiAgentRuntime,
        _cont: StepCont,
        _next: StepInterveneNext,
    ) -> crate::agent::StepInterveneResult {
        runtime.commit_message(PhiMessage::assistant("failed current"));
        Err(StepInterveneError::new(
            runtime,
            PhiAgentRuntimeError::module("failed after delta"),
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
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { detail }) if detail == "rewritten by intervene"
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

#[tokio::test]
async fn replace_base_step_extends_base_delta_with_current_delta() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("base")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(Vec::new()))
        .with_module(AppendAndRewriteInterveneModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("base"), PhiMessage::assistant("current")]
    );
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { detail }) if detail == "merged replacement"
    ));
}

#[tokio::test]
async fn runtime_failed_discards_current_delta_in_a_new_failed_frame() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("base")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(Vec::new()))
        .with_module(AppendThenFailInterveneModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), &[PhiMessage::user("base")]);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::Failed(failed) if failed.error().detail() == "failed after delta"
    ));
    let expr = outcome.session.into_expr();
    assert!(expr.delta().is_empty());
    assert_eq!(
        expr.expr()
            .expect("failed frame should retain its base expression")
            .history(),
        &[PhiMessage::user("base")]
    );
}
