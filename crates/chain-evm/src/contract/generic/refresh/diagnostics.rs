//! Host-only refresh comparisons expose no values or signed-worker diagnostics.
use super::GenericError;

#[derive(Clone, Copy, Debug)]
pub(super) enum Mismatch {
    Body,
    Envelope,
    Source,
    Material,
    FeeBudget,
    TotalDebitBudget,
}

pub(super) fn reject(reason: Mismatch) -> GenericError {
    #[cfg(debug_assertions)]
    if std::env::var_os("MINUTUS_SOURCE_QUALIFICATION_DIAGNOSTICS")
        .is_some_and(|value| value == "1")
    {
        eprintln!("{}", line(reason));
    }
    #[cfg(not(debug_assertions))]
    let _ = reason;
    GenericError::SnapshotMismatch
}

#[cfg(debug_assertions)]
fn line(reason: Mismatch) -> String {
    format!("genericRefreshDiagnostic mismatch={reason:?}")
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn comparison_categories_are_fixed_and_preserve_the_closed_error() {
        for (reason, expected) in [
            (Mismatch::Body, "Body"),
            (Mismatch::Envelope, "Envelope"),
            (Mismatch::Source, "Source"),
            (Mismatch::Material, "Material"),
            (Mismatch::FeeBudget, "FeeBudget"),
            (Mismatch::TotalDebitBudget, "TotalDebitBudget"),
        ] {
            assert_eq!(
                line(reason),
                format!("genericRefreshDiagnostic mismatch={expected}")
            );
            assert_eq!(reject(reason), GenericError::SnapshotMismatch);
        }
    }
}
