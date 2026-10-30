# Dependency licence policy

On 2026-10-13, the project owner approved CC0-1.0 for the existing wallet-recovery
dependency chain after reviewing its patent limitation. The approval applies to
these exact versions in Cargo.lock:

| Package | Approved version | Approved licence |
| --- | --- | --- |
| bip39 | 2.2.2 | CC0-1.0 |
| bitcoin_hashes | 0.14.101 | CC0-1.0 |
| hex-conservative | 0.2.2 | CC0-1.0 |

`deny.toml` enforces package-specific, exact-version exceptions. CC0-1.0 is not
in the global licence allowlist. Other CC0 packages and upgrades of these packages
require review rather than inheriting this approval.

CC0 waives copyright and related rights to the extent legally possible and
provides a fallback licence. Section 4(a) expressly excludes patent and trademark
rights. The project accepts that limitation for these dependencies; this decision
does not establish that the code is free of third-party patent claims or certify
its technical security. See the [CC0 legal terms](https://creativecommons.org/publicdomain/zero/1.0/legalcode.en).

The exception mechanism is documented by
[cargo-deny](https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html).
It does not disable advisory, source, integrity or other licence checks. Preserve
third-party notices and SBOM entries when distributing the application.

The unmaintained paste advisory (RUSTSEC-2024-0436) is governed by a separate
[temporary maintenance exception](dependency-maintenance.md), not these licence exceptions.


## Local execution dependency review, 30 October 2026

The new exact `aurora-engine-modexp 1.3.0` CC0 exception is qualified separately
in [local simulation dependency review](local-simulation-dependency-review.md).
It accepts the same explicit patent/trademark exclusion for this package only;
it does not inherit the earlier owner decision or add CC0 to the global list.
The policy wrapper binds its source, checksum, two precompile consumers and
2027-01-30 UTC review expiry. Preserve its declaration and terms when distributing
the helper. The qualification/staging worker overlay bundles the complete Base
MIT and CC0 notices, exact deduplicated Revm and Alloy MIT notices with pinned
version/source attribution, and the complete OpenZeppelin
MIT notice for the embedded reviewed contract profile. The notice source guard
and bundle verifier pin their exact contents; this packaging does not grant
release clearance or enable contract writes in release builds.
