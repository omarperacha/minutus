//! Internal dispatch follows compiler declaration edges and target C3 order.
use super::{GenericError, Program, id, reference};
use serde_json::Value;
use std::collections::BTreeSet;

impl<'a> Program<'a> {
    /// Lexical context may be the currently analysed function/modifier or its
    /// defining contract. It never changes this Program's most-derived target.
    pub(in crate::contract::generic::effect_envelope) fn resolve_helper(
        &self,
        expression: &Value,
        lexical: Option<i64>,
    ) -> Result<&'a Value, GenericError> {
        let selected = reference(expression)?;
        let function = self.internal_function(selected)?;
        let owner = self.owner(selected)?;
        let lexical = lexical.map(|node| self.lexical_owner(node)).transpose()?;
        match expression["nodeType"].as_str() {
            Some("Identifier") => {
                if let Some(lexical) = lexical {
                    self.inherits(lexical, owner)?;
                    if function["visibility"] == "private" && lexical != owner {
                        return Err(GenericError::OpaqueEffects);
                    }
                }
                if function["visibility"] == "private" || function["virtual"] == false {
                    Self::implemented(function)
                } else if function["virtual"] == true {
                    self.dispatch(selected, 0, "FunctionDefinition", "baseFunctions")
                } else {
                    Err(GenericError::OpaqueEffects)
                }
            }
            Some("MemberAccess") => {
                let base = &expression["expression"];
                let lexical = lexical.ok_or(GenericError::OpaqueEffects)?;
                if base["nodeType"] == "Identifier"
                    && base["name"] == "super"
                    && reference(base).is_ok_and(|id| id < 0)
                {
                    if function["visibility"] == "private" {
                        return Err(GenericError::OpaqueEffects);
                    }
                    let position = self
                        .contracts
                        .iter()
                        .position(|id| *id == lexical)
                        .ok_or(GenericError::OpaqueEffects)?;
                    self.dispatch(
                        selected,
                        position + 1,
                        "FunctionDefinition",
                        "baseFunctions",
                    )
                } else if matches!(
                    base["nodeType"].as_str(),
                    Some("Identifier" | "IdentifierPath")
                ) {
                    let explicit = reference(base)?;
                    if self.node(explicit)?["nodeType"] != "ContractDefinition"
                        || function["visibility"] == "private"
                    {
                        return Err(GenericError::OpaqueEffects);
                    }
                    self.inherits(lexical, explicit)?;
                    self.inherits(explicit, owner)?;
                    Self::implemented(function)
                } else {
                    Err(GenericError::OpaqueEffects)
                }
            }
            _ => Err(GenericError::OpaqueEffects),
        }
    }
    /// Qualified Base.modifier names remain closed until their static binding
    /// and invocation spelling have independent coverage.
    pub(in crate::contract::generic::effect_envelope) fn resolve_modifier(
        &self,
        name: &Value,
        lexical: Option<i64>,
    ) -> Result<&'a Value, GenericError> {
        if !matches!(
            name["nodeType"].as_str(),
            Some("Identifier" | "IdentifierPath")
        ) || name["name"]
            .as_str()
            .is_none_or(|name| name.is_empty() || name.contains('.'))
        {
            return Err(GenericError::OpaqueEffects);
        }
        let selected = reference(name)?;
        let lexical = self.lexical_owner(lexical.ok_or(GenericError::OpaqueEffects)?)?;
        self.inherits(lexical, self.owner(selected)?)?;
        self.modifier(selected)
    }
    pub(in crate::contract::generic::effect_envelope) fn modifier(
        &self,
        selected: i64,
    ) -> Result<&'a Value, GenericError> {
        let modifier = self.node(selected)?;
        self.owner(selected)?;
        if modifier["nodeType"] != "ModifierDefinition" {
            return Err(GenericError::OpaqueEffects);
        }
        if modifier["virtual"] == false {
            return Ok(modifier);
        }
        if modifier["virtual"] != true {
            return Err(GenericError::OpaqueEffects);
        }
        let resolved = self.dispatch(selected, 0, "ModifierDefinition", "baseModifiers")?;
        if !resolved["baseModifiers"].is_array() {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(resolved)
    }
    fn internal_function(&self, selected: i64) -> Result<&'a Value, GenericError> {
        self.owner(selected)?;
        let function = self.node(selected)?;
        if function["nodeType"] != "FunctionDefinition"
            || !matches!(
                function["visibility"].as_str(),
                Some("public" | "internal" | "private")
            )
        {
            return Err(GenericError::OpaqueEffects);
        }
        if function["visibility"] == "private" && function["virtual"] != false {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(function)
    }
    fn implemented(function: &'a Value) -> Result<&'a Value, GenericError> {
        if function["implemented"] != true || function["body"]["nodeType"] != "Block" {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(function)
    }
    fn lexical_owner(&self, lexical: i64) -> Result<i64, GenericError> {
        if self.contracts.contains(&lexical)
            && self.node(lexical)?["nodeType"] == "ContractDefinition"
        {
            Ok(lexical)
        } else {
            self.owner(lexical)
        }
    }
    fn owner(&self, member: i64) -> Result<i64, GenericError> {
        let mut found = None;
        for contract in &self.contracts {
            if self.node(*contract)?["nodeType"] != "ContractDefinition" {
                return Err(GenericError::OpaqueEffects);
            }
            let members = self.node(*contract)?["nodes"]
                .as_array()
                .ok_or(GenericError::OpaqueEffects)?;
            if members
                .iter()
                .any(|node| node["id"].as_i64() == Some(member))
                && found.replace(*contract).is_some()
            {
                return Err(GenericError::OpaqueEffects);
            }
        }
        found.ok_or(GenericError::OpaqueEffects)
    }
    fn inherits(&self, derived: i64, base: i64) -> Result<(), GenericError> {
        if !self.contracts.contains(&derived)
            || !self.contracts.contains(&base)
            || self.node(derived)?["linearizedBaseContracts"]
                .as_array()
                .is_none_or(|ids| !ids.iter().any(|id| id.as_i64() == Some(base)))
        {
            return Err(GenericError::OpaqueEffects);
        }
        Ok(())
    }
    fn dispatch(
        &self,
        selected: i64,
        start: usize,
        kind: &str,
        edges: &str,
    ) -> Result<&'a Value, GenericError> {
        for contract in self.contracts.iter().skip(start) {
            for candidate in self.node(*contract)?["nodes"]
                .as_array()
                .ok_or(GenericError::OpaqueEffects)?
            {
                if candidate["nodeType"] != kind {
                    continue;
                }
                let candidate_id = id(candidate)?;
                let mut ancestors = BTreeSet::new();
                let mut active = BTreeSet::new();
                self.ancestors(candidate_id, kind, edges, &mut ancestors, &mut active, 0)?;
                if candidate_id == selected || ancestors.contains(&selected) {
                    if kind == "FunctionDefinition" {
                        let function = self.internal_function(candidate_id)?;
                        if function["visibility"] == "private" {
                            continue;
                        }
                        return Self::implemented(function);
                    }
                    return Ok(candidate);
                }
            }
        }
        Err(GenericError::OpaqueEffects)
    }
    fn ancestors(
        &self,
        node: i64,
        kind: &str,
        edges: &str,
        seen: &mut BTreeSet<i64>,
        active: &mut BTreeSet<i64>,
        depth: usize,
    ) -> Result<(), GenericError> {
        if depth > 32 || seen.len() > 128 {
            return Err(GenericError::Limit);
        }
        if !active.insert(node) {
            return Err(GenericError::OpaqueEffects);
        }
        self.owner(node)?;
        let definition = self.node(node)?;
        if definition["nodeType"] != kind {
            return Err(GenericError::OpaqueEffects);
        }
        if definition.get("overrides").is_some_and(|v| !v.is_null())
            && !definition.get(edges).is_some_and(Value::is_array)
        {
            return Err(GenericError::OpaqueEffects);
        }
        if let Some(references) = definition.get(edges) {
            for parent in references.as_array().ok_or(GenericError::OpaqueEffects)? {
                let parent = parent
                    .as_i64()
                    .filter(|id| *id > 0)
                    .ok_or(GenericError::OpaqueEffects)?;
                if active.contains(&parent) {
                    return Err(GenericError::OpaqueEffects);
                }
                if seen.insert(parent) {
                    self.ancestors(parent, kind, edges, seen, active, depth + 1)?;
                }
            }
        }
        active.remove(&node);
        Ok(())
    }
}
