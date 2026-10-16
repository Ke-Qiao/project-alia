//! Real PostgreSQL history checks reuse the original Companion admission fixture.
use super::*;
use crate::modules::{
    companion::{
        live::{
            tests::{Fixture, with_fixture},
            types::{InvitationContent, LiveAdmissionPermit, RetirementReason},
        },
        types::{CompanionMode, CompanionPolicy},
    },
    delivery::{
        repository::DeliveryRepository,
        types::{NewResponse, ResponseKey, ResponseRecord},
    },
    memory::{MemoryNode, MemoryService, MemoryWriteOrigin, source::capture_source_epochs},
};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, TransactionTrait};

#[tokio::test]
async fn committed_live_invitation_is_read_only_history_for_a_new_generation_request() {
    with_fixture(|f| async move {
        f.enable_live(CompanionPolicy {
            mode: CompanionMode::Enabled,
            candidate_ttl_seconds: 60,
            ..Default::default()
        })
        .await;
        let live = f.live(Uuid::new_v4());
        let (_, invitation_binding, response) = f.fresh(&live).await;
        let invitation: InvitationContent =
            serde_json::from_value(response.content.clone()).unwrap();
        let assistant = Uuid::new_v4();
        let tx = f.db.begin().await.unwrap();
        tx.execute(stmt(
            "INSERT INTO chat_messages(id,session_id,user_id,role,content) VALUES($1,$2,$3,'assistant',$4)",
            vec![
                assistant.into(),
                f.session.into(),
                f.owner.to_string().into(),
                invitation.text.clone().into(),
            ],
        ))
        .await
        .unwrap();
        DeliveryRepository::mark_generated_in_transaction(
            &tx,
            f.owner,
            response.response_id,
            assistant,
            json!({"text": invitation.text}),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let epochs = capture_source_epochs(&f.db, f.owner).await.unwrap();
        let source_event_id = Uuid::new_v4();
        let text = "Yes, I would like to keep talking.";
        let row = f.db.query_one(stmt(
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at",
            vec![
                source_event_id.into(),
                f.session.into(),
                f.owner.to_string().into(),
                text.into(),
                epochs.privacy_epoch.into(),
                epochs.relationship_epoch.into(),
            ],
        )).await.unwrap().unwrap();
        let source = ContextSource {
            owner_id: f.owner,
            request_id: Uuid::new_v4(),
            channel_id: response.channel_id,
            conversation_id: Some(f.session),
            source_event_id: Some(source_event_id),
            source_received_at: row.try_get("", "source_received_at").unwrap(),
            source_privacy_epoch: epochs.privacy_epoch,
        };
        let mut before = Vec::new();
        for table in [
            "companion_admissions",
            "companion_source_claims",
            "delivery_responses",
            "deliveries",
            "execution_effects",
            "memory_items",
            "appraisal_records",
        ] {
            before.push((table, f.count(table).await));
        }

        let generation = f.context().materialize(source.clone(), text).await.unwrap();

        assert_eq!(generation.source(), &source);
        assert_ne!(generation.lease().id(), invitation_binding.context_id);
        assert_ne!(source.request_id, invitation_binding.request_id);
        for (table, count) in before {
            assert_eq!(f.count(table).await, count, "history replay mutated {table}");
        }
        assert!(
            generation.history().iter().any(|message| {
                message.id() == assistant
                    && message.message().role == "assistant"
                    && message.message().content == invitation.text
            }),
            "committed same-session invitation was omitted from ordinary Generation history"
        );
        assert!(has_marker(generation.lease().dependencies(), assistant));
        assert!(!generation.lease().dependencies().iter().any(|root| matches!(root, ContextEvidence::CompanionAdmission { .. })));
        f.context().validate(&generation.lease().binding(), ContextUse::ResponseCommit).await.unwrap();
    })
    .await;
}

async fn commit(f: &Fixture, response: &ResponseRecord, text: &str) -> Uuid {
    let assistant = Uuid::new_v4();
    let tx = f.db.begin().await.unwrap();
    tx.execute(stmt(
        "INSERT INTO chat_messages(id,session_id,user_id,role,content) VALUES($1,$2,$3,'assistant',$4)",
        vec![assistant.into(), f.session.into(), f.owner.to_string().into(), text.into()],
    )).await.unwrap();
    DeliveryRepository::mark_generated_in_transaction(
        &tx,
        f.owner,
        response.response_id,
        assistant,
        json!({"text": text}),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assistant
}

async fn committed_invitation(
    f: &Fixture,
    ttl: u32,
) -> (LiveAdmissionPermit, ContextBinding, ResponseRecord, Uuid) {
    f.enable_live(CompanionPolicy {
        mode: CompanionMode::Enabled,
        candidate_ttl_seconds: ttl,
        ..Default::default()
    })
    .await;
    let (permit, binding, response) = f.fresh(&f.live(Uuid::new_v4())).await;
    let content: InvitationContent = serde_json::from_value(response.content.clone()).unwrap();
    let assistant = commit(f, &response, &content.text).await;
    (permit, binding, response, assistant)
}

async fn generation_for(
    f: &Fixture,
    owner: Uuid,
    session: Uuid,
    channel: &str,
    text: &str,
) -> ContextResult<GenerationContext> {
    let epochs = capture_source_epochs(&f.db, owner).await.unwrap();
    let source_event_id = Uuid::new_v4();
    let row = f.db.query_one(stmt(
        "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at",
        vec![source_event_id.into(), session.into(), owner.to_string().into(), text.into(), epochs.privacy_epoch.into(), epochs.relationship_epoch.into()],
    )).await.unwrap().unwrap();
    f.context()
        .materialize(
            ContextSource {
                owner_id: owner,
                request_id: Uuid::new_v4(),
                channel_id: channel.into(),
                conversation_id: Some(session),
                source_event_id: Some(source_event_id),
                source_received_at: row.try_get("", "source_received_at").unwrap(),
                source_privacy_epoch: epochs.privacy_epoch,
            },
            text,
        )
        .await
}

async fn generation(f: &Fixture, text: &str) -> GenerationContext {
    generation_for(f, f.owner, f.session, "companion-device", text)
        .await
        .unwrap()
}

fn has_history(context: &GenerationContext, assistant: Uuid) -> bool {
    context
        .history()
        .iter()
        .any(|message| message.id() == assistant)
}

fn has_marker(roots: &[ContextEvidence], assistant: Uuid) -> bool {
    roots.iter().any(
        |root| matches!(root, ContextEvidence::CompanionHistory { id, .. } if *id == assistant),
    )
}

async fn commit_generation(f: &Fixture, context: &GenerationContext, text: &str) -> (Uuid, Uuid) {
    let source = context.source();
    let response = DeliveryRepository::new(f.db.clone())
        .prepare(NewResponse {
            key: ResponseKey {
                user_id: f.owner,
                request_id: source.request_id,
                output_key: "answer".into(),
            },
            context_id: context.lease().id(),
            conversation_id: source.conversation_id,
            source_event_id: source.source_event_id,
            source_received_at: source.source_received_at,
            source_privacy_epoch: Some(source.source_privacy_epoch),
            channel_id: source.channel_id.clone(),
            kind: "conversation".into(),
            content: json!({"text": text}),
        })
        .await
        .unwrap();
    (response.response_id, commit(f, &response, text).await)
}

async fn memory(f: &Fixture) -> MemoryService {
    let memory = MemoryService::new();
    memory
        .attach_runtime_for_test(f.db.clone(), None, Default::default())
        .await
        .unwrap();
    memory
}

async fn assert_revoked(f: &Fixture, context: &GenerationContext) {
    for usage in [ContextUse::ResponseCommit, ContextUse::OutputAdmission] {
        assert!(
            f.context()
                .validate(&context.lease().binding(), usage)
                .await
                .is_err(),
            "accepted {usage:?}"
        );
    }
}

#[tokio::test]
async fn invitation_history_requires_committed_response_and_same_owner_session_channel() {
    with_fixture(|f| async move {
        f.enable_live(CompanionPolicy { mode: CompanionMode::Enabled, ..Default::default() }).await;
        let (_, _, response) = f.fresh(&f.live(Uuid::new_v4())).await;
        let invitation: InvitationContent = serde_json::from_value(response.content.clone()).unwrap();
        let pending_assistant = Uuid::new_v4();
        // Deliberately persisted incomplete history must not become a committed fact.
        f.db.execute(stmt("INSERT INTO chat_messages(id,session_id,user_id,role,content) VALUES($1,$2,$3,'assistant',$4)", vec![pending_assistant.into(),f.session.into(),f.owner.to_string().into(),invitation.text.clone().into()])).await.unwrap();
        f.db.execute(stmt("UPDATE delivery_responses SET assistant_message_id=$2 WHERE response_id=$1", vec![response.response_id.into(),pending_assistant.into()])).await.unwrap();
        assert!(!has_history(&generation(&f, "Before commit").await, pending_assistant));
        let assistant = commit(&f, &response, &invitation.text).await;
        assert!(has_history(&generation(&f, "After commit").await, assistant));

        let other_session = Uuid::new_v4();
        f.db.execute(stmt("INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'companion-device',$2)", vec![other_session.into(),f.owner.to_string().into()])).await.unwrap();
        let other = generation_for(&f, f.owner, other_session, "companion-device", "Other session").await.unwrap();
        assert!(!has_history(&other, assistant));
        let other_owner = Uuid::new_v4();
        let owned_session = Uuid::new_v4();
        f.add_owner(other_owner, owned_session).await;
        let other = generation_for(&f, other_owner, owned_session, "companion-device", "Other owner").await.unwrap();
        assert!(!has_history(&other, assistant));
        assert!(generation_for(&f, f.owner, f.session, "different-channel", "Wrong channel").await.is_err());
    }).await;
}

async fn wait_for_database_deadline(f: &Fixture, deadline: DateTime<Utc>) {
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        let mut cadence = tokio::time::interval(std::time::Duration::from_millis(10));
        loop {
            cadence.tick().await;
            let reached: bool =
                f.db.query_one(stmt(
                    "SELECT clock_timestamp()>=$1 AS reached",
                    vec![deadline.into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "reached")
                .unwrap();
            if reached {
                break;
            }
        }
    })
    .await
    .expect("database clock did not reach original invitation deadline");
}

#[tokio::test]
async fn expired_retired_invitation_survives_context_reconstruction_and_shadow_as_history_only() {
    with_fixture(|f| async move {
        let (permit, binding, _, assistant) = committed_invitation(&f, 4).await;
        wait_for_database_deadline(&f, permit.deadline()).await;
        for usage in [ContextUse::ResponseCommit, ContextUse::OutputAdmission] {
            assert!(f.context().validate(&binding, usage).await.is_err());
        }
        let expired = generation(&f, "After output expiry").await;
        assert!(has_history(&expired, assistant));
        let source_expiry: DateTime<Utc> = f.db.query_one(stmt(
            "SELECT MIN(a.valid_until) AS expiry FROM appraisal_records a JOIN context_leases c ON c.context_id=$1 WHERE a.owner_id=$2 AND a.appraisal_id IN (SELECT (d->>'id')::uuid FROM jsonb_array_elements(c.dependencies) d WHERE d->>'kind'='appraisal')",
            vec![binding.context_id.into(), f.owner.into()],
        )).await.unwrap().unwrap().try_get("", "expiry").unwrap();
        assert!(source_expiry > permit.deadline());
        assert_eq!(expired.lease().valid_until(), Some(source_expiry));
        f.context().validate(&expired.lease().binding(), ContextUse::ResponseCommit).await.unwrap();

        let live = f.live(permit.server_instance_id());
        live.retire_candidate(permit.candidate_id(), RetirementReason::InstanceEnded).await.unwrap();
        let row = f.db.query_one(stmt("SELECT state,private_grant,grant_digest FROM companion_admissions WHERE admission_id=$1", vec![permit.admission_id().into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "state").unwrap(), "retired");
        assert!(row.try_get::<Option<serde_json::Value>>("", "private_grant").unwrap().is_none());
        assert!(row.try_get::<Option<String>>("", "grant_digest").unwrap().is_none());
        let current = f.repo().policy(f.owner).await.unwrap();
        live.replace_policy(f.owner, current.revision, CompanionPolicy { mode: CompanionMode::Shadow, ..current.policy }).await.unwrap();
        // generation() constructs a new ContextService over persisted SQL each time.
        let restarted = generation(&f, "After owner replacement and shadow").await;
        assert!(has_history(&restarted, assistant));
        assert!(has_marker(restarted.lease().dependencies(), assistant));
        for usage in [ContextUse::ResponseCommit, ContextUse::OutputAdmission] {
            f.context().validate(&restarted.lease().binding(), usage).await.unwrap();
            assert!(f.context().validate(&binding, usage).await.is_err());
        }
        assert_eq!(f.count("companion_admissions").await, 1);
        assert_eq!(f.count("delivery_responses").await, 1);
        assert_eq!(f.count("deliveries").await, 0);
    }).await;
}

#[tokio::test]
async fn invitation_history_rechecks_sources_text_policy_and_privacy_before_late_commit() {
    for cause in [
        "source_deleted",
        "source_changed",
        "assistant",
        "final_content",
        "response_channel",
        "policy",
        "privacy",
        "relationship",
    ] {
        with_fixture(|f| async move {
            let (_, _, response, assistant) = committed_invitation(&f, 60).await;
            let old = generation(&f, "Read invitation before revocation").await;
            assert!(has_history(&old, assistant));
            match cause {
                "source_deleted" => {
                    // The appraisal owns a non-cascading FK to its user fact.
                    // Remove it first: this is combined appraisal/source revocation.
                    f.db.execute(stmt("DELETE FROM appraisal_records WHERE source_event_id=$1", vec![response.source_event_id.unwrap().into()])).await.unwrap();
                    f.db.execute(stmt("DELETE FROM chat_messages WHERE id=$1", vec![response.source_event_id.unwrap().into()])).await.unwrap();
                }
                "source_changed" => { f.db.execute(stmt("UPDATE chat_messages SET content='changed source fact' WHERE id=$1", vec![response.source_event_id.unwrap().into()])).await.unwrap(); }
                "assistant" => { f.db.execute(stmt("UPDATE chat_messages SET content='changed invitation' WHERE id=$1", vec![assistant.into()])).await.unwrap(); }
                "final_content" => { f.db.execute(stmt("UPDATE delivery_responses SET final_content=$2 WHERE response_id=$1", vec![response.response_id.into(),json!({"text":"changed invitation"}).into()])).await.unwrap(); }
                "response_channel" => { f.db.execute(stmt("UPDATE delivery_responses SET channel_id='other-channel' WHERE response_id=$1", vec![response.response_id.into()])).await.unwrap(); }
                "policy" => {
                    let store = f.context().policy_store();
                    let mut current = store.current().await.unwrap();
                    current.policy.max_memory_chars += 1;
                    store.replace(current.version, current.policy).await.unwrap();
                }
                "privacy" => { memory(&f).await.clear(f.owner).await.unwrap(); }
                "relationship" => { f.context().clear_relationship(f.owner, Uuid::new_v4(), 0).await.unwrap(); }
                _ => unreachable!(),
            }
            assert_revoked(&f, &old).await;
            let current = generation(&f, "Fresh source after revocation").await;
            assert!(!has_history(&current, assistant), "reintroduced {cause} history");
        }).await;
    }
}

#[tokio::test]
async fn derived_history_cannot_omit_or_forge_companion_marker_or_inherit_live_admission() {
    with_fixture(|f| async move {
        let (_, binding, _, assistant) = committed_invitation(&f, 60).await;
        let origin = generation(&f, "Read invitation").await;
        let (_, derived_assistant) =
            commit_generation(&f, &origin, "A later ordinary answer").await;
        let derived = generation(&f, "Read later ordinary answer").await;
        assert!(has_history(&derived, derived_assistant));
        assert!(has_marker(derived.lease().dependencies(), assistant));
        let tx = f.db.begin().await.unwrap();
        validate_evidence(&tx, derived.lease()).await.unwrap();
        let mut missing = derived.lease().clone();
        missing
            .dependencies
            .retain(|root| !matches!(root, ContextEvidence::CompanionHistory { .. }));
        assert!(validate_evidence(&tx, &missing).await.is_err());
        for field in ["id", "response", "context", "digest"] {
            let mut forged = derived.lease().clone();
            for root in &mut forged.dependencies {
                if let ContextEvidence::CompanionHistory {
                    id,
                    response_id,
                    context_id,
                    content_digest,
                } = root
                {
                    match field {
                        "id" => *id = Uuid::new_v4(),
                        "response" => *response_id = Uuid::new_v4(),
                        "context" => *context_id = Uuid::new_v4(),
                        "digest" => *content_digest = "0".repeat(64),
                        _ => unreachable!(),
                    }
                }
            }
            assert!(
                validate_evidence(&tx, &forged).await.is_err(),
                "forged {field}"
            );
        }
        let invitation = load(&tx, f.owner, binding.context_id).await.unwrap();
        let admission = invitation
            .dependencies
            .iter()
            .find(|root| matches!(root, ContextEvidence::CompanionAdmission { .. }))
            .unwrap();
        let mut widened = derived.lease().clone();
        widened.dependencies.push(admission.clone());
        assert!(validate_evidence(&tx, &widened).await.is_err());
        tx.rollback().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn memory_and_cache_preserve_history_roots_and_reject_a_deleted_invitation_on_consumption() {
    with_fixture(|f| async move {
        use crate::providers::adapter::embedding_text_digest;
        let (_, _, _, assistant) = committed_invitation(&f, 60).await;
        let origin = generation(&f, "Remember this ordinary response context").await;
        let backend = memory(&f).await;
        let snapshot = backend.bind_context(backend.write_snapshot(f.owner).await.unwrap(), origin.lease().id(), MemoryWriteOrigin::Ai).await.unwrap();
        let id = backend.store_guarded(&snapshot, MemoryNode::new("core", "fact", "A safe history-derived fact", 1.0).with_runtime_fields("semantic", "user", 1.0, 1.0, "internal")).await.unwrap();
        let id = Uuid::parse_str(&id).unwrap();
        let tx = f.db.begin().await.unwrap();
        let (roots, _) = super::super::memory_appraisal::direct_roots(&tx, f.owner, id).await.unwrap();
        assert!(has_marker(&roots, assistant));
        super::super::memory_appraisal::validate_memory_roots_in_transaction(&tx, f.owner, id, 1).await.unwrap();
        tx.rollback().await.unwrap();

        let answer = "A cached ordinary answer with invitation provenance";
        let (response_id, _) = commit_generation(&f, &origin, answer).await;
        let cache_id = Uuid::new_v4();
        f.db.execute(stmt("INSERT INTO llm_cache(id,user_id,scope,query_hash,query_text,response_text,metadata,revision,context_id,origin_response_id,status,content_digest,expires_at) VALUES($1,$2,'conversation',$3,'history query',$4,'{}',1,$5,$6,'active',$7,clock_timestamp()+interval '1 hour')", vec![cache_id.into(),f.owner.into(),embedding_text_digest("history query").into(),answer.into(),origin.lease().id().into(),response_id.into(),digest(&json!({"response_text":answer})).unwrap().into()])).await.unwrap();
        let current = generation(&f, "Reuse an ordinary cached answer").await;
        let identity = CacheIdentity { cache_id, revision: 1, context_id: origin.lease().id(), scope: "conversation".into() };
        let merged = f.context().merge_cache(&current, identity.clone()).await.unwrap();
        assert!(has_marker(merged.lease().dependencies(), assistant));
        f.context().validate(&merged.lease().binding(), ContextUse::OutputAdmission).await.unwrap();

        // This is consumption-time revocation, not a claim of physical Memory erasure.
        f.db.execute(stmt("DELETE FROM chat_messages WHERE id=$1", vec![assistant.into()])).await.unwrap();
        let tx = f.db.begin().await.unwrap();
        assert!(super::super::memory_appraisal::validate_memory_roots_in_transaction(&tx, f.owner, id, 1).await.is_err());
        tx.rollback().await.unwrap();
        assert_revoked(&f, &merged).await;
        assert!(f.context().merge_cache(&current, identity).await.is_err());
    }).await;
}

#[tokio::test]
async fn ordinary_appraisal_with_invitation_history_cannot_become_another_companion_source() {
    with_fixture(|f| async move {
        let (_, _, _, assistant) = committed_invitation(&f, 60).await;
        let (appraisal_id, _) = f.seed().await;
        let tx = f.db.begin().await.unwrap();
        let context_id: Uuid = tx
            .query_one(stmt(
                "SELECT context_id FROM appraisal_records WHERE appraisal_id=$1",
                vec![appraisal_id.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "context_id")
            .unwrap();
        let context = load(&tx, f.owner, context_id).await.unwrap();
        assert!(has_marker(context.dependencies(), assistant));
        let authority = lock_appraisal_authority(&tx, f.owner).await.unwrap();
        let rejected = load_companion_source(&tx, f.owner, appraisal_id, &authority)
            .await
            .err()
            .expect("history appraisal became a new Companion source");
        assert!(
            rejected
                .to_string()
                .contains("an invitation cannot become a new Companion source"),
            "unexpected refusal: {rejected:#}"
        );
        tx.rollback().await.unwrap();
        assert_eq!(f.count("companion_admissions").await, 1);
        assert_eq!(f.count("deliveries").await, 0);
    })
    .await;
}
