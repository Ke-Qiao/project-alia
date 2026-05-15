use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alia_proto::audio::server_audio_ingress_client::ServerAudioIngressClient;
use alia_proto::audio::{AudioChunk, AudioEncoding, CapturedAudioChunk};
use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::context::{AudioBus, AudioFrame, EventBus, ModuleEvent};
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::traits::{
    AudioPlaybackBackend, DirectResponseEngine, EdgeSttEngine, VoiceActivityEngine,
    VoiceActivityState, WakeWordEngine, WakeWordEvent,
};
use crate::binding::BindingManager;
use crate::config::EdgeSttConfig;

pub struct SpeechPipeline {
    audio_bus: AudioBus,
    event_bus: EventBus,
    wakeword: Arc<dyn WakeWordEngine>,
    edge_stt: Arc<dyn EdgeSttEngine>,
    vad: Arc<dyn VoiceActivityEngine>,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    binding: Arc<BindingManager>,
    max_duration_ms: u32,
    post_wake_capture_ms: u32,
    post_wake_response_delay_ms: u32,
    cancel_token: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl SpeechPipeline {
    pub fn new(
        audio_bus: AudioBus,
        event_bus: EventBus,
        wakeword: Arc<dyn WakeWordEngine>,
        edge_stt: Arc<dyn EdgeSttEngine>,
        vad: Arc<dyn VoiceActivityEngine>,
        direct_response: Arc<dyn DirectResponseEngine>,
        playback: Arc<dyn AudioPlaybackBackend>,
        binding: Arc<BindingManager>,
        edge_stt_config: &EdgeSttConfig,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            audio_bus,
            event_bus,
            wakeword,
            edge_stt,
            vad,
            direct_response,
            playback,
            binding,
            max_duration_ms: edge_stt_config.max_duration_ms,
            post_wake_capture_ms: edge_stt_config.post_wake_capture_ms,
            post_wake_response_delay_ms: edge_stt_config.post_wake_response_delay_ms,
            cancel_token,
            task: Mutex::new(None),
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for SpeechPipeline {
    fn name(&self) -> &'static str {
        "speech_pipeline"
    }

    async fn start(&self) -> Result<()> {
        let mut task = self.task.lock().await;
        if task.is_some() {
            return Ok(());
        }

        let mut rx = self.audio_bus.subscribe();
        let event_bus = self.event_bus.clone();
        let wakeword = Arc::clone(&self.wakeword);
        let edge_stt = Arc::clone(&self.edge_stt);
        let vad = Arc::clone(&self.vad);
        let direct_response = Arc::clone(&self.direct_response);
        let playback = Arc::clone(&self.playback);
        let binding = Arc::clone(&self.binding);
        let max_duration_ms = self.max_duration_ms;
        let post_wake_capture_ms = self.post_wake_capture_ms;
        let post_wake_response_delay_ms = self.post_wake_response_delay_ms;
        let cancel = self.cancel_token.child_token();

        *task = Some(tokio::spawn(async move {
            let mut capture: Option<SpeechCapture> = None;
            let mut pending_wakeword: Option<WakeWordEvent> = None;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    frame = rx.recv() => {
                        let frame = match frame {
                            Ok(frame) => frame,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!(skipped, "speech pipeline lagged behind audio bus");
                                continue;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        };

                        if let Some(event) = pending_wakeword.take() {
                            event_bus.publish(ModuleEvent::SpeechStart);
                            let vad_enabled = prepare_vad_for_capture(vad.as_ref());
                            capture = Some(SpeechCapture::new(
                                event,
                                frame.sample_rate,
                                frame.timestamp_us,
                                Duration::from_millis(post_wake_capture_ms as u64),
                                Duration::from_millis(max_duration_ms as u64),
                                vad_enabled,
                            ));
                        }

                        if let Some(active) = capture.as_mut() {
                            active.push(&frame);
                            if active.vad_enabled() {
                                match vad.accept(&frame.pcm, frame.sample_rate) {
                                    Ok(state) => active.observe_vad(state),
                                    Err(error) => {
                                        warn!(error = ?error, "VAD failed during speech capture; falling back to fixed capture window");
                                        active.disable_vad();
                                    }
                                }
                            }
                            if active.is_complete(frame.timestamp_us) {
                                let finished = capture.take().expect("capture is active");
                                if vad.is_available()
                                    && let Err(error) = vad.reset()
                                {
                                    warn!(error = ?error, "failed to reset VAD after speech capture");
                                }
                                event_bus.publish(ModuleEvent::SpeechEnd);
                                handle_utterance(
                                    finished,
                                    Arc::clone(&edge_stt),
                                    Arc::clone(&direct_response),
                                    Arc::clone(&playback),
                                    Arc::clone(&binding),
                                    event_bus.clone(),
                                    max_duration_ms,
                                )
                                .await;
                            }
                            continue;
                        }

                        if let Some(event) = wakeword.detect(&frame.pcm) {
                            event_bus.publish(ModuleEvent::WakeWordDetected {
                                keyword: event.keyword.clone(),
                                timestamp_us: event.timestamp_us,
                            });
                            if let Some(action) = direct_response.match_keyword(&event.keyword) {
                                play_direct_response(action, Arc::clone(&playback), event_bus.clone())
                                    .await;
                                if post_wake_response_delay_ms > 0 {
                                    tokio::time::sleep(Duration::from_millis(
                                        post_wake_response_delay_ms as u64,
                                    ))
                                    .await;
                                }
                                rx = rx.resubscribe();
                            }
                            pending_wakeword = Some(event);
                        }
                    }
                }
            }
        }));

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.cancel_token.cancel();
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.task.lock().await.is_some() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Degraded
        }
    }
}

