//! One executable catalog entry. Rig owns tool dispatch and presentation;
//! Alia owns admission, identity and durable execution evidence.
use std::sync::Arc;

use rig_core::tool::{
    ContextValue, DynamicTool, IntoToolOutput, Tool, ToolExecutionError, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::adapter::{ToolContext, ToolResult};

pub fn compile_schema(schema: &Value) -> anyhow::Result<CompiledSchema> {
    anyhow::ensure!(
        serde_json::to_vec(schema)?.len() <= MAX_TOOL_SCHEMA_BYTES,
        "schema byte budget"
    );
    check_json_depth(schema, 0)?;
    if let Some(dialect) = schema.get("$schema") {
        anyhow::ensure!(
            dialect.as_str() == Some("https://json-schema.org/draft/2020-12/schema"),
            "unsupported tool schema dialect"
        );
    }
    let mut expanded_nodes = 0usize;
    schema_budget(schema, schema, &mut Vec::new(), &mut expanded_nodes)?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_pattern_options(
            jsonschema::PatternOptions::regex()
                .size_limit(256 * 1024)
                .dfa_size_limit(256 * 1024),
        )
        .offline()
        .build(schema)?;
    Ok(CompiledSchema {
        validator,
        expanded_nodes,
    })
}

/// jsonschema owns validation semantics. Admission restricts the graph before
/// compilation; the crate has no configurable total evaluation fuel.
pub struct CompiledSchema {
    validator: jsonschema::Validator,
    expanded_nodes: usize,
}
impl CompiledSchema {
    pub fn validate(&self, value: &Value) -> anyhow::Result<()> {
        check_json_depth(value, 0)?;
        let bytes = serde_json::to_vec(value)?.len().max(1);
        anyhow::ensure!(
            self.expanded_nodes.saturating_mul(bytes) <= 4 * 1024 * 1024,
            "tool schema evaluation work budget exceeded"
        );
        self.validator
            .validate(value)
            .map_err(|e| anyhow::anyhow!("invalid tool arguments: {e}"))
    }
    #[cfg(test)]
    fn is_valid(&self, value: &Value) -> bool {
        self.validate(value).is_ok()
    }
}

fn schema_budget(
    root: &Value,
    value: &Value,
    stack: &mut Vec<usize>,
    nodes: &mut usize,
) -> anyhow::Result<()> {
    *nodes += 1;
    anyhow::ensure!(
        *nodes <= 2048 && stack.len() < 64,
        "tool schema expanded graph budget exceeded"
    );
    let identity = value as *const Value as usize;
    anyhow::ensure!(
        !stack.contains(&identity),
        "recursive tool schemas are unsupported"
    );
    stack.push(identity);
    match value {
        Value::Object(map) => {
            for key in [
                "$id",
                "$anchor",
                "$dynamicRef",
                "$dynamicAnchor",
                "$recursiveRef",
                "$recursiveAnchor",
            ] {
                anyhow::ensure!(
                    !map.contains_key(key),
                    "tool schema keyword {key} is unsupported; use acyclic local JSON Pointer references"
                );
            }
            if let Some(reference) = map.get("$ref") {
                let reference = reference
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("schema $ref must be a string"))?;
                anyhow::ensure!(
                    reference == "#" || reference.starts_with("#/"),
                    "only local JSON Pointer schema references are supported"
                );
                let target = root
                    .pointer(&reference[1..])
                    .ok_or_else(|| anyhow::anyhow!("unresolved local schema reference"))?;
                anyhow::ensure!(
                    target.is_object() || target.is_boolean(),
                    "schema reference target must be a schema"
                );
                schema_budget(root, target, stack, nodes)?;
            }
            for child in map.values() {
                schema_budget(root, child, stack, nodes)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                schema_budget(root, child, stack, nodes)?;
            }
        }
        _ => {}
    }
    stack.pop();
    Ok(())
}

pub const MAX_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
pub const MAX_TOOL_SCHEMA_BYTES: usize = 128 * 1024;

