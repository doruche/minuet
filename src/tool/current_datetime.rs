use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};

use super::{Tool, ToolDefinition, ToolError};

pub(super) struct CurrentDatetimeTool;

#[async_trait]
impl Tool for CurrentDatetimeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "current_datetime".to_owned(),
            description: "Return the current UTC date and time in RFC 3339 format.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, arguments: Value) -> Result<Value, ToolError> {
        let arguments = arguments
            .as_object()
            .ok_or_else(|| ToolError::InvalidArguments("arguments must be an object".to_owned()))?;
        if !arguments.is_empty() {
            return Err(ToolError::InvalidArguments(
                "current_datetime accepts no arguments".to_owned(),
            ));
        }
        Ok(json!({
            "datetime": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "timezone": "UTC"
        }))
    }
}
