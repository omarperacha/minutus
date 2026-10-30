//! Opt-in read-only public probe. No wallet, key, signature or submission.
use chain_evm::{
    EvmNetwork, EvmRpcUrl,
    contract::{
        ContractBlock, ContractCall, ContractReadOutcome, ContractScalar, DeclaredContractCall,
        read_contract_with_admission, read_declared_contract_with_admission,
    },
    trusted_assets,
};

#[test]
#[ignore = "explicit read-only public RPC qualification; network availability is not a CI fixture"]
fn public_usdc_total_supply_is_hash_pinned_on_the_four_supported_networks()
-> Result<(), Box<dyn std::error::Error>> {
    for network in EvmNetwork::ALL {
        let target = trusted_assets(network)[1]
            .token_contract()
            .ok_or("missing trusted USDC")?;
        let endpoint = EvmRpcUrl::new(network.descriptor().default_rpc_url().into())?;
        let call = ContractCall::new(target, "totalSupply()", &[], &[ContractScalar::Uint256])?;
        let observation = read_contract_with_admission(
            network,
            &endpoint,
            &call,
            None,
            &ContractBlock::Latest,
            &|| true,
        )?;
        assert!(matches!(
            observation.outcome,
            ContractReadOutcome::Returned { .. }
        ));
        assert!(observation.context.canonical_at_observation);
        assert!(!observation.context.source_qualified);
        assert!(!observation.context.writes_available);
        println!("{}", serde_json::to_string(&observation)?);
        // Reuse is explicit and checks canonicality afresh; never a latest cache hit.
        let repeated = read_contract_with_admission(
            network,
            &endpoint,
            &call,
            None,
            &ContractBlock::new(&observation.context.block_hash)?,
            &|| true,
        )?;
        assert_eq!(observation, repeated);
    }
    Ok(())
}

#[test]
#[ignore = "explicit read-only public RPC qualification; network availability is not a CI fixture"]
fn public_dynamic_token_names_use_canonical_typed_results() -> Result<(), Box<dyn std::error::Error>>
{
    for network in EvmNetwork::ALL {
        let target = trusted_assets(network)[1].token_contract().ok_or("USDC")?;
        let endpoint = EvmRpcUrl::new(network.descriptor().default_rpc_url().into())?;
        let call = DeclaredContractCall::new(target, "name()", &[], &["string".into()])?;
        let observed = read_declared_contract_with_admission(
            network,
            &endpoint,
            &call,
            None,
            &ContractBlock::Latest,
            &|| true,
        )?;
        let ContractReadOutcome::Returned { values, .. } = &observed.outcome else {
            return Err("reverted".into());
        };
        assert_eq!(values.len(), 1);
        assert!(values[0].is_string());
        assert!(!observed.context.source_qualified);
        assert!(!observed.context.writes_available);
        let repeat = read_declared_contract_with_admission(
            network,
            &endpoint,
            &call,
            None,
            &ContractBlock::Hash(observed.context.block_hash.clone()),
            &|| true,
        )?;
        assert_eq!(observed, repeat);
        println!("{}", serde_json::to_string(&observed)?);
    }
    Ok(())
}
