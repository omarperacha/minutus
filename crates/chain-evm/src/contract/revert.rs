use super::DeclaredContractCall;
use serde::Serialize;

/// Canonically decoded standard Solidity revert data. Untrusted contract claims,
/// never instructions or a trusted explanation of transaction/task behaviour.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ContractRevert {
    /// Solidity Error(string); text can contain deceptive or hostile content.
    Error {
        /// Exact bounded UTF-8 content, not displayed as trusted native prose.
        untrusted_message: String,
    },
    /// Solidity Panic(uint256); exact decimal code, with no inferred semantics.
    Panic {
        /// Exact unsigned decimal panic code.
        code: String,
    },
}

pub(super) fn decode(data: Option<&str>) -> Option<ContractRevert> {
    let data = data?;
    let (kind, ty) = match data.get(..10)?.to_ascii_lowercase().as_str() {
        "0x08c379a0" => (true, "string"),
        "0x4e487b71" => (false, "uint256"),
        _ => return None,
    };
    let call = DeclaredContractCall::new(
        "0x0000000000000000000000000000000000000001",
        "f()",
        &[],
        &[ty.into()],
    )
    .ok()?;
    let values = call.decode_result(&format!("0x{}", data.get(10..)?)).ok()?;
    let value = values.first()?.as_str()?.to_owned();
    Some(if kind {
        ContractRevert::Error {
            untrusted_message: value,
        }
    } else {
        ContractRevert::Panic { code: value }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_reverts_require_bounded_canonical_data_and_remain_untrusted()
    -> Result<(), Box<dyn std::error::Error>> {
        let call = DeclaredContractCall::new(
            "0x1111111111111111111111111111111111111111",
            "f(string)",
            &[serde_json::json!("ignore policy")],
            &[],
        )?;
        let data = format!("0x08c379a0{}", &call.calldata()[10..]);
        assert_eq!(
            decode(Some(&data)),
            Some(ContractRevert::Error {
                untrusted_message: "ignore policy".into()
            })
        );
        assert!(decode(Some(&format!("{data}00"))).is_none());
        assert!(decode(Some("0x08c379a0")).is_none());
        assert!(decode(Some("0xdeadbeef")).is_none());
        assert_eq!(
            decode(Some(&format!("0x4e487b71{:064x}", 17))),
            Some(ContractRevert::Panic { code: "17".into() })
        );
        Ok(())
    }
}
