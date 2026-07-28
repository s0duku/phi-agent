use minijinja::{Environment, context};
use roxmltree::{Document, Node};

use crate::{
    error::{PhiRuntimeError, PhiRuntimeResult},
    home::PhiHome,
    message::{PhiAssistantMessage, PhiMessage, PhiReasoningContent},
};

use super::{PhiProviderCall, PhiRenderedMessages};

pub(super) fn render_template(
    home: &dyn PhiHome,
    template: Option<&str>,
    request: &PhiProviderCall,
    messages: &PhiRenderedMessages,
) -> PhiRuntimeResult<PhiRenderedMessages> {
    let Some(template) = template.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(messages.clone());
    };

    let history = messages.to_history();
    let source = home.read_template(template)?;
    let mut environment = Environment::new();
    environment.add_template("phi", &source).map_err(|error| {
        PhiRuntimeError::session(format!("failed to load template {template}: {error}"))
    })?;
    let rendered = environment
        .get_template("phi")
        .map_err(|error| {
            PhiRuntimeError::session(format!("failed to resolve template {template}: {error}"))
        })?
        .render(context! {
            messages => history,
            request => request,
        })
        .map_err(|error| {
            PhiRuntimeError::session(format!("failed to render template {template}: {error}"))
        })?;

    parse_rendered_messages(template, &rendered)
        .map(|messages| PhiRenderedMessages::from_history(messages.into()))
}

fn parse_rendered_messages(template: &str, rendered: &str) -> PhiRuntimeResult<Vec<PhiMessage>> {
    let rendered = rendered.trim();
    if rendered.is_empty() {
        return Err(PhiRuntimeError::session(format!(
            "template {template} rendered an empty message payload"
        )));
    }

    parse_phi_dsl_messages(template, rendered).or_else(|dsl_error| {
        parse_json_messages(template, rendered).map_err(|json_error| {
            PhiRuntimeError::session(format!(
                "template {template} must render Phi DSL or PhiMessage JSON; dsl error: {}; json error: {}",
                dsl_error.detail(),
                json_error.detail(),
            ))
        })
    })
}

fn parse_phi_dsl_messages(template: &str, rendered: &str) -> PhiRuntimeResult<Vec<PhiMessage>> {
    let wrapped = format!("<phi>{rendered}</phi>");
    let document = Document::parse(&wrapped).map_err(|error| {
        PhiRuntimeError::session(format!(
            "template {template} produced invalid phi dsl/xml: {error}"
        ))
    })?;
    let root = document.root_element();
    let mut messages = Vec::new();

    for child in root.children().filter(|node| node.is_element()) {
        messages.push(parse_phi_dsl_message(template, child)?);
    }

    if messages.is_empty() {
        return Err(PhiRuntimeError::session(format!(
            "template {template} did not produce any phi dsl message tags"
        )));
    }

    Ok(messages)
}

fn parse_phi_dsl_message(template: &str, node: Node<'_, '_>) -> PhiRuntimeResult<PhiMessage> {
    let body = collect_node_text(node);
    match node.tag_name().name() {
        "system" => Ok(PhiMessage::system(body)),
        "user" => Ok(PhiMessage::user(body)),
        "assistant" => Ok(PhiMessage::Assistant(PhiAssistantMessage::text(body))),
        "reasoning" => Ok(PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
            id: optional_attr(node, "id"),
            content: vec![parse_reasoning_content(node, body)],
        })),
        "tool_call" => {
            let Some(name) = required_attr(node, "name")? else {
                unreachable!("required_attr returns Err when missing")
            };
            let arguments = parse_json_body(template, node, &body, "tool_call arguments")?;
            Ok(PhiMessage::tool_call(
                optional_attr(node, "id"),
                name,
                arguments,
            ))
        }
        "tool_result" => {
            let result = parse_json_body(template, node, &body, "tool_result payload")?;
            Ok(PhiMessage::tool_result(
                optional_attr(node, "id"),
                optional_attr(node, "name"),
                result,
            ))
        }
        other => Err(PhiRuntimeError::session(format!(
            "template {template} produced unsupported phi dsl tag <{other}>"
        ))),
    }
}

fn parse_reasoning_content(node: Node<'_, '_>, body: String) -> PhiReasoningContent {
    match node.attribute("type").unwrap_or("summary") {
        "text" => PhiReasoningContent::Text {
            text: body,
            signature: optional_attr(node, "signature"),
        },
        "redacted" => PhiReasoningContent::Redacted { data: body },
        "encrypted" => PhiReasoningContent::Encrypted(body),
        _ => PhiReasoningContent::Summary(body),
    }
}

fn parse_json_body(
    template: &str,
    node: Node<'_, '_>,
    body: &str,
    label: &str,
) -> PhiRuntimeResult<serde_json::Value> {
    serde_json::from_str(body.trim()).map_err(|error| {
        PhiRuntimeError::session(format!(
            "template {template} tag <{}> must contain valid JSON for {label}: {error}",
            node.tag_name().name()
        ))
    })
}

fn parse_json_messages(template: &str, rendered: &str) -> PhiRuntimeResult<Vec<PhiMessage>> {
    serde_json::from_str::<Vec<PhiMessage>>(rendered)
        .or_else(|_| serde_json::from_str::<PhiMessage>(rendered).map(|message| vec![message]))
        .map_err(|error| {
            PhiRuntimeError::session(format!(
                "template {template} must render PhiMessage JSON or a JSON array of PhiMessage values: {error}"
            ))
        })
}

fn collect_node_text(node: Node<'_, '_>) -> String {
    let text = node
        .children()
        .filter_map(|child| child.text())
        .collect::<String>();
    text.trim().to_string()
}

fn optional_attr(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.attribute(name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn required_attr(node: Node<'_, '_>, name: &str) -> PhiRuntimeResult<Option<String>> {
    let value = optional_attr(node, name);
    if value.is_none() {
        return Err(PhiRuntimeError::session(format!(
            "phi dsl tag <{}> requires attribute {name}",
            node.tag_name().name()
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::parse_rendered_messages;
    use crate::message::{PhiAssistantMessage, PhiMessage, PhiReasoningContent, PhiToolMessage};

    #[test]
    fn parse_phi_dsl_supports_basic_roles_and_tool_blocks() {
        let messages = parse_rendered_messages(
            "test",
            r#"
            <system>sys</system>
            <user>usr</user>
            <assistant>ast</assistant>
            <reasoning id="r1" type="text" signature="sig">think</reasoning>
            <tool_call id="call_1" name="bash">{"command":"ls"}</tool_call>
            <tool_result id="call_1" name="bash">{"ok":true}</tool_result>
            "#,
        )
        .expect("dsl should parse");

        assert_eq!(messages[0], PhiMessage::system("sys"));
        assert_eq!(messages[1], PhiMessage::user("usr"));
        assert_eq!(messages[2], PhiMessage::assistant("ast"));
        assert_eq!(
            messages[3],
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                id: Some("r1".to_string()),
                content: vec![PhiReasoningContent::Text {
                    text: "think".to_string(),
                    signature: Some("sig".to_string()),
                }],
            })
        );
        assert_eq!(
            messages[4],
            PhiMessage::Tool(PhiToolMessage::ToolCall {
                id: Some("call_1".to_string()),
                name: "bash".to_string(),
                arguments: serde_json::json!({ "command": "ls" }),
            })
        );
        assert_eq!(
            messages[5],
            PhiMessage::Tool(PhiToolMessage::ToolResult {
                id: Some("call_1".to_string()),
                name: Some("bash".to_string()),
                result: serde_json::json!({ "ok": true }),
            })
        );
    }
}
