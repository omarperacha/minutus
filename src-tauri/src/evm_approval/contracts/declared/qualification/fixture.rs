//! Fixed public Storage invocation and a fresh, disposable workflow journal.
use super::super::request::DeclaredWorkflowRequest;
use app_core::{AppCore, DeclaredContractWorkflowSession};
use ipc_types::IpcError;
use serde_json::json;
use std::{path::Path, sync::Arc};

pub(super) const CALLER: &str = "0xdc61120438889f790111f72c8f9ca2054a0654d3";
pub(super) const TARGET: &str = "0x2738d13e81e30bc615766a0410e7cf199fd59a83";

pub(super) struct Fixture {
    pub(super) session: DeclaredContractWorkflowSession,
}
impl Fixture {
    fn new(directory: &Path) -> Result<Self, IpcError> {
        let core = Arc::new(AppCore::initialize(directory).map_err(|_| IpcError::internal())?);
        let project = core
            .create_project("Public preparation qualification", "2026-11-11T00:00:00Z")
            .map_err(|_| IpcError::internal())?;
        let conversation = core
            .create_project_conversation(&project.id, "2026-11-11T00:00:00Z")
            .map_err(|_| IpcError::internal())?;
        let conversation = core
            .append_user_message(
                &conversation.id,
                "Prepare the fixed public unsigned Storage fixture; do not sign or submit".into(),
                vec![],
                "2026-11-11T00:00:01Z",
            )
            .map_err(|_| IpcError::internal())?;
        let message = conversation
            .messages
            .last()
            .ok_or_else(IpcError::internal)?;
        let attempt = core
            .claim_contract_workflow_attempt(&conversation.id, &message.id)
            .map_err(|_| IpcError::internal())?
            .ok_or_else(IpcError::internal)?;
        let session = attempt
            .start_declared(request()?.declaration(CALLER)?)
            .map_err(|_| IpcError::internal())?
            .ok_or_else(IpcError::internal)?;
        Ok(Self { session })
    }
}

// AppCore owns blocking HTTP/runtime lifetimes. Construct and retain its last
// owner outside the async bridge; the callback only borrows the fixture, so
// both successful and failed futures return before the core is dropped.
pub(super) fn with<T>(
    directory: &Path,
    operation: impl FnOnce(&Fixture) -> Result<T, IpcError>,
) -> Result<T, IpcError> {
    let fixture = Fixture::new(directory)?;
    operation(&fixture)
}

fn request() -> Result<DeclaredWorkflowRequest, IpcError> {
    let condition = json!({"contract":"storage","read":{"kind":"sourceField","path":["number"]},"comparison":"equal","expected":"42"});
    let prerequisite = json!({"contract":"storage","read":{"kind":"sourceField","path":["number"]},"comparison":"equal","expected":"1"});
    serde_json::from_value(json!({"contractVersion":2,"network":"ethereumSepolia",
        "contracts":[{"id":"storage","address":TARGET}],
        "steps":[{"id":"store","contract":"storage","function":"store(uint256)","arguments":["42"],
            "valueWei":"0","dependsOn":[],"prerequisites":[prerequisite],"completion":[condition.clone()]}],
        "goals":[condition]}))
    .map_err(|_| IpcError::internal())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ephemeral_core_lifetime_stays_outside_the_async_bridge() -> Result<(), IpcError> {
        let directory = tempfile::tempdir().map_err(|_| IpcError::internal())?;
        for fail in [false, true] {
            let result = with(
                &directory.path().join(format!("attempt-{fail}")),
                |fixture| {
                    tauri::async_runtime::block_on(async {
                        assert!(tokio::runtime::Handle::try_current().is_ok());
                        assert_eq!(
                            fixture
                                .session
                                .steps()
                                .map_err(|_| IpcError::internal())?
                                .len(),
                            1
                        );
                        if fail {
                            Err(IpcError::invalid_request("fixture failure"))
                        } else {
                            Ok(())
                        }
                    })
                },
            );
            assert_eq!(result.is_err(), fail);
            assert!(tokio::runtime::Handle::try_current().is_err());
        }
        Ok(())
    }
    #[test]
    fn journal_uses_only_fixed_unowned_public_account_and_never_records_a_send()
    -> Result<(), IpcError> {
        let directory = tempfile::tempdir().map_err(|_| IpcError::internal())?;
        let fixture = Fixture::new(directory.path())?;
        let declaration = fixture.session.declaration();
        assert_eq!(declaration.intent().account, CALLER);
        assert_eq!(declaration.intent().contracts[0].address, TARGET);
        assert_eq!(declaration.intent().steps[0].function, "store(uint256)");
        assert_eq!(declaration.intent().steps[0].arguments, vec![json!("42")]);
        assert_eq!(
            declaration.intent().steps[0].prerequisites[0].expected,
            json!("1")
        );
        let steps = fixture.session.steps().map_err(|_| IpcError::internal())?;
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].state, app_core::ContractStepState::Planned);
        assert!(steps[0].transaction_hash.is_none());
        Ok(())
    }
}
