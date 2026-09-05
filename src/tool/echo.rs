use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, ToolDefinition, ToolError};

pub(super) struct EchoTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EchoArguments {
    text: String,
}

#[async_trait]
impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "echo".to_owned(),
            description: "Return the provided text unchanged.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(
        &self,
        arguments: Value,
        _output: &dyn super::ToolOutput,
    ) -> Result<Value, ToolError> {
        let arguments: EchoArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        Ok(json!({"text": arguments.text}))
    }
}
