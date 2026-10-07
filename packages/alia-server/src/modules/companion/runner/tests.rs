//! Real durable admissions with a deliberately fake voice-actor mailbox.
//! Dropping a Reserve reply models unknown acknowledgement, not actual playback.
use super::*;
use crate::{
    modules::{
        companion::{
            live::{
                AdmissionState,
                tests::{Fixture, with_fixture},
            },
            types::{CompanionMode, CompanionPolicy},
        },
        delivery::repository::DeliveryRepository,
        interaction::initiative::{Request as VoiceRequest, VoiceOpportunity},
        platform::lifecycle::ModuleLifecycle,
    },
    providers::{
        conversation::database::DatabaseConversationBackend,
        tts::{
            NoopTts,
            audio_budget::{EncodedAudio, TtsAudioStream},
        },
    },
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountedNoopTts(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl ModuleLifecycle for CountedNoopTts {
    fn name(&self) -> &'static str {
        "companion-unreached-tts"
    }
}
#[async_trait::async_trait]
impl TtsAdapter for CountedNoopTts {
    async fn synthesize_request(
        &self,
        request: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<EncodedAudio> {
        self.0.fetch_add(1, Ordering::SeqCst);
        NoopTts.synthesize_request(request, budget).await
    }
    async fn synthesize_stream_request(
        &self,
        request: &TtsRequest,
        budget: ResponseAudioBudget,
    ) -> anyhow::Result<TtsAudioStream> {
        self.0.fetch_add(1, Ordering::SeqCst);
        NoopTts.synthesize_stream_request(request, budget).await
    }
}

fn sql(query: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

fn runner(
    f: &Fixture,
    live: Arc<LiveRepository>,
    voice: VoiceInitiativeHandle,
    stop: CancellationToken,
) -> (LiveRunner, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let outputs = LiveOutputs {
        context: Arc::new(f.context()),
        // No output work is admitted in these failure paths, so no supervisor
        // needs starting. The real SQL repository remains installed.
        delivery: DeliveryService::new(DeliveryRepository::new(f.db.clone()), &stop),
        conversation: Arc::new(DatabaseConversationBackend::new()),
        tts: Arc::new(CountedNoopTts(calls.clone())),
    };
    (LiveRunner::new(live, voice, outputs, stop), calls)
}

async fn answer_snapshot(
    receiver: &mut tokio::sync::mpsc::Receiver<VoiceRequest>,
    opportunity: &VoiceOpportunity,
) {
    match receiver.recv().await.expect("original snapshot request") {
        VoiceRequest::Snapshot { owner, reply } => {
            assert_eq!(owner, opportunity.owner);
            reply.send(Ok(opportunity.clone())).unwrap();
        }
        VoiceRequest::Reserve { .. } => panic!("reserve before the only original snapshot"),
    }
}

#[derive(Clone, Copy)]
enum ReserveFault {
    Rejected,
    ReplyLost,
    Shutdown,
}

async fn reserve_fault(fault: ReserveFault) {
    with_fixture(|f| async move {
        f.enable_live(CompanionPolicy { mode:CompanionMode::Enabled, ..Default::default() }).await;
        let voice = VoiceInitiativeHandle::default();
        let live = Arc::new(f.live(voice.instance_id()));
        let pending = f.seed_pending(&live).await;
        let opportunity = f.opportunity(&pending).await;
        // Save the exact immutable intent only for a post-failure SQL retry.
        // No permit can be constructed from this intent or from a receipt.
        let original_intent = live.prepare_intent(pending.candidate_id, opportunity.clone()).await.unwrap();
        let (_registration, mut mailbox) = voice.register(pending.channel_id.clone(), opportunity.scope.session_id);
        let stop = CancellationToken::new();
        let (runner, tts_calls) = runner(&f, live.clone(), voice, stop.clone());
        let actor = async {
            answer_snapshot(&mut mailbox, &opportunity).await;
            let VoiceRequest::Reserve { request, reply } = mailbox.recv().await.expect("fresh durable reserve request") else {
                panic!("runner took a replacement snapshot");
            };
            assert_eq!(request.permit.candidate_id(), pending.candidate_id);
            assert_eq!(request.permit.admission_id(), original_intent.admission_id());
            assert_eq!(request.permit.opportunity(), &opportunity);
            assert_eq!(f.count("companion_admissions").await, 1, "reserve precedes acknowledged durable admission");
            assert_eq!(f.count("delivery_responses").await, 1);
            let original_cancel = request.cancel.clone();
            match fault {
                ReserveFault::Rejected => { reply.send(Err(anyhow::anyhow!("fixture rejects before creating a producer"))).unwrap(); }
                ReserveFault::ReplyLost => { drop(reply); }
                ReserveFault::Shutdown => {
                    stop.cancel();
                    tokio::time::timeout(Duration::from_secs(1), original_cancel.cancelled()).await.expect("runner shutdown did not cancel original reserve");
                    drop(reply);
                }
            }
            tokio::time::timeout(Duration::from_secs(1), original_cancel.cancelled()).await.expect("lost or rejected reserve kept original task live");
            // This fake actor never creates an OutputProducer or emits media.
            drop(request);
        };
        let (outcome, ()) = tokio::join!(runner.run(pending.clone()), actor);
        assert!(matches!(outcome, RunOutcome::Failed("original_output")));
        assert!(matches!(mailbox.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)), "runner requested another snapshot/reserve");
        assert_eq!(tts_calls.load(Ordering::SeqCst), 0);
        for table in ["deliveries", "output_control_scopes", "execution_effects"] { assert_eq!(f.count(table).await, 0, "{table}"); }
        assert_eq!(f.count("chat_messages").await, 1, "no assistant history was generated");
        assert_eq!(f.count("companion_admissions").await, 1);
        assert_eq!(f.count("delivery_responses").await, 1);
        let row = f.db.query_one(sql("SELECT a.state,a.retirement_reason,a.private_grant,a.grant_digest,r.cancel_requested FROM companion_admissions a JOIN delivery_responses r ON r.context_id=a.context_id WHERE a.admission_id=$1", vec![original_intent.admission_id().into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "state").unwrap(), "retired");
        assert_eq!(row.try_get::<String>("", "retirement_reason").unwrap(), if matches!(fault, ReserveFault::Shutdown) { "shutdown" } else { "attempt_failed" });
        assert!(row.try_get::<bool>("", "cancel_requested").unwrap());
        assert!(row.try_get::<Option<serde_json::Value>>("", "private_grant").unwrap().is_none());
        assert!(row.try_get::<Option<String>>("", "grant_digest").unwrap().is_none());
        match live.admit(&original_intent).await.unwrap() {
            AdmissionResult::Existing(receipt) => {
                assert_eq!(receipt.admission_id, original_intent.admission_id());
                assert_eq!(receipt.state, AdmissionState::Retired);
                let value = serde_json::to_value(receipt).unwrap();
                assert_eq!(value.as_object().unwrap().len(), 6, "Existing must remain an opaque receipt");
                assert!(value.get("private_grant").is_none() && value.get("context").is_none() && value.get("response").is_none());
            }
            other => panic!("failed original reserve recovered new permission: {other:?}"),
        }
        assert_eq!(f.count("companion_admissions").await, 1, "retry refunded or recharged original attempt");
    }).await;
}

#[tokio::test]
async fn rejected_reserve_cancels_original_identity_without_second_permit_or_tts() {
    reserve_fault(ReserveFault::Rejected).await;
}

#[tokio::test]
async fn lost_reserve_reply_cancels_original_identity_without_second_permit_or_tts() {
    reserve_fault(ReserveFault::ReplyLost).await;
}

#[tokio::test]
async fn shutdown_during_original_reserve_joins_failure_and_preserves_charge() {
    reserve_fault(ReserveFault::Shutdown).await;
}

#[tokio::test]
async fn existing_acknowledged_transaction_returns_receipt_without_reserve_or_tts() {
    with_fixture(|f| async move {
        f.enable_live(CompanionPolicy { mode:CompanionMode::Enabled, ..Default::default() }).await;
        let voice = VoiceInitiativeHandle::default();
        let live = Arc::new(f.live(voice.instance_id()));
        let pending = f.seed_pending(&live).await;
        let opportunity = f.opportunity(&pending).await;
        let intent = live.prepare_intent(pending.candidate_id, opportunity.clone()).await.unwrap();
        // Stage the same production grant/Context/Response transaction without
        // handing out a permit. Its commit is the other contender's outcome.
        let staged = live.stage_for_test(&intent).await.unwrap();
        let blocker:i32 = staged.query_one(sql("SELECT pg_backend_pid() AS pid", vec![])).await.unwrap().unwrap().try_get("", "pid").unwrap();
        let (_registration, mut mailbox) = voice.register(pending.channel_id.clone(), opportunity.scope.session_id);
        let (runner, tts_calls) = runner(&f, live.clone(), voice, CancellationToken::new());
        let actor_and_commit = async {
            answer_snapshot(&mut mailbox, &opportunity).await;
            // Observe the actual SQL wait; a yield alone would not prove that
            // the runner froze its intent before this transaction committed.
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let blocked:bool = f.db.query_one(sql("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS blocked", vec![blocker.into()])).await.unwrap().unwrap().try_get("", "blocked").unwrap();
                    if blocked { break; }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }).await.expect("runner did not reach original admission transaction barrier");
            staged.commit().await.unwrap();
        };
        let (outcome, ()) = tokio::join!(runner.run(pending), actor_and_commit);
        assert!(matches!(outcome, RunOutcome::Existing));
        assert!(matches!(mailbox.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)), "Existing triggered external reserve");
        assert_eq!(tts_calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.count("companion_admissions").await, 1);
        assert_eq!(f.count("delivery_responses").await, 1);
        assert_eq!(f.count("chat_messages").await, 1);
        assert_eq!(f.count("deliveries").await, 0);
        assert_eq!(f.count("output_control_scopes").await, 0);
        assert!(matches!(live.admit(&intent).await.unwrap(), AdmissionResult::Existing(_)));
    }).await;
}
