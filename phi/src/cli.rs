use std::collections::BTreeMap;

use clap::{Arg, ArgAction, ArgMatches, Command};

use crate::message::{PhiMessage, PhiToolMessage};

#[derive(Clone, Debug, Eq, PartialEq)]
enum MessageArg {
    Message(PhiMessage),
    ToolResult(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MessageArgs {
    messages: Vec<MessageArg>,
}

impl MessageArgs {
    pub(crate) fn augment(command: Command) -> Command {
        command
            .arg(
                Arg::new("user")
                    .long("user")
                    .value_name("TEXT")
                    .action(ArgAction::Append),
            )
            .arg(
                Arg::new("assistant")
                    .long("assistant")
                    .value_name("TEXT")
                    .action(ArgAction::Append),
            )
            .arg(
                Arg::new("tool_result")
                    .long("tool-result")
                    .value_name("CONTENT")
                    .action(ArgAction::Append),
            )
    }

    pub(crate) fn parse(matches: &ArgMatches) -> Self {
        let mut ordered = BTreeMap::new();
        collect(
            matches,
            "user",
            |text| MessageArg::Message(PhiMessage::user(text)),
            &mut ordered,
        );
        collect(
            matches,
            "assistant",
            |text| MessageArg::Message(PhiMessage::assistant(text)),
            &mut ordered,
        );
        collect(matches, "tool_result", MessageArg::ToolResult, &mut ordered);
        Self {
            messages: ordered.into_values().collect(),
        }
    }

    pub(crate) fn extend_from_matches(&mut self, matches: &ArgMatches) {
        self.messages.extend(Self::parse(matches).messages);
    }

    pub(crate) fn resolve(
        &self,
        history: impl IntoIterator<Item = PhiMessage>,
    ) -> Result<Vec<PhiMessage>, String> {
        let mut resolved = history.into_iter().collect::<Vec<_>>();
        let history_len = resolved.len();
        for argument in &self.messages {
            match argument {
                MessageArg::Message(message) => resolved.push(message.clone()),
                MessageArg::ToolResult(content) => {
                    let (id, name) = latest_unanswered_tool_call(&resolved)?;
                    let result = serde_json::from_str(content)
                        .unwrap_or_else(|_| serde_json::Value::String(content.clone()));
                    resolved.push(PhiMessage::tool_result(id, Some(name), result));
                }
            }
        }
        Ok(resolved.split_off(history_len))
    }
}

fn latest_unanswered_tool_call(history: &[PhiMessage]) -> Result<(Option<String>, String), String> {
    let Some((call_index, id, name)) =
        history
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, message)| {
                let PhiMessage::Tool(PhiToolMessage::ToolCall { id, name, .. }) = message else {
                    return None;
                };
                Some((index, id.clone(), name.clone()))
            })
    else {
        return Err("--tool-result requires a preceding tool call in the session".to_string());
    };

    let answered = history[call_index + 1..].iter().any(|message| {
        let PhiMessage::Tool(PhiToolMessage::ToolResult {
            id: result_id,
            name: result_name,
            ..
        }) = message
        else {
            return false;
        };
        match &id {
            Some(id) => result_id.as_ref() == Some(id),
            None => result_id.is_none() && result_name.as_deref() == Some(name.as_str()),
        }
    });
    if answered {
        return Err(format!(
            "--tool-result cannot answer tool call {name}: it already has a result"
        ));
    }
    Ok((id, name))
}

fn collect(
    matches: &ArgMatches,
    id: &str,
    construct: impl Fn(String) -> MessageArg,
    ordered: &mut BTreeMap<usize, MessageArg>,
) {
    let Some(values) = matches.get_many::<String>(id) else {
        return;
    };
    let indices = matches
        .indices_of(id)
        .expect("message values must retain their argument positions");
    for (text, index) in values.zip(indices) {
        ordered.insert(index, construct(text.clone()));
    }
}

#[cfg(test)]
mod tests {
    use clap::{ArgMatches, Command};

    use super::MessageArgs;
    use crate::message::{PhiMessage, PhiToolMessage};

    #[test]
    fn messages_preserve_interleaved_cli_order() {
        let matches: ArgMatches = MessageArgs::augment(Command::new("messages"))
            .try_get_matches_from([
                "messages",
                "--user",
                "one",
                "--assistant",
                "two",
                "--user",
                "three",
            ])
            .expect("message arguments should parse");

        assert_eq!(
            MessageArgs::parse(&matches)
                .resolve(Vec::new())
                .expect("messages should resolve"),
            [
                PhiMessage::user("one"),
                PhiMessage::assistant("two"),
                PhiMessage::user("three"),
            ]
        );
    }

    #[test]
    fn tool_result_uses_latest_unanswered_call_metadata_and_parses_json() {
        let matches = MessageArgs::augment(Command::new("messages"))
            .try_get_matches_from(["messages", "--tool-result", r#"{"ok":true}"#])
            .expect("tool result should parse");
        let messages = MessageArgs::parse(&matches)
            .resolve(vec![PhiMessage::tool_call(
                Some("call-7".to_string()),
                "lookup",
                serde_json::json!({"query": "phi"}),
            )])
            .expect("tool result should resolve");

        assert!(matches!(
            &messages[0],
            PhiMessage::Tool(PhiToolMessage::ToolResult { id, name, result })
                if id.as_deref() == Some("call-7")
                    && name.as_deref() == Some("lookup")
                    && result == &serde_json::json!({"ok": true})
        ));
    }

    #[test]
    fn tool_result_rejects_missing_and_already_answered_calls() {
        let matches = MessageArgs::augment(Command::new("messages"))
            .try_get_matches_from(["messages", "--tool-result", "done"])
            .expect("tool result should parse");
        let args = MessageArgs::parse(&matches);
        assert!(args.resolve(Vec::new()).is_err());
        assert!(
            args.resolve(vec![
                PhiMessage::tool_call(Some("call-1".to_string()), "lookup", serde_json::json!({})),
                PhiMessage::tool_result(
                    Some("call-1".to_string()),
                    Some("lookup".to_string()),
                    serde_json::json!("old"),
                ),
            ])
            .is_err()
        );
    }
}
