//! Glimmer turn, recipient, multimodal and ATEM serialization.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, ser::Formatter};

use crate::prompt::{
    PromptContent, PromptMessage, PromptRenderRequest, PromptToolCall, PromptToolDefinition,
};

use super::{ContentRole, GlimmerRenderError, validate::validate_request};

const BOS: &str = "<|begin_of_text|>";
const START: &str = "<|start|>";
const MESSAGE: &str = "<|message|>";
const EOT: &str = "<|eot|>";
const EOM: &str = "<|eom|>";
const PATCH: &str = "<|patch|>";
const DEFAULT_SYSTEM: &str = "You are a helpful AI assistant.";
const KNOWLEDGE_CUTOFF: &str = "2026-01-04";
const MAX_RENDERED_BYTES: usize = 8 * 1024 * 1024;

/// Renders validated model-neutral messages into Glimmer's native prompt.
///
/// # Errors
///
/// Returns a typed error for invalid role order, unmatched tool results,
/// unsupported content, unsafe protocol delimiters, invalid identifiers or
/// excessive input/output.
pub(crate) fn render_glimmer_prompt(
    request: &PromptRenderRequest,
) -> Result<String, GlimmerRenderError> {
    validate_request(request)?;
    let mut rendered = String::with_capacity(4_096);
    rendered.push_str(BOS);

    let has_system = matches!(request.messages.first(), Some(PromptMessage::System { .. }));
    if !has_system {
        render_system(
            &mut rendered,
            DEFAULT_SYSTEM,
            request,
            request.tools.iter(),
            true,
        )?;
    }

    let mut calls = BTreeMap::<&str, &str>::new();
    for message in &request.messages {
        match message {
            PromptMessage::System { content } => {
                let text = render_parts(content, ContentRole::System)?;
                render_system(&mut rendered, &text, request, request.tools.iter(), false)?;
            }
            PromptMessage::User { content } => render_user(&mut rendered, content)?,
            PromptMessage::Assistant {
                reasoning,
                content,
                tool_calls,
            } => render_assistant(
                &mut rendered,
                reasoning.as_deref(),
                content,
                tool_calls,
                &mut calls,
            )?,
            PromptMessage::Tool { call_id, content } => {
                render_tool_result(&mut rendered, call_id, content, &calls)?;
            }
        }
        enforce_output_limit(&rendered)?;
    }
    if request.add_generation_prompt {
        rendered.push_str(START);
        rendered.push_str("assistant");
    }
    enforce_output_limit(&rendered)?;
    Ok(rendered)
}

fn render_user(output: &mut String, content: &[PromptContent]) -> Result<(), GlimmerRenderError> {
    output.push_str(START);
    output.push_str("user");
    output.push_str(MESSAGE);
    output.push_str(&render_parts(content, ContentRole::User)?);
    output.push_str(EOT);
    Ok(())
}

fn render_assistant<'a>(
    output: &mut String,
    reasoning: Option<&str>,
    content: &[PromptContent],
    tool_calls: &'a [PromptToolCall],
    calls: &mut BTreeMap<&'a str, &'a str>,
) -> Result<(), GlimmerRenderError> {
    if let Some(reasoning) = reasoning {
        output.push_str(START);
        output.push_str("assistant to=self");
        output.push_str(MESSAGE);
        output.push_str(reasoning);
        output.push_str(EOM);
    }
    if tool_calls.is_empty() {
        output.push_str(START);
        output.push_str("assistant to=user");
        output.push_str(MESSAGE);
        output.push_str(&render_parts(content, ContentRole::Assistant)?);
        output.push_str(EOT);
        return Ok(());
    }
    if !content.is_empty() {
        // Visible preamble belongs to the same assistant turn as these calls.
        output.push_str(START);
        output.push_str("assistant to=user");
        output.push_str(MESSAGE);
        output.push_str(&render_parts(content, ContentRole::Assistant)?);
        output.push_str(EOM);
    }
    for (index, call) in tool_calls.iter().enumerate() {
        calls.insert(&call.call_id, &call.name);
        output.push_str(START);
        output.push_str("assistant to=");
        output.push_str(&call.name);
        output.push_str(MESSAGE);
        render_atem_call(output, call)?;
        output.push_str(if index + 1 == tool_calls.len() {
            EOT
        } else {
            EOM
        });
    }
    Ok(())
}

