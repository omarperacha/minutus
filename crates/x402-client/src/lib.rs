//! Bounded x402 v2 client protocol without direct signing authority.

mod approval;
mod payment;
mod policy;
mod preparation;
mod response;

pub use approval::{
    MAX_X402_AGENT_REASON_BYTES, PreparedX402Approval, X402_APPROVAL_LIFETIME_SECONDS,
    X402_EXPIRY_WARNING_SECONDS, X402_UNUSUAL_PRICE_ATOMIC_UNITS, X402AgentReason,
    X402ApprovalDecision, X402ApprovalError, X402ApprovalPresentation, X402ApprovalWarning,
    prepare_x402_payment_approval,
};
pub use payment::{
    MAX_X402_PAYMENT_HEADER_BYTES, ValidatedX402PaymentResponse, X402PaymentError,
    X402PaymentSignatureHeader, create_x402_payment_signature_header,
    validate_x402_payment_response,
};
#[cfg(debug_assertions)]
pub use policy::admit_debug_loopback_payment_requirement;
pub use policy::{
    AdmittedPaymentRequirement, SupportedX402Scheme, X402_POLICY_REVISION, X402OriginPolicy,
    X402PolicyError, admit_payment_requirement, validate_x402_payment_terms,
};
pub use preparation::{
    ActiveX402WalletAccount, MAX_X402_AUTHORIZATION_LIFETIME_SECONDS,
    MIN_X402_PREPARATION_LIFETIME_SECONDS, PreparedX402Authorization, X402_PREPARATION_REVISION,
    X402AuthorizationFacts, X402HttpMethod, X402PreparationError,
    prepare_x402_payment_authorization, revalidate_prepared_x402_authorization,
};
pub use response::{
    MAX_PAYMENT_REQUIRED_DECODED_BYTES, MAX_PAYMENT_REQUIRED_ENCODED_BYTES,
    MAX_PAYMENT_REQUIREMENTS, PaymentRequiredChallenge, PaymentRequiredParseError,
    PaymentRequirement, ResourceBinding, X402_V2, parse_payment_required_response,
};
