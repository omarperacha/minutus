use crate::{Error, Role};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) struct Identity {
    pub(crate) service: String,
    pub(crate) broker_cdhash: String,
    pub(crate) worker_cdhash: String,
    broker: PathBuf,
    broker_sha: String,
    worker: PathBuf,
    pub(crate) worker_sha: String,
}
impl Identity {
    pub(crate) fn packaged(role: Role) -> Result<Self, Error> {
        let bytes = crate::native::manifest()?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::Unavailable)?;
        let fields = parse_manifest(text, role)?;
        let service = fields[1].to_owned();
        let broker_name = fields[2];
        let worker_name = fields[5];
        let host = std::env::current_exe().map_err(|_| Error::Unavailable)?;
        let contents = host
            .parent()
            .and_then(Path::parent)
            .filter(|p| p.file_name().is_some_and(|name| name == "Contents"))
            .ok_or(Error::Unavailable)?;
        let directory = contents
            .join("XPCServices")
            .join(format!("{service}.xpc"))
            .join("Contents/MacOS");
        let identity = Self {
            service,
            broker_cdhash: fields[4].into(),
            worker_cdhash: fields[7].into(),
            broker: directory.join(broker_name),
            broker_sha: fields[3].into(),
            worker: directory.join(worker_name),
            worker_sha: fields[6].into(),
        };
        identity.verify()?;
        Ok(identity)
    }
    pub(crate) fn verify(&self) -> Result<(), Error> {
        verify_file(&self.broker, &self.broker_sha)?;
        verify_file(&self.worker, &self.worker_sha)
    }
}
fn parse_manifest(text: &str, selected: Role) -> Result<[&str; 8], Error> {
    let lines = text.lines().collect::<Vec<_>>();
    if text.len() > 4096 || lines.len() != 4 || lines[0] != "minutus-worker-brokers-v1" {
        return Err(Error::Unavailable);
    }
    let mut found = None;
    let mut seen = std::collections::BTreeSet::new();
    for line in &lines[1..] {
        let fields: [&str; 8] = line
            .split(' ')
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|_| Error::Unavailable)?;
        let role = match fields[0] {
            "compiler" => Role::Compiler,
            "evm-online" => Role::EvmOnline,
            "base-offline" => Role::BaseOffline,
            _ => return Err(Error::Unavailable),
        };
        let worker = match role {
            Role::Compiler => "minutus-solidity-compiler",
            Role::EvmOnline => "minutus-evm-simulator",
            Role::BaseOffline => "minutus-base-offline-simulator",
        };
        if !seen.insert(fields[0])
            || fields[1] != format!("ai.minutus.worker.{}", role.name())
            || fields[2] != format!("minutus-{}-broker", role.name())
            || fields[5] != worker
            || !hash(fields[3], 64)
            || !hash(fields[4], 40)
            || !hash(fields[6], 64)
            || !hash(fields[7], 40)
        {
            return Err(Error::Unavailable);
        }
        if role == selected {
            found = Some(fields);
        }
    }
    found.ok_or(Error::Unavailable)
}
fn hash(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn verify_file(path: &Path, expected: &str) -> Result<(), Error> {
    let descriptor = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| Error::Unavailable)?;
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(|_| Error::Unavailable)?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        return Err(Error::Unavailable);
    }
    let mut digest = Sha256::new();
    let mut stream = file.take(256 * 1024 * 1024 + 1);
    let mut buffer = [0; 16384];
    let mut count = 0;
    loop {
        let length = stream.read(&mut buffer).map_err(|_| Error::Unavailable)?;
        if length == 0 {
            break;
        }
        count += length;
        digest.update(&buffer[..length]);
    }
    if count > 256 * 1024 * 1024 || format!("{:x}", digest.finalize()) != expected {
        return Err(Error::Unavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
