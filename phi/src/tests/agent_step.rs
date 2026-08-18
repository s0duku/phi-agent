use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::{
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    executor::{PhiTool, ToolCallRequest, ToolCallResponse},
    home::LocalPhiHome,
    message::{PhiAssistantMessage, PhiHistory, PhiMessage, PhiReasoningContent},
    module::{PhiAgentCommitEvent, PhiAgentStepEvent, PhiModule},
    render::{PhiModelResponse, PhiModelTurnState, PhiProviderCall, TestClient},
    session::{PhiAgentStep, PhiReActStep, Session},
};

use super::support::{
    default_step_agent_builder, shell_echo_ok_command, shell_echo_rewritten_command,
    shell_stdout_ok, shell_tool_name, test_model_defaults,
};
use super::support::{env_lock, step_agent_builder, stub_client};

struct RewriteToolCallModule;
struct RejectAfterModelResponseModule;
struct RejectAfterToolCallModule;
struct RejectExecutorReplaceModule;
struct RewriteAllAssistantMessagesModule;
struct CaptureModelRequestModule {
    captured_temperature: Arc<Mutex<Option<f64>>>,
    captured_messages: Arc<Mutex<Vec<PhiMessage>>>,
}
struct CaptureEchoEventsModule {
    events: Arc<Mutex<Vec<String>>>,
}
struct CaptureWarningsModule {
    warnings: Arc<Mutex<Vec<String>>>,
}
struct CaptureCommitEventsModule {
    events: Arc<Mutex<Vec<&'static str>>>,
}
struct RewriteArgumentsInsideTool;
struct RegisterRewriteArgumentsToolModule;
struct RegisterStructuredFailureToolModule;
struct RegisterDeterministicSuccessToolModule;
struct StructuredFailureTool;
struct DeterministicSuccessTool;

#[derive(serde::Serialize)]
struct TestStructuredError {
    code: u16,
    reason: &'static str,
}

impl crate::error::PhiStructureError for TestStructuredError {
    fn into_value(self: Box<Self>) -> serde_json::Value {
        serde_json::to_value(*self).unwrap()
    }
}
struct CaptureRequestToolsProvider {
    request: Arc<Mutex<Option<PhiProviderCall>>>,
    response: Vec<PhiMessage>,
}
struct ModelResponseProvider {
    response: PhiModelResponse,
}

impl PhiModule for RewriteToolCallModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        if let PhiAgentStepEvent::BeforeToolCall { request, .. } = event {
            request.arguments = serde_json::json!({ "cmd": shell_echo_rewritten_command() });
        }
        Ok(())
    }
}

impl PhiModule for RejectAfterModelResponseModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        if let PhiAgentStepEvent::AfterModelResponse { .. } = event {
            return Err(PhiAgentRuntimeError::module(
                "module rejected model response",
            ));
        }
        Ok(())
    }
}

impl PhiModule for RejectAfterToolCallModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        if let PhiAgentStepEvent::AfterToolCall { .. } = event {
            return Err(PhiAgentRuntimeError::module("module rejected tool result"));
        }
        Ok(())
    }
}

impl PhiModule for RejectExecutorReplaceModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        if matches!(event, PhiAgentStepEvent::BeforeReplaceBaseStep { .. }) {
            return Err(PhiAgentRuntimeError::module(
                "module rejected executor replace",
            ));
        }
        Ok(())
    }
}

impl PhiModule for RewriteAllAssistantMessagesModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        let PhiAgentStepEvent::AfterModelResponse { assistant, .. } = event else {
            return Ok(());
        };

        if let Some(content) = &mut assistant.content {
            *content = "rewritten text".to_string();
        }
        for reasoning in &mut assistant.reasoning {
            reasoning.content = vec![PhiReasoningContent::Summary(
                "rewritten reasoning".to_string(),
            )];
        }

        Ok(())
    }
}

impl PhiModule for CaptureModelRequestModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        let PhiAgentStepEvent::BeforeModelRequest {
            history, request, ..
        } = event
        else {
            return Ok(());
        };

        request.temperature = Some(0.42);
        history.push(PhiMessage::system("middleware-added context"));
        *self
            .captured_temperature
            .lock()
            .expect("captured temperature mutex should not be poisoned") = request.temperature;
        *self
            .captured_messages
            .lock()
            .expect("captured messages mutex should not be poisoned") = history.to_messages();
        Ok(())
    }
}

