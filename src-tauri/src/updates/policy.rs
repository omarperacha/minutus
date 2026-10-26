//! Embedded update trust and signed release/version validation.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;

pub(super) const PUBLIC_KEY: &str = include_str!("../../release-updater.pub");
pub(super) const ENDPOINT: &str = "https://minutus.ai/releases/latest.json";
pub(super) const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Release {
    pub(super) version: String,
    pub(super) url: String,
    pub(super) signature: String,
}

fn decoded_text(value: &str) -> Result<String, ()> {
    if value.starts_with("untrusted comment:") {
        return Ok(value.to_owned());
    }
    String::from_utf8(STANDARD.decode(value.trim()).map_err(|_| ())?).map_err(|_| ())
}

pub(super) fn public_key(value: &str) -> Result<PublicKey, ()> {
    PublicKey::decode(&decoded_text(value)?).map_err(|_| ())
}

pub(super) fn signature(release: &Release) -> Result<Signature, ()> {
    if release.signature.len() > 4096 {
        return Err(());
    }
    Signature::decode(&decoded_text(&release.signature)?).map_err(|_| ())
}

pub(super) fn validate_release(release: &Release, current: &str) -> Result<(), ()> {
    let next = semver::Version::parse(&release.version).map_err(|_| ())?;
    let current = semver::Version::parse(current).map_err(|_| ())?;
    if next <= current || !next.pre.is_empty() || !next.build.is_empty() {
        return Err(());
    }
    let url = reqwest::Url::parse(&release.url).map_err(|_| ())?;
    if url.scheme() != "https"
        || url.host_str() != Some("minutus.ai")
        || url.port().is_some_and(|port| port != 443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().starts_with("/releases/")
        || !std::path::Path::new(url.path())
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dmg"))
    {
        return Err(());
    }
    signature(release)?;
    Ok(())
}

// Call only after cryptographic verification of the complete artifact and the
// Minisign global signature: the trusted comment is otherwise untrusted input.
pub(super) fn validate_signed_version(comment: &str, version: &str) -> Result<(), ()> {
    let fields: Vec<_> = comment
        .split('\t')
        .filter_map(|field| field.strip_prefix("version:"))
        .collect();
    if fields.len() != 1 || fields[0] != version {
        return Err(());
    }
    Ok(())
}
