use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;
use serde_json::ser::{Formatter, Serializer};

use super::Session;

pub fn load(path: impl AsRef<Path>) -> Result<Session, Box<dyn std::error::Error>> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|error| format!("failed to read session file {}: {error}", path.display()))?;
    load_bytes(&bytes)
        .map_err(|error| format!("failed to parse session file {}: {error}", path.display()).into())
}

pub fn load_bytes(input: &[u8]) -> Result<Session, Box<dyn std::error::Error>> {
    let decoded = std::str::from_utf8(input)?;
    if decoded.trim().is_empty() {
        return Err("session input is empty; create one explicitly with `phi session new`".into());
    }
    let session: Session =
        serde_json::from_str(decoded).map_err(|error| format!("invalid session JSON: {error}"))?;
    session
        .validate()
        .map_err(|error| format!("invalid session JSON: {}", error.detail()))?;
    Ok(session)
}

pub fn write_json<W>(session: &Session, writer: &mut W) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write,
{
    serde_json::to_writer_pretty(&mut *writer, session)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn write_stdout(session: &Session) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::stdout().lock();
    write_ascii_json(&mut stdout, session)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

pub(super) fn save_atomic(
    session: &Session,
    path: &Path,
    parent: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    let file_name = path
        .file_name()
        .ok_or_else(|| format!("session path has no file name: {}", path.display()))?;
    let (temp_path, mut file) = loop {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp_name = format!(
            ".{}.{}.{}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
            id
        );
        let temp_path = parent.join(temp_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => break (temp_path, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to create temporary session file for {}: {error}",
                    path.display()
                )
                .into());
            }
        }
    };

    let write_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        write_json(session, &mut file)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temp_path, path)?;
        sync_parent(parent)?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("failed to save session file {}: {error}", path.display()).into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

fn write_ascii_json<W, T>(writer: &mut W, value: &T) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write,
    T: Serialize,
{
    let mut serializer = Serializer::with_formatter(&mut *writer, EnsureAsciiFormatter);
    value.serialize(&mut serializer)?;
    Ok(())
}

#[derive(Default)]
struct EnsureAsciiFormatter;

impl Formatter for EnsureAsciiFormatter {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        for ch in fragment.chars() {
            if ch.is_ascii() {
                writer.write_all(ch.encode_utf8(&mut [0; 4]).as_bytes())?;
            } else {
                write_unicode_escape(writer, ch)?;
            }
        }
        Ok(())
    }
}

