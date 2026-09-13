//! Real PostgreSQL delivery state and fault tests.
mod delivery_support;
use alia_core::delivery::{ObservationEnd, SubmissionBackend, SubmissionBoundary};
use alia_server::modules::delivery::types::*;
use delivery_support::*;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use serde_json::json;
use uuid::Uuid;
#[tokio::test]
async fn policy_revocation_blocks_new_audio_admission_and_retains_inflight_facts() {
    with_db(|f| async move {
        let (response, input, record) = f.prepared().await;
        let repo = f.repo();
        let permit = repo
            .dispatch(f.user, record.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        repo.offered(&permit, 100).await.unwrap();
        let mut accepted = receipt(input.identity.unwrap(), 1, 100);
        accepted.end_accepted = true;
        repo.progress(&permit, accepted.clone()).await.unwrap();
        let contexts = f.context();
        let policy = contexts.policy_store();
        let current = policy.current().await.unwrap();
        let mut next = current.policy;
        next.max_history_messages = 7;
        policy.replace(current.version, next).await.unwrap();
        assert!(matches!(
            repo.heartbeat(&permit).await,
            Err(DeliveryError::Unauthorized(_))
        ));
        assert!(matches!(
            repo.offered(&permit, 101).await,
            Err(DeliveryError::Unauthorized(_))
        ));
        assert!(matches!(
            repo.prepare_delivery(
                f.user,
                NewDelivery {
                    output_key: "late-sentence".into(),
                    ..input.clone()
                }
            )
            .await,
            Err(DeliveryError::Unauthorized(_))
        ));
        let tx = f.db.begin().await.unwrap();
        assert!(matches!(
            repo.mark_generated_in_transaction(
                &tx,
                f.user,
                response.response_id,
                Uuid::new_v4(),
                json!({"text":"late result"})
            )
            .await,
            Err(DeliveryError::Unauthorized(_))
        ));
        tx.rollback().await.unwrap();

        accepted.receipt_sequence = 2;
        accepted.submitted_source_frames = Some(100);
        accepted.backend = Some(SubmissionBackend {
            instance_id: Uuid::new_v4(),
            boundary: SubmissionBoundary::AlsaWriteAccepted,
            output_sample_rate_hz: 48000,
            output_channels: 2,
        });
        accepted.observation_end = Some(ObservationEnd::SubmissionFinished);
        let final_record = repo.progress(&permit, accepted).await.unwrap();
        assert_eq!(final_record.status, DeliveryStatus::Submitted);
        assert_eq!(final_record.submitted_frames, Some(100));
        assert_eq!(final_record.presented_frames, None);
        let fresh = f.response().await;
        assert!(repo.prepare(fresh).await.is_ok());
    })
    .await;
}

#[tokio::test]
async fn concurrent_response_intent_and_delivery_reservation_are_idempotent() {
    with_db(|f| async move {
        let input = f.response().await;
        let repo = f.repo();
        let results = futures::future::join_all((0..12).map(|_| repo.prepare(input.clone()))).await;
        let id = results[0].as_ref().unwrap().response_id;
        assert!(
            results
                .iter()
                .all(|result| result.as_ref().unwrap().response_id == id)
        );
        assert_eq!(f.count("delivery_responses").await, 1);
        let mut retry = input.clone();
        retry.source_received_at += chrono::Duration::minutes(1);
        assert!(matches!(
            repo.prepare(retry).await,
            Err(DeliveryError::Unauthorized(_))
        ));
        for mode in 0..3 {
            let mut changed = input.clone();
            match mode {
                0 => changed.source_privacy_epoch = Some(1),
                1 => changed.content = json!({"other":true}),
                _ => changed.source_event_id = Some(Uuid::new_v4()),
            };
            let error = repo.prepare(changed).await.unwrap_err();
            if mode == 1 {
                assert!(matches!(error, DeliveryError::IdempotencyConflict));
            } else {
                assert!(matches!(error, DeliveryError::Unauthorized(_)));
            }
        }
        let delivery = NewDelivery {
            response_id: id,
            output_key: "sentence:0".into(),
            content: json!({"text":"hello"}),
            identity: Some(identity()),
            owner_id: Uuid::new_v4(),
        };
        let results = futures::future::join_all(
            (0..8).map(|_| repo.prepare_delivery(f.user, delivery.clone())),
        )
        .await;
        let id = results[0].as_ref().unwrap().delivery_id;
        assert!(
            results
                .iter()
                .all(|result| result.as_ref().unwrap().delivery_id == id)
        );
        assert_eq!(f.count("deliveries").await, 1);
        let mut changed = delivery;
        changed.identity = Some(identity());
        assert!(matches!(
            repo.prepare_delivery(f.user, changed).await,
            Err(DeliveryError::IdempotencyConflict)
        ));
    })
    .await;
}

#[tokio::test]
async fn only_one_dispatch_permit_and_no_replay_after_dispatch_ack_loss() {
    with_db(|f| async move {
        let (_, input, record) = f.prepared().await;
        let repo = f.repo();
        let results = futures::future::join_all(
            (0..8).map(|_| repo.dispatch(f.user, record.delivery_id, input.owner_id)),
        )
        .await;
        let permits = results
            .into_iter()
            .map(Result::unwrap)
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(permits.len(), 1);
        drop(permits);
        assert!(
            repo.dispatch(f.user, record.delivery_id, input.owner_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repo.get(f.user, record.delivery_id).await.unwrap().status,
            DeliveryStatus::Sending
        );
    })
    .await;
}

#[tokio::test]
async fn end_accepted_is_observing_and_submission_never_invents_presentation() {
    with_db(|f| async move {
        let (_, input, record) = f.prepared().await;
        let repo = f.repo();
        let permit = repo
            .dispatch(f.user, record.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        repo.offered(&permit, 100).await.unwrap();
        let mut end = receipt(input.identity.unwrap(), 1, 100);
        end.end_accepted = true;
        let record = repo.progress(&permit, end.clone()).await.unwrap();
        assert_eq!(record.status, DeliveryStatus::Observing);
        assert_eq!(record.presented_frames, None);
        let mut submitted = end;
        submitted.receipt_sequence = 2;
        submitted.submitted_source_frames = Some(100);
        submitted.backend = Some(SubmissionBackend {
            instance_id: Uuid::new_v4(),
            boundary: SubmissionBoundary::AlsaWriteAccepted,
            output_sample_rate_hz: 48000,
            output_channels: 2,
        });
        submitted.observation_end = Some(ObservationEnd::SubmissionFinished);
        let record = repo.progress(&permit, submitted.clone()).await.unwrap();
        assert_eq!(record.status, DeliveryStatus::Submitted);
        assert_eq!(record.presented_frames, None);
        assert_eq!(
            repo.progress(&permit, submitted.clone())
                .await
                .unwrap()
                .status,
            DeliveryStatus::Submitted
        );
        submitted.receipt_sequence = 3;
        submitted.presented_source_frames = Some(100);
        submitted.observation_end = Some(ObservationEnd::PresentedFinished);
        assert!(repo.progress(&permit, submitted).await.is_err());
        assert!(
            repo.dispatch(f.user, record.delivery_id, input.owner_id)
                .await
                .unwrap()
                .is_none()
        );
    })
    .await;
}

#[tokio::test]
async fn rejects_bad_identity_regression_and_counts_above_durable_offer() {
    with_db(|f| async move {
        let (_, input, record) = f.prepared().await;
        let repo = f.repo();
        let permit = repo
            .dispatch(f.user, record.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        assert!(
            repo.progress(&permit, receipt(input.identity.unwrap(), 1, 1))
                .await
                .is_err()
        );
        repo.offered(&permit, 100).await.unwrap();
        let first = receipt(input.identity.unwrap(), 1, 50);
        repo.progress(&permit, first.clone()).await.unwrap();
        for mode in 0..5 {
            let mut bad = first.clone();
            bad.receipt_sequence = 2;
            match mode {
                0 => bad.accepted_source_frames = 49,
                1 => bad.accepted_source_frames = 101,
                2 => bad.identity.scope.generation_epoch += 1,
                3 => bad.observation_end = Some(ObservationEnd::PresentedFinished),
                _ => {
                    bad.receipt_sequence = 1;
                    bad.accepted_source_frames = 51;
                }
            };
            assert!(repo.progress(&permit, bad).await.is_err(), "mode {mode}");
        }
        assert_eq!(
            repo.get(f.user, record.delivery_id)
                .await
                .unwrap()
                .accepted_frames,
            50
        );
    })
    .await;
}

#[tokio::test]
async fn receipt_and_final_status_roll_back_together_on_audit_failure() {
    with_db(|f|async move{
 let (_,input,record)=f.prepared().await;let repo=f.repo();let permit=repo.dispatch(f.user,record.delivery_id,input.owner_id).await.unwrap().unwrap();repo.start(&permit,24000,1).await.unwrap();repo.offered(&permit,20).await.unwrap();
 f.sql("CREATE FUNCTION fail_progress() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='progress' THEN RAISE EXCEPTION 'injected receipt audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_progress BEFORE INSERT ON delivery_events FOR EACH ROW EXECUTE FUNCTION fail_progress()").await;
 let mut progress=receipt(input.identity.unwrap(),1,20);progress.end_accepted=true;assert!(repo.progress(&permit,progress.clone()).await.is_err());let old=repo.get(f.user,record.delivery_id).await.unwrap();assert_eq!(old.receipt_sequence,0);assert_eq!(old.accepted_frames,0);assert_eq!(old.status,DeliveryStatus::Sending);
 f.sql("DROP TRIGGER fail_progress ON delivery_events").await;assert_eq!(repo.progress(&permit,progress).await.unwrap().status,DeliveryStatus::Observing);
}).await;
}

#[tokio::test]
async fn dispatch_audit_failure_gives_no_permit_and_preserves_pending() {
    with_db(|f|async move{
 let (_,input,record)=f.prepared().await;let repo=f.repo();f.sql("CREATE FUNCTION fail_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='dispatch' THEN RAISE EXCEPTION 'dispatch audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_dispatch BEFORE INSERT ON delivery_events FOR EACH ROW EXECUTE FUNCTION fail_dispatch()").await;
 assert!(repo.dispatch(f.user,record.delivery_id,input.owner_id).await.is_err());let old=repo.get(f.user,record.delivery_id).await.unwrap();assert_eq!(old.status,DeliveryStatus::Pending);assert!(old.dispatch_id.is_none());
 f.sql("DROP TRIGGER fail_dispatch ON delivery_events").await;assert!(repo.dispatch(f.user,record.delivery_id,input.owner_id).await.unwrap().is_some());
}).await;
}

#[tokio::test]
async fn cancel_request_preserves_prefix_until_exact_fence_receipt() {
    with_db(|f| async move {
        let (response, input, record) = f.prepared().await;
        let repo = f.repo();
        let permit = repo
            .dispatch(f.user, record.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        repo.offered(&permit, 100).await.unwrap();
        let first = receipt(input.identity.unwrap(), 1, 40);
        repo.progress(&permit, first.clone()).await.unwrap();
        let cancel = repo
            .request_cancel(f.user, response.response_id, "user cancelled")
            .await
            .unwrap();
        assert_eq!(cancel[0].status, DeliveryStatus::Sending);
        assert!(cancel[0].cancel_requested);
        assert_eq!(
            repo.response(f.user, response.response_id)
                .await
                .unwrap()
                .status,
            ResponseStatus::Interrupted
        );
        assert!(repo.offered(&permit, 101).await.is_err());
        let mut acknowledged = first;
        acknowledged.receipt_sequence = 2;
        acknowledged.observation_end = Some(ObservationEnd::Cancelled {
            reason: "exact playback fence acknowledged".into(),
        });
        let cancelled = repo.progress(&permit, acknowledged).await.unwrap();
        assert_eq!(cancelled.status, DeliveryStatus::Cancelled);
        assert_eq!(cancelled.accepted_frames, 40);
        assert_eq!(cancelled.presented_frames, None);
        assert!(
            repo.prepare_delivery(
                f.user,
                NewDelivery {
                    output_key: "sentence:1".into(),
                    identity: Some(identity()),
                    ..input
                }
            )
            .await
            .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn owner_lease_recovery_changes_only_expired_rows_and_never_replays() {
    with_db(|f|async move{
 let (_,input,old)=f.prepared().await;let repo=f.repo();let permit=repo.dispatch(f.user,old.delivery_id,input.owner_id).await.unwrap().unwrap();repo.start(&permit,24000,1).await.unwrap();repo.offered(&permit,50).await.unwrap();repo.progress(&permit,receipt(input.identity.unwrap(),1,30)).await.unwrap();
 let (_,_,live)=f.prepared().await;let (_,_,pending)=f.prepared().await;
 f.sql(&format!("UPDATE deliveries SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE delivery_id IN ('{}','{}')",old.delivery_id,pending.delivery_id)).await;
 assert_eq!(repo.recover_expired(256).await.unwrap(),2);assert_eq!(repo.recover_expired(256).await.unwrap(),0);let recovered=repo.get(f.user,old.delivery_id).await.unwrap();assert_eq!(recovered.status,DeliveryStatus::Unknown);assert_eq!(recovered.accepted_frames,30);assert_eq!(repo.get(f.user,pending.delivery_id).await.unwrap().status,DeliveryStatus::Cancelled);assert_eq!(repo.get(f.user,live.delivery_id).await.unwrap().status,DeliveryStatus::Pending);assert!(repo.dispatch(f.user,old.delivery_id,input.owner_id).await.unwrap().is_none());
}).await;
}

#[tokio::test]
async fn response_history_commit_is_atomic_and_freezes_final_content() {
    with_db(|f|async move{
 let session=Uuid::new_v4();f.sql(&format!("INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES('{session}','test-device','{}')",f.user)).await;let input=f.response_in_session(session).await;let repo=f.repo();let response=repo.prepare(input).await.unwrap();let message=Uuid::new_v4();
 let tx=f.db.begin().await.unwrap();repo.mark_generated_in_transaction(&tx,f.user,response.response_id,message,json!({"text":"hello"})).await.unwrap();tx.rollback().await.unwrap();assert_eq!(repo.response(f.user,response.response_id).await.unwrap().status,ResponseStatus::Pending);
 let tx=f.db.begin().await.unwrap();let committed=repo.mark_generated_in_transaction(&tx,f.user,response.response_id,message,json!({"text":"hello"})).await.unwrap();tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,role,content) VALUES($1,$2,'assistant','hello')",vec![message.into(),session.into()])).await.unwrap();tx.commit().await.unwrap();assert_eq!(committed.assistant_message_id,Some(message));
 let tx=f.db.begin().await.unwrap();let replay=repo.mark_generated_in_transaction(&tx,f.user,response.response_id,Uuid::new_v4(),json!({"text":"hello"})).await.unwrap();assert_eq!(replay.assistant_message_id,Some(message));assert!(matches!(repo.mark_generated_in_transaction(&tx,f.user,response.response_id,Uuid::new_v4(),json!({"text":"different"})).await,Err(DeliveryError::IdempotencyConflict)));tx.rollback().await.unwrap();assert_eq!(f.assistant_count().await,1);
}).await;
}

#[tokio::test]
async fn reads_and_cancellation_cannot_cross_owners_and_revocation_blocks_dispatch() {
    with_db(|f| async move {
        let (response, input, record) = f.prepared().await;
        let repo = f.repo();
        let stranger = Uuid::new_v4();
        assert!(matches!(
            repo.response(stranger, response.response_id).await,
            Err(DeliveryError::NotFound)
        ));
        assert!(matches!(
            repo.get(stranger, record.delivery_id).await,
            Err(DeliveryError::NotFound)
        ));
        assert!(repo.list(stranger, response.response_id).await.is_err());
        assert!(
            repo.events(stranger, response.response_id, 0, 100)
                .await
                .is_err()
        );
        assert!(
            repo.request_cancel(stranger, response.response_id, "no")
                .await
                .is_err()
        );
        f.sql(&format!(
            "UPDATE users SET status='disabled' WHERE id='{}'",
            f.user
        ))
        .await;
        assert!(matches!(
            repo.dispatch(f.user, record.delivery_id, input.owner_id)
                .await,
            Err(DeliveryError::Unauthorized(_))
        ));
        assert_eq!(
            repo.get(f.user, record.delivery_id).await.unwrap().status,
            DeliveryStatus::Pending
        );
    })
    .await;
}

#[tokio::test]
async fn privacy_epoch_fences_every_new_media_boundary_but_keeps_observed_facts() {
    with_db(|f| async move {
        let (_response, input, record) = f.prepared().await;
        let repo = f.repo();
        f.sql(&format!(
            "UPDATE memory_privacy_state SET privacy_epoch=1 WHERE user_id='{}'",
            f.user
        ))
        .await;
        assert!(matches!(
            repo.dispatch(f.user, record.delivery_id, input.owner_id)
                .await,
            Err(DeliveryError::Unauthorized(_))
        ));
        assert!(
            repo.prepare_delivery(
                f.user,
                NewDelivery {
                    output_key: "sentence:1".into(),
                    identity: Some(identity()),
                    ..input.clone()
                }
            )
            .await
            .is_err()
        );
        let mut new_response = f.response().await;
        new_response.source_privacy_epoch = Some(1);
        let response = repo.prepare(new_response).await.unwrap();
        let delivery = repo
            .prepare_delivery(
                f.user,
                NewDelivery {
                    response_id: response.response_id,
                    output_key: "new:0".into(),
                    identity: Some(identity()),
                    ..input.clone()
                },
            )
            .await
            .unwrap();
        let permit = repo
            .dispatch(f.user, delivery.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        f.sql(&format!(
            "UPDATE memory_privacy_state SET privacy_epoch=2 WHERE user_id='{}'",
            f.user
        ))
        .await;
        assert!(repo.start(&permit, 24000, 1).await.is_err());
        assert!(repo.heartbeat(&permit).await.is_err());
        assert_eq!(
            repo.finish_unknown(&permit, "source retired before start")
                .await
                .unwrap()
                .status,
            DeliveryStatus::Unknown
        );
        let mut new_response = f.response().await;
        new_response.source_privacy_epoch = Some(2);
        let response = repo.prepare(new_response).await.unwrap();
        let delivery = repo
            .prepare_delivery(
                f.user,
                NewDelivery {
                    response_id: response.response_id,
                    identity: Some(identity()),
                    output_key: "new:1".into(),
                    ..input
                },
            )
            .await
            .unwrap();
        let permit = repo
            .dispatch(f.user, delivery.delivery_id, delivery.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        repo.offered(&permit, 100).await.unwrap();
        f.sql(&format!(
            "UPDATE memory_privacy_state SET privacy_epoch=3 WHERE user_id='{}'",
            f.user
        ))
        .await;
        assert!(repo.offered(&permit, 101).await.is_err());
        assert!(repo.heartbeat(&permit).await.is_err());
        let progress = receipt(delivery.identity.unwrap(), 1, 40);
        assert_eq!(
            repo.progress(&permit, progress)
                .await
                .unwrap()
                .accepted_frames,
            40
        );
        let mut final_progress = receipt(delivery.identity.unwrap(), 2, 40);
        final_progress.observation_end = Some(ObservationEnd::Cancelled {
            reason: "fence acknowledged after privacy clear".into(),
        });
        let cancelled = repo
            .terminal_checkpoint(&permit, final_progress)
            .await
            .unwrap();
        assert_eq!(cancelled.status, DeliveryStatus::Cancelled);
        assert_eq!(cancelled.accepted_frames, 40);
    })
    .await;
}

#[tokio::test]
async fn first_visible_cancel_checkpoint_after_ack_loss_is_valid_and_late_facts_do_not_reopen_unknown()
 {
    with_db(|f| async move {
        let (_, input, record) = f.prepared().await;
        let repo = f.repo();
        let permit = repo
            .dispatch(f.user, record.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        repo.offered(&permit, 100).await.unwrap();
        let mut checkpoint = receipt(record.identity.unwrap(), 9, 60);
        checkpoint.observation_end = Some(ObservationEnd::Cancelled {
            reason: "exact output quiescent".into(),
        });
        f.sql(&format!("UPDATE deliveries SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE delivery_id='{}'",record.delivery_id)).await;
        let final_record = repo
            .terminal_checkpoint(&permit, checkpoint.clone())
            .await
            .unwrap();
        assert_eq!(final_record.status, DeliveryStatus::Cancelled);
        assert_eq!(final_record.receipt_sequence, 9);
        assert_eq!(
            repo.terminal_checkpoint(&permit, checkpoint)
                .await
                .unwrap()
                .status,
            DeliveryStatus::Cancelled
        );

        let (_, input, record) = f.prepared().await;
        let permit = repo
            .dispatch(f.user, record.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        repo.offered(&permit, 100).await.unwrap();
        repo.progress(&permit, receipt(record.identity.unwrap(), 1, 20))
            .await
            .unwrap();
        repo.finish_unknown(&permit, "checkpoint RPC timeout")
            .await
            .unwrap();
        let mut checkpoint = receipt(record.identity.unwrap(), 7, 60);
        checkpoint.observation_end = Some(ObservationEnd::Cancelled {
            reason: "late exact quiescent checkpoint".into(),
        });
        let late = repo.terminal_checkpoint(&permit, checkpoint.clone()).await.unwrap();
        repo.terminal_checkpoint(&permit, checkpoint).await.unwrap();
        assert_eq!(late.status, DeliveryStatus::Unknown);
        assert_eq!(late.accepted_frames, 20);
        let events = repo
            .events(f.user, record.response_id, 0, 256)
            .await
            .unwrap();
        assert_eq!(events.iter().filter(|event| event.kind == "late_checkpoint"
            && event.payload["accepted_source_frames"] == 60).count(), 1);
        assert!(
            repo.dispatch(f.user, record.delivery_id, input.owner_id)
                .await
                .unwrap()
                .is_none()
        );
    })
    .await;
}

#[tokio::test]
async fn cleared_source_cannot_commit_old_assistant_history() {
    with_db(|f|async move{
 let repo=f.repo();let session=Uuid::new_v4();f.sql(&format!("INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES('{session}','test-device','{}')",f.user)).await;let input=f.response_in_session(session).await;let response=repo.prepare(input).await.unwrap();f.sql(&format!("UPDATE memory_privacy_state SET privacy_epoch=1 WHERE user_id='{}'",f.user)).await;
 let tx=f.db.begin().await.unwrap();assert!(matches!(repo.mark_generated_in_transaction(&tx,f.user,response.response_id,Uuid::new_v4(),json!({"text":"old response"})).await,Err(DeliveryError::Unauthorized(_))));tx.rollback().await.unwrap();assert_eq!(repo.response(f.user,response.response_id).await.unwrap().status,ResponseStatus::Pending);
 let mut unstamped=f.response().await;unstamped.source_privacy_epoch=None;assert!(matches!(repo.prepare(unstamped).await,Err(DeliveryError::Unauthorized(_))));
}).await;
}

#[tokio::test]
async fn progress_audit_foreign_key_does_not_invert_response_delivery_lock_order() {
    with_db(|f| async move {
        let (response, input, record) = f.prepared().await;
        let repo = f.repo();
        let permit = repo
            .dispatch(f.user, record.delivery_id, input.owner_id)
            .await
            .unwrap()
            .unwrap();
        repo.start(&permit, 24000, 1).await.unwrap();
        repo.offered(&permit, 100).await.unwrap();
        let tx = f.db.begin().await.unwrap();
        tx.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT response_id FROM delivery_responses WHERE response_id=$1 FOR NO KEY UPDATE",
            [response.response_id.into()],
        ))
        .await
        .unwrap();
        let progress = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            repo.progress(&permit, receipt(input.identity.unwrap(), 1, 50)),
        )
        .await
        .expect("audit FK must not wait for response's non-key status lock")
        .unwrap();
        assert_eq!(progress.accepted_frames, 50);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tx.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT delivery_id FROM deliveries WHERE delivery_id=$1 FOR UPDATE",
                [record.delivery_id.into()],
            )),
        )
        .await
        .unwrap()
        .unwrap();
        tx.rollback().await.unwrap();
    })
    .await;
}
