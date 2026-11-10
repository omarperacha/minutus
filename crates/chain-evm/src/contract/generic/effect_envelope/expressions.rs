use super::{Analysis, GenericError, syntax};
use alloy_primitives::{Address, U256};
use serde_json::{Value, json};

impl Analysis<'_> {
    pub(super) fn value(&self, node: &Value, depth: usize) -> Result<Value, GenericError> {
        self.step()?;
        if depth > 16 {
            return Err(GenericError::Limit);
        }
        match node["nodeType"].as_str() {
            Some("Literal") => literal(node),
            Some("Identifier") => {
                if node["name"] == "this" && syntax::reference(node).is_ok_and(|id| id < 0) {
                    return Ok(json!(self.target));
                }
                let id = syntax::reference(node)?;
                if let Some(value) = self.values.get(&id) {
                    return Ok(value.clone());
                }
                let declaration = self.program.node(id)?;
                if declaration["constant"] == true {
                    self.value(&declaration["value"], depth + 1)
                } else {
                    Err(GenericError::OpaqueEffects)
                }
            }
            Some("MemberAccess") => self.member_value(node, depth + 1),
            Some("IndexAccess") => {
                let array = self.value(&node["baseExpression"], depth + 1)?;
                let index = self.value(&node["indexExpression"], depth + 1)?;
                let index = index
                    .as_str()
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|v| *v < 32)
                    .ok_or(GenericError::OpaqueEffects)?;
                array
                    .as_array()
                    .and_then(|v| v.get(index))
                    .cloned()
                    .ok_or(GenericError::OpaqueEffects)
            }
            Some("TupleExpression") => {
                let values = node["components"]
                    .as_array()
                    .filter(|v| v.len() <= 32)
                    .ok_or(GenericError::OpaqueEffects)?;
                values
                    .iter()
                    .map(|value| self.value(value, depth + 1))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
            Some("UnaryOperation") => unary(node, &self.value(&node["subExpression"], depth + 1)?),
            Some("BinaryOperation") => binary(
                node["operator"]
                    .as_str()
                    .ok_or(GenericError::OpaqueEffects)?,
                &self.value(&node["leftExpression"], depth + 1)?,
                &self.value(&node["rightExpression"], depth + 1)?,
                node["commonType"]["typeString"].as_str(),
            ),
            Some("FunctionCall") if node["kind"] == "typeConversion" => {
                self.cast_value(node, depth + 1)
            }
            Some("FunctionCall") if node["kind"] == "functionCall" => {
                self.pure_helper(node, depth + 1)
            }
            _ => Err(GenericError::OpaqueEffects),
        }
    }
    fn cast_value(&self, node: &Value, depth: usize) -> Result<Value, GenericError> {
        let arguments = node["arguments"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or(GenericError::OpaqueEffects)?;
        let value = self.value(&arguments[0], depth + 1)?;
        let ty = base_type(node)?;
        let source = base_type(&arguments[0])?;
        // Bytes/string casts change representation, not just a type label.
        // Reject them until explicitly implemented instead of interpreting a
        // string such as "0x0102" as its two-byte hexadecimal spelling.
        if matches!(ty, "bytes" | "string") && source != ty {
            return Err(GenericError::OpaqueEffects);
        }
        canonical(ty, &value)?;
        Ok(value)
    }
    fn pure_helper(&self, call: &Value, depth: usize) -> Result<Value, GenericError> {
        if !self.modern
            || call["expression"]["nodeType"] != "Identifier"
            || call["names"]
                .as_array()
                .is_some_and(|names| !names.is_empty())
        {
            return Err(GenericError::OpaqueEffects);
        }
        let function = self
            .program
            .helper(syntax::reference(&call["expression"])?)?;
        let id = syntax::id(function)?;
        if function["stateMutability"] != "pure"
            || self.stack.contains(&id)
            || self.stack.len() >= 8
            || function["modifiers"]
                .as_array()
                .is_none_or(|v| !v.is_empty())
        {
            return Err(GenericError::OpaqueEffects);
        }
        let statements = function["body"]["statements"]
            .as_array()
            .filter(|v| v.len() == 1 && v[0]["nodeType"] == "Return")
            .ok_or(GenericError::OpaqueEffects)?;
        let parameters = function["parameters"]["parameters"]
            .as_array()
            .ok_or(GenericError::OpaqueEffects)?;
        let arguments = call["arguments"]
            .as_array()
            .filter(|v| v.len() <= 16 && v.len() == parameters.len())
            .ok_or(GenericError::OpaqueEffects)?;
        let mut values = self.values.clone();
        for (parameter, argument) in parameters.iter().zip(arguments) {
            values.insert(syntax::id(parameter)?, self.value(argument, depth + 1)?);
        }
        let mut stack = self.stack.clone();
        stack.push(id);
        let analysis = Analysis {
            program: self.program,
            layout: self.layout,
            values,
            relations: Vec::new(),
            steps: std::cell::Cell::new(self.steps.get()),
            modern: self.modern,
            account: self.account,
            target: self.target,
            native_value: self.native_value,
            terminated: false,
            stack,
        };
        let result = analysis.value(&statements[0]["expression"], depth + 1);
        self.steps.set(analysis.steps.get());
        result
    }
    fn member_value(&self, node: &Value, depth: usize) -> Result<Value, GenericError> {
        let base = &node["expression"];
        let member = node["memberName"]
            .as_str()
            .ok_or(GenericError::OpaqueEffects)?;
        let builtin =
            base["nodeType"] == "Identifier" && syntax::reference(base).is_ok_and(|id| id < 0);
        match (base["name"].as_str(), member, builtin) {
            (Some("msg"), "sender", true) | (Some("tx"), "origin", true) => Ok(json!(self.account)),
            (Some("msg"), "value", true) => Ok(json!(self.native_value)),
            _ => {
                let value = self.value(base, depth + 1)?;
                if member == "length" {
                    let ty = base_type(base)?;
                    if ty == "bytes"
                        || ty
                            .strip_prefix("bytes")
                            .is_some_and(|size| size.parse::<usize>().is_ok())
                    {
                        canonical(ty, &value)?;
                        let data = value
                            .as_str()
                            .and_then(|v| v.strip_prefix("0x"))
                            .ok_or(GenericError::OpaqueEffects)?;
                        return Ok(json!((data.len() / 2).to_string()));
                    }
                    if ty.contains('[') && !matches!(ty, "struct" | "mapping") {
                        return value
                            .as_array()
                            .map(|v| json!(v.len().to_string()))
                            .ok_or(GenericError::OpaqueEffects);
                    }
                }
                let member = syntax::reference(node)?;
                let index = self.program.member_index(member)?;
                value
                    .as_array()
                    .and_then(|v| v.get(index))
                    .cloned()
                    .ok_or(GenericError::OpaqueEffects)
            }
        }
    }
    pub(super) fn path(&self, node: &Value) -> Result<Vec<String>, GenericError> {
        self.step()?;
        match node["nodeType"].as_str() {
            Some("Identifier") => {
                let reference = syntax::reference(node)?;
                let fields = self.layout["storage"]
                    .as_array()
                    .ok_or(GenericError::OpaqueEffects)?;
                let matches = fields
                    .iter()
                    .filter(|field| field["astId"].as_i64() == Some(reference))
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err(GenericError::OpaqueEffects);
                }
                Ok(vec![
                    matches[0]["label"]
                        .as_str()
                        .ok_or(GenericError::OpaqueEffects)?
                        .into(),
                ])
            }
            Some("MemberAccess") => {
                let mut path = self.path(&node["expression"])?;
                path.push(
                    node["memberName"]
                        .as_str()
                        .ok_or(GenericError::OpaqueEffects)?
                        .into(),
                );
                Ok(path)
            }
            Some("IndexAccess") => {
                let mut path = self.path(&node["baseExpression"])?;
                let key = self.value(&node["indexExpression"], 0)?;
                path.push(match key {
                    Value::String(v) => v,
                    Value::Bool(v) => v.to_string(),
                    _ => return Err(GenericError::OpaqueEffects),
                });
                Ok(path)
            }
            _ => Err(GenericError::OpaqueEffects),
        }
    }
    /// State reads are permitted only where they cannot expand material effects:
    /// revert-only guards, event data and return observations. Calls and hidden
    /// side effects are rejected recursively, regardless of the current branch.
    pub(super) fn read_only(&self, node: &Value, depth: usize) -> Result<(), GenericError> {
        self.step()?;
        if depth > 32 {
            return Err(GenericError::Limit);
        }
        match node["nodeType"].as_str() {
            Some("Literal" | "Identifier" | "ElementaryTypeNameExpression") => Ok(()),
            Some("MemberAccess") => self.read_only(&node["expression"], depth + 1),
            Some("IndexAccess") => {
                self.read_only(&node["baseExpression"], depth + 1)?;
                self.read_only(&node["indexExpression"], depth + 1)
            }
            Some("UnaryOperation")
                if matches!(node["operator"].as_str(), Some("!" | "-" | "~")) =>
            {
                self.read_only(&node["subExpression"], depth + 1)
            }
            Some("BinaryOperation") => {
                self.read_only(&node["leftExpression"], depth + 1)?;
                self.read_only(&node["rightExpression"], depth + 1)
            }
            Some("TupleExpression") => {
                for item in node["components"]
                    .as_array()
                    .ok_or(GenericError::OpaqueEffects)?
                {
                    self.read_only(item, depth + 1)?;
                }
                Ok(())
            }
            Some("FunctionCall") if node["kind"] == "functionCall" => {
                if !self.modern
                    || node["expression"]["nodeType"] != "Identifier"
                    || node["names"]
                        .as_array()
                        .is_some_and(|names| !names.is_empty())
                {
                    return Err(GenericError::OpaqueEffects);
                }
                let helper = self
                    .program
                    .helper(syntax::reference(&node["expression"])?)?;
                if !matches!(helper["stateMutability"].as_str(), Some("pure" | "view"))
                    || helper["modifiers"].as_array().is_none_or(|v| !v.is_empty())
                {
                    return Err(GenericError::OpaqueEffects);
                }
                let statements = helper["body"]["statements"]
                    .as_array()
                    .filter(|v| v.len() == 1 && v[0]["nodeType"] == "Return")
                    .ok_or(GenericError::OpaqueEffects)?;
                for argument in node["arguments"]
                    .as_array()
                    .filter(|v| v.len() <= 16)
                    .ok_or(GenericError::OpaqueEffects)?
                {
                    self.read_only(argument, depth + 1)?;
                }
                self.read_only(&statements[0]["expression"], depth + 1)
            }
            Some("FunctionCall") if node["kind"] == "typeConversion" => {
                for item in node["arguments"]
                    .as_array()
                    .ok_or(GenericError::OpaqueEffects)?
                {
                    self.read_only(item, depth + 1)?;
                }
                Ok(())
            }
            _ => Err(GenericError::OpaqueEffects),
        }
    }
}
fn literal(node: &Value) -> Result<Value, GenericError> {
    if node["kind"] == "hexString" {
        return Ok(json!(format!(
            "0x{}",
            node["hexValue"]
                .as_str()
                .filter(|v| v.len() <= 2048)
                .ok_or(GenericError::OpaqueEffects)?
                .to_ascii_lowercase()
        )));
    }
    let value = node["value"].as_str().ok_or(GenericError::OpaqueEffects)?;
    match node["kind"].as_str() {
        Some("bool") => match value {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            _ => Err(GenericError::OpaqueEffects),
        },
        Some("string" | "unicodeString") if value.len() <= 1024 => Ok(json!(value)),
        Some("hexString") => Ok(json!(format!(
            "0x{}",
            node["hexValue"]
                .as_str()
                .filter(|v| v.len() <= 2048)
                .ok_or(GenericError::OpaqueEffects)?
        ))),
        Some("number") => {
            if node["typeDescriptions"]["typeString"] == "address" {
                let address: Address = value.parse().map_err(|_| GenericError::OpaqueEffects)?;
                return Ok(json!(format!("{address:#x}")));
            }
            let number = if let Some(hex) = value.strip_prefix("0x") {
                U256::from_str_radix(hex, 16)
            } else {
                U256::from_str_radix(value, 10)
            }
            .map_err(|_| GenericError::OpaqueEffects)?;
            let units = match node["subdenomination"].as_str() {
                None | Some("wei") => U256::ONE,
                Some("gwei") => U256::from(1_000_000_000u64),
                Some("ether") => U256::from(1_000_000_000_000_000_000u64),
                _ => return Err(GenericError::OpaqueEffects),
            };
            Ok(json!(
                number
                    .checked_mul(units)
                    .ok_or(GenericError::OpaqueEffects)?
                    .to_string()
            ))
        }
        _ => Err(GenericError::OpaqueEffects),
    }
}
fn binary(
    operator: &str,
    left: &Value,
    right: &Value,
    common: Option<&str>,
) -> Result<Value, GenericError> {
    match operator {
        "==" | "!=" => {
            let ty = common
                .and_then(|ty| ty.split_whitespace().next())
                .ok_or(GenericError::OpaqueEffects)?;
            canonical(ty, left)?;
            canonical(ty, right)?;
            return Ok(json!(if operator == "==" {
                left == right
            } else {
                left != right
            }));
        }
        "&&" | "||" => {
            let a = left.as_bool().ok_or(GenericError::OpaqueEffects)?;
            let b = right.as_bool().ok_or(GenericError::OpaqueEffects)?;
            return Ok(json!(if operator == "&&" { a && b } else { a || b }));
        }
        _ => {}
    }
    let a = left
        .as_str()
        .and_then(|v| U256::from_str_radix(v, 10).ok())
        .ok_or(GenericError::OpaqueEffects)?;
    let b = right
        .as_str()
        .and_then(|v| U256::from_str_radix(v, 10).ok())
        .ok_or(GenericError::OpaqueEffects)?;
    let number = match operator {
        "+" => a.checked_add(b),
        "-" => a.checked_sub(b),
        "*" => a.checked_mul(b),
        "/" => a.checked_div(b),
        "%" => a.checked_rem(b),
        "<" => return Ok(json!(a < b)),
        "<=" => return Ok(json!(a <= b)),
        ">" => return Ok(json!(a > b)),
        ">=" => return Ok(json!(a >= b)),
        _ => return Err(GenericError::OpaqueEffects),
    };
    Ok(json!(
        number.ok_or(GenericError::OpaqueEffects)?.to_string()
    ))
}

