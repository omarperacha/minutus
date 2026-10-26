//! Signed installer updates owned by the desktop distribution boundary.

mod commands;
mod download;
mod policy;
#[cfg(test)]
mod tests;

pub(crate) use commands::{
    check_release_update, release_updates_configured, save_verified_release_update,
};

#[derive(Default)]
pub(crate) struct ReleaseUpdateControl(tokio::sync::Mutex<Option<policy::Release>>);
