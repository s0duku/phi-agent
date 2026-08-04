use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{
    agent::PhiAgentCommand,
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    executor::ToolCallRequest,
    home::LocalPhiHome,
    message::{PhiHistory, PhiMessage, PhiToolMessage},
    module::{PhiAgentStepEvent, PhiModule},
    render::{PhiModelResponse, PhiModelTurnState, PhiProviderCall, TestClient},
    session::{PhiAgentStep, PhiReActStep, Session},
};

use super::support::{
    default_step_agent_builder, shell_echo_ok_command, shell_stdout_ok, shell_tool_name,
    step_agent_builder, stub_client, test_model_defaults,
};

struct EmptyProvider;
struct SequenceProvider {
    responses: Mutex<VecDeque<PhiModelResponse>>,
}

struct HistoryDrivenProvider;

#[async_trait]
impl TestClient for EmptyProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        Ok(PhiModelResponse::unspecified(Vec::new()))
    }
}

#[async_trait]
impl TestClient for SequenceProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        self.responses
            .lock()
            .expect("response queue should be healthy")
            .pop_front()
            .ok_or_else(|| PhiAgentRuntimeError::provider_response("response queue exhausted"))
    }
}

#[async_trait]
impl TestClient for HistoryDrivenProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        let assistant_count = messages
            .iter()
            .filter(|message| matches!(message, PhiMessage::Assistant(_)))
            .count();
        if assistant_count == 0 {
            Ok(PhiModelResponse::new(
                vec![PhiMessage::assistant("first")],
                PhiModelTurnState::Continue,
            ))
        } else {
            Ok(PhiModelResponse::new(
                vec![PhiMessage::assistant("second")],
                PhiModelTurnState::Complete,
            ))
        }
    }
}

struct RejectAfterToolCallModule;
struct RejectFirstModelResponseModule {
    rejected: bool,
}

impl PhiModule for RejectAfterToolCallModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        if let PhiAgentStepEvent::AfterToolCall { .. } = event {
            return Err(PhiAgentRuntimeError::module("module rejected tool result"));
        }
        Ok(())
    }
}

impl PhiModule for RejectFirstModelResponseModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        if matches!(event, PhiAgentStepEvent::AfterModelResponse { .. }) && !self.rejected {
            self.rejected = true;
            return Err(PhiAgentRuntimeError::module(
                "module rejected first model response",
            ));
        }
        Ok(())
    }
}

fn compact_equivalence_agent(session: Session, command: PhiAgentCommand) -> crate::agent::PhiAgent {
    let post_compact_history = PhiHistory::from_messages(vec![
        PhiMessage::user("compacted context"),
        PhiMessage::assistant("first"),
    ]);
    let compact_threshold = crate::render::compact_prompt_token_count()
        + crate::render::approx_history_token_count(&post_compact_history)
        + 1;
    let render = crate::render::PhiRender::from_test_client(Arc::new(HistoryDrivenProvider))
        .with_compact_override(Arc::new(|_history| {
            Ok(PhiHistory::from_messages(vec![PhiMessage::user(
                "compacted context",
            )]))
        }));
    crate::agent::PhiAgent::builder(session, command)
        .with_home(Arc::new(LocalPhiHome::new(
            super::support::unique_test_home(),
        )))
        .with_model_defaults(test_model_defaults())
        .with_render(render)
        .with_module(
            crate::features::governance::auto_compact::AutoCompactPolicy::with_threshold(
                compact_threshold,
            ),
        )
        .build()
        .expect("compact equivalence agent should build")
}

async fn run_to_completion(mut agent: crate::agent::PhiAgent) -> crate::agent::AgentStepRunOutcome {
    loop {
        agent.step().await;
        let session = agent.session();
        let step = session.step();
        if step.is_terminal()
            || matches!(
                step,
                PhiAgentStep::ReAct(PhiReActStep::RequestCompact { .. })
            )
        {
            return crate::agent::AgentStepRunOutcome {
                error: session.step().error().cloned(),
                session: agent.into_session(),
            };
        }
    }
}

