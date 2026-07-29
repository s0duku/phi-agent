use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::{
    error::{PhiErrorKind, PhiRuntimeError, PhiRuntimeResult},
    executor::{PhiTool, ToolCallRequest, ToolCallResponse},
    home::LocalPhiHome,
    message::{
        PhiAssistantMessage, PhiHistory, PhiMessage, PhiReasoningContent, PhiToolMessage,
        PhiUserMessage,
    },
    module::{PhiAgentCommitEvent, PhiAgentStepEvent, PhiModule},
    render::{PhiModelResponse, PhiModelTurnState, PhiProviderCall, TestClient},
    session::{PhiAgentStep, Session},
};

use super::support::{
    default_step_agent_builder, shell_echo_ok_command, shell_echo_rewritten_command,
    shell_stdout_ok, shell_tool_name, test_model_defaults,
};
use super::support::{env_lock, step_agent_builder, stub_client};

struct RewriteToolCallModule;
struct RejectAfterModelResponseModule;
struct RejectAfterToolCallModule;
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
struct RewriteArgumentsInsideTool;
struct RegisterRewriteArgumentsToolModule;
struct CaptureRequestToolsProvider {
    request: Arc<Mutex<Option<PhiProviderCall>>>,
    response: Vec<PhiMessage>,
}
struct ModelResponseProvider {
    response: PhiModelResponse,
}

impl PhiModule for RewriteToolCallModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        if let PhiAgentStepEvent::BeforeToolCall { request, .. } = event {
            request.arguments = serde_json::json!({ "cmd": shell_echo_rewritten_command() });
        }
        Ok(())
    }
}

impl PhiModule for RejectAfterModelResponseModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        if let PhiAgentStepEvent::AfterModelResponse { .. } = event {
            return Err(PhiRuntimeError::module("module rejected model response")
                .with_source_step("request_complete"));
        }
        Ok(())
    }
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

impl PhiModule for RewriteAllAssistantMessagesModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        let PhiAgentStepEvent::AfterModelResponse { message, .. } = event else {
            return Ok(());
        };

        match message {
            PhiMessage::Assistant(PhiAssistantMessage::Text(text)) => {
                *text = "rewritten text".to_string();
            }
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning { content, .. }) => {
                *content = vec![PhiReasoningContent::Summary(
                    "rewritten reasoning".to_string(),
                )];
            }
            _ => {}
        }

        Ok(())
    }
}

impl PhiModule for CaptureModelRequestModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
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
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
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
    type ProbInfo = ();

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

impl PhiModule for RegisterRewriteArgumentsToolModule {
    type ProbInfo = ();

    fn module_tools(
        &mut self,
        _context: &crate::agent::PhiAgentBuildContext,
    ) -> Vec<Arc<dyn PhiTool>> {
        vec![Arc::new(RewriteArgumentsInsideTool)]
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
    ) -> ToolCallResponse {
        request.arguments = serde_json::json!({ "value": "rewritten-inside-tool" });
        ToolCallResponse::success(request, self.name(), serde_json::json!({ "ok": true }))
    }
}

struct EmptyProvider;

#[async_trait]
impl TestClient for CaptureRequestToolsProvider {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiRuntimeResult<PhiModelResponse> {
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
    ) -> PhiRuntimeResult<PhiModelResponse> {
        Ok(PhiModelResponse::unspecified(Vec::new()))
    }
}

#[async_trait]
impl TestClient for ModelResponseProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiRuntimeResult<PhiModelResponse> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn request_complete_merges_custom_tools_from_config() {
    let _lock = env_lock();
    let root = unique_temp_dir("phi-request-complete-tools");
    std::fs::create_dir_all(&root).expect("temp home root should be creatable");
    std::fs::write(
        root.join("config.toml"),
        r#"PHI_TOOLS = '[{"name":"external_lookup","description":"External lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}}]'
"#,
    )
    .expect("config.toml should be writable");

    let captured_request = Arc::new(Mutex::new(None));
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );
    let outcome = crate::agent::PhiAgent::builder(
        session,
        crate::agent::PhiAgentCommand::Step(crate::agent::PhiAgentCommand::step()),
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
    assert!(
        request
            .tools
            .iter()
            .any(|tool| tool.name == "external_lookup"),
        "custom tool definition should be merged into the request"
    );
    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("hello"), PhiMessage::assistant("ok")]
    );

    std::fs::remove_dir_all(root).expect("temp home should be removable");
}

