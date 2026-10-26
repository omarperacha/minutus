use super::PasswordVaultError;
use zeroize::Zeroizing;

/// Minimum Unicode scalar count for a wallet password.
pub const MIN_WALLET_PASSWORD_CHARACTERS: usize = 15;
/// Maximum accepted UTF-8 size; no silent truncation or normalisation.
pub const MAX_WALLET_PASSWORD_BYTES: usize = 1024;

/// Native-only, non-cloneable, non-formattable and non-serialisable credential.
pub struct WalletPassword(Zeroizing<String>);

impl WalletPassword {
    /// Takes ownership immediately and wipes rejected input too.
    ///
    /// # Errors
    /// Rejects fewer than 15 scalar values, over 1024 bytes, control characters
    /// and all-whitespace input. Every accepted byte is used unchanged.
    pub fn new(input: String) -> Result<Self, PasswordVaultError> {
        let input = Zeroizing::new(input);
        if input.len() > MAX_WALLET_PASSWORD_BYTES
            || input.chars().count() < MIN_WALLET_PASSWORD_CHARACTERS
            || input.chars().any(char::is_control)
            || input.chars().all(char::is_whitespace)
        {
            return Err(PasswordVaultError::InvalidPassword);
        }
        Ok(Self(input))
    }

    /// Compares exact bytes for native confirmation without exporting either value.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        let mut difference = self.0.len() ^ other.0.len();
        for index in 0..self.0.len().max(other.0.len()) {
            difference |= usize::from(
                self.0.as_bytes().get(index).copied().unwrap_or(0)
                    ^ other.0.as_bytes().get(index).copied().unwrap_or(0),
            );
        }
        difference == 0
    }

    pub(super) fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_exact_unicode_and_spaces_without_silent_conversion()
    -> Result<(), PasswordVaultError> {
        let password = WalletPassword::new("  café wallet passphrase  ".to_owned())?;
        assert!(password.matches(&WalletPassword::new(
            "  café wallet passphrase  ".to_owned()
        )?));
        assert!(!password.matches(&WalletPassword::new("café wallet passphrase".to_owned())?));
        assert!(!password.matches(&WalletPassword::new(
            "  cafe\u{301} wallet passphrase  ".to_owned()
        )?));
        for bad in [
            String::new(),
            "x".repeat(14),
            " ".repeat(15),
            "x".repeat(1025),
            "long password\nwith newline".to_owned(),
        ] {
            assert!(WalletPassword::new(bad).is_err());
        }
        assert!(WalletPassword::new("密".repeat(15)).is_ok());
        assert!(WalletPassword::new("x".repeat(1024)).is_ok());
        Ok(())
    }
}
