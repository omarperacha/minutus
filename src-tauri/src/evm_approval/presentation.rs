//! Trusted projection from an independently verified candidate to native text.

use std::fmt::Write as _;

use app_core::{
    TransactionAuditError, TransactionAuditEvent, TransactionAuditEventKind,
    TransactionAuditOrigin, TransactionAuditTransactionType, TransactionAuditWarningCode,
};
use chain_evm::{
    AuthorizationWarningCode, DecodedSigningPayload, DecodedTransactionAction,
    DecodedTransactionWarningCode, PreparedTransactionCandidate, SigningPayload,
    SigningPayloadDecodeError, SimulationState, decode_and_verify_prepared_transaction,
    decode_signing_payload, eip1559_signing_digest, encode_prepared_transaction,
};
use ipc_types::IpcError;
use sha3::{Digest as _, Keccak256};
use transaction_policy::{APPROVAL_POLICY_REVISION, ApprovalBindingInput, ApprovalTransactionType};

use super::{ApprovalCandidate, ApprovalOrigin};

pub(super) struct ApprovalDisplay {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) has_warnings: bool,
    pub(super) approval_allowed: bool,
}

pub(super) struct PreparedApproval {
    pub(super) display: ApprovalDisplay,
    pub(super) payload: Vec<u8>,
    pub(super) evidence: ApprovalEvidence,
    pub(super) audit_warning_codes: Vec<TransactionAuditWarningCode>,
}

pub(crate) struct ApprovalEvidence {
    signing_digest: [u8; 32],
    chain_id: u64,
    account: String,
    session_id: String,
    expires_at_unix: u64,
    transaction_type: ApprovalTransactionType,
    decoder_digest: [u8; 32],
    simulation_digest: [u8; 32],
}

impl ApprovalEvidence {
    pub(crate) fn binding_input(&self) -> ApprovalBindingInput<'_> {
        ApprovalBindingInput {
            signing_digest: self.signing_digest,
            chain_id: self.chain_id,
            account: &self.account,
            session_id: &self.session_id,
            expires_at_unix: self.expires_at_unix,
            transaction_type: self.transaction_type,
            decoder_digest: self.decoder_digest,
            simulation_digest: self.simulation_digest,
        }
    }

    pub(super) fn audit_event(
        &self,
        transaction_id: &str,
        origin: ApprovalOrigin,
        data: TransactionAuditEventKind,
    ) -> Result<TransactionAuditEvent, TransactionAuditError> {
        let transaction_type = match self.transaction_type {
            ApprovalTransactionType::Eip1559 { .. } => TransactionAuditTransactionType::Eip1559,
            ApprovalTransactionType::Eip712 => TransactionAuditTransactionType::Eip712,
        };
        let origin = match origin {
            ApprovalOrigin::User => TransactionAuditOrigin::User,
            ApprovalOrigin::Chat { .. } => TransactionAuditOrigin::Agent,
        };
        TransactionAuditEvent::new(
            &self.session_id,
            transaction_id,
            &self.account,
            self.chain_id,
            transaction_type,
            Some(self.signing_digest),
            None,
            origin,
            data,
        )
    }
}

struct CandidateContext<'a> {
    prepared: PreparedTransactionCandidate<'a>,
    account: &'a str,
    simulation: &'a chain_evm::TransactionSimulation,
    maximum_network_fee_eth: &'a str,
    maximum_network_fee_wei: &'a str,
    maximum_l1_fee_wei: &'a str,
    operator_fee_wei: &'a str,
}

#[derive(Clone, Copy)]
struct ApprovalBindingDisplay<'a> {
    session_id: &'a str,
    expires_at_unix: u64,
    signing_digest: &'a str,
}

