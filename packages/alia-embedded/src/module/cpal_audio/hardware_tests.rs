//! Explicit opt-in hardware output; no microphone, UART, model or PCM recording.
use super::*;
use crate::module::context::{AudioReferenceReader, AudioSampleBoundary, ReferenceContent};
use crate::module::playback_session::PlaybackSession;
use crate::module::volume::AudioVolumeConfig;
use alia_core::audio_clock::AudioDirection;
use alia_core::delivery::{ObservationEnd, VoiceScope};
use alia_core::output_control::OutputBinding;
use alia_core::playback_control::*;
use anyhow::ensure;
use futures::FutureExt;
use std::time::Duration;
use uuid::Uuid;

#[derive(Default)]
struct Evidence {
    held_frames: u64,
    media_frames: u64,
    peak: f64,
}

fn collect(reader: &mut AudioReferenceReader, evidence: &mut Evidence) -> Result<()> {
    while let Some(block) = reader.try_read() {
        ensure!(
            block.lost_blocks_before == 0,
            "playback reference overflowed"
        );
        if block.sample_count == 0 {
            continue;
        }
        ensure!(block.boundary == Some(AudioSampleBoundary::AlsaWriteAccepted));
        ensure!(block.timing.stream.direction == AudioDirection::Playback);
        ensure!(block.timing.format.sample_rate_hz == 48000 && block.timing.format.channels == 2);
        let samples = &block.samples[..block.sample_count];
        ensure!(samples.chunks_exact(2).all(|pair| pair[0] == pair[1]));
        let peak = samples
            .iter()
            .fold(0.0_f64, |peak, sample| peak.max(sample.abs()));
        ensure!(
            peak < 0.01,
            "reference exceeds the bounded synthetic tone gain"
        );
        if block.content == ReferenceContent::HeldSilence {
            ensure!(peak == 0.0, "held reference is not actual silence");
            evidence.held_frames += block.timing.frames;
        } else if peak > 0.0 {
            evidence.media_frames += block.timing.frames;
            evidence.peak = evidence.peak.max(peak);
        }
    }
    Ok(())
}

async fn until(
    reader: &mut AudioReferenceReader,
    evidence: &mut Evidence,
    ready: impl Fn(&Evidence) -> bool,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            collect(reader, evidence)?;
            if ready(evidence) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("hardware playback checkpoint timed out")?
}

fn pause(scope: VoiceScope, sequence: u64, through_sequence: u64) -> ControlCommand {
    ControlCommand {
        scope,
        hold_id: Uuid::new_v4(),
        command_id: Uuid::new_v4(),
        sequence,
        action: ControlAction::Pause {
            reason: HoldReason::ExplicitWait,
            through_sequence,
            budget_ms: 3000,
        },
    }
}

fn resume(command: &ControlCommand, through_sequence: u64) -> ControlCommand {
    ControlCommand {
        command_id: Uuid::new_v4(),
        sequence: command.sequence + 1,
        action: ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind: ResumeKind::ExplicitContinue,
                detection_id: Uuid::new_v4(),
                through_sequence,
            },
        },
        ..command.clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "opens the C-Media speaker; requires an agreed playback window and ALIA_R1_PLAYBACK_TEST_CONFIG"]
