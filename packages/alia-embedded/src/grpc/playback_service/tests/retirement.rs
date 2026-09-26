use super::*;
use alia_core::output_control::{OutputRetirementEvidence, OutputRetirementRequest};

fn request(f: &Fixture, scope: VoiceScope) -> OutputRetirementRequest {
    OutputRetirementRequest {
        scope,
        binding: Some(output_binding(scope)),
        expected_backend: Some(f.device.backend.clone()),
    }
}
async fn original_tail(
    f: &mut Fixture,
) -> (
    tonic::Streaming<proto::PlaybackReceipt>,
    Arc<AudioSubmission>,
) {
    let id = Uuid::new_v4().to_string();
    let mut output = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &id, 1, start()),
            frame(f.scope, &id, 1, audio(0, 123)),
            frame(f.scope, &id, 1, end(1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    until(&mut output, |receipt| receipt.end_accepted).await;
    let submission = f
        .device
        .submissions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    (output, submission)
}

#[tokio::test]
async fn retirement_after_replacement_waits_original_write_and_preserves_precise_cancel() {
    let mut f = Fixture::new(8).await;
    let (mut stream, original) = original_tail(&mut f).await;
    let write = original.try_begin_output().unwrap();
    let next = VoiceScope {
        session_id: Uuid::new_v4(),
        ..f.scope
    };
    f.session.activate(&next).unwrap();
    let replacement = f.register(next).await;
    let new_generation = f.device.state.lock().unwrap().0;
    // The original Register ACK could have been lost; exact binding remains.
    let retirement = OutputRetirementRequest {
        expected_backend: None,
        ..request(&f, f.scope)
    };
    let mut client = f.client.clone();
    let wire_request = codec::encode_retirement_request(&retirement);
    let mut task =
        tokio::spawn(async move { client.retire_output(authenticated(wire_request)).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(2100), &mut task)
            .await
            .is_err()
    );
    assert!(original.snapshot(0).observation_end.is_none());
    assert_eq!(f.session.active_scope().unwrap(), Some(next));
    assert_eq!(f.device.state.lock().unwrap().0, new_generation);
    write.finish(1);
    let receipt =
        codec::decode_retirement_receipt(&task.await.unwrap().unwrap().into_inner(), &retirement)
            .unwrap();
    let OutputRetirementEvidence::Settled { backend, progress } = receipt.evidence else {
        panic!("missing native evidence")
    };
    assert_eq!(backend, original.backend);
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].submitted_source_frames, Some(1));
    assert_eq!(progress[0].presented_source_frames, None);
    let exact = f
        .client
        .fence(authenticated(proto::FenceRequest {
            scope: Some(encode_scope(f.scope)),
            reason: "late exact cancellation".into(),
            close_session: false,
            playback_id: Some(original.identity.playback_id.to_string()),
            playback_sequence: Some(1),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(exact.final_progress.unwrap().submitted_samples, Some(1));
    assert_eq!(f.register(next).await, replacement);
    assert_eq!(f.device.state.lock().unwrap().0, new_generation);
    until(&mut stream, |receipt| receipt.observation_end != 0).await;
    drop(stream);
    f.finish().await;
}

#[tokio::test]
async fn no_native_retirement_fences_late_registration_but_unknown_and_conflicts_fail() {
    let mut f = Fixture::new(8).await;
    let native = request(&f, f.scope);
    let mut missing = native.clone();
    missing.binding = None;
    missing.expected_backend = None;
    assert!(
        f.client
            .retire_output(authenticated(codec::encode_retirement_request(&missing)))
            .await
            .is_err()
    );
    let mut wrong = native.clone();
    wrong.binding.as_mut().unwrap().context_id = Uuid::new_v4();
    assert!(
        f.client
            .retire_output(authenticated(codec::encode_retirement_request(&wrong)))
            .await
            .is_err()
    );
    assert_eq!(
        f.session.observe_control(&f.scope).unwrap().phase,
        ControlPhase::Running
    );
    let next = VoiceScope {
        session_id: Uuid::new_v4(),
        ..f.scope
    };
    f.session.activate(&next).unwrap();
    let empty = OutputRetirementRequest {
        scope: next,
        binding: Some(output_binding(next)),
        expected_backend: None,
    };
    let receipt = f
        .client
        .retire_output(authenticated(codec::encode_retirement_request(&empty)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        codec::decode_retirement_receipt(&receipt, &empty)
            .unwrap()
            .evidence,
        OutputRetirementEvidence::NoNativeOwner
    );
    assert!(f.session.register_output(&output_binding(next)).is_err());
    assert!(f.session.activate(&next).is_err());
    let unknown = OutputRetirementRequest {
        scope: VoiceScope {
            session_id: Uuid::new_v4(),
            ..next
        },
        binding: None,
        expected_backend: None,
    };
    assert_eq!(
        f.client
            .retire_output(authenticated(codec::encode_retirement_request(&unknown)))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    // The scope remains known even when no canonical Response was prepared.
    let no_binding = OutputRetirementRequest {
        binding: None,
        ..empty
    };
    let receipt = f
        .client
        .retire_output(authenticated(codec::encode_retirement_request(&no_binding)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        codec::decode_retirement_receipt(&receipt, &no_binding)
            .unwrap()
            .evidence,
        OutputRetirementEvidence::NoNativeOwner
    );
    f.finish().await;
}

#[tokio::test]
async fn retirement_capacity_never_evicts_entered_writes_and_evicted_is_unknown() {
    let f = Fixture::new(8).await;
    let mut current = f.scope;
    let original = request(&f, current);
    let mut writes = Vec::new();
    for index in 0..64 {
        let binding = output_binding(current);
        f.session.register_output(&binding).unwrap();
        let lease = f
            .session
            .begin_bound(&binding, &Uuid::new_v4().to_string(), 1, 16000)
            .unwrap();
        f.session.enqueue(&lease, &[1]).unwrap();
        f.session.end(&lease).unwrap();
        writes.push(lease.submission.try_begin_output().unwrap());
        let next = VoiceScope {
            session_id: Uuid::new_v4(),
            ..current
        };
        let activated = f.session.activate(&next);
        if index == 63 {
            assert!(
                activated.is_err(),
                "all retired slots still own entered writes"
            );
        } else {
            activated.unwrap();
        }
        current = next;
    }
    f.session.retire_output(&original).unwrap();
    writes.remove(0).finish(1);
    f.session.activate(&current).unwrap();
    assert!(
        f.session.retire_output(&original).is_err(),
        "evicted history cannot prove zero output"
    );
    for write in writes {
        write.finish(1);
    }
    f.finish().await;
}

#[tokio::test]
async fn retirement_rechecks_token_after_original_write_settles() {
    let mut f = Fixture::new(8).await;
    let (stream, original) = original_tail(&mut f).await;
    let write = original.try_begin_output().unwrap();
    let retirement = request(&f, f.scope);
    let wire = codec::encode_retirement_request(&retirement);
    let mut client = f.client.clone();
    let mut task = tokio::spawn(async move {
        client
            .retire_output(authenticated_for(wire, Duration::from_secs(1)))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(2100), &mut task)
            .await
            .is_err()
    );
    write.finish(1);
    assert_eq!(
        task.await.unwrap().unwrap_err().code(),
        tonic::Code::Unauthenticated
    );
    let receipt = f
        .client
        .retire_output(authenticated(codec::encode_retirement_request(&retirement)))
        .await
        .unwrap()
        .into_inner();
    codec::decode_retirement_receipt(&receipt, &retirement).unwrap();
    drop(stream);
    f.finish().await;
}
