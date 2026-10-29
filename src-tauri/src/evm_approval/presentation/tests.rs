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
