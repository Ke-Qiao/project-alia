use super::*;
use crate::modules::delivery::types::{
    DeliveryMedium, DeliveryStatus, ResponseRecord, ResponseStatus,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

struct Model {
    broken: bool,
    large: bool,
}
impl ModuleLifecycle for Model {
    fn name(&self) -> &'static str {
        "text-model-test"
    }
}
#[async_trait]
impl LlmAdapter for Model {
    async fn chat(&self, _: &[ChatMessage]) -> anyhow::Result<String> {
        Ok("完整回答。".into())
    }
    async fn chat_stream(
        &self,
        _: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        let mut chunks = if self.large {
            vec![Ok("你".repeat(4000))]
        } else {
            vec![Ok("流式".into()), Ok("回答。".into())]
        };
        if self.broken {
            chunks.push(Err(anyhow::anyhow!("injected model stream disconnected")));
        }
        Ok(Box::pin(futures::stream::iter(chunks)))
    }
}
async fn respond(f: &Fixture) {
    process_event(
        ChannelEvent::TextMessage {
            channel_id: "test".into(),
            user_id: Uuid::from_u128(42).to_string(),
            text: "Say a reply".into(),
            source_received_at: chrono::Utc::now() - chrono::Duration::hours(2),
            source_privacy_epoch: 41,
            source_relationship_epoch: 0,
        },
        &f.modules,
        &f.channels,
        &f.sensors,
        &EventBus::new(32),
        CancellationToken::new(),
        false,
        &f.policy.conversation,
        &f.policy.tools,
        &f.policy.cache,
        &f.policy.cache_policy,
        &f.policy.memory,
    )
    .await
    .unwrap();
}
async fn configure(f: &mut Fixture, stream: bool, broken: bool, large: bool) {
    let model = Arc::new(Model { broken, large });
    let modules = Arc::get_mut(&mut f.modules).unwrap();
    modules.llm = model.clone();
    modules.major_llm = model.clone();
    modules.minor_llm = model;
    f.output = Arc::new(OutputChannel {
        owner: Some(Uuid::from_u128(42)),
        streaming: stream,
        ..Default::default()
    });
    f.channels = Arc::new(ChannelRegistry::new());
    f.channels.register(f.output.clone()).await.unwrap();
}
async fn response(f: &Fixture) -> ResponseRecord {
    let db = f.conversation.db.as_ref().unwrap();
    let id: Uuid = db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT response_id FROM delivery_responses".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "response_id")
        .unwrap();
    f.modules
        .delivery
        .get_response(Uuid::from_u128(42), id)
        .await
        .unwrap()
}

#[tokio::test]
async fn buffered_and_streamed_text_each_commit_one_response_and_source_epoch() {
    for streaming in [false, true] {
        with_linked_fixture(move |mut f| async move {
            configure(&mut f, streaming, false, false).await;
            respond(&f).await;
            let r = response(&f).await;
            assert_eq!(r.status, ResponseStatus::Committed);
            assert_eq!(r.source_privacy_epoch, Some(41));
            assert!(r.source_event_id.is_some());
            assert!(r.source_received_at < chrono::Utc::now() - chrono::Duration::minutes(90));
            let deliveries = f
                .modules
                .delivery
                .list_deliveries(Uuid::from_u128(42), r.response_id)
                .await
                .unwrap();
            assert_eq!(deliveries.len(), 1);
            let d = &deliveries[0];
            assert_eq!(d.media_kind, DeliveryMedium::Text);
            assert_eq!(
                d.status,
                DeliveryStatus::Unknown,
                "memory-only fixture must not invent native write evidence"
            );
            assert!(d.identity.is_none());
            assert!(d.text.as_ref().unwrap().end_offered);
            let text = d.text.as_ref().unwrap();
            assert!(text.identity.is_some());
            assert!(text.progress.as_ref().unwrap().end_accepted);
            assert!(text.progress.as_ref().unwrap().written_utf8_bytes.is_none());
            let chunks = f
                .modules
                .delivery
                .text_chunks(Uuid::from_u128(42), d.delivery_id, None, 256)
                .await
                .unwrap();
            let full = chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>();
            assert_eq!(r.final_content, Some(serde_json::json!({"text":full})));
            assert_eq!(
                f.conversation
                    .messages
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|message| message.role == "assistant")
                    .count(),
                1
            );
            if !streaming {
                assert_eq!(
                    *f.output.texts.lock().unwrap(),
                    vec!["完整回答。".to_string()]
                );
            }
        })
        .await;
    }
}

#[tokio::test]
async fn broken_model_stream_keeps_partial_chunks_without_success_end_or_assistant_history() {
    with_linked_fixture(|mut f| async move {
        configure(&mut f, true, true, false).await;
        respond(&f).await;
        let r = response(&f).await;
        assert_eq!(r.status, ResponseStatus::Interrupted);
        assert!(r.final_content.is_none());
        let deliveries = f
            .modules
            .delivery
            .list_deliveries(Uuid::from_u128(42), r.response_id)
            .await
            .unwrap();
        let d = deliveries
            .iter()
            .find(|record| record.output_key == "text:response")
            .unwrap();
        assert_ne!(d.status, DeliveryStatus::Submitted);
        assert!(!d.text.as_ref().unwrap().end_offered);
        let chunks = f
            .modules
            .delivery
            .text_chunks(Uuid::from_u128(42), d.delivery_id, None, 256)
            .await
            .unwrap();
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>(),
            "流式回答。"
        );
        assert!(
            !f.conversation
                .messages
                .lock()
                .unwrap()
                .iter()
                .any(|message| message.role == "assistant")
        );
    })
    .await;
}

#[tokio::test]
async fn a_large_provider_chunk_splits_only_at_utf8_boundaries_under_one_delivery() {
    with_linked_fixture(|mut f| async move {
        configure(&mut f, true, false, true).await;
        respond(&f).await;
        let r = response(&f).await;
        assert_eq!(r.status, ResponseStatus::Committed);
        let d = f
            .modules
            .delivery
            .list_deliveries(Uuid::from_u128(42), r.response_id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let chunks = f
            .modules
            .delivery
            .text_chunks(Uuid::from_u128(42), d.delivery_id, None, 256)
            .await
            .unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].utf8_offset, 0);
        assert_eq!(chunks[1].utf8_offset, chunks[0].text.len() as u64);
        assert_eq!(chunks[1].sequence, 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.text.len() <= alia_core::text_delivery::MAX_TEXT_CHUNK_BYTES)
        );
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<String>(),
            "你".repeat(4000)
        );
    })
    .await;
}
