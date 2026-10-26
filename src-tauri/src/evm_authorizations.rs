//! Read-only authorization decoding without wallet or signing authority.

use chain_evm::{
    AuthorizationDecodeError, AuthorizationKind, AuthorizationStatus, AuthorizationWarningCode,
    DecodedAuthorization, WarningSeverity, decode_approval_calldata, decode_typed_data,
};
use ipc_types::{
    EvmAuthorizationDecodeRequest, EvmAuthorizationField, EvmAuthorizationKind,
    EvmAuthorizationPayloadKind, EvmAuthorizationStatus, EvmAuthorizationWarning,
    EvmAuthorizationWarningCode, EvmAuthorizationWarningSeverity, EvmDecodedAuthorization,
    IpcError, IpcErrorCode,
};
use time::OffsetDateTime;

use crate::evm_rpc_settings::chain_network;

#[tauri::command]
pub(crate) fn decode_evm_authorization(
    request: EvmAuthorizationDecodeRequest,
) -> Result<EvmDecodedAuthorization, IpcError> {
    let EvmAuthorizationDecodeRequest {
        network: selected,
        payload_kind,
        owner,
        token_contract,
        payload,
    } = request;
    let network = chain_network(selected);
    let owner = owner.as_deref().filter(|owner| !owner.is_empty());
    let decoded = match payload_kind {
        EvmAuthorizationPayloadKind::ApprovalCalldata => {
            let contract = token_contract
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid("Token contract is required for approval calldata"))?;
            decode_approval_calldata(network, contract, owner, &payload)
        }
        EvmAuthorizationPayloadKind::TypedData => {
            let now = OffsetDateTime::now_utc().unix_timestamp();
            let now = u64::try_from(now).map_err(|_| IpcError::internal())?;
            decode_typed_data(network, owner, &payload, now)
        }
    }
    .map_err(decode_error)?;
    Ok(project(selected, &decoded))
}

fn project(
    network: ipc_types::EvmNetwork,
    decoded: &DecodedAuthorization,
) -> EvmDecodedAuthorization {
    let descriptor = chain_network(network).descriptor();
    EvmDecodedAuthorization {
        network,
        network_name: descriptor.display_name().to_owned(),
        is_testnet: descriptor.is_testnet(),
        kind: match decoded.kind() {
            AuthorizationKind::Approval => EvmAuthorizationKind::Approval,
            AuthorizationKind::Permit => EvmAuthorizationKind::Permit,
            AuthorizationKind::UnknownCalldata => EvmAuthorizationKind::UnknownCalldata,
            AuthorizationKind::UnknownTypedData => EvmAuthorizationKind::UnknownTypedData,
        },
        status: match decoded.status() {
            AuthorizationStatus::Verified => EvmAuthorizationStatus::Verified,
            AuthorizationStatus::Warning => EvmAuthorizationStatus::Warning,
            AuthorizationStatus::Unknown => EvmAuthorizationStatus::Unknown,
        },
        fields: decoded
            .fields()
            .iter()
            .map(|field| EvmAuthorizationField {
                label: field.label().to_owned(),
                value: field.value().to_owned(),
            })
            .collect(),
        warnings: decoded
            .warnings()
            .iter()
            .map(|warning| EvmAuthorizationWarning {
                code: warning_code(warning.code()),
                severity: match warning.severity() {
                    WarningSeverity::Caution => EvmAuthorizationWarningSeverity::Caution,
                    WarningSeverity::Critical => EvmAuthorizationWarningSeverity::Critical,
                    WarningSeverity::Unknown => EvmAuthorizationWarningSeverity::Unknown,
                },
                message: warning.message().to_owned(),
            })
            .collect(),
        raw_payload: decoded.raw_payload().to_owned(),
    }
}

fn warning_code(code: AuthorizationWarningCode) -> EvmAuthorizationWarningCode {
    match code {
        AuthorizationWarningCode::UnlimitedApproval => {
            EvmAuthorizationWarningCode::UnlimitedApproval
        }
        AuthorizationWarningCode::EffectivelyUnlimitedApproval => {
            EvmAuthorizationWarningCode::EffectivelyUnlimitedApproval
        }
        AuthorizationWarningCode::UnknownContract => EvmAuthorizationWarningCode::UnknownContract,
        AuthorizationWarningCode::SuspiciousSpender => {
            EvmAuthorizationWarningCode::SuspiciousSpender
        }
        AuthorizationWarningCode::UnrecognizedSpender => {
            EvmAuthorizationWarningCode::UnrecognizedSpender
        }
        AuthorizationWarningCode::ChainMismatch => EvmAuthorizationWarningCode::ChainMismatch,
        AuthorizationWarningCode::OwnerMismatch => EvmAuthorizationWarningCode::OwnerMismatch,
        AuthorizationWarningCode::UnrecognizedDomain => {
            EvmAuthorizationWarningCode::UnrecognizedDomain
        }
        AuthorizationWarningCode::ExpiredAuthorization => {
            EvmAuthorizationWarningCode::ExpiredAuthorization
        }
        AuthorizationWarningCode::ExpiringAuthorization => {
            EvmAuthorizationWarningCode::ExpiringAuthorization
        }
        AuthorizationWarningCode::UnknownSelector => EvmAuthorizationWarningCode::UnknownSelector,
        AuthorizationWarningCode::UnsupportedTypedData => {
            EvmAuthorizationWarningCode::UnsupportedTypedData
        }
        AuthorizationWarningCode::UndecodableField => EvmAuthorizationWarningCode::UndecodableField,
    }
}

fn decode_error(error: AuthorizationDecodeError) -> IpcError {
    invalid(&error.to_string())
}

fn invalid(message: &str) -> IpcError {
    IpcError {
        code: IpcErrorCode::InvalidRequest,
        message: message.to_owned(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_contract_fails_without_decoding_or_authority() {
        let result = decode_evm_authorization(EvmAuthorizationDecodeRequest {
            network: ipc_types::EvmNetwork::BaseMainnet,
            payload_kind: EvmAuthorizationPayloadKind::ApprovalCalldata,
            owner: None,
            token_contract: None,
            payload: "0x095ea7b3".to_owned(),
        });
        assert!(matches!(
            result,
            Err(IpcError {
                code: IpcErrorCode::InvalidRequest,
                retryable: false,
                ..
            })
        ));
    }
}
