use crate::{
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    message::{PhiAssistantMessage, PhiHistory, PhiMessage, PhiUserMessage},
    utils::APPROX_BYTES_PER_TOKEN,
};

use super::{PhiProviderCall, PhiRender, approx_history_token_count, approx_text_token_count};

const AUTO_COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS: usize = 20_000;
const COMPACT_SUMMARY_MAX_TOKENS: u64 = 4_096;
const COMPACT_PROMPT: &str = include_str!("../prompts/compact.txt");
const COMPACT_SUMMARY_PREFIX: &str = "[CONTEXT CHECKPOINT SUMMARY]";

pub(super) async fn compact_history(
    render: &PhiRender,
    request: &PhiProviderCall,
    history: &PhiHistory,
) -> PhiAgentRuntimeResult<PhiHistory> {
    let history_tokens = approx_history_token_count(history);
    if history_tokens <= AUTO_COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS {
        return Ok(history.clone());
    }

    let first_non_system = history
        .iter()
        .position(|message| !matches!(message, PhiMessage::System(_)));

    let Some(index) = first_non_system else {
        return Ok(history.clone());
    };

    let mut remaining_tokens = AUTO_COMPACT_RETAINED_USER_MESSAGE_MAX_TOKENS;
    let mut retained_users: Vec<(usize, PhiMessage)> = Vec::new();
    let non_system_messages = history.iter().skip(index).enumerate().collect::<Vec<_>>();

    for (offset, message) in non_system_messages.iter().rev() {
        let PhiMessage::User(PhiUserMessage::Text(text)) = message else {
            continue;
        };

        if remaining_tokens == 0 {
            break;
        }

        let message_index = index + offset;
        let tokens = approx_text_token_count(text);
        if tokens <= remaining_tokens {
            retained_users.push((message_index, (*message).clone()));
            remaining_tokens = remaining_tokens.saturating_sub(tokens);
            continue;
        }

        retained_users.push((
            message_index,
            PhiMessage::User(PhiUserMessage::Text(truncate_user_text_to_token_budget(
                text,
                remaining_tokens,
            ))),
        ));
        break;
    }

    retained_users.reverse();

    let retained_indices = retained_users
        .iter()
        .map(|(message_index, _)| *message_index)
        .collect::<std::collections::BTreeSet<_>>();
    let compacted = non_system_messages
        .iter()
        .filter(|(offset, _)| !retained_indices.contains(&(index + offset)))
        .map(|(_, message)| (*message).clone())
        .collect::<Vec<_>>();

    if compacted.is_empty() {
        return Ok(history.clone());
    }

    let prefix = retained_users
        .into_iter()
        .map(|(_, message)| message)
        .collect::<Vec<_>>();
    let compacted_history = PhiHistory::from_messages(compacted);
    let mut summary_input = compacted_history.clone();
    summary_input.push(PhiMessage::user(COMPACT_PROMPT));

    let mut summary_request = request.clone();
    summary_request.tools.clear();
    summary_request.temperature = Some(0.0);
    summary_request.enable_reasoning = false;
    summary_request.thinking_token_budget = 0;
    summary_request.max_tokens = summary_request
        .max_tokens
        .min(COMPACT_SUMMARY_MAX_TOKENS)
        .max(1);

    let summary = render
        .complete_rendered(&summary_request, render.render_messages(&summary_input))
        .await?
        .messages
        .into_iter()
        .filter_map(|message| match message {
            PhiMessage::Assistant(PhiAssistantMessage::Text(text)) => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if summary.is_empty() {
        return Err(PhiAgentRuntimeError::provider_response(
            "compact provider returned no assistant summary",
        ));
    }

    let mut next_history = history.iter().take(index).cloned().collect::<Vec<_>>();
    next_history.extend(prefix);
    next_history.push(PhiMessage::user(format!(
        "{COMPACT_SUMMARY_PREFIX}\n{summary}"
    )));

    Ok(next_history.into())
}

fn truncate_user_text_to_token_budget(text: &str, token_budget: usize) -> String {
    if token_budget == 0 {
        return String::new();
    }

    let mut end = text
        .len()
        .min(token_budget.saturating_mul(APPROX_BYTES_PER_TOKEN));
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::{
        config::ModelRequestDefaults,
        message::{PhiMessage, PhiUserMessage},
        render::{PhiModelResponse, PhiProviderCall, PhiRender, TestClient},
    };

    use super::compact_history;

    struct SummaryProvider;

    #[async_trait]
    impl TestClient for SummaryProvider {
        async fn complete(
            &self,
            _request: &PhiProviderCall,
            _messages: &crate::message::PhiHistory,
        ) -> crate::error::PhiAgentRuntimeResult<PhiModelResponse> {
            Ok(PhiModelResponse::unspecified(vec![PhiMessage::assistant(
                "summary from provider",
            )]))
        }
    }

    fn test_request() -> PhiProviderCall {
        PhiProviderCall::from_parts(&ModelRequestDefaults::defaults(), Vec::new())
    }

    #[tokio::test]
    async fn compact_history_leaves_small_history_unchanged() {
        let history = vec![
            PhiMessage::system("sys"),
            PhiMessage::user("hello"),
            PhiMessage::assistant("world"),
        ]
        .into();

        let render = PhiRender::from_test_client(Arc::new(SummaryProvider));
        assert_eq!(
            compact_history(&render, &test_request(), &history)
                .await
                .expect("compact should succeed"),
            history
        );
    }

    #[tokio::test]
    async fn compact_history_summarizes_middle_and_retains_recent_users() {
        let large = "x".repeat(100_000);
        let history = vec![
            PhiMessage::system("sys"),
            PhiMessage::user(&large),
            PhiMessage::assistant("a1"),
            PhiMessage::user("recent"),
        ]
        .into();

        let render = PhiRender::from_test_client(Arc::new(SummaryProvider));
        let compacted = compact_history(&render, &test_request(), &history)
            .await
            .expect("compact should succeed");
        let messages = compacted.to_messages();

        assert_eq!(messages[0], PhiMessage::system("sys"));
        assert_eq!(
            messages.last(),
            Some(&PhiMessage::user(
                "[CONTEXT CHECKPOINT SUMMARY]\nsummary from provider"
            ))
        );
        assert!(matches!(
            &messages[1],
            PhiMessage::User(PhiUserMessage::Text(text))
                if !text.is_empty() && text.len() < large.len()
        ));
        assert_eq!(messages[2], PhiMessage::user("recent"));
    }
}
