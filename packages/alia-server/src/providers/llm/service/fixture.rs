//! Test transports still pass every reply through Rig's decoder and fold.
use super::*;
use futures::future::BoxFuture;
use rig_core::{
    Model,
    driver::{Exchange, Opened, Opening, Transport},
    test_utils::{MockFrame, MockScript, MockStreamEvent},
};
use std::{future::Future, sync::Arc};

type Handler = dyn Fn(Vec<ChatMessage>) -> BoxFuture<'static, Result<TextStream>> + Send + Sync;

#[derive(Clone)]
struct ScriptTransport(Arc<Handler>);

impl Transport<MockScript> for ScriptTransport {
    fn send(&self, request: CompletionRequest, _: Exchange) -> Opening<MockFrame> {
        let handler = self.0.clone();
        Opening::new(async move {
            let messages = request
                .chat_history
                .into_iter()
                .map(|message| match message {
                    Message::System { content } => ChatMessage::new("system", content),
                    Message::User { content } => ChatMessage::new(
                        "user",
                        content
                            .into_iter()
                            .filter_map(|part| match part {
                                rig_core::message::UserContent::Text(text) => Some(text.text),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(""),
                    ),
                    Message::Assistant(turn) => ChatMessage::new(
                        "assistant",
                        turn.content
                            .into_iter()
                            .filter_map(|part| match part {
                                AssistantContent::Text(text) => Some(text.text),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(""),
                    ),
                })
                .collect();
            let mut stream = handler(messages).await.map_err(|_| {
                rig_core::error::ProviderError::Response("scripted model failed".into())
            })?;
            Ok(Opened::new(async_stream::stream! {
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(text) => yield Ok(MockFrame::Event(MockStreamEvent::Text(text))),
                        Err(_) => { yield Err(rig_core::error::ProviderError::Response("scripted model stream failed".into())); return; }
                    }
                }
                yield Ok(MockFrame::Event(MockStreamEvent::FinalResponse(rig_core::operation::Finish { reason: Some(FinishReason::Stop), ..Default::default() })));
            }))
        })
    }
}

impl LlmService {
    pub fn test_text<F, Fut>(name: &'static str, handler: F) -> Self
    where
        F: Fn(Vec<ChatMessage>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String>> + Send + 'static,
    {
        Self::test_stream(name, move |messages| {
            let future = handler(messages);
            async move {
                let text = future.await?;
                Ok(Box::pin(futures::stream::once(async { Ok(text) })) as TextStream)
            }
        })
    }

    pub fn test_stream<F, Fut>(name: &'static str, handler: F) -> Self
    where
        F: Fn(Vec<ChatMessage>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<TextStream>> + Send + 'static,
    {
        Self::from_model(
            name,
            Model::new(
                MockScript::new(name),
                ScriptTransport(Arc::new(move |messages| Box::pin(handler(messages)))),
            ),
        )
    }
}