fn unary(node: &Value, value: &Value) -> Result<Value, GenericError> {
    match node["operator"].as_str() {
        Some("!") => value
            .as_bool()
            .map(|value| json!(!value))
            .ok_or(GenericError::OpaqueEffects),
        Some("-") => {
            let value = value.as_str().ok_or(GenericError::OpaqueEffects)?;
            let parsed =
                U256::from_str_radix(value, 10).map_err(|_| GenericError::OpaqueEffects)?;
            if parsed.to_string() != value {
                return Err(GenericError::OpaqueEffects);
            }
            if parsed == U256::ZERO {
                Ok(json!("0"))
            } else {
                Ok(json!(format!("-{parsed}")))
            }
        }
        _ => Err(GenericError::OpaqueEffects),
    }
}
pub(super) fn base_type(node: &Value) -> Result<&str, GenericError> {
    if let Some(ty) = node["typeDescriptions"]["typeString"].as_str() {
        return ty
            .split_whitespace()
            .next()
            .ok_or(GenericError::OpaqueEffects);
    }
    if node["nodeType"] == "Literal" {
        return match node["kind"].as_str() {
            Some("number") => Ok("int_const"),
            Some("bool") => Ok("bool"),
            Some("string" | "unicodeString" | "hexString") => Ok("literal_string"),
            _ => Err(GenericError::OpaqueEffects),
        };
    }
    Err(GenericError::OpaqueEffects)
}
pub(super) fn canonical(ty: &str, value: &Value) -> Result<(), GenericError> {
    if ty == "int_const" {
        let text = value.as_str().ok_or(GenericError::OpaqueEffects)?;
        let normalized = if text.starts_with('-') {
            text.parse::<alloy_primitives::I256>()
                .map_err(|_| GenericError::OpaqueEffects)?
                .to_string()
        } else {
            U256::from_str_radix(text, 10)
                .map_err(|_| GenericError::OpaqueEffects)?
                .to_string()
        };
        return if normalized == text {
            Ok(())
        } else {
            Err(GenericError::OpaqueEffects)
        };
    }
    super::super::condition::validate_expected(ty, super::super::GenericComparison::Equal, value)
        .map_err(|_| GenericError::OpaqueEffects)
}
