//! Durable physical-output intent, source revocation and late factual evidence.
mod delivery_support;
use alia_core::embodiment::motion::*;
use alia_server::modules::embodiment::motion_delivery::*;
use delivery_support::*;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

fn command(owner: Uuid) -> MotionCommand {
    MotionCommand {
        identity: MotionCommandIdentity {
            lease: MotionControlLease {
                device_boot_id: Uuid::new_v4(),
                server_instance_id: Uuid::new_v4(),
                generation: 1,
            },
            sequence: 1,
        },
        plan_id: Uuid::new_v4(),
        source_command_id: Uuid::new_v4().to_string(),
        owner_id: owner,
        scope: MotionScope::Nonvoice {
            scope_id: Uuid::new_v4(),
        },
        priority: MotionPriority::Normal,
        interruptible: true,
        actions: vec![MotionAction {
            duration_ms: 400,
            intensity: 0.5,
            motion: MotionKind::SmallNod,
        }],
    }
}
fn progress(command: &MotionCommand, sequence: u64, frames: u64) -> MotionProgress {
    MotionProgress {
        identity: command.identity,
        receipt_sequence: sequence,
        accepted_actions: 1,
        started_actions: u32::from(frames > 0),
        submitted_actions: 0,
        submitted_frames: frames,
        partial_frame: None,
        last_partial_channels: vec![],
        last_uncertain_channels: vec![],
        backend: Some(MotionBackend {
            instance_id: command.identity.lease.device_boot_id,
            boundary: MotionSubmissionBoundary::Simulated,
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
    MotionDeliveryRepository,
    MotionDeliveryOrigin,
    MotionCommand,
    Uuid,
    Uuid,
) {
    let response = f.repo().prepare(f.response().await).await.unwrap();
    let origin = MotionDeliveryOrigin {
        response_id: response.response_id,
        output_key: "speech_start".into(),
    };
    let command = command(f.user);
    let repo = MotionDeliveryRepository::new(f.db.clone());
    let MotionDispatch::Fresh {
        delivery_id,
        dispatch_id,
    } = repo.claim(&origin, "test-device", &command).await.unwrap()
    else {
        panic!("first claim must be unique");
    };
    (repo, origin, command, delivery_id, dispatch_id)
}

#[tokio::test]
async fn fixed_source_notice_cannot_authorize_a_motion_command() {
    with_db(|f| async move {
        let response = f.repo().prepare(f.notice().await).await.unwrap();
        let origin = MotionDeliveryOrigin {
            response_id: response.response_id,
            output_key: "pre_speech".into(),
        };
        let error = MotionDeliveryRepository::new(f.db.clone())
            .claim(&origin, "test-device", &command(f.user))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("notice cannot authorize motion"));
        assert_eq!(f.count("motion_deliveries").await, 0);
    })
    .await;
}

#[tokio::test]
async fn concurrent_claims_grant_one_physical_permission_and_preserve_original_identity() {
    with_db(|f| async move {
        let response = f.repo().prepare(f.response().await).await.unwrap();
        let origin = MotionDeliveryOrigin {
            response_id: response.response_id,
            output_key: "pre_speech".into(),
        };
        let command = command(f.user);
        let repo = MotionDeliveryRepository::new(f.db.clone());
        let results = futures::future::join_all(
            (0..12).map(|_| repo.claim(&origin, "test-device", &command)),
        )
        .await;
        let mut fresh = 0;
        for result in results {
            match result.unwrap() {
                MotionDispatch::Fresh { .. } => fresh += 1,
                MotionDispatch::Existing(old) => assert_eq!(old.command, command),
            }
        }
        assert_eq!(fresh, 1);
        assert_eq!(f.count("motion_deliveries").await, 1);
        let mut restart = command.clone();
        restart.identity.lease = MotionControlLease {
            device_boot_id: Uuid::new_v4(),
            server_instance_id: Uuid::new_v4(),
            generation: 99,
        };
        restart.identity.sequence = 3;
        restart.source_command_id = "new server instance correlation".into();
        let MotionDispatch::Existing(old) =
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
async fn animation_ledger_lookup_preserves_owner_response_channel_and_exact_content() {
    with_db(|f| async move {
        let response = f.repo().prepare(f.response().await).await.unwrap();
        let origin = MotionDeliveryOrigin { response_id: response.response_id, output_key: "speech_start".into() };
        let mut command = command(f.user);
        let content = b"frames\n0 0 0\n";
        let asset = MotionAnimationAsset { file: "reviewed.anim".into(), version: 1,
            sha256: motion_animation_sha256(content), bytes: content.len() as u32 };
        command.actions[0].motion = MotionKind::Animation { asset: asset.clone(), fps: 30, loops: 1 };
        let repo = MotionDeliveryRepository::new(f.db.clone());
        assert!(matches!(repo.claim(&origin, "test-device", &command).await.unwrap(), MotionDispatch::Fresh { .. }));
        let original = repo.lookup_origin(&origin, "test-device", f.user).await.unwrap().unwrap();
        assert_eq!(original.command, command);

        let other = Uuid::new_v4();
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Other owner','not-a-login','user','active')",
            vec![other.into(), format!("{other}@motion.test").into()])).await.unwrap();
        assert!(repo.lookup_origin(&origin, "test-device", other).await.unwrap().is_none());
        assert!(repo.lookup_origin(&origin, "foreign-channel", f.user).await.is_err());
        assert!(repo.lookup_origin(&MotionDeliveryOrigin { response_id: Uuid::new_v4(), ..origin.clone() }, "test-device", f.user).await.unwrap().is_none());
        assert!(repo.lookup_origin(&MotionDeliveryOrigin { output_key: "post_speech".into(), ..origin.clone() }, "test-device", f.user).await.unwrap().is_none());

        for changed_asset in [
            MotionAnimationAsset { version: 2, ..asset.clone() },
            MotionAnimationAsset { sha256: motion_animation_sha256(b"frames\n1 0 0\n"), ..asset.clone() },
            MotionAnimationAsset { bytes: asset.bytes + 1, ..asset.clone() },
            MotionAnimationAsset { file: "other.anim".into(), ..asset.clone() },
        ] {
            let mut changed = command.clone();
            changed.actions[0].motion = MotionKind::Animation { asset: changed_asset, fps: 30, loops: 1 };
            assert!(repo.claim(&origin, "test-device", &changed).await.is_err());
        }
        assert_eq!(f.count("motion_deliveries").await, 1);
        assert_eq!(repo.lookup_origin(&origin, "test-device", f.user).await.unwrap().unwrap().command, command);
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE users SET status='disabled' WHERE id=$1", vec![f.user.into()])).await.unwrap();
        assert!(repo.lookup_origin(&origin, "test-device", f.user).await.is_err());
    }).await;
}

#[tokio::test]
async fn audit_failure_rolls_back_permission_before_external_output() {
    with_db(|f| async move {
        let response = f.repo().prepare(f.response().await).await.unwrap();
        let origin = MotionDeliveryOrigin { response_id: response.response_id, output_key: "pre_speech".into() };
        let command = command(f.user);
        let repo = MotionDeliveryRepository::new(f.db.clone());
        f.sql("CREATE FUNCTION deny_motion_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='motion_dispatch' THEN RAISE EXCEPTION 'injected motion audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER deny_motion BEFORE INSERT ON delivery_events FOR EACH ROW EXECUTE FUNCTION deny_motion_dispatch()").await;
        assert!(repo.claim(&origin,"test-device",&command).await.is_err());
        assert_eq!(f.count("motion_deliveries").await,0);
        f.sql("DROP TRIGGER deny_motion ON delivery_events").await;
        assert!(matches!(repo.claim(&origin,"test-device",&command).await.unwrap(),MotionDispatch::Fresh{..}));
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
            .lookup_origin(&origin, "test-device", command.owner_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(original.delivery_id, id);
        assert_eq!(original.command, command);
        assert!(
            repo.lookup_origin(&origin, "wrong-device", command.owner_id)
                .await
                .is_err()
        );
        let mut submitted = progress(&command, 1, 20);
        submitted.submitted_actions = 1;
        submitted.writer_quiescent = true;
        submitted.end = Some(MotionEnd::Submitted);
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
        f.sql("UPDATE motion_deliveries SET observation_deadline_at=clock_timestamp()-interval '1 second'").await;
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
        late.end = Some(MotionEnd::Cancelled {
            reason: "exact cancellation confirmed".into(),
        });
        repo.progress(id, permit, &late).await.unwrap();
        repo.progress(id, permit, &late).await.unwrap();
        assert_eq!(f.count("motion_delivery_late_checkpoints").await, 1);
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
        revived.end = Some(MotionEnd::Submitted);
        assert!(repo.progress(id, permit, &revived).await.is_err());
        assert!(
            f.repo()
                .events(f.user, row.response_id, 0, 64)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == "motion_late_checkpoint")
        );
    })
    .await;
}

#[tokio::test]
async fn wrong_owner_channel_scope_or_dispatch_cannot_touch_motion_delivery() {
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
        wrong.scope = MotionScope::Voice {
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
        forged.end = Some(MotionEnd::Cancelled {
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
