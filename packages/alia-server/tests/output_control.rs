//! Real PostgreSQL control authority and bounded evidence; no device RPC is faked
//! into a SQL fact. The one transport fixture drops an actual COMMIT response.
mod delivery_support;

use alia_core::{
    delivery::{SubmissionBackend, SubmissionBoundary},
    output_control::{
        BoundControlCommand, BoundControlReceipt, ControlMode, OutputBinding, OutputRegistration,
    },
    playback_control::{
        CommandOutcome, ControlAction, ControlCommand, ControlPhase, ControlReceipt,
        ControlSnapshot, HoldReason, ResumeAuthorization, ResumeKind,
    },
};
use alia_server::modules::delivery::{
    repository::{DeliveryRepository, control::*},
    types::*,
};
use delivery_support::*;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}
async fn registration(f: &Fixture) -> OutputRegistration {
    let mut response = f.response().await;
    let scope = identity().scope;
    response.content["voice_scope"] = json!(scope);
    let response = f.repo().prepare(response).await.unwrap();
    OutputRegistration {
        binding: OutputBinding {
            scope,
            response_id: response.response_id,
            context_id: response.context_id.unwrap(),
            server_instance_id: Uuid::new_v4(),
        },
        backend: SubmissionBackend {
            instance_id: Uuid::new_v4(),
            boundary: SubmissionBoundary::AlsaWriteAccepted,
            output_sample_rate_hz: 48000,
            output_channels: 2,
        },
        maximum_hold_ms: DEFAULT_CONTROL_HOLD_MS,
    }
}
fn pause(reg: &OutputRegistration, sequence: u64) -> BoundControlCommand {
    BoundControlCommand {
        binding: reg.binding,
        expected_backend: reg.backend.clone(),
        command: ControlCommand {
            scope: reg.binding.scope,
            hold_id: Uuid::new_v4(),
            command_id: Uuid::new_v4(),
            sequence,
            action: ControlAction::Pause {
                reason: HoldReason::AcousticCandidate,
                through_sequence: sequence,
                budget_ms: 60000,
            },
        },
    }
}
fn resume(pause: &BoundControlCommand, sequence: u64) -> BoundControlCommand {
    let mut next = pause.clone();
    next.command.command_id = Uuid::new_v4();
    next.command.sequence = sequence;
    next.command.action = ControlAction::Resume {
        authorization: ResumeAuthorization {
            kind: ResumeKind::FreshBackchannel,
            detection_id: Uuid::new_v4(),
            through_sequence: sequence,
        },
    };
    next
}
async fn admitted(f: &Fixture, command: &BoundControlCommand) -> ControlPermit {
    match f
        .repo()
        .admit_control(f.user, command.binding.server_instance_id, command)
        .await
        .unwrap()
    {
        ControlAdmission::Fresh(permit) => permit,
        other => panic!("expected fresh SQL admission, got {other:?}"),
    }
}
async fn held(f: &Fixture) -> (OutputRegistration, BoundControlCommand) {
    let reg = registration(f).await;
    f.repo()
        .register_control(f.user, reg.clone())
        .await
        .unwrap();
    let cmd = pause(&reg, 1);
    admitted(f, &cmd).await;
    (reg, cmd)
}
fn observed(
    command: &BoundControlCommand,
    observation: u64,
    watermark: u64,
    phase: ControlPhase,
) -> BoundControlReceipt {
    let is_held = matches!(phase, ControlPhase::Held | ControlPhase::Pausing);
    BoundControlReceipt {
        observation_sequence: observation,
        command: command.clone(),
        intent_digest: command.intent_digest(),
        backend: command.expected_backend.clone(),
        mode: ControlMode::SoftwareGate,
        native_writes_quiescent: phase != ControlPhase::Pausing,
        progress: vec![],
        receipt: ControlReceipt {
            command_id: command.command.command_id,
            outcome: CommandOutcome::Applied,
            snapshot: ControlSnapshot {
                scope: command.binding.scope,
                phase,
                hold_id: is_held.then_some(command.command.hold_id),
                reason: is_held.then_some(HoldReason::AcousticCandidate),
                sequence: watermark,
                remaining_ms: is_held.then_some(30000),
                retired: None,
            },
        },
    }
}
async fn wait_blocked_by(f: &Fixture, pid: i32) {
    tokio::time::timeout(Duration::from_secs(1),async {
        loop {
            let row=f.db.query_one(sql("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS blocked",vec![pid.into()])).await.unwrap().unwrap();
            if row.try_get::<bool>("","blocked").unwrap() {break;}
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("second real transaction never waited on the authority lock");
}

#[tokio::test]
async fn prepared_binding_is_unique_and_first_admission_is_the_only_send_permit() {
    with_db(|f| async move {
        let reg = registration(&f).await;
        let repo = f.repo();
        let registrations =
            futures::future::join_all((0..8).map(|_| repo.register_control(f.user, reg.clone())))
                .await;
        let deadline = registrations[0].as_ref().unwrap().deadline_at;
        assert!(
            registrations
                .iter()
                .all(|r| r.as_ref().unwrap().deadline_at == deadline)
        );
        assert_eq!(
            f.count("deliveries").await,
            0,
            "first-start registration needs no audio reservation"
        );
        let cmd = pause(&reg, 1);
        let results = futures::future::join_all(
            (0..8).map(|_| repo.admit_control(f.user, reg.binding.server_instance_id, &cmd)),
        )
        .await;
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Ok(ControlAdmission::Fresh(_))))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Ok(ControlAdmission::Existing(_))))
                .count(),
            7
        );
        drop(results); // a lost caller/ACK cannot recreate a permit.
        assert!(matches!(
            repo.admit_control(f.user, reg.binding.server_instance_id, &cmd)
                .await
                .unwrap(),
            ControlAdmission::Existing(_)
        ));
        let mut altered = cmd.clone();
        altered.command.action = ControlAction::Pause {
            reason: HoldReason::ExplicitWait,
            through_sequence: 1,
            budget_ms: 60000,
        };
        assert!(matches!(
            repo.admit_control(f.user, reg.binding.server_instance_id, &altered)
                .await,
            Err(ControlLedgerError::Conflict)
        ));
        let mut rebound = reg.clone();
        rebound.binding.server_instance_id = Uuid::new_v4();
        assert!(matches!(
            repo.register_control(f.user, rebound).await,
            Err(ControlLedgerError::Conflict)
        ));
        assert!(
            repo.admit_control(f.user, Uuid::new_v4(), &resume(&cmd, 2))
                .await
                .is_err()
        );
        assert!(
            repo.query_control(Uuid::new_v4(), &reg.binding)
                .await
                .is_err()
        );
        assert_eq!(f.count("output_control_scopes").await, 1);
        assert_eq!(f.count("output_control_commands").await, 1);
        assert_eq!(f.assistant_count().await, 0);
        assert_eq!(f.count("delivery_responses").await, 1);
    })
    .await;
}

