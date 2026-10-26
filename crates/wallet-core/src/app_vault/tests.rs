use super::*;
use crate::*;

fn entry(id: &str, seed: u8) -> Result<AppVaultEntry, Box<dyn std::error::Error>> {
    let seed = RecoverySeed::new([seed; 64]);
    let metadata = derive_default_evm_account(&seed)?.metadata("account-1", 8453)?;
    Ok(AppVaultEntry::recovery(
        VaultKeyId::new(id)?,
        &seed,
        metadata,
    )?)
}
#[test]
fn shared_password_atomic_mutations_and_global_rewrap() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let store = AppVaultStore::initialize(&temp.path().canonicalize()?.join("vault"))?;
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let password = WalletPassword::new("synthetic shared app password".to_owned())?;
    let replacement = WalletPassword::new("synthetic replacement password".to_owned())?;
    let first = AppVaultDocument::create(
        VaultKeyId::new("app-root")?,
        entry("first", 7)?,
        &password,
        &job,
    )?;
    store.create(&first, &job)?;
    let mut opened = first.authenticate(&password, &job)?;
    opened.add(entry("second", 8)?)?;
    let input = EvmPrivateKeyInput::new("42".repeat(32))?;
    let preview = EvmPrivateKeyImportPreview::validate(input)?;
    let metadata = AccountMetadata::new(
        "account-1",
        preview.address().as_str(),
        8453,
        AccountKeyOrigin::ImportedPrivateKey,
    )?;
    let ack = preview.acknowledge_recovery_warning(true)?;
    opened.add(AppVaultEntry::imported(
        VaultKeyId::new("third")?,
        preview.confirm(&ack)?,
        metadata,
    )?)?;
    let all = AppVaultDocument::seal(&opened, &password, &job)?;
    store.replace(&first, &all, &job)?;
    assert_eq!(all.inventory()?.len(), 3);
    assert_eq!(
        store.replace(&first, &all, &job),
        Err(PasswordVaultError::Conflict)
    );
    let changed = all.change_password(&password, &replacement, &job)?;
    store.replace(&all, &changed, &job)?;
    assert_eq!(all.inventory()?, changed.inventory()?);
    let mut opened = changed.authenticate(&replacement, &job)?;
    for (id, meta) in changed.inventory()? {
        assert_eq!(opened.signer(&id)?.account().as_str(), meta.address());
    }
    assert!(changed.authenticate(&password, &job).is_err());
    opened.remove(&VaultKeyId::new("second")?)?;
    let removed = AppVaultDocument::seal(&opened, &replacement, &job)?;
    store.replace(&changed, &removed, &job)?;
    assert_eq!(removed.inventory()?.len(), 2);
    let bytes = removed.to_json()?;
    assert_eq!(AppVaultDocument::from_json(&bytes)?, removed);
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    for path in ["/catalogue/tag", "/wrapping/tag", "/kdf/salt"] {
        let mut tampered = value.clone();
        if let Some(v) = tampered.pointer_mut(path) {
            let original = v.as_str().ok_or("field")?;
            let mut field = original.as_bytes().to_vec();
            field[0] = if field[0] == b'A' { b'B' } else { b'A' };
            *v = serde_json::Value::String(String::from_utf8(field)?);
        }
        let mut encoded = removed.to_json()?;
        // Preserve canonical order by replacing only the fixed base64 field bytes.
        let old = value
            .pointer(path)
            .and_then(serde_json::Value::as_str)
            .ok_or("old")?;
        let new = tampered
            .pointer(path)
            .and_then(serde_json::Value::as_str)
            .ok_or("new")?;
        let text = String::from_utf8(std::mem::take(&mut encoded))?.replacen(old, new, 1);
        let bad = AppVaultDocument::from_json(text.as_bytes())?;
        assert!(bad.authenticate(&replacement, &job).is_err());
    }
    let mut noncanonical = bytes.clone();
    noncanonical.push(b' ');
    assert!(AppVaultDocument::from_json(&noncanonical).is_err());
    work.cancel();
    assert!(removed.authenticate(&replacement, &job).is_err());
    Ok(())
}
#[test]
fn duplicate_identity_or_address_cannot_replace_an_existing_entry()
-> Result<(), Box<dyn std::error::Error>> {
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let password = WalletPassword::new("synthetic duplicate passphrase".to_owned())?;
    let doc = AppVaultDocument::create(
        VaultKeyId::new("root")?,
        entry("first", 7)?,
        &password,
        &job,
    )?;
    let mut opened = doc.authenticate(&password, &job)?;
    assert_eq!(
        opened.add(entry("second", 7)?),
        Err(PasswordVaultError::Conflict)
    );
    assert_eq!(
        opened.add(entry("first", 8)?),
        Err(PasswordVaultError::Conflict)
    );
    assert_eq!(opened.entries().len(), 1);
    assert_eq!(opened.entries()[0].id(), &VaultKeyId::new("first")?);
    assert_eq!(opened.entries()[0].account(), &doc.inventory()?[0].1);
    Ok(())
}
