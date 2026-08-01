use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const PHI: &str = env!("CARGO_BIN_EXE_phi");

#[test]
fn agent_commands_show_help_when_stdin_is_empty() {
    for name in ["step", "run", "yolo"] {
        let output = Command::new(PHI)
            .arg(name)
            .stdin(Stdio::null())
            .output()
            .expect("phi agent command should execute");

        assert!(
            output.status.success(),
            "phi {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("Usage: phi {name} ")), "{stdout}");
        assert!(stdout.contains("--user <TEXT>"), "{stdout}");
    }
}

#[test]
fn missing_session_path_shows_help_without_creating_a_file() {
    for name in ["step", "run", "yolo"] {
        let session_path = unique_session_path(&format!("missing-{name}"));
        let output = Command::new(PHI)
            .args([
                name,
                session_path.to_string_lossy().as_ref(),
                "--user",
                "hello",
            ])
            .stdin(Stdio::null())
            .output()
            .expect("phi agent command should execute");

        assert!(
            output.status.success(),
            "phi {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("Usage: phi {name} ")));
        assert!(!session_path.exists());
    }
}

#[test]
fn session_new_explicitly_creates_without_overwriting() {
    let session_path = unique_session_path("new");
    let path = session_path.to_string_lossy();
    let created = Command::new(PHI)
        .args(["session", "new", path.as_ref()])
        .output()
        .expect("phi session new should execute");

    assert!(
        created.status.success(),
        "phi session new failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let session = phi::session::Session::load(&session_path)
        .expect("new command should create a valid session");
    assert!(matches!(
        session.history().iter().next(),
        Some(phi::message::PhiMessage::System(_))
    ));

    let duplicate = Command::new(PHI)
        .args(["session", "new", path.as_ref()])
        .output()
        .expect("duplicate phi session new should execute");
    assert!(!duplicate.status.success());
    assert!(phi::session::Session::load(&session_path).is_ok());

    std::fs::remove_file(session_path).expect("test session should be removable");
}

#[test]
fn session_new_commits_system_prompt_from_selected_home() {
    let root = unique_session_path("home").with_extension("home");
    std::fs::create_dir_all(&root).expect("test home should be created");
    std::fs::write(
        root.join("config.toml"),
        "PHI_SYSTEM = \"System prompt from explicit home.\"\nPHI_MODEL = \"session-home-model\"\n",
    )
    .expect("test home config should be written");
    let session_path = unique_session_path("system");

    let output = Command::new(PHI)
        .args([
            "session",
            "new",
            session_path.to_string_lossy().as_ref(),
            "--home",
            root.to_string_lossy().as_ref(),
        ])
        .env_remove("PHI_SYSTEM")
        .env_remove("PHI_MODEL")
        .output()
        .expect("phi session new should execute");
    assert!(
        output.status.success(),
        "phi session new failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let session = phi::session::Session::load(&session_path)
        .expect("new command should create a valid session");
    assert_eq!(
        session.history(),
        &[phi::message::PhiMessage::system(
            "System prompt from explicit home."
        )]
    );
    assert!(matches!(
        session.step(),
        phi::session::PhiAgentStep::ReAct(phi::session::PhiReActStep::RequestProvider { call, .. })
            if call.model == "session-home-model"
    ));

    std::fs::remove_file(session_path).expect("test session should be removable");
    std::fs::remove_dir_all(root).expect("test home should be removable");
}

#[test]
fn cli_messages_append_to_the_outer_session_delta_before_step_evaluation() {
    let session_path = unique_session_path("append-message");
    std::fs::write(
        &session_path,
        r#"{
            "frames": [
                {
                    "step": {"kind": "turn_end", "detail": "done"},
                    "delta": {
                        "history": [{"role": "assistant", "content": "previous"}]
                    }
                }
            ]
        }"#,
    )
    .expect("test session should be written");

    let output = Command::new(PHI)
        .args([
            "step",
            session_path.to_string_lossy().as_ref(),
            "--user",
            "next",
            "--quiet",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("phi step should execute");
    assert!(
        output.status.success(),
        "phi step failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&session_path).expect("updated session should be readable"),
    )
    .expect("updated session should be JSON");
    let frames = json["frames"]
        .as_array()
        .expect("session should have frames");
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["step"]["kind"], "turn_end");
    assert_eq!(frames[0]["delta"]["history"][0]["content"], "previous");
    assert_eq!(frames[0]["delta"]["history"][1]["role"], "user");
    assert_eq!(frames[0]["delta"]["history"][1]["content"], "next");
    assert_eq!(frames[1]["step"]["kind"], "request_provider");
    assert!(frames[1].get("delta").is_none());

    std::fs::remove_file(session_path).expect("test session should be removable");
}

fn unique_session_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "phi-{label}-session-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}