#[tokio::test]
async fn original_response_scope_context_and_existing_audio_owner_are_required() {
    with_db(|f| async move {
        let reg = registration(&f).await;
        for mode in 0..3 {
            let mut wrong = reg.clone();
            match mode {
                0 => wrong.binding.scope.generation_epoch += 1,
                1 => wrong.binding.context_id = Uuid::new_v4(),
                _ => wrong.maximum_hold_ms = 120001,
            };
            assert!(f.repo().register_control(f.user, wrong).await.is_err());
        }
        let audio = NewDelivery {
            response_id: reg.binding.response_id,
            output_key: "audio:0".into(),
            content: json!({"text":"audio"}),
            identity: Some(alia_core::delivery::PlaybackIdentity {
                scope: reg.binding.scope,
                ..identity()
            }),
            owner_id: Uuid::new_v4(),
        };
        f.repo().prepare_delivery(f.user, audio).await.unwrap();
        assert!(f.repo().register_control(f.user, reg).await.is_err());
        let notice = f.repo().prepare(f.notice().await).await.unwrap();
        let mut notice_reg = registration(&f).await;
        notice_reg.binding.response_id = notice.response_id;
        notice_reg.binding.context_id = notice.context_id.unwrap();
        assert!(f.repo().register_control(f.user, notice_reg).await.is_err());
        assert_eq!(f.count("output_control_scopes").await, 0);
    })
    .await;
}