async fn run_to_completed(mut agent: crate::agent::PhiAgent) -> crate::agent::AgentStepRunOutcome {
    let mut previous_was_failed = false;
    loop {
        agent.step().await;
        let session = agent.session();
        let step = session.step();
        match step {
            PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. }) => {
                return crate::agent::AgentStepRunOutcome {
                    error: session.step().error().cloned(),
                    session: agent.into_session(),
                };
            }
            PhiAgentStep::Failed(_) if previous_was_failed => {
                return crate::agent::AgentStepRunOutcome {
                    error: session.step().error().cloned(),
                    session: agent.into_session(),
                };
            }
            PhiAgentStep::Failed(_) => previous_was_failed = true,
            _ => previous_was_failed = false,
        }
    }
}

fn serialized_session(session: &Session) -> serde_json::Value {
    serde_json::to_value(session).expect("session should serialize")
}

fn pending_tool_session(
    history: Vec<PhiMessage>,
    pending_messages: Vec<PhiMessage>,
    arguments: serde_json::Value,
) -> Session {
    Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            pending_messages,
            vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments,
            }],
        ),
        history,
    )
}

#[tokio::test]
async fn invariant_model_step_with_tool_call_keeps_history_clean() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("list files")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![
            PhiMessage::assistant("running bash now"),
            PhiMessage::tool_call(
                Some("call_1".to_string()),
                shell_tool_name(),
                serde_json::json!({ "cmd": "ls" }),
            ),
        ]))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), &[PhiMessage::user("list files")]);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { pending_messages, tool_calls, .. })
        if pending_messages.as_slice() == [PhiMessage::assistant("running bash now")].as_slice()
            && tool_calls.len() == 1
            && tool_calls[0].name == shell_tool_name()
            && tool_calls[0].call_id.as_deref() == Some("call_1")
    ));
}

#[tokio::test]
async fn invariant_tool_step_commits_pending_messages_atomically() {
    let session = pending_tool_session(
        vec![PhiMessage::user("hello")],
        vec![PhiMessage::assistant("running bash now")],
        serde_json::json!({ "cmd": shell_echo_ok_command() }),
    );

    let outcome = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    let history = outcome.session.history();
    assert_eq!(history[0], PhiMessage::user("hello"));
    assert_eq!(history[1], PhiMessage::assistant("running bash now"));
    assert_eq!(
        history[2],
        PhiMessage::tool_call(
            Some("call_1".to_string()),
            shell_tool_name(),
            serde_json::json!({ "cmd": shell_echo_ok_command() }),
        )
    );
    let PhiMessage::Tool(PhiToolMessage::ToolResult { id, name, result }) = &history[3] else {
        panic!("fourth history entry should be a tool result");
    };
    assert_eq!(id.as_deref(), Some("call_1"));
    assert_eq!(name.as_deref(), Some(shell_tool_name()));
    assert_eq!(result["output"], serde_json::json!(shell_stdout_ok()));
    assert_eq!(result["status"], serde_json::json!("exited"));
    assert_eq!(result["exit_code"], serde_json::json!(0));
    assert_eq!(result["handle"], serde_json::Value::Null);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn invariant_failed_tool_step_drops_pending_messages_and_rolls_back() {
    let session = pending_tool_session(
        vec![PhiMessage::user("hello")],
        vec![PhiMessage::assistant("running bash now")],
        serde_json::json!({ "cmd": shell_echo_ok_command() }),
    );
    let original = serde_json::to_value(&session).expect("session should serialize");

    let failed = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .with_module(RejectAfterToolCallModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert_eq!(failed.history(), &[PhiMessage::user("hello")]);
    assert!(matches!(
        failed.step(),
        PhiAgentStep::Failed(failed)
        if matches!(failed.error(), PhiAgentRuntimeError::Module { .. })
            && failed.error().detail() == "module rejected tool result"
    ));

    let resumed = default_step_agent_builder(failed)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert_eq!(
        serde_json::to_value(&resumed).expect("session should serialize"),
        original
    );
    assert_eq!(resumed.history(), &[PhiMessage::user("hello")]);
    assert!(matches!(
        resumed.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { .. })
    ));
}

#[tokio::test]
async fn invariant_completed_resumes_and_root_failed_stays_failed() {
    let completed = Session::from_root(
        PhiAgentStep::turn_end("done"),
        vec![PhiMessage::user("hello"), PhiMessage::assistant("world")],
    );
    let resumed_completed = default_step_agent_builder(completed)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;
    assert_eq!(
        resumed_completed.history(),
        &[PhiMessage::user("hello"), PhiMessage::assistant("world")]
    );
    assert!(matches!(
        resumed_completed.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. })
    ));

    let failed = Session::from_root(
        PhiAgentStep::failed(PhiAgentRuntimeError::module("failed")),
        vec![PhiMessage::user("hello")],
    );
    let resumed_failed = default_step_agent_builder(failed)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;
    assert_eq!(resumed_failed.history(), &[PhiMessage::user("hello")]);
    assert!(matches!(resumed_failed.step(), PhiAgentStep::Failed(_)));
}

