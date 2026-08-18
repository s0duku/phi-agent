use crate::{
    executor::{ToolCallOutput, ToolOutputLimits},
    utils::{APPROX_BYTES_PER_TOKEN, approx_token_count},
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum JsonPathSegment {
    Key(String),
    Index(usize),
}

#[derive(Clone, Debug)]
struct StringLeaf {
    path: Vec<JsonPathSegment>,
    tokens: usize,
}

const TRUNCATION_NOTICE_RESERVE_NUMERATOR: usize = 3;
const TRUNCATION_NOTICE_RESERVE_DENOMINATOR: usize = 2;

pub fn format_truncated_text(text: &str, token_budget: usize) -> String {
    if approx_token_count(text) <= token_budget {
        return text.to_string();
    }

    truncate_text_to_token_budget(text, token_budget)
}

pub fn maybe_truncate_text(text: &str, threshold_tokens: usize) -> String {
    if approx_token_count(text) <= threshold_tokens {
        return text.to_string();
    }
    format_truncated_text(text, threshold_tokens)
}

pub fn sanitize_tool_call_output(
    output: ToolCallOutput,
    limits: ToolOutputLimits,
) -> ToolCallOutput {
    ToolCallOutput::new(sanitize_json_string_leaves(
        output.into_value(),
        limits.output_threshold_tokens,
    ))
}

pub fn sanitize_json_string_leaves(
    value: serde_json::Value,
    total_token_budget: usize,
) -> serde_json::Value {
    let mut leaves = Vec::new();
    collect_string_leaves(&value, &mut Vec::new(), &mut leaves);

    let total_tokens: usize = leaves.iter().map(|leaf| leaf.tokens).sum();
    if total_tokens <= total_token_budget {
        return value;
    }

    let mut sanitized = value;
    leaves.sort_by(|left, right| left.tokens.cmp(&right.tokens));

    let mut remaining_budget = total_token_budget;
    let mut remaining_leaves = leaves.len();

    for leaf in leaves {
        if remaining_leaves == 0 {
            break;
        }

        let share = remaining_budget / remaining_leaves;
        let assigned_tokens = share.min(leaf.tokens);

        if assigned_tokens < leaf.tokens
            && let Some(slot) = value_at_path_mut(&mut sanitized, &leaf.path)
            && let Some(text) = slot.as_str()
        {
            *slot = serde_json::Value::String(truncate_text_to_token_budget(text, assigned_tokens));
        }

        remaining_budget = remaining_budget.saturating_sub(assigned_tokens);
        remaining_leaves -= 1;
    }

    sanitized
}

pub fn truncate_middle_text(text: &str, body_bytes: usize) -> String {
    if text.len() <= body_bytes {
        return text.to_string();
    }

    if body_bytes == 0 {
        return truncation_marker(text.len(), text.len());
    }

    let head_budget = body_bytes / 2;
    let tail_budget = body_bytes - head_budget;
    let head = take_prefix_at_char_boundary(text, head_budget);
    let tail = take_suffix_at_char_boundary(text, tail_budget);
    let omitted = text
        .len()
        .saturating_sub(head.len())
        .saturating_sub(tail.len());
    let marker = truncation_marker(omitted, text.len());

    format!("{head}{marker}{tail}")
}

fn truncate_text_to_token_budget(text: &str, token_budget: usize) -> String {
    if token_budget == 0 {
        return String::new();
    }
    let max_notice = truncation_marker(text.len(), text.len());
    let notice_reserve_tokens = approx_token_count(&max_notice)
        .saturating_mul(TRUNCATION_NOTICE_RESERVE_NUMERATOR)
        .div_ceil(TRUNCATION_NOTICE_RESERVE_DENOMINATOR);
    let body_tokens = token_budget.saturating_sub(notice_reserve_tokens);
    truncate_middle_text(text, body_tokens.saturating_mul(APPROX_BYTES_PER_TOKEN))
}

fn truncation_marker(omitted_bytes: usize, original_bytes: usize) -> String {
    format!("\n\n[truncated: omitted {omitted_bytes} of {original_bytes} bytes]\n\n")
}

fn collect_string_leaves(
    value: &serde_json::Value,
    path: &mut Vec<JsonPathSegment>,
    leaves: &mut Vec<StringLeaf>,
) {
    match value {
        serde_json::Value::String(text) => leaves.push(StringLeaf {
            path: path.clone(),
            tokens: approx_token_count(text),
        }),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(JsonPathSegment::Index(index));
                collect_string_leaves(item, path, leaves);
                path.pop();
            }
        }
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                path.push(JsonPathSegment::Key(key.clone()));
                collect_string_leaves(value, path, leaves);
                path.pop();
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn value_at_path_mut<'a>(
    value: &'a mut serde_json::Value,
    path: &[JsonPathSegment],
) -> Option<&'a mut serde_json::Value> {
    let mut current = value;
    for segment in path {
        match segment {
            JsonPathSegment::Key(key) => {
                current = current.as_object_mut()?.get_mut(key)?;
            }
            JsonPathSegment::Index(index) => {
                current = current.as_array_mut()?.get_mut(*index)?;
            }
        }
    }
    Some(current)
}

