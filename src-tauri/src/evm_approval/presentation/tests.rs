use super::*;

#[test]
fn checksummed_review_account_binds_the_canonical_signer_without_changing_display()
-> Result<(), Box<dyn std::error::Error>> {
    // A fixed, unsigned synthetic ETH transaction, independently decoded by
    // the same entry point used for native review. No RPC or signing occurs.
    let hex = concat!(
        "02ef0180843b9aca00847735940082520894",
        "1111111111111111111111111111111111111111",
        "880de0b6b3a764000080c0"
    );
    let payload = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16))
        .collect::<Result<Vec<_>, _>>()?;
    let canonical = "0x9858effd232b4033e47d90003d41ec34ecaeda94";
    let checksummed = chain_evm::PublicEvmAddress::new(canonical)?.checksum();
    assert_ne!(canonical, checksummed);
    for spelling in [canonical, checksummed.as_str()] {
        let DecodedSigningPayload::Eip1559Transaction(decoded) =
            decode_signing_payload(SigningPayload::Eip1559Transaction {
                account: spelling,
                payload: &payload,
            })?
        else {
            return Err("expected transaction".into());
        };
        assert_eq!(decoded.account(), checksummed);
        assert_eq!(
            signing_account(&decoded).map_err(|error| error.message)?,
            canonical
        );
        assert_ne!(
            signing_account(&decoded).map_err(|error| error.message)?,
            "0x1111111111111111111111111111111111111111"
        );
    }
    Ok(())
}

#[test]
fn increasing_a_reviewed_fee_budget_invalidates_exact_approval_even_with_the_same_transaction()
-> Result<(), Box<dyn std::error::Error>> {
    use transaction_policy::{ApprovalBindingInput, ApprovalCommitment, ApprovalTransactionType};
    let original = chain_evm::TransferFeeBudget::new("10000", "100", "10")?;
    let changed = chain_evm::TransferFeeBudget::new("10000", "101", "10")?;
    let digest = |budget| {
        simulation_budget_digest(b"fixed simulation", budget, ["quote", "10110", "100", "10"])
            .map_err(|e| e.message)
    };
    let binding = ApprovalBindingInput {
        signing_digest: [1; 32],
        chain_id: 84532,
        account: "0x1111111111111111111111111111111111111111",
        session_id: "fee-budget-test",
        expires_at_unix: 1060,
        transaction_type: ApprovalTransactionType::Eip1559 {
            transaction_type: 2,
        },
        decoder_digest: [2; 32],
        simulation_digest: digest(&original)?,
    };
    let approval = ApprovalCommitment::issue(binding, 1000)?;
    assert!(approval.verify(binding, 1001).is_ok());
    let altered = ApprovalBindingInput {
        simulation_digest: digest(&changed)?,
        ..binding
    };
    assert!(approval.verify(altered, 1001).is_err());
    assert_eq!(binding.signing_digest, altered.signing_digest);
    assert_eq!(original.l1_wei(), "150");
    Ok(())
}
