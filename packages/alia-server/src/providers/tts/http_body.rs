//! Bounded application copies. Reqwest/HTTP framing, TLS and socket buffers
//! remain transport-owned; these limits are not a process RSS claim.
use crate::modules::delivery::audio::AudioReservation;
use anyhow::{Result, ensure};
use futures::StreamExt;

pub(super) const ERROR_BODY_BYTES: usize = 4096;
pub(super) fn check_length(response: &reqwest::Response, limit: usize) -> Result<()> {
    ensure!(
        response.content_length().is_none_or(|n| n <= limit as u64),
        "GPT-SoVITS Content-Length exceeds {limit} bytes"
    );
    Ok(())
}
pub(super) async fn read_body(
    response: reqwest::Response,
    reservation: &AudioReservation,
    limit: usize,
) -> Result<Vec<u8>> {
    ensure!(
        limit <= reservation.capacity(),
        "body copy has no prior reservation"
    );
    check_length(&response, limit)?;
    let mut chunks = response.bytes_stream();
    let mut data = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        ensure!(
            chunk.len() <= limit - data.len(),
            "GPT-SoVITS body exceeds {limit} bytes"
        );
        data.try_reserve_exact(chunk.len())?;
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}
pub(super) async fn error_body(
    response: reqwest::Response,
    reservation: &AudioReservation,
) -> String {
    if reservation.capacity() < ERROR_BODY_BYTES {
        return "error body has no reservation".into();
    }
    let mut chunks = response.bytes_stream();
    let mut data = Vec::new();
    while data.len() < ERROR_BODY_BYTES {
        let Some(Ok(chunk)) = chunks.next().await else {
            break;
        };
        let count = chunk.len().min(ERROR_BODY_BYTES - data.len());
        data.reserve_exact(count);
        data.extend_from_slice(&chunk[..count]);
    }
    let text = String::from_utf8_lossy(&data);
    let mut end = text.len().min(ERROR_BODY_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
