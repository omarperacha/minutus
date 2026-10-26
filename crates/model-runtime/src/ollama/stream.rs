//! Owned cancellable HTTP I/O behind the synchronous provider boundary.

use std::{
    sync::mpsc::{self, TrySendError},
    thread::{self, JoinHandle},
    time::Duration,
};

use super::{DownloadCancellation, OllamaError, OllamaProviderConfig, classify_status};

const POLL: Duration = Duration::from_millis(20);
const CHUNK_BYTES: usize = 64 * 1024;

struct Worker {
    stop: DownloadCancellation,
    thread: Option<JoinHandle<Result<(), OllamaError>>>,
}

impl Worker {
    fn finish(&mut self) -> Result<(), OllamaError> {
        self.thread
            .take()
            .ok_or(OllamaError::Unavailable)?
            .join()
            .map_err(|_| OllamaError::Unavailable)?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Also covers callback unwinding: no detached request or runtime survives.
        self.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn receive(
    config: OllamaProviderConfig,
    url: String,
    body: Vec<u8>,
    is_cancelled: impl Fn() -> bool,
    mut on_chunk: impl FnMut(&[u8]) -> Result<bool, OllamaError>,
) -> Result<(), OllamaError> {
    if is_cancelled() {
        return Err(OllamaError::Cancelled);
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let stop = DownloadCancellation::new();
    let worker_stop = stop.clone();
    let thread = thread::Builder::new()
        .name("zeta-ollama-stream".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| OllamaError::Unavailable)?;
            runtime.block_on(async {
                tokio::select! {
                    biased;
                    () = cancelled_or_stopped(&worker_stop) => Err(OllamaError::Cancelled),
                    result = tokio::time::timeout(config.stream_timeout, transfer(config, url, body, sender)) => {
                        result.map_err(|_| OllamaError::DeadlineExceeded)?
                    }
                }
            })
        })
        .map_err(|_| OllamaError::Unavailable)?;
    let mut worker = Worker {
        stop,
        thread: Some(thread),
    };
    loop {
        if is_cancelled() {
            return Err(OllamaError::Cancelled);
        }
        match receiver.recv_timeout(POLL) {
            Ok(chunk) if !on_chunk(&chunk)? => return Ok(()),
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return worker.finish(),
        }
    }
}

async fn cancelled_or_stopped(stop: &DownloadCancellation) {
    while !stop.is_cancelled() {
        tokio::time::sleep(POLL).await;
    }
}

async fn transfer(
    config: OllamaProviderConfig,
    url: String,
    body: Vec<u8>,
    sender: mpsc::SyncSender<Vec<u8>>,
) -> Result<(), OllamaError> {
    let client = reqwest::Client::builder()
        .connect_timeout(config.connect_timeout)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| OllamaError::Unavailable)?;
    let mut response = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| OllamaError::Unavailable)?;
    if !response.status().is_success() {
        return Err(classify_status(response.status()));
    }
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OllamaError::Unavailable)?
    {
        for piece in chunk.chunks(CHUNK_BYTES) {
            let mut owned = piece.to_vec();
            loop {
                match sender.try_send(owned) {
                    Ok(()) => break,
                    Err(TrySendError::Disconnected(_)) => return Ok(()),
                    Err(TrySendError::Full(value)) => owned = value,
                }
                tokio::time::sleep(POLL).await;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