fn write_unicode_escape<W>(writer: &mut W, ch: char) -> io::Result<()>
where
    W: ?Sized + Write,
{
    let value = ch as u32;
    if value <= 0xFFFF {
        return write!(writer, "\\u{value:04x}");
    }

    let value = value - 0x1_0000;
    let high = 0xD800 + ((value >> 10) & 0x3FF);
    let low = 0xDC00 + (value & 0x3FF);
    write!(writer, "\\u{high:04x}\\u{low:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PhiAgentRuntimeError;
    use crate::expr::{PhiStepExpr, PhiVariable};
    use crate::message::{PhiHistory, PhiMessage};
    use crate::session::{PhiAgentStep, PhiReActStep, SessionStepKind};
    use crate::tests::support::test_model_defaults;

    const COUNT: PhiVariable<i32> = PhiVariable::new("count");
    const CLEARED: PhiVariable<bool> = PhiVariable::new("cleared");
    const ATTEMPT: PhiVariable<i32> = PhiVariable::new("attempt");

    #[test]
    fn round_trips_session_json() {
        let session = Session::from_root(
            PhiAgentStep::failed(PhiAgentRuntimeError::provider_request(
                "provider request failed",
            )),
            vec![PhiMessage::user("inspect the code")],
        );
        let mut output = Vec::new();

        write_json(&session, &mut output).unwrap();
        let loaded = load_bytes(&output).unwrap();

        assert_eq!(loaded.step(), session.step());
        assert_eq!(loaded.history(), session.history());
    }

    #[test]
    fn saves_and_loads_session_file() {
        let path = std::env::temp_dir().join(format!(
            "phi-session-save-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session = Session::from_root(
            PhiAgentStep::turn_end("done"),
            vec![PhiMessage::user("hello from phi")],
        );

        session.save(&path).unwrap();
        let loaded = Session::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(loaded.history(), session.history());
    }

    #[test]
    fn save_atomically_replaces_a_valid_session_without_temp_files() {
        let root = std::env::temp_dir().join(format!(
            "phi-session-atomic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("work.session");
        let initial = Session::from_root(
            PhiAgentStep::request_provider("initial", &test_model_defaults()),
            vec![PhiMessage::user("initial")],
        );
        let updated = Session::from_root(
            PhiAgentStep::request_provider("updated", &test_model_defaults()),
            vec![PhiMessage::user("updated")],
        );

        initial.save(&path).unwrap();
        updated.save(&path).unwrap();

        let restored = Session::load(&path).unwrap();
        assert_eq!(restored.history(), updated.history());
        assert_eq!(restored.step(), updated.step());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_bytes_rejects_empty_input() {
        let error = load_bytes(b"   \n\t  ").unwrap_err();
        assert!(error.to_string().contains("session input is empty"));
    }

    #[test]
    fn load_bytes_requires_utf8_json_without_encoding_markers() {
        assert!(load_bytes(&[0xFF, 0xFE, b'{', 0]).is_err());
        assert!(load_bytes(b"\xEF\xBB\xBF{}").is_err());
    }

    #[test]
    fn rejects_empty_session_file() {
        let path = std::env::temp_dir().join(format!(
            "phi-session-empty-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"").unwrap();

        let error = Session::load(&path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(error.to_string().contains("session input is empty"));
    }

    #[test]
    fn invalid_session_json_reports_clear_error() {
        let error = load_bytes(br#"{"not":"a session""#).unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("invalid session JSON"),
            "unexpected error message: {message}"
        );
    }

    #[test]
    fn session_uses_phi_step_expr_storage() {
        let root = PhiStepExpr::new(
            PhiAgentStep::turn_end("root"),
            vec![PhiMessage::user("hello")],
        );
        let session = Session::from_expr(root.commit(
            PhiAgentStep::request_provider(
                "next",
                &crate::config::ModelRequestDefaults {
                    model: "test-model".into(),
                    temperature: None,
                    max_tokens: 128,
                    enable_reasoning: false,
                    reasoning_effort: crate::config::ReasoningEffort::Medium,
                },
            ),
            vec![PhiMessage::assistant("answer")],
        ));

        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["frames"].as_array().unwrap().len(), 2);
        assert_eq!(json["frames"][0]["step"]["kind"], "turn_end");
        assert_eq!(json["frames"][1]["step"]["kind"], "request_provider");

        let loaded: Session = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.history(), session.history());
        assert_eq!(loaded.step(), session.step());
    }

    #[test]
    fn pending_tool_result_without_call_id_round_trips() {
        let assistant = crate::message::PhiAssistantMessage::tool_calls(vec![
            crate::executor::ToolCallRequest {
                id: "call-1".to_string(),
                call_id: None,
                name: "lookup".to_string(),
                arguments: serde_json::json!({}),
            },
            crate::executor::ToolCallRequest {
                id: "call-2".to_string(),
                call_id: None,
                name: "lookup".to_string(),
                arguments: serde_json::json!({}),
            },
        ]);
        let session = Session::from_root(
            PhiAgentStep::ReAct(
                crate::session::PhiReActStep::request_executor_turn(
                    "pending",
                    Vec::new(),
                    assistant,
                    vec![crate::message::PhiToolResultMessage {
                        id: Some("call-1".to_string()),
                        name: Some("lookup".to_string()),
                        result: serde_json::json!({"ok": true}),
                    }],
                )
                .expect("serialized request executor fixture must be valid"),
            ),
            vec![PhiMessage::user("lookup twice")],
        );

        let serialized = serde_json::to_vec(&session).unwrap();
        let restored = load_bytes(&serialized).unwrap();
        assert_eq!(restored.step(), session.step());
    }

    #[test]
    fn session_serializes_structured_variable_effects() {
        let root = PhiStepExpr::new(
            PhiAgentStep::turn_end("root"),
            vec![PhiMessage::user("hello")],
        )
        .store(COUNT, 3)
        .remove(CLEARED);
        let session = Session::from_expr(root);

        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["frames"][0]["delta"]["history"][0]["role"], "user");
        assert_eq!(
            json["frames"][0]["delta"]["effects"]["count"]["kind"],
            "store"
        );
        assert_eq!(json["frames"][0]["delta"]["effects"]["count"]["value"], 3);
        assert_eq!(
            json["frames"][0]["delta"]["effects"]["cleared"]["kind"],
            "remove"
        );

        let loaded: Session = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.history(), session.history());
        let expr = loaded.into_expr();
        assert_eq!(expr.lookup(COUNT), Some(3));
        assert_eq!(expr.lookup(CLEARED), None);
    }

    #[test]
    fn external_store_and_remove_update_the_outer_delta() {
        let session = Session::from_root(PhiAgentStep::turn_end("root"), Vec::<PhiMessage>::new())
            .store_json("answer", serde_json::json!({"value": 42}))
            .unwrap();
        let stored = serde_json::to_value(&session).unwrap();
        assert_eq!(
            stored["frames"][0]["delta"]["effects"]["answer"]["kind"],
            "store"
        );
        assert_eq!(
            stored["frames"][0]["delta"]["effects"]["answer"]["value"]["value"],
            42
        );

        let removed = session.remove_key("answer").unwrap();
        let removed = serde_json::to_value(&removed).unwrap();
        assert_eq!(
            removed["frames"][0]["delta"]["effects"]["answer"]["kind"],
            "remove"
        );
    }

    #[test]
    fn appending_messages_only_rebuilds_the_outermost_frame() {
        let parent = PhiStepExpr::new(
            PhiAgentStep::turn_end("parent"),
            vec![PhiMessage::user("old input")],
        );
        let session = Session::from_expr(
            parent
                .clone()
                .commit(PhiAgentStep::request_compact(), Vec::<PhiMessage>::new())
                .store(ATTEMPT, 2),
        );

        let appended = session.append_messages([
            PhiMessage::user("new input"),
            PhiMessage::assistant("prefill"),
        ]);
        let expr = appended.clone().into_expr();

        assert!(matches!(
            appended.step(),
            PhiAgentStep::ReAct(crate::session::PhiReActStep::RequestCompact { retain_rate: 0.1 })
        ));
        assert_eq!(expr.expr().unwrap().history(), parent.history());
        assert_eq!(expr.lookup(ATTEMPT), Some(2));
        assert_eq!(
            appended.history(),
            &[
                PhiMessage::user("old input"),
                PhiMessage::user("new input"),
                PhiMessage::assistant("prefill"),
            ]
        );

        let restored = load_bytes(&serde_json::to_vec(&appended).unwrap()).unwrap();
        assert_eq!(restored.step(), appended.step());
        assert_eq!(restored.history(), appended.history());
    }

    #[test]
    fn rollback_removes_one_frame_and_keeps_a_root_session_stable() {
        let root = Session::from_root(
            PhiAgentStep::turn_end("root"),
            vec![PhiMessage::user("root message")],
        );
        let branched = Session::from_expr(root.clone().into_expr().commit(
            PhiAgentStep::turn_end("outer"),
            vec![PhiMessage::assistant("outer message")],
        ));

        let rolled_back = branched.rollback();
        assert_eq!(rolled_back.step(), root.step());
        assert_eq!(rolled_back.history(), root.history());

        let root_after_rollback = root.clone().rollback();
        assert_eq!(root_after_rollback.step(), root.step());
        assert_eq!(root_after_rollback.history(), root.history());
    }

    #[test]
    fn rollback_to_keeps_the_nearest_requested_step_kind() {
        let root = Session::from_root(PhiAgentStep::turn_end("root"), Vec::<PhiMessage>::new());
        let compact = root.next(PhiReActStep::request_compact()).unwrap();
        let current = compact
            .clone()
            .next(PhiReActStep::request_provider(
                "resume",
                &test_model_defaults(),
            ))
            .unwrap();

        let rolled_back = current
            .rollback_to(SessionStepKind::RequestCompact)
            .unwrap();
        assert!(matches!(
            rolled_back.step(),
            PhiAgentStep::ReAct(crate::session::PhiReActStep::RequestCompact { .. })
        ));
        assert_eq!(rolled_back.frames().len(), 2);
        assert_eq!(rolled_back.history(), compact.history());
    }

    #[test]
    fn round_trips_deep_session_without_nested_expr_json() {
        let mut expr = PhiStepExpr::new(PhiAgentStep::turn_end("root"), PhiHistory::default());
        for index in 0..512 {
            expr = expr.commit(
                PhiAgentStep::turn_end(format!("frame-{index}")),
                PhiHistory::default(),
            );
        }
        let session = Session::from_expr(expr);

        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["frames"].as_array().unwrap().len(), 513);

        let loaded: Session = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.step().detail(), "frame-511");
    }

    #[test]
    fn failed_frame_with_non_empty_delta_round_trips() {
        let session = load_bytes(
            br#"{
                "frames": [
                    {
                        "step": {"kind": "turn_end", "detail": "done"},
                        "delta": {
                            "history": [{"role": "user", "content": "hello"}]
                        }
                    },
                    {
                        "step": {
                            "kind": "failed",
                            "error": {"kind": "module", "detail": "bad"}
                        },
                        "delta": {
                            "history": [{"role": "assistant", "content": {"content": "oops"}}]
                        }
                    }
                ]
            }"#,
        )
        .unwrap();

        assert!(
            matches!(session.step(), PhiAgentStep::Failed(failed) if failed.error().detail() == "bad")
        );
        assert_eq!(
            session.history(),
            &[PhiMessage::user("hello"), PhiMessage::assistant("oops")]
        );
        let serialized = serde_json::to_vec(&session).unwrap();
        let restored = load_bytes(&serialized).unwrap();
        assert_eq!(restored.step(), session.step());
        assert_eq!(restored.history(), session.history());
    }

    #[test]
    fn rejects_root_compacted_frame_without_parent_expr() {
        let error = load_bytes(
            br#"{
                "frames": [
                    {
                        "step": {"kind": "compacted"},
                        "delta": {
                            "history": [{"role": "user", "content": "summary"}]
                        }
                    }
                ]
            }"#,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("compacted frame must preserve a parent expr"),
            "unexpected error: {error}"
        );
    }
}
