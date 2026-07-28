use crate::{
    message::{PhiAssistantMessage, PhiMessage, PhiReasoningContent, PhiUserMessage},
    utils::approx_token_count,
};

pub(crate) fn approx_text_token_count(text: &str) -> usize {
    approx_token_count(text)
}

pub(crate) fn approx_message_token_count(message: &PhiMessage) -> usize {
    match message {
        PhiMessage::System(text) => approx_text_token_count(text),
        PhiMessage::User(PhiUserMessage::Text(text)) => approx_text_token_count(text),
        PhiMessage::Tool(tool) => approx_text_token_count(
            &serde_json::to_string(tool).expect("tool messages should serialize"),
        ),
        PhiMessage::Assistant(PhiAssistantMessage::Text(text)) => approx_text_token_count(text),
        PhiMessage::Assistant(PhiAssistantMessage::Reasoning { content, .. }) => content
            .iter()
            .filter_map(PhiReasoningContent::display_text)
            .map(approx_text_token_count)
            .sum(),
    }
}

pub(crate) fn approx_history_token_count(history: &crate::message::PhiHistory) -> usize {
    history.iter().map(approx_message_token_count).sum()
}

#[cfg(test)]
mod tests {
    use super::approx_history_token_count;
    use crate::{message::PhiMessage, utils::approx_token_count};

    #[test]
    fn history_token_count_sums_message_footprints() {
        let history = crate::message::PhiHistory::from_messages(vec![
            PhiMessage::system("sys"),
            PhiMessage::user("hello"),
            PhiMessage::assistant("world"),
        ]);

        assert_eq!(
            approx_history_token_count(&history),
            approx_token_count("sys") + approx_token_count("hello") + approx_token_count("world")
        );
    }
}
