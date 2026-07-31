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