impl PhiModule for CaptureEchoEventsModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        let label = match event {
            PhiAgentStepEvent::AfterModelResponseParsed { messages } => {
                format!("messages:{}", messages.len())
            }
            PhiAgentStepEvent::BeforeToolCall { request, .. } => {
                format!("tool_call:{}", request.name)
            }
            PhiAgentStepEvent::AfterToolCall { result, .. } => {
                format!("tool_result:{}", result.name)
            }
            _ => return Ok(()),
        };

        self.events
            .lock()
            .expect("echo event mutex should not be poisoned")
            .push(label);
        Ok(())
    }
}

impl PhiModule for CaptureWarningsModule {
    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        let PhiAgentCommitEvent::WarningEmitted { message } = event else {
            return;
        };
        self.warnings
            .lock()
            .expect("warning mutex should not be poisoned")
            .push((*message).to_string());
    }
}

impl PhiModule for CaptureCommitEventsModule {
    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        let label = match event {
            PhiAgentCommitEvent::ModelResponseCommitted => "model",
            PhiAgentCommitEvent::MessageCommitted {
                message: PhiMessage::ToolResult(_),
            } => "tool_result",
            _ => return,
        };
        self.events.lock().unwrap().push(label);
    }
}

impl PhiModule for RegisterRewriteArgumentsToolModule {
    fn module_tools(
        &mut self,
        _context: &crate::agent::PhiAgentBuildContext,
    ) -> Vec<Arc<dyn PhiTool>> {
        vec![Arc::new(RewriteArgumentsInsideTool)]
    }
}

impl PhiModule for RegisterStructuredFailureToolModule {
    fn module_tools(
        &mut self,
        _context: &crate::agent::PhiAgentBuildContext,
    ) -> Vec<Arc<dyn PhiTool>> {
        vec![Arc::new(StructuredFailureTool)]
    }
}

impl PhiModule for RegisterDeterministicSuccessToolModule {
    fn module_tools(
        &mut self,
        _context: &crate::agent::PhiAgentBuildContext,
    ) -> Vec<Arc<dyn PhiTool>> {
        vec![Arc::new(DeterministicSuccessTool)]
    }
}

#[async_trait]
impl PhiTool for StructuredFailureTool {
    fn name(&self) -> &str {
        "structured_failure"
    }

    fn description(&self) -> &str {
        "Return a structured test failure."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn call(
        &self,
        _request: &mut ToolCallRequest,
        _runtime: &crate::agent::PhiAgentRuntime,
    ) -> crate::executor::PhiToolResult {
        Err(Box::new(TestStructuredError {
            code: 73,
            reason: "terminal unavailable",
        }))
    }
}

#[async_trait]
impl PhiTool for DeterministicSuccessTool {
    fn name(&self) -> &str {
        "deterministic_success"
    }

    fn description(&self) -> &str {
        "Return a deterministic successful test result."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn call(
        &self,
        request: &mut ToolCallRequest,
        _runtime: &crate::agent::PhiAgentRuntime,
    ) -> crate::executor::PhiToolResult {
        Ok(ToolCallResponse::new(
            request,
            self.name(),
            serde_json::json!({"status": "exited"}),
        ))
    }
}

#[async_trait]
impl PhiTool for RewriteArgumentsInsideTool {
    fn name(&self) -> &str {
        "rewrite_args"
    }

    fn description(&self) -> &str {
        "Rewrite tool arguments before execution."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            },
            "required": ["value"],
            "additionalProperties": false
        })
    }

    async fn call(
        &self,
        request: &mut ToolCallRequest,
        _runtime: &crate::agent::PhiAgentRuntime,
    ) -> crate::executor::PhiToolResult {
        request.arguments = serde_json::json!({ "value": "rewritten-inside-tool" });
        Ok(ToolCallResponse::new(
            request,
            self.name(),
            serde_json::json!({ "ok": true }),
        ))
    }
}

struct EmptyProvider;

#[async_trait]
impl TestClient for CaptureRequestToolsProvider {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        *self
            .request
            .lock()
            .expect("request capture mutex should be healthy") = Some(request.clone());
        Ok(PhiModelResponse::unspecified(self.response.clone()))
    }
}

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
impl TestClient for ModelResponseProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn null_executor_request_provider_keeps_only_custom_tools_from_config() {
    let _lock = env_lock();
    let root = unique_temp_dir("phi-request-complete-tools");
    std::fs::create_dir_all(&root).expect("temp home root should be creatable");
    std::fs::write(
        root.join("config.yml"),
        r#"tools:
  - name: external_lookup
    description: External lookup
    parameters:
      type: object
      properties:
        query:
          type: string
      required: [query]
      additionalProperties: false
"#,
    )
    .expect("config.yml should be writable");

