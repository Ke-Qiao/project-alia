//! Real PostgreSQL checks for the dedicated grant and every Delivery boundary.
use super::*;
use crate::modules::{
    companion::{
        live::{
            LiveRepository,
            tests::{Fixture, with_fixture},
            types::*,
        },
        types::{CandidateKind, CompanionMode, CompanionPolicy},
    },
    delivery::{
        repository::DeliveryRepository,
        types::{NewDelivery, NewResponse, NewTextDelivery, ResponseRecord},
    },
};
use alia_core::{delivery::PlaybackIdentity, text_delivery::TextChunk};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde_json::Value;

fn enabled(ttl: u32) -> CompanionPolicy {
    CompanionPolicy {
        mode: CompanionMode::Enabled,
        candidate_ttl_seconds: ttl,
        ..Default::default()
    }
}

fn response_input(response: &ResponseRecord) -> NewResponse {
    NewResponse {
        key: response.key.clone(),
        context_id: response.context_id.unwrap(),
        conversation_id: response.conversation_id,
        source_event_id: response.source_event_id,
        source_received_at: response.source_received_at,
        source_privacy_epoch: response.source_privacy_epoch,
        channel_id: response.channel_id.clone(),
        kind: response.kind.clone(),
        content: response.content.clone(),
    }
}

fn audio(response: &ResponseRecord, owner_id: Uuid) -> NewDelivery {
    let content: InvitationContent = serde_json::from_value(response.content.clone()).unwrap();
    NewDelivery {
        response_id: response.response_id,
        output_key: INVITATION_MEDIA_KEY.into(),
        content: json!({"text": content.text}),
        owner_id,
        identity: Some(PlaybackIdentity {
            scope: content.voice_scope,
            playback_id: Uuid::new_v4(),
            playback_sequence: 1,
        }),
    }
}

async fn validate(
    f: &Fixture,
    binding: &ContextBinding,
    usage: ContextUse,
) -> ContextResult<ValidatedContext> {
    let tx = f.db.begin().await.unwrap();
    let result = ContextService::validate_in_transaction(&tx, binding, usage).await;
    tx.rollback().await.unwrap();
    result
}

