//! Durable physical-output intent, source revocation and late factual evidence.
mod delivery_support;
use alia_core::body::*;
use alia_server::modules::body_delivery::*;
use delivery_support::*;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

fn command(owner: Uuid) -> BodyCommand {
    BodyCommand {
        identity: BodyCommandIdentity {
            lease: BodyControlLease {
                device_boot_id: Uuid::new_v4(),
                server_instance_id: Uuid::new_v4(),
                generation: 1,
            },
            sequence: 1,
        },
        plan_id: Uuid::new_v4(),
        source_command_id: Uuid::new_v4().to_string(),
        owner_id: owner,
        scope: BodyScope::Nonvoice {
            scope_id: Uuid::new_v4(),
        },
        priority: BodyPriority::Normal,
        interruptible: true,
        actions: vec![BodyAction {
            duration_ms: 400,
            intensity: 0.5,
            motion: BodyMotion::SmallNod,
        }],
    }
}
fn progress(command: &BodyCommand, sequence: u64, frames: u64) -> BodyProgress {
    BodyProgress {
        identity: command.identity,
        receipt_sequence: sequence,
        accepted_actions: 1,
        started_actions: u32::from(frames > 0),
        submitted_actions: 0,
        submitted_frames: frames,
        partial_frame: None,
        last_partial_channels: vec![],
        last_uncertain_channels: vec![],
        backend: Some(BodyBackend {
            instance_id: command.identity.lease.device_boot_id,
            boundary: BodySubmissionBoundary::Simulated,
            channels: 3,
        }),
        writer_quiescent: false,
        position_confirmed_actions: None,
        end: None,
    }
}
async fn setup(
    f: &Fixture,
) -> (
    BodyDeliveryRepository,
    BodyDeliveryOrigin,
    BodyCommand,
    Uuid,
    Uuid,
) {
    let response = f.repo().prepare(f.response()).await.unwrap();
    let origin = BodyDeliveryOrigin {
        response_id: response.response_id,
        output_key: "speech_start".into(),
    };
    let command = command(f.user);
    let repo = BodyDeliveryRepository::new(f.db.clone());
    let BodyDispatch::Fresh {
        delivery_id,
        dispatch_id,
    } = repo.claim(&origin, "test-device", &command).await.unwrap()
    else {
        panic!("first claim must be unique");
    };
    (repo, origin, command, delivery_id, dispatch_id)
}

#[tokio::test]
async fn concurrent_claims_grant_one_physical_permission_and_preserve_original_identity() {
    with_db(|f| async move {
        let response = f.repo().prepare(f.response()).await.unwrap();
        let origin = BodyDeliveryOrigin {
            response_id: response.response_id,
            output_key: "pre_speech".into(),
        };
        let command = command(f.user);
        let repo = BodyDeliveryRepository::new(f.db.clone());
        let results = futures::future::join_all(
            (0..12).map(|_| repo.claim(&origin, "test-device", &command)),
        )
        .await;
        let mut fresh = 0;
        for result in results {
            match result.unwrap() {
                BodyDispatch::Fresh { .. } => fresh += 1,
                BodyDispatch::Existing(old) => assert_eq!(old.command, command),
            }
        }
        assert_eq!(fresh, 1);
        assert_eq!(f.count("body_deliveries").await, 1);
        let mut restart = command.clone();
        restart.identity.lease = BodyControlLease {
            device_boot_id: Uuid::new_v4(),
            server_instance_id: Uuid::new_v4(),
            generation: 99,
        };
        restart.identity.sequence = 3;
        restart.source_command_id = "new server instance correlation".into();
        let BodyDispatch::Existing(old) =
            repo.claim(&origin, "test-device", &restart).await.unwrap()
        else {
            panic!("restart cannot reacquire send permission");
        };
        assert_eq!(old.command, command);
        restart.actions[0].intensity = 0.9;
        assert!(repo.claim(&origin, "test-device", &restart).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn audit_failure_rolls_back_permission_before_external_output() {
    with_db(|f| async move {
        let response = f.repo().prepare(f.response()).await.unwrap();
        let origin = BodyDeliveryOrigin { response_id: response.response_id, output_key: "pre_speech".into() };
        let command = command(f.user);
        let repo = BodyDeliveryRepository::new(f.db.clone());
        f.sql("CREATE FUNCTION deny_body_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='body_dispatch' THEN RAISE EXCEPTION 'injected body audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER deny_body BEFORE INSERT ON delivery_events FOR EACH ROW EXECUTE FUNCTION deny_body_dispatch()").await;
        assert!(repo.claim(&origin,"test-device",&command).await.is_err());
        assert_eq!(f.count("body_deliveries").await,0);
        f.sql("DROP TRIGGER deny_body ON delivery_events").await;
        assert!(matches!(repo.claim(&origin,"test-device",&command).await.unwrap(),BodyDispatch::Fresh{..}));
    }).await;
}

#[tokio::test]
async fn revocation_stops_admission_but_keeps_actual_submitted_facts() {
    with_db(|f| async move {
        let (repo, origin, command, id, permit) = setup(&f).await;
        assert!(repo.authorize(id, permit).await.unwrap());
        f.sql("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1")
            .await;
        assert!(repo.authorize(id, permit).await.is_err());
        assert!(repo.claim(&origin, "test-device", &command).await.is_err());
        let original = repo
            .lookup_origin(&origin, "test-device", &command)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(original.delivery_id, id);
        assert_eq!(original.command, command);
        assert!(
            repo.lookup_origin(&origin, "wrong-device", &command)
                .await
                .is_err()
        );
        let mut submitted = progress(&command, 1, 20);
        submitted.submitted_actions = 1;
        submitted.writer_quiescent = true;
        submitted.end = Some(BodyEnd::Submitted);
        repo.progress(id, permit, &submitted).await.unwrap();
        let row = repo.get(f.user, id).await.unwrap();
        assert_eq!(row.status, "submitted");
        assert!(row.progress.unwrap().position_confirmed_actions.is_none());
    })
    .await;
}

#[tokio::test]
async fn parent_cancellation_and_absolute_deadline_cannot_be_renewed() {
    with_db(|f| async move {
        let (repo, origin, _, id, permit) = setup(&f).await;
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"UPDATE delivery_responses SET cancel_requested=true WHERE response_id=$1",[origin.response_id.into()])).await.unwrap();
        assert!(!repo.authorize(id,permit).await.unwrap());
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"UPDATE delivery_responses SET cancel_requested=false WHERE response_id=$1",[origin.response_id.into()])).await.unwrap();
        f.sql("UPDATE body_deliveries SET observation_deadline_at=clock_timestamp()-interval '1 second'").await;
        assert!(!repo.authorize(id,permit).await.unwrap());
        repo.recover_expired().await.unwrap();
        assert_eq!(repo.get(f.user,id).await.unwrap().status,"unknown");
        assert!(!repo.authorize(id,permit).await.unwrap());
    }).await;
}

