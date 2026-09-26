use super::*;

fn resume(
    pause: &BoundControlCommand,
    sequence: u64,
    through_sequence: u64,
    kind: ResumeKind,
) -> BoundControlCommand {
    BoundControlCommand {
        command: ControlCommand {
            command_id: Uuid::new_v4(),
            sequence,
            action: ControlAction::Resume {
                authorization: ResumeAuthorization {
                    kind,
                    detection_id: Uuid::new_v4(),
                    through_sequence,
                },
            },
            ..pause.command.clone()
        },
        ..pause.clone()
    }
}

#[tokio::test]
async fn registration_is_exact_observe_unknown_does_not_apply_and_old_contract_cannot_register() {
    let mut f = Fixture::new(8).await;
    let first = f.register(f.scope).await;
    let repeat = f.register(f.scope).await;
    assert_eq!(first, repeat);
    let command = f.pause(HoldReason::AcousticCandidate, 30_000);
    let unknown = f
        .client
        .observe_control(authenticated(codec::encode_control_request(&command)))
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), tonic::Code::NotFound);
    assert_eq!(f.session.observe_control(&f.scope).unwrap().sequence, 0);
    for field in 0..4 {
        let mut changed = command.clone();
        match field {
            0 => changed.binding.response_id = Uuid::new_v4(),
            1 => changed.binding.context_id = Uuid::new_v4(),
            2 => changed.binding.server_instance_id = Uuid::new_v4(),
            _ => changed.expected_backend.output_channels = 2,
        }
        let status = f
            .client
            .control(authenticated(codec::encode_control_request(&changed)))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::AlreadyExists);
    }
    let mut changed = output_binding(f.scope);
    changed.response_id = Uuid::new_v4();
    assert_eq!(
        f.client
            .register_output(authenticated(proto::RegisterOutputRequest {
                binding: Some(codec::encode_binding(changed)),
                contract_version: PLAYBACK_CONTRACT_VERSION
            }))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::AlreadyExists
    );
    assert_eq!(
        f.client
            .register_output(authenticated(proto::RegisterOutputRequest {
                binding: Some(codec::encode_binding(output_binding(f.scope))),
                contract_version: 2
            }))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    let original_generation = f.device.state.lock().unwrap().0;
    let newer = VoiceScope {
        generation_epoch: 2,
        ..f.scope
    };
    f.session.fence(&newer, false).unwrap();
    let generation_after_fence = f.device.state.lock().unwrap().0;
    assert!(generation_after_fence > original_generation);
    let id = Uuid::new_v4().to_string();
    let mut output = f
        .client
        .push(authenticated(tokio_stream::iter([frame(
            newer,
            &id,
            2,
            start(),
        )])))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        output.message().await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    assert_eq!(f.device.state.lock().unwrap().0, generation_after_fence);
    assert!(f.device.submissions.lock().unwrap().is_empty());
    drop(output);
    f.finish().await;
}

#[tokio::test]
async fn pause_before_start_preserves_end_tail_and_old_resume_cannot_cross_new_local_speech() {
    let mut f = Fixture::new(8).await;
    let p = f.pause(HoldReason::ExplicitWait, 30_000);
    let held = f.command(&p).await;
    assert_eq!(held.receipt.snapshot.phase, ControlPhase::Held);
    assert!(held.native_writes_quiescent && held.progress.is_empty());
    let generation = f.device.state.lock().unwrap().0;
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
    until(&mut output, |r| r.end_accepted).await;
    let progress = f.observe(&p).await;
    assert_eq!(progress.receipt.outcome, CommandOutcome::Applied);
    assert_eq!(progress.receipt.snapshot.phase, ControlPhase::Held);
    assert_eq!(progress.progress[0].submitted_source_frames, Some(0));
    assert!(progress.progress[0].end_accepted);
    assert!(progress.observation_sequence > held.observation_sequence);
    // This frame is captured locally and still has no upload receipt.
    f.session
        .accept_capture(&f.capture, 1, true, || Ok(()))
        .unwrap();
    let stale = resume(&p, 2, 0, ResumeKind::ExplicitContinue);
    assert_eq!(
        f.command(&stale).await.receipt.outcome,
        CommandOutcome::Rejected(ControlRejection::StaleEvidence)
    );
    let backchannel = resume(&p, 3, 1, ResumeKind::FreshBackchannel);
    assert_eq!(
        f.command(&backchannel).await.receipt.outcome,
        CommandOutcome::Rejected(ControlRejection::ExplicitContinueRequired)
    );
    let continued = resume(&p, 4, 1, ResumeKind::ExplicitContinue);
    assert_eq!(
        f.command(&continued).await.receipt.snapshot.phase,
        ControlPhase::Running
    );
    assert_eq!(f.device.state.lock().unwrap().0, generation);
    assert_eq!(
        f.command(&stale).await.receipt.outcome,
        CommandOutcome::Rejected(ControlRejection::StaleEvidence)
    );
    let old_pause = f.observe(&p).await;
    assert_eq!(old_pause.receipt.outcome, CommandOutcome::Applied);
    assert_eq!(old_pause.receipt.snapshot.phase, ControlPhase::Running);
    f.device.submit_frames(&id, 1);
    assert_eq!(
        until(&mut output, |r| r.observation_end != 0)
            .await
            .submitted_samples,
        Some(1)
    );
    drop(output);
    f.finish().await;
}