#[tokio::test]
async fn request_complete_commits_assistant_response_before_completion() {
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
        PhiAgentStep::Completed { detail }
        if detail == "model response committed; no tool execution is pending"
    ));
}

#[tokio::test]
async fn provider_continue_response_commits_assistant_and_requests_another_completion() {
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
        PhiAgentStep::RequestComplete { detail, .. }
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
async fn request_compact_leaves_unchanged_history_alone() {
    for history in [
        vec![
            PhiMessage::system("sys1"),
            PhiMessage::system("sys2"),
            PhiMessage::user("hello"),
            PhiMessage::assistant("world"),
        ],
        vec![PhiMessage::system("sys1"), PhiMessage::system("sys2")],
    ] {
        let session = Session::from_root(PhiAgentStep::request_compact(), history.clone());

        let outcome = step_agent_builder(session)
            .with_client(Arc::new(EmptyProvider))
            .build()
            .expect("agent should build")
            .run_single_step()
            .await;

        assert_eq!(outcome.session.history(), history);
        assert!(matches!(outcome.session.step(), PhiAgentStep::Compacted));
    }
}

#[tokio::test]
async fn request_compact_retains_recent_user_tail_and_compacts_everything_else() {
    let big = "x".repeat(100_000);
    let session = Session::from_root(
        PhiAgentStep::request_compact(),
        vec![
            PhiMessage::system("sys"),
            PhiMessage::user(big.clone()),
            PhiMessage::assistant("a1"),
            PhiMessage::tool_result(
                Some("call-1".into()),
                Some("bash".into()),
                serde_json::json!({"ok": true}),
            ),
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
    assert_eq!(history.len(), 5);
    assert_eq!(history[0], PhiMessage::system("sys"));
    assert!(matches!(
        &history[1],
        PhiMessage::User(PhiUserMessage::Text(text))
            if !text.is_empty() && text.len() < big.len()
    ));
    assert_eq!(history[2], PhiMessage::user("second"));
    assert_eq!(history[3], PhiMessage::user("third"));
    assert_eq!(history[4], PhiMessage::user("summary"));

    let expr = outcome.session.clone().into_expr();
    let compacted = &expr;
    assert!(matches!(compacted.step(), PhiAgentStep::Compacted));
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
            Err(crate::error::PhiRuntimeError::internal("compact exploded"))
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
        PhiAgentStep::Failed { error }
        if error.kind() == PhiErrorKind::RequestCompact
            && error.detail() == "compact exploded"
            && error.source_step() == Some("step_request_compact")
    ));
}

#[tokio::test]
async fn before_model_request_rewrites_explicit_request_payload() {
    let captured_temperature = Arc::new(Mutex::new(None));
    let captured_messages = Arc::new(Mutex::new(Vec::new()));

    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
        PhiAgentStep::Failed { error }
        if error.kind() == PhiErrorKind::Module
            && error.detail() == "module rejected model response"
    ));
}

#[tokio::test]
async fn after_model_response_rewrites_are_committed_for_all_assistant_messages() {
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                id: None,
                content: vec![PhiReasoningContent::Summary("original reasoning".into())],
            }),
            PhiMessage::assistant("original text"),
        ]))
        .with_module(RewriteAllAssistantMessagesModule)
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        outcome.session.history(),
        &[
            PhiMessage::user("hello"),
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                id: None,
                content: vec![PhiReasoningContent::Summary("rewritten reasoning".into())],
            }),
            PhiMessage::assistant("rewritten text"),
        ]
    );
}

#[tokio::test]
async fn failed_step_without_default_module_stays_failed() {
    let session = Session::from_root(
        PhiAgentStep::failed(PhiRuntimeError::internal("failed")),
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
        PhiAgentStep::Failed { error } if error.detail() == "failed"
    ));

    let resumed = default_step_agent_builder(outcome.session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert!(matches!(
        resumed.session.step(),
        PhiAgentStep::RequestComplete { detail, .. }
        if detail == "resuming from failed step"
    ));
}

#[tokio::test]
async fn tool_call_message_commits_rewritten_request_payload() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            Vec::new(),
            vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": "echo original" }),
            }],
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
            PhiMessage::Tool(PhiToolMessage::ToolCall { arguments, .. }) => Some(arguments),
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
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
        PhiAgentStep::RequestExecutor { pending_messages, tool_calls, .. }
        if pending_messages.is_empty()
            && tool_calls.len() == 1
            && tool_calls[0].name == shell_tool_name()
            && tool_calls[0].call_id.as_deref() == Some("call_1")
            && tool_calls[0].arguments == serde_json::json!({ "cmd": "ls" })
    ));
}

