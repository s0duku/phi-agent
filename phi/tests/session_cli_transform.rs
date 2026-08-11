use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const PHI: &str = env!("CARGO_BIN_EXE_phi");

#[test]
fn append_updates_a_file_and_preserves_cli_message_order() {
    let path = unique_session_path("append");
    std::fs::write(&path, root_session_json()).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "append",
            path.to_string_lossy().as_ref(),
            "--user",
            "one",
            "--assistant",
            "two",
            "--user",
            "three",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());

    let session = phi::session::Session::load(&path).unwrap();
    assert_eq!(
        session.history(),
        &[
            phi::message::PhiMessage::system("system"),
            phi::message::PhiMessage::user("one"),
            phi::message::PhiMessage::assistant("two"),
            phi::message::PhiMessage::user("three"),
        ]
    );
    assert!(matches!(
        session.step(),
        phi::session::PhiAgentStep::ReAct(phi::session::PhiReActStep::TurnEnd { .. })
    ));

    std::fs::remove_file(path).unwrap();
}

#[test]
fn append_and_rollback_compose_through_stdio() {
    let mut append = Command::new(PHI)
        .args(["session", "append", "--user", "piped"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    append
        .stdin
        .take()
        .unwrap()
        .write_all(branched_session_json().as_bytes())
        .unwrap();
    let appended = append.wait_with_output().unwrap();
    assert!(
        appended.status.success(),
        "{}",
        String::from_utf8_lossy(&appended.stderr)
    );

    let mut rollback = Command::new(PHI)
        .args(["session", "rollback"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    rollback
        .stdin
        .take()
        .unwrap()
        .write_all(&appended.stdout)
        .unwrap();
    let rolled_back = rollback.wait_with_output().unwrap();
    assert!(
        rolled_back.status.success(),
        "{}",
        String::from_utf8_lossy(&rolled_back.stderr)
    );

    let session = phi::session::Session::load_bytes(&rolled_back.stdout).unwrap();
    assert_eq!(
        session.history(),
        &[phi::message::PhiMessage::system("system")]
    );
    assert!(matches!(
        session.step(),
        phi::session::PhiAgentStep::ReAct(phi::session::PhiReActStep::TurnEnd { detail })
            if detail == "root"
    ));
}

#[test]
fn peek_reports_the_current_session_state_as_json() {
    let path = unique_session_path("peek");
    std::fs::write(&path, root_session_json()).unwrap();

    let output = Command::new(PHI)
        .args(["session", "peek", path.to_string_lossy().as_ref()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["step"]["kind"], "turn_end");
    assert_eq!(report["step"]["detail"], "root");
    assert_eq!(report["step"]["is_terminal"], true);
    assert_eq!(report["history_messages"], 1);
    assert!(report["modules"].is_array());

    std::fs::remove_file(path).unwrap();
}

#[test]
fn history_defaults_to_json_and_view_keeps_transcript_output() {
    let path = unique_session_path("history");
    std::fs::write(&path, root_session_json()).unwrap();

    let json_output = Command::new(PHI)
        .args(["session", "history", path.to_string_lossy().as_ref()])
        .output()
        .unwrap();
    assert!(json_output.status.success());
    let history: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(
        history,
        serde_json::json!([{ "role": "system", "content": "system" }])
    );

    let view_output = Command::new(PHI)
        .args([
            "session",
            "history",
            "--view",
            path.to_string_lossy().as_ref(),
        ])
        .output()
        .unwrap();
    assert!(view_output.status.success());
    let view = String::from_utf8(view_output.stdout).unwrap();
    assert!(view.contains("system"));

    std::fs::remove_file(path).unwrap();
}

#[test]
fn next_provider_adds_an_empty_outer_frame() {
    let path = unique_session_path("next-provider");
    std::fs::write(&path, root_session_json()).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "next",
            path.to_string_lossy().as_ref(),
            "--provider",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let frames = json["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["step"]["kind"], "turn_end");
    assert_eq!(frames[0]["delta"]["history"].as_array().unwrap().len(), 1);
    assert_eq!(frames[1]["step"]["kind"], "request_provider");
    assert!(frames[1].get("delta").is_none());

    std::fs::remove_file(path).unwrap();
}

#[test]
fn replace_provider_preserves_the_outer_delta() {
    let path = unique_session_path("replace-provider");
    std::fs::write(&path, root_session_json()).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "replace",
            path.to_string_lossy().as_ref(),
            "--provider",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let frames = json["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["step"]["kind"], "request_provider");
    assert_eq!(frames[0]["delta"]["history"].as_array().unwrap().len(), 1);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn tool_result_json_resolves_the_current_executor_step() {
    let path = unique_session_path("tool-result-json");
    std::fs::write(&path, request_executor_session_json(false)).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "tool-result",
            path.to_string_lossy().as_ref(),
            "--json",
            r#"{"value":42}"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let session = phi::session::Session::load(&path).unwrap();
    assert!(matches!(
        session.step(),
        phi::session::PhiAgentStep::ReAct(phi::session::PhiReActStep::RequestProvider { .. })
    ));
    assert_eq!(
        session.history()[1],
        phi::message::PhiMessage::assistant("pending")
    );
    assert!(matches!(
        &session.history()[2],
        phi::message::PhiMessage::Tool(phi::message::PhiToolMessage::ToolCall { id, name, .. })
            if id.as_deref() == Some("call-1") && name == "lookup"
    ));
    assert!(matches!(
        &session.history()[3],
        phi::message::PhiMessage::Tool(phi::message::PhiToolMessage::ToolResult { id, name, result })
            if id.as_deref() == Some("call-1")
                && name.as_deref() == Some("lookup")
                && result == &serde_json::json!({"value": 42})
    ));

    std::fs::remove_file(path).unwrap();
}

#[test]
fn tool_result_text_consumes_only_the_first_of_multiple_calls() {
    let path = unique_session_path("tool-result-text");
    std::fs::write(&path, request_executor_session_json(true)).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "tool-result",
            path.to_string_lossy().as_ref(),
            "--text",
            "done",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let session = phi::session::Session::load(&path).unwrap();
    assert!(matches!(
        session.step(),
        phi::session::PhiAgentStep::ReAct(phi::session::PhiReActStep::RequestExecutor {
            pending_messages,
            tool_calls,
            ..
        }) if pending_messages.is_empty()
            && tool_calls.len() == 1
            && tool_calls[0].id == "call-2"
    ));
    assert!(matches!(
        session.history().iter().last(),
        Some(phi::message::PhiMessage::Tool(phi::message::PhiToolMessage::ToolResult { result, .. }))
            if result == &serde_json::Value::String("done".to_string())
    ));

    std::fs::remove_file(path).unwrap();
}

#[test]
fn tool_result_json_file_resolves_the_current_executor_step() {
    let path = unique_session_path("tool-result-json-file");
    let result_path = unique_session_path("tool-result-json-input");
    std::fs::write(&path, request_executor_session_json(false)).unwrap();
    std::fs::write(&result_path, br#"{"value":42,"items":[1,2,3]}"#).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "tool-result",
            path.to_string_lossy().as_ref(),
            "--json-file",
            result_path.to_string_lossy().as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let session = phi::session::Session::load(&path).unwrap();
    assert!(matches!(
        session.history().iter().last(),
        Some(phi::message::PhiMessage::Tool(phi::message::PhiToolMessage::ToolResult { result, .. }))
            if result == &serde_json::json!({"value": 42, "items": [1, 2, 3]})
    ));

    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(result_path).unwrap();
}

#[test]
fn tool_result_text_file_preserves_the_complete_text() {
    let path = unique_session_path("tool-result-text-file");
    let result_path = unique_session_path("tool-result-text-input");
    std::fs::write(&path, request_executor_session_json(false)).unwrap();
    std::fs::write(&result_path, "first line\nsecond line\n").unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "tool-result",
            path.to_string_lossy().as_ref(),
            "--text-file",
            result_path.to_string_lossy().as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let session = phi::session::Session::load(&path).unwrap();
    assert!(matches!(
        session.history().iter().last(),
        Some(phi::message::PhiMessage::Tool(phi::message::PhiToolMessage::ToolResult { result, .. }))
            if result == &serde_json::Value::String("first line\nsecond line\n".to_string())
    ));

    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(result_path).unwrap();
}

#[test]
fn tool_result_rejects_invalid_state_and_json_without_modifying_the_file() {
    let path = unique_session_path("tool-result-invalid");
    let original = root_session_json().as_bytes().to_vec();
    std::fs::write(&path, &original).unwrap();

    let wrong_step = Command::new(PHI)
        .args([
            "session",
            "tool-result",
            path.to_string_lossy().as_ref(),
            "--text",
            "done",
        ])
        .output()
        .unwrap();
    assert!(!wrong_step.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let invalid_json = Command::new(PHI)
        .args([
            "session",
            "tool-result",
            path.to_string_lossy().as_ref(),
            "--json",
            "not-json",
        ])
        .output()
        .unwrap();
    assert!(!invalid_json.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let pending = request_executor_session_json(false).into_bytes();
    let invalid_json_path = unique_session_path("invalid-tool-result-json");
    std::fs::write(&path, &pending).unwrap();
    std::fs::write(&invalid_json_path, "not-json").unwrap();
    let invalid_json_file = Command::new(PHI)
        .args([
            "session",
            "tool-result",
            path.to_string_lossy().as_ref(),
            "--json-file",
            invalid_json_path.to_string_lossy().as_ref(),
        ])
        .output()
        .unwrap();
    assert!(!invalid_json_file.status.success());
    assert!(String::from_utf8_lossy(&invalid_json_file.stderr).contains("invalid JSON"));
    assert_eq!(std::fs::read(&path).unwrap(), pending);

    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(invalid_json_path).unwrap();
}

fn root_session_json() -> &'static str {
    r#"{
        "frames": [{
            "step": {"kind": "turn_end", "detail": "root"},
            "delta": {"history": [{"role": "system", "content": "system"}]}
        }]
    }"#
}

fn branched_session_json() -> &'static str {
    r#"{
        "frames": [
            {
                "step": {"kind": "turn_end", "detail": "root"},
                "delta": {"history": [{"role": "system", "content": "system"}]}
            },
            {
                "step": {"kind": "turn_end", "detail": "outer"},
                "delta": {"history": [{"role": "assistant", "content": "outer"}]}
            }
        ]
    }"#
}

fn request_executor_session_json(multiple: bool) -> String {
    let second = if multiple {
        r#", {
            "id": "call-2",
            "call_id": "call-2",
            "name": "lookup",
            "arguments": {"query": "second"}
        }"#
    } else {
        ""
    };
    format!(
        r#"{{
            "frames": [{{
                "step": {{
                    "kind": "request_executor",
                    "detail": "tool execution is pending",
                    "pending_messages": [{{"role": "assistant", "content": "pending"}}],
                    "tool_calls": [{{
                        "id": "call-1",
                        "call_id": "call-1",
                        "name": "lookup",
                        "arguments": {{"query": "first"}}
                    }}{second}]
                }},
                "delta": {{"history": [{{"role": "system", "content": "system"}}]}}
            }}]
        }}"#
    )
}

fn unique_session_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "phi-session-{label}-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}
