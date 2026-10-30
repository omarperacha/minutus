use std::{error::Error, fmt};

use serde::Serialize;
use url::Url;
use zeroize::Zeroizing;

/// Base mainnet EIP-155 chain identifier.
pub const BASE_MAINNET_CHAIN_ID: u64 = 8_453;
/// Base Sepolia EIP-155 chain identifier.
pub const BASE_SEPOLIA_CHAIN_ID: u64 = 84_532;
/// Ethereum mainnet EIP-155 chain identifier.
pub const ETHEREUM_MAINNET_CHAIN_ID: u64 = 1;
/// Ethereum Sepolia EIP-155 chain identifier.
pub const ETHEREUM_SEPOLIA_CHAIN_ID: u64 = 11_155_111;
/// Official rate-limited Base mainnet JSON-RPC endpoint.
pub const BASE_MAINNET_DEFAULT_RPC_URL: &str = "https://mainnet.base.org";
/// Official rate-limited Base Sepolia JSON-RPC endpoint.
pub const BASE_SEPOLIA_DEFAULT_RPC_URL: &str = "https://sepolia.base.org";
/// Official Base mainnet block explorer.
pub const BASE_MAINNET_BLOCK_EXPLORER_URL: &str = "https://base.blockscout.com";
/// Official Base Sepolia block explorer.
pub const BASE_SEPOLIA_BLOCK_EXPLORER_URL: &str = "https://sepolia-explorer.base.org";
/// `PublicNode` Ethereum mainnet endpoint used when no custom URL is configured.
pub const ETHEREUM_MAINNET_DEFAULT_RPC_URL: &str = "https://ethereum-rpc.publicnode.com";
/// `PublicNode` Ethereum Sepolia endpoint used when no custom URL is configured.
pub const ETHEREUM_SEPOLIA_DEFAULT_RPC_URL: &str = "https://ethereum-sepolia-rpc.publicnode.com";
/// Ethereum mainnet block explorer.
pub const ETHEREUM_MAINNET_BLOCK_EXPLORER_URL: &str = "https://etherscan.io";
/// Ethereum Sepolia block explorer.
pub const ETHEREUM_SEPOLIA_BLOCK_EXPLORER_URL: &str = "https://sepolia.etherscan.io";

const ETH_SYMBOL: &str = "ETH";
const MAX_RPC_URL_BYTES: usize = 2_048;

/// Closed set of networks supported by the EVM implementation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EvmNetwork {
    /// Base production network.
    BaseMainnet,
    /// Base public Sepolia test network.
    BaseSepolia,
    /// Ethereum production network.
    EthereumMainnet,
    /// Ethereum public Sepolia test network.
    EthereumSepolia,
}

impl EvmNetwork {
    /// Backward-compatible Base mainnet name retained for existing callers.
    #[allow(non_upper_case_globals)]
    pub const Mainnet: Self = Self::BaseMainnet;
    /// Backward-compatible Base Sepolia name retained for existing callers.
    #[allow(non_upper_case_globals)]
    pub const Sepolia: Self = Self::BaseSepolia;
    /// Supported networks in stable mainnet-then-testnet display order.
    pub const ALL: [Self; 4] = [
        Self::BaseMainnet,
        Self::BaseSepolia,
        Self::EthereumMainnet,
        Self::EthereumSepolia,
    ];

