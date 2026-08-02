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
fn append_tool_result_infers_the_latest_tool_call_metadata() {
    let path = unique_session_path("tool-result");
    std::fs::write(&path, tool_call_session_json()).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "append",
            path.to_string_lossy().as_ref(),
            "--tool-result",
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
        session.history().iter().last(),
        Some(phi::message::PhiMessage::Tool(
            phi::message::PhiToolMessage::ToolResult { id, name, result }
        )) if id.as_deref() == Some("call-1")
            && name.as_deref() == Some("lookup")
            && result == &serde_json::json!({"value": 42})
    ));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn append_tool_result_rejects_missing_call_without_modifying_file() {
    let path = unique_session_path("tool-result-missing");
    let original = root_session_json().as_bytes().to_vec();
    std::fs::write(&path, &original).unwrap();

    let output = Command::new(PHI)
        .args([
            "session",
            "append",
            path.to_string_lossy().as_ref(),
            "--tool-result",
            "done",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_file(path).unwrap();
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

fn tool_call_session_json() -> &'static str {
    r#"{
        "frames": [{
            "step": {"kind": "turn_end", "detail": "root"},
            "delta": {
                "history": [{
                    "role": "tool",
                    "content": {
                        "ToolCall": {
                            "id": "call-1",
                            "name": "lookup",
                            "arguments": {"query": "phi"}
                        }
                    }
                }]
            }
        }]
    }"#
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