struct SpeechCapture {
    wakeword: WakeWordEvent,
    sample_rate: u32,
    fallback_deadline_us: u64,
    max_deadline_us: u64,
    vad_enabled: bool,
    vad_saw_speech: bool,
    vad_finished: bool,
    pcm: Vec<i16>,
}

impl SpeechCapture {
    fn new(
        wakeword: WakeWordEvent,
        sample_rate: u32,
        started_at_us: u64,
        fallback_duration: Duration,
        max_duration: Duration,
        vad_enabled: bool,
    ) -> Self {
        let fallback_deadline_us =
            started_at_us.saturating_add(fallback_duration.as_micros() as u64);
        let max_deadline_us = started_at_us.saturating_add(max_duration.as_micros() as u64);
        Self {
            wakeword,
            sample_rate,
            fallback_deadline_us,
            max_deadline_us,
            vad_enabled,
            vad_saw_speech: false,
            vad_finished: false,
            pcm: Vec::new(),
        }
    }

    fn push(&mut self, frame: &AudioFrame) {
        if frame.sample_rate != self.sample_rate {
            warn!(
                expected = self.sample_rate,
                actual = frame.sample_rate,
                "dropping frame with changed sample rate during speech capture"
            );
            return;
        }
        self.pcm.extend_from_slice(&frame.pcm);
    }

    fn vad_enabled(&self) -> bool {
        self.vad_enabled
    }

    fn disable_vad(&mut self) {
        self.vad_enabled = false;
    }

    fn observe_vad(&mut self, state: VoiceActivityState) {
        match state {
            VoiceActivityState::Unavailable => self.disable_vad(),
            VoiceActivityState::Silence => {}
            VoiceActivityState::Speech => self.vad_saw_speech = true,
            VoiceActivityState::Finished => {
                self.vad_saw_speech = true;
                self.vad_finished = true;
            }
        }
    }

    fn is_complete(&self, timestamp_us: u64) -> bool {
        if !self.vad_enabled {
            return timestamp_us >= self.fallback_deadline_us;
        }

        self.vad_finished
            || timestamp_us >= self.max_deadline_us
            || (!self.vad_saw_speech && timestamp_us >= self.fallback_deadline_us)
    }
}

fn prepare_vad_for_capture(vad: &dyn VoiceActivityEngine) -> bool {
    if !vad.is_available() {
        return false;
    }

    match vad.reset() {
        Ok(()) => true,
        Err(error) => {
            warn!(error = ?error, "failed to reset VAD before speech capture; using fixed capture window");
            false
        }
    }
}

