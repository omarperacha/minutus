# Minutus releases

## Source beta

The first beta is source-only on GitHub. Follow the [build instructions](build.md)
for a local ad hoc signed app. This is separate from the future notarised DMG and
website launch. Source users update their checkout and rebuild. Current package
metadata is `0.1.0-beta.1`; use `v0.1.0-beta.1` for its Git tag.

## Official DMG distribution

Official builds must come from a reviewed public source commit and qualified
runtime inputs. Publish versioned artefacts and metadata on `minutus.ai` with:
version, DMG, SHA-256, offline-controlled signature, public source commit, release
notes, SBOM, and model download sizes and hardware requirements.

The committed Qwen binary pin remains distinct from local public-source rebuilds.
Before promoting a rebuilt runtime, review its provenance and rerun application,
security and hardware qualification. The signing command rejects a payload that
does not match the committed reviewed artefact identity.

Use `.minutus-release.example.json` to prepare the ignored local signing config.
Developer ID certificates, provisioning profiles, notarisation credentials and
offline private keys stay outside source control. The release scripts verify
nested signatures, hardened runtime and identity before notarisation. Successful
notarisation and clean-Mac Gatekeeper acceptance must be recorded separately.

The repository contains the public Minisign verification key. Choosing
**Check for updates** uses the manual
`https://minutus.ai/releases/latest.json` feed. The signature covers the installer
and its signed version. The app saves only a verified installer, preserving
quarantine; the user quits Minutus and installs through Finder. Unsigned artefacts
are rejected. The enrolled public key is in `src-tauri/release-updater.pub`;
empty or invalid keys disable update networking. The current updater accepts
newer stable versions only, not beta/prerelease versions.

Maintainer commands, after preparing the ignored signing configuration and the
reviewed runtime payload:

```sh
python3 scripts/macos_distribution.py check
pnpm release:sign
pnpm release:verify
pnpm release:notarize
```

Notarisation writes the DMG and reports beneath
`target/aarch64-apple-darwin/release/bundle/distribution/`. These commands do not
publish a release, create its offline update signature, or generate its SBOM.

## Update feed contract

`latest.json` contains exactly three fields: `version`, `url` and `signature`.
Keep release notes, SHA-256, source commit and SBOM in separate release metadata;
extra fields in the updater feed are rejected. `signature` is the complete
Minisign signature text or its base64 encoding. Its authenticated trusted comment
must contain exactly one tab-delimited `version:VERSION` field matching the feed.

The DMG URL must use HTTPS on `minutus.ai`, under `/releases/`, with no query or
fragment. Downloads are capped at 512 MiB. See the
[enforced policy](../src-tauri/src/updates/policy.rs) before preparing a feed.

## Remaining DMG launch gates

- Complete Developer ID signing, accepted notarisation and final DMG qualification.
- Publish the offline-signed artefacts/feed and qualify the complete update installation.
- Publish the release SBOM and pass `python3 scripts/check_dependency_policy.py`.
  The temporary paste exception requires review by 2027-10-13.

Public CI validates source builds and tests. It does not hold production signing
credentials, notarise releases automatically, or enrol an updater trust key.
It separately executes both signed XPC qualification scripts and the Rust and
JavaScript dependency gates. Ordinary Cargo tests explicitly mark packaged XPC
fixtures ignored; they are not evidence of completed containment qualification.
