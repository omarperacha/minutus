use super::*;
#[test]
fn cancellation_and_invalid_targets_stop_before_provider_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoint = EvmRpcUrl::new("https://never-contact.invalid".into())?;
    let proof = DeploymentProof {
        block_hash: format!("0x{}", "11".repeat(32)),
        transaction_index: 0,
        transaction: String::new(),
        transaction_proof: vec![],
        receipt: String::new(),
        receipt_proof: vec![],
    };
    assert_eq!(
        qualify_contract_workflow_source(
            EvmNetwork::EthereumSepolia,
            &endpoint,
            "0x0000000000000000000000000000000000000000",
            &proof,
            &AtomicBool::new(false)
        )
        .err(),
        Some(QualificationError::InvalidIntent)
    );
    assert_eq!(
        qualify_contract_workflow_source(
            EvmNetwork::EthereumSepolia,
            &endpoint,
            "0x2222222222222222222222222222222222222222",
            &proof,
            &AtomicBool::new(true)
        )
        .err(),
        Some(QualificationError::Simulation(
            evm_simulation::SimulationError::Cancelled
        ))
    );
    Ok(())
}

mod rpc_integration;