    let captured_request = Arc::new(Mutex::new(None));
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );
    let outcome = crate::agent::PhiAgent::builder(
        session,
        crate::agent::PhiAgentCommand::Step(
            crate::agent::PhiAgentCommand::step().with_null_executor(true),
        ),
    )
    .with_home(Arc::new(LocalPhiHome::new(root.clone())))
    .with_model_defaults(test_model_defaults())
    .with_client(Arc::new(CaptureRequestToolsProvider {
        request: captured_request.clone(),
        response: vec![PhiMessage::assistant("ok")],
    }))
    .build()
    .expect("agent should build")
    .run_single_step()
    .await;

    let request = captured_request
        .lock()
        .expect("request capture mutex should be healthy")
        .clone()
        .expect("provider should receive a request");
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.tools[0].name, "external_lookup");
    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("hello"), PhiMessage::assistant("ok")]
    );

    std::fs::remove_dir_all(root).expect("temp home should be removable");
}

#[tokio::test]
async fn request_provider_commits_assistant_response_before_completion() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );
    let outcome = crate::agent::PhiAgent::builder(
        session,
        crate::agent::PhiAgentCommand::Step(crate::agent::PhiAgentCommand::step()),
    )
    .with_home(Arc::new(LocalPhiHome::new(
        super::support::unique_test_home(),
    )))
    .with_model_defaults(super::support::test_model_defaults())
    .with_client(stub_client(vec![PhiMessage::assistant("world")]))
    .build()
    .expect("agent should build")
    .run_single_step()
    .await;

    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("hello"), PhiMessage::assistant("world")]
    );
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::TurnEnd { detail })
        if detail == "model response committed; no tool execution is pending"
    ));
}

#[tokio::test]
async fn provider_continue_response_commits_assistant_and_requests_another_completion() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("inspect files")],
    );
    let outcome = step_agent_builder(session)
        .with_client(Arc::new(ModelResponseProvider {
            response: PhiModelResponse::new(
                vec![PhiMessage::assistant("I will inspect the files now.")],
                PhiModelTurnState::Continue,
            ),
        }))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        outcome.session.history(),
        &[
            PhiMessage::user("inspect files"),
            PhiMessage::assistant("I will inspect the files now."),
        ]
    );
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
        if detail == "provider response requires another model request"
    ));
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}

#[tokio::test]
async fn request_compact_leaves_all_system_history_unchanged() {
    let history = vec![PhiMessage::system("sys1"), PhiMessage::system("sys2")];
    let session = Session::from_root(PhiAgentStep::request_compact(), history.clone());

    let outcome = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), history);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::Compacted)
    ));
}

#[tokio::test]
async fn request_compact_retains_recent_user_tail_and_compacts_everything_else() {
    let big = "x".repeat(200_000);
    let tool_call = PhiMessage::tool_call(
        Some("call-1".into()),
        "bash",
        serde_json::json!({"cmd": "echo ok"}),
    );
    let tool_result = PhiMessage::tool_result(
        Some("call-1".into()),
        Some("bash".into()),
        serde_json::json!({"ok": true}),
    );
    let session = Session::from_root(
        PhiAgentStep::request_compact(),
        vec![
            PhiMessage::system("sys"),
            PhiMessage::user(big.clone()),
            PhiMessage::assistant("a1"),
            tool_call.clone(),
            tool_result.clone(),
            PhiMessage::user("second"),
            PhiMessage::assistant("a2"),
            PhiMessage::user("third"),
        ],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::assistant("summary")]))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    let history = outcome.session.history();
    assert_eq!(history.len(), 8);
    assert_eq!(history[0], PhiMessage::system("sys"));
    assert_eq!(
        history[1],
        PhiMessage::user("<compaction>summary</compaction>")
    );
    assert_eq!(history[2], PhiMessage::assistant("a1"));
    assert_eq!(history[3], tool_call);
    assert_eq!(history[4], tool_result);
    assert_eq!(history[5], PhiMessage::user("second"));
    assert_eq!(history[6], PhiMessage::assistant("a2"));
    assert_eq!(history[7], PhiMessage::user("third"));

    let expr = outcome.session.clone().into_expr();
    let compacted = &expr;
    assert!(matches!(
        compacted.step(),
        PhiAgentStep::ReAct(PhiReActStep::Compacted)
    ));
    assert_eq!(history, compacted.delta().history().to_messages());
    assert_eq!(
        compacted
            .expr()
            .expect("compacted frame should retain original full-history expr")
            .history()
            .to_messages(),
        vec![
            PhiMessage::system("sys"),
            PhiMessage::user(big.clone()),
            PhiMessage::assistant("a1"),
            PhiMessage::tool_call(
                Some("call-1".into()),
                "bash",
                serde_json::json!({"cmd": "echo ok"}),
            ),
            PhiMessage::tool_result(
                Some("call-1".into()),
                Some("bash".into()),
                serde_json::json!({"ok": true}),
            ),
            PhiMessage::user("second"),
            PhiMessage::assistant("a2"),
            PhiMessage::user("third"),
        ]
    );
}