/// Provider definition plus host policy. There is only one copy of the schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(flatten)]
    pub function: rig_core::completion::ToolDefinition,
    #[serde(default)]
    pub routing_hints: Vec<String>,
    pub source: String,
    pub require_confirmation: bool,
    pub required_role: Option<String>,
}

impl std::ops::Deref for ToolDefinition {
    type Target = rig_core::completion::ToolDefinition;
    fn deref(&self) -> &Self::Target {
        &self.function
    }
}

impl std::ops::DerefMut for ToolDefinition {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.function
    }
}

impl ToolDefinition {
    pub fn new(
        name: &str,
        description: impl Into<String>,
        parameters: Value,
        source: impl Into<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            function: rig_core::completion::ToolDefinition::new(
                rig_core::message::ToolName::new(name)?,
                description,
                parameters,
            ),
            routing_hints: Vec::new(),
            source: source.into(),
            require_confirmation: false,
            required_role: None,
        })
    }
    pub fn to_rig(&self) -> rig_core::completion::ToolDefinition {
        self.function.clone()
    }
}

impl ContextValue for ToolContext {
    const KEY: &'static str = "alia.invocation.v1";
}
impl ContextValue for ToolResult {
    const KEY: &'static str = "alia.receipt.v1";
}

impl ToolResult {
    /// Host receipt metadata is separate from Rig's model-visible content.
    pub fn publish(
        self,
        context: &mut rig_core::tool::ToolContext,
    ) -> Result<ToolOutput, ToolExecutionError> {
        let output = self.to_rig_output();
        context.insert_result(self).map_err(tool_error)?;
        Ok(output)
    }

    pub fn to_rig_output(&self) -> ToolOutput {
        self.presentation
            .clone()
            .unwrap_or_else(|| ToolOutput::json(self.output.clone()))
    }

    /// Call only with the canonical Execution record loaded by the host. Foreign
    /// tool JSON is never passed here. Preserve status evidence beside rich data.
    pub fn canonical_presentation(record: &Value) -> Option<ToolOutput> {
        if record.get("status")?.as_str()? != "succeeded"
            || record.get("completion")?.get("status")?.as_str()? != "succeeded"
        {
            return None;
        }
        let receipt: Self =
            serde_json::from_value(record.get("completion")?.get("receipt")?.clone()).ok()?;
        if !receipt.success || record.get("model_call_id")?.as_str()? != receipt.call_id {
            return None;
        }
        let rich = receipt.presentation?;
        let mut evidence = record.clone();
        // The complete original receipt remains in the ledger. Do not duplicate
        // images/base64 in the JSON evidence block sent to the model.
        evidence["completion"]["receipt"]
            .as_object_mut()?
            .remove("presentation");
        let mut content = ToolOutput::json(evidence).into_content();
        content.extend(rich.into_content());
        ToolOutput::content(content).ok()
    }
}

pub fn tool_error(error: impl std::fmt::Display) -> ToolExecutionError {
    ToolExecutionError::other(error.to_string())
}

