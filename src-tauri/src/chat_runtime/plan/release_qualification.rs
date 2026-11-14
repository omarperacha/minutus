//! Pure production schema qualification before app setup. No execution owners,
//! wallet, endpoint, native window or signing capabilities are constructed here.
use super::{FORECAST_TOOL, PlanPolicy};

pub(crate) fn run() -> Result<(), String> {
    // The production release gate returns before consulting helper identity.
    // Staged valid helper pins therefore cannot promote this write capability.
    if crate::evm_approval::contracts::available() {
        return Err("The release host enabled contract workflows".into());
    }
    let catalogue = crate::chat_tools::chat_tool_definitions(&[]).map_err(|error| error.message)?;
    if catalogue
        .iter()
        .any(|tool| tool.name == "contract.request_workflow")
    {
        return Err("The release catalogue advertised contract workflows".into());
    }
    let startup = PlanPolicy::default().tools(&catalogue);
    if startup
        .iter()
        .any(|tool| tool.name == "contract.request_workflow")
    {
        return Err("The release startup schema advertised contract workflows".into());
    }
    let forecast = catalogue
        .iter()
        .find(|tool| tool.name == FORECAST_TOOL)
        .ok_or("The release catalogue omitted its forecast schema")?;
    let routes = forecast
        .parameters
        .pointer("/properties/entries/items/properties/routes/items/oneOf")
        .and_then(serde_json::Value::as_array)
        .ok_or("The release forecast omitted its closed operation routes")?;
    if routes.iter().any(|route| {
        route.pointer("/properties/operation/const")
            == Some(&serde_json::json!("contract.request_workflow"))
    }) {
        return Err("The release forecast advertised contract workflows".into());
    }
    Ok(())
}
