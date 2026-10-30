# Additional local simulation notices

The exact Base 1.5.1 source at commit
`5476069f1eb74a56bccb56fdefd4b4bb0e3cda42` is MIT licensed. Its complete
upstream copyright/licence notice is preserved in [Base-MIT.txt](Base-MIT.txt).

`aurora-engine-modexp 1.3.0`, registry archive SHA-256
`5188e264926edbd2e90d61bf8b33aa3471db8acdf427fa37946f9c82898fe502`,
declares CC0-1.0 in its packaged manifest. The package contains no separate
licence file. [CC0-1.0.txt](CC0-1.0.txt) preserves the canonical legal terms from
[Creative Commons](https://creativecommons.org/publicdomain/zero/1.0/legalcode.txt).
The package authors are the Aurora authors named in its packaged manifest; see
[exact source and applicability review](../../local-simulation-dependency-review.md).

## Revm and Alloy direct dependency families

[Revm-MIT.txt](Revm-MIT.txt) is the unchanged full `LICENSE` file from both exact
Revm registry packages below. Their notice bytes are identical and are included
once. Each package declares MIT; the original upstream copyright is preserved.

[Alloy-MIT.txt](Alloy-MIT.txt) is the unchanged full upstream `LICENSE-MIT` file.
The Alloy packages below declare `MIT OR Apache-2.0`; this notice set preserves
the MIT option. Their registry archives omit root licence files, so the notice
was obtained from the immutable upstream commits recorded in their packaged
`.cargo_vcs_info.json` files:
[alloy-rs/core](https://raw.githubusercontent.com/alloy-rs/core/87080853083c909b34f8c8cff3622570c114985d/LICENSE-MIT)
and [alloy-rs/alloy](https://raw.githubusercontent.com/alloy-rs/alloy/59174912873b337ea5868c295cefa70997596f29/LICENSE-MIT).
The files are byte-identical and are included once. Neither pinned upstream
repository root contains a `NOTICE` file. No copyright text has been invented
or appended to the upstream notice.

| Registry package | Version | Registry archive SHA-256 | Packaged upstream commit |
| --- | --- | --- | --- |
| revm | 42.0.1 | `b7c7fcd426beab3b91300a6d0d5eb8d583c0fad0fba451126e19fd51443eca62` | `fbb0028e10fac9ba97316463478bce1284fc0edd` |
| revm | 43.0.3 | `62b2fbb3bdce72fef4f156e2164bdfde44dc3aeebc5b4f31a413fba0c09c524c` | `6b2e655b91f21c3a21770bed78def5b379ed1d17` |
| alloy-primitives | 1.7.3 | `d5e9dbd49258ac3ab893a481d46be29b58be7f734dcac46cd80b6b13ee36566c` | `87080853083c909b34f8c8cff3622570c114985d` |
| alloy-dyn-abi | 1.7.3 | `14b3b9990889736af898bd4e51f7d0c0228d6a9b9c3a5fecb3e6b25a7abc791c` | `87080853083c909b34f8c8cff3622570c114985d` |
| alloy-consensus | 2.5.0 | `727016fed834c2392d014dd697ea473f3e47bcec31ba31ccffa9c1d18ed904e5` | `59174912873b337ea5868c295cefa70997596f29` |

These notices supplement, rather than replace, the complete resolved dependency
SBOM/licence bundle needed for distribution. The qualification/staging worker
overlay distributes these exact notices beside the isolated helpers. The host
also bundles the original OpenZeppelin MIT notice for its embedded reviewed
contract profile. Release contract writes remain disabled pending native
interaction qualification and final human security review. Remaining registry
dependencies retain their own packaged licences; no global licence or advisory
waiver is granted.
