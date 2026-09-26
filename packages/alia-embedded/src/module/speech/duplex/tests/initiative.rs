use super::*;

fn setup(negotiated: bool) -> (SessionGuard, String, String) {
    let boot = Uuid::new_v4().to_string();
    let session = Uuid::new_v4().to_string();
    let playback = Arc::new(PlaybackSession::new(Arc::new(LocalAudio::default())));
    let mut guard = SessionGuard::new(boot.clone(), playback);
    let mut event = ready(&boot, &session);
    if let Some(session_event::Payload::Ready(ready)) = event.payload.as_mut() {
        ready.initiative_admission = negotiated;
    }
    guard.accept(event, 0).unwrap();
    (guard, boot, session)
}

fn admitted(boot: &str, session: &str) -> SessionEvent {
    SessionEvent {
        session_id: session.into(),
        device_boot_id: boot.into(),
        generation_epoch: 2,
        payload: Some(session_event::Payload::InitiativeAdmitted(
            alia_proto::voice::InitiativeAdmitted {
                initiative_id: Uuid::new_v4().to_string(),
                request_id: Uuid::new_v4().to_string(),
                context_id: Uuid::new_v4().to_string(),
                through_sequence: 3,
            },
        )),
    }
}

#[test]
fn initiative_requires_negotiation_and_real_capture_watermark() {
    let (mut old_peer, boot, session) = setup(false);
    assert!(old_peer.accept(admitted(&boot, &session), 4).is_err());
    assert_eq!(old_peer.epoch, 1);
    let (mut guard, boot, session) = setup(true);
    assert!(guard.accept(admitted(&boot, &session), 3).is_err());
    assert_eq!(guard.epoch, 1);
    let event = admitted(&boot, &session);
    guard.accept(event.clone(), 4).unwrap();
    assert_eq!(guard.epoch, 2);
    assert_eq!(guard.through_sequence, Some(3));
    assert!(guard.accept(event.clone(), 4).is_err());
    let mut retried = event;
    retried.generation_epoch = 3;
    assert!(guard.accept(retried, 4).is_err());
    assert_eq!(guard.epoch, 2);
}

#[test]
fn initiative_rejects_identity_epoch_and_watermark_changes_without_fencing() {
    let (mut guard, boot, session) = setup(true);
    let original = admitted(&boot, &session);
    let mut invalid = Vec::new();
    let mut changed = original.clone();
    changed.device_boot_id = Uuid::new_v4().to_string();
    invalid.push(changed);
    let mut changed = original.clone();
    changed.session_id = Uuid::new_v4().to_string();
    invalid.push(changed);
    let mut changed = original.clone();
    changed.generation_epoch = 3;
    invalid.push(changed);
    for field in 0..3 {
        let mut changed = original.clone();
        let Some(session_event::Payload::InitiativeAdmitted(value)) = changed.payload.as_mut()
        else {
            unreachable!()
        };
        match field {
            0 => value.initiative_id = Uuid::nil().to_string(),
            1 => value.request_id = Uuid::nil().to_string(),
            _ => value.context_id = "invalid".into(),
        }
        invalid.push(changed);
    }
    for event in invalid {
        assert!(guard.accept(event, 4).is_err());
        assert_eq!(guard.epoch, 1);
    }
    guard.accept(original, 4).unwrap();
    let mut stale = admitted(&boot, &session);
    stale.generation_epoch = 3;
    if let Some(session_event::Payload::InitiativeAdmitted(value)) = stale.payload.as_mut() {
        value.through_sequence = 2;
    }
    assert!(guard.accept(stale, 4).is_err());
    assert_eq!(guard.epoch, 2);
}
