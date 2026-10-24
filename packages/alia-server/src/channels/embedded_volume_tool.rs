use std::sync::Arc;

use crate::channels::adapter::ChannelAction;
use crate::channels::registry::ChannelRegistry;
use crate::providers::adapter::{ToolContext, ToolEntry, ToolResult};

pub struct EmbeddedVolumeTool {
    channels: Arc<ChannelRegistry>,
}

impl EmbeddedVolumeTool {
    pub fn new(channels: Arc<ChannelRegistry>) -> Self {
        Self { channels }
    }
}

impl EmbeddedVolumeTool {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.EmbeddedVolumeTool";
        ToolEntry::from_tool(
            self,
            "component:embedded".to_string(),
            revision,
            false,
            None,
        )
        .map(|entry| {
            entry.with_routing_hints(vec![
                "pi".to_string(),
                "raspberry".to_string(),
                "mic".to_string(),
                "microphone".to_string(),
                "microphone volume".to_string(),
                "capture volume".to_string(),
                "speaker".to_string(),
                "speaker volume".to_string(),
                "playback volume".to_string(),
                "audio volume".to_string(),
                "voice volume".to_string(),
                "set volume".to_string(),
                "adjust volume".to_string(),
                "louder".to_string(),
                "quieter".to_string(),
                "volume".to_string(),
                "麦克风".to_string(),
                "麦克風".to_string(),
                "麥克風".to_string(),
                "话筒".to_string(),
                "話筒".to_string(),
                "拾音".to_string(),
                "收音".to_string(),
                "输入音量".to_string(),
                "輸入音量".to_string(),
                "喇叭".to_string(),
                "扬声器".to_string(),
                "揚聲器".to_string(),
                "播放音量".to_string(),
                "音量".to_string(),
                "声音".to_string(),
                "聲音".to_string(),
                "语音".to_string(),
                "語音".to_string(),
                "设置音量".to_string(),
                "設定音量".to_string(),
                "调整音量".to_string(),
                "調整音量".to_string(),
                "调整".to_string(),
                "調整".to_string(),
                "调大".to_string(),
                "調大".to_string(),
                "调小".to_string(),
                "調小".to_string(),
                "放大".to_string(),
                "小声".to_string(),
                "小聲".to_string(),
            ])
        })
    }

    pub async fn execute(
        &self,
        arguments: EmbeddedVolumeToolArgs,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let operation = arguments.operation;
        let channel =
            resolve_embedded_channel(&self.channels, arguments.channel_id.as_deref(), ctx).await?;
        let action = match operation {
            VolumeOperation::Get => ChannelAction {
                name: "get_volume".into(),
                params: serde_json::json!({}),
            },
            VolumeOperation::Set => {
                anyhow::ensure!(
                    arguments.capture_volume.is_some() || arguments.playback_volume.is_some(),
                    "set operation requires capture_volume or playback_volume"
                );
                ChannelAction {
                    name: "set_volume".into(),
                    params: serde_json::json!({"capture_volume":arguments.capture_volume,"playback_volume":arguments.playback_volume}),
                }
            }
        };

        let output = channel.execute_action(action).await?;
        Ok(ToolResult {
            presentation: None,
            call_id: String::new(),
            success: true,
            display: Some(display_volume_result(operation, &output)),
            output,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedVolumeToolArgs {
    pub operation: VolumeOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[schemars(range(min = 0, max = 8))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_volume: Option<f64>,
    #[schemars(range(min = 0, max = 8))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_volume: Option<f64>,
}

impl rig_core::tool::Tool for EmbeddedVolumeTool {
    const NAME: &'static str = "embedded_volume_control";
    type Args = EmbeddedVolumeToolArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Get or set microphone capture volume and speaker playback volume for an alia-embedded channel. Use this when the user asks to make the Pi mic/speaker louder, quieter, or adjust voice/audio volume.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

async fn resolve_embedded_channel(
    channels: &ChannelRegistry,
    channel_id: Option<&str>,
    ctx: &ToolContext,
) -> anyhow::Result<Arc<dyn crate::channels::adapter::ChannelAdapter>> {
    if let Some(channel_id) = channel_id
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

fn display_volume_result(operation: VolumeOperation, output: &serde_json::Value) -> String {
    let capture = output
        .get("capture_volume")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let playback = output
        .get("playback_volume")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    match operation {
        VolumeOperation::Set => {
            format!("Updated embedded volume: mic {capture:.2}, speaker {playback:.2}")
        }
        _ => format!("Embedded volume: mic {capture:.2}, speaker {playback:.2}"),
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VolumeOperation {
    Get,
    Set,
}
