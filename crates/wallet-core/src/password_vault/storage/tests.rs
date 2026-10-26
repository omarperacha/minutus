use super::*;
use crate::password_vault::{PasswordVaultWork, tests::fixture};
use std::os::unix::fs::OpenOptionsExt as _;
use std::{
    error::Error as StdError,
    fs as stdfs,
    os::unix::fs::{PermissionsExt as _, symlink},
};

struct Fail(Phase);
impl Faults for Fail {
    fn at(&self, phase: Phase) -> Result<(), Error> {
        if phase == self.0 {
            Err(Error::StorageUnavailable)
        } else {
            Ok(())
        }
    }
}

#[test]
fn empty_slot_creation_rejects_another_identifier_and_competing_owner()
-> Result<(), Box<dyn StdError>> {
    let root = tempfile::tempdir()?;
    let path = root.path().canonicalize()?.join("vaults");
    let store = PasswordVaultStore::initialize(&path)?;
    let competing = PasswordVaultStore::initialize(&path)?;
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (_, document) = fixture(&job)?;
    {
        let _lock = store.lock()?;
        assert_eq!(
            competing.create_in_empty_store(&document, &job),
            Err(Error::Busy)
        );
    }
    store.create_in_empty_store(&document, &job)?;
    // A distinct ID must not bypass the single-slot policy after another owner
    // has published. No KDF or valid wrapper is needed to reject occupied state.
    let mut different = document.clone();
    different.id = crate::VaultKeyId::new("different-test-wallet")?;
    assert_eq!(
        competing.create_in_empty_store(&different, &job),
        Err(Error::Conflict)
    );
    assert_eq!(store.vault_ids()?, vec![document.id().clone()]);
    assert_eq!(store.load(document.id())?, document);
    Ok(())
}

#[test]
fn atomic_creation_replacement_and_faults_preserve_usable_encrypted_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path().canonicalize()?.join("password-wallets");
    let store = PasswordVaultStore::initialize(&path)?;
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (password, document) = fixture(&job)?;
    store.create(&document, &job)?;
    assert_eq!(store.load(document.id())?, document);
    assert_eq!(store.vault_ids()?, vec![document.id().clone()]);
    assert_eq!(store.create(&document, &job), Err(Error::Conflict));
    // Reproduce the exact link-publication crash window, not just an invalid save.
    let stage = path.join(".stage-pw-0123456789abcdef0123456789abcdef");
    stdfs::hard_link(path.join(filename(document.id())), &stage)?;
    drop(store);
    let store = PasswordVaultStore::initialize(&path)?;
    assert!(!stage.exists());
    assert_eq!(store.load(document.id())?, document);
    let replacement =
        super::super::WalletPassword::new("replacement test passphrase only".to_owned())?;
    let changed = document.change_password(&password, &replacement, &job)?;
    for phase in [Phase::Write, Phase::FileSync, Phase::Reopen, Phase::Publish] {
        assert!(
            store
                .publish(&changed, Some(&document), &job, &Fail(phase))
                .is_err()
        );
        assert_eq!(store.load(document.id())?, document);
    }
    assert_eq!(
        store.publish(&changed, Some(&document), &job, &Fail(Phase::DirectorySync)),
        Err(Error::CommitUncertain)
    );
    assert_eq!(store.load(document.id())?, changed);
    let second = PasswordVaultStore::initialize(&path)?;
    store.with_revision(&changed, || {
        assert!(matches!(second.load(changed.id()), Err(Error::Busy)));
        Ok(())
    })?;
    let mut called = false;
    assert_eq!(
        store.with_revision(&document, || {
            called = true;
            Ok(())
        }),
        Err(Error::Conflict)
    );
    assert!(!called);
    let preview = crate::VaultDeletionPreview::new_password_protected(document.id().clone());
    let typed = crate::VaultTypedConfirmation::new(preview.required_confirmation().to_owned())?;
    let commit = preview.confirm(
        crate::VaultRecoveryConfirmation::AllRecoveryMaterialAvailable,
        true,
        typed,
    )?;
    let mut session = crate::VaultSession::new(
        document.id().clone(),
        crate::VaultInactivityTimeout::new(std::time::Duration::from_mins(1))?,
    );
    assert_eq!(
        store.delete(&document, commit, &mut session, &job),
        Err(Error::Conflict)
    );
    assert_eq!(store.load(changed.id())?, changed);
    assert_eq!(
        store.replace(&document, &changed, &job),
        Err(Error::Conflict)
    );
    drop(store);
    let reopened = PasswordVaultStore::initialize(&path)?;
    reopened
        .load(changed.id())?
        .authenticate_unlock(&replacement, &job)?;
    assert!(
        stdfs::read_dir(&path)?
            .all(|e| e.is_ok_and(|e| !e.file_name().to_string_lossy().starts_with(".stage-pw-")))
    );
    Ok(())
}

