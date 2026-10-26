//! Async-runtime-safe ownership for reqwest's blocking HTTP client.

use std::{
    mem,
    ops::Deref,
    sync::{OnceLock, mpsc},
    thread,
};

use reqwest::blocking::{Client, ClientBuilder};

static CLIENT_REAPER: OnceLock<Option<mpsc::Sender<Client>>> = OnceLock::new();

/// Owns a blocking reqwest client without ever dropping its internal Tokio
/// runtime on an application's async worker.
pub(crate) struct BlockingClient {
    client: Client,
}

impl BlockingClient {
    pub(crate) fn build(builder: ClientBuilder) -> Result<Self, ()> {
        let worker = thread::Builder::new()
            .name("zeta-http-client-builder".to_owned())
            .spawn(move || builder.build())
            .map_err(|_| ())?;
        let client = worker.join().map_err(|_| ())?.map_err(|_| ())?;
        Ok(Self { client })
    }
}

impl Deref for BlockingClient {
    type Target = Client;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

impl Drop for BlockingClient {
    fn drop(&mut self) {
        // Keep one shared handle alive until the reaper drops it. The field can
        // then drop on this thread without being the final reqwest handle.
        let client = self.client.clone();
        let sender = CLIENT_REAPER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Client>();
            match thread::Builder::new()
                .name("zeta-http-client-reaper".to_owned())
                .spawn(move || {
                    for client in receiver {
                        drop(client);
                    }
                }) {
                Ok(_) => Some(sender),
                Err(_) => None,
            }
        });
        let Some(sender) = sender else {
            // Failure to create one process-lifetime cleanup thread must not
            // turn application shutdown into a Tokio runtime panic.
            mem::forget(client);
            return;
        };
        if let Err(mpsc::SendError(client)) = sender.send(client) {
            mem::forget(client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_can_be_released_inside_a_tokio_runtime() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        runtime.block_on(async {
            drop(BlockingClient::build(Client::builder()));
        });
        Ok(())
    }
}