#[tokio::test]
async fn request_compact_failure_enters_failed_step_and_preserves_history() {
    let input_history = vec![
        PhiMessage::system("sys"),
        PhiMessage::user("hello"),
        PhiMessage::assistant("world"),
    ];
    let session = Session::from_root(PhiAgentStep::request_compact(), input_history.clone());

    let render = crate::render::PhiRender::from_test_client(Arc::new(EmptyProvider))
        .with_compact_override(Arc::new(|_history| {
            Err(crate::error::PhiAgentRuntimeError::request_compact(
                "compact exploded",
            ))
        }));

    let outcome = step_agent_builder(session)
        .with_render(render)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), input_history);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::Failed(failed)
        if matches!(failed.error(), PhiAgentRuntimeError::RequestCompact { .. })
            && failed.error().detail() == "compact exploded"
    ));
}

#[tokio::test]
async fn before_model_request_rewrites_explicit_request_payload() {
    let captured_temperature = Arc::new(Mutex::new(None));
    let captured_messages = Arc::new(Mutex::new(Vec::new()));

    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::assistant("world")]))
        .with_module(CaptureModelRequestModule {
            captured_temperature: captured_temperature.clone(),
            captured_messages: captured_messages.clone(),
        })
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        *captured_temperature
            .lock()
            .expect("captured temperature mutex should not be poisoned"),
        Some(0.42)
    );
    assert_eq!(
        *captured_messages
            .lock()
            .expect("captured messages mutex should not be poisoned"),
        vec![
            PhiMessage::user("hello"),
            PhiMessage::system("middleware-added context"),
        ]
    );
    assert_eq!(
        outcome.session.history(),
        &[
            PhiMessage::user("hello"),
            PhiMessage::system("middleware-added context"),
            PhiMessage::assistant("world"),
        ]
    );
}

#[tokio::test]
async fn after_model_rejection_does_not_commit_partial_model_history() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::assistant("world")]))
        .with_module(RejectAfterModelResponseModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), &[PhiMessage::user("hello")]);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::Failed(failed)
        if matches!(failed.error(), PhiAgentRuntimeError::Module { .. })
            && failed.error().detail() == "module rejected model response"
    ));
}

#[tokio::test]
async fn after_model_response_rewrites_are_committed_for_all_assistant_messages() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::Assistant(
            PhiAssistantMessage::from_parts(
                Some("original text".into()),
                vec![crate::message::PhiReasoningBlock {
                    id: None,
                    content: vec![PhiReasoningContent::Summary("original reasoning".into())],
                }],
                Vec::new(),
            ),
        )]))
        .with_module(RewriteAllAssistantMessagesModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        outcome.session.history(),
        &[
            PhiMessage::user("hello"),
            PhiMessage::Assistant(crate::message::PhiAssistantMessage::from_parts(
                Some("rewritten text".into()),
                vec![crate::message::PhiReasoningBlock {
                    id: None,
                    content: vec![PhiReasoningContent::Summary("rewritten reasoning".into())],
                }],
                Vec::new(),
            )),
        ]
    );
}

#[tokio::test]
async fn failed_step_without_default_module_stays_failed() {
    let session = Session::from_root(
        PhiAgentStep::failed(PhiAgentRuntimeError::module("failed")),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), &[PhiMessage::user("hello")]);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::Failed(failed) if failed.error().detail() == "failed"
    ));

    let resumed = default_step_agent_builder(outcome.session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert!(matches!(resumed.session.step(), PhiAgentStep::Failed(_)));
}

