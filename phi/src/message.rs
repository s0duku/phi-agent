use std::ops::Index;
use std::sync::Arc;

use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize};

use crate::executor::ToolCallRequest;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "role", content = "content", rename_all = "lowercase")]
pub enum PhiMessage {
    System(String),
    User(PhiUserMessage),
    Assistant(PhiAssistantMessage),
    ToolResult(PhiToolResultMessage),
}

impl PhiMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System(content.into())
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User(PhiUserMessage::Text(content.into()))
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant(PhiAssistantMessage::text(content))
    }

    pub fn reasoning(id: Option<String>, content: Vec<PhiReasoningContent>) -> Self {
        Self::Assistant(PhiAssistantMessage::from_parts(
            None,
            vec![PhiReasoningBlock { id, content }],
            Vec::new(),
        ))
    }

    pub fn tool_call(
        id: Option<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        let name = name.into();
        Self::Assistant(PhiAssistantMessage::tool_calls(vec![ToolCallRequest {
            id: id.clone().unwrap_or_else(|| name.clone()),
            call_id: id,
            name,
            arguments,
        }]))
    }

    pub fn tool_result(
        id: Option<String>,
        name: Option<String>,
        result: serde_json::Value,
    ) -> Self {
        Self::ToolResult(PhiToolResultMessage { id, name, result })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiToolResultMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub result: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(untagged)]
pub enum PhiUserMessage {
    Text(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiAssistantMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning: Vec<PhiReasoningBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_context: Option<serde_json::Value>,
}

impl PhiAssistantMessage {
    pub fn from_parts(
        content: Option<String>,
        reasoning: Vec<PhiReasoningBlock>,
        tool_calls: Vec<ToolCallRequest>,
    ) -> Self {
        Self {
            content,
            reasoning,
            tool_calls,
            provider_context: None,
        }
    }

    pub fn text(content: impl Into<String>) -> Self {
        Self::from_parts(Some(content.into()), Vec::new(), Vec::new())
    }

    pub fn tool_calls(tool_calls: Vec<ToolCallRequest>) -> Self {
        Self::from_parts(None, Vec::new(), tool_calls)
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCallRequest>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.content.as_deref().is_none_or(str::is_empty)
            && self.reasoning.is_empty()
            && self.tool_calls.is_empty()
    }

    pub(crate) fn from_provider_parts(
        content: Option<String>,
        reasoning: Vec<PhiReasoningBlock>,
        tool_calls: Vec<ToolCallRequest>,
        provider_context: Option<serde_json::Value>,
    ) -> Self {
        Self {
            content,
            reasoning,
            tool_calls,
            provider_context,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiReasoningBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub content: Vec<PhiReasoningContent>,
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

    pub(crate) fn latest_provider_context(&self) -> Option<serde_json::Value> {
        self.iter_rev().find_map(|message| match message {
            PhiMessage::Assistant(assistant) => assistant.provider_context.clone(),
            _ => None,
        })
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