    /// Returns the complete trusted descriptor for this network.
    #[must_use]
    pub const fn descriptor(self) -> EvmNetworkDescriptor {
        match self {
            Self::BaseMainnet => EvmNetworkDescriptor {
                network: self,
                display_name: "Base mainnet",
                caip2_id: "eip155:8453",
                chain_id: BASE_MAINNET_CHAIN_ID,
                native_currency_symbol: ETH_SYMBOL,
                default_rpc_url: BASE_MAINNET_DEFAULT_RPC_URL,
                block_explorer_url: BASE_MAINNET_BLOCK_EXPLORER_URL,
                is_testnet: false,
            },
            Self::BaseSepolia => EvmNetworkDescriptor {
                network: self,
                display_name: "Base Sepolia",
                caip2_id: "eip155:84532",
                chain_id: BASE_SEPOLIA_CHAIN_ID,
                native_currency_symbol: ETH_SYMBOL,
                default_rpc_url: BASE_SEPOLIA_DEFAULT_RPC_URL,
                block_explorer_url: BASE_SEPOLIA_BLOCK_EXPLORER_URL,
                is_testnet: true,
            },
            Self::EthereumMainnet => EvmNetworkDescriptor {
                network: self,
                display_name: "Ethereum mainnet",
                caip2_id: "eip155:1",
                chain_id: ETHEREUM_MAINNET_CHAIN_ID,
                native_currency_symbol: ETH_SYMBOL,
                default_rpc_url: ETHEREUM_MAINNET_DEFAULT_RPC_URL,
                block_explorer_url: ETHEREUM_MAINNET_BLOCK_EXPLORER_URL,
                is_testnet: false,
            },
            Self::EthereumSepolia => EvmNetworkDescriptor {
                network: self,
                display_name: "Ethereum Sepolia",
                caip2_id: "eip155:11155111",
                chain_id: ETHEREUM_SEPOLIA_CHAIN_ID,
                native_currency_symbol: ETH_SYMBOL,
                default_rpc_url: ETHEREUM_SEPOLIA_DEFAULT_RPC_URL,
                block_explorer_url: ETHEREUM_SEPOLIA_BLOCK_EXPLORER_URL,
                is_testnet: true,
            },
        }
    }

    /// Whether the network uses Base-specific L1 and operator fee components.
    #[must_use]
    pub const fn is_base(self) -> bool {
        matches!(self, Self::BaseMainnet | Self::BaseSepolia)
    }

    /// Validates a strict `eth_chainId` quantity against this network.
    ///
    /// # Errors
    ///
    /// Returns a malformed-response error for a non-canonical JSON-RPC quantity
    /// and a mismatch containing only numeric chain identities otherwise.
    pub fn validate_rpc_chain_id(self, response: &str) -> Result<(), RpcChainIdError> {
        let actual = parse_chain_id(response)?;
        let expected = self.descriptor().chain_id;
        if actual != expected {
            return Err(RpcChainIdError::Mismatch { expected, actual });
        }
        Ok(())
    }
}

/// Backward-compatible name for the original Base-only network type.
pub type BaseNetwork = EvmNetwork;

/// Trusted, credential-free identity and presentation metadata for one network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvmNetworkDescriptor {
    network: EvmNetwork,
    display_name: &'static str,
    caip2_id: &'static str,
    chain_id: u64,
    native_currency_symbol: &'static str,
    default_rpc_url: &'static str,
    block_explorer_url: &'static str,
    is_testnet: bool,
}

impl EvmNetworkDescriptor {
    /// Network represented by this descriptor.
    #[must_use]
    pub const fn network(self) -> EvmNetwork {
        self.network
    }

    /// User-visible network name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        self.display_name
    }

    /// CAIP-2 network identifier.
    #[must_use]
    pub const fn caip2_id(self) -> &'static str {
        self.caip2_id
    }

    /// EIP-155 chain identifier.
    #[must_use]
    pub const fn chain_id(self) -> u64 {
        self.chain_id
    }

    /// Native fee-currency symbol.
    #[must_use]
    pub const fn native_currency_symbol(self) -> &'static str {
        self.native_currency_symbol
    }

    /// Official rate-limited endpoint used when no custom endpoint is present.
    #[must_use]
    pub const fn default_rpc_url(self) -> &'static str {
        self.default_rpc_url
    }

    /// Trusted block-explorer base URL.
    #[must_use]
    pub const fn block_explorer_url(self) -> &'static str {
        self.block_explorer_url
    }

    /// Whether this network uses test assets and must be visually distinct.
    #[must_use]
    pub const fn is_testnet(self) -> bool {
        self.is_testnet
    }
}

/// Backward-compatible name for the original Base-only descriptor type.
pub type BaseNetworkDescriptor = EvmNetworkDescriptor;

/// Validated custom HTTPS JSON-RPC URL that may contain provider credentials.
///
/// This type intentionally implements neither `Clone`, `Debug`, formatting nor
/// serialization. Its backing string is zeroized when dropped and must never be
/// written to ordinary diagnostics or projected into model or frontend data.
pub struct EvmRpcUrl(Zeroizing<String>);

