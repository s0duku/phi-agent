use std::{collections::BTreeMap, path::PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Command};

use crate::message::PhiMessage;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MessageArgs {
    messages: Vec<PhiMessage>,
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
                Arg::new("user_file")
                    .long("user-file")
                    .value_name("FILE")
                    .value_parser(clap::value_parser!(PathBuf))
                    .action(ArgAction::Append),
            )
            .arg(
                Arg::new("assistant_file")
                    .long("assistant-file")
                    .value_name("FILE")
                    .value_parser(clap::value_parser!(PathBuf))
                    .action(ArgAction::Append),
            )
    }

    pub(crate) fn parse(matches: &ArgMatches) -> Result<Self, String> {
        let mut ordered = BTreeMap::new();
        collect(matches, "user", PhiMessage::user, &mut ordered);
        collect(matches, "assistant", PhiMessage::assistant, &mut ordered);
        collect_files(matches, "user_file", PhiMessage::user, &mut ordered)?;
        collect_files(matches, "assistant_file", PhiMessage::assistant, &mut ordered)?;
        Ok(Self {
            messages: ordered.into_values().collect(),
        })
    }

    pub(crate) fn extend_from_matches(&mut self, matches: &ArgMatches) -> Result<(), String> {
        self.messages.extend(Self::parse(matches)?.messages);
        Ok(())
    }

    pub(crate) fn messages(&self) -> Vec<PhiMessage> {
        self.messages.clone()
    }
}

fn collect(
    matches: &ArgMatches,
    id: &str,
    construct: impl Fn(String) -> PhiMessage,
    ordered: &mut BTreeMap<usize, PhiMessage>,
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

fn collect_files(
    matches: &ArgMatches,
    id: &str,
    construct: impl Fn(String) -> PhiMessage,
    ordered: &mut BTreeMap<usize, PhiMessage>,
) -> Result<(), String> {
    let Some(values) = matches.get_many::<PathBuf>(id) else {
        return Ok(());
    };
    let indices = matches
        .indices_of(id)
        .expect("message file values must retain their argument positions");
    for (path, index) in values.zip(indices) {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read {id} {}: {error}", path.display()))?;
        ordered.insert(index, construct(text));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::{ArgMatches, Command};

    use super::MessageArgs;
    use crate::message::PhiMessage;

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
            MessageArgs::parse(&matches).unwrap().messages(),
            [
                PhiMessage::user("one"),
                PhiMessage::assistant("two"),
                PhiMessage::user("three"),
            ]
        );
    }

    #[test]
    fn tool_result_is_not_a_cli_message_option() {
        assert!(
            MessageArgs::augment(Command::new("messages"))
                .try_get_matches_from(["messages", "--tool-result", "done"])
                .is_err()
        );
    }

    #[test]
    fn message_files_preserve_interleaved_cli_order() {
        let path = std::env::temp_dir().join(format!(
            "phi-message-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, "from file").unwrap();
        let matches: ArgMatches = MessageArgs::augment(Command::new("messages"))
            .try_get_matches_from([
                "messages",
                "--user",
                "before",
                "--assistant-file",
                path.to_string_lossy().as_ref(),
                "--user",
                "after",
            ])
            .expect("message file arguments should parse");

        assert_eq!(
            MessageArgs::parse(&matches).unwrap().messages(),
            [
                PhiMessage::user("before"),
                PhiMessage::assistant("from file"),
                PhiMessage::user("after"),
            ]
        );
        std::fs::remove_file(path).unwrap();
    }
}
