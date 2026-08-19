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

pub fn truncate_tool_output(output: ToolCallOutput, limits: ToolOutputLimits) -> ToolCallOutput {
    ToolCallOutput::new(truncate_tool_output_value(
        output.into_value(),
        limits.output_threshold_tokens,
    ))
}

pub fn truncate_tool_output_value(
    value: serde_json::Value,
    total_token_budget: usize,
) -> serde_json::Value {
    let original_tokens = serialized_token_count(&value);
    if original_tokens <= total_token_budget {
        return value;
    }

    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(truncate_text_to_token_budget(&text, total_token_budget))
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            let conservative = conservatively_truncate_strings(value, total_token_budget);
            if serialized_token_count(&conservative) <= total_token_budget {
                conservative
            } else {
                aggressively_truncate_containers(conservative, total_token_budget)
            }
        }
        scalar => scalar,
    }
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
        return truncation_marker(text.len(), text.len());
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

fn conservatively_truncate_strings(
    value: serde_json::Value,
    total_token_budget: usize,
) -> serde_json::Value {
    let mut leaves = Vec::new();
    collect_string_leaves(&value, &mut Vec::new(), &mut leaves);
    if leaves.is_empty() {
        return value;
    }

    let structure_only = value_without_string_contents(&value);
    let structure_tokens = serialized_token_count(&structure_only);
    let mut remaining_budget = total_token_budget.saturating_sub(structure_tokens);
    let mut remaining_leaves = leaves.len();
    leaves.sort_by_key(|leaf| leaf.tokens);

    let mut truncated = value;
    for leaf in leaves {
        let share = remaining_budget / remaining_leaves;
        let assigned_tokens = leaf.tokens.min(share);
        let text = value_at_path_mut(&mut truncated, &leaf.path)
            .and_then(|value| value.as_str())
            .map(str::to_string);
        if assigned_tokens < leaf.tokens
            && let Some(text) = text
        {
            let replacement = truncate_text_to_token_budget(&text, assigned_tokens);
            if let Some(slot) = value_at_path_mut(&mut truncated, &leaf.path) {
                *slot = serde_json::Value::String(replacement);
            }
        }

        remaining_budget = remaining_budget.saturating_sub(assigned_tokens);
        remaining_leaves -= 1;
    }
    truncated
}

fn collect_string_leaves(
    value: &serde_json::Value,
    path: &mut Vec<JsonPathSegment>,
    leaves: &mut Vec<StringLeaf>,
) {
    match value {
        serde_json::Value::String(text) => {
            leaves.push(StringLeaf {
                path: path.clone(),
                tokens: approx_token_count(text),
            });
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(JsonPathSegment::Index(index));
                collect_string_leaves(item, path, leaves);
                path.pop();
            }
        }
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                path.push(JsonPathSegment::Key(key.clone()));
                collect_string_leaves(child, path, leaves);
                path.pop();
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn value_without_string_contents(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(_) => serde_json::Value::String(String::new()),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(value_without_string_contents).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, child)| (key.clone(), value_without_string_contents(child)))
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

fn aggressively_truncate_containers(
    mut value: serde_json::Value,
    total_token_budget: usize,
) -> serde_json::Value {
    loop {
        let current_tokens = serialized_token_count(&value);
        if current_tokens <= total_token_budget {
            return value;
        }

        let mut paths = Vec::new();
        collect_container_paths(&value, &mut Vec::new(), &mut paths);
        paths.sort_by_key(|path| std::cmp::Reverse(path.len()));

        let mut changed = false;
        for path in paths {
            let Some(container) = value_at_path_mut(&mut value, &path) else {
                continue;
            };
            if !container.is_array() && !container.is_object() {
                continue;
            }

            let original_container_tokens = serialized_token_count(container);
            let replacement = truncated_container_string(container, total_token_budget);
            if serialized_token_count(&replacement) < original_container_tokens {
                *container = replacement;
                changed = true;
                break;
            }
        }

        if !changed {
            if value.is_array() || value.is_object() {
                return truncated_container_string(&value, total_token_budget);
            }
            if value.is_string() {
                return value;
            }
            return truncated_value(
                "result too large; no further compression is worthwhile",
                current_tokens,
            );
        }
    }
}