impl ApprovalDisplay {
    pub(super) fn from_candidate(
        candidate: &ApprovalCandidate,
        origin: ApprovalOrigin,
        simulation_is_stale: bool,
        session_id: &str,
        expires_at_unix: u64,
        now_unix: u64,
    ) -> Result<PreparedApproval, IpcError> {
        let context = context(candidate);
        let payload =
            encode_prepared_transaction(context.prepared).map_err(|_| invalid_candidate_error())?;
        let (decoded, approval_allowed) = verified_decoded(&context, &payload)?;
        let simulation = context.simulation;
        let signing_digest = eip1559_signing_digest(&payload);
        let signing_digest_hex = hex_digest(&signing_digest);
        let approval_is_expired = now_unix >= expires_at_unix;
        let warnings = collect_warnings(
            &decoded,
            simulation,
            simulation_is_stale,
            approval_is_expired,
        );
        let audit_warning_codes =
            collect_audit_warning_codes(&decoded, simulation, simulation_is_stale);
        let body = render_body(
            &decoded,
            &context,
            origin,
            simulation_is_stale,
            &warnings,
            ApprovalBindingDisplay {
                session_id,
                expires_at_unix,
                signing_digest: &signing_digest_hex,
            },
        );
        let approval_allowed = approval_allowed && !approval_is_expired;

        let display = Self {
            title: if !approval_allowed {
                "⚠ Cannot approve — evidence is stale or discrepant".to_owned()
            } else if warnings.is_empty() {
                "Review exact transaction".to_owned()
            } else {
                format!("⚠ {} warning(s) — review exact transaction", warnings.len())
            },
            body,
            has_warnings: !warnings.is_empty(),
            approval_allowed,
        };
        let decoded_bytes = serde_json::to_vec(&decoded).map_err(|_| invalid_candidate_error())?;
        let simulation_bytes =
            serde_json::to_vec(simulation).map_err(|_| invalid_candidate_error())?;
        let decoder_digest = digest_parts(b"zeta.approval-decoder.v1", &[&decoded_bytes]);
        let simulation_digest = digest_parts(
            b"zeta.approval-simulation.v1",
            &[
                &simulation_bytes,
                context.maximum_network_fee_eth.as_bytes(),
                context.maximum_network_fee_wei.as_bytes(),
                context.maximum_l1_fee_wei.as_bytes(),
                context.operator_fee_wei.as_bytes(),
            ],
        );
        Ok(PreparedApproval {
            display,
            payload,
            evidence: ApprovalEvidence {
                signing_digest,
                chain_id: decoded.chain_id(),
                // Display keeps EIP-55 spelling; signing binds the same address
                // in the wallet signer's canonical lowercase representation.
                account: signing_account(&decoded)?,
                session_id: session_id.to_owned(),
                expires_at_unix,
                transaction_type: ApprovalTransactionType::Eip1559 {
                    transaction_type: decoded.transaction_type(),
                },
                decoder_digest,
                simulation_digest,
            },
            audit_warning_codes,
        })
    }
}

fn signing_account(decoded: &chain_evm::DecodedEip1559Transaction) -> Result<String, IpcError> {
    chain_evm::PublicEvmAddress::new(decoded.account())
        .map(|account| account.as_str().to_owned())
        .map_err(|_| invalid_candidate_error())
}

#[cfg(test)]
#[path = "presentation/tests.rs"]
mod tests;

