use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};

use super::{Tool, ToolDefinition, ToolError};

pub(super) struct CurrentDatetimeTool;

#[async_trait]
impl Tool for CurrentDatetimeTool {
    fn display_arguments(&self, arguments: &Value) -> String {
        if arguments.as_object().is_some_and(|args| args.is_empty()) {
            String::new()
        } else {
            "invalid arguments".into()
        }
    }

    fn display_result(&self, result: &Value) -> String {
        let datetime = result["datetime"]
            .as_str()
            .expect("datetime result contains timestamp");
        chrono::DateTime::parse_from_rfc3339(datetime)
            .expect("datetime result is RFC 3339")
            .format("%Y-%m-%d %H:%M:%S%.3f UTC")
            .to_string()
    }

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

    async fn invoke(
        &self,
        arguments: Value,
        _output: &dyn super::ToolOutput,
    ) -> Result<Value, ToolError> {
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
