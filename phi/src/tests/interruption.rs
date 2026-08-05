use std::{
    future,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::{
    error::PhiAgentRuntimeResult,
    executor::{ToolCallOutput, ToolCallRequest},
    headlessterm::{HeadlessTerminal, JobHandle, ReturnWhen},
    message::{PhiHistory, PhiMessage, PhiToolMessage},
    module::{PhiAgentStepEvent, PhiModule},
    render::{PhiModelResponse, PhiProviderCall, TestClient},
    session::{PhiAgentStep, PhiReActStep, Session},
    tests::support::{step_agent_builder, test_model_defaults},
};

use super::support::unique_test_home;

struct PendingProvider {
    started: Arc<Notify>,
}

struct MarkInterruptedToolResult;

impl PhiModule for MarkInterruptedToolResult {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        let PhiAgentStepEvent::AfterToolCall { result, .. } = event else {
            return Ok(());
        };
        let mut output = result.output.as_value().clone();
        output["after_tool_call"] = serde_json::Value::Bool(true);
        result.output = ToolCallOutput::new(output);
        Ok(())
    }
}

#[async_trait]
impl TestClient for PendingProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        self.started.notify_one();
        future::pending().await
    }
}

#[tokio::test]
async fn interrupted_step_leaves_the_pre_call_session_checkpoint_serializable() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("keep me")],
    );
    let started = Arc::new(Notify::new());
    let mut agent = step_agent_builder(session)
        .with_client(Arc::new(PendingProvider {
            started: Arc::clone(&started),
        }))
        .build()
        .unwrap();
    let checkpoint = agent.session();
    let interrupt = async move {
        started.notified().await;
        Ok(())
    };

    assert!(matches!(
        crate::step_or_interrupt(&mut agent, interrupt)
            .await
            .unwrap(),
        crate::CliAgentExit::Interrupted
    ));

    let mut serialized = Vec::new();
    checkpoint.write_json(&mut serialized).unwrap();
    let restored = Session::load_bytes(&serialized).unwrap();
    assert_eq!(restored.history(), checkpoint.history());
    assert_eq!(restored.step(), checkpoint.step());
}

#[tokio::test]
async fn interrupted_job_interact_commits_a_structured_tool_result_before_exit() {
    let terminal = HeadlessTerminal::new();
    let command = if cfg!(windows) {
        "Start-Sleep -Seconds 30"
    } else {
        "sleep 30"
    };
    let (handle, initial) = terminal
        .exec_job(
            command,
            ReturnWhen::output_settled(std::time::Duration::ZERO),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(initial.is_running());
    let handle = handle.unwrap();
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            vec![ToolCallRequest {
                id: "call_1".into(),
                call_id: Some("call_1".into()),
                name: "job_interact".into(),
                arguments: serde_json::json!({
                    "handle": handle.0,
                    "wait_ms": 30_000,
                }),
            }],
        ),
        vec![PhiMessage::user("wait for it")],
    );
    let mut agent = step_agent_builder(session)
        .with_module(MarkInterruptedToolResult)
        .build()
        .unwrap();

    let exit = crate::step_or_interrupt(&mut agent, async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Ok(())
    })
    .await
    .unwrap();

    assert!(matches!(exit, crate::CliAgentExit::InterruptedAfterStep));
    assert!(matches!(
        agent.session().step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. })
    ));
    let history = agent.session().history();
    assert!(matches!(
        &history[1],
        PhiMessage::Tool(PhiToolMessage::ToolCall { id, name, .. })
            if id.as_deref() == Some("call_1") && name == "job_interact"
    ));
    assert!(matches!(
        &history[2],
        PhiMessage::Tool(PhiToolMessage::ToolResult { id, name, result })
            if id.as_deref() == Some("call_1")
                && name.as_deref() == Some("job_interact")
                && result["status"] == "running_user_interrupted"
                && result["running"] == true
                && result["after_tool_call"] == true
                && result["output"] == ""
                && result["truncated"] == false
                && result["handle"] == handle.0
                && result["waited_ms"].as_u64().is_some_and(|waited| waited >= 50)
                && result["waited_ms"].as_u64().is_some_and(|waited| waited < 5_000)
    ));

    let _ = terminal.close_job(JobHandle(handle.0)).await;
}

struct PanickingProvider;

#[async_trait]
impl TestClient for PanickingProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        panic!("provider panic")
    }
}

#[tokio::test]
async fn panicked_step_returns_the_pre_call_session_checkpoint_and_payload() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("keep me")],
    );
    let mut agent = step_agent_builder(session)
        .with_client(Arc::new(PanickingProvider))
        .build()
        .unwrap();
    let checkpoint = agent.session();

    let exit = crate::step_or_interrupt(&mut agent, future::pending())
        .await
        .unwrap();
    let crate::CliAgentExit::Panicked(payload) = exit else {
        panic!("step should report its panic")
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"provider panic"));

    let mut serialized = Vec::new();
    checkpoint.write_json(&mut serialized).unwrap();
    let restored = Session::load_bytes(&serialized).unwrap();
    assert_eq!(restored.history(), checkpoint.history());
    assert_eq!(restored.step(), checkpoint.step());
}

#[test]
fn panicked_cli_agent_persists_checkpoint_before_resuming_unwind() {
    let checkpoint = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("persist me")],
    );
    let session_path = unique_test_home().join("session.json");
    checkpoint.save(&session_path).unwrap();
    let session_input = crate::SessionInput::FileBacked {
        path: session_path.clone(),
        session: checkpoint.clone(),
        stdin_user_message: None,
    };

    let panic = catch_unwind(AssertUnwindSafe(|| {
        crate::persist_cli_agent_session(
            checkpoint.clone(),
            crate::CliAgentExit::Panicked(Box::new("original panic")),
            &session_input,
            true,
        )
        .unwrap();
    }))
    .expect_err("panic should resume after checkpoint persistence");

    assert_eq!(panic.downcast_ref::<&str>(), Some(&"original panic"));
    let restored = Session::load(&session_path).unwrap();
    assert_eq!(restored.history(), checkpoint.history());
    assert_eq!(restored.step(), checkpoint.step());
}