#[tokio::test]
async fn tool_call_message_commits_rewritten_request_payload() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::tool_calls(vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": "echo original" }),
            }]),
        ),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .with_module(RewriteToolCallModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    let history = outcome.session.history();
    let tool_call = history
        .iter()
        .find_map(|message| match message {
            PhiMessage::Assistant(assistant) => assistant
                .tool_calls
                .first()
                .map(|request| &request.arguments),
            _ => None,
        })
        .expect("tool call message should be committed");

    assert_eq!(
        tool_call,
        &serde_json::json!({ "cmd": shell_echo_rewritten_command() })
    );
}

#[tokio::test]
async fn tool_call_only_response_transitions_to_request_executor() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("list files")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::tool_call(
            Some("call_1".to_string()),
            shell_tool_name(),
            serde_json::json!({ "cmd": "ls" }),
        )]))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), &[PhiMessage::user("list files")]);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { pending_messages, assistant, .. })
        if pending_messages.is_empty()
            && assistant.tool_calls.len() == 1
            && assistant.tool_calls[0].name == shell_tool_name()
            && assistant.tool_calls[0].call_id.as_deref() == Some("call_1")
            && assistant.tool_calls[0].arguments == serde_json::json!({ "cmd": "ls" })
    ));
}

#[tokio::test]
async fn tool_internal_argument_rewrite_is_committed_into_history() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("running custom tool").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: "rewrite_args".to_string(),
                    arguments: serde_json::json!({ "value": "original" }),
                },
            ]),
        ),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_module(RegisterRewriteArgumentsToolModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    let history = outcome.session.history();
    let tool_call = history
        .iter()
        .find_map(|message| match message {
            PhiMessage::Assistant(assistant) => assistant
                .tool_calls
                .first()
                .map(|request| &request.arguments),
            _ => None,
        })
        .expect("tool call should be committed");

    assert_eq!(
        tool_call,
        &serde_json::json!({ "value": "rewritten-inside-tool" })
    );
}

#[tokio::test]
async fn assistant_and_tool_call_response_stays_pending_until_tool_step_commits() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("list files")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::Assistant(
            PhiAssistantMessage::text("running bash now").with_tool_calls(vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": "ls" }),
            }]),
        )]))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(outcome.session.history(), &[PhiMessage::user("list files")]);
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { pending_messages, assistant, .. })
        if pending_messages.is_empty()
            && assistant.content.as_deref() == Some("running bash now")
            && assistant.tool_calls.len() == 1
            && assistant.tool_calls[0].name == shell_tool_name()
            && assistant.tool_calls[0].call_id.as_deref() == Some("call_1")
            && assistant.tool_calls[0].arguments == serde_json::json!({ "cmd": "ls" })
    ));
}

#[tokio::test]
async fn echo_events_follow_response_parse_and_tool_evaluation_boundaries() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("list files")],
    );

    let first = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::Assistant(
            PhiAssistantMessage::text("running bash now").with_tool_calls(vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
            }]),
        )]))
        .with_module(CaptureEchoEventsModule {
            events: events.clone(),
        })
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert_eq!(
        *events
            .lock()
            .expect("echo event mutex should not be poisoned"),
        vec!["messages:1"]
    );

    let _second = step_agent_builder(first)
        .with_client(Arc::new(EmptyProvider))
        .with_module(CaptureEchoEventsModule {
            events: events.clone(),
        })
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        *events
            .lock()
            .expect("echo event mutex should not be poisoned"),
        vec![
            "messages:1".to_string(),
            format!("tool_call:{}", shell_tool_name()),
            format!("tool_result:{}", shell_tool_name()),
        ]
    );
}

#[tokio::test]
async fn multiple_tool_calls_response_transitions_to_request_executor_queue() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("run two commands")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![PhiMessage::Assistant(
            PhiAssistantMessage::text("running two tools").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": "printf first" }),
                },
                ToolCallRequest {
                    id: "call_2".to_string(),
                    call_id: Some("call_2".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": "printf second" }),
                },
            ]),
        )]))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("run two commands")]
    );
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { pending_messages, assistant, .. })
        if pending_messages.is_empty()
            && assistant.content.as_deref() == Some("running two tools")
            && assistant.tool_calls.len() == 2
            && assistant.tool_calls[0].call_id.as_deref() == Some("call_1")
            && assistant.tool_calls[1].call_id.as_deref() == Some("call_2")
    ));
}