#[tokio::test]
async fn yolo_continues_when_run_would_stop_at_runtime_failure() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );

    let run_home = Arc::new(LocalPhiHome::new(super::support::unique_test_home()));
    let run_builder = crate::agent::PhiAgent::builder(
        session.clone(),
        PhiAgentCommand::Run(PhiAgentCommand::run()),
    )
    .with_home(run_home)
    .with_model_defaults(test_model_defaults())
    .with_client(stub_client(vec![PhiMessage::assistant("ok")]))
    .prepare()
    .expect("run builder should prepare");
    let run_modules = crate::features::build_default_modules(run_builder.context());
    let run_outcome = run_to_completion(
        run_builder
            .with_module_layout(run_modules)
            .with_module(RejectFirstModelResponseModule { rejected: false })
            .build()
            .expect("run agent should build"),
    )
    .await;

    assert!(matches!(
        run_outcome.session.step(),
        PhiAgentStep::Failed(failed)
        if matches!(failed.error(), PhiAgentRuntimeError::Module { .. })
            && failed.error().detail() == "module rejected first model response"
    ));

    let yolo_home = Arc::new(LocalPhiHome::new(super::support::unique_test_home()));
    let yolo_builder =
        crate::agent::PhiAgent::builder(session, PhiAgentCommand::Yolo(PhiAgentCommand::yolo()))
            .with_home(yolo_home)
            .with_model_defaults(test_model_defaults())
            .with_client(stub_client(vec![PhiMessage::assistant("ok")]))
            .prepare()
            .expect("yolo builder should prepare");
    let yolo_modules = crate::features::build_default_modules(yolo_builder.context());
    let yolo_outcome = run_to_completed(
        yolo_builder
            .with_module_layout(yolo_modules)
            .with_module(RejectFirstModelResponseModule { rejected: false })
            .build()
            .expect("yolo agent should build"),
    )
    .await;

    assert!(yolo_outcome.error.is_none());
    assert_eq!(
        yolo_outcome.session.history(),
        vec![PhiMessage::user("hello"), PhiMessage::assistant("ok")]
    );
    assert!(matches!(
        yolo_outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
    ));
}

#[tokio::test]
async fn yolo_continues_after_provider_requests_follow_up_without_a_tool_call() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("inspect files")],
    );
    let client = SequenceProvider {
        responses: Mutex::new(VecDeque::from([
            PhiModelResponse::new(
                vec![PhiMessage::assistant("I will inspect the files now.")],
                PhiModelTurnState::Continue,
            ),
            PhiModelResponse::new(
                vec![PhiMessage::assistant("The files have been inspected.")],
                PhiModelTurnState::Complete,
            ),
        ])),
    };
    let outcome = run_to_completed(
        crate::agent::PhiAgent::builder(session, PhiAgentCommand::Yolo(PhiAgentCommand::yolo()))
            .with_home(Arc::new(LocalPhiHome::new(
                super::support::unique_test_home(),
            )))
            .with_model_defaults(test_model_defaults())
            .with_client(Arc::new(client))
            .build()
            .expect("agent should build"),
    )
    .await;

    assert_eq!(
        outcome.session.history(),
        &[
            PhiMessage::user("inspect files"),
            PhiMessage::assistant("I will inspect the files now."),
            PhiMessage::assistant("The files have been inspected."),
        ]
    );
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
    ));
}