#[tokio::test]
async fn invitation_keeps_original_parent_and_every_root_but_grants_only_new_output_request() {
    with_fixture(|f| async move {
        f.enable_live(enabled(60)).await;
        let live = f.live(Uuid::new_v4());
        let (permit, binding, response) = f.fresh(&live).await;
        let tx = f.db.begin().await.unwrap();
        let lease = load(&tx, f.owner, binding.context_id).await.unwrap();
        let admission = read_invitation_grant(&tx, f.owner, permit.admission_id())
            .await
            .unwrap();
        let source = &admission.candidate.observation;
        let parent = load(&tx, f.owner, source.context_id).await.unwrap();
        assert_ne!(binding.request_id, parent.source.request_id);
        assert_ne!(lease.id, parent.id);
        let mut original_source = lease.source.clone();
        original_source.request_id = parent.source.request_id;
        assert_eq!(original_source, parent.source);
        assert_eq!(lease.valid_until, Some(admission.candidate.expires_at));
        assert_eq!(parent.source.request_id, source.request_id);
        assert!(
            parent
                .dependencies
                .iter()
                .all(|root| lease.dependencies.contains(root))
        );
        let row = tx
            .query_one(stmt(
                "SELECT request_id,context_id FROM appraisal_records WHERE appraisal_id=$1",
                vec![source.appraisals[0].appraisal_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.try_get::<Uuid>("", "request_id").unwrap(),
            parent.source.request_id
        );
        assert_eq!(row.try_get::<Uuid>("", "context_id").unwrap(), parent.id);
        for index in 0..lease.dependencies.len() {
            let mut incomplete = lease.clone();
            incomplete.dependencies.remove(index);
            assert!(
                validate_evidence(&tx, &incomplete).await.is_err(),
                "omitted root {index} accepted"
            );
        }
        let mut ordinary = lease.clone();
        ordinary.purpose = ContextPurpose::Generation;
        assert!(validate_evidence(&tx, &ordinary).await.is_err());
        let mut duplicate = lease.clone();
        duplicate.dependencies.push(
            lease
                .dependencies
                .iter()
                .find(|root| matches!(root, ContextEvidence::CompanionAdmission { .. }))
                .unwrap()
                .clone(),
        );
        assert!(validate_evidence(&tx, &duplicate).await.is_err());
        tx.rollback().await.unwrap();
        for usage in [ContextUse::ResponseCommit, ContextUse::OutputAdmission] {
            validate(&f, &binding, usage).await.unwrap();
        }
        for usage in [
            ContextUse::AutomaticMemoryWrite,
            ContextUse::AiMemoryWrite,
            ContextUse::ManualMemoryWrite,
            ContextUse::MemoryMaintenance,
            ContextUse::MemoryEmbedding,
            ContextUse::CacheWrite,
            ContextUse::CacheReuse,
            ContextUse::CacheReview,
            ContextUse::CacheEmbedding,
            ContextUse::Execution,
            ContextUse::AppraisalWrite,
            ContextUse::AffectRead,
            ContextUse::AppraisalMaintenance,
        ] {
            assert!(
                validate(&f, &binding, usage).await.is_err(),
                "accepted {usage:?}"
            );
        }
        let mut other_owner = binding.clone();
        other_owner.owner_id = Uuid::new_v4();
        assert!(
            validate(&f, &other_owner, ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        // Ordinary Generation has broad uses, but never this fixed invitation identity.
        let mut forged = response_input(&response);
        forged.context_id = parent.id;
        forged.key.request_id = parent.source.request_id;
        assert!(
            DeliveryRepository::new(f.db.clone())
                .prepare(forged)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn invitation_rejects_changed_envelopes_final_text_and_additional_media_at_real_delivery_gates()
 {
    with_fixture(|f| async move {
        f.enable_live(enabled(60)).await;
        let live = f.live(Uuid::new_v4());
        let (_, _, response) = f.fresh(&live).await;
        let delivery = DeliveryRepository::new(f.db.clone());
        let valid = response_input(&response);
        assert_eq!(delivery.prepare(valid.clone()).await.unwrap().response_id, response.response_id);
        let mut variants = Vec::new();
        let mut changed = valid.clone(); changed.content["text"] = json!("different"); variants.push(changed);
        let mut changed = valid.clone(); changed.content["extra"] = json!(true); variants.push(changed);
        let mut changed = valid.clone(); changed.content["voice_scope"]["generation_epoch"] = json!(900); variants.push(changed);
        let mut changed = valid.clone(); changed.kind = "generation".into(); variants.push(changed);
        let mut changed = valid.clone(); changed.key.output_key = "other".into(); variants.push(changed);
        for changed in variants { assert!(delivery.prepare(changed).await.is_err()); }
        for final_content in [json!({"text":"different"}), json!({"text":CandidateKind::FollowUpInvitation.text(),"extra":true})] {
            let tx = f.db.begin().await.unwrap();
            assert!(DeliveryRepository::mark_generated_in_transaction(&tx, f.owner, response.response_id, Uuid::new_v4(), final_content).await.is_err());
            tx.rollback().await.unwrap();
        }
        let tx = f.db.begin().await.unwrap();
        let assistant = Uuid::new_v4();
        tx.execute(stmt("INSERT INTO chat_messages(id,session_id,user_id,role,content) VALUES($1,$2,$3,'assistant',$4)", vec![assistant.into(), f.session.into(), f.owner.to_string().into(), CandidateKind::FollowUpInvitation.text().into()])).await.unwrap();
        DeliveryRepository::mark_generated_in_transaction(&tx, f.owner, response.response_id, assistant, json!({"text":CandidateKind::FollowUpInvitation.text()})).await.unwrap();
        tx.commit().await.unwrap();
        let instance = Uuid::new_v4();
        let valid_audio = audio(&response, instance);
        let mut altered = valid_audio.clone(); altered.content["extra"] = json!(true);
        assert!(delivery.prepare_delivery(f.owner, altered).await.is_err());
        let mut altered = valid_audio.clone(); altered.identity.as_mut().unwrap().scope.generation_epoch += 1;
        assert!(delivery.prepare_delivery(f.owner, altered).await.is_err());
        let mut altered = valid_audio.clone(); altered.output_key = "extra-audio".into();
        assert!(delivery.prepare_delivery(f.owner, altered).await.is_err());
        assert!(delivery.prepare_text_delivery(f.owner, NewTextDelivery { response_id:response.response_id,output_key:INVITATION_MEDIA_KEY.into(),content:valid_audio.content.clone(),identity:None,owner_id:instance }).await.is_err());
        let prepared = delivery.prepare_delivery(f.owner, valid_audio.clone()).await.unwrap();
        assert_eq!(delivery.prepare_delivery(f.owner, valid_audio.clone()).await.unwrap().delivery_id, prepared.delivery_id);
        let mut second = valid_audio; second.identity.as_mut().unwrap().playback_id = Uuid::new_v4();
        assert!(delivery.prepare_delivery(f.owner, second).await.is_err());
        let dispatch = delivery.dispatch(f.owner, prepared.delivery_id, instance).await.unwrap().unwrap();
        assert!(delivery.text_start(&dispatch).await.is_err());
        assert!(delivery.offer_text_chunk(&dispatch, &TextChunk { sequence:0,utf8_offset:0,text:CandidateKind::FollowUpInvitation.text().into() }).await.is_err());
        assert!(delivery.offer_text_end(&dispatch, 0, 0).await.is_err());
        delivery.start(&dispatch, 16000, 1).await.unwrap();
        assert_eq!(f.count("delivery_responses").await, 1);
        assert_eq!(f.count("deliveries").await, 1);
    }).await;
}

async fn wait_until(f: &Fixture, deadline: DateTime<Utc>) {
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        loop {
            let reached =
                f.db.query_one(stmt(
                    "SELECT clock_timestamp()>=$1 AS reached",
                    vec![deadline.into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<bool>("", "reached")
                .unwrap();
            if reached {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("database clock did not reach fixed deadline");
}

#[tokio::test]
async fn invitation_short_admission_expiry_preserves_existing_output_until_original_candidate_deadline()
 {
    with_fixture(|f| async move {
        f.enable_live(enabled(4)).await;
        let live = f.live(Uuid::new_v4());
        let (permit, binding, response) = f.fresh(&live).await;
        let tx = f.db.begin().await.unwrap();
        let admission = read_invitation_grant(&tx, f.owner, permit.admission_id())
            .await
            .unwrap();
        let mut at_admission_boundary = admission.clone();
        at_admission_boundary.admit_before = at_admission_boundary.input.as_of;
        assert!(at_admission_boundary.validate().is_err());
        tx.rollback().await.unwrap();
        assert!(permit.admit_before() < permit.deadline());
        wait_until(&f, permit.admit_before()).await;
        validate(&f, &binding, ContextUse::OutputAdmission)
            .await
            .unwrap();
        let tx = f.db.begin().await.unwrap();
        assert!(
            issue_invitation_in_transaction(&tx, f.owner, permit.admission_id())
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
        let delivery = DeliveryRepository::new(f.db.clone());
        delivery
            .prepare_delivery(f.owner, audio(&response, Uuid::new_v4()))
            .await
            .unwrap();
        wait_until(&f, permit.deadline()).await;
        assert!(
            validate(&f, &binding, ContextUse::OutputAdmission)
                .await
                .is_err()
        );
        assert!(
            validate(&f, &binding, ContextUse::ResponseCommit)
                .await
                .is_err()
        );
        assert!(delivery.prepare(response_input(&response)).await.is_err());
        assert_eq!(f.count("companion_admissions").await, 1);
    })
    .await;
}

#[tokio::test]
async fn invitation_stored_content_and_final_content_are_revalidated_before_output() {
    with_fixture(|f| async move {
        f.enable_live(enabled(60)).await;
        let live = f.live(Uuid::new_v4());
        let (_, _, response) = f.fresh(&live).await;
        let valid = response_input(&response);
        for (field, value) in [("content", json!({"text":"corrupt"})), ("final_content", json!({"text":"corrupt"}))] {
            let tx = f.db.begin().await.unwrap();
            if field == "content" {
                tx.execute(stmt("UPDATE delivery_responses SET content=$2 WHERE response_id=$1", vec![response.response_id.into(), value.into()])).await.unwrap();
            } else {
                // Deliberate persisted corruption: no production bypass or permit is used.
                tx.execute(stmt("UPDATE delivery_responses SET status='committed',assistant_message_id=$2,final_content=$3 WHERE response_id=$1", vec![response.response_id.into(), Uuid::new_v4().into(), value.into()])).await.unwrap();
            }
            assert!(DeliveryRepository::prepare_in_transaction(&tx, valid.clone()).await.is_err());
            assert!(crate::modules::delivery::repository::validate_response_source(&tx, f.owner, response.response_id).await.is_err());
            tx.rollback().await.unwrap();
        }
    }).await;
}

#[tokio::test]
async fn invitation_current_policy_and_source_erasure_revoke_gates_but_preserve_cleanup_and_charge()
{
    for cause in ["policy", "source", "privacy", "relationship"] {
        with_fixture(|f| async move {
            let policy = f.enable_live(enabled(60)).await;
            let live: LiveRepository = f.live(Uuid::new_v4());
            let (permit, binding, response) = f.fresh(&live).await;
            let delivery = DeliveryRepository::new(f.db.clone());
            let instance = Uuid::new_v4();
            let prepared = delivery.prepare_delivery(f.owner, audio(&response, instance)).await.unwrap();
            let dispatch = delivery.dispatch(f.owner, prepared.delivery_id, instance).await.unwrap().unwrap();
            match cause {
                "policy" => { live.replace_policy(f.owner, policy.revision, CompanionPolicy { paused:true, ..policy.policy }).await.unwrap(); }
                "source" => { f.db.execute(stmt("UPDATE chat_messages SET content='changed source' WHERE id=$1", vec![response.source_event_id.unwrap().into()])).await.unwrap(); }
                "privacy" => { f.db.execute(stmt("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1", vec![f.owner.into()])).await.unwrap(); }
                "relationship" => { f.context().clear_relationship(f.owner, Uuid::new_v4(), 0).await.unwrap(); }
                _ => unreachable!(),
            }
            assert!(validate(&f, &binding, ContextUse::OutputAdmission).await.is_err(), "{cause}");
            assert!(delivery.prepare(response_input(&response)).await.is_err(), "{cause}");
            assert!(delivery.start(&dispatch, 16000, 1).await.is_err(), "{cause}");
            delivery.finish_unknown(&dispatch, "owner closed after authority revocation").await.unwrap();
            let row = f.db.query_one(stmt("SELECT state,private_grant,grant_digest,intent_digest FROM companion_admissions WHERE admission_id=$1", vec![permit.admission_id().into()])).await.unwrap().unwrap();
            assert_eq!(row.try_get::<String>("", "state").unwrap(), "retired");
            assert!(row.try_get::<Option<Value>>("", "private_grant").unwrap().is_none());
            assert!(row.try_get::<Option<String>>("", "grant_digest").unwrap().is_none());
            assert!(row.try_get::<Option<String>>("", "intent_digest").unwrap().is_none());
            assert!(delivery.response(f.owner, response.response_id).await.unwrap().cancel_requested);
            assert_eq!(f.count("companion_admissions").await, 1);
            assert_eq!(f.count("companion_live_sources").await, 0);
        }).await;
    }
}