#[tokio::test]
async fn confirm_and_resume_inherit_original_hold_deadline_and_commands_are_bounded() {
    with_db(|f| async move {
        let (reg, pause) = held(&f).await;
        let original = f
            .repo()
            .query_control(f.user, &reg.binding)
            .await
            .unwrap()
            .commands[0]
            .deadline_at;
        let mut confirm = resume(&pause, 2);
        confirm.command.action = ControlAction::ConfirmExplicitWait {
            detection_id: Uuid::new_v4(),
            through_sequence: 2,
        };
        assert_eq!(admitted(&f, &confirm).await.deadline_at(), original);
        assert_eq!(
            admitted(&f, &resume(&pause, 3)).await.deadline_at(),
            original
        );
        for sequence in 4..=32 {
            admitted(&f, &self::pause(&reg, sequence)).await;
        }
        assert!(matches!(
            f.repo()
                .admit_control(
                    f.user,
                    reg.binding.server_instance_id,
                    &self::pause(&reg, 33)
                )
                .await,
            Err(ControlLedgerError::Capacity)
        ));
        assert!(matches!(
            f.repo()
                .admit_control(f.user, reg.binding.server_instance_id, &pause)
                .await
                .unwrap(),
            ControlAdmission::Existing(_)
        ));
        assert_eq!(f.count("output_control_commands").await, 32);
        let view = f.repo().query_control(f.user, &reg.binding).await.unwrap();
        assert_eq!(view.scope.last_command_sequence, 32);
        assert_eq!(view.commands.len(), 32);
        assert!(
            view.commands
                .iter()
                .all(|c| c.deadline_at <= view.scope.deadline_at)
        );
    })
    .await;
}

#[tokio::test]
async fn late_original_outcome_does_not_rewind_current_snapshot_or_grow_poll_audit() {
    with_db(|f| async move {
        let (reg, pause) = held(&f).await;
        let resume = resume(&pause, 2);
        admitted(&f, &resume).await;
        f.repo()
            .unknown_control(&pause, "RPC ACK lost")
            .await
            .unwrap();
        let running = observed(&resume, 20, 2, ControlPhase::Running);
        assert_eq!(
            f.repo().record_control(&running).await.unwrap().disposition,
            ControlRecordDisposition::Advanced
        );
        let late = observed(&pause, 10, 1, ControlPhase::Held);
        let result = f.repo().record_control(&late).await.unwrap();
        assert_eq!(result.disposition, ControlRecordDisposition::LateOutcome);
        assert_eq!(result.scope.latest_receipt.unwrap(), running);
        assert_eq!(result.command.first_receipt, Some(late.clone()));
        assert_eq!(
            result.command.unknown_reason.as_deref(),
            Some("RPC ACK lost")
        );
        assert_eq!(
            f.repo().record_control(&late).await.unwrap().disposition,
            ControlRecordDisposition::Duplicate
        );
        let mut conflict = late.clone();
        conflict.receipt.snapshot.remaining_ms = Some(29999);
        assert!(f.repo().record_control(&conflict).await.is_err());
        let mut reversed = late.clone();
        reversed.observation_sequence = 21;
        assert!(f.repo().record_control(&reversed).await.is_err());
        let audit = f.count("delivery_events").await;
        for observation in 21..=290 {
            let mut next = running.clone();
            next.observation_sequence = observation;
            f.repo().record_control(&next).await.unwrap();
        }
        assert_eq!(
            f.count("delivery_events").await,
            audit,
            "read-only polls must not append audit"
        );
        let mut evicted = running.clone();
        evicted.observation_sequence = 25;
        assert_eq!(
            f.repo().record_control(&evicted).await.unwrap().disposition,
            ControlRecordDisposition::Stale
        );
        let mut false_outcome = running.clone();
        false_outcome.observation_sequence = 291;
        false_outcome.receipt.outcome =
            CommandOutcome::Rejected(alia_core::playback_control::ControlRejection::Retired);
        assert!(f.repo().record_control(&false_outcome).await.is_err());
        f.repo()
            .retire_control(
                f.user,
                reg.binding.server_instance_id,
                &reg.binding,
                "local cancellation",
            )
            .await
            .unwrap();
        let mut late_running = running;
        late_running.observation_sequence = 292;
        assert_eq!(
            f.repo()
                .record_control(&late_running)
                .await
                .unwrap()
                .scope
                .status,
            OutputControlStatus::Retired
        );
        assert!(
            f.repo()
                .admit_control(
                    f.user,
                    reg.binding.server_instance_id,
                    &self::pause(&reg, 3)
                )
                .await
                .is_err()
        );
        assert_eq!(f.count("deliveries").await, 0);
        assert_eq!(f.assistant_count().await, 0);
    })
    .await;
}

