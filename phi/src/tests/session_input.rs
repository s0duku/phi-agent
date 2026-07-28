use std::fs;

use crate::{read_session_input, tests::support::unique_test_home};

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
fn missing_file_backed_session_does_not_report_resume_notice() {
    let root = unique_test_home();
    fs::create_dir_all(&root).expect("test home directory should be created");
    let session_path = root.join("missing-session.json");

    let session_input =
        read_session_input(Some(&session_path)).expect("missing session path should start empty");

    assert_eq!(session_input.existing_session_path(), None);
}