#[tokio::test]
async fn independent_control_passes_a_full_media_queue_and_retains_original_bytes() {
    let mut f = Fixture::new(1).await;
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(4);
    tx.send(frame(f.scope, &id, 1, start())).await.unwrap();
    tx.send(frame(f.scope, &id, 1, audio(0, 31))).await.unwrap();
    let mut output = f
        .client
        .push(authenticated(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .unwrap()
        .into_inner();
    until(&mut output, |r| r.accepted_samples == 1).await;
    tx.send(frame(f.scope, &id, 1, audio(1, 32))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.device.blocked.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let p = f.pause(HoldReason::AcousticCandidate, 30_000);
    let held = tokio::time::timeout(Duration::from_secs(1), f.command(&p))
        .await
        .unwrap();
    assert_eq!(held.receipt.snapshot.phase, ControlPhase::Held);
    assert_eq!(f.device.state.lock().unwrap().1, [31]);
    assert_eq!(held.progress[0].accepted_source_frames, 1);
    assert!(f.device.blocked.load(Ordering::Acquire) > 0);
    f.command(&resume(&p, 2, 0, ResumeKind::FreshBackchannel))
        .await;
    f.device.submit_frames(&id, 1);
    until(&mut output, |r| r.accepted_samples == 2).await;
    assert_eq!(f.device.state.lock().unwrap().1, [32]);
    tx.send(frame(f.scope, &id, 1, end(2))).await.unwrap();
    drop(tx);
    until(&mut output, |r| r.end_accepted).await;
    f.device.submit_frames(&id, 1);
    assert_eq!(
        until(&mut output, |r| r.observation_end != 0)
            .await
            .submitted_samples,
        Some(2)
    );
    drop(output);
    f.finish().await;
}

#[tokio::test]
async fn held_freezes_media_idle_but_preserves_absolute_deadline_and_revocation() {
    let mut f = Fixture::new(8).await;
    let p = f.pause(HoldReason::ExplicitWait, MAX_HOLD_MS);
    f.command(&p).await;
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(4);
    tx.send(frame(f.scope, &id, 1, start())).await.unwrap();
    let mut output = f
        .client
        .push(authenticated(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .unwrap()
        .into_inner();
    receipt_from(&mut output).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::time::resume();
    let heartbeat = receipt_from(&mut output).await;
    assert_eq!(heartbeat.observation_end, 0);
    assert_eq!(
        f.observe(&p).await.receipt.snapshot.phase,
        ControlPhase::Held
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(115)).await;
    tokio::time::resume();
    let ended = until(&mut output, |r| r.observation_end != 0).await;
    assert_eq!(
        ended.observation_end,
        proto::ObservationEnd::Cancelled as i32
    );
    drop(tx);
    drop(output);
    f.binding.unbind("server").await.unwrap();
    assert!(f.session.active_scope().unwrap().is_none());
    assert_eq!(
        f.client
            .observe_control(authenticated(codec::encode_control_request(&p)))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    f.finish().await;
}

#[tokio::test]
async fn dropped_control_ack_is_observed_without_reapplying_or_renewing() {
    let mut f = Fixture::new(8).await;
    let id = Uuid::new_v4().to_string();
    let mut output = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &id, 1, start()),
            frame(f.scope, &id, 1, audio(0, 9)),
            frame(f.scope, &id, 1, end(1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    until(&mut output, |r| r.end_accepted).await;
    let submission = f
        .device
        .submissions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    let entered = submission.begin_output().unwrap();
    let p = f.pause(HoldReason::ExplicitWait, 30_000);
    let mut client = f.client.clone();
    let request = codec::encode_control_request(&p);
    let task = tokio::spawn(async move { client.control(authenticated(request)).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.session.observe_control(&f.scope).unwrap().phase != ControlPhase::Pausing {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    let observed_pausing = f.observe(&p).await;
    assert_eq!(observed_pausing.receipt.outcome, CommandOutcome::Applied);
    assert!(!observed_pausing.native_writes_quiescent);
    entered.finish(1);
    let observed_held = f.observe(&p).await;
    assert_eq!(observed_held.receipt.outcome, CommandOutcome::Applied);
    assert_eq!(observed_held.receipt.snapshot.phase, ControlPhase::Held);
    assert!(observed_held.observation_sequence > observed_pausing.observation_sequence);
    assert!(
        observed_held.receipt.snapshot.remaining_ms
            <= observed_pausing.receipt.snapshot.remaining_ms
    );
    let mut changed = p.clone();
    if let ControlAction::Pause { budget_ms, .. } = &mut changed.command.action {
        *budget_ms += 1;
    }
    assert_eq!(
        f.client
            .observe_control(authenticated(codec::encode_control_request(&changed)))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::AlreadyExists
    );
    assert_eq!(f.session.observe_control(&f.scope).unwrap().sequence, 1);
    drop(output);
    f.finish().await;
}

#[tokio::test]
async fn held_keeps_checking_original_push_token_expiry_and_input_disconnect() {
    let mut f = Fixture::new(8).await;
    let p = f.pause(HoldReason::ExplicitWait, 30_000);
    f.command(&p).await;
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(2);
    tx.send(frame(f.scope, &id, 1, start())).await.unwrap();
    let mut output = f
        .client
        .push(authenticated_for(
            tokio_stream::wrappers::ReceiverStream::new(rx),
            Duration::from_secs(2),
        ))
        .await
        .unwrap()
        .into_inner();
    receipt_from(&mut output).await;
    let expired = until(&mut output, |r| r.observation_end != 0).await;
    assert_eq!(
        expired.observation_end,
        proto::ObservationEnd::Cancelled as i32
    );
    assert!(f.device.state.lock().unwrap().1.is_empty());
    drop(tx);
    drop(output);
    let next = Uuid::new_v4().to_string();
    let mut output = f
        .client
        .push(authenticated(tokio_stream::iter([frame(
            f.scope,
            &next,
            2,
            start(),
        )])))
        .await
        .unwrap()
        .into_inner();
    let disconnected = until(&mut output, |r| r.observation_end != 0).await;
    assert_eq!(
        disconnected.observation_end,
        proto::ObservationEnd::Unknown as i32
    );
    drop(output);
    f.finish().await;
}

#[tokio::test]
async fn capture_gap_queue_failure_close_and_replacement_never_revive_old_registration() {
    let mut f = Fixture::new(8).await;
    let p = f.pause(HoldReason::AcousticCandidate, 30_000);
    f.command(&p).await;
    assert!(
        f.session
            .accept_capture(&f.capture, 1, true, || Err::<(), _>(anyhow::anyhow!(
                "upload full"
            )))
            .is_err()
    );
    assert!(f.session.register_output(&p.binding).is_err());
    assert_eq!(
        f.client
            .control(authenticated(codec::encode_control_request(&resume(
                &p,
                2,
                0,
                ResumeKind::FreshBackchannel
            ))))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    let replacement = VoiceScope {
        session_id: Uuid::new_v4(),
        ..f.scope
    };
    let next_capture = CaptureControlBarrier::default();
    f.session
        .accept_capture(&next_capture, 0, false, || Ok(()))
        .unwrap();
    f.session
        .activate_with_capture(&replacement, next_capture.clone(), MAX_HOLD_MS)
        .unwrap();
    f.register(replacement).await;
    f.session.retire_capture(&f.capture).unwrap();
    assert_eq!(f.session.active_scope().unwrap(), Some(replacement));
    assert!(
        f.session
            .accept_capture(&next_capture, 2, false, || Ok(()))
            .is_err()
    );
    assert!(
        f.session
            .register_output(&output_binding(replacement))
            .is_err()
    );
    f.session.close(&replacement).unwrap();
    assert_eq!(
        f.client
            .observe_control(authenticated(codec::encode_control_request(&p)))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::NotFound
    );
    f.finish().await;
}
