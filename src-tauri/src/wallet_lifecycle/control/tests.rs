use super::*;
use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
use wallet_core::AppVaultEntry;
fn entry(id: &str, seed: u8) -> Result<AppVaultEntry, Box<dyn std::error::Error>> {
    // Synthetic material only. No native/Keychain calls or real app-data namespace.
    let preview = wallet_core::EvmPrivateKeyImportPreview::validate(
        wallet_core::EvmPrivateKeyInput::new(format!("{seed:02x}").repeat(32))?,
    )?;
    let metadata = wallet_core::AccountMetadata::new(
        "account-1",
        preview.address().as_str(),
        8453,
        wallet_core::AccountKeyOrigin::ImportedPrivateKey,
    )?;
    let ack = preview.acknowledge_recovery_warning(true)?;
    Ok(AppVaultEntry::imported(
        VaultKeyId::new(id)?,
        preview.confirm(&ack)?,
        metadata,
    )?)
}
#[test]
fn normal_shared_controller_preserves_unlock_on_selection_and_revokes_old_submissions()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let directory = temp.path().canonicalize()?.join("app-vault");
    let control = WalletControl::initialize(&directory, None)?;
    let password = WalletPassword::new("synthetic main controller password".to_owned())?;
    let job = control.begin().map_err(|e| e.message)?;
    let first = AppVaultDocument::create(
        VaultKeyId::new("app-root")?,
        entry("first", 7)?,
        &password,
        &job,
    )?;
    control.publish(
        None,
        first.clone(),
        &password,
        VaultKeyId::new("first")?,
        &job,
    )?;
    let mut open = first.authenticate(&password, &job)?;
    open.add(entry("second", 8)?)?;
    let both = AppVaultDocument::seal(&open, &password, &job)?;
    control.publish(
        Some(&first),
        both.clone(),
        &password,
        VaultKeyId::new("first")?,
        &job,
    )?;
    drop(job);
    let last = control.state.lock().map_err(|_| "state")?.last_activity;
    control
        .select(&WalletId("second".to_owned()))
        .map_err(|e| e.message)?;
    assert_eq!(
        control.state.lock().map_err(|_| "state")?.last_activity,
        last
    );
    assert!(
        control
            .inventory()
            .map_err(|e| e.message)?
            .iter()
            .all(|s| s.availability == WalletAvailability::Ready)
    );
    let guard = super::super::submission::WalletSubmission::new(
        control.clone(),
        both.clone(),
        VaultKeyId::new("second")?,
        control.epoch.load(Ordering::Acquire),
        super::super::unix_now().map_err(|e| e.message)? + 1000,
    );
    control.lock();
    let job = control.begin().map_err(|e| e.message)?;
    control.unlock_blocking(both.clone(), VaultKeyId::new("second")?, &password, &job)?;
    drop(job);
    assert!(guard.begin().is_err());
    let guard = super::super::submission::WalletSubmission::new(
        control.clone(),
        both.clone(),
        VaultKeyId::new("second")?,
        control.epoch.load(Ordering::Acquire),
        super::super::unix_now().map_err(|e| e.message)? + 1000,
    );
    let now = control
        .state
        .lock()
        .map_err(|_| "state")?
        .last_activity
        .ok_or("activity")?;
    control.enforce_inactivity(now + Duration::from_mins(5));
    let job = control.begin().map_err(|e| e.message)?;
    control.unlock_blocking(both.clone(), VaultKeyId::new("second")?, &password, &job)?;
    drop(job);
    assert!(guard.begin().is_err());
    let guard = super::super::submission::WalletSubmission::new(
        control.clone(),
        both.clone(),
        VaultKeyId::new("second")?,
        control.epoch.load(Ordering::Acquire),
        super::super::unix_now().map_err(|e| e.message)? + 1000,
    );
    control
        .select(&WalletId("first".to_owned()))
        .map_err(|e| e.message)?;
    control
        .select(&WalletId("second".to_owned()))
        .map_err(|e| e.message)?;
    assert!(guard.begin().is_err());
    std::fs::rename(
        directory.join("app-root.password-vault.json"),
        temp.path().join("moved-root.json"),
    )?;
    assert!(control.snapshot().is_err());
    assert!(control.missing_vault.load(Ordering::Acquire));
    assert!(control.snapshot().map_err(|e| e.message)?.0.is_none());
    Ok(())
}
#[test]
fn malformed_or_missing_saved_vault_is_never_implicitly_fresh_setup()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let directory = temp.path().canonicalize()?.join("app-vault");
    let control = WalletControl::initialize(&directory, Some("account-remembered"))?;
    assert!(control.missing_vault.load(Ordering::Acquire));
    drop(control);
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(directory.join("app-broken.password-vault.json"))?;
    file.write_all(b"{invalid encrypted document}")?;
    drop(file);
    let control = WalletControl::initialize(&directory, Some("account-remembered"))?;
    assert!(control.public_state().is_err());
    assert!(control.snapshot().is_err());
    assert_eq!(
        std::fs::read(directory.join("app-broken.password-vault.json"))?,
        b"{invalid encrypted document}"
    );
    Ok(())
}

#[test]
fn locked_public_metadata_never_exposes_a_receiving_address()
-> Result<(), Box<dyn std::error::Error>> {
    let account = wallet_core::AccountMetadata::new(
        "synthetic",
        "0x1111111111111111111111111111111111111111",
        8453,
        wallet_core::AccountKeyOrigin::ImportedPrivateKey,
    )?;
    let id = VaultKeyId::new("synthetic")?;
    assert!(public(&id, &account, false).address.is_none());
    assert_eq!(
        public(&id, &account, true).address.as_deref(),
        Some(account.address())
    );
    Ok(())
}

#[test]
fn revocation_persists_locked_metadata_without_republishing_a_receiving_address()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let control = WalletControl::initialize(temp.path().canonicalize()?.join("app-vault"), None)?;
    let core = app_core::AppCore::initialize(temp.path().join("display-metadata"))?;
    let password = WalletPassword::new("synthetic metadata password".to_owned())?;
    let job = control.begin().map_err(|e| e.message)?;
    let document = AppVaultDocument::create(
        VaultKeyId::new("app-root")?,
        entry("first", 7)?,
        &password,
        &job,
    )?;
    control.publish(None, document, &password, VaultKeyId::new("first")?, &job)?;
    drop(job);
    super::super::persist_public(&core, &control).map_err(|e| e.message)?;
    let before = core
        .latest_public_wallet()?
        .ok_or("missing display metadata")?;
    assert!(!before.locked);
    control.lock();
    super::super::persist_public(&core, &control).map_err(|e| e.message)?;
    let after = core
        .latest_public_wallet()?
        .ok_or("missing locked metadata")?;
    assert!(after.locked);
    assert_eq!(before.address, after.address);
    assert!(
        control
            .public_state()
            .map_err(|e| e.message)?
            .address
            .is_none()
    );
    Ok(())
}
