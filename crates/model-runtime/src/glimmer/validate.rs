//! Fail-closed validation for Glimmer prompt history and content.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::prompt::{
    PromptContent, PromptMessage, PromptRenderRequest, PromptToolCall, PromptToolDefinition,
};

use super::{ContentRole, GlimmerRenderError};

const MAX_MESSAGES: usize = 1_024;
const MAX_TOOLS: usize = 64;
const MAX_CONTENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;

pub(super) fn validate_request(request: &PromptRenderRequest) -> Result<(), GlimmerRenderError> {
    if request.messages.is_empty() || request.messages.len() > MAX_MESSAGES {
        return Err(GlimmerRenderError::InvalidMessageCount);
    }
    if request.tools.len() > MAX_TOOLS {
        return Err(GlimmerRenderError::TooManyTools);
    }
    validate_date(&request.current_date)?;
    validate_tools(&request.tools)?;
    validate_historical_tool_names(&request.historical_tool_names)?;

    let mut context = ValidationContext::new(&request.tools, &request.historical_tool_names);
    for (index, message) in request.messages.iter().enumerate() {
        context.validate_message(index, message)?;
    }
    context.finish(request.add_generation_prompt)
}

struct ValidationContext<'a> {
    tools: BTreeSet<&'a str>,
    pending: BTreeMap<&'a str, &'a str>,
    seen_calls: BTreeSet<&'a str>,
    previous: Option<MessageKind>,
    total_bytes: usize,
}

impl<'a> ValidationContext<'a> {
    fn new(tools: &'a [PromptToolDefinition], historical_tool_names: &'a [String]) -> Self {
        Self {
            tools: tools
                .iter()
                .map(|tool| tool.name.as_str())
                .chain(historical_tool_names.iter().map(String::as_str))
                .collect(),
            pending: BTreeMap::new(),
            seen_calls: BTreeSet::new(),
            previous: None,
            total_bytes: 0,
        }
    }

    fn validate_message(
        &mut self,
        index: usize,
        message: &'a PromptMessage,
    ) -> Result<(), GlimmerRenderError> {
        match message {
            PromptMessage::System { content } => self.validate_system(index, content)?,
            PromptMessage::User { content } => self.validate_user(index, content)?,
            PromptMessage::Assistant {
                reasoning,
                content,
                tool_calls,
            } => self.validate_assistant(index, reasoning.as_deref(), content, tool_calls)?,
            PromptMessage::Tool { call_id, content } => {
                self.validate_tool_result(index, call_id, content)?;
            }
        }
        if self.total_bytes > MAX_CONTENT_BYTES {
            return Err(GlimmerRenderError::InputTooLarge);
        }
        Ok(())
    }

    fn validate_system(
        &mut self,
        index: usize,
        content: &[PromptContent],
    ) -> Result<(), GlimmerRenderError> {
        if index != 0 || self.previous.is_some() {
            return Err(GlimmerRenderError::InvalidSequence(index));
        }
        validate_parts(content, ContentRole::System, &mut self.total_bytes)?;
        self.previous = Some(MessageKind::System);
        Ok(())
    }

    fn validate_user(
        &mut self,
        index: usize,
        content: &[PromptContent],
    ) -> Result<(), GlimmerRenderError> {
        if !self.pending.is_empty()
            || !matches!(
                self.previous,
                None | Some(
                    MessageKind::System
                        | MessageKind::User
                        | MessageKind::Assistant
                        | MessageKind::Tool
                )
            )
        {
            return Err(GlimmerRenderError::InvalidSequence(index));
        }
        validate_parts(content, ContentRole::User, &mut self.total_bytes)?;
        self.previous = Some(MessageKind::User);
        Ok(())
    }

    fn validate_assistant(
        &mut self,
        index: usize,
        reasoning: Option<&str>,
        content: &[PromptContent],
        tool_calls: &'a [PromptToolCall],
    ) -> Result<(), GlimmerRenderError> {
        if !self.pending.is_empty()
            || !matches!(
                self.previous,
                Some(MessageKind::User | MessageKind::Tool | MessageKind::Assistant)
            )
        {
            return Err(GlimmerRenderError::InvalidSequence(index));
        }
        if let Some(reasoning) = reasoning {
            validate_nonempty_text(reasoning, &mut self.total_bytes)?;
        }
        if tool_calls.is_empty() {
            validate_parts(content, ContentRole::Assistant, &mut self.total_bytes)?;
        } else {
            if !content.is_empty() {
                validate_parts(content, ContentRole::Assistant, &mut self.total_bytes)?;
            }
            for call in tool_calls {
                self.validate_tool_call(call)?;
            }
        }
        self.previous = Some(MessageKind::Assistant);
        Ok(())
    }

    fn validate_tool_call(&mut self, call: &'a PromptToolCall) -> Result<(), GlimmerRenderError> {
        validate_call_id(&call.call_id)?;
        validate_recipient(&call.name)?;
        if !self.tools.contains(call.name.as_str()) {
            return Err(GlimmerRenderError::UnknownTool(call.name.clone()));
        }
        if !self.seen_calls.insert(&call.call_id) {
            return Err(GlimmerRenderError::DuplicateToolCall(call.call_id.clone()));
        }
        for (name, value) in &call.arguments {
            validate_parameter_name(name)?;
            validate_json(value)?;
        }
        self.total_bytes = self
            .total_bytes
            .checked_add(
                serde_json::to_vec(&call.arguments)
                    .map_err(GlimmerRenderError::Json)?
                    .len(),
            )
            .ok_or(GlimmerRenderError::InputTooLarge)?;
        self.pending.insert(&call.call_id, &call.name);
        Ok(())
    }