fn collect_audit_warning_codes(
    decoded: &chain_evm::DecodedEip1559Transaction,
    simulation: &chain_evm::TransactionSimulation,
    simulation_is_stale: bool,
) -> Vec<TransactionAuditWarningCode> {
    let mut warnings = Vec::new();
    if decoded.is_testnet() {
        warnings.push(TransactionAuditWarningCode::Testnet);
    }
    warnings.extend(
        decoded
            .warnings()
            .iter()
            .map(|warning| match warning.code() {
                DecodedTransactionWarningCode::UnknownContract => {
                    TransactionAuditWarningCode::UnknownContract
                }
                DecodedTransactionWarningCode::UnknownSelector => {
                    TransactionAuditWarningCode::UnknownSelector
                }
                DecodedTransactionWarningCode::UnexpectedNativeValue => {
                    TransactionAuditWarningCode::UnexpectedNativeValue
                }
            }),
    );
    if let DecodedTransactionAction::TokenApproval { approval } = decoded.action() {
        warnings.extend(
            approval
                .warnings()
                .iter()
                .map(|warning| match warning.code() {
                    AuthorizationWarningCode::UnlimitedApproval => {
                        TransactionAuditWarningCode::UnlimitedApproval
                    }
                    AuthorizationWarningCode::EffectivelyUnlimitedApproval => {
                        TransactionAuditWarningCode::EffectivelyUnlimitedApproval
                    }
                    AuthorizationWarningCode::UnknownContract => {
                        TransactionAuditWarningCode::UnknownContract
                    }
                    AuthorizationWarningCode::SuspiciousSpender => {
                        TransactionAuditWarningCode::SuspiciousSpender
                    }
                    AuthorizationWarningCode::UnrecognizedSpender => {
                        TransactionAuditWarningCode::UnrecognizedSpender
                    }
                    AuthorizationWarningCode::ChainMismatch => {
                        TransactionAuditWarningCode::ChainMismatch
                    }
                    AuthorizationWarningCode::OwnerMismatch => {
                        TransactionAuditWarningCode::OwnerMismatch
                    }
                    AuthorizationWarningCode::UnrecognizedDomain => {
                        TransactionAuditWarningCode::UnrecognizedDomain
                    }
                    AuthorizationWarningCode::ExpiredAuthorization => {
                        TransactionAuditWarningCode::ExpiredAuthorization
                    }
                    AuthorizationWarningCode::ExpiringAuthorization => {
                        TransactionAuditWarningCode::ExpiringAuthorization
                    }
                    AuthorizationWarningCode::UnknownSelector => {
                        TransactionAuditWarningCode::UnknownSelector
                    }
                    AuthorizationWarningCode::UnsupportedTypedData => {
                        TransactionAuditWarningCode::UnsupportedTypedData
                    }
                    AuthorizationWarningCode::UndecodableField => {
                        TransactionAuditWarningCode::UndecodableField
                    }
                }),
        );
    }
    warnings.extend(match simulation.state() {
        SimulationState::Succeeded => None,
        SimulationState::Reverted => Some(TransactionAuditWarningCode::SimulationReverted),
        SimulationState::Mismatch => Some(TransactionAuditWarningCode::SimulationDiscrepancy),
        SimulationState::Inconclusive => Some(TransactionAuditWarningCode::SimulationInconclusive),
        SimulationState::Unavailable => Some(TransactionAuditWarningCode::SimulationUnavailable),
    });
    if simulation_is_stale {
        warnings.push(TransactionAuditWarningCode::SimulationStale);
    }
    warnings.sort_unstable();
    warnings.dedup();
    warnings
}

fn verified_decoded(
    context: &CandidateContext<'_>,
    payload: &[u8],
) -> Result<(chain_evm::DecodedEip1559Transaction, bool), IpcError> {
    match decode_and_verify_prepared_transaction(context.prepared, payload) {
        Ok(decoded) => Ok((decoded, true)),
        Err(SigningPayloadDecodeError::SimulationMismatch) => {
            let decoded = decode_signing_payload(SigningPayload::Eip1559Transaction {
                account: context.account,
                payload,
            })
            .map_err(|_| invalid_candidate_error())?;
            let DecodedSigningPayload::Eip1559Transaction(decoded) = decoded else {
                return Err(invalid_candidate_error());
            };
            Ok((*decoded, false))
        }
        Err(_) => Err(invalid_candidate_error()),
    }
}