#[tokio::test]
async fn tool_internal_argument_rewrite_is_committed_into_history() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("running custom tool")],
            vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: "rewrite_args".to_string(),
                arguments: serde_json::json!({ "value": "original" }),
            }],
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
            PhiMessage::Tool(PhiToolMessage::ToolCall { arguments, .. }) => Some(arguments),
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
            && tool_calls[0].arguments == serde_json::json!({ "cmd": "ls" })
    ));
}

#[tokio::test]
async fn echo_events_follow_response_parse_and_tool_evaluation_boundaries() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
        vec![PhiMessage::user("list files")],
    );

    let first = step_agent_builder(session)
        .with_client(stub_client(vec![
            PhiMessage::assistant("running bash now"),
            PhiMessage::tool_call(
                Some("call_1".to_string()),
                shell_tool_name(),
                serde_json::json!({ "cmd": shell_echo_ok_command() }),
            ),
        ]))
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
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
        vec![PhiMessage::user("run two commands")],
    );

    let outcome = step_agent_builder(session)
        .with_client(stub_client(vec![
            PhiMessage::assistant("running two tools"),
            PhiMessage::tool_call(
                Some("call_1".to_string()),
                shell_tool_name(),
                serde_json::json!({ "cmd": "printf first" }),
            ),
            PhiMessage::tool_call(
                Some("call_2".to_string()),
                shell_tool_name(),
                serde_json::json!({ "cmd": "printf second" }),
            ),
        ]))
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
        PhiAgentStep::RequestExecutor { pending_messages, tool_calls, .. }
        if pending_messages.as_slice() == [PhiMessage::assistant("running two tools")].as_slice()
            && tool_calls.len() == 2
            && tool_calls[0].call_id.as_deref() == Some("call_1")
            && tool_calls[1].call_id.as_deref() == Some("call_2")
    ));
}

#[tokio::test]
async fn multiple_tool_calls_execute_sequentially_without_dropping_queue() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("running two tools")],
            vec![
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
            ],
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

    assert_eq!(first.history()[0], PhiMessage::user("hello"));
    assert_eq!(
        first.history()[1],
        PhiMessage::assistant("running two tools")
    );
    assert!(matches!(
        &first.history()[2],
        PhiMessage::Tool(PhiToolMessage::ToolCall { id, .. }) if id.as_deref() == Some("call_1")
    ));
    assert!(matches!(
        first.step(),
        PhiAgentStep::RequestExecutor { pending_messages, tool_calls, detail }
        if detail == "additional tool execution is pending"
            && pending_messages.is_empty()
            && tool_calls.len() == 1
            && tool_calls[0].call_id.as_deref() == Some("call_2")
    ));

    let second = step_agent_builder(first)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert!(matches!(
        &second.history()[4],
        PhiMessage::Tool(PhiToolMessage::ToolCall { id, .. }) if id.as_deref() == Some("call_2")
    ));
    assert!(matches!(
        second.step(),
        PhiAgentStep::RequestComplete { detail, .. }
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn after_tool_call_rejection_does_not_commit_half_finished_tool_history() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("running bash now")],
            vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
            }],
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
        PhiAgentStep::Failed { error }
        if error.kind() == PhiErrorKind::Module
            && error.detail() == "module rejected tool result"
    ));
}

#[tokio::test]
async fn tool_step_commits_pending_assistant_then_tool_call_then_result() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("running bash now")],
            vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
            }],
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
}

#[tokio::test]
async fn unknown_tool_fails_without_recovery_module() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("trying custom tool")],
            vec![ToolCallRequest {
                id: "call_missing".to_string(),
                call_id: Some("call_missing".to_string()),
                name: "no_exist".to_string(),
                arguments: serde_json::json!({ "name": "phi" }),
            }],
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
        PhiAgentStep::Failed { error }
        if error.kind() == PhiErrorKind::ToolNotFound
            && error.tool_request().is_some_and(|request| request.name == "no_exist")
            && error.pending_messages().is_some_and(|messages| messages == [PhiMessage::assistant("trying custom tool")].as_slice())
    ));
}

