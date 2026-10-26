use super::{
    EvmPrivateKeyImportError, EvmPrivateKeyImportPreview, EvmPrivateKeyInput,
    IMPORTED_KEY_RECOVERY_WARNING,
};

const PRIVATE_KEY_ONE: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const PRIVATE_KEY_ONE_ADDRESS: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const SECP256K1_ORDER: &str = "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141";

#[test]
fn valid_private_key_previews_expected_address() -> Result<(), EvmPrivateKeyImportError> {
    let preview = preview(&format!("0x{PRIVATE_KEY_ONE}"))?;

    assert_eq!(preview.address().as_str(), PRIVATE_KEY_ONE_ADDRESS);
    assert_eq!(preview.recovery_warning(), IMPORTED_KEY_RECOVERY_WARNING);
    Ok(())
}

#[test]
fn encoding_length_and_characters_are_validated() {
    for invalid in [
        "01",
        "zz00000000000000000000000000000000000000000000000000000000000000",
    ] {
        assert_eq!(
            preview(invalid).err(),
            Some(EvmPrivateKeyImportError::InvalidEncoding)
        );
    }
}

#[test]
fn zero_and_curve_order_are_rejected() {
    let zero = "0".repeat(64);
    for invalid in [zero.as_str(), SECP256K1_ORDER] {
        assert_eq!(
            preview(invalid).err(),
            Some(EvmPrivateKeyImportError::ScalarOutOfRange)
        );
    }
}

#[test]
fn recovery_warning_must_be_acknowledged_before_commit() -> Result<(), EvmPrivateKeyImportError> {
    let preview = preview(PRIVATE_KEY_ONE)?;
    assert_eq!(
        preview.acknowledge_recovery_warning(false).err(),
        Some(EvmPrivateKeyImportError::RecoveryWarningNotAcknowledged)
    );

    let acknowledgement = preview.acknowledge_recovery_warning(true)?;
    let commit = preview.confirm(&acknowledgement)?;
    assert_eq!(commit.address().as_str(), PRIVATE_KEY_ONE_ADDRESS);
    let private_key = commit.into_private_key();
    assert_eq!(private_key.expose_secret().len(), 32);
    assert_eq!(private_key.expose_secret()[31], 1);
    Ok(())
}

fn preview(encoded: &str) -> Result<EvmPrivateKeyImportPreview, EvmPrivateKeyImportError> {
    EvmPrivateKeyImportPreview::validate(EvmPrivateKeyInput::new(encoded.to_owned())?)
}