#[test]
fn pinned_directory_rejects_symlinks_hardlinks_permissions_and_old_formats()
-> Result<(), Box<dyn StdError>> {
    let root = tempfile::tempdir()?;
    let base = root.path().canonicalize()?;
    let store = PasswordVaultStore::initialize(&base.join("vaults"))?;
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (_password, document) = fixture(&job)?;
    store.create(&document, &job)?;
    let live = base.join("vaults").join(filename(document.id()));
    stdfs::set_permissions(&live, stdfs::Permissions::from_mode(0o644))?;
    assert!(matches!(
        store.load(document.id()),
        Err(Error::UnsafeStorage)
    ));
    stdfs::set_permissions(&live, stdfs::Permissions::from_mode(0o600))?;
    stdfs::hard_link(&live, base.join("copy"))?;
    assert!(store.load(document.id()).is_err());
    stdfs::remove_file(base.join("copy"))?;
    stdfs::remove_file(&live)?;
    symlink(base.join("unrelated"), &live)?;
    assert!(store.load(document.id()).is_err());
    stdfs::remove_file(&live)?;
    let mut file = stdfs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&live)?;
    file.write_all(b"{\"version\":1}")?;
    assert_eq!(
        store.load(document.id()).err(),
        Some(Error::UnsupportedVersion)
    );
    assert_eq!(stdfs::read(&live)?, b"{\"version\":1}");
    symlink(base.join("vaults"), base.join("linked"))?;
    assert!(PasswordVaultStore::initialize(&base.join("linked")).is_err());
    assert!(PasswordVaultStore::initialize(&base.join("linked").join("child")).is_err());
    // Replacing an ancestor cannot redirect a previously pinned store handle.
    stdfs::rename(base.join("vaults"), base.join("original"))?;
    stdfs::create_dir(base.join("vaults"))?;
    assert_eq!(
        store.load(document.id()).err(),
        Some(Error::UnsupportedVersion)
    );
    assert!(!base.join("vaults").join(filename(document.id())).exists());
    Ok(())
}

#[test]
fn exclusive_store_coordination_and_password_specific_deletion_gate()
-> Result<(), Box<dyn StdError>> {
    use crate::{
        VaultDeletionPreview, VaultInactivityTimeout, VaultRecoveryConfirmation, VaultSession,
        VaultSessionStatus, VaultTypedConfirmation,
    };
    use std::time::Duration;
    let root = tempfile::tempdir()?;
    let path = root.path().canonicalize()?.join("vaults");
    let store = PasswordVaultStore::initialize(&path)?;
    let second = PasswordVaultStore::initialize(&path)?;
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let (_, document) = fixture(&job)?;
    store.create(&document, &job)?;
    {
        let _lock = store.lock()?;
        assert!(matches!(second.load(document.id()), Err(Error::Busy)));
    }
    let mut session = VaultSession::new(
        document.id().clone(),
        VaultInactivityTimeout::new(Duration::from_mins(1))?,
    );
    let commit = |password: bool| -> Result<_, Box<dyn StdError>> {
        let preview = if password {
            VaultDeletionPreview::new_password_protected(document.id().clone())
        } else {
            VaultDeletionPreview::new(document.id().clone())
        };
        let typed = VaultTypedConfirmation::new(preview.required_confirmation().to_owned())?;
        Ok(preview.confirm(
            VaultRecoveryConfirmation::AllRecoveryMaterialAvailable,
            true,
            typed,
        )?)
    };
    assert!(
        store
            .delete(&document, commit(false)?, &mut session, &job)
            .is_err()
    );
    assert_eq!(store.load(document.id())?, document);
    store.delete(&document, commit(true)?, &mut session, &job)?;
    assert!(matches!(store.load(document.id()), Err(Error::NotFound)));
    assert!(matches!(session.status(), VaultSessionStatus::Locked(_)));
    Ok(())
}