fn collect_warnings(
    decoded: &chain_evm::DecodedEip1559Transaction,
    simulation: &chain_evm::TransactionSimulation,
    simulation_is_stale: bool,
    approval_is_expired: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if decoded.is_testnet() {
        warnings.push("TESTNET: assets on this network may have no financial value".to_owned());
    }
    warnings.extend(
        decoded
            .warnings()
            .iter()
            .map(|warning| warning.message().to_owned()),
    );
    if let DecodedTransactionAction::TokenApproval { approval } = decoded.action() {
        warnings.extend(
            approval
                .warnings()
                .iter()
                .map(|warning| warning.message().to_owned()),
        );
    }
    match simulation.state() {
        SimulationState::Succeeded => {}
        SimulationState::Reverted => warnings.push(format!(
            "SIMULATION REVERTED: {}",
            simulation
                .revert_reason()
                .unwrap_or("No revert reason was available")
        )),
        SimulationState::Mismatch => warnings.push(
            "SIMULATION DISCREPANCY: observed effects do not match the decoded transaction"
                .to_owned(),
        ),
        SimulationState::Inconclusive => warnings.push(
            "SIMULATION INCONCLUSIVE: the RPC could not prove all decoded effects".to_owned(),
        ),
        SimulationState::Unavailable => warnings.push(
            "SIMULATION UNAVAILABLE: exact-block execution could not be completed".to_owned(),
        ),
    }
    if simulation_is_stale {
        warnings.push(
            "STALE SIMULATION: this result is more than two minutes old; prepare again for current chain state"
                .to_owned(),
        );
    }
    if approval_is_expired {
        warnings.push(
            "APPROVAL EXPIRED: prepare and simulate the transaction again before approving"
                .to_owned(),
        );
    }
    warnings
}

fn render_body(
    decoded: &chain_evm::DecodedEip1559Transaction,
    context: &CandidateContext<'_>,
    origin: ApprovalOrigin,
    simulation_is_stale: bool,
    warnings: &[String],
    binding: ApprovalBindingDisplay<'_>,
) -> String {
    let simulation = context.simulation;
    let mut body = String::new();
    write_warnings(&mut body, warnings);
    let _ = writeln!(
        body,
        "NETWORK\n{} · {} · Chain ID {}\n",
        decoded.network_name(),
        decoded.caip2_id(),
        decoded.chain_id()
    );
    let _ = writeln!(body, "WALLET ACCOUNT\n{}\n", decoded.account());
    write_action(&mut body, decoded.action());
    write_fees(&mut body, decoded, context);
    let _ = writeln!(body, "\nORIGIN\n{}\n", origin.display_name());
    let _ = writeln!(
        body,
        "APPROVAL BINDING\nSigning digest: {}\nSession: {}\nExpires at Unix second: {}\nPolicy revision: {APPROVAL_POLICY_REVISION}\n",
        binding.signing_digest, binding.session_id, binding.expires_at_unix
    );
    let _ = writeln!(
        body,
        "BOUND SIMULATION\n{}\nState: {}\nBlock: {}\nBlock hash: {}\nCandidate digest: {}",
        simulation.summary(),
        simulation_label(simulation.state(), simulation_is_stale),
        simulation.block_number(),
        simulation.block_hash(),
        simulation.candidate_digest()
    );
    for effect in simulation.effects() {
        let _ = writeln!(body, "Effect: {}", effect.summary());
    }
    let _ = writeln!(
        body,
        "\nTRANSACTION\nType {} · Nonce {} · Gas limit {}",
        decoded.transaction_type(),
        decoded.nonce(),
        decoded.gas_limit()
    );
    body
}

