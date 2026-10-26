//! Public EVM balance reads and exact quantity formatting.

use std::{error::Error, fmt};

use serde_json::json;
use sha3::{Digest, Keccak256};

use crate::{
    BaseNetwork, EvmRpcUrl, RpcChainIdError,
    rpc::{RpcClient, RpcError, Uint256, format_units},
};

const ETH_DECIMALS: u8 = 18;
const USDC_DECIMALS: u8 = 6;
const BASE_MAINNET_USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
const BASE_SEPOLIA_USDC: &str = "0x036cbd53842c5426634e7929541ec2318f3dcf7e";
const ETHEREUM_MAINNET_USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const ETHEREUM_SEPOLIA_USDC: &str = "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238";
const BALANCE_OF_SELECTOR: &str = "70a08231";

/// A validated public EVM account address with no signing capability.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PublicEvmAddress(String);

impl PublicEvmAddress {
    /// Validates and normalizes a 20-byte hexadecimal EVM address.
    ///
    /// # Errors
    ///
    /// Returns [`EvmBalanceError::InvalidAddress`] for any non-address input.
    pub fn new(address: &str) -> Result<Self, EvmBalanceError> {
        let Some(hex) = address.strip_prefix("0x") else {
            return Err(EvmBalanceError::InvalidAddress);
        };
        if hex.len() != 40 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(EvmBalanceError::InvalidAddress);
        }
        let address = Self(format!("0x{}", hex.to_ascii_lowercase()));
        let is_mixed_case = hex.bytes().any(|byte| byte.is_ascii_lowercase())
            && hex.bytes().any(|byte| byte.is_ascii_uppercase());
        if is_mixed_case && address.checksum() != format!("0x{hex}") {
            return Err(EvmBalanceError::InvalidAddress);
        }
        Ok(address)
    }

    /// Returns the normalized lowercase public address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the EIP-55 mixed-case checksum representation.
    #[must_use]
    pub fn checksum(&self) -> String {
        let lowercase = self.0.trim_start_matches("0x");
        let digest = Keccak256::digest(lowercase.as_bytes());
        let mut checksummed = String::with_capacity(42);
        checksummed.push_str("0x");
        for (index, character) in lowercase.bytes().enumerate() {
            let nibble = if index % 2 == 0 {
                digest[index / 2] >> 4
            } else {
                digest[index / 2] & 0x0f
            };
            if character.is_ascii_alphabetic() && nibble >= 8 {
                checksummed.push(char::from(character.to_ascii_uppercase()));
            } else {
                checksummed.push(char::from(character));
            }
        }
        checksummed
    }
}

/// Trusted identity and denomination metadata for an initial EVM asset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BaseAssetDescriptor {
    chain_id: u64,
    symbol: &'static str,
    decimals: u8,
    token_contract: Option<&'static str>,
}

impl BaseAssetDescriptor {
    /// Explicit asset symbol from the trusted application table.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        self.symbol
    }

    /// Fixed number of decimal places used by this asset.
    #[must_use]
    pub const fn decimals(self) -> u8 {
        self.decimals
    }

    /// Trusted token contract, or `None` for the network's native currency.
    #[must_use]
    pub const fn token_contract(self) -> Option<&'static str> {
        self.token_contract
    }
}

/// One exact public asset balance returned by an EVM RPC endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseAssetBalance {
    descriptor: BaseAssetDescriptor,
    raw_amount: String,
    formatted_amount: String,
}

impl BaseAssetBalance {
    /// Trusted asset descriptor used for this read.
    #[must_use]
    pub const fn descriptor(&self) -> BaseAssetDescriptor {
        self.descriptor
    }

    /// Exact base-unit integer represented in decimal notation.
    #[must_use]
    pub fn raw_amount(&self) -> &str {
        &self.raw_amount
    }

    /// Exact human-readable amount with no floating-point conversion.
    #[must_use]
    pub fn formatted_amount(&self) -> &str {
        &self.formatted_amount
    }
}

