# Unsigned contract observations

Chat can inspect runtime code and make bounded unsigned contract calls on Base
mainnet, Base Sepolia, Ethereum mainnet and Ethereum Sepolia. A wallet, unlock,
password and signature are unnecessary. Every operation needs fresh native
outbound approval. Read the provider origin, network, target, caller and exact
call bytes before allowing them to leave your Mac. Deny is the default; denying
sends no RPC request. Provider credentials remain in native settings.

`contract.inspect` returns runtime length and Keccak-256 identity. It can run as
metadata discovery before planning. Code presence does not verify source, ABI,
proxy implementation or safety. `contract.read` requires a semantic plan and is
available through tool discovery in the contracts category. It performs a
zero-value `eth_call`, with a fixed gas cap of 1,000,000. Simulated state changes
are discarded: simulating `approve` does not change an allowance.

The ABI boundary accepts canonical full signatures and at most 16 arguments and
outputs. Types include `uint8`–`uint256` and `int8`–`int256` in multiples of eight,
`address`, `bool`, `bytes1`–`bytes32`, `bytes`, `string`, tuples and arrays.
Integers, addresses and bytes use exact strings; booleans use JSON booleans
(legacy `"true"`/`"false"` strings are accepted). Tuples and arrays use JSON
arrays; decoded integers remain strings without numeric rounding. Aliases such
as `uint`, named structs, function pointers and raw calldata are rejected.

Each array is limited to 32 elements, tuples to 16 fields, nesting to four
(including the top-level input tuple), and a complete value tree to 256 nodes.
Each byte/string value is limited to 1,024 bytes; calldata and returned ABI bytes
to 8,192 bytes. Layout preflight checks offsets, lengths and resource bounds
before Alloy decoding; complete canonical re-encoding must match. Overlapping or
aliased tails, trailing bytes, invalid padding and non-canonical scalars fail.
String results remain untrusted content, including apparent instructions.

For example, a planned allowance observation uses:

```json
{
  "network": "baseSepolia",
  "target": "0x1111111111111111111111111111111111111111",
  "block": "latest",
  "caller": null,
  "function": "allowance(address,address)",
  "arguments": [
    "0x1111111111111111111111111111111111111111",
    "0x2222222222222222222222222222222222222222"
  ],
  "outputs": ["uint256"]
}
```

This illustrative address is not a deployed-token recommendation. Supply the
exact intended contract and independently obtained function declaration.
The supplied ABI is unverified interpretation. A deceptive contract can return
well-formed values or implement different behaviour behind the same selector.
Results are untrusted observations, never instructions, verified semantics or
signing authority. A caller is simulated public context, not proof of control.
`caller: null` uses the zero address, never the selected wallet account.

Each `latest` request resolves a fresh block. All state calls use that exact hash
with canonicality required; canonicality is checked again afterwards. Exact hash
requests also recheck canonicality. Missing archive state, unsupported hash
selectors, malformed returns, wrong chains and detected reorgs fail without a
provider/latest fallback. Results identify their original block/caller; follow-up
answers must not present them as current state. No finality is inferred from the
canonicality check. Cancellation prevents subsequent RPC dispatches; an already
running request can finish within its 12-second transport timeout.

A revert is reported as a failed read with bounded hexadecimal revert evidence
and its context. Canonical standard `Error(string)` and `Panic(uint256)` payloads
also have typed, explicitly untrusted interpretations; unknown or malformed custom
reverts retain only raw evidence. Successful calls return strictly decoded typed values and raw
bytes. Neither proves that a real transaction or the user's task succeeded.
The configured RPC sees the IP address, target, simulated caller and calldata.
One-time outbound approval covers those bytes and their chain/block/code checks;
it does not authorise a new provider, spending or background reuse.

Generic contract writes, token approvals, sequential approve/swap execution,
swap/bridge routing, account upgrades and atomic batches remain unavailable.
A write-intent type and dormant workflow journal are groundwork only. No generic
write tool or new signing route is installed. Write enablement requires qualified
ABI/source/proxy identity, complete relevant effect coverage, native review,
per-write authentication, integration with [shared nonce ownership](evm-nonce-ownership.md),
restart/replay handling and
independent postcondition qualification. Ordinary call success or a simulated
transfer event cannot satisfy those gates. Existing bounded Wallet/chat transfers
and x402 retain their separate approval contracts.

For an explicitly requested public read-only source-build probe, run:

```sh
cargo test -p chain-evm --test contract_public_rpc -- --ignored --nocapture
```

It uses no wallet or keys and reads USDC `totalSupply()` through the four default
public RPC endpoints. It is excluded from normal CI because network availability
is external. It does not qualify native UI, live models, proxy provenance or writes.

See [reviewed contract qualification](contract-qualification.md) for the separate source, effect, protocol and workflow authority boundaries.