fn digest_parts(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(66);
    encoded.push_str("0x");
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn write_warnings(body: &mut String, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    body.push_str("IMPORTANT WARNINGS\n");
    for warning in warnings {
        let _ = writeln!(body, "• {warning}");
    }
    body.push('\n');
}

fn write_fees(
    body: &mut String,
    decoded: &chain_evm::DecodedEip1559Transaction,
    context: &CandidateContext<'_>,
) {
    let _ = writeln!(
        body,
        "MAXIMUM FEES\n{} ETH total network fee ({} wei)\n{} ETH execution fee ({} wei)",
        context.maximum_network_fee_eth,
        context.maximum_network_fee_wei,
        decoded.maximum_execution_fee_eth(),
        decoded.maximum_execution_fee_wei()
    );
    if context.maximum_l1_fee_wei != "0" || context.operator_fee_wei != "0" {
        let _ = writeln!(
            body,
            "Candidate/simulation fee bounds: {} wei L1 data + {} wei operator fee",
            context.maximum_l1_fee_wei, context.operator_fee_wei
        );
    }
}

fn context(candidate: &ApprovalCandidate) -> CandidateContext<'_> {
    match candidate {
        ApprovalCandidate::Native(candidate) => CandidateContext {
            prepared: PreparedTransactionCandidate::Native(candidate),
            account: candidate.sender(),
            simulation: candidate.simulation(),
            maximum_network_fee_eth: candidate.maximum_network_fee_eth(),
            maximum_network_fee_wei: candidate.maximum_network_fee_wei(),
            maximum_l1_fee_wei: candidate.maximum_l1_fee_wei(),
            operator_fee_wei: candidate.operator_fee_wei(),
        },
        ApprovalCandidate::Token(candidate) => CandidateContext {
            prepared: PreparedTransactionCandidate::Token(candidate),
            account: candidate.sender(),
            simulation: candidate.simulation(),
            maximum_network_fee_eth: candidate.maximum_network_fee_eth(),
            maximum_network_fee_wei: candidate.maximum_network_fee_wei(),
            maximum_l1_fee_wei: candidate.maximum_l1_fee_wei(),
            operator_fee_wei: candidate.operator_fee_wei(),
        },
    }
}

fn write_action(body: &mut String, action: &DecodedTransactionAction) {
    match action {
        DecodedTransactionAction::NativeTransfer {
            recipient,
            value_wei,
            amount_eth,
        } => {
            let _ = writeln!(
                body,
                "ACTION\nSend native asset\n\nASSET / AMOUNT\n{amount_eth} ETH ({value_wei} wei)\n\nRECIPIENT\n{recipient}\n"
            );
        }
        DecodedTransactionAction::TokenTransfer { transfer } => {
            let _ = writeln!(
                body,
                "ACTION\nSend trusted ERC-20 token\n\nASSET / AMOUNT\n{} {} ({} base units)\n\nRECIPIENT\n{}\n\nTOKEN CONTRACT\n{}\n",
                transfer.amount(),
                transfer.symbol(),
                transfer.raw_amount(),
                transfer.recipient(),
                transfer.token_contract()
            );
        }
        DecodedTransactionAction::TokenApproval { approval } => {
            body.push_str("ACTION\nToken approval\n");
            for field in approval.fields() {
                let _ = writeln!(
                    body,
                    "\n{}\n{}",
                    field.label().to_uppercase(),
                    field.value()
                );
            }
            body.push('\n');
        }
        DecodedTransactionAction::UnknownContractCall { contract, selector } => {
            let _ = writeln!(
                body,
                "ACTION\nUnknown contract call\n\nCONTRACT\n{contract}\nSelector: {}\n",
                selector.as_deref().unwrap_or("none")
            );
        }
    }
}

pub(super) const fn simulation_label(
    simulation_state: SimulationState,
    is_stale: bool,
) -> &'static str {
    if is_stale {
        return "STALE";
    }
    match simulation_state {
        SimulationState::Succeeded => "SUCCEEDED",
        SimulationState::Reverted => "REVERTED",
        SimulationState::Mismatch => "DISCREPANCY",
        SimulationState::Inconclusive => "INCONCLUSIVE",
        SimulationState::Unavailable => "UNAVAILABLE",
    }
}

fn invalid_candidate_error() -> IpcError {
    IpcError {
        code: ipc_types::IpcErrorCode::InvalidRequest,
        message: "The retained transaction no longer matches its signing payload or simulation"
            .to_owned(),
        retryable: false,
    }
}
