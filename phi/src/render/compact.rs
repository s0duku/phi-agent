use crate::{
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    message::{PhiHistory, PhiMessage},
};

use super::{
    PhiProviderCall, PhiRender, approx_history_token_count, approx_message_token_count,
    approx_text_token_count,
};

const COMPACT_SUMMARY_MAX_TOKENS: u64 = 4_096;
const COMPACT_PROMPT: &str = include_str!("../prompts/compact.txt");

pub(super) fn compact_prompt_token_count() -> usize {
    approx_text_token_count(COMPACT_PROMPT)
}

pub(super) async fn compact_history(
    render: &PhiRender,
    request: &PhiProviderCall,
    history: &PhiHistory,
    retain_rate: f32,
) -> PhiAgentRuntimeResult<PhiHistory> {
    let system_end = history
        .iter()
        .position(|message| !matches!(message, PhiMessage::System(_)))
        .unwrap_or(history.len());
    if system_end == history.len() {
        return Ok(history.clone());
    }
    let retain_tokens =
        ((approx_history_token_count(history) as f32) * retain_rate).floor() as usize;
    let retain_start = retained_suffix_start(history, system_end, retain_tokens);
    if retain_start == system_end {
        return Ok(history.clone());
    }
    let messages = history.to_messages();
    let retained = messages[retain_start..].to_vec();
    let mut summary_input = PhiHistory::from_messages(messages[..retain_start].to_vec());
    summary_input.push(PhiMessage::user(COMPACT_PROMPT));

    let mut summary_request = request.clone();
    summary_request.tools.clear();
    summary_request.temperature = Some(0.0);
    summary_request.enable_reasoning = false;
    summary_request.max_tokens = summary_request
        .max_tokens
        .min(COMPACT_SUMMARY_MAX_TOKENS)
        .max(1);

    let summary = render
        .complete_rendered(&summary_request, render.render_messages(&summary_input))
        .await?
        .assistant
        .and_then(|assistant| assistant.content)
        .unwrap_or_default()
        .trim()
        .to_string();

    if summary.is_empty() {
        return Err(PhiAgentRuntimeError::provider_response(
            "compact provider returned no assistant summary",
        ));
    }

    let mut next_history = messages[..system_end].to_vec();
    next_history.push(PhiMessage::user(format!(
        "<compaction>{summary}</compaction>"
    )));
    next_history.extend(retained);

    Ok(next_history.into())
}