#[tokio::test]
async fn multiple_tool_calls_execute_sequentially_without_dropping_queue() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("running two tools").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
                },
                ToolCallRequest {
                    id: "call_2".to_string(),
                    call_id: Some("call_2".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
                },
            ]),
        ),
        vec![PhiMessage::user("hello")],
    );

    let first = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert_eq!(first.history(), &[PhiMessage::user("hello")]);
    assert_eq!(
        serde_json::to_value(&first).unwrap()["frames"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "advancing within one tool batch must replace its executor frame"
    );
    assert!(matches!(
        first.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor {
            pending_messages,
            assistant,
            pending_results,
            detail,
        })
        if detail == "additional tool execution is pending"
            && pending_messages.is_empty()
            && assistant.content.as_deref() == Some("running two tools")
            && assistant.tool_calls.len() == 2
            && pending_results.len() == 1
            && pending_results[0].id.as_deref() == Some("call_1")
    ));

    let second = step_agent_builder(first)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert_eq!(
        serde_json::to_value(&second).unwrap()["frames"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "completing one tool batch must create a provider frame over its executor"
    );
    assert_eq!(second.history().len(), 4);
    assert!(
        matches!(&second.history()[1], PhiMessage::Assistant(assistant)
        if assistant.tool_calls.len() == 2)
    );
    assert!(
        matches!(&second.history()[2], PhiMessage::ToolResult(result)
        if result.id.as_deref() == Some("call_1"))
    );
    assert!(
        matches!(&second.history()[3], PhiMessage::ToolResult(result)
        if result.id.as_deref() == Some("call_2"))
    );
    assert!(matches!(
        second.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn failed_executor_replace_expands_over_the_original_executor() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::tool_calls(vec![
                ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
                },
                ToolCallRequest {
                    id: "call_2".to_string(),
                    call_id: Some("call_2".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
                },
            ]),
        ),
        vec![PhiMessage::user("hello")],
    );

    let failed = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .with_module(RejectExecutorReplaceModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert!(matches!(failed.step(), PhiAgentStep::Failed(_)));
    let rolled_back = Session::rollback(failed);
    assert_eq!(rolled_back.history(), &[PhiMessage::user("hello")]);
    assert!(matches!(
        rolled_back.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { pending_results, .. })
            if pending_results.is_empty()
    ));
}

#[tokio::test]
async fn after_tool_call_rejection_does_not_commit_half_finished_tool_history() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("running bash now").with_tool_calls(vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
            }]),
        ),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .with_module(RejectAfterToolCallModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("hello")],
        "tool steps must not leave behind any pending model/tool history when the step fails",
    );
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::Failed(failed)
        if matches!(failed.error(), PhiAgentRuntimeError::Module { .. })
            && failed.error().detail() == "module rejected tool result"
    ));
}

#[tokio::test]
async fn tool_step_commits_pending_assistant_then_tool_call_then_result() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("running bash now").with_tool_calls(vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
            }]),
        ),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    let history = outcome.session.history();
    assert_eq!(history[0], PhiMessage::user("hello"));
    assert!(matches!(&history[1], PhiMessage::Assistant(assistant)
        if assistant.content.as_deref() == Some("running bash now")
            && assistant.tool_calls.len() == 1
            && assistant.tool_calls[0].call_id.as_deref() == Some("call_1")));
    let PhiMessage::ToolResult(crate::message::PhiToolResultMessage { id, name, result }) =
        &history[2]
    else {
        panic!("fourth history entry should be a tool result");
    };
    assert_eq!(id.as_deref(), Some("call_1"));
    assert_eq!(name.as_deref(), Some(shell_tool_name()));
    assert_eq!(
        result["output"],
        serde_json::json!(shell_stdout_ok()),
        "unexpected shell tool result: {result:#}"
    );
    assert_eq!(
        result["status"],
        serde_json::json!("exited"),
        "unexpected shell tool result: {result:#}"
    );
    assert_eq!(
        result["exit_code"],
        serde_json::json!(0),
        "unexpected shell tool result: {result:#}"
    );
    assert_eq!(
        result["handle"],
        serde_json::Value::Null,
        "unexpected shell tool result: {result:#}"
    );
}

