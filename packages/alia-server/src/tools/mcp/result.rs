use crate::tools::entry::tool_error;
use base64::Engine;
use rig_core::{
    message::{ImageMediaType, MimeType, ToolResultContent},
    tool::{ToolExecutionError, ToolOutput},
};
use rmcp::model::{CallToolResult, ContentBlock};

pub(super) fn presentation(result: &CallToolResult) -> Result<ToolOutput, ToolExecutionError> {
    if result.content.len() > 128 {
        return Err(ToolExecutionError::other(
            "MCP content count budget; outcome unknown",
        ));
    }
    let mut output = Vec::new();
    if let Some(value) = &result.structured_content {
        output.push(ToolResultContent::json(value.clone()));
    }
    for content in &result.content {
        output.push(match content {
            ContentBlock::Text(text) => ToolResultContent::text(text.text.clone()),
            ContentBlock::Image(image) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&image.data)
                    .map_err(tool_error)?;
                if bytes.len() > 512 * 1024 {
                    return Err(ToolExecutionError::other(
                        "MCP image byte budget; outcome unknown",
                    ));
                }
                match ImageMediaType::from_mime_type(&image.mime_type) {
                    Some(mime) => {
                        ToolResultContent::image_base64(image.data.clone(), Some(mime), None)
                    }
                    None => {
                        ToolResultContent::json(serde_json::to_value(content).map_err(tool_error)?)
                    }
                }
            }
            // Preserve the explicit URI, MIME type and metadata with the body;
            // never dereference resource links, icons or embedded resource URIs.
            _ => ToolResultContent::json(serde_json::to_value(content).map_err(tool_error)?),
        });
    }
    if output.is_empty() {
        output.push(ToolResultContent::text(""));
    }
    ToolOutput::content(output)
}