pub fn parameters<T: schemars::JsonSchema>() -> Value {
    serde_json::to_value(
        schemars::generate::SchemaSettings::draft2020_12()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .expect("schema serializes")
}

pub struct ToolEntry {
    definition: ToolDefinition,
    tool: DynamicTool,
    validator: CompiledSchema,
    revision: String,
    principal: Option<uuid::Uuid>,
    definitive_rejection: Option<Arc<dyn Fn(&ToolResult) -> bool + Send + Sync>>,
    /// Optional extra metadata included in the immutable implementation identity.
    pub identity: Value,
}

impl ToolEntry {
    pub fn from_tool<T: Tool + 'static>(
        tool: T,
        source: impl Into<String>,
        revision: impl Into<String>,
        require_confirmation: bool,
        required_role: Option<String>,
    ) -> anyhow::Result<Arc<Self>> {
        let definition =
            ToolDefinition::new(T::NAME, tool.description(), tool.parameters(), source)?;
        let tool = Arc::new(tool);
        let dynamic = DynamicTool::new_with_context(
            definition.name.clone(),
            definition.description.clone(),
            definition.parameters.clone(),
            move |context, args| {
                let tool = tool.clone();
                Box::pin(async move {
                    let args = serde_json::from_value::<T::Args>(args)
                        .map_err(|e| ToolExecutionError::invalid_args(e.to_string()))?;
                    let output = tool
                        .call(context, args)
                        .await
                        .map_err(|e| tool.map_error(e))?;
                    output.into_tool_output()
                })
            },
        );
        Self::dynamic(
            dynamic,
            definition.source,
            revision,
            require_confirmation,
            required_role,
            Value::Null,
        )
    }

    pub fn dynamic(
        tool: DynamicTool,
        source: impl Into<String>,
        revision: impl Into<String>,
        require_confirmation: bool,
        required_role: Option<String>,
        identity: Value,
    ) -> anyhow::Result<Arc<Self>> {
        let revision = revision.into();
        anyhow::ensure!(
            !revision.trim().is_empty(),
            "tool implementation revision is required"
        );
        let function = tool.definition();
        anyhow::ensure!(
            function.name.len() <= 64
                && function
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "tool name must use 1..64 portable ASCII characters"
        );
        anyhow::ensure!(
            serde_json::to_vec(&function)?.len() <= MAX_TOOL_SCHEMA_BYTES,
            "tool definition exceeds byte budget"
        );
        check_json_depth(&function.parameters, 0)?;
        anyhow::ensure!(
            function.parameters.is_object(),
            "tool schema must be an object"
        );
        let validator = compile_schema(&function.parameters)?;
        Ok(Arc::new(Self {
            definition: ToolDefinition {
                function,
                routing_hints: Vec::new(),
                source: source.into(),
                require_confirmation,
                required_role,
            },
            tool,
            validator,
            revision,
            principal: None,
            definitive_rejection: None,
            identity,
        }))
    }

    pub fn with_definitive_rejection(
        mut self: Arc<Self>,
        policy: impl Fn(&ToolResult) -> bool + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("unpublished tool entry")
            .definitive_rejection = Some(Arc::new(policy));
        self
    }
    pub fn with_routing_hints(mut self: Arc<Self>, hints: Vec<String>) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("unpublished tool entry")
            .definition
            .routing_hints = hints;
        self
    }
    pub fn principal(&self) -> Option<uuid::Uuid> {
        self.principal
    }
    pub fn allows_user(&self, user: uuid::Uuid) -> bool {
        self.principal.is_none_or(|id| id == user)
    }
    pub fn for_user(mut self: Arc<Self>, principal: uuid::Uuid) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("unpublished tool entry")
            .principal = Some(principal);
        self
    }
    pub fn authorize(&self, context: &ToolContext) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.principal.is_none_or(|id| id == context.user_id),
            "tool credentials belong to another principal"
        );
        Ok(())
    }
    pub fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    pub fn implementation_revision(&self) -> &str {
        &self.revision
    }
    pub fn is_live(&self) -> bool {
        self.tool.is_live()
    }
    pub fn validate(&self, arguments: &Value) -> anyhow::Result<()> {
        anyhow::ensure!(self.is_live(), "tool owner is unavailable");
        anyhow::ensure!(arguments.is_object(), "tool arguments must be an object");
        anyhow::ensure!(
            serde_json::to_vec(arguments)?.len() <= MAX_TOOL_ARGUMENT_BYTES,
            "tool arguments exceed byte budget"
        );
        check_json_depth(arguments, 0)?;
        self.validator
            .validate(arguments)
            .map_err(|e| anyhow::anyhow!("invalid tool arguments: {e}"))
    }

    pub(crate) async fn execute(
        &self,
        arguments: Value,
        context: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        self.authorize(context)?;
        self.validate(&arguments)?;
        let mut rig_context = rig_core::tool::ToolContext::new();
        rig_context.insert(context.clone())?;
        let output = self.tool.execute_with(&mut rig_context, arguments).await?;
        if let Some(receipt) = rig_context.result::<ToolResult>()? {
            return Ok(receipt);
        }
        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: output.as_json().cloned().unwrap_or(Value::Null),
            display: output.as_text().map(str::to_owned),
            presentation: output.as_json().is_none().then_some(output),
        })
    }

    /// A remote/domain failure can follow partial effects. Only pre-dispatch
    /// validation failures are definitive, and those never call this method.
    pub fn failure_is_definitive(&self, result: &ToolResult) -> bool {
        self.definitive_rejection
            .as_ref()
            .is_some_and(|policy| policy(result))
    }
}

