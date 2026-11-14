//! Source response correspondence and exact compiler input selection.
use super::{
    super::GenericError,
    failure::{FailureClock, SourceFailureReason as Reason, SourceFailureStage as Stage},
};
use crate::PublicEvmAddress;
use serde_json::Value;
use solidity_compilation::CompilationInput;
use std::time::Instant;

pub(super) fn parse(
    bytes: &[u8],
    chain: u64,
    target: &PublicEvmAddress,
    started: Instant,
) -> Result<CompilationInput, GenericError> {
    let clock = FailureClock::start(started, Stage::SourceInput);
    let response: Value =
        serde_json::from_slice(bytes).map_err(|_| clock.fail(Reason::JsonDecode))?;
    // These assertions are correspondence checks, never source/compiler authority.
    if response["chainId"].as_str() != Some(&chain.to_string())
        || response["address"]
            .as_str()
            .and_then(|v| PublicEvmAddress::new(v).ok())
            .as_ref()
            .map(PublicEvmAddress::as_str)
            != Some(target.as_str())
    {
        return Err(clock.fail(Reason::SourceSchema));
    }
    let compilation = &response["compilation"];
    let version = compilation["compilerVersion"]
        .as_str()
        .ok_or_else(|| clock.fail(Reason::SourceSchema))?;
    let qualified = compilation["fullyQualifiedName"]
        .as_str()
        .ok_or_else(|| clock.fail(Reason::SourceSchema))?;
    let (file, name) = qualified
        .rsplit_once(':')
        .ok_or_else(|| clock.fail(Reason::SourceSchema))?;
    let input = CompilationInput::new(
        version,
        file,
        name,
        response
            .get("stdJsonInput")
            .ok_or_else(|| clock.fail(Reason::SourceSchema))?
            .clone(),
    )
    .map_err(|error| clock.fail(Reason::Compiler(error)))?;
    clock.success();
    Ok(input)
}
