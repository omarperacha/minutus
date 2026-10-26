//! Bounded HTTPS download into a private temporary file with streaming verification.

use std::{
    io::{Read, Write},
    time::Duration,
};

use super::policy::{self, Release};

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, ()> {
    reqwest::blocking::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()
        .map_err(|_| ())
}

pub(super) fn check(current: &str) -> Result<Option<Release>, ()> {
    // Trust must already be provisioned before any network request.
    policy::public_key(policy::PUBLIC_KEY)?;
    let response = client(Duration::from_secs(30))?
        .get(policy::ENDPOINT)
        .send()
        .map_err(|_| ())?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(());
    }
    let mut bytes = Vec::new();
    response
        .take(32 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > 32 * 1024 {
        return Err(());
    }
    let release: Release = serde_json::from_slice(&bytes).map_err(|_| ())?;
    let offered = semver::Version::parse(&release.version).map_err(|_| ())?;
    let current_version = semver::Version::parse(current).map_err(|_| ())?;
    if offered <= current_version {
        return Ok(None);
    }
    policy::validate_release(&release, current)?;
    Ok(Some(release))
}

pub(super) fn verified_artifact(release: &Release) -> Result<tempfile::NamedTempFile, ()> {
    let key = policy::public_key(policy::PUBLIC_KEY)?;
    let signature = policy::signature(release)?;
    let mut verifier = key.verify_stream(&signature).map_err(|_| ())?;
    let mut response = client(Duration::from_mins(10))?
        .get(&release.url)
        .send()
        .map_err(|_| ())?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .content_length()
            .is_some_and(|size| size > policy::MAX_ARTIFACT_BYTES)
    {
        return Err(());
    }
    let mut temporary = tempfile::NamedTempFile::new().map_err(|_| ())?;
    let mut size = 0_u64;
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let count = response.read(&mut chunk).map_err(|_| ())?;
        if count == 0 {
            break;
        }
        size += u64::try_from(count).map_err(|_| ())?;
        if size > policy::MAX_ARTIFACT_BYTES {
            return Err(());
        }
        verifier.update(&chunk[..count]);
        temporary.write_all(&chunk[..count]).map_err(|_| ())?;
    }
    if size == 0 {
        return Err(());
    }
    verifier.finalize().map_err(|_| ())?;
    policy::validate_signed_version(signature.trusted_comment(), &release.version)?;
    temporary.as_file().sync_all().map_err(|_| ())?;
    Ok(temporary)
}