fn render_tool_result(
    output: &mut String,
    call_id: &str,
    content: &[PromptContent],
    calls: &BTreeMap<&str, &str>,
) -> Result<(), GlimmerRenderError> {
    let name = calls
        .get(call_id)
        .copied()
        .ok_or_else(|| GlimmerRenderError::UnknownToolCall(call_id.to_owned()))?;
    output.push_str(START);
    output.push_str("tool ");
    output.push_str(name);
    output.push_str(MESSAGE);
    output.push_str("<tool_output name=\"");
    output.push_str(name);
    output.push_str("\" trust=\"untrusted\">\n");
    output.push_str(&render_parts(content, ContentRole::Tool)?);
    output.push_str("\n</tool_output>");
    output.push_str(EOT);
    Ok(())
}

fn render_system<'a>(
    output: &mut String,
    content: &str,
    request: &PromptRenderRequest,
    tools: impl Iterator<Item = &'a PromptToolDefinition>,
    include_default_metadata: bool,
) -> Result<(), GlimmerRenderError> {
    let content = normalize_reasoning_directive(content);
    output.push_str(START);
    output.push_str("system");
    output.push_str(MESSAGE);
    output.push_str(&content);
    if include_default_metadata {
        output.push_str("\nKnowledge cutoff: ");
        output.push_str(KNOWLEDGE_CUTOFF);
        output.push_str(".\nCurrent date: ");
        output.push_str(&request.current_date);
        output.push('.');
    }
    if !content.to_ascii_lowercase().contains("reasoning strength") {
        output.push_str("\n\nReasoning strength: ");
        // Glimmer has no hard thinking switch; use its supported low directive.
        output.push_str(match request.reasoning_strength {
            crate::ReasoningStrength::Disabled => "low",
            strength => strength.as_str(),
        });
        output.push('.');
    }
    if !request.tools.is_empty() {
        output.push_str("\n\n");
        render_tool_definitions(output, tools)?;
    }
    output.push_str("\n\n# Valid recipients: \"self\"");
    for namespace in tool_namespaces(&request.tools) {
        output.push_str(", \"");
        output.push_str(namespace);
        output.push_str(".*\"");
    }
    output.push_str(", \"user\".");
    output.push_str(EOT);
    Ok(())
}

fn normalize_reasoning_directive(content: &str) -> String {
    content
        .replace("Reasoning effort", "Reasoning strength")
        .replace("Reasoning Effort", "Reasoning Strength")
        .replace("reasoning effort", "reasoning strength")
        .replace("REASONING EFFORT", "REASONING STRENGTH")
}

