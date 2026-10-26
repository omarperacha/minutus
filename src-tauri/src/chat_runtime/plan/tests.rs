use std::collections::BTreeMap;

use ipc_types::{
    TaskActivity, TaskActivityKind, ToolCallId, ToolRequest, ToolResult, WorkspaceGrantId,
    WorkspacePermissionLevel,
};

use super::*;
use ipc_types::JsonValue;

mod plan_fixtures;
mod result_fixtures;

use plan_fixtures::*;
use result_fixtures::*;

mod admission_basics;
mod forecasting;
mod progress;
mod revision;
mod semantic_contracts;
mod semantic_evidence;
mod semantic_routing;
mod semantic_v6;
