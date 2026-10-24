use std::sync::{Arc, Weak};

use anyhow::{Context, Result};
use rig_core::{
    message::ToolName,
    tool::{DynamicTool, ToolOutput},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AdapterDescriptor, EmbodimentService};
use crate::{
    providers::adapter::ToolContext,
    tools::entry::{ToolEntry, tool_error},
};

const SOURCE: &str = "component:embodiment";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationArgument {
    operation_id: Uuid,
}

fn id_schema() -> Value {
    json!({"type":"object","properties":{"operation_id":{"type":"string","format":"uuid"}},"required":["operation_id"],"additionalProperties":false})
}

pub(super) fn status(owner: Weak<EmbodimentService>) -> Result<Arc<ToolEntry>> {
    let dynamic = DynamicTool::new_with_context(
        ToolName::new("embodiment_operation_status")?,
        "Read the current user's durable physical operation evidence. An accepted goal is not completed; cancellation is not proof that motion stopped.",
        id_schema(),
        move |context, args| {
            let owner = owner.clone();
            Box::pin(async move {
                let owner = owner
                    .upgrade()
                    .context("operation owner unavailable")
                    .map_err(tool_error)?;
                let caller = context.require::<ToolContext>().map_err(tool_error)?;
                let arg: OperationArgument = serde_json::from_value(args).map_err(tool_error)?;
                let record = owner
                    .get(caller.user_id, arg.operation_id)
                    .await
                    .map_err(tool_error)?
                    .context("operation not found")
                    .map_err(tool_error)?;
                Ok(ToolOutput::json(
                    serde_json::to_value(record).map_err(tool_error)?,
                ))
            })
        },
    );
    ToolEntry::dynamic(
        dynamic,
        SOURCE,
        "embodiment-operation-v1",
        false,
        None,
        json!({"read_only":true,"owner_scoped":true}),
    )
}

pub(super) fn adapter_entries(
    owner: Weak<EmbodimentService>,
    descriptor: &AdapterDescriptor,
) -> Result<Vec<Arc<ToolEntry>>> {
    [false, true]
        .into_iter()
        .map(|cancel| {
            let name = if cancel {
                descriptor.cancel_tool_name()
            } else {
                descriptor.start_tool_name()
            };
            let description = if cancel {
                "Request cancellation of an existing physical operation. This is a separate authorized effect, not proof of a physical stop."
            } else {
                "Submit a durable physical goal. Submitting or accepted is not completion; use embodiment_operation_status to inspect authoritative progress and stopped evidence."
            };
            let schema = if cancel {
                id_schema()
            } else {
                descriptor.goal_schema.clone()
            };
            let owner = owner.clone();
            let probe = owner.clone();
            let id = descriptor.adapter_id.clone();
            let dynamic = DynamicTool::new_with_context(
                ToolName::new(name)?,
                description,
                schema,
                move |context, args| {
                    let owner = owner.clone();
                    let id = id.clone();
                    Box::pin(async move {
                        let owner = owner
                            .upgrade()
                            .context("operation owner unavailable")
                            .map_err(tool_error)?;
                        let caller = context.require::<ToolContext>().map_err(tool_error)?;
                        let record = if cancel {
                            let arg: OperationArgument =
                                serde_json::from_value(args).map_err(tool_error)?;
                            owner.cancel(&id, &caller, arg.operation_id).await
                        } else {
                            owner.submit(&id, &caller, args).await
                        }
                        .map_err(tool_error)?;
                        Ok(ToolOutput::json(
                            serde_json::to_value(record).map_err(tool_error)?,
                        ))
                    })
                },
            )
            .with_liveness(move || probe.upgrade().is_some_and(|owner| !owner.is_closed()));
            Ok(ToolEntry::dynamic(
                dynamic,
                SOURCE,
                &descriptor.revision,
                true,
                None,
                json!({"adapter":descriptor,"action":if cancel{"cancel"}else{"start"}}),
            )?
            .for_user(descriptor.owner_id))
        })
        .collect()
}