async fn r1_cmedia_playback_host_boundaries() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_test_writer()
        .try_init();
    let path = std::env::var_os("ALIA_R1_PLAYBACK_TEST_CONFIG")
        .map(std::path::PathBuf::from)
        .context("set ALIA_R1_PLAYBACK_TEST_CONFIG to the approved absolute config path")?;
    ensure!(
        path.is_absolute() && path.is_file(),
        "explicit existing config required"
    );
    let config = crate::config::load_config(&path)?;
    ensure!(config.modules.audio_playback == "cpal");
    ensure!(config.audio.playback_backend == AudioBackend::Alsa);
    ensure!(config.audio.playback_device == "hw:CARD=Device,DEV=0");
    ensure!(config.audio.playback_sample_rate == 48000 && config.audio.playback_channels == 2);
    ensure!(!config.audio.alsa_playback_lock_to_capture_rate);
    let mut gain = AudioVolumeConfig::from_audio_config(&config.audio);
    gain.playback_volume = gain.playback_volume.min(0.05);
    ensure!(
        gain.playback_volume > 0.0,
        "muted config cannot verify nonzero playback reference"
    );
    let reference = AudioBus::new(2).reference();
    let mut reader = reference
        .try_claim_reader()
        .context("reference reader unavailable")?;
    let mut native = CpalAudioPlayback::new(config.audio, AudioVolumeState::new(gain));
    // This is the only init field; avoid constructing unrelated device modules or state stores.
    native.reference = Some(reference.clone());
    let native = Arc::new(native);
    let session = PlaybackSession::new(native.clone());
    let scope = VoiceScope {
        device_boot_id: reader.boot_id(),
        session_id: Uuid::new_v4(),
        generation_epoch: 1,
    };
    let mut evidence = Evidence::default();
    let outcome = tokio::time::timeout(Duration::from_secs(10), std::panic::AssertUnwindSafe(async {
        native.start().await?;
        session.activate(&scope)?;
        let binding = OutputBinding { scope, response_id: Uuid::new_v4(), context_id: Uuid::new_v4(), server_instance_id: Uuid::new_v4(), initiative_capture: None };
        let registration = session.register_output(&binding)?;
        ensure!(registration.backend.boundary == SubmissionBoundary::AlsaWriteAccepted);
        ensure!(registration.backend.output_sample_rate_hz == 48000 && registration.backend.output_channels == 2);
        let lease = session.begin_bound(&binding, &Uuid::new_v4().to_string(), 1, 24000)?;
        // Synthetic control evidence exercises the owner; this test does not open capture.
        session.observe_capture_for_control(&scope, 7, false)?;
        let hold = pause(scope, 1, 7);
        ensure!(session.apply_control(&hold)?.outcome == CommandOutcome::Applied);
        ensure!(session.wait_held(&scope, hold.hold_id).await?.phase == ControlPhase::Held);
        let tone: Vec<i16> = (0..4800).map(|n| {
            let envelope = (n as f64 / 240.0).min((4799 - n) as f64 / 240.0).min(1.0);
            (2048.0 * envelope * (std::f64::consts::TAU * 440.0 * n as f64 / 24000.0).sin()) as i16
        }).collect();
        ensure!(session.enqueue(&lease, &tone)? && session.end(&lease)?);
        until(&mut reader, &mut evidence, |e| e.held_frames > 0).await.context("waiting for actual held-silence ALSA writes")?;
        ensure!(lease.submission.snapshot(1).submitted_source_frames == Some(0));
        ensure!(session.apply_control(&resume(&hold, 7))?.snapshot.phase == ControlPhase::Running);
        until(&mut reader, &mut evidence, |e| e.media_frames > 0 && lease.submission.snapshot(2).observation_end.is_some()).await.context("waiting for resumed media and submission end")?;
        let finished = lease.submission.snapshot(3);
        ensure!(finished.accepted_source_frames == 4800 && finished.submitted_source_frames == Some(4800));
        ensure!(finished.end_accepted && finished.observation_end == Some(ObservationEnd::SubmissionFinished));
        ensure!(finished.presented_source_frames.is_none(), "submission is not DAC presentation");

        let cancelled = session.begin_bound(&binding, &Uuid::new_v4().to_string(), 2, 24000)?;
        session.observe_capture_for_control(&scope, 8, false)?;
        let hold = pause(scope, 3, 8);
        ensure!(session.apply_control(&hold)?.outcome == CommandOutcome::Applied);
        session.wait_held(&scope, hold.hold_id).await?;
        ensure!(session.enqueue(&cancelled, &tone)?);
        let terminal = session.cancel_observed(&cancelled.identity(), "hardware fixture cancellation").await?;
        ensure!(matches!(terminal.observation_end, Some(ObservationEnd::Cancelled { .. })));
        ensure!(terminal.submitted_source_frames == Some(0));
        ensure!(session.apply_control(&resume(&hold, 8))?.snapshot.phase == ControlPhase::Running);
        ensure!(!session.enqueue(&cancelled, &tone)? && !session.end(&cancelled)?);
        tokio::time::sleep(Duration::from_millis(100)).await;
        collect(&mut reader, &mut evidence)?;
        ensure!(cancelled.submission.snapshot(4).submitted_source_frames == terminal.submitted_source_frames);
        ensure!(reference.snapshot(AudioDirection::Playback).context("missing native stream")?.lost_blocks == 0);
        ensure!(native.is_available(), "active native playback owner reported unavailable");
        eprintln!("R1 playback host evidence: {}", serde_json::json!({
            "sample_rate_hz": 48000, "channels": 2, "gain": gain.playback_volume,
            "held_frames": evidence.held_frames, "media_frames": evidence.media_frames,
            "normalized_reference_peak": evidence.peak, "finished": finished, "cancelled": terminal,
            "pcm_saved": false, "acoustic_aec_or_physical_tail_verified": false
        }));
        Ok::<_, anyhow::Error>(())
    }).catch_unwind()).await;
    eprintln!(
        "R1 playback native state before cleanup: {}",
        serde_json::json!({
            "available": native.is_available(),
            "stream": reference.snapshot(AudioDirection::Playback),
            "held_frames": evidence.held_frames,
            "media_frames": evidence.media_frames,
        })
    );
    // Cleanup is outside the timed/caught body, including startup errors and assertion panics.
    let retired = session.revoke_all();
    let native_closed = native.stop().await;
    let session_closed = session.stop().await;
    retired?;
    native_closed?;
    session_closed?;
    ensure!(
        native
            .runtime
            .lock()
            .map_err(|_| anyhow!("runtime lock poisoned"))?
            .is_none()
    );
    if let Some(stream) = reference.snapshot(AudioDirection::Playback) {
        ensure!(
            stream.closed && !stream.running,
            "native playback owner is not closed"
        );
    }
    match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => bail!("hardware playback exceeded its 10-second body budget"),
    }
}