impl EvmRpcUrl {
    // Synthetic transport tests only: the public runtime constructor remains
    // HTTPS-only and accepts no exception or insecure-scheme flag.
    #[cfg(test)]
    pub(crate) fn loopback_test(port: u16) -> Self {
        Self(Zeroizing::new(format!("http://127.0.0.1:{port}")))
    }

    /// Validates and takes ownership of a user-supplied custom endpoint.
    ///
    /// Paths, queries and user information are accepted because RPC providers
    /// commonly place credentials there. Fragments are rejected because they
    /// are not sent in HTTP requests and make the effective endpoint ambiguous.
    ///
    /// # Errors
    ///
    /// Returns a closed validation error for empty, oversized, malformed,
    /// non-HTTPS, hostless, fragmented or port-zero URLs.
    pub fn new(value: String) -> Result<Self, EvmRpcUrlError> {
        let value = Zeroizing::new(value);
        if value.is_empty() {
            return Err(EvmRpcUrlError::Empty);
        }
        if value.len() > MAX_RPC_URL_BYTES {
            return Err(EvmRpcUrlError::TooLong);
        }
        if value.chars().any(char::is_whitespace) {
            return Err(EvmRpcUrlError::Malformed);
        }
        let parsed = Url::parse(value.as_str()).map_err(|_| EvmRpcUrlError::Malformed)?;
        if parsed.scheme() != "https" {
            return Err(EvmRpcUrlError::InsecureScheme);
        }
        if parsed.host_str().is_none() {
            return Err(EvmRpcUrlError::MissingHost);
        }
        if parsed.fragment().is_some() {
            return Err(EvmRpcUrlError::FragmentNotAllowed);
        }
        if parsed.port() == Some(0) {
            return Err(EvmRpcUrlError::InvalidPort);
        }
        Ok(Self(value))
    }

    /// Credential-free origin for native outbound review; excludes path/query.
    #[must_use]
    pub fn public_origin(&self) -> String {
        Url::parse(self.0.as_str()).map_or_else(
            |_| "Invalid configured provider".into(),
            |url| url.origin().ascii_serialization(),
        )
    }

    /// Borrows the sensitive URL only for the owning RPC transport operation.
    #[must_use]
    pub fn expose_sensitive(&self) -> &str {
        self.0.as_str()
    }
}

/// Closed custom RPC URL validation failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvmRpcUrlError {
    /// No endpoint was supplied.
    Empty,
    /// The endpoint exceeds its host-owned bound.
    TooLong,
    /// The endpoint is not a valid absolute URL.
    Malformed,
    /// Only TLS-protected HTTPS endpoints are accepted.
    InsecureScheme,
    /// The endpoint has no host.
    MissingHost,
    /// URL fragments do not participate in an HTTP request.
    FragmentNotAllowed,
    /// TCP port zero cannot identify a usable provider.
    InvalidPort,
}

impl fmt::Display for EvmRpcUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "EVM RPC URL is empty",
            Self::TooLong => "EVM RPC URL exceeds its length limit",
            Self::Malformed => "EVM RPC URL is malformed",
            Self::InsecureScheme => "EVM RPC URL must use HTTPS",
            Self::MissingHost => "EVM RPC URL must include a host",
            Self::FragmentNotAllowed => "EVM RPC URL cannot include a fragment",
            Self::InvalidPort => "EVM RPC URL has an invalid port",
        })
    }
}

impl Error for EvmRpcUrlError {}

/// Closed failures when validating an RPC endpoint's reported chain ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcChainIdError {
    /// The response is not a canonical EVM hexadecimal quantity.
    MalformedResponse,
    /// The endpoint reports a different EIP-155 chain ID.
    Mismatch {
        /// Chain selected by the trusted network configuration.
        expected: u64,
        /// Chain reported by the endpoint.
        actual: u64,
    },
}

impl fmt::Display for RpcChainIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedResponse => formatter.write_str("RPC chain ID response is malformed"),
            Self::Mismatch { expected, actual } => {
                write!(
                    formatter,
                    "RPC chain mismatch: expected {expected}, received {actual}"
                )
            }
        }
    }
}

impl Error for RpcChainIdError {}