#[tokio::test]
async fn resume_waits_for_committing_revocation_and_never_refreshes_original_context() {
    // Each mutation starts in an actual transaction and owns the precise row
    // before Resume starts. pg_blocking_pids proves the second transaction waits.
    for kind in [
        "privacy",
        "policy",
        "relationship",
        "revision",
        "private",
        "expiry",
        "cancel",
        "owner",
    ] {
        with_db(|f|async move {
            let root=Uuid::new_v4();
            f.db.execute(sql("INSERT INTO memory_items(id,tenant_id,user_id,kind,scope,memory_layer,node_type,content) VALUES($1,$2,$3,'semantic','user','core','fact','answer source')",vec![root.into(),Uuid::nil().into(),f.user.into()])).await.unwrap();
            let (reg,pause)=held(&f).await;
            let roots:Value=f.db.query_one(sql("SELECT dependencies FROM context_leases WHERE context_id=$1",vec![reg.binding.context_id.into()])).await.unwrap().unwrap().try_get("","dependencies").unwrap();
            assert!(roots.to_string().contains(&root.to_string()),"fixture must pin the actual Memory root");
            let tx=f.db.begin().await.unwrap();
            let pid:i32=tx.query_one(sql("SELECT pg_backend_pid() AS pid",vec![])).await.unwrap().unwrap().try_get("","pid").unwrap();
            let mutation=match kind {
                "privacy"=>format!("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id='{}'",f.user),
                "policy"=>"UPDATE context_policy SET version=version+1".into(),
                "relationship"=>format!("UPDATE relationship_authority SET relationship_epoch=relationship_epoch+1 WHERE owner_id='{}'",f.user),
                "revision"=>format!("UPDATE memory_items SET revision=revision+1 WHERE id='{root}'"),
                "private"=>format!("UPDATE memory_items SET sensitive=TRUE WHERE id='{root}'"),
                "expiry"=>format!("UPDATE memory_items SET expires_at=clock_timestamp()-interval '1 second' WHERE id='{root}'"),
                "cancel"=>format!("UPDATE delivery_responses SET cancel_requested=TRUE WHERE response_id='{}'",reg.binding.response_id),
                _=>format!("UPDATE users SET status='disabled' WHERE id='{}'",f.user),
            };
            tx.execute_unprepared(&mutation).await.unwrap();
            let command=resume(&pause,2);let task_repo=f.repo();let owner=f.user;let task_command=command.clone();
            let task=tokio::spawn(async move {task_repo.admit_control(owner,task_command.binding.server_instance_id,&task_command).await});
            wait_blocked_by(&f,pid).await;tx.commit().await.unwrap();
            assert!(task.await.unwrap().is_err(),"{kind} must revoke original Resume admission");
            assert_eq!(f.count("output_control_commands").await,1);
            // A revoked owner/source cannot suppress already-happened facts or cancellation.
            f.repo().record_control(&observed(&pause,1,1,ControlPhase::Held)).await.unwrap();
            f.repo().retire_control(f.user,reg.binding.server_instance_id,&reg.binding,"revoked original owner").await.unwrap();
        }).await;
    }
}

