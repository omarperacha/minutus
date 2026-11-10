use super::{Analysis, GenericEffectOperation, GenericError, syntax};
use serde_json::{Value, json};

impl Analysis<'_> {
    pub(super) fn function(&mut self, function: &Value, depth: usize) -> Result<(), GenericError> {
        if depth > 8 {
            return Err(GenericError::Limit);
        }
        let id = syntax::id(function)?;
        if self.stack.contains(&id) || (!self.modern && !self.stack.is_empty()) {
            return Err(GenericError::OpaqueEffects);
        }
        self.stack.push(id);
        let modifiers = function["modifiers"]
            .as_array()
            .ok_or(GenericError::OpaqueEffects)?;
        if !self.modern && !modifiers.is_empty() {
            return Err(GenericError::OpaqueEffects);
        }
        if !self.modern {
            let statements = function["body"]["statements"]
                .as_array()
                .filter(|v| v.len() == 1)
                .ok_or(GenericError::OpaqueEffects)?;
            let expression = &statements[0]["expression"];
            if statements[0]["nodeType"] != "ExpressionStatement"
                || expression["nodeType"] != "Assignment"
                || expression["operator"] != "="
                || function["parameters"]["parameters"]
                    .as_array()
                    .is_none_or(|parameters| {
                        parameters
                            .iter()
                            .any(|p| p["typeDescriptions"]["typeString"] != "uint256")
                    })
            {
                return Err(GenericError::OpaqueEffects);
            }
        }
        for invocation in modifiers {
            if invocation["arguments"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
            {
                return Err(GenericError::OpaqueEffects);
            }
            let definition = self
                .program
                .node(syntax::reference(&invocation["modifierName"])?)?;
            if definition["nodeType"] != "ModifierDefinition" || definition["virtual"] != false {
                return Err(GenericError::OpaqueEffects);
            }
            self.modifier(&definition["body"])?;
        }
        self.block(&function["body"], depth)?;
        let _ = self.stack.pop();
        Ok(())
    }
    fn modifier(&mut self, body: &Value) -> Result<(), GenericError> {
        let statements = body["statements"]
            .as_array()
            .filter(|v| v.len() <= 32)
            .ok_or(GenericError::OpaqueEffects)?;
        let mut placeholder = false;
        for statement in statements {
            self.step()?;
            if statement["nodeType"] == "PlaceholderStatement" {
                if placeholder {
                    return Err(GenericError::OpaqueEffects);
                }
                placeholder = true;
            } else if statement["nodeType"] == "ExpressionStatement" {
                let before = self.relations.len();
                self.expression(&statement["expression"])?;
                if self.relations.len() != before {
                    return Err(GenericError::OpaqueEffects);
                }
            } else if statement["nodeType"] == "IfStatement"
                && aborts(&statement["trueBody"])
                && statement["falseBody"].is_null()
            {
                self.read_only(&statement["condition"], 0)?;
                self.abort_guard(&statement["trueBody"])?;
            } else {
                return Err(GenericError::OpaqueEffects);
            }
        }
        if !placeholder {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(())
    }
    fn block(&mut self, block: &Value, depth: usize) -> Result<(), GenericError> {
        if depth > 16 || block["nodeType"] != "Block" {
            return Err(GenericError::OpaqueEffects);
        }
        for statement in block["statements"]
            .as_array()
            .filter(|v| v.len() <= 64)
            .ok_or(GenericError::OpaqueEffects)?
        {
            if self.terminated {
                break;
            }
            self.statement(statement, depth + 1)?;
        }
        Ok(())
    }
    fn statement(&mut self, node: &Value, depth: usize) -> Result<(), GenericError> {
        self.step()?;
        if depth > 16 {
            return Err(GenericError::Limit);
        }
        match node["nodeType"].as_str() {
            Some("Block") => self.block(node, depth + 1),
            Some("ExpressionStatement") => self.expression(&node["expression"]),
            Some("Return") => {
                let expression = &node["expression"];
                self.terminated = true;
                if expression.is_null() {
                    return Ok(());
                }
                if expression["nodeType"] == "UnaryOperation"
                    && matches!(expression["operator"].as_str(), Some("++" | "--"))
                {
                    self.expression(expression)
                } else {
                    self.read_only(expression, 0)
                }
            }
            Some("EmitStatement") => {
                let call = &node["eventCall"];
                if self.program.node(syntax::reference(&call["expression"])?)?["nodeType"]
                    != "EventDefinition"
                {
                    return Err(GenericError::OpaqueEffects);
                }
                for argument in call["arguments"]
                    .as_array()
                    .ok_or(GenericError::OpaqueEffects)?
                {
                    self.read_only(argument, 0)?;
                }
                Ok(())
            }
            Some("VariableDeclarationStatement") => {
                let declarations = node["declarations"]
                    .as_array()
                    .filter(|v| v.len() == 1)
                    .ok_or(GenericError::OpaqueEffects)?;
                let declaration = &declarations[0];
                if matches!(declaration["storageLocation"].as_str(), Some("storage")) {
                    return Err(GenericError::OpaqueEffects);
                }
                let value = self.value(&node["initialValue"], 0)?;
                super::expressions::canonical(super::expressions::base_type(declaration)?, &value)?;
                self.values.insert(syntax::id(declaration)?, value);
                Ok(())
            }
            Some("IfStatement") => self.conditional(node, depth + 1),
            // Loops, unchecked arithmetic, try/catch, assembly, creation and
            // unknown statements cannot silently expand an approved envelope.
            _ => Err(GenericError::OpaqueEffects),
        }
    }
    fn conditional(&mut self, node: &Value, depth: usize) -> Result<(), GenericError> {
        if let Ok(Value::Bool(condition)) = self.value(&node["condition"], 0) {
            let selected = if condition {
                &node["trueBody"]
            } else {
                &node["falseBody"]
            };
            if selected.is_null() {
                return Ok(());
            }
            return self.statement(selected, depth + 1);
        }
        self.read_only(&node["condition"], 0)?;
        let yes = aborts(&node["trueBody"]);
        let no = aborts(&node["falseBody"]);
        if yes {
            self.abort_guard(&node["trueBody"])?;
        }
        if no {
            self.abort_guard(&node["falseBody"])?;
        }
        if yes && no {
            return Err(GenericError::OpaqueEffects);
        }
        let normal = if yes {
            &node["falseBody"]
        } else if no {
            &node["trueBody"]
        } else {
            return Err(GenericError::OpaqueEffects);
        };
        if normal.is_null() {
            Ok(())
        } else {
            self.statement(normal, depth + 1)
        }
    }
    fn expression(&mut self, node: &Value) -> Result<(), GenericError> {
        match node["nodeType"].as_str() {
            Some("Assignment") => self.assignment(node),
            Some("UnaryOperation") if matches!(node["operator"].as_str(), Some("++" | "--")) => {
                if !self.modern {
                    return Err(GenericError::OpaqueEffects);
                }
                let path = self.path(&node["subExpression"])?;
                self.add(
                    path,
                    if node["operator"] == "++" {
                        GenericEffectOperation::Increase
                    } else {
                        GenericEffectOperation::Decrease
                    },
                    json!("1"),
                )
            }
            Some("FunctionCall") => {
                if node["expression"]["referencedDeclaration"]
                    .as_i64()
                    .is_some_and(|id| id >= 0)
                {
                    self.internal_call(node)
                } else {
                    self.guard(node)
                }
            }
            _ => Err(GenericError::OpaqueEffects),
        }
    }
    fn assignment(&mut self, node: &Value) -> Result<(), GenericError> {
        let left = &node["leftHandSide"];
        let right = &node["rightHandSide"];
        let path = self.path(left)?;
        let old_safe = match right["nodeType"].as_str() {
            Some("Identifier") => self.values.contains_key(&syntax::reference(right)?),
            Some("Literal") => true,
            _ => false,
        };
        if !self.modern && !old_safe {
            return Err(GenericError::OpaqueEffects);
        }
        let (operation, value) = match node["operator"].as_str() {
            Some("+=") => (GenericEffectOperation::Increase, self.value(right, 0)?),
            Some("-=") => (GenericEffectOperation::Decrease, self.value(right, 0)?),
            Some("=")
                if right["nodeType"] == "BinaryOperation"
                    && matches!(right["operator"].as_str(), Some("+" | "-"))
                    && self.path(&right["leftExpression"]).ok() == Some(path.clone()) =>
            {
                (
                    if right["operator"] == "+" {
                        GenericEffectOperation::Increase
                    } else {
                        GenericEffectOperation::Decrease
                    },
                    self.value(&right["rightExpression"], 0)?,
                )
            }
            Some("=") => (GenericEffectOperation::Set, self.value(right, 0)?),
            _ => return Err(GenericError::OpaqueEffects),
        };
        self.add(path, operation, value)
    }
    fn guard(&self, call: &Value) -> Result<(), GenericError> {
        let function = &call["expression"];
        if function["nodeType"] != "Identifier"
            || !matches!(function["name"].as_str(), Some("require" | "assert"))
            || syntax::reference(function)? >= 0
        {
            return Err(GenericError::OpaqueEffects);
        }
        for argument in call["arguments"]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 2)
            .ok_or(GenericError::OpaqueEffects)?
        {
            self.read_only(argument, 0)?;
        }
        Ok(())
    }
    fn internal_call(&mut self, call: &Value) -> Result<(), GenericError> {
        if !self.modern
            || call["kind"] != "functionCall"
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
        let parameters = function["parameters"]["parameters"]
            .as_array()
            .ok_or(GenericError::OpaqueEffects)?;
        let arguments = call["arguments"]
            .as_array()
            .filter(|v| v.len() <= 16 && v.len() == parameters.len())
            .ok_or(GenericError::OpaqueEffects)?;
        let values = arguments
            .iter()
            .map(|argument| self.value(argument, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let saved = self.values.clone();
        let terminated = self.terminated;
        for (parameter, value) in parameters.iter().zip(values) {
            if parameter["storageLocation"] == "storage" {
                return Err(GenericError::OpaqueEffects);
            }
            self.values.insert(syntax::id(parameter)?, value);
        }
        self.terminated = false;
        let result = self.function(function, self.stack.len());
        self.values = saved;
        self.terminated = terminated;
        result
    }
    fn abort_guard(&self, node: &Value) -> Result<(), GenericError> {
        if node["nodeType"] == "Block" {
            return self.abort_guard(&node["statements"][0]);
        }
        let call = if node["nodeType"] == "RevertStatement" {
            &node["errorCall"]
        } else {
            &node["expression"]
        };
        for argument in call["arguments"]
            .as_array()
            .ok_or(GenericError::OpaqueEffects)?
        {
            self.read_only(argument, 0)?;
        }
        Ok(())
    }
}
fn aborts(node: &Value) -> bool {
    if node["nodeType"] == "RevertStatement" {
        return true;
    }
    if node["nodeType"] == "ExpressionStatement" {
        let function = &node["expression"]["expression"];
        return function["nodeType"] == "Identifier"
            && function["name"] == "revert"
            && syntax::reference(function).is_ok_and(|id| id < 0);
    }
    node["nodeType"] == "Block"
        && node["statements"]
            .as_array()
            .is_some_and(|v| v.len() == 1 && aborts(&v[0]))
}
