use std::error::Error;

use crate::VaultKeyId;

use super::{
    RecoveryPhraseInput, VaultInventory, WalletRestoreChoice, WalletRestoreError,
    WalletRestorePreview,
};

const TEST_PHRASE: &str = "test test test test test test test test test test test junk";
const TEST_ADDRESS: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn valid_phrase_checksum_derives_expected_address_before_confirmation() -> TestResult {
    let preview = preview(TEST_PHRASE, VaultInventory::empty())?;

    assert_eq!(preview.address().as_str(), TEST_ADDRESS);
    assert!(preview.inventory().vault_ids().is_empty());
    Ok(())
}

#[test]
fn invalid_checksum_is_rejected_locally() -> TestResult {
    let invalid_checksum = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
    let input = RecoveryPhraseInput::new(invalid_checksum.to_owned())?;

    assert_eq!(
        WalletRestorePreview::validate(input, VaultInventory::empty()).err(),
        Some(WalletRestoreError::InvalidRecoveryPhrase)
    );
    Ok(())
}

#[test]
fn add_requires_an_absent_explicit_vault_id() -> TestResult {
    let existing = VaultKeyId::new("existing")?;
    let inventory = VaultInventory::new(vec![existing.clone()])?;
    let preview = preview(TEST_PHRASE, inventory)?;

    assert_eq!(
        preview
            .confirm(WalletRestoreChoice::Add {
                new_vault_id: existing,
            })
            .err(),
        Some(WalletRestoreError::AddTargetExists)
    );
    Ok(())
}

#[test]
fn replace_requires_an_exact_existing_vault_id() -> TestResult {
    let preview = preview(TEST_PHRASE, VaultInventory::empty())?;

    assert_eq!(
        preview
            .confirm(WalletRestoreChoice::Replace {
                existing_vault_id: VaultKeyId::new("missing")?,
            })
            .err(),
        Some(WalletRestoreError::ReplaceTargetMissing)
    );
    Ok(())
}

#[test]
fn confirmed_add_produces_seed_only_after_fresh_inventory_check() -> TestResult {
    let inventory = VaultInventory::empty();
    let preview = preview(TEST_PHRASE, inventory.clone())?;
    let confirmed = preview.confirm(WalletRestoreChoice::Add {
        new_vault_id: VaultKeyId::new("restored")?,
    })?;

    let commit = confirmed.authorize_commit(&inventory)?;
    assert_eq!(commit.address().as_str(), TEST_ADDRESS);
    assert!(matches!(commit.choice(), WalletRestoreChoice::Add { .. }));
    assert_eq!(commit.into_seed().expose_secret().len(), 64);
    Ok(())
}

#[test]
fn inventory_change_invalidates_confirmed_restore() -> TestResult {
    let preview = preview(TEST_PHRASE, VaultInventory::empty())?;
    let confirmed = preview.confirm(WalletRestoreChoice::Add {
        new_vault_id: VaultKeyId::new("restored")?,
    })?;
    let changed = VaultInventory::new(vec![VaultKeyId::new("concurrent")?])?;

    assert_eq!(
        confirmed.authorize_commit(&changed).err(),
        Some(WalletRestoreError::InventoryChanged)
    );
    Ok(())
}

#[test]
fn inventory_is_bounded_canonical_and_duplicate_free() -> TestResult {
    let duplicate = VaultKeyId::new("same")?;
    assert_eq!(
        VaultInventory::new(vec![duplicate.clone(), duplicate]).err(),
        Some(WalletRestoreError::DuplicateVaultId)
    );

    let inventory = VaultInventory::new(vec![
        VaultKeyId::new("z-vault")?,
        VaultKeyId::new("a-vault")?,
    ])?;
    assert_eq!(inventory.vault_ids()[0].as_str(), "a-vault");
    Ok(())
}

fn preview(
    phrase: &str,
    inventory: VaultInventory,
) -> Result<WalletRestorePreview, WalletRestoreError> {
    WalletRestorePreview::validate(RecoveryPhraseInput::new(phrase.to_owned())?, inventory)
}
