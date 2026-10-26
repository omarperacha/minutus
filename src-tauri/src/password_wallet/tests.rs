//! Synthetic lifecycle coordinator qualification; no native secret auto-confirm path.
use super::control::Control;
use std::{
    error::Error,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};
use wallet_core::{
    PasswordVaultError, RecoveryPhrase, RecoverySeed, RecoveryWordAnswers, VaultKeyId,
    WalletPassword, derive_default_evm_account,
};
use zeroize::Zeroizing;

fn verified_test_seed() -> Result<RecoverySeed, Box<dyn Error>> {
    recovery_material().map(|(seed, _)| seed)
}

fn recovery_material() -> Result<(RecoverySeed, Zeroizing<String>), Box<dyn Error>> {
    // Test binary only; native recovery never confirms itself or logs words.
    let mut phrase = RecoveryPhrase::generate()?;
    let words = phrase.present_once_for_onboarding(|text| Zeroizing::new(text.to_owned()))?;
    let prompt = phrase.begin_verification()?;
    let warnings = prompt.acknowledge_warnings(true, true, true)?;
    let split: Vec<&str> = words.split_whitespace().collect();
    let answers = prompt
        .positions()
        .map(|position| split[position.number() - 1].to_owned());
    phrase.verify_recovery_words(&prompt, &warnings, RecoveryWordAnswers::new(answers)?)?;
    Ok((phrase.derive_seed()?, words))
}

#[test]
fn coordinator_reopen_inactivity_cancelled_kdf_and_next_unlock() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().canonicalize()?.join("test-vaults");
    let control = Arc::new(Control::initialize(&path)?);
    let password = WalletPassword::new("synthetic coordinator password".to_owned())?;
    let seed = verified_test_seed()?;
    let account = derive_default_evm_account(&seed)?;
    let job = control.work.begin()?;
    control.create(VaultKeyId::new("coordinator-test")?, &password, &seed, &job)?;
    drop(job);
    assert!(control.account().is_err());
    let job = control.begin_operation()?;
    control.unlock(&password, &job)?;
    drop(job);
    assert_eq!(control.account()?, account.address().as_str());
    control.enforce_inactivity(Instant::now() + Duration::from_mins(6));
    assert!(control.account().is_err());
    drop(control);
    let control = Arc::new(Control::initialize(&path)?);
    assert!(control.account().is_err());
    let job = control.work.begin()?;
    let worker_control = control.clone();
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = barrier.clone();
    let thread = std::thread::spawn(move || {
        worker_barrier.wait();
        worker_control.unlock(&password, &job)
    });
    barrier.wait();
    std::thread::sleep(Duration::from_millis(50));
    let cancel_started = Instant::now();
    control.lock();
    assert!(cancel_started.elapsed() < Duration::from_millis(250));
    assert_eq!(
        thread.join().map_err(|_| "synthetic worker panicked")?,
        Err(PasswordVaultError::Cancelled)
    );
    assert!(control.account().is_err());
    println!(
        "cancelled_unlock_cleanup_ms={}",
        cancel_started.elapsed().as_millis()
    );
    let password = WalletPassword::new("synthetic coordinator password".to_owned())?;
    let job = control.work.begin()?;
    control.unlock(&password, &job)?;
    assert_eq!(control.account()?, account.address().as_str());
    control.lock();
    assert!(control.account().is_err());
    Ok(())
}

#[test]
fn password_change_verifies_existing_credential_without_unlocking_or_changing_the_file()
-> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().canonicalize()?.join("vaults");
    let control = Control::initialize(&path)?;
    let store = wallet_core::PasswordVaultStore::initialize(&path)?;
    let current = WalletPassword::new("synthetic existing password".to_owned())?;
    let wrong = WalletPassword::new("synthetic incorrect password".to_owned())?;
    let replacement = WalletPassword::new("synthetic replacement password".to_owned())?;
    let seed = verified_test_seed()?;
    let id = VaultKeyId::new("password-change-test")?;
    let job = control.begin_operation()?;
    control.create(id.clone(), &current, &seed, &job)?;
    control.unlock(&current, &job)?;
    let original = store.load(&id)?;
    assert_eq!(
        control.verify_password_for_change(&wrong, &job),
        Err(PasswordVaultError::AuthenticationFailed)
    );
    assert!(control.account().is_err());
    assert_eq!(store.load(&id)?, original);
    control.verify_password_for_change(&current, &job)?;
    assert!(control.account().is_err()); // Preflight never activates a session.
    assert_eq!(store.load(&id)?, original);
    // Passing preflight does not bypass authentication at the eventual commit.
    assert_eq!(
        control.change_password(&wrong, &replacement, &job),
        Err(PasswordVaultError::AuthenticationFailed)
    );
    assert_eq!(store.load(&id)?, original);
    control.change_password(&current, &replacement, &job)?;
    let changed = store.load(&id)?;
    assert_eq!(changed.payload(), original.payload());
    assert!(control.account().is_err());
    control.unlock(&replacement, &job)?;
    control.lock();
    assert_eq!(
        control.verify_password_for_change(&replacement, &job),
        Err(PasswordVaultError::Cancelled)
    );
    assert_eq!(store.load(&id)?, changed);
    Ok(())
}