fn retained_suffix_start(history: &PhiHistory, system_end: usize, token_budget: usize) -> usize {
    let messages = history.iter().collect::<Vec<_>>();
    let mut start = messages.len();
    let mut tokens = 0usize;
    while start > system_end {
        let message_tokens = approx_message_token_count(messages[start - 1]);
        if start < messages.len() && tokens.saturating_add(message_tokens) > token_budget {
            break;
        }
        start -= 1;
        tokens = tokens.saturating_add(message_tokens);
    }

    loop {
        let mut paired_call_start = start;
        for message in &messages[start..] {
            let PhiMessage::ToolResult(result) = message else {
                continue;
            };
            let Some(id) = result.id.as_ref() else {
                continue;
            };
            if let Some(call_index) = messages[..start].iter().rposition(|candidate| {
                matches!(
                    candidate,
                    PhiMessage::Assistant(assistant)
                        if assistant.tool_calls.iter().any(|call| {
                            call.call_id.as_ref().unwrap_or(&call.id) == id
                        })
                )
            }) {
                paired_call_start = paired_call_start.min(call_index);
            }
        }
        if paired_call_start == start || paired_call_start < system_end {
            break;
        }
        start = paired_call_start;
    }
    start
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use crate::{
        config::ModelRequestDefaults,
        message::PhiMessage,
        render::{PhiModelResponse, PhiProviderCall, PhiRender, TestClient},
    };

    use super::{compact_history, retained_suffix_start};

    struct SummaryProvider;

    struct CapturingSummaryProvider {
        messages: Arc<Mutex<Option<crate::message::PhiHistory>>>,
    }

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

    #[async_trait]
    impl TestClient for CapturingSummaryProvider {
        async fn complete(
            &self,
            _request: &PhiProviderCall,
            messages: &crate::message::PhiHistory,
        ) -> crate::error::PhiAgentRuntimeResult<PhiModelResponse> {
            *self
                .messages
                .lock()
                .expect("capture lock should be healthy") = Some(messages.clone());
            Ok(PhiModelResponse::unspecified(vec![PhiMessage::assistant(
                "summary",
            )]))
        }
    }

    fn test_request() -> PhiProviderCall {
        PhiProviderCall::from_parts(&ModelRequestDefaults::defaults(), Vec::new())
    }

    #[tokio::test]
    async fn compact_history_leaves_history_unchanged_when_rate_covers_all() {
        let history = vec![
            PhiMessage::system("sys"),
            PhiMessage::user("hello"),
            PhiMessage::assistant("world"),
        ]
        .into();

        let render = PhiRender::from_test_client(Arc::new(SummaryProvider));
        assert_eq!(
            compact_history(&render, &test_request(), &history, 1.0)
                .await
                .expect("compact should succeed"),
            history
        );
    }

    #[tokio::test]
    async fn compact_history_places_summary_before_the_retained_suffix() {
        let large = "x".repeat(100_000);
        let history = vec![
            PhiMessage::system("sys"),
            PhiMessage::user(&large),
            PhiMessage::assistant("a1"),
            PhiMessage::user("recent"),
        ]
        .into();

        let render = PhiRender::from_test_client(Arc::new(SummaryProvider));
        let compacted = compact_history(&render, &test_request(), &history, 0.1)
            .await
            .expect("compact should succeed");
        let messages = compacted.to_messages();

        assert_eq!(messages[0], PhiMessage::system("sys"));
        assert_eq!(
            messages[1],
            PhiMessage::user("<compaction>summary from provider</compaction>")
        );
        assert_eq!(messages[2], PhiMessage::assistant("a1"));
        assert_eq!(messages[3], PhiMessage::user("recent"));
        assert_eq!(messages.len(), 4);
    }

    #[tokio::test]
    async fn compact_history_partitions_every_original_message_without_dropping_any() {
        let captured = Arc::new(Mutex::new(None));
        let history = vec![
            PhiMessage::system("system"),
            PhiMessage::user("old user"),
            PhiMessage::assistant("old assistant"),
            PhiMessage::user("recent user"),
        ]
        .into();
        let render = PhiRender::from_test_client(Arc::new(CapturingSummaryProvider {
            messages: captured.clone(),
        }));

        let compacted = compact_history(&render, &test_request(), &history, 0.1)
            .await
            .expect("compact should succeed");
        let summary_input = captured
            .lock()
            .expect("capture lock should be healthy")
            .clone()
            .expect("summary request should capture its input");
        let summary_messages = summary_input.to_messages();
        assert_eq!(
            summary_messages.last(),
            Some(&PhiMessage::user(include_str!("../prompts/compact.txt")))
        );

        let original = history.to_messages();
        let retained = compacted.to_messages()[2..].to_vec();
        assert_eq!(&original[..1], &compacted.to_messages()[..1]);
        assert_eq!(
            original,
            summary_messages[..summary_messages.len() - 1]
                .iter()
                .chain(retained.iter())
                .cloned()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn retained_suffix_keeps_tool_call_and_result_together() {
        let history = vec![
            PhiMessage::system("sys"),
            PhiMessage::user("old"),
            PhiMessage::tool_call(
                Some("call-1".into()),
                "bash",
                serde_json::json!({"cmd": "echo ok"}),
            ),
            PhiMessage::tool_result(
                Some("call-1".into()),
                Some("bash".into()),
                serde_json::json!({"stdout": "ok"}),
            ),
        ]
        .into();

        let start = retained_suffix_start(&history, 1, 1);

        assert_eq!(start, 2);
    }
}