fn collect_container_paths(
    value: &serde_json::Value,
    path: &mut Vec<JsonPathSegment>,
    paths: &mut Vec<Vec<JsonPathSegment>>,
) {
    match value {
        serde_json::Value::Array(items) => {
            paths.push(path.clone());
            for (index, item) in items.iter().enumerate() {
                path.push(JsonPathSegment::Index(index));
                collect_container_paths(item, path, paths);
                path.pop();
            }
        }
        serde_json::Value::Object(map) => {
            paths.push(path.clone());
            for (key, child) in map {
                path.push(JsonPathSegment::Key(key.clone()));
                collect_container_paths(child, path, paths);
                path.pop();
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

fn truncated_container_string(
    value: &serde_json::Value,
    target_tokens: usize,
) -> serde_json::Value {
    let kind = if value.is_array() { "array" } else { "object" };
    let serialized = serde_json::to_string(value).unwrap_or_default();
    let original_tokens = approx_token_count(&serialized);
    let notice = format!("[{kind} truncated: original {original_tokens} tokens]");
    let make_candidate = |body_bytes: usize| {
        let body = if body_bytes >= serialized.len() {
            serialized.clone()
        } else if body_bytes == 0 {
            String::new()
        } else {
            truncate_middle_text(&serialized, body_bytes)
        };
        serde_json::Value::String(if body.is_empty() {
            notice.clone()
        } else {
            format!("{notice}\n{body}")
        })
    };

    if serialized_token_count(&make_candidate(0)) > target_tokens {
        return serde_json::Value::String(notice);
    }

    let mut low = 0;
    let mut high = serialized.len().saturating_add(1);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if serialized_token_count(&make_candidate(middle)) <= target_tokens {
            low = middle;
        } else {
            high = middle;
        }
    }
    make_candidate(low)
}

fn serialized_token_count(value: &serde_json::Value) -> usize {
    serde_json::to_string(value)
        .map(|serialized| approx_token_count(&serialized))
        .unwrap_or(usize::MAX)
}

fn truncated_value(message: &str, original_tokens: usize) -> serde_json::Value {
    serde_json::json!({
        "__truncated__": format!("{message}; original result was about {original_tokens} tokens")
    })
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
        format_truncated_text, maybe_truncate_text, serialized_token_count, truncate_middle_text,
        truncate_tool_output, truncate_tool_output_value,
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
    fn truncates_string_leaves_without_destroying_structure() {
        let value = serde_json::json!({
            "stdout": "a".repeat(400),
            "stderr": "b".repeat(200),
            "nested": {
                "message": "short"
            }
        });

        let truncated = truncate_tool_output_value(value, 64);

        assert!(
            truncated["stdout"]
                .as_str()
                .expect("stdout should stay a string")
                .contains("[truncated: omitted ")
        );
        assert!(
            truncated["stderr"]
                .as_str()
                .expect("stderr should stay a string")
                .contains("[truncated: omitted ")
        );
        assert_eq!(truncated["nested"]["message"], serde_json::json!("short"));
    }

    #[test]
    fn truncates_tool_call_output_value() {
        let output = ToolCallOutput::new(serde_json::json!({
            "error": "x".repeat(400),
            "stdout": "y".repeat(400)
        }));

        let truncated = truncate_tool_output(output, ToolOutputLimits::new(64));

        assert!(
            truncated.as_value()["error"]
                .as_str()
                .expect("error should remain a string")
                .contains("[truncated: omitted ")
        );
        assert!(
            truncated.as_value()["stdout"]
                .as_str()
                .expect("stdout should stay a string")
                .contains("[truncated: omitted ")
        );
    }

    #[test]
    fn compacts_large_arrays_with_a_visible_notice() {
        let value = serde_json::Value::Array((0..200).map(serde_json::Value::from).collect());

        let truncated = truncate_tool_output_value(value, 40);
        let serialized = serde_json::to_string(&truncated).expect("truncated value should be JSON");

        assert!(
            truncated
                .as_str()
                .is_some_and(|text| text.contains("[array truncated:"))
        );
        assert!(serialized.contains("[truncated:"));
        assert!(
            serialized_token_count(&truncated) <= 40,
            "array result should fit the configured serialized budget"
        );
    }

    #[test]
    fn compacts_object_fields_with_a_visible_notice() {
        let value = serde_json::json!({
            "first": 1,
            "second": 2,
            "third": 3,
            "fourth": 4,
            "fifth": 5,
            "sixth": 6,
            "seventh": 7,
            "eighth": 8
        });

        let truncated = truncate_tool_output_value(value, 20);
        let serialized = serde_json::to_string(&truncated).expect("truncated value should be JSON");

        assert!(
            truncated
                .as_str()
                .is_some_and(|text| text.contains("[object truncated:"))
        );
        assert!(serialized.contains("object truncated"));
        assert!(serialized_token_count(&truncated) <= 20);
    }

    #[test]
    fn compacts_nested_containers_until_the_serialized_result_fits() {
        let value = serde_json::json!({
            "groups": [
                (0..100).collect::<Vec<_>>(),
                (100..200).collect::<Vec<_>>()
            ],
            "metadata": {
                "one": 1,
                "two": 2,
                "three": 3,
                "four": 4
            }
        });

        let truncated = truncate_tool_output_value(value, 40);
        let serialized = serde_json::to_string(&truncated).expect("truncated value should be JSON");

        assert!(serialized.contains("[truncated:") || serialized.contains("__truncated__"));
        assert!(
            serialized_token_count(&truncated) <= 40,
            "nested result should fit the configured serialized budget"
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