async fn handle_utterance(
    capture: SpeechCapture,
    edge_stt: Arc<dyn EdgeSttEngine>,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    binding: Arc<BindingManager>,
    event_bus: EventBus,
    max_duration_ms: u32,
) {
    if capture.pcm.is_empty() {
        return;
    }

    let transcript = match edge_stt
        .transcribe_short(&capture.pcm, max_duration_ms)
        .await
    {
        Ok(Some(text)) if !text.trim().is_empty() => text,
        Ok(_) => {
            debug!(keyword = %capture.wakeword.keyword, "edge STT produced no transcript");
            upload_to_server(&capture, &binding).await;
            return;
        }
        Err(error) => {
            warn!(error = %error, "edge STT failed");
            upload_to_server(&capture, &binding).await;
            return;
        }
    };

    let Some(action) = direct_response.match_keyword(&transcript) else {
        debug!(%transcript, "no direct response keyword matched");
        upload_to_server(&capture, &binding).await;
        return;
    };

    play_direct_response(action, playback, event_bus).await;
}

async fn play_direct_response(
    action: super::traits::DirectResponseAction,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
) {
    if let Some(action_name) = action.action.as_deref() {
        info!(action = action_name, "direct response action matched");
    }

    match read_pcm16_wav(Path::new(&action.audio_path)).await {
        Ok((pcm, sample_rate)) => {
            event_bus.publish(ModuleEvent::PlaybackStarted {
                source: action.audio_path.clone(),
            });
            if let Err(error) = playback.play(&pcm, sample_rate) {
                warn!(error = %error, audio = %action.audio_path, "direct response playback failed");
            } else if let Some(duration) = pcm_duration(&pcm, sample_rate) {
                tokio::time::sleep(duration).await;
            }
            event_bus.publish(ModuleEvent::PlaybackFinished {
                source: action.audio_path,
            });
        }
        Err(error) => {
            warn!(error = %error, audio = %action.audio_path, "failed to read direct response audio")
        }
    }
}

fn pcm_duration(pcm: &[i16], sample_rate: u32) -> Option<Duration> {
    if pcm.is_empty() || sample_rate == 0 {
        return None;
    }

    Some(Duration::from_secs_f64(
        pcm.len() as f64 / sample_rate as f64,
    ))
}

async fn upload_to_server(capture: &SpeechCapture, binding: &BindingManager) {
    let status = binding.status().await;
    let Some(server_endpoint) = status.server_endpoint.as_deref() else {
        warn!("cannot upload speech to server before device is bound");
        return;
    };
    let endpoint = match derive_audio_ingress_endpoint(server_endpoint) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            warn!(error = %error, server_endpoint, "failed to derive server audio ingress endpoint");
            return;
        }
    };
    let pcm = capture.pcm.clone();
    let sample_rate = capture.sample_rate;
    let channel_id = "embedded".to_string();
    let user_id = "embedded-user".to_string();

    if let Err(error) =
        upload_pcm16_to_server(endpoint, pcm, sample_rate, channel_id, user_id).await
    {
        warn!(error = %error, "failed to upload captured speech to server");
    }
}

fn derive_audio_ingress_endpoint(server_endpoint: &str) -> Result<String> {
    let (scheme, rest) = server_endpoint
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("server endpoint must include scheme"))?;
    let (authority, _path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("server endpoint must include host:port"))?;
    let port = port
        .parse::<u16>()
        .context("parsing server endpoint port")?;
    let ingress_port = port
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("server endpoint port cannot be incremented"))?;
    Ok(format!("{scheme}://{host}:{ingress_port}"))
}

async fn upload_pcm16_to_server(
    endpoint: String,
    pcm: Vec<i16>,
    sample_rate: u32,
    channel_id: String,
    user_id: String,
) -> Result<()> {
    let mut client = ServerAudioIngressClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("connecting to server audio ingress at {endpoint}"))?;

    let frames = pcm
        .chunks(1600)
        .enumerate()
        .map(move |(sequence, samples)| CapturedAudioChunk {
            chunk: Some(AudioChunk {
                opus_data: pcm16_to_le_bytes(samples),
                sequence: sequence as u64,
                timestamp_us: timestamp_us(),
                encoding: AudioEncoding::PcmS16le as i32,
            }),
            channel_id: channel_id.clone(),
            user_id: user_id.clone(),
            sample_rate,
            channels: 1,
        })
        .collect::<Vec<_>>();

    client
        .upload_captured_audio(tokio_stream::iter(frames))
        .await
        .context("uploading captured speech to server")?;
    Ok(())
}

fn pcm16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

async fn read_pcm16_wav(path: &Path) -> Result<(Vec<i16>, u32)> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || read_pcm16_wav_blocking(&path))
        .await
        .context("joining WAV reader task")?
}

