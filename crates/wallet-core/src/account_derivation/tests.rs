use std::error::Error;

use bip39::{Language, Mnemonic};

use super::*;

const BIP39_ZERO_ENTROPY_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const BIP39_ZERO_ENTROPY_DEFAULT_EVM_ADDRESS: &str = "0x9858effd232b4033e47d90003d41ec34ecaeda94";

#[test]
fn default_account_matches_the_bip39_zero_entropy_vector() -> Result<(), Box<dyn Error>> {
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, BIP39_ZERO_ENTROPY_MNEMONIC)?;
    let seed = RecoverySeed::new(mnemonic.to_seed_normalized(""));

    let account = derive_default_evm_account(&seed)?;

    assert_eq!(
        account.address().as_str(),
        BIP39_ZERO_ENTROPY_DEFAULT_EVM_ADDRESS
    );
    assert_eq!(account.derivation_path(), EVM_DEFAULT_DERIVATION_PATH);
    Ok(())
}

#[test]
fn default_account_derivation_is_deterministic() -> Result<(), Box<dyn Error>> {
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, BIP39_ZERO_ENTROPY_MNEMONIC)?;
    let first_seed = RecoverySeed::new(mnemonic.to_seed_normalized(""));
    let second_seed = RecoverySeed::new(mnemonic.to_seed_normalized(""));

    assert_eq!(
        derive_default_evm_account(&first_seed)?,
        derive_default_evm_account(&second_seed)?
    );
    Ok(())
}

#[test]
fn vault_metadata_records_the_default_path_and_recovery_origin() -> Result<(), Box<dyn Error>> {
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, BIP39_ZERO_ENTROPY_MNEMONIC)?;
    let seed = RecoverySeed::new(mnemonic.to_seed_normalized(""));
    let account = derive_default_evm_account(&seed)?;

    let metadata = account.metadata("primary", 84_532)?;

    assert_eq!(metadata.address(), BIP39_ZERO_ENTROPY_DEFAULT_EVM_ADDRESS);
    assert_eq!(
        metadata.key_origin().derivation_path(),
        Some(EVM_DEFAULT_DERIVATION_PATH)
    );
    assert_ne!(metadata.key_origin(), &AccountKeyOrigin::ImportedPrivateKey);
    Ok(())
}

#[test]
fn recovery_guidance_names_the_selected_path() {
    assert!(DEFAULT_EVM_ACCOUNT_RECOVERY_GUIDANCE.contains(EVM_DEFAULT_DERIVATION_PATH));
}
