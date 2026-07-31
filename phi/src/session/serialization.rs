use std::{
    io::{self, Write},
    path::Path,
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
    let decoded = decode_text(input)?;
    if decoded.trim().is_empty() {
        return Ok(Session::empty());
    }
    let session: Session =
        serde_json::from_str(&decoded).map_err(|error| format!("invalid session JSON: {error}"))?;
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

fn write_ascii_json<W, T>(writer: &mut W, value: &T) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write,
    T: Serialize,
{
    let mut serializer = Serializer::with_formatter(&mut *writer, EnsureAsciiFormatter);
    value.serialize(&mut serializer)?;
    Ok(())
}

fn decode_text(input: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(stripped) = input.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Ok(String::from_utf8(stripped.to_vec())?);
    }

    if let Some(stripped) = input.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16_bytes(stripped, true);
    }

    if let Some(stripped) = input.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16_bytes(stripped, false);
    }

    match String::from_utf8(input.to_vec()) {
        Ok(text) => Ok(text),
        Err(_) if looks_like_utf16le(input) => decode_utf16_bytes(input, true),
        Err(_) if looks_like_utf16be(input) => decode_utf16_bytes(input, false),
        Err(error) => Err(error.into()),
    }
}

fn decode_utf16_bytes(
    input: &[u8],
    little_endian: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    if input.len() % 2 != 0 {
        return Err("utf-16 input has an odd number of bytes".into());
    }

    let words = input
        .chunks_exact(2)
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();

    Ok(String::from_utf16(&words)?)
}

fn looks_like_utf16le(input: &[u8]) -> bool {
    input.len() >= 2
        && input
            .iter()
            .skip(1)
            .step_by(2)
            .take(32)
            .any(|byte| *byte == 0)
}

fn looks_like_utf16be(input: &[u8]) -> bool {
    input.len() >= 2 && input.iter().step_by(2).take(32).any(|byte| *byte == 0)
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
    use crate::expr::PhiStepExpr;
    use crate::message::{PhiHistory, PhiMessage};
    use crate::session::PhiAgentStep;

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
    fn load_bytes_treats_whitespace_as_empty_session() {
        let loaded = load_bytes(b"   \n\t  ").unwrap();

        assert_eq!(loaded.step(), Session::empty().step());
        assert_eq!(loaded.history(), Session::empty().history());
    }

    #[test]
    fn loads_empty_session_file_as_new_session() {
        let path = std::env::temp_dir().join(format!(
            "phi-session-empty-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"").unwrap();

        let loaded = Session::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(loaded.step(), Session::empty().step());
        assert_eq!(loaded.history(), Session::empty().history());
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
                    thinking_token_budget: 0,
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
    fn session_serializes_structured_delta_store() {
        let root = PhiStepExpr::new(
            PhiAgentStep::turn_end("root"),
            vec![PhiMessage::user("hello")],
        )
        .with_store("count", 3)
        .without_store("cleared");
        let session = Session::from_expr(root);

        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["frames"][0]["delta"]["history"][0]["role"], "user");
        assert_eq!(
            json["frames"][0]["delta"]["store"]["bindings"]["count"]["kind"],
            "set"
        );
        assert_eq!(
            json["frames"][0]["delta"]["store"]["bindings"]["count"]["value"],
            3
        );
        assert_eq!(
            json["frames"][0]["delta"]["store"]["bindings"]["cleared"]["kind"],
            "unset"
        );

        let loaded: Session = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.history(), session.history());
        let expr = loaded.into_expr();
        assert_eq!(expr.lookup::<i32>("count"), Some(3));
        assert_eq!(expr.lookup::<bool>("cleared"), None);
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
                            "history": [{"role": "assistant", "content": "oops"}]
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
