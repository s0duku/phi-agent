use std::fs;

use crate::{SessionInput, read_session_input, tests::support::unique_test_home};

#[test]
fn existing_file_backed_session_reports_resume_notice() {
    let root = unique_test_home();
    fs::create_dir_all(&root).expect("test home directory should be created");
    let session_path = root.join("session.json");
    fs::write(&session_path, b"").expect("empty session file should be written");

    let session_input =
        read_session_input(Some(&session_path)).expect("existing empty session file should load");

    assert_eq!(
        session_input.existing_session_path(),
        Some(session_path.as_path())
    );
}

#[test]
fn missing_file_backed_session_is_not_treated_as_a_new_session() {
    let root = unique_test_home();
    fs::create_dir_all(&root).expect("test home directory should be created");
    let session_path = root.join("missing-session.json");

    let session_input = read_session_input(Some(&session_path))
        .expect("missing session path should be represented");

    assert_eq!(session_input.existing_session_path(), None);
    assert!(matches!(
        session_input,
        SessionInput::MissingFile { path, .. } if path == session_path
    ));
}

#[test]
fn no_session_only_starts_when_cli_messages_are_present() {
    assert!(SessionInput::NoInput.session_for_agent().is_none());
}

#[test]
fn missing_file_never_starts_a_new_session() {
    let input = SessionInput::MissingFile {
        path: "missing.json".into(),
        stdin_user_message: Some("hello".to_owned()),
    };

    assert!(input.session_for_agent().is_none());
}
