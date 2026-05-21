use std::sync::Arc;

use async_trait::async_trait;

use crate::channels::adapter::{ChannelAction, ChannelAdapter};
use crate::channels::registry::ChannelRegistry;
use crate::providers::adapter::{ToolContext, ToolDefinition, ToolExecutor, ToolResult};

enum ResolvedConversationControl {
    Channel(Arc<dyn ChannelAdapter>),
    Noop(String),
}

pub struct EndContinuousDialogueTool {
    channels: Arc<ChannelRegistry>,
}

impl EndContinuousDialogueTool {
    pub fn new(channels: Arc<ChannelRegistry>) -> Self {
        Self { channels }
    }
}

#[async_trait]
impl ToolExecutor for EndContinuousDialogueTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "end_continuous_dialogue".to_string(),
            description: "End the current voice channel's continuous dialogue or follow-up listening mode when the user explicitly says goodbye, cancel, stop talking, or otherwise asks to end the exchange. This is a generic channel action; use it only for explicit conversation cancellation, not ordinary topic changes.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel_id": {
                        "type": "string",
                        "description": "Optional channel id. Omit when the current conversation channel should be controlled."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Short reason, for example 'user_said_goodbye'."
                    }
                }
            }),
            routing_hints: vec![
                "goodbye".to_string(),
                "bye".to_string(),
                "cancel".to_string(),
                "stop talking".to_string(),
                "stop listening".to_string(),
                "end conversation".to_string(),
                "continuous dialogue".to_string(),
                "follow-up listening".to_string(),
                "再见".to_string(),
                "拜拜".to_string(),
                "退下".to_string(),
                "取消".to_string(),
                "不用了".to_string(),
                "别说了".to_string(),
                "先这样".to_string(),
                "结束对话".to_string(),
                "结束聊天".to_string(),
                "停止回答".to_string(),
                "停止说话".to_string(),
                "关闭连续对话".to_string(),
            ],
            source: "component:channels".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let resolved =
            match resolve_conversation_control_channel(&self.channels, &arguments, ctx).await {
                Ok(resolved) => resolved,
                Err(error) => {
                    return Ok(ToolResult {
                        call_id: String::new(),
                        success: false,
                        output: serde_json::json!({
                            "ended": false,
                            "error": error.to_string(),
                        }),
                        display: Some(format!("Could not end continuous dialogue: {error}")),
                    });
                }
            };
        let reason = arguments
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("user requested continuous dialogue end");

        let channel = match resolved {
            ResolvedConversationControl::Channel(channel) => channel,
            ResolvedConversationControl::Noop(message) => {
                return Ok(ToolResult {
                    call_id: String::new(),
                    success: true,
                    output: serde_json::json!({
                        "ended": false,
                        "noop": true,
                        "reason": message,
                    }),
                    display: Some("No continuous dialogue to end".to_string()),
                });
            }
        };

        let output = channel
            .execute_action(ChannelAction {
                name: "end_continuous_dialogue".to_string(),
                params: serde_json::json!({
                    "reason": reason,
                }),
            })
            .await?;

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output,
            display: Some("Continuous dialogue ended".to_string()),
        })
    }
}

async fn resolve_conversation_control_channel(
    channels: &ChannelRegistry,
    arguments: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<ResolvedConversationControl> {
    let explicit_channel_id = arguments
        .get("channel_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty());

    if let Some(channel_id) = explicit_channel_id.or(ctx.channel_id.as_deref()) {
        let (resolved_channel_id, channel) = channels
            .get_resolved(channel_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("channel '{channel_id}' not found"))?;
        if !channel.capabilities().conversation_control {
            if explicit_channel_id.is_none() {
                return Ok(ResolvedConversationControl::Noop(format!(
                    "channel '{resolved_channel_id}' does not support conversation control"
                )));
            }
            anyhow::bail!("channel '{resolved_channel_id}' does not support conversation control");
        }
        return Ok(ResolvedConversationControl::Channel(channel));
    }

    let controllable = channels
        .list()
        .await
        .into_iter()
        .filter(|channel| channel.capabilities().conversation_control)
        .collect::<Vec<_>>();

    match controllable.len() {
        0 => Ok(ResolvedConversationControl::Noop(
            "no connected channel supports conversation control".to_string(),
        )),
        1 => Ok(ResolvedConversationControl::Channel(
            controllable[0].clone(),
        )),
        _ => anyhow::bail!("multiple channels support conversation control; provide channel_id"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::channels::adapter::{ChannelCapabilities, ChannelTarget};
    use crate::module::ModuleLifecycle;
    use crate::module::lifecycle::ModuleHealth;

    use super::*;

    struct MockControlChannel {
        id: String,
        actions: Mutex<Vec<ChannelAction>>,
    }

    impl MockControlChannel {
        fn new(id: &str) -> Self {
            Self {
                id: id.to_string(),
                actions: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ModuleLifecycle for MockControlChannel {
        fn name(&self) -> &'static str {
            "mock-control-channel"
        }

        async fn health_check(&self) -> ModuleHealth {
            ModuleHealth::Healthy
        }
    }

    #[async_trait]
    impl ChannelAdapter for MockControlChannel {
        fn channel_type(&self) -> &'static str {
            "mock"
        }

        fn channel_id(&self) -> &str {
            &self.id
        }

        fn capabilities(&self) -> ChannelCapabilities {
            ChannelCapabilities {
                conversation_control: true,
                ..ChannelCapabilities::default()
            }
        }

        async fn send_message(&self, _target: &ChannelTarget, _msg: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
            self.actions.lock().unwrap().push(action);
            Ok(serde_json::json!({"ended": true}))
        }
    }

    #[tokio::test]
    async fn tool_executes_generic_channel_action_on_current_channel() {
        let registry = Arc::new(ChannelRegistry::new());
        let channel = Arc::new(MockControlChannel::new("voice"));
        registry.register(channel.clone()).await.unwrap();
        let tool = EndContinuousDialogueTool::new(registry);
        let ctx = ToolContext {
            user_id: uuid::Uuid::new_v4(),
            conversation_id: None,
            source: "test".to_string(),
            channel_id: Some("voice".to_string()),
        };

        let result = tool
            .execute(serde_json::json!({"reason": "user_said_goodbye"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        let actions = channel.actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].name, "end_continuous_dialogue");
        assert_eq!(actions[0].params["reason"], "user_said_goodbye");
    }

    #[tokio::test]
    async fn tool_noops_when_no_channel_supports_conversation_control() {
        let tool = EndContinuousDialogueTool::new(Arc::new(ChannelRegistry::new()));
        let ctx = ToolContext {
            user_id: uuid::Uuid::new_v4(),
            conversation_id: None,
            source: "test".to_string(),
            channel_id: None,
        };

        let result = tool.execute(serde_json::json!({}), &ctx).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output["ended"], false);
        assert_eq!(result.output["noop"], true);
    }

    #[test]
    fn definition_has_cancel_routing_hints() {
        let registry = Arc::new(ChannelRegistry::new());
        let definition = EndContinuousDialogueTool::new(registry).definition();

        assert_eq!(definition.name, "end_continuous_dialogue");
        assert!(definition.routing_hints.iter().any(|hint| hint == "再见"));
        assert!(!definition.require_confirmation);
    }
}
