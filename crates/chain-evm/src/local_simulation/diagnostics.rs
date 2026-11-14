//! Debug-only closed acquisition stages. Never emit provider text, request
//! parameters, endpoint credentials, source/code/calldata or response values.
use crate::rpc::RpcError;
use evm_simulation::SimulationError;
#[derive(Clone, Copy, Debug)]
pub(super) enum Stage {
    LatestHeaderRpc,
    LatestHeaderDecode,
    CanonicalHeaderRpc,
    CanonicalHeaderDecode,
    CanonicalHeightDecode,
    StateReadRpc,
    HeaderDecode,
    AccountProof,
    RepeatedAccountProof,
    StorageProof,
    #[cfg(debug_assertions)]
    AbsentAccount,
    #[cfg(debug_assertions)]
    EmptyCode,
    #[cfg(debug_assertions)]
    AccountCode,
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Method {
    None,
    ChainId,
    BlockByNumber,
    BlockByHash,
    GetProof,
    GetCode,
    Other,
}
impl Method {
    pub(super) fn from_name(name: &str) -> Self {
        match name {
            "eth_chainId" => Self::ChainId,
            "eth_getBlockByNumber" => Self::BlockByNumber,
            "eth_getBlockByHash" => Self::BlockByHash,
            "eth_getProof" => Self::GetProof,
            "eth_getCode" => Self::GetCode,
            _ => Self::Other,
        }
    }
}
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
enum Outcome {
    Ok,
    RpcTransport,
    RpcTimeout,
    RpcHttpStatus,
    RpcResponseTooLarge,
    RpcMalformedResponse,
    RpcRejected,
    Simulation(SimulationError),
}
#[cfg(debug_assertions)]
fn rpc_outcome<T>(result: &Result<T, RpcError>) -> Outcome {
    match result {
        Ok(_) => Outcome::Ok,
        Err(RpcError::Transport) => Outcome::RpcTransport,
        Err(RpcError::TimedOut) => Outcome::RpcTimeout,
        Err(RpcError::HttpStatus(_)) => Outcome::RpcHttpStatus,
        Err(RpcError::ResponseTooLarge) => Outcome::RpcResponseTooLarge,
        Err(RpcError::MalformedResponse) => Outcome::RpcMalformedResponse,
        Err(RpcError::Rejected(_)) => Outcome::RpcRejected,
    }
}
#[cfg(debug_assertions)]
fn line(stage: Stage, method: Method, outcome: Outcome) -> String {
    // SimulationError is a closed data-free enum. RPC numeric payloads and
    // successful response values are deliberately absent from this formatter.
    match outcome {
        Outcome::Simulation(error) => {
            format!("localStateDiagnostic stage={stage:?} method={method:?} simulation={error:?}")
        }
        _ => format!("localStateDiagnostic stage={stage:?} method={method:?} outcome={outcome:?}"),
    }
}
#[cfg(debug_assertions)]
fn emit(stage: Stage, method: Method, outcome: Outcome) {
    if std::env::var_os("MINUTUS_SOURCE_QUALIFICATION_DIAGNOSTICS")
        .is_some_and(|value| value == "1")
    {
        eprintln!("{}", line(stage, method, outcome));
    }
}
pub(super) fn rpc<T>(
    stage: Stage,
    method: Method,
    result: Result<T, RpcError>,
) -> Result<T, RpcError> {
    #[cfg(debug_assertions)]
    emit(stage, method, rpc_outcome(&result));
    #[cfg(not(debug_assertions))]
    let _ = (stage, method);
    result
}
pub(super) fn state<T>(
    stage: Stage,
    method: Method,
    result: Result<T, SimulationError>,
) -> Result<T, SimulationError> {
    #[cfg(debug_assertions)]
    emit(
        stage,
        method,
        match &result {
            Ok(_) => Outcome::Ok,
            Err(error) => Outcome::Simulation(*error),
        },
    );
    #[cfg(not(debug_assertions))]
    let _ = (stage, method);
    result
}
pub(super) fn account(exists: bool, empty: bool) {
    #[cfg(debug_assertions)]
    emit(
        if !exists {
            Stage::AbsentAccount
        } else if empty {
            Stage::EmptyCode
        } else {
            Stage::AccountCode
        },
        Method::None,
        Outcome::Ok,
    );
    #[cfg(not(debug_assertions))]
    let _ = (exists, empty);
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    #[test]
    fn categories_exclude_provider_values_and_rpc_numeric_payloads() {
        let output = line(
            Stage::StateReadRpc,
            Method::from_name("https://credential.invalid/key?secret"),
            rpc_outcome::<()>(&Err(RpcError::Rejected(123_456_789))),
        );
        assert_eq!(
            output,
            "localStateDiagnostic stage=StateReadRpc method=Other outcome=RpcRejected"
        );
        let output = line(
            Stage::StateReadRpc,
            Method::GetProof,
            rpc_outcome(&Ok::<_, RpcError>("secret response and source")),
        );
        assert_eq!(
            output,
            "localStateDiagnostic stage=StateReadRpc method=GetProof outcome=Ok"
        );
        assert!(
            line(
                Stage::AccountProof,
                Method::None,
                Outcome::Simulation(SimulationError::StateUnavailable)
            )
            .ends_with("simulation=StateUnavailable")
        );
    }
    #[test]
    fn tracing_preserves_exact_results_including_closed_errors() {
        for error in [
            RpcError::Transport,
            RpcError::TimedOut,
            RpcError::HttpStatus(403),
            RpcError::ResponseTooLarge,
            RpcError::MalformedResponse,
            RpcError::Rejected(-32000),
        ] {
            assert_eq!(
                rpc::<()>(Stage::LatestHeaderRpc, Method::BlockByNumber, Err(error)),
                Err(error)
            );
        }
        assert_eq!(
            state::<()>(
                Stage::StorageProof,
                Method::None,
                Err(SimulationError::StateUnavailable)
            ),
            Err(SimulationError::StateUnavailable)
        );
        assert_eq!(state(Stage::HeaderDecode, Method::None, Ok(7)), Ok(7));
    }
}
