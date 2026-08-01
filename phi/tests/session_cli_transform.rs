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
