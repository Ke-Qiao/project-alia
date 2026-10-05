use super::*;
use crate::module::lifecycle::ModuleLifecycle;

#[derive(Default)]
struct Device(
    Mutex<(u64, Vec<i16>)>,
    std::sync::atomic::AtomicBool,
    Mutex<Vec<(PlaybackIdentity, Vec<i16>)>>,
);
#[tonic::async_trait]
impl ModuleLifecycle for Device {
    fn name(&self) -> &'static str {
        "test"
    }
}
impl AudioPlaybackBackend for Device {
    fn open_submission(
        &self,
        identity: PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> Result<Arc<AudioSubmission>> {
        Ok(AudioSubmission::test(identity, generation, rate))
    }
    fn submit_audio(
        &self,
        submission: &Arc<AudioSubmission>,
        offset: u64,
        pcm: &[i16],
    ) -> Result<bool> {
        let mut state = self.0.lock().unwrap();
        if state.0 != submission.generation || !submission.is_live() {
            return Ok(false);
        }
        submission.accept(offset, pcm.len() as u64)?;
        self.2
            .lock()
            .unwrap()
            .push((submission.identity, pcm.to_vec()));
        state.1.extend_from_slice(pcm);
        Ok(true)
    }
    fn end_submission(&self, submission: &Arc<AudioSubmission>, total: u64) -> Result<bool> {
        submission.accept_end(total)
    }
    fn cancel_submission(&self, submission: &Arc<AudioSubmission>, reason: &str) -> Result<bool> {
        let mut state = self.0.lock().unwrap();
        if state.0 != submission.generation {
            return Ok(false);
        }
        submission.terminate(ObservationEnd::Cancelled {
            reason: reason.into(),
        });
        let mut segments = self.2.lock().unwrap();
        segments.retain(|(id, _)| *id != submission.identity);
        state.1 = segments
            .iter()
            .flat_map(|(_, pcm)| pcm.iter().copied())
            .collect();
        Ok(true)
    }

    fn is_available(&self) -> bool {
        true
    }
    fn begin_scoped_playback(&self) -> Result<u64> {
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        state.1.clear();
        self.2.lock().unwrap().clear();
        Ok(state.0)
    }

    fn stop_if_current(&self, generation: u64) -> Result<bool> {
        anyhow::ensure!(
            !self.1.load(std::sync::atomic::Ordering::Acquire),
            "simulated hardware failure"
        );
        let mut state = self.0.lock().unwrap();
        if state.0 != generation {
            return Ok(false);
        }
        state.0 += 1;
        state.1.clear();
        self.2.lock().unwrap().clear();
        Ok(true)
    }
    fn is_playback_current(&self, generation: u64) -> bool {
        self.0.lock().unwrap().0 == generation
    }
}

fn scope(epoch: u64) -> VoiceScope {
    VoiceScope {
        device_boot_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        generation_epoch: epoch,
    }
}

#[test]
fn same_epoch_pushes_append_fifo_and_require_end_and_increasing_sequence() {
    let device = Arc::new(Device::default());
    let session = PlaybackSession::new(device.clone());
    let scope = scope(1);
    let id = Uuid::new_v4().to_string();
    assert!(
        session.begin(&scope, &id, 1, 16000).is_err(),
        "remote Start cannot activate a session"
    );
    session.activate(&scope).unwrap();
    let first = session.begin(&scope, &id, 1, 16000).unwrap();
    assert!(session.enqueue(&first, &[1, 2]).unwrap());
    assert!(
        session
            .begin(&scope, &Uuid::new_v4().to_string(), 2, 16000)
            .is_err(),
        "busy before End"
    );
    assert!(session.end(&first).unwrap());
    assert!(
        !session.enqueue(&first, &[9]).unwrap(),
        "no media after End"
    );
    assert!(
        session.begin(&scope, &id, 1, 16000).is_err(),
        "Start replay must fail"
    );
    let second = session
        .begin(&scope, &Uuid::new_v4().to_string(), 2, 24000)
        .unwrap();
    assert!(session.enqueue(&second, &[3]).unwrap());
    assert!(
        session.cancel(&first).unwrap(),
        "old Push cancellation applies to its identity and preserves its successor"
    );
    assert_eq!(device.0.lock().unwrap().1, vec![3]);
}

#[test]
fn newer_epoch_fences_queued_audio_and_late_network_fences_are_harmless() {
    let device = Arc::new(Device::default());
    let session = PlaybackSession::new(device.clone());
    let old_scope = scope(1);
    session.activate(&old_scope).unwrap();
    let old = session
        .begin(&old_scope, &Uuid::new_v4().to_string(), 10, 16000)
        .unwrap();
    session.enqueue(&old, &[1]).unwrap();
    let current_scope = VoiceScope {
        generation_epoch: 2,
        ..old_scope
    };
    assert!(session.fence(&current_scope, false).unwrap());
    assert!(!session.enqueue(&old, &[2]).unwrap());
    assert!(
        session
            .begin(&current_scope, &Uuid::new_v4().to_string(), 10, 16000)
            .is_err(),
        "epoch advance cannot reset playback sequence"
    );
    let current = session
        .begin(&current_scope, &Uuid::new_v4().to_string(), 11, 16000)
        .unwrap();
    session.enqueue(&current, &[3]).unwrap();
    assert!(!session.fence(&old_scope, false).unwrap());
    assert_eq!(device.0.lock().unwrap().1, vec![3]);
    assert!(
        session.fence(&old_scope, true).unwrap(),
        "Close retires whole matching session after epoch advance"
    );
    assert!(!session.enqueue(&current, &[4]).unwrap());
    let replacement = scope(1);
    session.activate(&replacement).unwrap();
    assert!(
        !session.fence(&old_scope, true).unwrap(),
        "old UUID Close cannot affect replacement session"
    );
    assert!(
        !session.close(&current_scope).unwrap(),
        "old stream close cannot close its replacement"
    );
    assert!(
        session
            .begin(&current_scope, &Uuid::new_v4().to_string(), 12, 16000)
            .is_err()
    );
    assert!(
        session
            .begin(&replacement, &Uuid::new_v4().to_string(), 1, 16000)
            .is_ok()
    );
    assert!(session.close(&replacement).unwrap());
    assert!(!session.is_active());
}

#[test]
fn hardware_stop_failure_never_leaves_a_stale_scope_admissible() {
    let device = Arc::new(Device::default());
    let session = PlaybackSession::new(device.clone());
    let scope = scope(1);
    session.activate(&scope).unwrap();
    let lease = session
        .begin(&scope, &Uuid::new_v4().to_string(), 1, 16000)
        .unwrap();
    device.1.store(true, std::sync::atomic::Ordering::Release);
    assert!(
        session
            .fence(
                &VoiceScope {
                    generation_epoch: 2,
                    ..scope
                },
                false
            )
            .is_err()
    );
    assert!(!session.enqueue(&lease, &[1]).unwrap());
    assert_eq!(session.active_scope().unwrap().unwrap().generation_epoch, 2);
    device.1.store(false, std::sync::atomic::Ordering::Release);
    let lease = session
        .begin(
            &VoiceScope {
                generation_epoch: 2,
                ..scope
            },
            &Uuid::new_v4().to_string(),
            2,
            16000,
        )
        .unwrap();
    device.1.store(true, std::sync::atomic::Ordering::Release);
    assert!(session.close(&scope).is_err());
    assert!(!session.is_active());
    assert!(!session.enqueue(&lease, &[2]).unwrap());
}

#[tokio::test]
async fn prior_native_and_body_retirement_failures_survive_later_successful_close() {
    use alia_core::output_control::OutputRetirementRequest;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Body(AtomicBool);
    impl VoiceOutputFence for Body {
        fn activate_voice(&self, _: &VoiceScope) -> Result<()> {
            Ok(())
        }
        fn fence_voice(&self, _: &VoiceScope) -> Result<()> {
            ensure!(!self.0.load(Ordering::Acquire), "original body stop failed");
            Ok(())
        }
        fn close_voice(&self, scope: &VoiceScope) -> Result<()> {
            self.fence_voice(scope)
        }
    }
    let device = Arc::new(Device::default());
    let session = PlaybackSession::new(device.clone());
    let original = scope(1);
    session.activate(&original).unwrap();
    session
        .begin(&original, &Uuid::new_v4().to_string(), 1, 16000)
        .unwrap();
    device.1.store(true, Ordering::Release);
    assert!(
        session
            .fence(
                &VoiceScope {
                    generation_epoch: 2,
                    ..original
                },
                false
            )
            .is_err()
    );
    device.1.store(false, Ordering::Release);
    assert!(
        session.stop().await.is_err(),
        "later close erased the earlier native failure"
    );

    let body = Arc::new(Body(AtomicBool::new(false)));
    let session = PlaybackSession::with_output_fence(Arc::new(Device::default()), body.clone());
    let original = scope(1);
    session.activate(&original).unwrap();
    let request = OutputRetirementRequest {
        scope: original,
        binding: None,
        expected_backend: None,
    };
    body.0.store(true, Ordering::Release);
    assert!(session.retire_output(&request).is_err());
    body.0.store(false, Ordering::Release);
    assert!(
        session.retire_output(&request).is_err(),
        "retry erased the original body failure"
    );
    assert!(
        session.stop().await.is_err(),
        "later close erased the earlier body failure"
    );
}

#[test]
fn observation_capacity_counts_ended_inputs_until_submission_or_cancel() {
    let device = Arc::new(Device::default());
    let session = PlaybackSession::new(device);
    let scope = scope(1);
    session.activate(&scope).unwrap();
    let mut leases = Vec::new();
    for sequence in 1..=32 {
        let lease = session
            .begin(&scope, &Uuid::new_v4().to_string(), sequence, 16000)
            .unwrap();
        session.enqueue(&lease, &[1]).unwrap();
        session.end(&lease).unwrap();
        leases.push(lease);
    }
    assert!(
        session
            .begin(&scope, &Uuid::new_v4().to_string(), 33, 16000)
            .is_err()
    );
    assert_eq!(session.progress(&leases[0], 1).observation_end, None);
    session.cancel(&leases[0]).unwrap();
    assert!(
        session
            .begin(&scope, &Uuid::new_v4().to_string(), 34, 16000)
            .is_ok()
    );
    assert_eq!(
        session.progress(&leases[1], 1).observation_end,
        None,
        "releasing one observer must not retire the others"
    );
}
