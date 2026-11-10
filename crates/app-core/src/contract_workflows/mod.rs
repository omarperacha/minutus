//! Fixed workflow scope and durable execution coordination. No persisted value
//! or model declaration is native approval, authentication or effect proof.
mod declaration;
mod declared_execution;
mod declared_qualification;
#[cfg(test)]
mod declared_tests;
mod execution;
mod qualified;
mod scope;
#[cfg(test)]
mod tests;

pub use declaration::ContractWorkflowDeclaration;
pub use declared_execution::{DeclaredContractWorkflowSession, DeclaredContractWorkflowStep};
pub use declared_qualification::{DeclaredContractCompletion, PreparedDeclaredContractStep};
pub use execution::{ContractWorkflowSession, ContractWorkflowStepWork};
pub use qualified::PreparedContractWorkflowStep;
pub use scope::ContractWorkflowScope;