fn read_pcm16_wav_blocking(path: &Path) -> Result<(Vec<i16>, u32)> {
    let data = std::fs::read(path).with_context(|| format!("reading WAV: {}", path.display()))?;
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("{} is not a RIFF/WAVE file", path.display());
    }

    let mut offset = 12;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut pcm_bytes = None;

    while offset + 8 <= data.len() {
        let id = &data[offset..offset + 4];
        let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if offset + size > data.len() {
            bail!("malformed WAV chunk in {}", path.display());
        }

        match id {
            b"fmt " => {
                if size < 16 {
                    bail!("invalid WAV fmt chunk in {}", path.display());
                }
                audio_format = Some(u16::from_le_bytes(
                    data[offset..offset + 2].try_into().unwrap(),
                ));
                channels = Some(u16::from_le_bytes(
                    data[offset + 2..offset + 4].try_into().unwrap(),
                ));
                sample_rate = Some(u32::from_le_bytes(
                    data[offset + 4..offset + 8].try_into().unwrap(),
                ));
                bits_per_sample = Some(u16::from_le_bytes(
                    data[offset + 14..offset + 16].try_into().unwrap(),
                ));
            }
            b"data" => {
                pcm_bytes = Some(&data[offset..offset + size]);
            }
            _ => {}
        }

        offset += size + (size % 2);
    }

    if audio_format != Some(1) {
        bail!("{} must be PCM WAV", path.display());
    }
    if bits_per_sample != Some(16) {
        bail!("{} must be 16-bit PCM WAV", path.display());
    }

    let channels = channels.context("missing WAV channel count")?;
    let sample_rate = sample_rate.context("missing WAV sample rate")?;
    let pcm_bytes = pcm_bytes.context("missing WAV data chunk")?;
    let samples: Vec<i16> = pcm_bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    if channels <= 1 {
        return Ok((samples, sample_rate));
    }

    let mono = samples
        .chunks_exact(channels as usize)
        .map(|frame| frame[0])
        .collect();
    Ok((mono, sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pcm16_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&16_000u32.to_le_bytes());
        bytes.extend_from_slice(&32_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&1i16.to_le_bytes());
        bytes.extend_from_slice(&(-2i16).to_le_bytes());
        std::fs::write(&path, bytes).unwrap();

        let (pcm, sample_rate) = read_pcm16_wav_blocking(&path).unwrap();
        assert_eq!(sample_rate, 16_000);
        assert_eq!(pcm, vec![1, -2]);
    }

    #[test]
    fn computes_pcm_duration() {
        assert_eq!(
            pcm_duration(&vec![0; 16_000], 16_000),
            Some(Duration::from_secs(1))
        );
        assert_eq!(pcm_duration(&[], 16_000), None);
        assert_eq!(pcm_duration(&[0], 0), None);
    }

    #[test]
    fn vad_capture_waits_for_silence_until_max_duration() {
        let mut capture = SpeechCapture::new(
            wakeword_event(),
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(5_000),
            true,
        );

        capture.observe_vad(VoiceActivityState::Speech);

        assert!(!capture.is_complete(1_000 + 2_600_000));
        capture.observe_vad(VoiceActivityState::Finished);
        assert!(capture.is_complete(1_000 + 2_700_000));
    }

    #[test]
    fn vad_capture_uses_no_speech_timeout_before_speech_starts() {
        let capture = SpeechCapture::new(
            wakeword_event(),
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(5_000),
            true,
        );

        assert!(!capture.is_complete(1_000 + 2_400_000));
        assert!(capture.is_complete(1_000 + 2_500_000));
    }

    #[test]
    fn fixed_capture_fallback_uses_post_wake_capture_window() {
        let capture = SpeechCapture::new(
            wakeword_event(),
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(5_000),
            false,
        );

        assert!(!capture.is_complete(1_000 + 2_400_000));
        assert!(capture.is_complete(1_000 + 2_500_000));
    }

    #[test]
    fn derives_audio_ingress_from_bound_server_endpoint() {
        let endpoint = derive_audio_ingress_endpoint("http://192.168.1.10:3000").unwrap();
        assert_eq!(endpoint, "http://192.168.1.10:3001");
    }

    fn wakeword_event() -> WakeWordEvent {
        WakeWordEvent {
            keyword: "HEY_ALIA".to_string(),
            confidence: 1.0,
            timestamp_us: 1_000,
        }
    }
}
