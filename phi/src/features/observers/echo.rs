use std::io::{self, IsTerminal, Write};

use owo_colors::{OwoColorize, Stream, Style};
use serde::Serialize;

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
    pretty_json_value(
        &serde_json::to_value(output).expect("ToolCallOutput must serialize as structured JSON"),
    )
}

fn pretty_json_value(value: &serde_json::Value) -> String {
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    value
        .serialize(&mut serializer)
        .ok()
        .and_then(|()| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| value.to_string())
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
            "\n[tool:call]\nbash #call_123\n{\n\t\"command\": \"pwd\"\n}"
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
            "\n[tool:call]\njob_close #call_123\n{\n\t\"handle\": \"mira-kest\"\n}"
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
    fn renders_every_shell_tool_result_field() {
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
            "\n[tool:result]\nbash call_123\n{\n\t\"tool_error\": null,\n\t\"tool_ok\": true,\n\t\"value\": {\n\t\t\"duration_ms\": 12,\n\t\t\"status\": 0,\n\t\t\"stderr\": \"\",\n\t\t\"stdout\": \"hello\\n\",\n\t\t\"timed_out\": false\n\t}\n}"
        );
    }

    #[test]
    fn renders_every_job_tool_result_field() {
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
            "\n[tool:result]\nbash_job call_123\n{\n\t\"tool_error\": null,\n\t\"tool_ok\": true,\n\t\"value\": {\n\t\t\"exit_code\": null,\n\t\t\"handle\": \"mira-kest\",\n\t\t\"output\": \"ready\\r\\n\",\n\t\t\"status\": \"running\",\n\t\t\"waited_ms\": 1500\n\t}\n}"
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
            "\n[tool:result]\njob_interact call_123\n{\n\t\"tool_error\": null,\n\t\"tool_ok\": true,\n\t\"value\": {\n\t\t\"handle\": \"mira-kest\",\n\t\t\"output\": \"\",\n\t\t\"status\": \"running\",\n\t\t\"waited_ms\": 60000\n\t}\n}"
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
            "\n[user]\nhello\n\n[tool:call]\nbash #call_123\n{\n\t\"command\": \"pwd\"\n}\n\n[tool:result]\nbash call_123\n{\n\t\"tool_error\": null,\n\t\"tool_ok\": true,\n\t\"value\": {\n\t\t\"status\": 0,\n\t\t\"stdout\": \"ok\\n\"\n\t}\n}\n\n[assistant]\ndone"
        );
    }
}
