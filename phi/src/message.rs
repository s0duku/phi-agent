use std::ops::Index;
use std::sync::Arc;

use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "role", content = "content", rename_all = "lowercase")]
pub enum PhiMessage {
    System(String),
    User(PhiUserMessage),
    Tool(PhiToolMessage),
    Assistant(PhiAssistantMessage),
}

impl PhiMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System(content.into())
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User(PhiUserMessage::Text(content.into()))
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant(PhiAssistantMessage::Text(content.into()))
    }

    pub fn tool_call(
        id: Option<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self::Tool(PhiToolMessage::ToolCall {
            id,
            name: name.into(),
            arguments,
        })
    }

    pub fn tool_result(
        id: Option<String>,
        name: Option<String>,
        result: serde_json::Value,
    ) -> Self {
        Self::Tool(PhiToolMessage::ToolResult { id, name, result })
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub enum PhiToolMessage {
    ToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        result: serde_json::Value,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(untagged)]
pub enum PhiUserMessage {
    Text(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(untagged)]
pub enum PhiAssistantMessage {
    Text(String),
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        content: Vec<PhiReasoningContent>,
    },
}

impl PhiAssistantMessage {
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text(content.into())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type", content = "content", rename_all = "snake_case")]
pub enum PhiReasoningContent {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Summary(String),
    Redacted {
        data: String,
    },
    Encrypted(String),
}

impl PhiReasoningContent {
    pub fn display_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            Self::Summary(text) => Some(text),
            Self::Redacted { data } => Some(data),
            Self::Encrypted(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PhiHistory(Vec<Arc<PhiMessage>>);

impl PhiHistory {
    pub fn from_messages(messages: Vec<PhiMessage>) -> Self {
        Self(messages.into_iter().map(Arc::new).collect())
    }

    pub fn to_messages(&self) -> Vec<PhiMessage> {
        self.0.iter().map(|message| (**message).clone()).collect()
    }

    pub fn into_messages(self) -> Vec<PhiMessage> {
        self.0
            .into_iter()
            .map(|message| match Arc::try_unwrap(message) {
                Ok(message) => message,
                Err(message) => (*message).clone(),
            })
            .collect()
    }

    pub(crate) fn into_arcs(self) -> Vec<Arc<PhiMessage>> {
        self.0
    }

    pub fn iter(&self) -> impl Iterator<Item = &PhiMessage> {
        self.0.iter().map(|message| message.as_ref())
    }

    pub fn iter_rev(&self) -> impl Iterator<Item = &PhiMessage> {
        self.0.iter().rev().map(|message| message.as_ref())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push(&mut self, message: PhiMessage) {
        self.0.push(Arc::new(message));
    }

    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    pub fn remove(&mut self, index: usize) -> Arc<PhiMessage> {
        self.0.remove(index)
    }
}

impl Index<usize> for PhiHistory {
    type Output = PhiMessage;

    fn index(&self, index: usize) -> &Self::Output {
        self.0[index].as_ref()
    }
}

impl PartialEq<Vec<PhiMessage>> for PhiHistory {
    fn eq(&self, other: &Vec<PhiMessage>) -> bool {
        self.iter().eq(other.iter())
    }
}

impl PartialEq<&[PhiMessage]> for PhiHistory {
    fn eq(&self, other: &&[PhiMessage]) -> bool {
        self.iter().eq(other.iter())
    }
}

impl<const N: usize> PartialEq<&[PhiMessage; N]> for PhiHistory {
    fn eq(&self, other: &&[PhiMessage; N]) -> bool {
        self.iter().eq(other.iter())
    }
}

impl From<Vec<PhiMessage>> for PhiHistory {
    fn from(value: Vec<PhiMessage>) -> Self {
        Self::from_messages(value)
    }
}

impl Serialize for PhiHistory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for message in &self.0 {
            sequence.serialize_element(message.as_ref())?;
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for PhiHistory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Vec::<PhiMessage>::deserialize(deserializer).map(Self::from_messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trips_message_sequence() {
        let history = PhiHistory::from_messages(vec![
            PhiMessage::system("sys"),
            PhiMessage::user("hello"),
            PhiMessage::assistant("world"),
        ]);

        assert_eq!(
            history.to_messages(),
            vec![
                PhiMessage::system("sys"),
                PhiMessage::user("hello"),
                PhiMessage::assistant("world"),
            ]
        );
    }
}
