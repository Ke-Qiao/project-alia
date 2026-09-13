//! Text shares the real PostgreSQL dispatch ledger; byte receipts stay separate
//! from audio frames and never imply a human has read the output.
mod delivery_support;
use alia_core::delivery::ObservationEnd;
use alia_core::text_delivery::{TextChunk, TextIdentity, TextProgress, TextWriteBoundary};
use alia_server::modules::delivery::{repository::DeliveryRepository, types::*};
use delivery_support::*;
use serde_json::json;
use uuid::Uuid;

async fn prepared(f: &Fixture) -> (ResponseRecord, DeliveryRecord, DispatchPermit, TextIdentity) {
    let response = f.repo().prepare(f.response()).await.unwrap();
    let identity = TextIdentity {
        text_id: Uuid::new_v4(),
        channel_instance_id: Uuid::new_v4(),
    };
    let instance = Uuid::new_v4();
    let delivery = f
        .repo()
        .prepare_text_delivery(
            f.user,
            NewTextDelivery {
                response_id: response.response_id,
                output_key: "text:response".into(),
                content: json!({"representation":"plain_utf8"}),
                identity: Some(identity),
                owner_id: instance,
            },
        )
        .await
        .unwrap();
    let permit = f
        .repo()
        .dispatch(f.user, delivery.delivery_id, instance)
        .await
        .unwrap()
        .unwrap();
    f.repo().text_start(&permit).await.unwrap();
    (response, delivery, permit, identity)
}
fn chunk(sequence: u64, utf8_offset: u64, text: &str) -> TextChunk {
    TextChunk {
        sequence,
        utf8_offset,
        text: text.into(),
    }
}
fn progress(identity: TextIdentity, seq: u64, bytes: u64, written: u64) -> TextProgress {
    TextProgress {
        identity,
        receipt_sequence: seq,
        accepted_utf8_bytes: bytes,
        written_utf8_bytes: Some(written),
        write_boundary: Some(TextWriteBoundary::StdoutWrite),
        end_accepted: false,
        observation_end: None,
    }
}

#[tokio::test]
async fn immutable_contiguous_utf8_chunks_require_exact_end_and_actual_written_prefix() {
    with_db(|f| async move {
        let (_, delivery, permit, identity) = prepared(&f).await;
        let first = chunk(0, 0, "你");
        f.repo().offer_text_chunk(&permit, &first).await.unwrap();
        f.repo().offer_text_chunk(&permit, &first).await.unwrap();
        assert!(matches!(
            f.repo().offer_text_chunk(&permit, &chunk(0, 0, "好")).await,
            Err(DeliveryError::IdempotencyConflict)
        ));
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(2, 3, "x"))
                .await
                .is_err()
        );
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(1, 1, "x"))
                .await
                .is_err()
        );
        f.repo()
            .offer_text_chunk(&permit, &chunk(1, 3, "!\n"))
            .await
            .unwrap();
        assert!(f.repo().offer_text_end(&permit, 1, 5).await.is_err());
        assert!(f.repo().offer_text_end(&permit, 2, 4).await.is_err());
        let mut p = progress(identity, 1, 5, 1);
        f.repo()
            .text_progress(&permit, p.clone(), false)
            .await
            .unwrap();
        p.receipt_sequence = 2;
        p.observation_end = Some(ObservationEnd::SubmissionFinished);
        assert!(
            f.repo()
                .text_progress(&permit, p.clone(), true)
                .await
                .is_err()
        );
        f.repo().offer_text_end(&permit, 2, 5).await.unwrap();
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(2, 5, "late"))
                .await
                .is_err()
        );
        p.written_utf8_bytes = Some(5);
        p.end_accepted = true;
        let record = f
            .repo()
            .text_progress(&permit, p.clone(), true)
            .await
            .unwrap();
        assert_eq!(record.status, DeliveryStatus::Submitted);
        assert_eq!(record.media_kind, DeliveryMedium::Text);
        assert_eq!(record.presented_frames, None);
        assert_eq!(record.offered_frames, 0);
        assert_eq!(record.text.unwrap().progress, Some(p.clone()));
        assert!(f.repo().text_progress(&permit, p, true).await.is_ok());
        let page = f
            .repo()
            .text_chunks(f.user, delivery.delivery_id, None, 1)
            .await
            .unwrap();
        assert_eq!(page, vec![first]);
        assert_eq!(
            f.repo()
                .text_chunks(f.user, delivery.delivery_id, Some(0), 64)
                .await
                .unwrap(),
            vec![chunk(1, 3, "!\n")]
        );
        assert!(matches!(
            f.repo()
                .text_chunks(Uuid::new_v4(), delivery.delivery_id, None, 64)
                .await,
            Err(DeliveryError::NotFound)
        ));
    })
    .await;
}

