//! Fixed synthetic candidates; no RPC, broadcast or caller-controlled raw digest.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chain_evm::{EvmNetwork, SigningPayload, decode_signing_payload, eip1559_signing_digest};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use transaction_policy::{ApprovalBindingInput, ApprovalTransactionType};
use wallet_core::PasswordVaultError as Error;

pub(super) struct SyntheticCandidate {
    pub(super) account: String,
    pub(super) digest: [u8; 32],
    pub(super) now: u64,
    session: String,
    expires: u64,
    decoder: [u8; 32],
    simulation: [u8; 32],
    kind: ApprovalTransactionType,
    pub(super) display: String,
}

impl SyntheticCandidate {
    pub(super) fn pair(account: &str) -> Result<[Self; 2], Error> {
        let now = unix_now();
        let session = format!("synthetic-{}", uuid::Uuid::now_v7());
        let expires = now.checked_add(120).ok_or(Error::InvalidDocument)?;
        let hex = concat!(
            "02ea83014a3480843b9aca00847735940082520894",
            "1111111111111111111111111111111111111111",
            "0180c0"
        );
        let bytes: Vec<u8> = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .ok_or(Error::InvalidDocument)
            })
            .collect::<Result<_, _>>()?;
        let decoded = decode_signing_payload(SigningPayload::Eip1559Transaction {
            account,
            payload: &bytes,
        })
        .map_err(|_| Error::InvalidDocument)?;
        let decoded = serde_json::to_vec(&decoded).map_err(|_| Error::InvalidDocument)?;
        let simulation = Sha256::digest(b"synthetic-only:no-rpc:no-funds:no-broadcast").into();
        let transaction = Self {
            account: account.to_owned(),
            digest: eip1559_signing_digest(&bytes),
            now,
            session: session.clone(),
            expires,
            decoder: Sha256::digest(&decoded).into(),
            simulation,
            kind: ApprovalTransactionType::Eip1559 {
                transaction_type: 2,
            },
            display: format!(
                "Base Sepolia EIP-1559 example\n{}",
                String::from_utf8(decoded).map_err(|_| Error::InvalidDocument)?
            ),
        };
        let resource = "https://pay.flowzed.com/premium-data?market=eth";
        let challenge = json!({"x402Version":2,"resource":{"url":resource},"accepts":[{"scheme":"exact","network":"eip155:84532","amount":"1",
            "asset":"0x036cbd53842c5426634e7929541ec2318f3dcf7e","payTo":"0x209693bc6afc0c5328ba36faf03c514ef312287c","maxTimeoutSeconds":120,"extra":{"name":"USDC","version":"2"}}]});
        let encoded =
            STANDARD.encode(serde_json::to_vec(&challenge).map_err(|_| Error::InvalidDocument)?);
        let challenge =
            x402_client::parse_payment_required_response(402, &[encoded.as_bytes()], resource, now)
                .map_err(|_| Error::InvalidDocument)?;
        let requirement = x402_client::admit_payment_requirement(&challenge, 0, now)
            .map_err(|_| Error::InvalidDocument)?;
        let active = x402_client::ActiveX402WalletAccount::new(account, EvmNetwork::BaseSepolia)
            .map_err(|_| Error::InvalidDocument)?;
        let prepared = x402_client::prepare_x402_payment_authorization(
            &requirement,
            x402_client::X402HttpMethod::Get,
            [0x42; 32],
            Some(&active),
            now,
        )
        .map_err(|_| Error::InvalidDocument)?;
        let typed = Self {
            account: account.to_owned(),
            digest: *prepared.signing_digest(),
            now,
            session,
            expires,
            decoder: *prepared.candidate_digest(),
            simulation,
            kind: ApprovalTransactionType::Eip712,
            display: format!(
                "Base Sepolia EIP-712 / EIP-3009 example\n{}",
                prepared.canonical_typed_data()
            ),
        };
        Ok([transaction, typed])
    }
    pub(super) fn binding(&self) -> ApprovalBindingInput<'_> {
        ApprovalBindingInput {
            signing_digest: self.digest,
            chain_id: 84532,
            account: &self.account,
            session_id: &self.session,
            expires_at_unix: self.expires,
            transaction_type: self.kind,
            decoder_digest: self.decoder,
            simulation_digest: self.simulation,
        }
    }
}
pub(super) fn unix_now() -> u64 {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_candidates_bind_two_distinct_approved_domains() -> Result<(), Error> {
        let pair = SyntheticCandidate::pair("0x1111111111111111111111111111111111111111")?;
        assert_ne!(pair[0].digest, pair[1].digest);
        assert_ne!(
            pair[0].binding().transaction_type,
            pair[1].binding().transaction_type
        );
        Ok(())
    }
}
