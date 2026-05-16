use std::sync::Arc;

use async_trait::async_trait;

use crate::channels::adapter::ChannelAction;
use crate::channels::registry::ChannelRegistry;
use crate::providers::adapter::{ToolContext, ToolDefinition, ToolExecutor, ToolResult};

pub struct EmbeddedVolumeTool {
    channels: Arc<ChannelRegistry>,
}

impl EmbeddedVolumeTool {
    pub fn new(channels: Arc<ChannelRegistry>) -> Self {
        Self { channels }
    }
}

#[async_trait]
impl ToolExecutor for EmbeddedVolumeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "embedded_volume_control".to_string(),
            description: "Get or set microphone and speaker volume for an alia-embedded channel. Use this when the user asks to make the Pi mic/speaker louder or quieter.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["get", "set"],
                        "description": "Use 'get' to read current volume or 'set' to update it."
                    },
                    "channel_id": {
                        "type": "string",
                        "description": "Embedded channel id. Optional when the current conversation is already on an embedded channel or only one embedded channel is connected."
                    },
                    "capture_volume": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": 8,
                        "description": "Microphone gain. 1.0 keeps the original level; values above 1.0 amplify."
                    },
                    "playback_volume": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": 8,
                        "description": "Speaker gain. 1.0 keeps the original level; values above 1.0 amplify."
                    }
                },
                "required": ["operation"]
            }),
            source: "component:embedded".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let operation = arguments
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                if arguments.get("capture_volume").is_some()
                    || arguments.get("playback_volume").is_some()
                {
                    "set"
                } else {
                    "get"
                }
            });
        let channel = resolve_embedded_channel(&self.channels, &arguments, ctx).await?;

        let action = match operation {
            "get" => ChannelAction {
                name: "get_volume".to_string(),
                params: serde_json::json!({}),
            },
            "set" => {
                if arguments.get("capture_volume").is_none()
                    && arguments.get("playback_volume").is_none()
                {
                    anyhow::bail!("set operation requires capture_volume or playback_volume");
                }
                ChannelAction {
                    name: "set_volume".to_string(),
                    params: serde_json::json!({
                        "capture_volume": arguments.get("capture_volume").cloned(),
                        "playback_volume": arguments.get("playback_volume").cloned(),
                    }),
                }
            }
            other => anyhow::bail!("unsupported embedded volume operation '{other}'"),
        };

        let output = channel.execute_action(action).await?;
        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            display: Some(display_volume_result(operation, &output)),
            output,
        })
    }
}

async fn resolve_embedded_channel(
    channels: &ChannelRegistry,
    arguments: &serde_json::Value,
    ctx: &ToolContext,
) -> anyhow::Result<Arc<dyn crate::channels::adapter::ChannelAdapter>> {
    if let Some(channel_id) = arguments
        .get("channel_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or(ctx.channel_id.as_deref())
    {
        let channel = channels
            .get(channel_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("embedded channel '{channel_id}' not found"))?;
        if channel.channel_type() != "embedded" {
            anyhow::bail!("channel '{channel_id}' is not an embedded channel");
        }
        return Ok(channel);
    }

    let embedded = channels
        .list()
        .await
        .into_iter()
        .filter(|channel| channel.channel_type() == "embedded")
        .collect::<Vec<_>>();

    match embedded.len() {
        0 => anyhow::bail!("no embedded channel is connected"),
        1 => Ok(embedded[0].clone()),
        _ => anyhow::bail!("multiple embedded channels are connected; provide channel_id"),
    }
}

fn display_volume_result(operation: &str, output: &serde_json::Value) -> String {
    let capture = output
        .get("capture_volume")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let playback = output
        .get("playback_volume")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    match operation {
        "set" => format!("Updated embedded volume: mic {capture:.2}, speaker {playback:.2}"),
        _ => format!("Embedded volume: mic {capture:.2}, speaker {playback:.2}"),
    }
}
