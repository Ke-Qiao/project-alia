use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::debug::{
    debug_component, debug_component_audio, debug_component_stream_chunk, debug_component_text,
    log_channel_delivery_failure,
};
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::channels::adapter::{
    AudioFormat, AudioPayload, AudioPayloadStream, ChannelAdapter, ChannelTarget,
};
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{ChatMessage, LlmAdapter, TtsAdapter};

const TTS_SENTENCE_MAX_CHARS: usize = 80;
const TTS_SENTENCE_GAP_MS: u32 = 80;

pub(super) async fn deliver_tts_audio(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    text: &str,
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) {
    if turn_is_interrupted(turn) {
        return;
    }
    if text.trim().is_empty() {
        tracing::warn!(
            channel_type = channel.channel_type(),
            channel_id = channel.channel_id(),
            "skipping TTS for empty response text"
        );
        return;
    }

    if channel.capabilities().audio_streaming {
        debug_component_text(
            debug_output,
            "tts",
            "stream_call",
            text,
            Some((channel_id, user_id)),
        );

        match cancellable_stage(
            turn,
            "tts_stream_start",
            modules.tts.synthesize_stream(text),
        )
        .await
        {
            Ok(Some(tts_stream)) => {
                let audio_started = Arc::new(AtomicBool::new(false));
                let audio_started_marker = audio_started.clone();
                let audio_stream = tts_stream.map(move |chunk| {
                    chunk
                        .map(|data| AudioPayload {
                            data,
                            sample_rate: 24000,
                            channels: 1,
                            format: AudioFormat::Wav,
                        })
                        .inspect(|_| {
                            audio_started_marker.store(true, Ordering::Relaxed);
                        })
                });

                let cancel_token = turn
                    .filter(|turn| {
                        turn.interrupt_config.enabled
                            && turn.interrupt_config.cancel_streaming_tts_on_interrupt
                    })
                    .map(|turn| turn.token.clone());
                let delivery = channel.send_audio_stream(target, Box::pin(audio_stream));
                let delivery_result = if let Some(cancel_token) = cancel_token {
                    tokio::select! {
                        _ = cancel_token.cancelled() => return,
                        result = delivery => result,
                    }
                } else {
                    delivery.await
                };

                if let Err(error) = delivery_result {
                    log_channel_delivery_failure(channel, error).await;
                    if audio_started.load(Ordering::Relaxed) {
                        return;
                    }
                    tracing::warn!(
                        channel_type = channel.channel_type(),
                        channel_id = channel.channel_id(),
                        "streaming TTS failed before audio playback; falling back to buffered TTS"
                    );
                } else {
                    return;
                }
            }
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    channel_type = channel.channel_type(),
                    channel_id = channel.channel_id(),
                    error = %error,
                    "streaming TTS failed; falling back to buffered TTS"
                );
                tracing::debug!(error = ?error, "streaming TTS failure details");
            }
        }
    }

    debug_component_text(
        debug_output,
        "tts",
        "call",
        text,
        Some((channel_id, user_id)),
    );
    let audio_bytes = match cancellable_stage(turn, "tts", modules.tts.synthesize(text)).await {
        Ok(Some(audio_bytes)) => audio_bytes,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(
                channel_type = channel.channel_type(),
                channel_id = channel.channel_id(),
                error = %error,
                "TTS synthesis failed; keeping channel enabled"
            );
            tracing::debug!(error = ?error, "TTS synthesis failure details");
            return;
        }
    };
    if turn_is_interrupted(turn) {
        return;
    }
    debug_component_audio(
        debug_output,
        "tts",
        "result",
        audio_bytes.len(),
        Some((channel_id, user_id)),
    );

    let audio_out = AudioPayload {
        data: audio_bytes,
        sample_rate: 24000,
        channels: 1,
        format: AudioFormat::Wav,
    };

    debug_component_audio(
        debug_output,
        "channel",
        "send_audio",
        audio_out.data.len(),
        Some((channel_id, user_id)),
    );
    let send_result = if let Some(turn) = turn.filter(|turn| turn.interrupt_config.enabled) {
        tokio::select! {
            _ = turn.token.cancelled() => return,
            result = channel.send_audio(target, audio_out) => result,
        }
    } else {
        channel.send_audio(target, audio_out).await
    };
    if let Err(error) = send_result {
        log_channel_delivery_failure(channel, error).await;
    }
}

