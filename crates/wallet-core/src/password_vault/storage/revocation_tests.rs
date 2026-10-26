use super::*;
use crate::{
    AppVaultDocument, AppVaultEntry, PasswordVaultWork, RecoverySeed, WalletPassword,
    derive_default_evm_account,
};
struct CancelAfterPublication(PasswordVaultWork);
impl Faults for CancelAfterPublication {
    fn at(&self, phase: Phase) -> Result<(), Error> {
        if phase == Phase::DirectorySync {
            self.0.cancel();
        }
        Ok(())
    }
}
#[test]
fn revocation_after_aggregate_publication_or_unlink_is_commit_uncertain()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().canonicalize()?.join("vault");
    let store = EncryptedDocumentStore::<AppVaultDocument>::initialize(&path)?;
    let seed = RecoverySeed::new([7; 64]);
    let account = derive_default_evm_account(&seed)?.metadata("account-1", 8453)?;
    let entry = AppVaultEntry::recovery(VaultKeyId::new("entry")?, &seed, account)?;
    let password = WalletPassword::new("synthetic atomic aggregate password".to_owned())?;
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let doc = AppVaultDocument::create(VaultKeyId::new("root")?, entry, &password, &job)?;
    assert_eq!(
        store.publish(&doc, None, &job, &CancelAfterPublication(work.clone())),
        Err(Error::CommitUncertain)
    );
    assert_eq!(store.load(doc.id())?, doc);
    drop(job);
    let job = work.begin()?;
    assert_eq!(
        store.remove_record_with_faults(&doc, &job, &CancelAfterPublication(work.clone())),
        Err(Error::CommitUncertain)
    );
    assert!(store.vault_ids()?.is_empty());
    Ok(())
}