#[tokio::test]
async fn unknown_tool_fails_without_recovery_module() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("trying custom tool").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_missing".to_string(),
                    call_id: Some("call_missing".to_string()),
                    name: "no_exist".to_string(),
                    arguments: serde_json::json!({ "name": "phi" }),
                },
            ]),
        ),
        vec![PhiMessage::user("force call the missing tool")],
    );

    let outcome = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("force call the missing tool")]
    );
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::Failed(failed)
        if matches!(failed.error(), PhiAgentRuntimeError::ToolNotFound { .. })
            && failed.error().tool_request().is_some_and(|request| request.name == "no_exist")
            && failed.error().pending_messages().is_some_and(<[_]>::is_empty)
            && failed.error().assistant().is_some_and(|assistant|
                assistant.content.as_deref() == Some("trying custom tool")
                    && assistant.tool_calls.len() == 1)
    ));
}

#[tokio::test]
async fn structured_tool_error_bubbles_to_failed_and_default_recovery_commits_it() {
    let request = ToolCallRequest {
        id: "call_failed".to_string(),
        call_id: Some("call_failed".to_string()),
        name: "structured_failure".to_string(),
        arguments: serde_json::json!({}),
    };
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("trying structured failure").with_tool_calls(vec![request]),
        ),
        vec![PhiMessage::user("run failing tool")],
    );

    let failed = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .with_module(RegisterStructuredFailureToolModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert!(matches!(
        failed.step(),
        PhiAgentStep::Failed(failed)
            if matches!(failed.error(), PhiAgentRuntimeError::ToolError { .. })
                && failed.error().tool_error_detail() == Some(&serde_json::json!({
                    "code": 73,
                    "reason": "terminal unavailable"
                }))
    ));

    let outcome = default_step_agent_builder(failed)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;
    let history = outcome.session.history();
    let result = history
        .iter()
        .find_map(|message| match message {
            PhiMessage::ToolResult(crate::message::PhiToolResultMessage { result, .. }) => {
                Some(result)
            }
            _ => None,
        })
        .expect("recovery should commit a tool result");
    assert_eq!(
        result,
        &serde_json::json!({"code": 73, "reason": "terminal unavailable"})
    );
}

#[tokio::test]
async fn unknown_tool_recovery_commits_failure_result_and_resumes_model_flow() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("trying custom tool").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_missing".to_string(),
                    call_id: Some("call_missing".to_string()),
                    name: "no_exist".to_string(),
                    arguments: serde_json::json!({ "name": "phi" }),
                },
            ]),
        ),
        vec![PhiMessage::user("force call the missing tool")],
    );

    let failed = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    let warnings = Arc::new(Mutex::new(Vec::new()));
    let outcome = default_step_agent_builder(failed)
        .with_client(Arc::new(EmptyProvider))
        .with_module(CaptureWarningsModule {
            warnings: warnings.clone(),
        })
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    let history = outcome.session.history();
    assert_eq!(history[0], PhiMessage::user("force call the missing tool"));
    assert!(matches!(&history[1], PhiMessage::Assistant(assistant)
        if assistant.content.as_deref() == Some("trying custom tool")
            && assistant.tool_calls.len() == 1
            && assistant.tool_calls[0].call_id.as_deref() == Some("call_missing")));
    let PhiMessage::ToolResult(crate::message::PhiToolResultMessage { id, name, result }) =
        &history[2]
    else {
        panic!("fourth history entry should be a tool result");
    };
    assert_eq!(id.as_deref(), Some("call_missing"));
    assert_eq!(name.as_deref(), Some("no_exist"));
    assert_eq!(result["kind"], serde_json::json!("tool_not_found"));
    assert_eq!(result["tool_name"], serde_json::json!("no_exist"));
    let warnings = warnings
        .lock()
        .expect("warning mutex should not be poisoned")
        .clone();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("structured result for no_exist"));
    assert!(warnings[0].contains("\"kind\":\"tool_not_found\""));
    assert!(warnings[0].contains("\"tool_name\":\"no_exist\""));
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn tool_recovery_emits_model_and_tool_result_commit_events() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("trying custom tool").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_missing".to_string(),
                    call_id: Some("call_missing".to_string()),
                    name: "no_exist".to_string(),
                    arguments: serde_json::json!({}),
                },
            ]),
        ),
        vec![PhiMessage::user("run missing tool")],
    );
    let failed = step_agent_builder(session)
        .build()
        .unwrap()
        .run_single_step()
        .await
        .session;
    let events = Arc::new(Mutex::new(Vec::new()));

    default_step_agent_builder(failed)
        .with_module(CaptureCommitEventsModule {
            events: events.clone(),
        })
        .build()
        .unwrap()
        .run_single_step()
        .await;

    assert_eq!(*events.lock().unwrap(), vec!["model", "tool_result"]);
}

