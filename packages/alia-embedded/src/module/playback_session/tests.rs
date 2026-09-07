use super::*;
use crate::module::lifecycle::ModuleLifecycle;

#[derive(Default)]
struct Device(Mutex<(u64, Vec<i16>)>, std::sync::atomic::AtomicBool);
#[tonic::async_trait]
impl ModuleLifecycle for Device {
    fn name(&self) -> &'static str {
        "test"
    }
}
impl AudioPlaybackBackend for Device {
    fn play(&self, _: &[i16], _: u32) -> Result<()> {
        unreachable!()
    }
    fn stop_playback(&self) -> Result<()> {
        unreachable!()
    }
    fn is_available(&self) -> bool {
        true
    }
    fn begin_scoped_playback(&self) -> Result<u64> {
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        state.1.clear();
        Ok(state.0)
    }
    fn play_if_current(&self, pcm: &[i16], _: u32, generation: u64) -> Result<bool> {
        let mut state = self.0.lock().unwrap();
        if state.0 != generation {
            return Ok(false);
        }
        state.1.extend_from_slice(pcm);
        Ok(true)
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
        session.begin(&scope, &id, 0, 16000).is_err(),
        "remote Start cannot activate a session"
    );
    session.activate(&scope).unwrap();
    let first = session.begin(&scope, &id, 0, 16000).unwrap();
    assert!(session.enqueue(&first, &[1, 2]).unwrap());
    assert!(
        session
            .begin(&scope, &Uuid::new_v4().to_string(), 1, 16000)
            .is_err(),
        "busy before End"
    );
    assert!(session.end(&first).unwrap());
    assert!(
        !session.enqueue(&first, &[9]).unwrap(),
        "no media after End"
    );
    assert!(
        session.begin(&scope, &id, 0, 16000).is_err(),
        "Start replay must fail"
    );
    let second = session
        .begin(&scope, &Uuid::new_v4().to_string(), 1, 24000)
        .unwrap();
    assert!(session.enqueue(&second, &[3]).unwrap());
    assert!(
        !session.cancel(&first).unwrap(),
        "old Push cancellation cannot affect its successor"
    );
    assert_eq!(device.0.lock().unwrap().1, vec![1, 2, 3]);
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
            .begin(&replacement, &Uuid::new_v4().to_string(), 0, 16000)
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
        .begin(&scope, &Uuid::new_v4().to_string(), 0, 16000)
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
            1,
            16000,
        )
        .unwrap();
    device.1.store(true, std::sync::atomic::Ordering::Release);
    assert!(session.close(&scope).is_err());
    assert!(!session.is_active());
    assert!(!session.enqueue(&lease, &[2]).unwrap());
}