#[tokio::test]
async fn yolo_step_updates_match_rebuilt_step_agents_at_every_frame() {
    let initial = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("inspect files")],
    );
    let mut yolo = crate::agent::PhiAgent::builder(
        initial.clone(),
        PhiAgentCommand::Yolo(PhiAgentCommand::yolo()),
    )
    .with_home(Arc::new(LocalPhiHome::new(
        super::support::unique_test_home(),
    )))
    .with_model_defaults(test_model_defaults())
    .with_client(Arc::new(HistoryDrivenProvider))
    .build()
    .expect("yolo agent should build");

    let mut rebuilt = initial;
    for _ in 0..4 {
        yolo.step().await;

        let rebuilt_agent = crate::agent::PhiAgent::builder(
            rebuilt,
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        )
        .with_home(Arc::new(LocalPhiHome::new(
            super::support::unique_test_home(),
        )))
        .with_model_defaults(test_model_defaults())
        .with_client(Arc::new(HistoryDrivenProvider))
        .build()
        .expect("rebuilt step agent should build");
        let rebuilt_outcome = rebuilt_agent.run_single_step().await;
        rebuilt = rebuilt_outcome.session;

        assert_eq!(
            serde_json::to_value(yolo.session()).expect("yolo session should serialize"),
            serde_json::to_value(&rebuilt).expect("rebuilt session should serialize"),
            "yolo and rebuilt step sessions diverged"
        );

        if matches!(
            rebuilt.step(),
            PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
        ) {
            break;
        }
    }

    assert!(matches!(
        yolo.session().step(),
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
    ));
    assert!(matches!(
        rebuilt.step(),
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
    ));
}

#[tokio::test]
async fn compact_trajectory_matches_yolo_run_and_rebuilt_step_agents() {
    let initial = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("x".repeat(1_000))],
    );

    let mut rebuilt_step_session = initial.clone();
    let mut step_trajectory = Vec::new();
    for _ in 0..8 {
        rebuilt_step_session = compact_equivalence_agent(
            rebuilt_step_session,
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        )
        .run_single_step()
        .await
        .session;
        step_trajectory.push(rebuilt_step_session.clone());
        if matches!(
            rebuilt_step_session.step(),
            PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
        ) {
            break;
        }
    }

    assert_eq!(step_trajectory.len(), 5);
    assert!(matches!(
        step_trajectory[0].step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate: 0.1 })
    ));
    assert!(matches!(
        step_trajectory[1].step(),
        PhiAgentStep::ReAct(PhiReActStep::Compacted)
    ));
    assert!(matches!(
        step_trajectory[2].step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. })
    ));
    assert!(matches!(
        step_trajectory[3].step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. })
    ));
    assert!(matches!(
        step_trajectory[4].step(),
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })
    ));

    let mut persistent_yolo = compact_equivalence_agent(
        initial.clone(),
        PhiAgentCommand::Yolo(PhiAgentCommand::yolo()),
    );
    for expected in &step_trajectory {
        persistent_yolo.step().await;
        assert_eq!(
            serialized_session(&persistent_yolo.session()),
            serialized_session(expected),
            "persistent yolo step diverged from rebuilt step trajectory"
        );
    }

    let yolo_final = run_to_completed(compact_equivalence_agent(
        initial.clone(),
        PhiAgentCommand::Yolo(PhiAgentCommand::yolo()),
    ))
    .await
    .session;
    assert_eq!(
        serialized_session(&yolo_final),
        serialized_session(step_trajectory.last().expect("trajectory should finish"))
    );

    let first_run = run_to_completion(compact_equivalence_agent(
        initial,
        PhiAgentCommand::Run(PhiAgentCommand::run()),
    ))
    .await
    .session;
    assert_eq!(
        serialized_session(&first_run),
        serialized_session(&step_trajectory[0]),
        "run should expose the request-compact boundary"
    );

    let final_run = run_to_completion(compact_equivalence_agent(
        first_run,
        PhiAgentCommand::Run(PhiAgentCommand::run()),
    ))
    .await
    .session;
    assert_eq!(
        serialized_session(&final_run),
        serialized_session(step_trajectory.last().expect("trajectory should finish")),
        "repeated run should reach the same TurnEnd session"
    );
}
