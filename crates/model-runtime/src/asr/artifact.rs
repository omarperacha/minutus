use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use crate::DownloadError;

// whisper.cpp b4938 reads this little-endian ggml header. ASR-001 qualifies
// only multilingual large-v3-turbo Q5_0 (quantization version 2, type 8).
const QUALIFIED_HEADER: [i32; 12] = [
    0x6767_6d6c,
    51866,
    1500,
    1280,
    20,
    32,
    448,
    1280,
    20,
    4,
    128,
    2008,
];

pub(crate) fn validate_header(file: &mut File) -> Result<(), DownloadError> {
    file.seek(SeekFrom::Start(0))
        .map_err(DownloadError::Storage)?;
    let mut bytes = [0; 48];
    file.read_exact(&mut bytes)
        .map_err(|_| DownloadError::IncompatibleArtifact)?;
    for (chunk, expected) in bytes.chunks_exact(4).zip(QUALIFIED_HEADER) {
        if chunk != expected.to_le_bytes() {
            return Err(DownloadError::IncompatibleArtifact);
        }
    }
    Ok(())
}