#[test]
fn app_switch_preserves_pending_work_but_security_lock_cancels_and_coalesces()
-> Result<(), Box<dyn Error>> {
    use desktop_lifecycle::LifecycleEvent::{ApplicationInactive, SecurityBoundary};
    let root = tempfile::tempdir()?;
    let control = Arc::new(Control::initialize(
        &root.path().canonicalize()?.join("vaults"),
    )?);
    let job = control.begin_operation()?;
    assert!(!Control::should_lock_for(ApplicationInactive));
    job.check()?;
    assert!(Control::should_lock_for(SecurityBoundary));
    assert!(control.queue_lock());
    assert!(!control.queue_lock());
    assert!(matches!(
        control.begin_operation(),
        Err(PasswordVaultError::Busy)
    ));
    control.complete_queued_lock();
    assert_eq!(job.check(), Err(PasswordVaultError::Cancelled));
    assert!(!Control::should_lock_for(ApplicationInactive));
    Ok(())
}

#[test]
fn lifecycle_callback_revokes_readiness_without_waiting_for_publication_guard()
-> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let control = Arc::new(Control::initialize(
        &root.path().canonicalize()?.join("vaults"),
    )?);
    let job = control.begin_operation()?;
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = barrier.clone();
    let thread = std::thread::spawn(move || {
        let result = job.with_current(|| {
            worker_barrier.wait();
            worker_barrier.wait();
            Ok(())
        });
        (result, job)
    });
    barrier.wait();
    let start = Instant::now();
    assert!(control.queue_lock());
    assert!(start.elapsed() < Duration::from_millis(250));
    assert!(matches!(
        control.begin_operation(),
        Err(PasswordVaultError::Busy)
    ));
    barrier.wait();
    let (result, job) = thread
        .join()
        .map_err(|_| "synthetic guard worker panicked")?;
    result?;
    control.complete_queued_lock();
    assert_eq!(job.check(), Err(PasswordVaultError::Cancelled));
    Ok(())
}

#[test]
fn deleted_wallet_recovers_same_account_with_new_password_and_rejects_cancelled_or_occupied_restore()
-> Result<(), Box<dyn Error>> {
    use wallet_core::{
        RecoveryPhraseInput, VaultDeletionPreview, VaultRecoveryConfirmation,
        VaultTypedConfirmation, WalletRestoreChoice, WalletRestorePreview,
    };
    let root = tempfile::tempdir()?;
    let path = root.path().canonicalize()?.join("vaults");
    let control = Arc::new(Control::initialize(&path)?);
    let (seed, words) = recovery_material()?;
    let account = derive_default_evm_account(&seed)?;
    let original = WalletPassword::new("synthetic original password".to_owned())?;
    let replacement = WalletPassword::new("synthetic recovered password".to_owned())?;
    let job = control.begin_operation()?;
    control.create(VaultKeyId::new("original-test")?, &original, &seed, &job)?;
    assert!(control.recovery_inventory().is_err());
    let preview = VaultDeletionPreview::new_password_protected(control.id()?);
    let typed = VaultTypedConfirmation::new(preview.required_confirmation().to_owned())?;
    let commit = preview.confirm(
        VaultRecoveryConfirmation::AllRecoveryMaterialAvailable,
        true,
        typed,
    )?;
    control.delete(&original, commit, &job)?;
    drop(job);
    drop(control);
    let control = Arc::new(Control::initialize(&path)?);
    let confirm = || -> Result<_, Box<dyn Error>> {
        let input = RecoveryPhraseInput::new(words.to_string())?;
        let preview = WalletRestorePreview::validate(input, control.recovery_inventory()?)?;
        assert_eq!(preview.address().as_str(), account.address().as_str());
        Ok(preview.confirm(WalletRestoreChoice::Add {
            new_vault_id: VaultKeyId::new("recovered-test")?,
        })?)
    };
    let cancelled = confirm()?;
    let job = control.begin_operation()?;
    control.lock();
    assert_eq!(
        control.restore(cancelled, &replacement, &job),
        Err(PasswordVaultError::Cancelled)
    );
    drop(job);
    assert!(control.recovery_inventory()?.vault_ids().is_empty());
    let confirmed = confirm()?;
    let stale = confirm()?;
    let job = control.begin_operation()?;
    control.restore(confirmed, &replacement, &job)?;
    assert!(control.account().is_err()); // Restoration saves a locked wallet.
    assert_eq!(
        control.restore(stale, &replacement, &job),
        Err(PasswordVaultError::Conflict)
    );
    control.unlock(&replacement, &job)?;
    assert_eq!(control.account()?, account.address().as_str());
    control.lock();
    drop(job);
    let job = control.begin_operation()?;
    assert_eq!(
        control.unlock(&original, &job),
        Err(PasswordVaultError::AuthenticationFailed)
    );
    Ok(())
}