#[tokio::test]
async fn unknown_tool_recovery_commits_failure_result_and_resumes_model_flow() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("trying custom tool")],
            vec![ToolCallRequest {
                id: "call_missing".to_string(),
                call_id: Some("call_missing".to_string()),
                name: "no_exist".to_string(),
                arguments: serde_json::json!({ "name": "phi" }),
            }],
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
    assert_eq!(history[1], PhiMessage::assistant("trying custom tool"));
    assert_eq!(
        history[2],
        PhiMessage::tool_call(
            Some("call_missing".to_string()),
            "no_exist",
            serde_json::json!({ "name": "phi" }),
        )
    );
    let PhiMessage::Tool(PhiToolMessage::ToolResult { id, name, result }) = &history[3] else {
        panic!("fourth history entry should be a tool result");
    };
    assert_eq!(id.as_deref(), Some("call_missing"));
    assert_eq!(name.as_deref(), Some("no_exist"));
    assert_eq!(result["ok"], serde_json::json!(false));
    assert_eq!(
        result["error"],
        serde_json::json!("assistant requested unknown tool: no_exist")
    );
    assert_eq!(result["value"]["kind"], serde_json::json!("tool_not_found"));
    assert_eq!(result["value"]["tool_name"], serde_json::json!("no_exist"));
    let warnings = warnings
        .lock()
        .expect("warning mutex should not be poisoned")
        .clone();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("structured tool_not_found result for no_exist"));
    assert!(warnings[0].contains("\"ok\":false"));
    assert!(warnings[0].contains("\"kind\":\"tool_not_found\""));
    assert!(warnings[0].contains("\"tool_name\":\"no_exist\""));
    assert!(matches!(
        outcome.session.step(),
        PhiAgentStep::RequestComplete { detail, .. }
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn unknown_tool_recovery_drops_remaining_tool_queue_by_default() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("trying two tools")],
            vec![
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
            ],
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
    assert_eq!(
        recovered.history()[1],
        PhiMessage::assistant("trying two tools")
    );
    assert!(matches!(
        &recovered.history()[2],
        PhiMessage::Tool(PhiToolMessage::ToolCall { id, .. }) if id.as_deref() == Some("call_missing")
    ));
    assert_eq!(recovered.history().len(), 4);
    assert!(matches!(
        recovered.step(),
        PhiAgentStep::RequestComplete { detail, .. }
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn multi_tool_recovery_keeps_prior_success_and_ignores_remaining_after_failure() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("trying three tools")],
            vec![
                ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: shell_tool_name().to_string(),
                    arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
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
            ],
        ),
        vec![PhiMessage::user("force call three tools")],
    );

    let after_first = step_agent_builder(session)
        .with_client(Arc::new(EmptyProvider))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await
        .session;

    assert!(matches!(
        after_first.step(),
        PhiAgentStep::RequestExecutor { tool_calls, .. }
        if tool_calls.len() == 2
            && tool_calls[0].call_id.as_deref() == Some("call_missing")
            && tool_calls[1].call_id.as_deref() == Some("call_3")
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
        PhiAgentStep::Failed { error }
        if error.kind() == PhiErrorKind::ToolNotFound
            && error.remaining_tool_requests().is_some_and(|requests| requests.len() == 1)
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
    assert_eq!(history[1], PhiMessage::assistant("trying three tools"));
    assert!(matches!(
        &history[2],
        PhiMessage::Tool(PhiToolMessage::ToolCall { id, .. }) if id.as_deref() == Some("call_1")
    ));
    assert!(matches!(
        &history[3],
        PhiMessage::Tool(PhiToolMessage::ToolResult { id, result, .. })
            if id.as_deref() == Some("call_1") && result["ok"] == serde_json::json!(true)
    ));
    assert!(matches!(
        &history[4],
        PhiMessage::Tool(PhiToolMessage::ToolCall { id, .. }) if id.as_deref() == Some("call_missing")
    ));
    assert!(matches!(
        &history[5],
        PhiMessage::Tool(PhiToolMessage::ToolResult { id, result, .. })
            if id.as_deref() == Some("call_missing") && result["ok"] == serde_json::json!(false)
    ));
    assert_eq!(history.len(), 6);
    assert!(matches!(
        recovered.step(),
        PhiAgentStep::RequestComplete { detail, .. }
        if detail == "tool result committed; model response is pending"
    ));
}

#[tokio::test]
async fn failed_tool_step_resumes_from_clean_history_on_next_step() {
    let session = Session::from_root(
        PhiAgentStep::request_executor(
            "tool execution is pending",
            vec![PhiMessage::assistant("running bash now")],
            vec![ToolCallRequest {
                id: "call_1".to_string(),
                call_id: Some("call_1".to_string()),
                name: shell_tool_name().to_string(),
                arguments: serde_json::json!({ "cmd": shell_echo_ok_command() }),
            }],
        ),
        vec![PhiMessage::user("hello")],
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
    assert!(matches!(failed.step(), PhiAgentStep::Failed { .. }));

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
