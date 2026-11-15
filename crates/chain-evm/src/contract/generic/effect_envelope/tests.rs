use super::*;
use serde_json::json;
use std::{cell::Cell, collections::BTreeMap};

fn identify(node: &mut Value, next: &mut i64) {
    if let Some(object) = node.as_object_mut() {
        if object.get("nodeType").is_some_and(Value::is_string) && !object.contains_key("id") {
            object.insert("id".into(), json!(*next));
            *next += 1;
        }
        for value in object.values_mut() {
            identify(value, next);
        }
    } else if let Some(array) = node.as_array_mut() {
        for value in array {
            identify(value, next);
        }
    }
}
fn identifier(reference: i64) -> Value {
    json!({"nodeType":"Identifier","referencedDeclaration":reference})
}
fn number(value: &str) -> Value {
    json!({"nodeType":"Literal","kind":"number","value":value,"subdenomination":null})
}
fn assignment(operator: &str, value: Value) -> Value {
    let mut result = json!({"nodeType":"ExpressionStatement","expression":{"nodeType":"Assignment","operator":operator,"leftHandSide":identifier(3)}});
    result["expression"]["rightHandSide"] = value;
    result
}
fn fixture(mut statements: Vec<Value>) -> (Value, Value, Value) {
    let mut next = 100;
    for statement in &mut statements {
        identify(statement, &mut next);
    }
    let ast = json!({"Example.sol":{"nodeType":"SourceUnit","id":1,"nodes":[{"nodeType":"ContractDefinition","id":2,"name":"Example","linearizedBaseContracts":[2],"nodes":[
        {"nodeType":"VariableDeclaration","id":3,"stateVariable":true,"name":"counter","constant":false},
        {"nodeType":"FunctionDefinition","id":5,"name":"store","functionSelector":"6057361d","implemented":true,"modifiers":[],"parameters":{"nodeType":"ParameterList","id":6,"parameters":[{"nodeType":"VariableDeclaration","id":4,"name":"value","storageLocation":"default","typeDescriptions":{"typeString":"uint256"}}]},"body":{"nodeType":"Block","id":7,"statements":statements}}
    ]}]}});
    let metadata = json!({"settings":{"compilationTarget":{"Example.sol":"Example"}}});
    let layout = json!({"storage":[{"astId":3,"label":"counter","slot":"0","offset":0,"type":"u"}],"types":{"u":{"encoding":"inplace","label":"uint256","numberOfBytes":"32"}}});
    (ast, metadata, layout)
}
fn analyse(
    statements: Vec<Value>,
    modern: bool,
) -> Result<Vec<GenericEffectRelation>, GenericError> {
    analyse_fixture(fixture(statements), modern)
}
fn analyse_fixture(
    parts: (Value, Value, Value),
    modern: bool,
) -> Result<Vec<GenericEffectRelation>, GenericError> {
    analyse_values(parts, modern, [(4, json!("42"))].into())
}
fn analyse_values(
    (asts, metadata, layout): (Value, Value, Value),
    modern: bool,
    values: BTreeMap<i64, Value>,
) -> Result<Vec<GenericEffectRelation>, GenericError> {
    let program = Program::new(&asts, &metadata)?;
    let mut analysis = Analysis {
        program: &program,
        layout: &layout,
        values,
        relations: Vec::new(),
        calls: Vec::new(),
        steps: Cell::new(0),
        modern,
        features: super::eligibility::Features::Structured,
        base: false,
        account: "0x1111111111111111111111111111111111111111",
        origin: "0x1111111111111111111111111111111111111111",
        target: "0x2222222222222222222222222222222222222222",
        code_address: "0x2222222222222222222222222222222222222222",
        immutable_bindings: &BTreeMap::new(),
        native_value: "0",
        terminated: false,
        stack: Vec::new(),
    };
    analysis.function(program.function("6057361d")?, 0)?;
    Ok(analysis.relations)
}