    fn validate_tool_result(
        &mut self,
        index: usize,
        call_id: &str,
        content: &[PromptContent],
    ) -> Result<(), GlimmerRenderError> {
        if !matches!(
            self.previous,
            Some(MessageKind::Assistant | MessageKind::Tool)
        ) {
            return Err(GlimmerRenderError::InvalidSequence(index));
        }
        validate_call_id(call_id)?;
        if self.pending.remove(call_id).is_none() {
            return Err(GlimmerRenderError::UnknownToolCall(call_id.to_owned()));
        }
        validate_parts(content, ContentRole::Tool, &mut self.total_bytes)?;
        self.previous = Some(MessageKind::Tool);
        Ok(())
    }

    fn finish(self, add_generation_prompt: bool) -> Result<(), GlimmerRenderError> {
        if !self.pending.is_empty() {
            return Err(GlimmerRenderError::UnresolvedToolCalls);
        }
        if add_generation_prompt
            && !matches!(self.previous, Some(MessageKind::User | MessageKind::Tool))
        {
            return Err(GlimmerRenderError::InvalidGenerationPoint);
        }
        Ok(())
    }
}

fn validate_tools(tools: &[PromptToolDefinition]) -> Result<(), GlimmerRenderError> {
    let mut names = BTreeSet::new();
    for tool in tools {
        validate_recipient(&tool.name)?;
        validate_safe_text(&tool.description)?;
        validate_json(&tool.parameters)?;
        if !tool.parameters.is_object() {
            return Err(GlimmerRenderError::InvalidToolSchema(tool.name.clone()));
        }
        if !names.insert(tool.name.as_str()) {
            return Err(GlimmerRenderError::DuplicateTool(tool.name.clone()));
        }
    }
    Ok(())
}

fn validate_historical_tool_names(names: &[String]) -> Result<(), GlimmerRenderError> {
    if names.len() > MAX_TOOLS {
        return Err(GlimmerRenderError::TooManyTools);
    }
    for name in names {
        validate_recipient(name)?;
    }
    Ok(())
}

fn validate_parts(
    parts: &[PromptContent],
    role: ContentRole,
    total_bytes: &mut usize,
) -> Result<(), GlimmerRenderError> {
    if parts.is_empty() {
        return Err(GlimmerRenderError::EmptyContent);
    }
    for part in parts {
        match part {
            PromptContent::Text(text) => validate_nonempty_text(text, total_bytes)?,
            PromptContent::Image if matches!(role, ContentRole::User | ContentRole::Tool) => {}
            PromptContent::Json(value) if role == ContentRole::Tool => {
                validate_json(value)?;
                *total_bytes = total_bytes
                    .checked_add(
                        serde_json::to_vec(value)
                            .map_err(GlimmerRenderError::Json)?
                            .len(),
                    )
                    .ok_or(GlimmerRenderError::InputTooLarge)?;
            }
            PromptContent::Image | PromptContent::Json(_) => {
                return Err(GlimmerRenderError::UnsupportedContent);
            }
        }
    }
    Ok(())
}

fn validate_nonempty_text(text: &str, total_bytes: &mut usize) -> Result<(), GlimmerRenderError> {
    if text.is_empty() {
        return Err(GlimmerRenderError::EmptyContent);
    }
    validate_safe_text(text)?;
    *total_bytes = total_bytes
        .checked_add(text.len())
        .ok_or(GlimmerRenderError::InputTooLarge)?;
    Ok(())
}

fn validate_json(value: &Value) -> Result<(), GlimmerRenderError> {
    match value {
        Value::String(value) => validate_safe_text(value),
        Value::Array(values) => values.iter().try_for_each(validate_json),
        Value::Object(values) => {
            for (key, value) in values {
                validate_safe_text(key)?;
                validate_json(value)?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn validate_safe_text(text: &str) -> Result<(), GlimmerRenderError> {
    if [
        "<|",
        "|>",
        "<atem:",
        "</atem:",
        "<tool_output",
        "</tool_output",
    ]
    .iter()
    .any(|marker| text.contains(marker))
    {
        return Err(GlimmerRenderError::ReservedDelimiter);
    }
    Ok(())
}

fn validate_recipient(value: &str) -> Result<(), GlimmerRenderError> {
    if !is_valid_recipient(value) {
        return Err(GlimmerRenderError::InvalidRecipient(value.to_owned()));
    }
    Ok(())
}

fn validate_parameter_name(value: &str) -> Result<(), GlimmerRenderError> {
    if !is_valid_parameter_name(value) {
        return Err(GlimmerRenderError::InvalidParameter(value.to_owned()));
    }
    Ok(())
}

pub(super) fn is_valid_recipient(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.split('.').all(valid_identifier_part)
}

pub(super) fn is_valid_parameter_name(value: &str) -> bool {
    value.len() <= MAX_IDENTIFIER_BYTES && valid_identifier_part(value)
}

fn valid_identifier_part(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn validate_call_id(value: &str) -> Result<(), GlimmerRenderError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(GlimmerRenderError::InvalidToolCallId);
    }
    Ok(())
}

fn validate_date(value: &str) -> Result<(), GlimmerRenderError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return Err(GlimmerRenderError::InvalidCurrentDate);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum MessageKind {
    System,
    User,
    Assistant,
    Tool,
}
