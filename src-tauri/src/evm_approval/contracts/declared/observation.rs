//! Receipt observations, canonical inclusion and completion remain separate.
mod monitor;
mod proof;
mod state;
mod verification;
pub(super) use verification::wait_and_verify;

use super::context::error;
use app_core::{DeclaredContractCompletion, PreparedDeclaredContractStep};
use chain_evm::contract::generic::GenericError;
use ipc_types::IpcError;
use std::sync::Arc;

pub(super) struct CompletedStep {
    pub(super) step: Arc<PreparedDeclaredContractStep>,
    pub(super) completion: Arc<DeclaredContractCompletion>,
}
pub(super) enum ObservationFailure {
    Unsatisfied,
    Unknown(IpcError),
}
impl From<GenericError> for ObservationFailure {
    fn from(failure: GenericError) -> Self {
        if failure == GenericError::ConditionUnsatisfied {
            Self::Unsatisfied
        } else {
            Self::Unknown(error(failure))
        }
    }
}
impl From<IpcError> for ObservationFailure {
    fn from(error: IpcError) -> Self {
        Self::Unknown(error)
    }
}