fn coincidence_branch(condition: Value) -> Value {
    let mut result = json!({"nodeType":"IfStatement","trueBody":assignment("+=",number("1")),"falseBody":assignment("=",number("5"))});
    result["condition"] = condition;
    result
}
fn equal(left: Value, right: Value, ty: &str) -> Value {
    let mut result =
        json!({"nodeType":"BinaryOperation","operator":"==","commonType":{"typeString":ty}});
    result["leftExpression"] = left;
    result["rightExpression"] = right;
    result
}
#[test]
fn signed_zero_equality_cannot_select_a_snapshot_coincidence_envelope() -> Result<(), GenericError>
{
    let zero = json!({"nodeType":"FunctionCall","kind":"typeConversion","typeDescriptions":{"typeString":"int256"},"arguments":[number("0")]});
    let negative = json!({"nodeType":"UnaryOperation","operator":"-","subExpression":zero});
    let proof = analyse(
        vec![coincidence_branch(equal(
            negative,
            json!({"nodeType":"FunctionCall","kind":"typeConversion","typeDescriptions":{"typeString":"int256"},"arguments":[number("0")]}),
            "int256",
        ))],
        true,
    )?;
    // Both branches yield five at snapshot four. Only +1 is correct when the
    // counter changes to one hundred before inclusion.
    assert_eq!(proof[0].operation, GenericEffectOperation::Increase);
    assert_eq!(proof[0].value, json!("1"));
    Ok(())
}
#[test]
fn bytes_length_is_bytes_not_hex_characters_and_ambiguous_string_casts_reject()
-> Result<(), GenericError> {
    let mut data = identifier(4);
    data["typeDescriptions"] = json!({"typeString":"bytes calldata"});
    let length = json!({"nodeType":"MemberAccess","expression":data,"memberName":"length"});
    let parts = fixture(vec![coincidence_branch(equal(
        length,
        number("2"),
        "uint256",
    ))]);
    let proof = analyse_values(parts, true, [(4, json!("0x0102"))].into())?;
    assert_eq!(proof[0].operation, GenericEffectOperation::Increase);
    let mut text = identifier(4);
    text["typeDescriptions"] = json!({"typeString":"string calldata"});
    let cast = json!({"nodeType":"FunctionCall","kind":"typeConversion","arguments":[text],"typeDescriptions":{"typeString":"bytes memory"}});
    let length = json!({"nodeType":"MemberAccess","expression":cast,"memberName":"length"});
    assert!(
        analyse_values(
            fixture(vec![coincidence_branch(equal(
                length,
                number("2"),
                "uint256"
            ))]),
            true,
            [(4, json!("0x0102"))].into()
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn shadowed_msg_and_struct_length_are_members_not_builtin_context_or_tuple_length()
-> Result<(), GenericError> {
    let mut base = identifier(4);
    base["name"] = json!("msg");
    base["typeDescriptions"] = json!({"typeString":"struct Example.Info calldata"});
    for (name, id, right, ty, expected) in [
        (
            "sender",
            41,
            json!({"nodeType":"Literal","kind":"number","value":"0x1111111111111111111111111111111111111111","typeDescriptions":{"typeString":"address"}}),
            "address",
            GenericEffectOperation::Set,
        ),
        (
            "length",
            42,
            number("7"),
            "uint256",
            GenericEffectOperation::Increase,
        ),
    ] {
        let member = json!({"nodeType":"MemberAccess","expression":base,"memberName":name,"referencedDeclaration":id});
        let (mut ast, metadata, layout) =
            fixture(vec![coincidence_branch(equal(member, right, ty))]);
        append(
            &mut ast,
            json!({"nodeType":"StructDefinition","id":40,"members":[{"nodeType":"VariableDeclaration","id":41,"name":"sender"},{"nodeType":"VariableDeclaration","id":42,"name":"length"}]}),
        );
        let proof = analyse_values(
            (ast, metadata, layout),
            true,
            [(
                4,
                json!(["0x3333333333333333333333333333333333333333", "7"]),
            )]
            .into(),
        )?;
        assert_eq!(proof[0].operation, expected);
    }
    Ok(())
}

fn helper(id: i64, parameter: i64, statements: Vec<Value>, pure: bool) -> Value {
    let mut result = json!({"nodeType":"FunctionDefinition","id":id,"name":"_helper","implemented":true,"visibility":"private","virtual":false,"stateMutability":if pure {"pure"} else {"nonpayable"},"modifiers":[],"parameters":{"nodeType":"ParameterList","parameters":[{"nodeType":"VariableDeclaration","id":parameter,"storageLocation":"default"}]},"body":{"nodeType":"Block"}});
    result["body"]["statements"] = Value::Array(statements);
    result
}
fn call(id: i64, arguments: Vec<Value>) -> Value {
    let mut result = json!({"nodeType":"FunctionCall","kind":"functionCall","names":[],"expression":identifier(id)});
    result["arguments"] = Value::Array(arguments);
    result
}
fn append(ast: &mut Value, mut node: Value) {
    let mut next = 1000;
    identify(&mut node, &mut next);
    if let Some(nodes) = ast["Example.sol"]["nodes"][0]["nodes"].as_array_mut() {
        nodes.push(node);
    }
}

#[test]
fn static_private_helpers_and_pure_argument_helpers_preserve_bound_relations()
-> Result<(), GenericError> {
    let (mut ast, metadata, layout) = fixture(vec![
        json!({"nodeType":"ExpressionStatement","expression":call(20,vec![call(30,vec![identifier(4)])])}),
    ]);
    append(
        &mut ast,
        helper(20, 21, vec![assignment("=", identifier(21))], false),
    );
    let mut pure = helper(
        30,
        31,
        vec![
            json!({"nodeType":"Return","expression":{"nodeType":"BinaryOperation","operator":"*","leftExpression":identifier(31),"rightExpression":number("2")}}),
        ],
        true,
    );
    let mut next = 2000;
    identify(&mut pure, &mut next);
    if let Some(nodes) = ast["Example.sol"]["nodes"][0]["nodes"].as_array_mut() {
        nodes.push(pure);
    }
    let result = analyse_fixture((ast, metadata, layout), true)?;
    assert_eq!(result[0].operation, GenericEffectOperation::Set);
    assert_eq!(result[0].value, json!("84"));
    Ok(())
}

#[test]
fn recursion_virtual_external_disguised_and_named_internal_calls_reject() {
    for kind in 0..5 {
        let mut invocation = call(20, vec![identifier(4)]);
        if kind == 3 {
            invocation["expression"] = json!({"nodeType":"MemberAccess","referencedDeclaration":20,"memberName":"_helper","expression":identifier(4)});
        }
        if kind == 4 {
            invocation["names"] = json!(["amount"]);
        }
        let (mut ast, metadata, layout) = fixture(vec![
            json!({"nodeType":"ExpressionStatement","expression":invocation}),
        ]);
        let statements = if kind == 0 {
            vec![
                json!({"nodeType":"ExpressionStatement","expression":call(20,vec![identifier(21)])}),
            ]
        } else {
            vec![assignment("=", identifier(21))]
        };
        let mut helper = helper(20, 21, statements, false);
        if kind == 1 {
            helper["virtual"] = json!(true);
        }
        if kind == 2 {
            helper["visibility"] = json!("external");
        }
        append(&mut ast, helper);
        assert!(analyse_fixture((ast, metadata, layout), true).is_err());
    }
}

#[test]
fn modifier_single_placeholder_revert_guards_admit_but_hidden_writes_and_extra_placeholders_reject()
-> Result<(), GenericError> {
    for kind in 0..3 {
        let (mut ast, metadata, layout) = fixture(vec![assignment("=", identifier(4))]);
        ast["Example.sol"]["nodes"][0]["nodes"][1]["modifiers"] = json!([{"nodeType":"ModifierInvocation","id":90,"modifierName":{"nodeType":"IdentifierPath","id":91,"name":"guard","referencedDeclaration":40},"arguments":[]}]);
        let guard = json!({"nodeType":"ExpressionStatement","expression":{"nodeType":"FunctionCall","expression":{"nodeType":"Identifier","name":"require","referencedDeclaration":-18},"arguments":[{"nodeType":"BinaryOperation","operator":">","leftExpression":identifier(3),"rightExpression":number("0")}]}});
        let mut statements = vec![
            if kind == 1 {
                assignment("=", number("7"))
            } else {
                guard
            },
            json!({"nodeType":"PlaceholderStatement"}),
        ];
        if kind == 2 {
            statements.push(json!({"nodeType":"PlaceholderStatement"}));
        }
        append(
            &mut ast,
            json!({"nodeType":"ModifierDefinition","id":40,"virtual":false,"body":{"nodeType":"Block","statements":statements}}),
        );
        let result = analyse_fixture((ast, metadata, layout), true);
        if kind == 0 {
            assert_eq!(result?.len(), 1);
        } else {
            assert!(result.is_err());
        }
    }
    Ok(())
}

#[test]
fn independent_box_counter_and_relative_assignment_have_enforced_relations()
-> Result<(), GenericError> {
    for modern in [false, true] {
        let proof = analyse(vec![assignment("=", identifier(4))], modern)?;
        assert_eq!(
            proof,
            [GenericEffectRelation {
                storage_address: "0x2222222222222222222222222222222222222222".into(),
                code_address: "0x2222222222222222222222222222222222222222".into(),
                path: vec!["counter".into()],
                operation: GenericEffectOperation::Set,
                value: json!("42"),
                checked: true
            }]
        );
    }
    for statement in [
        assignment("+=", number("1")),
        assignment(
            "=",
            json!({"nodeType":"BinaryOperation","operator":"+","leftExpression":identifier(3),"rightExpression":number("1")}),
        ),
        json!({"nodeType":"Return","expression":{"nodeType":"UnaryOperation","operator":"++","subExpression":identifier(3)}}),
    ] {
        let proof = analyse(vec![statement.clone()], true)?;
        assert_eq!(proof[0].operation, GenericEffectOperation::Increase);
        assert_eq!(proof[0].value, json!("1"));
        assert!(analyse(vec![statement], false).is_err());
    }
    Ok(())
}

#[test]
fn unbounded_state_derived_amounts_external_drains_unchecked_and_state_dependent_effects_reject() {
    let drain = json!({"nodeType":"FunctionCall","kind":"functionCall","expression":{"nodeType":"MemberAccess","memberName":"transferFrom","expression":identifier(3)},"arguments":[identifier(3)]});
    let condition = json!({"nodeType":"BinaryOperation","operator":">","leftExpression":identifier(3),"rightExpression":number("0")});
    for statement in [
        assignment("=", identifier(3)),
        assignment("+=", identifier(3)),
        assignment("=", drain.clone()),
        json!({"nodeType":"ExpressionStatement","expression":drain}),
        json!({"nodeType":"UncheckedBlock","statements":[assignment("+=",number("1"))]}),
        json!({"nodeType":"IfStatement","condition":condition,"trueBody":assignment("=",identifier(4)),"falseBody":assignment("=",number("1"))}),
    ] {
        assert!(analyse(vec![statement], true).is_err());
    }
}

#[test]
fn state_guards_only_revert_and_calldata_branch_is_fixed_but_cast_truncation_rejects()
-> Result<(), GenericError> {
    let revert = json!({"nodeType":"RevertStatement","errorCall":{"nodeType":"FunctionCall","arguments":[]}});
    let guard = json!({"nodeType":"IfStatement","condition":{"nodeType":"BinaryOperation","operator":"==","leftExpression":identifier(3),"rightExpression":number("0")},"trueBody":revert,"falseBody":null});
    let proof = analyse(vec![guard, assignment("=", identifier(4))], true)?;
    assert_eq!(proof.len(), 1);
    let mut casting = json!({"nodeType":"FunctionCall","kind":"typeConversion","typeDescriptions":{"typeString":"uint8"},"arguments":[number("300")]});
    assert!(analyse(vec![assignment("=", casting.clone())], true).is_err());
    casting["arguments"][0] = number("7");
    assert_eq!(
        analyse(vec![assignment("=", casting)], true)?[0].value,
        json!("7")
    );
    let returned = analyse(
        vec![
            json!({"nodeType":"Return","expression":null}),
            assignment("=", identifier(4)),
        ],
        true,
    )?;
    assert!(returned.is_empty());
    Ok(())
}

#[test]
fn older_compiler_complex_features_are_not_blanket_qualified() -> Result<(), GenericError> {
    assert!(!super::modern("0.8.7+commit.e28d00a7")?);
    assert!(!super::modern("0.8.24+commit.e11b9ed9")?);
    assert!(super::modern("0.8.38+commit.1835d247")?);
    assert!(super::modern("0.7.6+commit.7338295f").is_err());
    Ok(())
}
