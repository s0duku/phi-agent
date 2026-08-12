use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const PHI: &str = env!("CARGO_BIN_EXE_phi");

#[test]
fn agent_commands_require_an_explicit_session_target() {
    for name in ["step", "run", "yolo"] {
        let output = Command::new(PHI)
            .arg(name)
            .stdin(Stdio::null())
            .output()
            .expect("phi agent command should execute");

        assert!(
            !output.status.success(),
            "phi {name} unexpectedly succeeded"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("required arguments were not provided"));
        assert!(stderr.contains("<SESSION>"));
    }
}

#[test]
fn missing_session_path_fails_without_creating_a_file() {
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
            !output.status.success(),
            "phi {name} unexpectedly succeeded"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("failed to read session file"));
        assert!(!session_path.exists());
    }
}

#[test]
fn session_new_and_append_compose_through_explicit_stdio() {
    let new_session = Command::new(PHI)
        .args(["session", "new", "-"])
        .env_remove("PHI_MODEL")
        .env_remove("PHI_PROVIDER")
        .env_remove("PHI_API")
        .env_remove("PHI_KEY")
        .output()
        .expect("phi session new - should execute");
    assert!(
        new_session.status.success(),
        "{}",
        String::from_utf8_lossy(&new_session.stderr)
    );

    let mut append = Command::new(PHI)
        .args(["session", "append", "-", "--user", "hello"])
        .env_remove("PHI_MODEL")
        .env_remove("PHI_PROVIDER")
        .env_remove("PHI_API")
        .env_remove("PHI_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("phi session append - should execute");
    use std::io::Write;
    append
        .stdin
        .take()
        .unwrap()
        .write_all(&new_session.stdout)
        .unwrap();
    let output = append.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let session = phi::session::Session::load_bytes(&output.stdout).unwrap();
    assert!(session.history().iter().any(|message| matches!(
        message,
        phi::message::PhiMessage::User(phi::message::PhiUserMessage::Text(text))
            if text == "hello"
    )));
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
        root.join("config.yml"),
        "model:\n  name: session-home-model\nruntime:\n  system: System prompt from explicit home.\n",
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
fn explicit_config_replaces_home_config_for_session_creation() {
    let root = unique_session_path("config-home").with_extension("home");
    std::fs::create_dir_all(&root).expect("test home should be created");
    std::fs::write(
        root.join("config.yml"),
        "model:\n  name: home-model\nruntime:\n  system: Home system.\n",
    )
    .expect("home config should be written");
    let config_path = unique_session_path("explicit-config").with_extension("yml");
    std::fs::write(
        &config_path,
        "model:\n  name: explicit-model\nruntime:\n  system: Explicit system.\n",
    )
    .expect("explicit config should be written");
    let session_path = unique_session_path("explicit-config-session");

    let output = Command::new(PHI)
        .args([
            "session",
            "new",
            session_path.to_string_lossy().as_ref(),
            "--home",
            root.to_string_lossy().as_ref(),
            "--config",
            config_path.to_string_lossy().as_ref(),
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

    let session = phi::session::Session::load(&session_path).expect("session should load");
    assert_eq!(
        session.history(),
        &[phi::message::PhiMessage::system("Explicit system.")]
    );
    assert!(matches!(
        session.step(),
        phi::session::PhiAgentStep::ReAct(phi::session::PhiReActStep::RequestProvider { call, .. })
            if call.model == "explicit-model"
    ));

    std::fs::remove_file(session_path).expect("test session should be removable");
    std::fs::remove_file(config_path).expect("test config should be removable");
    std::fs::remove_dir_all(root).expect("test home should be removable");
}

#[test]
fn environment_overrides_explicit_config() {
    let root = unique_session_path("env-config-home").with_extension("home");
    std::fs::create_dir_all(&root).expect("test home should be created");
    let config_path = unique_session_path("env-explicit-config").with_extension("yml");
    std::fs::write(
        &config_path,
        "model:\n  name: explicit-model\nruntime:\n  system: Explicit system.\n",
    )
    .expect("explicit config should be written");
    let session_path = unique_session_path("env-config-session");

    let output = Command::new(PHI)
        .args([
            "session",
            "new",
            session_path.to_string_lossy().as_ref(),
            "--home",
            root.to_string_lossy().as_ref(),
            "--config",
            config_path.to_string_lossy().as_ref(),
        ])
        .env("PHI_MODEL", "environment-model")
        .env("PHI_SYSTEM", "Environment system.")
        .output()
        .expect("phi session new should execute");
    assert!(output.status.success());

    let session = phi::session::Session::load(&session_path).expect("session should load");
    assert_eq!(
        session.history(),
        &[phi::message::PhiMessage::system("Environment system.")]
    );
    assert!(matches!(
        session.step(),
        phi::session::PhiAgentStep::ReAct(phi::session::PhiReActStep::RequestProvider { call, .. })
            if call.model == "environment-model"
    ));

    std::fs::remove_file(session_path).expect("test session should be removable");
    std::fs::remove_file(config_path).expect("test config should be removable");
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
                        "history": [{"role": "assistant", "content": {"content": "previous"}}]
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
    assert_eq!(
        frames[0]["delta"]["history"][0]["content"]["content"],
        "previous"
    );
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