fn check_json_depth(value: &Value, depth: usize) -> anyhow::Result<()> {
    anyhow::ensure!(depth <= 32, "tool JSON nesting exceeds budget");
    match value {
        Value::Array(values) => {
            for value in values {
                check_json_depth(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                check_json_depth(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn schema_uses_one_dto_and_offline_full_validation() {
        #[derive(schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct Args {
            #[schemars(range(min = 1, max = 3))]
            count: u32,
        }
        let validator = compile_schema(&parameters::<Args>()).unwrap();
        assert!(validator.is_valid(&json!({"count":2})));
        assert!(!validator.is_valid(&json!({"count":4})));
        assert!(!validator.is_valid(&json!({"count":2,"extra":true})));
        assert!(!validator.is_valid(&Value::Null));
        assert!(compile_schema(&json!({"$ref":"https://invalid.invalid/schema"})).is_err());
        assert!(
            compile_schema(&json!({"$schema":"http://json-schema.org/draft-07/schema#"})).is_err()
        );
    }
    #[test]
    fn schema_rejects_cycles_and_exponential_reference_expansion_before_compile() {
        assert!(compile_schema(&json!({"$ref":"#"})).is_err());
        assert!(compile_schema(&json!({"$defs":{"A":{"allOf":[{"$ref":"#/$defs/B"}]} ,"B":{"$ref":"#/$defs/A"}},"$ref":"#/$defs/A"})).is_err());
        assert!(compile_schema(&json!({"$dynamicRef":"#node","$dynamicAnchor":"node"})).is_err());
        let mut defs = serde_json::Map::new();
        defs.insert("n0".into(), json!({"type":"string"}));
        for i in 1..13 {
            defs.insert(format!("n{i}"),json!({"allOf":[{"$ref":format!("#/$defs/n{}",i-1)},{"$ref":format!("#/$defs/n{}",i-1)}]}));
        }
        assert!(compile_schema(&json!({"$defs":defs,"$ref":"#/$defs/n12"})).is_err());
        let valid=compile_schema(&json!({"$defs":{"name":{"type":"string","minLength":1}},"type":"object","properties":{"name":{"$ref":"#/$defs/name"}},"required":["name"]})).unwrap();
        assert!(valid.is_valid(&json!({"name":"Alia"})));
        assert!(!valid.is_valid(&json!({"name":""})));
        let costly = compile_schema(&json!({"allOf":vec![json!({"type":"string"});64]})).unwrap();
        assert!(costly.is_valid(&json!("small")));
        assert!(
            costly
                .validate(&json!("x".repeat(64 * 1024)))
                .unwrap_err()
                .to_string()
                .contains("work budget")
        );
    }
    #[test]
    fn foreign_json_cannot_claim_rich_content_and_unknown_cannot_unwrap_receipt() {
        let receipt = ToolResult {
            call_id: "call-1".into(),
            success: true,
            output: json!({"alia_content_version":1,"content":"pretend host data"}),
            presentation: None,
            display: None,
        };
        assert_eq!(receipt.to_rig_output().as_json(), Some(&receipt.output));
        let mut host = receipt;
        host.presentation = Some(ToolOutput::text("foreign text"));
        let mut record = json!({"status":"succeeded","model_call_id":"call-1","completion":{"status":"succeeded","receipt":host}});
        assert_eq!(
            ToolResult::canonical_presentation(&record)
                .unwrap()
                .as_content()
                .len(),
            2
        );
        record["status"] = json!("unknown");
        assert!(ToolResult::canonical_presentation(&record).is_none());
        record["status"] = json!("succeeded");
        record["model_call_id"] = json!("other");
        assert!(ToolResult::canonical_presentation(&record).is_none());
    }
}
