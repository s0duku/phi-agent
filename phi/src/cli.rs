use std::collections::BTreeMap;

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
    }

    pub(crate) fn parse(matches: &ArgMatches) -> Self {
        let mut ordered = BTreeMap::new();
        collect(matches, "user", PhiMessage::user, &mut ordered);
        collect(matches, "assistant", PhiMessage::assistant, &mut ordered);
        Self {
            messages: ordered.into_values().collect(),
        }
    }

    pub(crate) fn extend_from_matches(&mut self, matches: &ArgMatches) {
        self.messages.extend(Self::parse(matches).messages);
    }

    pub(crate) fn as_slice(&self) -> &[PhiMessage] {
        &self.messages
    }

    pub(crate) fn into_messages(self) -> Vec<PhiMessage> {
        self.messages
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
            MessageArgs::parse(&matches).as_slice(),
            [
                PhiMessage::user("one"),
                PhiMessage::assistant("two"),
                PhiMessage::user("three"),
            ]
        );
    }
}
