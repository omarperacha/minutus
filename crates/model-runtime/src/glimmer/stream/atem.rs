//! Parsing of complete Glimmer ATEM tool-call envelopes.

use std::collections::BTreeMap;

use serde_json::Value;

use super::GlimmerStreamError;
use crate::glimmer::validate::{is_valid_parameter_name, is_valid_recipient};

const OPEN: &str = "<atem:function_calls>";
const INVOKE_PREFIX: &str = "<atem:invoke name=\"";
const INVOKE_CLOSE: &str = "</atem:invoke>";
const PARAMETER_PREFIX: &str = "<atem:parameter name=\"";
const PARAMETER_CLOSE: &str = "</atem:parameter>";
const CLOSE: &str = "</atem:function_calls>";

pub(super) fn parse_atem(
    payload: &str,
) -> Result<(String, BTreeMap<String, Value>), GlimmerStreamError> {
    let mut cursor = AtemCursor::new(payload);
    cursor.consume(OPEN)?;
    cursor.consume_line_breaks();
    cursor.consume(INVOKE_PREFIX)?;
    let name = cursor.take_until("\">")?.to_owned();
    if !is_valid_recipient(&name) {
        return Err(GlimmerStreamError::MalformedToolCall);
    }
    cursor.consume("\">")?;
    cursor.consume_line_breaks();

    let mut arguments = BTreeMap::new();
    while !cursor.remaining().starts_with(INVOKE_CLOSE) {
        cursor.consume(PARAMETER_PREFIX)?;
        let parameter = cursor.take_until("\">")?.to_owned();
        if !is_valid_parameter_name(&parameter) {
            return Err(GlimmerStreamError::MalformedToolCall);
        }
        cursor.consume("\">")?;
        let raw_value = cursor.take_until(PARAMETER_CLOSE)?;
        let value =
            serde_json::from_str(raw_value).unwrap_or_else(|_| Value::String(raw_value.to_owned()));
        if arguments.insert(parameter, value).is_some() {
            return Err(GlimmerStreamError::DuplicateParameter);
        }
        cursor.consume(PARAMETER_CLOSE)?;
        cursor.consume_line_breaks();
    }
    cursor.consume(INVOKE_CLOSE)?;
    cursor.consume_line_breaks();
    cursor.consume(CLOSE)?;
    cursor.consume_ascii_whitespace();
    if !cursor.remaining().is_empty() {
        return Err(GlimmerStreamError::MalformedToolCall);
    }
    Ok((name, arguments))
}

struct AtemCursor<'a> {
    remaining: &'a str,
}

impl<'a> AtemCursor<'a> {
    const fn new(input: &'a str) -> Self {
        Self { remaining: input }
    }

    const fn remaining(&self) -> &'a str {
        self.remaining
    }

    fn consume(&mut self, expected: &str) -> Result<(), GlimmerStreamError> {
        self.remaining = self
            .remaining
            .strip_prefix(expected)
            .ok_or(GlimmerStreamError::MalformedToolCall)?;
        Ok(())
    }

    fn take_until(&mut self, delimiter: &str) -> Result<&'a str, GlimmerStreamError> {
        let index = self
            .remaining
            .find(delimiter)
            .ok_or(GlimmerStreamError::MalformedToolCall)?;
        let value = &self.remaining[..index];
        self.remaining = &self.remaining[index..];
        Ok(value)
    }

    fn consume_line_breaks(&mut self) {
        while let Some(remaining) = self
            .remaining
            .strip_prefix("\r\n")
            .or_else(|| self.remaining.strip_prefix('\n'))
        {
            self.remaining = remaining;
        }
    }

    fn consume_ascii_whitespace(&mut self) {
        self.remaining = self
            .remaining
            .trim_start_matches(|character: char| character.is_ascii_whitespace());
    }
}