#[tokio::test]
async fn unknown_tool_recovery_drops_remaining_tool_queue_by_default() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("trying two tools").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_missing".to_string(),
                    call_id: Some("call_missing".to_string()),
                    name: "no_exist".to_string(),
                    arguments: serde_json::json!({ "name": "phi" }),
                },
                ToolCallRequest {
                    id: "call_2".to_string(),
                    call_id: Some("call_2".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
                },
            ]),
        ),
        vec![PhiMessage::user("force call two tools")],
    );

    let failed = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    let recovered = default_step_agent_builder(failed)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert_eq!(
        recovered.history()[0],
        PhiMessage::user("force call two tools")
    );
    assert!(
        matches!(&recovered.history()[1], PhiMessage::Assistant(assistant)
        if assistant.tool_calls.len() == 1
            && assistant.tool_calls[0].call_id.as_deref() == Some("call_missing"))
    );
    assert_eq!(recovered.history().len(), 3);
    assert!(matches!(
        recovered.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn multi_tool_recovery_keeps_prior_success_and_ignores_remaining_after_failure() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("trying three tools").with_tool_calls(vec![
                ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: "deterministic_success".to_string(),
                    arguments: serde_json::json!({}),
                },
                ToolCallRequest {
                    id: "call_missing".to_string(),
                    call_id: Some("call_missing".to_string()),
                    name: "no_exist".to_string(),
                    arguments: serde_json::json!({ "name": "phi" }),
                },
                ToolCallRequest {
                    id: "call_3".to_string(),
                    call_id: Some("call_3".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
                },
            ]),
        ),
        vec![PhiMessage::user("force call three tools")],
    );

    let after_first = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .with_module(RegisterDeterministicSuccessToolModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert!(matches!(
        after_first.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { assistant, pending_results, .. })
        if assistant.tool_calls.len() == 3
            && pending_results.len() == 1
            && assistant.tool_calls[1].call_id.as_deref() == Some("call_missing")
            && assistant.tool_calls[2].call_id.as_deref() == Some("call_3")
    ));

    let failed = step_agent_builder(after_first)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert!(matches!(
        failed.step(),
        PhiAgentStep::Failed(failed)
        if matches!(failed.error(), PhiAgentRuntimeError::ToolNotFound { .. })
            && failed.error().remaining_tool_requests().is_some_and(|requests| requests.len() == 1)
    ));
    assert_eq!(
        serde_json::to_value(&failed).unwrap()["frames"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "a tool failure must add exactly one failed frame"
    );
    let rolled_back = Session::rollback(failed.clone());
    assert_eq!(
        serde_json::to_value(&rolled_back).unwrap()["frames"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        rolled_back.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { pending_results, .. })
            if pending_results.len() == 1
    ));

    let recovered = default_step_agent_builder(failed)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    let history = recovered.history();
    assert_eq!(history[0], PhiMessage::user("force call three tools"));
    assert!(matches!(&history[1], PhiMessage::Assistant(assistant)
        if assistant.content.as_deref() == Some("trying three tools")
            && assistant.tool_calls.len() == 2
            && assistant.tool_calls[0].call_id.as_deref() == Some("call_1")
            && assistant.tool_calls[1].call_id.as_deref() == Some("call_missing")));
    assert!(matches!(
        &history[2],
        PhiMessage::ToolResult(crate::message::PhiToolResultMessage { id, result, .. })
            if id.as_deref() == Some("call_1") && result["status"] == serde_json::json!("exited")
    ));
    assert!(matches!(
        &history[3],
        PhiMessage::ToolResult(crate::message::PhiToolResultMessage { id, result, .. })
            if id.as_deref() == Some("call_missing")
                && result["kind"] == serde_json::json!("tool_not_found")
    ));
    assert_eq!(history.len(), 4);
    assert!(matches!(
        recovered.step(),
        PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, .. })
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn failed_tool_step_rolls_back_to_original_request() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            PhiAssistantMessage::text("running bash now").with_tool_calls(vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
            }]),
        ),
        vec![PhiMessage::user("hello")],
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
    assert!(matches!(failed.step(), PhiAgentStep::Failed(_)));

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
