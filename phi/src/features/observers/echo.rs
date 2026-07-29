use std::io::{self, IsTerminal, Write};

use owo_colors::{OwoColorize, Stream, Style};

use crate::{
    error::{PhiRuntimeError, PhiRuntimeResult},
    executor::ToolCallOutput,
    message::{
        PhiAssistantMessage, PhiHistory, PhiMessage, PhiReasoningContent, PhiToolMessage,
        PhiUserMessage,
    },
    module::{PhiAgentCommitEvent, PhiAgentStepEvent, PhiModule},
};

pub struct EchoModule {
    printed_any: bool,
}

impl EchoModule {
    pub fn new() -> Self {
        Self { printed_any: false }
    }

    fn echo_message(&mut self, message: &PhiMessage) {
        self.printed_any = true;
        eprintln!("{}", pretty_message(message));
    }
}

impl Drop for EchoModule {
    fn drop(&mut self) {
        if self.printed_any && io::stderr().is_terminal() {
            let _ = writeln!(io::stderr().lock());
            let _ = io::stderr().lock().flush();
        }
    }
}

impl PhiModule for EchoModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        match event {
            PhiAgentStepEvent::AfterModelResponseParsed { messages } => {
                for message in messages.iter() {
                    self.echo_message(message);
                }
            }
            PhiAgentStepEvent::BeforeToolCall { request, .. } => {
                let message = PhiMessage::tool_call(
                    request.call_id.clone().or(Some(request.id.clone())),
                    request.name.clone(),
                    request.arguments.clone(),
                );
                self.echo_message(&message);
            }
            PhiAgentStepEvent::AfterToolCall { result, .. } => {
                self.printed_any = true;
                let rendered = pretty_tool_result_event(&result.name, &result.id, &result.output);
                eprintln!("{rendered}");
            }
            _ => {}
        }
        Ok(())
    }

    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        match event {
            PhiAgentCommitEvent::WarningEmitted { message } => {
                self.printed_any = true;
                eprintln!("{}", pretty_warning(message));
            }
            PhiAgentCommitEvent::StepFailed { error } => {
                self.printed_any = true;
                eprintln!("{}", pretty_runtime_error(error));
            }
            _ => {}
        }
    }
}

fn pretty_warning(message: &str) -> String {
    pretty_warning_for_stream(message, Stream::Stderr)
}

fn pretty_warning_for_stream(message: &str, stream: Stream) -> String {
    format!(
        "\n{}\n{}",
        style_header("warning", stream),
        style_body("warning", message.trim_end(), stream)
    )
}

pub fn pretty_info(message: &str) -> String {
    pretty_info_for_stream(message, Stream::Stderr)
}

fn pretty_info_for_stream(message: &str, stream: Stream) -> String {
    format!(
        "\n{}\n{}",
        style_header("info", stream),
        style_body("info", message.trim_end(), stream)
    )
}

fn pretty_runtime_error(error: &PhiRuntimeError) -> String {
    pretty_runtime_error_for_stream(error, Stream::Stderr)
}

fn pretty_runtime_error_for_stream(error: &PhiRuntimeError, stream: Stream) -> String {
    format!(
        "\n{}\n{}",
        style_header("error", stream),
        style_body("error", error.detail().trim_end(), stream)
    )
}

pub fn pretty_message(message: &PhiMessage) -> String {
    pretty_message_for_stream(message, Stream::Stderr)
}

fn pretty_message_for_stream(message: &PhiMessage, stream: Stream) -> String {
    match message {
        PhiMessage::System(content) => format_message("system", content, stream),
        PhiMessage::User(content) => format_message("user", &pretty_user_content(content), stream),
        PhiMessage::Tool(content) => pretty_tool_message(content, stream),
        PhiMessage::Assistant(content) => pretty_assistant_message(content, stream),
    }
}

fn format_message(role: &str, text: &str, stream: Stream) -> String {
    format!(
        "\n{}\n{}",
        style_header(role, stream),
        style_body(role, text.trim(), stream)
    )
}

fn pretty_user_content(content: &PhiUserMessage) -> String {
    match content {
        PhiUserMessage::Text(text) => text.clone(),
    }
}