/// Returns ETH and the chain-specific Circle-issued USDC descriptor.
#[must_use]
pub const fn trusted_assets(network: BaseNetwork) -> [BaseAssetDescriptor; 2] {
    let chain_id = network.descriptor().chain_id();
    let usdc_contract = match network {
        BaseNetwork::Mainnet => BASE_MAINNET_USDC,
        BaseNetwork::Sepolia => BASE_SEPOLIA_USDC,
        BaseNetwork::EthereumMainnet => ETHEREUM_MAINNET_USDC,
        BaseNetwork::EthereumSepolia => ETHEREUM_SEPOLIA_USDC,
    };
    [
        BaseAssetDescriptor {
            chain_id,
            symbol: "ETH",
            decimals: ETH_DECIMALS,
            token_contract: None,
        },
        BaseAssetDescriptor {
            chain_id,
            symbol: "USDC",
            decimals: USDC_DECIMALS,
            token_contract: Some(usdc_contract),
        },
    ]
}

/// Reads ETH and USDC balances after verifying the endpoint's chain identity.
///
/// This operation accepts only public data and has no dependency on wallet
/// vault state, private keys or signing capabilities.
///
/// # Errors
///
/// Returns a closed transport, response, address, asset or chain-identity error.
pub fn fetch_base_balances(
    network: BaseNetwork,
    endpoint: &EvmRpcUrl,
    address: &PublicEvmAddress,
) -> Result<Vec<BaseAssetBalance>, EvmBalanceError> {
    let client = RpcClient::new(endpoint).map_err(EvmBalanceError::from)?;
    let chain_id = client
        .string_result("eth_chainId", &json!([]))
        .map_err(EvmBalanceError::from)?;
    network.validate_rpc_chain_id(&chain_id)?;

    trusted_assets(network)
        .into_iter()
        .map(|asset| {
            validate_asset(network, asset)?;
            let encoded = if let Some(contract) = asset.token_contract {
                let calldata = format!(
                    "0x{BALANCE_OF_SELECTOR}{:0>64}",
                    address.as_str().trim_start_matches("0x")
                );
                client
                    .string_result(
                        "eth_call",
                        &json!([{ "to": contract, "data": calldata }, "latest"]),
                    )
                    .map_err(EvmBalanceError::from)?
            } else {
                client
                    .string_result("eth_getBalance", &json!([address.as_str(), "latest"]))
                    .map_err(EvmBalanceError::from)?
            };
            let amount = if asset.token_contract.is_some() {
                parse_abi_uint256(&encoded)?
            } else {
                parse_quantity(&encoded)?
            };
            let raw_amount = amount.to_decimal();
            let formatted_amount = format_units(&raw_amount, asset.decimals);
            Ok(BaseAssetBalance {
                descriptor: asset,
                raw_amount,
                formatted_amount,
            })
        })
        .collect()
}

fn validate_asset(network: BaseNetwork, asset: BaseAssetDescriptor) -> Result<(), EvmBalanceError> {
    if asset.chain_id != network.descriptor().chain_id() {
        return Err(EvmBalanceError::AssetChainMismatch);
    }
    Ok(())
}

fn parse_quantity(value: &str) -> Result<Uint256, EvmBalanceError> {
    Uint256::from_quantity(value).map_err(|_| EvmBalanceError::MalformedResponse)
}

fn parse_abi_uint256(value: &str) -> Result<Uint256, EvmBalanceError> {
    Uint256::from_abi_word(value).map_err(|_| EvmBalanceError::MalformedResponse)
}

/// Closed failures for public EVM balance reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvmBalanceError {
    /// The public account address is malformed.
    InvalidAddress,
    /// The HTTPS request could not be completed.
    Transport,
    /// The endpoint returned a non-success HTTP status.
    HttpStatus(u16),
    /// The bounded RPC response exceeded the accepted size.
    ResponseTooLarge,
    /// The endpoint returned invalid or unexpected JSON-RPC data.
    MalformedResponse,
    /// The endpoint returned a JSON-RPC error code.
    Rpc(i64),
    /// The endpoint identifies a different chain.
    ChainId(RpcChainIdError),
    /// A token descriptor was used with the wrong chain.
    AssetChainMismatch,
}

