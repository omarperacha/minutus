//! Synthetic password-vault trajectory across crypto, policy and durable storage.
#![cfg(all(feature = "password-vault-prototype", unix))]

use app_core::AppCore;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chain_evm::{EvmNetwork, SigningPayload, decode_signing_payload, eip1559_signing_digest};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    error::Error,
    fmt::Write as _,
    time::{Duration, Instant},
};
use transaction_policy::{ApprovalBindingInput, ApprovalCommitment, ApprovalTransactionType};
use wallet_core::{
    EvmSigningObservation, PasswordVaultDocument, PasswordVaultStore, PasswordVaultWork,
    RecoveryPhrase, RecoverySeed, RecoveryWordAnswers, VaultInactivityTimeout, VaultKeyId,
    VaultSession, VaultSessionStatus, WalletPassword, derive_default_evm_account,
};
use x402_client::{
    ActiveX402WalletAccount, X402HttpMethod, admit_payment_requirement,
    parse_payment_required_response, prepare_x402_payment_authorization,
};
use zeroize::Zeroizing;

fn verified_test_seed() -> Result<RecoverySeed, Box<dyn Error>> {
    // Test binary only. Interactive builds have no automatic phrase confirmation.
    let mut phrase = RecoveryPhrase::generate()?;
    let words = phrase.present_once_for_onboarding(|words| {
        Zeroizing::new(
            words
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    })?;
    let prompt = phrase.begin_verification()?;
    let acknowledgement = prompt.acknowledge_warnings(true, true, true)?;
    let answers = prompt
        .positions()
        .map(|position| words[position.number() - 1].clone());
    phrase.verify_recovery_words(
        &prompt,
        &acknowledgement,
        RecoveryWordAnswers::new(answers)?,
    )?;
    Ok(phrase.derive_seed()?)
}

#[test]
fn password_vault_create_reopen_exact_signatures_and_durable_replay() -> Result<(), Box<dyn Error>>
{
    let root = tempfile::tempdir()?;
    let base = root.path().canonicalize()?;
    let core = AppCore::initialize(base.join("app"))?;
    let store = PasswordVaultStore::initialize(&base.join("password-wallets"))?;
    let (password, credential) = synthetic_password()?;
    let work = PasswordVaultWork::new();
    let job = work.begin()?;
    let seed = verified_test_seed()?;
    let account = derive_default_evm_account(&seed)?;
    let document = PasswordVaultDocument::create_recovery(
        VaultKeyId::new("synthetic-password-wallet")?,
        &password,
        &seed,
        vec![account.metadata("account-1", 84532)?],
        &job,
    )?;
    store.create(&document, &job)?;
    drop(job);
    let document = store.load(document.id())?;
    let job = work.begin()?;
    let mut session = VaultSession::new(
        document.id().clone(),
        VaultInactivityTimeout::new(Duration::from_mins(1))?,
    );
    let key = document.authenticate_unlock(&password, &job)?.install(
        &mut session,
        Instant::now(),
        &job,
    )?;
    drop(job);
    assert_eq!(key.account(), account.address());

    let payload = hex(concat!(
        "02ea83014a3480843b9aca00847735940082520894",
        "1111111111111111111111111111111111111111",
        "0180c0"
    ))?;
    let decoded = decode_signing_payload(SigningPayload::Eip1559Transaction {
        account: account.address().as_str(),
        payload: &payload,
    })?;
    let transaction_binding = ApprovalBindingInput {
        signing_digest: eip1559_signing_digest(&payload),
        chain_id: 84532,
        account: account.address().as_str(),
        session_id: "synthetic-password-transaction",
        expires_at_unix: 1060,
        transaction_type: ApprovalTransactionType::Eip1559 {
            transaction_type: 2,
        },
        decoder_digest: Sha256::digest(serde_json::to_vec(&decoded)?).into(),
        simulation_digest: Sha256::digest(b"synthetic:no-rpc:no-broadcast:no-funds").into(),
    };
    let typed = typed_candidate(account.address().as_str())?;
    let typed_binding =
        typed.approval_binding_input("synthetic-password-typed", 1060, [0x43; 32])?;
    for binding in [transaction_binding, typed_binding] {
        let job = work.begin()?;
        let approval = ApprovalCommitment::issue(binding, 1000)?;
        let mut changed = binding;
        changed.signing_digest[0] ^= 1;
        assert!(approval.verify(changed, 1001).is_err());
        let mut changed = binding;
        changed.chain_id += 1;
        assert!(approval.verify(changed, 1001).is_err());
        let mut changed = binding;
        changed.account = "0x1111111111111111111111111111111111111111";
        assert!(approval.verify(changed, 1001).is_err());
        let mut changed = binding;
        changed.decoder_digest[0] ^= 1;
        assert!(approval.verify(changed, 1001).is_err());
        let mut changed = binding;
        changed.simulation_digest[0] ^= 1;
        assert!(approval.verify(changed, 1001).is_err());
        let mut changed = binding;
        changed.session_id = "another-session";
        assert!(approval.verify(changed, 1001).is_err());
        assert!(approval.verify(binding, 1060).is_err());
        let signature = document.signing_attempt(&password, &job).sign(
            &key,
            &approval,
            &mut session,
            &core,
            1001,
            || EvmSigningObservation::new(binding, 1002, Instant::now(), true),
        )?;
        signature.verify(&binding.signing_digest, binding.account)?;
        // Successful transaction/x402 signing refreshes the existing session;
        // it does not impose a prototype-only lock between wallet operations.
        assert_eq!(session.status(), VaultSessionStatus::Unlocked);
        let mut wrong_digest = binding.signing_digest;
        wrong_digest[0] ^= 1;
        assert!(signature.verify(&wrong_digest, binding.account).is_err());
        // Restarted durable storage still denies the exact already-consumed ID.
        let restarted = AppCore::initialize(base.join("app"))?;
        assert!(approval.consume(binding, 1003, &restarted).is_err());
    }
    let file =
        std::fs::read(base.join("password-wallets/synthetic-password-wallet.password-vault.json"))?;
    assert!(!String::from_utf8_lossy(&file).contains(credential.as_str()));
    println!("password_vault_synthetic_create_reopen_eip1559_eip712_verified");
    Ok(())
}

fn synthetic_password() -> Result<(WalletPassword, Zeroizing<String>), Box<dyn Error>> {
    let mut entropy = Zeroizing::new([0_u8; 32]);
    getrandom::fill(entropy.as_mut())?;
    let mut credential = Zeroizing::new(String::with_capacity(64));
    for byte in entropy.iter() {
        write!(&mut *credential, "{byte:02x}")?;
    }
    // Test-only zeroising canary retained solely to check encrypted storage.
    let password = WalletPassword::new(credential.to_string())?;
    Ok((password, credential))
}

fn typed_candidate(
    account: &str,
) -> Result<x402_client::PreparedX402Authorization, Box<dyn Error>> {
    let value: Value = json!({"x402Version":2,"resource":{"url":"https://pay.flowzed.com/premium-data?market=eth"},"accepts":[{
        "scheme":"exact","network":"eip155:84532","amount":"1","asset":"0x036cbd53842c5426634e7929541ec2318f3dcf7e",
        "payTo":"0x209693bc6afc0c5328ba36faf03c514ef312287c","maxTimeoutSeconds":120,"extra":{"name":"USDC","version":"2"}}]});
    let encoded = STANDARD.encode(serde_json::to_vec(&value)?);
    let challenge = parse_payment_required_response(
        402,
        &[encoded.as_bytes()],
        "https://pay.flowzed.com/premium-data?market=eth",
        1000,
    )?;
    let admitted = admit_payment_requirement(&challenge, 0, 1000)?;
    let active = ActiveX402WalletAccount::new(account, EvmNetwork::BaseSepolia)?;
    Ok(prepare_x402_payment_authorization(
        &admitted,
        X402HttpMethod::Get,
        [0x42; 32],
        Some(&active),
        1000,
    )?)
}

fn hex(input: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
        .collect()
}