#[tokio::test]
async fn unknown_freezes_prefix_and_keeps_idempotent_ordered_late_checkpoints() {
    with_db(|f| async move {
        let (repo, _, command, id, permit) = setup(&f).await;
        let first = progress(&command, 1, 3);
        repo.progress(id, permit, &first).await.unwrap();
        repo.unknown(id, permit, "RPC confirmation lost")
            .await
            .unwrap();
        let mut late = progress(&command, 2, 5);
        late.writer_quiescent = true;
        late.end = Some(BodyEnd::Cancelled {
            reason: "exact cancellation confirmed".into(),
        });
        repo.progress(id, permit, &late).await.unwrap();
        repo.progress(id, permit, &late).await.unwrap();
        assert_eq!(f.count("body_delivery_late_checkpoints").await, 1);
        let row = repo.get(f.user, id).await.unwrap();
        assert_eq!(row.status, "unknown");
        assert_eq!(row.progress, Some(first));
        assert_eq!(row.latest_checkpoint, Some(late.clone()));
        let mut regressed = late.clone();
        regressed.receipt_sequence = 3;
        regressed.submitted_frames = 4;
        assert!(repo.progress(id, permit, &regressed).await.is_err());
        regressed = late.clone();
        regressed.submitted_frames = 6;
        assert!(repo.progress(id, permit, &regressed).await.is_err());
        let mut revived = late.clone();
        revived.receipt_sequence = 3;
        revived.submitted_actions = 1;
        revived.end = Some(BodyEnd::Submitted);
        assert!(repo.progress(id, permit, &revived).await.is_err());
        assert!(
            f.repo()
                .events(f.user, row.response_id, 0, 64)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == "body_late_checkpoint")
        );
    })
    .await;
}

#[tokio::test]
async fn wrong_owner_channel_scope_or_dispatch_cannot_touch_body_delivery() {
    with_db(|f| async move {
        let (repo, origin, command, id, permit) = setup(&f).await;
        assert!(repo.get(Uuid::new_v4(), id).await.is_err());
        assert!(
            repo.claim(&origin, "replacement-device", &command)
                .await
                .is_err()
        );
        assert!(
            repo.progress(id, Uuid::new_v4(), &progress(&command, 1, 0))
                .await
                .is_err()
        );
        let mut wrong = command.clone();
        wrong.owner_id = Uuid::new_v4();
        assert!(repo.claim(&origin, "test-device", &wrong).await.is_err());
        wrong = command.clone();
        wrong.scope = BodyScope::Voice {
            scope: alia_core::delivery::VoiceScope {
                device_boot_id: command.identity.lease.device_boot_id,
                session_id: Uuid::new_v4(),
                generation_epoch: 1,
            },
        };
        assert!(repo.claim(&origin, "test-device", &wrong).await.is_err());
        let mut forged = progress(&command, 1, 5);
        forged.position_confirmed_actions = Some(1);
        assert!(repo.progress(id, permit, &forged).await.is_err());
        forged.position_confirmed_actions = None;
        forged.end = Some(BodyEnd::Cancelled {
            reason: "no native proof".into(),
        });
        assert!(repo.progress(id, permit, &forged).await.is_err());
        assert_eq!(
            repo.for_response(f.user, origin.response_id)
                .await
                .unwrap()
                .len(),
            1
        );
    })
    .await;
}