impl fmt::Display for EvmBalanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress => formatter.write_str("Enter a valid 0x-prefixed EVM address"),
            Self::Transport => formatter.write_str("The EVM RPC endpoint could not be reached"),
            Self::HttpStatus(status) => {
                write!(formatter, "The EVM RPC endpoint returned HTTP {status}")
            }
            Self::ResponseTooLarge => {
                formatter.write_str("The EVM RPC response exceeded its safety limit")
            }
            Self::MalformedResponse => {
                formatter.write_str("The EVM RPC endpoint returned a malformed response")
            }
            Self::Rpc(code) => write!(
                formatter,
                "The EVM RPC endpoint rejected the balance query (code {code})"
            ),
            Self::ChainId(error) => error.fmt(formatter),
            Self::AssetChainMismatch => formatter
                .write_str("The trusted token configuration does not match the selected chain"),
        }
    }
}

impl Error for EvmBalanceError {}

impl From<RpcChainIdError> for EvmBalanceError {
    fn from(error: RpcChainIdError) -> Self {
        Self::ChainId(error)
    }
}

impl From<RpcError> for EvmBalanceError {
    fn from(error: RpcError) -> Self {
        match error {
            RpcError::Transport => Self::Transport,
            RpcError::HttpStatus(status) => Self::HttpStatus(status),
            RpcError::ResponseTooLarge => Self::ResponseTooLarge,
            RpcError::MalformedResponse => Self::MalformedResponse,
            RpcError::Rejected(code) => Self::Rpc(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_zero_and_asset_decimals_exactly() -> Result<(), EvmBalanceError> {
        assert_eq!(parse_quantity("0x0")?.to_decimal(), "0");
        assert_eq!(format_units("0", 18), "0");
        assert_eq!(format_units("1234567", 6), "1.234567");
        assert_eq!(format_units("1000000", 6), "1");
        assert_eq!(format_units("1", 18), "0.000000000000000001");
        Ok(())
    }

    #[test]
    fn formats_full_uint256_without_float_loss() -> Result<(), EvmBalanceError> {
        let value =
            parse_quantity("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")?;
        assert_eq!(
            value.to_decimal(),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
        Ok(())
    }

    #[test]
    fn rejects_malformed_rpc_quantities() {
        for invalid in [
            "0x",
            "0x00",
            "1",
            "0xgg",
            "0x10000000000000000000000000000000000000000000000000000000000000000",
        ] {
            assert_eq!(
                parse_quantity(invalid),
                Err(EvmBalanceError::MalformedResponse)
            );
        }
    }

    #[test]
    fn trusted_usdc_is_bound_to_each_selected_chain() -> Result<(), EvmBalanceError> {
        let mainnet_usdc = trusted_assets(BaseNetwork::Mainnet)[1];
        let sepolia_usdc = trusted_assets(BaseNetwork::Sepolia)[1];
        assert_eq!(mainnet_usdc.decimals(), 6);
        assert_eq!(sepolia_usdc.decimals(), 6);
        assert_ne!(mainnet_usdc.token_contract(), sepolia_usdc.token_contract());
        validate_asset(BaseNetwork::Mainnet, mainnet_usdc)?;
        assert_eq!(
            validate_asset(BaseNetwork::Sepolia, mainnet_usdc),
            Err(EvmBalanceError::AssetChainMismatch)
        );
        Ok(())
    }

    #[test]
    fn ethereum_usdc_contracts_are_chain_specific() {
        let mainnet = trusted_assets(BaseNetwork::EthereumMainnet)[1];
        let sepolia = trusted_assets(BaseNetwork::EthereumSepolia)[1];
        assert_eq!(
            mainnet.token_contract(),
            Some("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48")
        );
        assert_eq!(
            sepolia.token_contract(),
            Some("0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238")
        );
    }

    #[test]
    fn public_address_validation_requires_exact_twenty_bytes() -> Result<(), EvmBalanceError> {
        let address = PublicEvmAddress::new("0x9858effd232b4033e47d90003d41ec34ecaeda94")?;
        assert_eq!(
            address.as_str(),
            "0x9858effd232b4033e47d90003d41ec34ecaeda94"
        );
        assert_eq!(
            address.checksum(),
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
        );
        assert_eq!(
            PublicEvmAddress::new("9858effd"),
            Err(EvmBalanceError::InvalidAddress)
        );
        assert_eq!(
            PublicEvmAddress::new("0x9858EfFd232B4033E47D90003D41EC34EcaEda94"),
            Err(EvmBalanceError::InvalidAddress)
        );
        Ok(())
    }
}