fn parse_chain_id(value: &str) -> Result<u64, RpcChainIdError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(RpcChainIdError::MalformedResponse)?;
    if digits.is_empty()
        || digits.len() > 16
        || (digits.len() > 1 && digits.starts_with('0'))
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RpcChainIdError::MalformedResponse);
    }
    u64::from_str_radix(digits, 16).map_err(|_| RpcChainIdError::MalformedResponse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_pin_base_identity_and_official_defaults() {
        let mainnet = BaseNetwork::Mainnet.descriptor();
        assert_eq!(mainnet.chain_id(), 8_453);
        assert_eq!(mainnet.caip2_id(), "eip155:8453");
        assert_eq!(mainnet.default_rpc_url(), "https://mainnet.base.org");
        assert_eq!(mainnet.native_currency_symbol(), "ETH");
        assert!(!mainnet.is_testnet());

        let sepolia = BaseNetwork::Sepolia.descriptor();
        assert_eq!(sepolia.chain_id(), 84_532);
        assert_eq!(sepolia.caip2_id(), "eip155:84532");
        assert_eq!(sepolia.default_rpc_url(), "https://sepolia.base.org");
        assert!(sepolia.is_testnet());
    }

    #[test]
    fn descriptors_pin_ethereum_identity_and_public_defaults() {
        let mainnet = EvmNetwork::EthereumMainnet.descriptor();
        assert_eq!(mainnet.chain_id(), 1);
        assert_eq!(mainnet.caip2_id(), "eip155:1");
        assert_eq!(
            mainnet.default_rpc_url(),
            "https://ethereum-rpc.publicnode.com"
        );
        assert_eq!(mainnet.block_explorer_url(), "https://etherscan.io");
        assert!(!mainnet.is_testnet());
        assert!(!EvmNetwork::EthereumMainnet.is_base());

        let sepolia = EvmNetwork::EthereumSepolia.descriptor();
        assert_eq!(sepolia.chain_id(), 11_155_111);
        assert_eq!(sepolia.caip2_id(), "eip155:11155111");
        assert_eq!(
            sepolia.default_rpc_url(),
            "https://ethereum-sepolia-rpc.publicnode.com"
        );
        assert_eq!(sepolia.block_explorer_url(), "https://sepolia.etherscan.io");
        assert!(sepolia.is_testnet());
    }

    #[test]
    fn custom_rpc_url_accepts_sensitive_provider_shapes() -> Result<(), Box<dyn Error>> {
        for value in [
            "https://base.example/rpc/provider-token",
            "https://user:password@base.example/rpc",
            "https://unique-provider-host.example/?key=secret",
        ] {
            let endpoint = EvmRpcUrl::new(value.to_owned())?;
            assert_eq!(endpoint.expose_sensitive(), value);
        }
        Ok(())
    }

    #[test]
    fn custom_rpc_url_requires_an_unambiguous_https_endpoint() {
        for (value, expected) in [
            ("", EvmRpcUrlError::Empty),
            ("not a url", EvmRpcUrlError::Malformed),
            ("http://base.example", EvmRpcUrlError::InsecureScheme),
            ("https://", EvmRpcUrlError::Malformed),
            (
                "https://base.example/rpc#ignored",
                EvmRpcUrlError::FragmentNotAllowed,
            ),
            ("https://base.example:0", EvmRpcUrlError::InvalidPort),
        ] {
            assert_eq!(EvmRpcUrl::new(value.to_owned()).err(), Some(expected));
        }
    }

    #[test]
    fn rpc_chain_id_must_be_canonical_and_match_the_selected_network() {
        assert_eq!(BaseNetwork::Mainnet.validate_rpc_chain_id("0x2105"), Ok(()));
        assert_eq!(
            BaseNetwork::Sepolia.validate_rpc_chain_id("0x14a34"),
            Ok(())
        );
        assert_eq!(
            BaseNetwork::Mainnet.validate_rpc_chain_id("0x14a34"),
            Err(RpcChainIdError::Mismatch {
                expected: 8_453,
                actual: 84_532,
            })
        );
        for malformed in ["8453", "0X2105", "0x", "0x02105", "0xzz"] {
            assert_eq!(
                BaseNetwork::Mainnet.validate_rpc_chain_id(malformed),
                Err(RpcChainIdError::MalformedResponse)
            );
        }
    }
}
