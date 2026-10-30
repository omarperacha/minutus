//! Bounded unsigned observations and canonical ABI wire facts.
//!
//! Declared ABI types are untrusted interpretation, not source verification or
//! proof of effects. This module deliberately supplies no signing capability.

mod abi;
mod declared_abi;
mod intent;
pub use declared_abi::DeclaredContractCall;
mod observation;
mod revert;
pub use intent::UnsignedContractIntent;
pub use revert::ContractRevert;

pub use abi::{ContractCall, ContractError, ContractScalar};
pub use observation::{
    ContractBlock, ContractCodeObservation, ContractReadObservation, ContractReadOutcome,
    observe_contract_code_with_admission, read_contract_with_admission,
    read_declared_contract_with_admission,
};

mod qualification;
pub use qualification::{
    AllowanceEffect, ContractAction, ContractPredicate, DeploymentProof, PredicateComparison,
    PredicateObservation, QualificationError, QualifiedBaseWriteCandidate,
    QualifiedContractCandidate, QualifiedContractIdentity, QualifiedEffects,
    QualifiedMaterialEffects, QualifiedWriteCandidate, RefreshedQualifiedCandidate,
    TokenBalanceEffect, ValidatedContractExecution, VerifiedReceiptPredicates,
    acquire_contract_transaction_proof, prepare_qualified_base_contract,
    prepare_qualified_contract, prepare_qualified_contract_with_worker,
    qualify_contract_workflow_source, refresh_qualified_contract,
    verify_contract_receipt_predicates,
};
