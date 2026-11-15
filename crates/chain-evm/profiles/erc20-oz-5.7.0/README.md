# Direct ERC20 source profile fixture

This fixed source/compiler bundle supports independent qualification work for
`approve`, `transfer` and `transferFrom`. Its existence does not enable signing,
qualify arbitrary ERC20 implementations, establish token value, or turn simulation
output into authority. The runtime must match all 1,764 bytes, including metadata.
Unknown code/proxies, changed sources/settings, links and immutable patches are
outside this profile. The source derivative adds only constructor-time minting;
it adds no runtime hook or administrative method.

The OpenZeppelin input is the stable
[v5.7.0 source](https://github.com/OpenZeppelin/openzeppelin-contracts/tree/cab19933c33c2ad1d4c7a84864a3601dddfd16f3)
at commit `cab19933c33c2ad1d4c7a84864a3601dddfd16f3`. The official
[Solidity 0.8.38 release](https://www.soliditylang.org/blog/2026/10/22/solidity-0.8.38-release-announcement/)
fixes the preceding optimiser bookkeeping issue. This bundle uses the exact
compiler build `0.8.38+commit.1835d247.Emscripten.clang`, the existing repository
Node 22.23.4 build tool, standard JSON, Cancun target, evmasm (`viaIR=false`),
200 optimiser runs, literal source metadata and an IPFS metadata commitment. It has no
external libraries, immutable references or import callback.

## Reproduce without installation

Download the compiler into a temporary directory, then run the fixed build tool:

```sh
curl -fsSL 'https://binaries.soliditylang.org/emscripten-wasm32/solc-emscripten-wasm32-v0.8.38+commit.1835d247.js' -o /private/tmp/soljson-0.8.38.js
python3 crates/chain-evm/profiles/erc20-oz-5.7.0/rebuild.py --soljson /private/tmp/soljson-0.8.38.js
```

Run under pinned Node 22.23.4 (`nvm use` from the checkout), or pass its executable
with `--node`. The script checks the published SHA-256
`6c74ffba0ea4cf9521e78b16052a98208770bb7677616de08ced239f4312d390`
before loading the compiler. The matching official list entry is preserved in
`compiler-record.json`; the originating HTTPS list and snapshot hash are recorded
in `manifest.json`. The [compiler distribution documentation](https://docs.soliditylang.org/en/v0.8.38/installing-solidity.html#static-binaries)
describes these immutable filenames and checksum verification.

`rebuild.py` fetches nothing and uses no package manager, signer, wallet or network
provider. Standard input embeds all six source files; unresolved imports fail.
The compiler is a build-only tool in the temporary directory and is not bundled
with the application. `--write` restores deterministic outputs only if they still
match this fixed manifest; source/settings/profile changes require a new reviewed
revision, rather than overwriting trust roots silently.

## Artefacts and trust root

- `input.json` is the complete standard JSON input; `sources/` retains the exact
  corresponding source bytes and licence declarations.
- `output.json` preserves the complete requested compiler output, including ABI,
  metadata, storage layout, creation/runtime bytes and method identifiers.
- `abi.json`, `storage-layout.json`, `method-identifiers.json`, `metadata.json`,
  `creation.hex` and `runtime.hex` are exact deterministic projections.
- `manifest.json` binds compiler/source/input/output hashes and a profile
  commitment. Ethereum runtime Keccak is
  `0x34ad9dbf5175a1e9da19c8edb9d390a0026ac815cdf159a903a04f53e39fd841`.
- `OpenZeppelin-MIT.txt` preserves the upstream licence. The local derivative is
  MIT licensed. Preserve these notices if the source profile is distributed.

The application must pin this reviewed bundle/profile in code. Loading arbitrary
JSON that claims the same profile name or source label must never register an
implementation. A successful reproducibility check establishes correspondence,
not complete semantic safety. In particular, matching runtime does not prove
which creation code or initial storage was used on chain. Global token-supply
invariants cannot be assumed from this runtime alone; transfer qualification
needs independent checked effect arithmetic and a canonical deployment
certificate. The first policy admits only the exact compiler creation code with
canonical bounded `(string,string,address,uint256)` arguments, a nonzero initial
holder, and a successful independently included type-2 CREATE transaction and
receipt. The initial supply may be zero through `uint256::MAX`. Token name and
symbol may be empty and are bounded to 1,024 UTF-8 bytes each. Factory creations,
proxies and other initcode returning the same runtime require separate review.

ABI labels/token names are not evidence of value or trust. Runtime metadata is
part of exact identity, not an instruction source. No metadata stripping or
normalisation is permitted by this fixture.
