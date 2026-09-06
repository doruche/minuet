use async_trait::async_trait;
use rand::Rng;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, ToolDefinition, ToolError};

pub(super) struct RandomIntegerTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RandomIntegerArguments {
    min: i64,
    max: i64,
}

#[async_trait]
impl Tool for RandomIntegerTool {
    fn display_arguments(&self, arguments: &Value) -> String {
        match serde_json::from_value::<RandomIntegerArguments>(arguments.clone()) {
            Ok(arguments) => format!("{}, {}", arguments.min, arguments.max),
            Err(_) => "invalid arguments".into(),
        }
    }

    fn display_result(&self, result: &Value) -> String {
        result["value"]
            .as_i64()
            .expect("random result contains integer")
            .to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "random_integer".to_owned(),
            description: "Generate a uniformly distributed integer in an inclusive range."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "min": {"type": "integer"},
                    "max": {"type": "integer"}
                },
                "required": ["min", "max"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(
        &self,
        arguments: Value,
        _output: &dyn super::ToolOutput,
    ) -> Result<Value, ToolError> {
        let arguments: RandomIntegerArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        if arguments.min > arguments.max {
            return Err(ToolError::InvalidArguments(
                "min must be less than or equal to max".to_owned(),
            ));
        }
        let value = rand::rng().random_range(arguments.min..=arguments.max);
        Ok(json!({"value": value}))
    }
}
