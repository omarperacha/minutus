use super::*;
use app_core::{
    StoredDeclaredWorkflowStep, StoredWorkflowComparison, StoredWorkflowContract,
    StoredWorkflowDeclaration,
};

fn declaration(count: usize, string: bool) -> Result<ContractWorkflowDeclaration, GenericError> {
    let conditions = (0..count)
        .map(|index| StoredWorkflowCondition {
            contract: "app".into(),
            read: StoredWorkflowRead::SourceField {
                path: vec![json!(format!("field{index}"))],
            },
            comparison: StoredWorkflowComparison::Equal,
            expected: if string { json!("") } else { json!("0") },
        })
        .collect::<Vec<_>>();
    ContractWorkflowDeclaration::new(StoredWorkflowDeclaration {
        version: 2,
        network: "eip155:11155111".into(),
        account: "0x1111111111111111111111111111111111111111".into(),
        contracts: vec![StoredWorkflowContract {
            id: "app".into(),
            address: "0x2222222222222222222222222222222222222222".into(),
        }],
        steps: conditions
            .iter()
            .enumerate()
            .map(|(index, condition)| StoredDeclaredWorkflowStep {
                id: format!("step{index}"),
                contract: "app".into(),
                function: "store(uint256)".into(),
                arguments: vec![json!("0")],
                value_wei: "0".into(),
                depends_on: vec![],
                prerequisites: vec![],
                completion: vec![condition.clone()],
            })
            .collect(),
        goals: conditions.last().cloned().into_iter().collect(),
    })
    .map_err(|_| GenericError::InvalidAbi)
}
fn types(
    declaration: &ContractWorkflowDeclaration,
    ty: &str,
) -> Result<Vec<(GenericCondition, String)>, GenericError> {
    Ok(declaration
        .condition_requests()
        .map_err(|_| GenericError::InvalidAbi)?
        .into_iter()
        .map(|condition| (condition, ty.into()))
        .collect())
}

#[test]
fn multistep_numeric_reports_fit_but_complete_blob_reports_reject_before_writes()
-> Result<(), GenericError> {
    let numeric = declaration(8, false)?;
    let numeric_types = types(&numeric, "uint256")?;
    assert!(maximum_bytes(&numeric, &numeric_types)? <= REPORT_LIMIT);
    admit(&numeric, &numeric_types)?;
    let blobs = declaration(8, true)?;
    let blob_types = types(&blobs, "string")?;
    assert!(maximum_bytes(&blobs, &blob_types)? > REPORT_LIMIT);
    assert_eq!(admit(&blobs, &blob_types), Err(GenericError::Limit));
    let narrow = declaration(1, true)?;
    admit(&narrow, &types(&narrow, "string")?)?;
    Ok(())
}

#[test]
fn bounds_include_json_escaping_hex_expansion_and_full_declared_paths() -> Result<(), GenericError>
{
    let string =
        serde_json::to_string(&maximum_value("string")?).map_err(|_| GenericError::Limit)?;
    assert_eq!(string.len(), 1024 * 6 + 2);
    let bytes = serde_json::to_string(&maximum_value("bytes")?).map_err(|_| GenericError::Limit)?;
    assert_eq!(bytes.len(), 1024 * 2 + 4);
    let normal = declaration(8, false)?;
    let mut intent = normal.intent().clone();
    for step in &mut intent.steps {
        step.completion[0].read = StoredWorkflowRead::SourceField {
            path: vec![json!("\u{1}".repeat(64)); 8],
        };
    }
    let long = ContractWorkflowDeclaration::new(intent).map_err(|_| GenericError::InvalidAbi)?;
    assert!(
        maximum_bytes(&long, &types(&long, "uint256")?)?
            > maximum_bytes(&normal, &types(&normal, "uint256")?)?
    );
    assert_eq!(
        admit(&long, &types(&long, "uint256")?),
        Err(GenericError::Limit)
    );
    Ok(())
}