#[tokio::test]
async fn resume_authority_locks_are_held_until_the_intent_commit() {
    with_db(|f|async move {
        let (reg,pause)=held(&f).await;
        let key=56781023i64;
        f.sql(&format!("CREATE FUNCTION hold_control_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER hold_control_insert BEFORE INSERT ON output_control_commands FOR EACH ROW EXECUTE FUNCTION hold_control_insert();")).await;
        let barrier=f.db.begin().await.unwrap();barrier.execute(sql("SELECT pg_advisory_xact_lock($1)",vec![key.into()])).await.unwrap();
        let barrier_pid:i32=barrier.query_one(sql("SELECT pg_backend_pid() AS pid",vec![])).await.unwrap().unwrap().try_get("","pid").unwrap();
        let repo=f.repo();let owner=f.user;let command=resume(&pause,2);let task=tokio::spawn(async move{repo.admit_control(owner,command.binding.server_instance_id,&command).await});
        wait_blocked_by(&f,barrier_pid).await;
        let resume_pid:i32=f.db.query_one(sql("SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) LIMIT 1",vec![barrier_pid.into()])).await.unwrap().unwrap().try_get("","pid").unwrap();
        let service=f.context();let policy=service.policy_store();let current=policy.current().await.unwrap();
        let revoke=tokio::spawn(async move{policy.replace(current.version,current.policy).await});
        wait_blocked_by(&f,resume_pid).await;
        barrier.commit().await.unwrap();
        assert!(matches!(task.await.unwrap().unwrap(),ControlAdmission::Fresh(_)));
        revoke.await.unwrap().unwrap();
        assert!(f.repo().admit_control(f.user,reg.binding.server_instance_id,&self::pause(&reg,3)).await.is_err());
    }).await;
}

#[tokio::test]
async fn expired_recovery_is_owner_independent_but_cannot_steal_a_live_instance() {
    with_db(|f|async move {
        let (expired,pause)=held(&f).await;let live=registration(&f).await;f.repo().register_control(f.user,live.clone()).await.unwrap();
        f.sql(&format!("UPDATE output_control_scopes SET deadline_at=clock_timestamp()-interval '1 second' WHERE response_id='{}'",expired.binding.response_id)).await;
        let restarted=DeliveryRepository::new(f.db.clone());assert_eq!(restarted.recover_expired_controls(256).await.unwrap(),1);
        let old=restarted.query_control(f.user,&expired.binding).await.unwrap();assert_eq!(old.scope.status,OutputControlStatus::Retired);assert_eq!(old.commands[0].status,ControlCommandStatus::Unknown);
        assert_eq!(restarted.query_control(f.user,&live.binding).await.unwrap().scope.status,OutputControlStatus::Active);
        assert!(restarted.admit_control(f.user,Uuid::new_v4(),&resume(&pause,2)).await.is_err());
        assert!(matches!(restarted.admit_control(f.user,expired.binding.server_instance_id,&pause).await.unwrap(),ControlAdmission::Existing(_)));
        assert_eq!(restarted.register_control(f.user,expired.clone()).await.unwrap().deadline_at,old.scope.deadline_at);
        assert_eq!(restarted.record_control(&observed(&pause,1,1,ControlPhase::Held)).await.unwrap().scope.status,OutputControlStatus::Retired);
        assert_eq!(restarted.recover_expired_controls(256).await.unwrap(),0);
    }).await;
}

#[tokio::test]
async fn control_intent_and_first_outcome_roll_back_with_their_audit() {
    with_db(|f|async move {
        let reg=registration(&f).await;f.repo().register_control(f.user,reg.clone()).await.unwrap();let command=pause(&reg,1);
        f.sql("CREATE FUNCTION reject_control_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind LIKE 'control_%' THEN RAISE EXCEPTION 'injected control audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_control_audit BEFORE INSERT ON delivery_events FOR EACH ROW EXECUTE FUNCTION reject_control_audit();").await;
        assert!(f.repo().admit_control(f.user,reg.binding.server_instance_id,&command).await.is_err());assert_eq!(f.count("output_control_commands").await,0);
        assert_eq!(f.repo().query_control(f.user,&reg.binding).await.unwrap().scope.last_command_sequence,0);
        f.sql("ALTER TABLE delivery_events DISABLE TRIGGER reject_control_audit").await;admitted(&f,&command).await;f.repo().unknown_control(&command,"transport lost").await.unwrap();
        f.sql("ALTER TABLE delivery_events ENABLE TRIGGER reject_control_audit").await;
        assert!(f.repo().record_control(&observed(&command,1,1,ControlPhase::Held)).await.is_err());
        let view=f.repo().query_control(f.user,&reg.binding).await.unwrap();assert_eq!(view.commands[0].status,ControlCommandStatus::Unknown);assert!(view.commands[0].first_receipt.is_none());assert!(view.scope.latest_receipt.is_none());
    }).await;
}