fn render_tool_definitions<'a>(
    output: &mut String,
    tools: impl Iterator<Item = &'a PromptToolDefinition>,
) -> Result<(), GlimmerRenderError> {
    output.push_str("In this environment you have access to a set of tools you can use to answer the user's question.\n\n");
    output.push_str("You can invoke a function by writing a \"<atem:function_calls>\" block like the following:\n");
    output.push_str("<atem:function_calls>\n<atem:invoke name=\"$FUNCTION_NAME\">\n<atem:parameter name=\"$PARAMETER_NAME\">$PARAMETER_VALUE</atem:parameter>\n...\n</atem:invoke>\n</atem:function_calls>\n\n");
    output.push_str("String and scalar parameters should be specified as is, while lists and objects should use JSON format. Note that spaces for string values are not stripped. The output is not expected to be valid XML and is parsed with regular expressions.\n");
    output.push_str("Here are the functions available in JSONSchema format:\n// Tool metadata\n");
    let tools = tools.collect::<Vec<_>>();
    for namespace in tool_namespaces_from_iter(tools.iter().copied()) {
        output.push_str("{\"name\": ");
        output.push_str(&jinja_json(namespace)?);
        output.push_str(", \"description\": \"\"}\n");
    }
    output.push_str("// Function schemas");
    for tool in tools {
        output.push_str("\n{\"name\": ");
        output.push_str(&jinja_json(&tool.name)?);
        output.push_str(", \"description\": ");
        output.push_str(&jinja_json(&tool.description)?);
        output.push_str(", \"parameters\": ");
        output.push_str(&jinja_json(&tool.parameters)?);
        output.push('}');
    }
    output.push_str("\n\nHere's an example of how to call a function in the tool set:\n");
    output.push_str("(If the tool namespace is not specified, invoke the function directly as `example_function_name` rather than `example_tool_name.example_function_name`)\n\n");
    output.push_str("to=example_tool_name.example_function_name\n\n");
    output.push_str(
        "<atem:function_calls>\n<atem:invoke name=\"example_tool_name.example_function_name\">\n",
    );
    output.push_str("<atem:parameter name=\"example_parameter_1\">value_1</atem:parameter>\n");
    output.push_str("<atem:parameter name=\"example_parameter_2\">This is the value for the second parameter\nthat can span\n\"multiple\" lines\n</atem:parameter>\n");
    output.push_str("</atem:invoke>\n</atem:function_calls>");
    Ok(())
}

fn render_atem_call(output: &mut String, call: &PromptToolCall) -> Result<(), GlimmerRenderError> {
    output.push_str("<atem:function_calls>\n<atem:invoke name=\"");
    output.push_str(&call.name);
    output.push_str("\">\n");
    for (name, value) in &call.arguments {
        output.push_str("<atem:parameter name=\"");
        output.push_str(name);
        output.push_str("\">");
        match value {
            Value::String(value) => output.push_str(value),
            _ => output.push_str(&jinja_json(value)?),
        }
        output.push_str("</atem:parameter>\n");
    }
    output.push_str("</atem:invoke>\n</atem:function_calls>");
    Ok(())
}

fn render_parts(parts: &[PromptContent], role: ContentRole) -> Result<String, GlimmerRenderError> {
    let mut output = String::new();
    for part in parts {
        match part {
            PromptContent::Text(text) => output.push_str(text),
            PromptContent::Image if matches!(role, ContentRole::User | ContentRole::Tool) => {
                output.push_str(PATCH);
            }
            PromptContent::Json(value) if role == ContentRole::Tool => {
                output.push_str(&jinja_json(value)?);
            }
            PromptContent::Image | PromptContent::Json(_) => {
                return Err(GlimmerRenderError::UnsupportedContent);
            }
        }
    }
    Ok(output)
}

fn tool_namespaces(tools: &[PromptToolDefinition]) -> Vec<&str> {
    tool_namespaces_from_iter(tools.iter())
}

fn tool_namespaces_from_iter<'a>(
    tools: impl Iterator<Item = &'a PromptToolDefinition>,
) -> Vec<&'a str> {
    let mut namespaces = Vec::new();
    for tool in tools {
        let namespace = tool.name.split('.').next().unwrap_or(tool.name.as_str());
        if !namespaces.contains(&namespace) {
            namespaces.push(namespace);
        }
    }
    namespaces
}

fn jinja_json<T: Serialize + ?Sized>(value: &T) -> Result<String, GlimmerRenderError> {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, SpacedFormatter);
    value
        .serialize(&mut serializer)
        .map_err(GlimmerRenderError::Json)?;
    String::from_utf8(bytes).map_err(|_| GlimmerRenderError::OutputTooLarge)
}

fn enforce_output_limit(output: &str) -> Result<(), GlimmerRenderError> {
    if output.len() > MAX_RENDERED_BYTES {
        Err(GlimmerRenderError::OutputTooLarge)
    } else {
        Ok(())
    }
}

struct SpacedFormatter;

impl Formatter for SpacedFormatter {
    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        writer.write_all(b": ")
    }
}
