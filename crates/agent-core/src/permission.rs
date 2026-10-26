mod binding;
mod mediator;

pub use binding::{
    DurableCapability, ExecutionAuthorization, PermissionBinding, PermissionResourceKind,
    PermissionResourceScope,
};
pub use mediator::{
    PermissionDecisionReceipt, PermissionMediationError, PermissionMediator,
    PermissionMediatorSnapshot, PermissionPrompt,
};

pub(crate) use binding::request_arguments_sha256;

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