fn pretty_assistant_content(content: &PhiAssistantMessage) -> String {
    match content {
        PhiAssistantMessage::Text(text) => text.clone(),
        PhiAssistantMessage::Reasoning { content, .. } => content
            .iter()
            .filter_map(PhiReasoningContent::display_text)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn pretty_assistant_message(content: &PhiAssistantMessage, stream: Stream) -> String {
    match content {
        PhiAssistantMessage::Text(text) => format_message("assistant", text, stream),
        PhiAssistantMessage::Reasoning { .. } => format_message(
            "assistant:reasoning",
            &pretty_assistant_content(content),
            stream,
        ),
    }
}

fn pretty_tool_message(content: &PhiToolMessage, stream: Stream) -> String {
    match content {
        PhiToolMessage::ToolCall {
            id,
            name,
            arguments,
        } => format_message(
            "tool:call",
            &pretty_tool_call(name, id.as_deref(), arguments),
            stream,
        ),
        PhiToolMessage::ToolResult { id, name, result } => {
            let label = id.clone().unwrap_or_else(|| "unknown".to_string());
            if let Ok(output) = serde_json::from_value::<ToolCallOutput>(result.clone()) {
                let tool_name = name.as_deref().unwrap_or("unknown");
                return format_message(
                    "tool:result",
                    &format!("{tool_name} {label}\n{}", pretty_tool_output(&output)),
                    stream,
                );
            }
            format_message(
                "tool:result",
                &format!("{label}\n{}", pretty_json_value(result)),
                stream,
            )
        }
    }
}

fn pretty_tool_result_event(tool_name: &str, tool_id: &str, result: &ToolCallOutput) -> String {
    pretty_tool_result_event_for_stream(tool_name, tool_id, result, Stream::Stderr)
}

fn pretty_tool_result_event_for_stream(
    tool_name: &str,
    tool_id: &str,
    result: &ToolCallOutput,
    stream: Stream,
) -> String {
    format_message(
        "tool:result",
        &format!("{} {}\n{}", tool_name, tool_id, pretty_tool_output(result)),
        stream,
    )
}

pub fn pretty_history(history: &PhiHistory) -> String {
    history
        .iter()
        .map(|message| pretty_message_for_stream(message, Stream::Stdout))
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn pretty_tool_call(name: &str, id: Option<&str>, arguments: &serde_json::Value) -> String {
    let mut parts = vec![name.to_string()];
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        parts.push(format!("#{id}"));
    }

    let rendered_arguments = match arguments {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) if map.is_empty() => String::new(),
        _ => format!("\n{}", pretty_json_value(arguments)),
    };

    format!("{}{}", parts.join(" "), rendered_arguments)
}

fn pretty_tool_output(output: &ToolCallOutput) -> String {
    let mut parts = Vec::new();
    parts.push(format!("ok: {}", output.is_ok()));
    if let Some(error) = output.error() {
        parts.push(format!("error: {error}"));
    }
    parts.push(format!("value:\n{}", pretty_json_value(output.as_value())));
    parts.join("\n")
}

fn pretty_json_value(value: &serde_json::Value) -> String {
    let Some(object) = value.as_object() else {
        return serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    };

    let status = object
        .get("status")
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let timed_out = object
        .get("timed_out")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let duration_ms = object
        .get("duration_ms")
        .and_then(serde_json::Value::as_u64);
    let waited_ms = object.get("waited_ms").and_then(serde_json::Value::as_u64);
    let stderr = object
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let stdout = object
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let output = object.get("output").and_then(serde_json::Value::as_str);
    let exit_code = object.get("exit_code").and_then(serde_json::Value::as_i64);
    let handle = object.get("handle").and_then(serde_json::Value::as_str);
    let stdout_artifact = object.get("stdout_artifact");
    let stderr_artifact = object.get("stderr_artifact");

    if object.contains_key("status")
        || object.contains_key("timed_out")
        || object.contains_key("duration_ms")
        || object.contains_key("waited_ms")
        || object.contains_key("stdout")
        || object.contains_key("stderr")
        || object.contains_key("output")
        || object.contains_key("exit_code")
        || stdout_artifact.is_some()
        || stderr_artifact.is_some()
    {
        let mut parts = vec![format!("status: {status}")];
        if let Some(duration_ms) = duration_ms {
            parts.push(format!("duration_ms: {duration_ms}"));
        }
        if let Some(waited_ms) = waited_ms {
            parts.push(format!("waited_ms: {waited_ms}"));
        }
        if timed_out {
            parts.push("timed_out: true".to_string());
        }
        if let Some(exit_code) = exit_code {
            parts.push(format!("exit_code: {exit_code}"));
        }
        if let Some(handle) = handle {
            parts.push(format!("handle: {handle}"));
        }
        if let Some(output) = output {
            if output.trim().is_empty() {
                parts.push(format!(
                    "output: {}",
                    serde_json::to_string(output).unwrap_or_else(|_| "\"\"".to_string())
                ));
            } else {
                parts.push(format!("output:\n{}", output.trim_end()));
            }
        }
        if !stdout.trim().is_empty() {
            parts.push(format!("stdout:\n{}", stdout.trim_end()));
        }
        if !stderr.trim().is_empty() {
            parts.push(format!("stderr:\n{}", stderr.trim_end()));
        }
        if let Some(artifact) = stdout_artifact {
            parts.push(format!(
                "stdout_artifact:\n{}",
                serde_json::to_string_pretty(artifact).unwrap_or_else(|_| artifact.to_string())
            ));
        }
        if let Some(artifact) = stderr_artifact {
            parts.push(format!(
                "stderr_artifact:\n{}",
                serde_json::to_string_pretty(artifact).unwrap_or_else(|_| artifact.to_string())
            ));
        }
        return parts.join("\n");
    }

    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn style_header(role: &str, stream: Stream) -> String {
    let label = format!("[{role}]");
    label
        .if_supports_color(stream, |text| text.style(header_style(role)))
        .to_string()
}

fn style_body(role: &str, text: &str, stream: Stream) -> String {
    text.if_supports_color(stream, |value| value.style(body_style(role)))
        .to_string()
}

fn header_style(role: &str) -> Style {
    match role {
        "system" => Style::new().blue().bold(),
        "user" => Style::new().cyan().bold(),
        "assistant" => Style::new().green().bold(),
        "assistant:reasoning" => Style::new().bright_black().bold(),
        "tool:call" => Style::new().yellow().bold(),
        "tool:result" => Style::new().magenta().bold(),
        "info" => Style::new().bright_black().bold(),
        "warning" => Style::new().yellow().bold(),
        "error" => Style::new().red().bold(),
        _ => Style::new().bold(),
    }
}

fn body_style(role: &str) -> Style {
    match role {
        "system" => Style::new().blue(),
        "user" => Style::new().cyan(),
        "assistant" => Style::new().green(),
        "assistant:reasoning" => Style::new().bright_black(),
        "tool:call" => Style::new().yellow(),
        "info" => Style::new().bright_black(),
        "warning" => Style::new().yellow(),
        "error" => Style::new().red(),
        _ => Style::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(input: &str) -> String {
        let mut output = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' && matches!(chars.peek(), Some('[')) {
                let _ = chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                continue;
            }
            output.push(ch);
        }
        output
    }

    #[test]
    fn formats_tool_call_as_transcript_block() {
        let rendered = pretty_message(&PhiMessage::tool_call(
            Some("call_123".to_string()),
            "bash",
            serde_json::json!({ "command": "pwd" }),
        ));

        assert_eq!(
            strip_ansi(&rendered),
            "\n[tool:call]\nbash #call_123\n{\n  \"command\": \"pwd\"\n}"
        );
    }

    #[test]
    fn job_close_call_arguments_are_not_misread_as_a_job_result() {
        let rendered = pretty_message(&PhiMessage::tool_call(
            Some("call_123".to_string()),
            "job_close",
            serde_json::json!({ "handle": "mira-kest" }),
        ));

        assert_eq!(
            strip_ansi(&rendered),
            "\n[tool:call]\njob_close #call_123\n{\n  \"handle\": \"mira-kest\"\n}"
        );
        assert!(!rendered.contains("status: unknown"));
    }

    #[test]
    fn formats_reasoning_separately_from_assistant_text() {
        let rendered = pretty_message(&PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
            id: None,
            content: vec![PhiReasoningContent::Summary("thinking".to_string())],
        }));

        assert_eq!(strip_ansi(&rendered), "\n[assistant:reasoning]\nthinking");
    }

    #[test]
    fn trims_message_boundary_whitespace_for_display_only() {
        let rendered = pretty_message(&PhiMessage::assistant("\n  answer\n\n"));

        assert_eq!(strip_ansi(&rendered), "\n[assistant]\nanswer");
    }

    #[test]
    fn condenses_shell_like_tool_results() {
        let rendered = pretty_tool_result_event(
            "bash",
            "call_123",
            &ToolCallOutput::success(serde_json::json!({
                "status": 0,
                "timed_out": false,
                "duration_ms": 12,
                "stdout": "hello\n",
                "stderr": ""
            })),
        );

        assert_eq!(
            strip_ansi(&rendered),
            "\n[tool:result]\nbash call_123\nok: true\nvalue:\nstatus: 0\nduration_ms: 12\nstdout:\nhello"
        );
    }

    #[test]
    fn condenses_job_tool_results() {
        let rendered = pretty_tool_result_event(
            "bash_job",
            "call_123",
            &ToolCallOutput::success(serde_json::json!({
                "status": "running",
                "exit_code": null,
                "handle": "mira-kest",
                "waited_ms": 1500,
                "output": "ready\r\n"
            })),
        );

        assert_eq!(
            strip_ansi(&rendered),
            "\n[tool:result]\nbash_job call_123\nok: true\nvalue:\nstatus: \"running\"\nwaited_ms: 1500\nhandle: mira-kest\noutput:\nready"
        );
    }

    #[test]
    fn shows_empty_job_output_field() {
        let rendered = pretty_tool_result_event(
            "job_interact",
            "call_123",
            &ToolCallOutput::success(serde_json::json!({
                "status": "running",
                "handle": "mira-kest",
                "waited_ms": 60000,
                "output": ""
            })),
        );

        assert_eq!(
            strip_ansi(&rendered),
            "\n[tool:result]\njob_interact call_123\nok: true\nvalue:\nstatus: \"running\"\nwaited_ms: 60000\nhandle: mira-kest\noutput: \"\""
        );
    }

    #[test]
    fn formats_runtime_error_as_red_transcript_block_without_color_codes() {
        let rendered = pretty_runtime_error(&PhiRuntimeError::tool_not_found(
            "assistant requested unknown tool: no_exist",
            crate::executor::ToolCallRequest {
                id: "call_missing".to_string(),
                call_id: Some("call_missing".to_string()),
                name: "no_exist".to_string(),
                arguments: serde_json::json!({ "name": "phi" }),
            },
        ));
        assert!(rendered.contains("[error]"));
        assert!(rendered.contains("assistant requested unknown tool: no_exist"));
    }

    #[test]
    fn formats_warning_as_yellow_transcript_block_without_color_codes() {
        let rendered = pretty_warning("resuming from failed step by requesting the provider again");
        assert!(rendered.contains("[warning]"));
        assert!(rendered.contains("resuming from failed step by requesting the provider again"));
    }

    #[test]
    fn formats_info_as_transcript_block_without_color_codes() {
        let rendered = pretty_info("resuming from existing session file: session");
        assert!(rendered.contains("[info]"));
        assert!(rendered.contains("resuming from existing session file: session"));
    }

    #[test]
    fn formats_history_as_stdout_transcript_blocks() {
        let history = PhiHistory::from_messages(vec![
            PhiMessage::user("hello"),
            PhiMessage::tool_call(
                Some("call_123".to_string()),
                "bash",
                serde_json::json!({ "command": "pwd" }),
            ),
            PhiMessage::tool_result(
                Some("call_123".to_string()),
                Some("bash".to_string()),
                serde_json::to_value(ToolCallOutput::success(serde_json::json!({
                    "status": 0,
                    "stdout": "ok\n",
                })))
                .expect("tool output should serialize"),
            ),
            PhiMessage::assistant("done"),
        ]);

        assert_eq!(
            strip_ansi(&pretty_history(&history)),
            "\n[user]\nhello\n\n[tool:call]\nbash #call_123\n{\n  \"command\": \"pwd\"\n}\n\n[tool:result]\nbash call_123\nok: true\nvalue:\nstatus: 0\nstdout:\nok\n\n[assistant]\ndone"
        );
    }
}