pub(super) async fn stream_llm_response_with_sentence_tts(
    modules: &ModuleRegistry,
    llm: &dyn LlmAdapter,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    event_bus: &EventBus,
    messages: &[ChatMessage],
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> anyhow::Result<(String, bool)> {
    let (sentence_tx, sentence_rx) = mpsc::channel::<String>(4);
    let (audio_tx, audio_rx) = mpsc::channel::<anyhow::Result<AudioPayload>>(8);
    let audio_started = Arc::new(AtomicBool::new(false));
    let tts_cancel_token = turn
        .filter(|turn| {
            turn.interrupt_config.enabled && turn.interrupt_config.cancel_streaming_tts_on_interrupt
        })
        .map(|turn| turn.token.clone());
    let tts_worker = tokio::spawn(stream_tts_sentences(
        modules.tts.clone(),
        sentence_rx,
        audio_tx,
        audio_started.clone(),
        debug_output,
        channel_id.to_string(),
        user_id.to_string(),
        tts_cancel_token.clone(),
    ));

    let audio_stream: AudioPayloadStream =
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(audio_rx));
    let audio_delivery = async move {
        if let Some(cancel_token) = tts_cancel_token {
            tokio::select! {
                _ = cancel_token.cancelled() => Ok(()),
                result = channel.send_audio_stream(target, audio_stream) => result,
            }
        } else {
            channel.send_audio_stream(target, audio_stream).await
        }
    };

    let llm_delivery = async move {
        let mut token_stream =
            match cancellable_stage(turn, "llm_stream_start", llm.chat_stream(messages)).await? {
                Some(token_stream) => token_stream,
                None => return Ok::<_, anyhow::Error>(String::new()),
            };

        debug_component(
            debug_output,
            "channel",
            "begin_stream",
            Some((channel_id, user_id)),
        );
        if let Err(error) = channel.begin_stream(target).await {
            tracing::warn!(channel_id, error = %error, "stream begin delivery failed");
            tracing::debug!(channel_id, error = ?error, "stream begin delivery failure details");
        }

        let mut full_text = String::new();
        let mut sentence_buffer = String::new();
        let mut sentence_tx = Some(sentence_tx);

        loop {
            let chunk_result = if let Some(turn) = turn.filter(|turn| turn.interrupt_config.enabled)
            {
                tokio::select! {
                    _ = turn.token.cancelled() => break,
                    chunk_result = token_stream.next() => chunk_result,
                }
            } else {
                token_stream.next().await
            };

            let Some(chunk_result) = chunk_result else {
                break;
            };
            match chunk_result {
                Ok(chunk) => {
                    full_text.push_str(&chunk);
                    sentence_buffer.push_str(&chunk);
                    debug_component_stream_chunk(debug_output, &chunk, Some((channel_id, user_id)));

                    if let Err(e) = channel.send_stream_chunk(target, &chunk).await {
                        tracing::warn!(error = %e, "stream chunk delivery failed");
                        tracing::debug!(error = ?e, "stream chunk delivery failure details");
                    }

                    event_bus.publish(ServerEvent::StreamChunk {
                        channel_id: channel_id.to_string(),
                        user_id: user_id.to_string(),
                        chunk,
                    });

                    while let Some(segment) = drain_speakable_segment(&mut sentence_buffer, false) {
                        let Some(tx) = sentence_tx.as_ref() else {
                            break;
                        };
                        if tx.send(segment).await.is_err() {
                            sentence_tx = None;
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "LLM stream error");
                    tracing::debug!(error = ?e, "LLM stream error details");
                    break;
                }
            }
        }

        while let Some(segment) = drain_speakable_segment(&mut sentence_buffer, true) {
            let Some(tx) = sentence_tx.as_ref() else {
                break;
            };
            if tx.send(segment).await.is_err() {
                sentence_tx = None;
                break;
            }
        }
        drop(sentence_tx);

        if let Err(error) = channel.end_stream(target, &full_text).await {
            tracing::warn!(channel_id, error = %error, "stream end delivery failed");
            tracing::debug!(channel_id, error = ?error, "stream end delivery failure details");
        }
        debug_component_text(
            debug_output,
            "channel",
            "end_stream",
            &full_text,
            Some((channel_id, user_id)),
        );

        event_bus.publish(ServerEvent::StreamEnd {
            channel_id: channel_id.to_string(),
            user_id: user_id.to_string(),
        });

        Ok::<_, anyhow::Error>(full_text)
    };

    let (llm_result, audio_result) = futures::join!(llm_delivery, audio_delivery);
    let tts_result = tts_worker
        .await
        .map_err(|error| anyhow::anyhow!("sentence TTS worker join failed: {error}"));
    let audio_streamed = audio_started.load(Ordering::Relaxed);

    if let Err(error) = audio_result {
        tracing::warn!(
            channel_type = channel.channel_type(),
            channel_id = channel.channel_id(),
            error = %error,
            "sentence-level TTS audio delivery failed"
        );
        tracing::debug!(error = ?error, "sentence-level TTS audio delivery failure details");
    }
    if let Err(error) = tts_result.and_then(|result| result) {
        tracing::warn!(error = %error, "sentence-level TTS worker failed");
        tracing::debug!(error = ?error, "sentence-level TTS worker failure details");
    }

    Ok((llm_result?, audio_streamed))
}

async fn stream_tts_sentences(
    tts: Arc<dyn TtsAdapter>,
    mut sentence_rx: mpsc::Receiver<String>,
    audio_tx: mpsc::Sender<anyhow::Result<AudioPayload>>,
    audio_started: Arc<AtomicBool>,
    debug_output: bool,
    channel_id: String,
    user_id: String,
    cancel_token: Option<CancellationToken>,
) -> anyhow::Result<()> {
    loop {
        let sentence = if let Some(cancel_token) = cancel_token.as_ref() {
            tokio::select! {
                _ = cancel_token.cancelled() => return Ok(()),
                sentence = sentence_rx.recv() => sentence,
            }
        } else {
            sentence_rx.recv().await
        };
        let Some(sentence) = sentence else {
            break;
        };
        let sentence = sentence.trim();
        if sentence.is_empty() {
            continue;
        }

        debug_component_text(
            debug_output,
            "tts",
            "sentence_call",
            sentence,
            Some((&channel_id, &user_id)),
        );
        let synth = tts.synthesize(sentence);
        let audio_bytes = if let Some(cancel_token) = cancel_token.as_ref() {
            tokio::select! {
                _ = cancel_token.cancelled() => return Ok(()),
                result = synth => match result {
                    Ok(audio_bytes) => audio_bytes,
                    Err(error) => {
                        let message = format!("TTS sentence synthesis failed: {error}");
                        let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                        return Err(error);
                    }
                },
            }
        } else {
            match synth.await {
                Ok(audio_bytes) => audio_bytes,
                Err(error) => {
                    let message = format!("TTS sentence synthesis failed: {error}");
                    let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                    return Err(error);
                }
            }
        };

        let payload = match tts_bytes_to_pcm_payload(audio_bytes) {
            Ok(payload) => payload,
            Err(error) => {
                let message = format!("TTS sentence audio decode failed: {error}");
                let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                return Err(error);
            }
        };

        if payload.data.is_empty() {
            continue;
        }

        let silence =
            pcm16_silence_payload(payload.sample_rate, payload.channels, TTS_SENTENCE_GAP_MS);
        if audio_tx.send(Ok(payload)).await.is_err() {
            return Ok(());
        }
        audio_started.store(true, Ordering::Relaxed);
        if audio_tx.send(Ok(silence)).await.is_err() {
            return Ok(());
        }
    }

    Ok(())
}

pub(super) fn tts_bytes_to_pcm_payload(bytes: Vec<u8>) -> anyhow::Result<AudioPayload> {
    if let Some((data, sample_rate, channels)) = pcm16_payload_from_wav(&bytes)? {
        return Ok(AudioPayload {
            data,
            sample_rate,
            channels,
            format: AudioFormat::Pcm16,
        });
    }

    // The TTS adapter contract historically returned WAV bytes, but keep a
    // conservative raw-PCM fallback for test/noop/custom adapters.
    Ok(AudioPayload {
        data: bytes,
        sample_rate: 24000,
        channels: 1,
        format: AudioFormat::Pcm16,
    })
}

fn pcm16_payload_from_wav(bytes: &[u8]) -> anyhow::Result<Option<(Vec<u8>, u32, u16)>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Ok(None);
    }

    let mut pos = 12usize;
    let mut channels = None;
    let mut sample_rate = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut data_range = None;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into()?) as usize;
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(size);
        if data_end > bytes.len() {
            break;
        }

        match chunk_id {
            b"fmt " if size >= 16 => {
                audio_format = Some(u16::from_le_bytes(
                    bytes[data_start..data_start + 2].try_into()?,
                ));
                channels = Some(u16::from_le_bytes(
                    bytes[data_start + 2..data_start + 4].try_into()?,
                ));
                sample_rate = Some(u32::from_le_bytes(
                    bytes[data_start + 4..data_start + 8].try_into()?,
                ));
                bits_per_sample = Some(u16::from_le_bytes(
                    bytes[data_start + 14..data_start + 16].try_into()?,
                ));
            }
            b"data" => data_range = Some((data_start, data_end)),
            _ => {}
        }

        let padded = size + (size % 2);
        pos = data_start.saturating_add(padded);
    }

    let Some((data_start, data_end)) = data_range else {
        anyhow::bail!("WAV missing data chunk");
    };
    let channels = channels.unwrap_or(1);
    let sample_rate = sample_rate.unwrap_or(24_000);
    let bits_per_sample = bits_per_sample.unwrap_or(16);
    let audio_format = audio_format.unwrap_or(1);

    if audio_format != 1 || bits_per_sample != 16 {
        anyhow::bail!(
            "unsupported WAV format: audio_format={}, bits_per_sample={}",
            audio_format,
            bits_per_sample
        );
    }

    let mut data = bytes[data_start..data_end].to_vec();
    if data.len() % 2 != 0 {
        data.pop();
    }
    Ok(Some((data, sample_rate, channels.max(1))))
}