struct CommitAckProxy {
    db: DatabaseConnection,
    commits: Arc<AtomicUsize>,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl CommitAckProxy {
    async fn new(f: &Fixture) -> Self {
        use sea_orm::sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::{TcpListener, TcpStream},
        };
        let options: PgConnectOptions = std::env::var("ALIA_TEST_DATABASE_URL")
            .unwrap()
            .parse()
            .unwrap();
        let upstream = (options.get_host().to_owned(), options.get_port());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let commits = Arc::new(AtomicUsize::new(0));
        let seen = commits.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {biased;
                    _=stopped.cancelled()=>break,
                    Some(_)=connections.join_next(),if !connections.is_empty()=>{},
                    accepted=listener.accept()=>{
                        let(client,_)=accepted.unwrap();let upstream=upstream.clone();let seen=seen.clone();
                        connections.spawn(async move {
                            let server=TcpStream::connect(upstream).await.unwrap();client.set_nodelay(true).unwrap();server.set_nodelay(true).unwrap();
                            let(mut cr,mut cw)=client.into_split();let(mut sr,mut sw)=server.into_split();
                            let upload=tokio::io::copy(&mut cr,&mut sw);
                            let download=async {loop {
                                let mut header=[0u8;5];sr.read_exact(&mut header).await?;let length=u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
                                if !(4..=1024*1024).contains(&length){return Err(std::io::Error::other("invalid PG frame"));}
                                let mut body=vec![0;length-4];sr.read_exact(&mut body).await?;
                                if header[0]==b'C' && body==b"COMMIT\0" {seen.fetch_add(1,Ordering::SeqCst);return Ok::<_,std::io::Error>(());}
                                let mut frame=Vec::with_capacity(header.len()+body.len());frame.extend_from_slice(&header);frame.extend_from_slice(&body);cw.write_all(&frame).await?;
                            }};
                            tokio::select!{_=upload=>{},_=download=>{}}
                        });
                    }
                }
            }
            connections.shutdown().await;
        });
        let schema: String =
            f.db.query_one(sql("SELECT current_schema() AS schema", vec![]))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "schema")
                .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(2))
            .connect_with(
                options
                    .host("127.0.0.1")
                    .port(port)
                    .ssl_mode(PgSslMode::Disable)
                    .options([("search_path", format!("{schema},public"))]),
            )
            .await
            .unwrap();
        Self {
            db: sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
            commits,
            stop,
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.db.clone().close().await.unwrap();
        self.stop.cancel();
        self.task.take().unwrap().await.unwrap();
    }
}
impl Drop for CommitAckProxy {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
#[tokio::test]
async fn actual_lost_commit_ack_returns_unknown_and_lookup_cannot_authorize_replay() {
    with_db(|f|async move {
        let reg=registration(&f).await;f.repo().register_control(f.user,reg.clone()).await.unwrap();let command=pause(&reg,1);
        let proxy=CommitAckProxy::new(&f).await;let repo=DeliveryRepository::new(proxy.db.clone());
        let result=repo.admit_control(f.user,reg.binding.server_instance_id,&command).await;
        let dropped=proxy.commits.load(Ordering::SeqCst);drop(repo);proxy.close().await;
        assert_eq!(dropped,1);assert!(matches!(result,Err(ControlLedgerError::CommitUnknown {command_id:Some(id),..}) if id==command.command.command_id));
        let view=f.repo().query_control(f.user,&reg.binding).await.unwrap();assert_eq!(view.commands.len(),1);assert_eq!(view.commands[0].status,ControlCommandStatus::Requested);
        assert!(matches!(f.repo().admit_control(f.user,reg.binding.server_instance_id,&command).await.unwrap(),ControlAdmission::Existing(_)));
    }).await;
}
