use super::*;
use crate::{
    modules::{
        companion::live::tests::with_fixture,
        delivery::{DeliveryService, repository::DeliveryRepository},
        interaction::initiative::Request,
    },
    providers::{conversation::DatabaseConversationBackend, tts::NoopTts},
};

#[tokio::test]
async fn same_channel_has_one_outstanding_snapshot_and_close_drains_its_original_attempt() {
    with_fixture(|f| async move {
        let voice = VoiceInitiativeHandle::default();
        let instance = voice.instance_id();
        let (_registration, mut mailbox) = voice.register("companion-device".into(), f.session);
        let stop = CancellationToken::new();
        let delivery = DeliveryService::new(DeliveryRepository::new(f.db.clone()), &stop);
        let service = Arc::new(
            CompanionService::new(f.db.clone(), voice, stop).with_live_outputs(LiveOutputs {
                context: Arc::new(f.context()),
                delivery,
                // No opportunity is released. Any accidental output path would
                // encounter this deliberately uninitialized conversation backend.
                conversation: Arc::new(DatabaseConversationBackend::new()),
                tts: Arc::new(NoopTts),
            }),
        );
        f.enable_live(CompanionPolicy {
            mode: super::super::types::CompanionMode::Enabled,
            ..Default::default()
        })
        .await;
        let live = f.live(instance);
        let first = f.seed_pending(&live).await;
        let second = f.seed_pending(&live).await;
        assert_ne!(first.candidate_id, second.candidate_id);
        assert_eq!(first.channel_id, second.channel_id);
        service.start().await.unwrap();
        let held_reply = match tokio::time::timeout(Duration::from_secs(3), mailbox.recv())
            .await
            .expect("service never requested its first original opportunity")
            .unwrap()
        {
            Request::Snapshot { owner, reply } => {
                assert_eq!(owner, f.owner);
                reply
            }
            Request::Reserve { .. } => panic!("reserve without a snapshot/admission"),
        };
        // Keep the first actor reply unresolved across the observation window.
        // The second canonical candidate cannot overwrite that channel's actor
        // opportunity, even though the global runner capacity is four.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), mailbox.recv())
                .await
                .is_err(),
            "same-channel candidate requested a competing snapshot"
        );
        service.close();
        tokio::time::timeout(Duration::from_secs(3), service.stop())
            .await
            .expect("closing did not drain the original blocked snapshot")
            .unwrap();
        assert!(held_reply.is_closed());
        assert!(mailbox.try_recv().is_err());
        assert_eq!(service.active_requests(), 0);
        assert_eq!(f.count("companion_admissions").await, 0);
        assert_eq!(f.count("delivery_responses").await, 0);
        assert_eq!(f.count("deliveries").await, 0);
        assert!(service.start().await.is_err());
        service.stop().await.unwrap();
    })
    .await;
}