pub(super) fn pcm16_silence_payload(
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
) -> AudioPayload {
    let sample_count = sample_rate
        .saturating_mul(u32::from(channels.max(1)))
        .saturating_mul(duration_ms)
        .saturating_div(1_000) as usize;
    AudioPayload {
        data: vec![0u8; sample_count * 2],
        sample_rate,
        channels: channels.max(1),
        format: AudioFormat::Pcm16,
    }
}

fn drain_speakable_segment(buffer: &mut String, final_flush: bool) -> Option<String> {
    let sentence_boundary = buffer
        .char_indices()
        .find(|(_, ch)| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n'))
        .map(|(idx, ch)| idx + ch.len_utf8());

    let boundary = sentence_boundary.or_else(|| {
        if final_flush {
            return Some(buffer.len());
        }
        if buffer.chars().count() < TTS_SENTENCE_MAX_CHARS {
            return None;
        }

        let max_byte = buffer
            .char_indices()
            .nth(TTS_SENTENCE_MAX_CHARS)
            .map(|(idx, _)| idx)
            .unwrap_or(buffer.len());
        buffer[..max_byte]
            .char_indices()
            .rev()
            .find(|(_, ch)| matches!(ch, ',' | ';' | ':' | '，' | '；' | '：' | ' '))
            .map(|(idx, ch)| idx + ch.len_utf8())
            .or(Some(max_byte))
    })?;

    let segment = buffer[..boundary].trim().to_string();
    let rest = buffer[boundary..].to_string();
    *buffer = rest;

    if segment.is_empty() {
        None
    } else {
        Some(segment)
    }
}

pub(super) async fn stream_llm_response(
    _modules: &ModuleRegistry,
    llm: &dyn LlmAdapter,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    event_bus: &EventBus,
    messages: &[ChatMessage],
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> anyhow::Result<String> {
    let mut token_stream =
        match cancellable_stage(turn, "llm_stream_start", llm.chat_stream(messages)).await? {
            Some(token_stream) => token_stream,
            None => return Ok(String::new()),
        };

    debug_component(
        debug_output,
        "channel",
        "begin_stream",
        Some((channel_id, user_id)),
    );
    if let Err(error) = channel.begin_stream(target).await {
        tracing::warn!(channel_id, error = %error, "stream begin delivery failed");
        tracing::debug!(channel_id, error = ?error, "stream begin delivery failure details");
    }

    let mut full_text = String::new();

    loop {
        let chunk_result = if let Some(turn) = turn.filter(|turn| turn.interrupt_config.enabled) {
            tokio::select! {
                _ = turn.token.cancelled() => break,
                chunk_result = token_stream.next() => chunk_result,
            }
        } else {
            token_stream.next().await
        };

        let Some(chunk_result) = chunk_result else {
            break;
        };
        match chunk_result {
            Ok(chunk) => {
                full_text.push_str(&chunk);
                debug_component_stream_chunk(debug_output, &chunk, Some((channel_id, user_id)));

                if let Err(e) = channel.send_stream_chunk(target, &chunk).await {
                    tracing::warn!(error = %e, "stream chunk delivery failed");
                    tracing::debug!(error = ?e, "stream chunk delivery failure details");
                }

                event_bus.publish(ServerEvent::StreamChunk {
                    channel_id: channel_id.to_string(),
                    user_id: user_id.to_string(),
                    chunk,
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "LLM stream error");
                tracing::debug!(error = ?e, "LLM stream error details");
                break;
            }
        }
    }

    if let Err(error) = channel.end_stream(target, &full_text).await {
        tracing::warn!(channel_id, error = %error, "stream end delivery failed");
        tracing::debug!(channel_id, error = ?error, "stream end delivery failure details");
    }
    debug_component_text(
        debug_output,
        "channel",
        "end_stream",
        &full_text,
        Some((channel_id, user_id)),
    );

    event_bus.publish(ServerEvent::StreamEnd {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
    });

    Ok(full_text)
}
