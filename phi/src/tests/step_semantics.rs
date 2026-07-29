use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{
    agent::PhiAgentCommand,
    error::{PhiErrorKind, PhiRuntimeError, PhiRuntimeResult},
    executor::ToolCallRequest,
    home::LocalPhiHome,
    message::{PhiHistory, PhiMessage, PhiToolMessage},
    module::{PhiAgentStepEvent, PhiModule},
    render::{PhiModelResponse, PhiModelTurnState, PhiProviderCall, TestClient},
    session::{PhiAgentStep, Session},
};

use super::support::{
    default_step_agent_builder, shell_echo_ok_command, shell_stdout_ok, shell_tool_name,
    step_agent_builder, stub_client, test_model_defaults,
};

struct EmptyProvider;
struct SequenceProvider {
    responses: Mutex<VecDeque<PhiModelResponse>>,
}

#[async_trait]
impl TestClient for EmptyProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiRuntimeResult<PhiModelResponse> {
        Ok(PhiModelResponse::unspecified(Vec::new()))
    }
}

#[async_trait]
impl TestClient for SequenceProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiRuntimeResult<PhiModelResponse> {
        self.responses
            .lock()
            .expect("response queue should be healthy")
            .pop_front()
            .ok_or_else(|| PhiRuntimeError::provider_response("response queue exhausted"))
    }
}

struct RejectAfterToolCallModule;
struct RejectFirstModelResponseModule {
    rejected: bool,
}

impl PhiModule for RejectAfterToolCallModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        if let PhiAgentStepEvent::AfterToolCall { .. } = event {
            return Err(PhiRuntimeError::module("module rejected tool result")
                .with_source_step("step_tool"));
        }
        Ok(())
    }
}

impl PhiModule for RejectFirstModelResponseModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        if matches!(event, PhiAgentStepEvent::AfterModelResponse { .. }) && !self.rejected {
            self.rejected = true;
            return Err(
                PhiRuntimeError::module("module rejected first model response")
                    .with_source_step("request_complete"),
            );
        }
        Ok(())
    }
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
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
        PhiAgentStep::RequestExecutor { pending_messages, tool_calls, .. }
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
    assert_eq!(result["ok"], serde_json::json!(true));
    assert_eq!(result["error"], serde_json::Value::Null);
    assert_eq!(
        result["value"]["output"],
        serde_json::json!(shell_stdout_ok())
    );
    assert_eq!(result["value"]["status"], serde_json::json!("exited"));
    assert_eq!(result["value"]["exit_code"], serde_json::json!(0));
    assert_eq!(result["value"]["handle"], serde_json::Value::Null);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::RequestComplete { detail, .. }
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn invariant_failed_tool_step_drops_pending_messages_and_resumes_cleanly() {
    let session = pending_tool_session(
        vec![PhiMessage::user("hello")],
        vec![PhiMessage::assistant("running bash now")],
        serde_json::json!({ "cmd": shell_echo_ok_command() }),
    );

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
        PhiAgentStep::Failed { error }
        if error.kind() == PhiErrorKind::Module
            && error.detail() == "module rejected tool result"
    ));

    let resumed = default_step_agent_builder(failed)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert_eq!(resumed.history(), &[PhiMessage::user("hello")]);
    assert!(matches!(
        resumed.step(),
        PhiAgentStep::RequestComplete { detail, .. }
        if detail == "resuming from failed step"
    ));
}

#[tokio::test]
async fn invariant_completed_and_failed_steps_only_resume_never_execute_immediately() {
    let completed = Session::from_root(
        PhiAgentStep::completed("done"),
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
        PhiAgentStep::RequestComplete { .. }
    ));

    let failed = Session::from_root(
        PhiAgentStep::failed(PhiRuntimeError::internal("failed")),
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
    assert!(matches!(
        resumed_failed.step(),
        PhiAgentStep::RequestComplete { .. }
    ));
}

#[tokio::test]
async fn yolo_continues_when_run_would_stop_at_runtime_failure() {
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
    let run_outcome = run_builder
        .with_module_layout(run_modules)
        .with_module(RejectFirstModelResponseModule { rejected: false })
        .build()
        .expect("run agent should build")
        .run_to_completion()
        .await;

    assert!(matches!(
        run_outcome.session.step(),
        PhiAgentStep::Failed { error }
        if error.kind() == PhiErrorKind::Module
            && error.detail() == "module rejected first model response"
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
    let yolo_outcome = yolo_builder
        .with_module_layout(yolo_modules)
        .with_module(RejectFirstModelResponseModule { rejected: false })
        .build()
        .expect("yolo agent should build")
        .run_to_completed()
        .await;

    assert!(yolo_outcome.error.is_none());
    assert_eq!(
        yolo_outcome.session.history(),
        vec![PhiMessage::user("hello"), PhiMessage::assistant("ok")]
    );
    assert!(matches!(
        yolo_outcome.session.step(),
        PhiAgentStep::Completed { .. }
    ));
}

#[tokio::test]
async fn yolo_continues_after_provider_requests_follow_up_without_a_tool_call() {
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
    let outcome =
        crate::agent::PhiAgent::builder(session, PhiAgentCommand::Yolo(PhiAgentCommand::yolo()))
            .with_home(Arc::new(LocalPhiHome::new(
                super::support::unique_test_home(),
            )))
            .with_model_defaults(test_model_defaults())
            .with_client(Arc::new(client))
            .build()
            .expect("agent should build")
            .run_to_completed()
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
        PhiAgentStep::Completed { .. }
    ));
}