fn take_prefix_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if max_bytes >= text.len() {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn take_suffix_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if max_bytes >= text.len() {
        return text;
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::{
        format_truncated_text, maybe_truncate_text, sanitize_json_string_leaves,
        sanitize_tool_call_output, truncate_middle_text,
    };
    use crate::{
        executor::{ToolCallOutput, ToolOutputLimits},
        utils::approx_token_count,
    };

    #[test]
    fn middle_truncation_keeps_head_and_tail() {
        let text = "0123456789abcdefghijklmnopqrstuvwxyz";
        let truncated = truncate_middle_text(text, 32);
        assert!(truncated.contains("[truncated: omitted "));
        let (head, tail) = truncated
            .split_once(" bytes]\n\n")
            .expect("truncated text should contain a middle marker");
        let head = head
            .split_once("[truncated:")
            .map(|(prefix, _)| prefix)
            .expect("truncated text should contain a marker prefix");
        assert!(!head.is_empty(), "head preview should be preserved");
        assert!(!tail.is_empty(), "tail preview should be preserved");
        assert!(
            text.starts_with(head.trim_end()),
            "head should come from original prefix"
        );
        assert!(
            text.ends_with(tail.trim_start()),
            "tail should come from original suffix"
        );
    }

    #[test]
    fn formatted_truncation_reports_original_shape() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let formatted = format_truncated_text(text, 4);
        assert!(formatted.contains("[truncated: omitted "));
        assert!(formatted.contains(" of "));
        assert!(formatted.contains(" bytes]"));
    }

    #[test]
    fn maybe_truncate_leaves_small_text_unchanged() {
        let text = "short output";
        assert_eq!(maybe_truncate_text(text, 32), text);
    }

    #[test]
    fn sanitizes_json_string_leaves_without_destroying_structure() {
        let value = serde_json::json!({
            "stdout": "a".repeat(400),
            "stderr": "b".repeat(200),
            "nested": {
                "message": "short"
            }
        });

        let sanitized = sanitize_json_string_leaves(value, 64);

        assert!(
            sanitized["stdout"]
                .as_str()
                .expect("stdout should stay a string")
                .contains("[truncated: omitted ")
        );
        assert!(
            sanitized["stderr"]
                .as_str()
                .expect("stderr should stay a string")
                .contains("[truncated: omitted ")
        );
        assert_eq!(sanitized["nested"]["message"], serde_json::json!("short"));
    }

    #[test]
    fn sanitizes_tool_call_output_value() {
        let output = ToolCallOutput::new(serde_json::json!({
            "error": "x".repeat(400),
            "stdout": "y".repeat(400)
        }));

        let sanitized = sanitize_tool_call_output(output, ToolOutputLimits::new(64));

        assert!(
            sanitized.as_value()["error"]
                .as_str()
                .expect("error should remain a string")
                .contains("[truncated: omitted ")
        );
        assert!(
            sanitized.as_value()["stdout"]
                .as_str()
                .expect("stdout should stay a string")
                .contains("[truncated: omitted ")
        );
    }

    #[test]
    fn truncation_uses_assigned_token_budget_after_notice_reserve() {
        let text = "0123456789".repeat(100);
        let truncated = format_truncated_text(&text, 64);

        assert!(truncated.contains("[truncated: omitted "));
        assert!(
            truncated.len() > 48,
            "preview should not be capped by a tiny byte limit"
        );
        assert!(
            approx_token_count(&truncated) <= 64,
            "truncated output should fit assigned token budget"
        );
    }
}