#[tokio::test]
async fn text_bounds_and_cross_medium_evidence_are_enforced() {
    with_db(|f| async move {
        let (_, _, permit, identity) = prepared(&f).await;
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(0, 0, ""))
                .await
                .is_err()
        );
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(0, 0, &"x".repeat(8193)))
                .await
                .is_err()
        );
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(1024, 0, "x"))
                .await
                .is_err()
        );
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(0, 262144, "x"))
                .await
                .is_err()
        );
        assert!(f.repo().start(&permit, 24000, 1).await.is_err());
        f.repo()
            .offer_text_chunk(&permit, &chunk(0, 0, "abc"))
            .await
            .unwrap();
        let mut p = progress(identity, 1, 3, 3);
        p.observation_end = Some(ObservationEnd::PresentedFinished);
        assert!(f.repo().text_progress(&permit, p, true).await.is_err());
        let (_, _, audio) = f.prepared().await;
        let audio_permit = f
            .repo()
            .dispatch(f.user, audio.delivery_id, audio.owner_id)
            .await
            .unwrap()
            .unwrap();
        assert!(f.repo().text_start(&audio_permit).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn revoked_source_closes_new_offers_but_preserves_actual_late_write_facts() {
    with_db(|f| async move {
        let (_, _, permit, identity) = prepared(&f).await;
        f.repo()
            .offer_text_chunk(&permit, &chunk(0, 0, "你"))
            .await
            .unwrap();
        f.sql("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1")
            .await;
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(1, 3, "好"))
                .await
                .is_err()
        );
        assert!(f.repo().offer_text_end(&permit, 1, 3).await.is_err());
        let mut p = progress(identity, 1, 3, 1);
        p.observation_end = Some(ObservationEnd::Cancelled {
            reason: "source epoch revoked".into(),
        });
        let record = f.repo().text_progress(&permit, p, true).await.unwrap();
        assert_eq!(record.status, DeliveryStatus::Cancelled);
        assert_eq!(record.text.unwrap().offered_utf8_bytes, 3);
    })
    .await;
}

#[tokio::test]
async fn recovered_unknown_keeps_prefix_and_audits_late_checkpoint_without_replay() {
    with_db(|f| async move {
        let (response, delivery, permit, identity) = prepared(&f).await;
        f.repo()
            .offer_text_chunk(&permit, &chunk(0, 0, "你"))
            .await
            .unwrap();
        let p = progress(identity, 1, 3, 1);
        f.repo()
            .text_progress(&permit, p.clone(), false)
            .await
            .unwrap();
        f.sql("UPDATE deliveries SET lease_expires_at=clock_timestamp()-interval '1 second'")
            .await;
        let restarted = DeliveryRepository::new(f.db.clone());
        assert_eq!(restarted.recover_expired(256).await.unwrap(), 1);
        assert!(
            restarted
                .dispatch(f.user, delivery.delivery_id, Uuid::new_v4())
                .await
                .is_err()
        );
        let mut late = progress(identity, 2, 3, 2);
        late.observation_end = Some(ObservationEnd::Cancelled {
            reason: "original native write joined".into(),
        });
        for _ in 0..2 {
            restarted
                .text_progress(&permit, late.clone(), true)
                .await
                .unwrap();
        }
        let frozen = restarted.get(f.user, delivery.delivery_id).await.unwrap();
        assert_eq!(frozen.status, DeliveryStatus::Unknown);
        assert_eq!(frozen.text.unwrap().progress, Some(p));
        let events = restarted
            .events(f.user, response.response_id, 0, 256)
            .await
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "late_text_checkpoint")
                .count(),
            1
        );
        assert_eq!(f.count("delivery_text_chunks").await, 1);
    })
    .await;
}

#[tokio::test]
async fn text_dispatch_permit_and_intent_are_unique_across_repository_instances() {
    with_db(|f| async move {
        let (_, delivery, permit, _) = prepared(&f).await;
        assert!(
            f.repo()
                .dispatch(f.user, delivery.delivery_id, delivery.owner_id)
                .await
                .unwrap()
                .is_none()
        );
        f.sql(&format!(
            "UPDATE deliveries SET dispatch_id='{}'",
            Uuid::new_v4()
        ))
        .await;
        assert!(
            f.repo()
                .offer_text_chunk(&permit, &chunk(0, 0, "stale"))
                .await
                .is_err()
        );
        assert!(matches!(
            f.repo()
                .prepare_text_delivery(
                    f.user,
                    NewTextDelivery {
                        response_id: delivery.response_id,
                        output_key: delivery.output_key,
                        content: json!({"different":true}),
                        identity: delivery.text.unwrap().identity,
                        owner_id: delivery.owner_id
                    }
                )
                .await,
            Err(DeliveryError::IdempotencyConflict)
        ));
        assert_eq!(f.count("deliveries").await, 1);
    })
    .await;
}
