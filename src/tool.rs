use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use rand::Rng;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    async fn invoke(&self, arguments: Value) -> Result<Value, ToolError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolStatus {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ToolInvocation {
    pub output: String,
    pub is_error: bool,
}

pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
}

struct RegisteredTool {
    tool: Arc<dyn Tool>,
    definition: ToolDefinition,
    enabled: bool,
}

impl ToolRegistry {
    pub fn with_builtins(enabled: &[String]) -> Result<Self, ToolRegistryError> {
        Self::new(
            [
                Arc::new(EchoTool) as Arc<dyn Tool>,
                Arc::new(RandomIntegerTool),
                Arc::new(CurrentDatetimeTool),
            ],
            enabled,
        )
    }

    pub fn new(
        tools: impl IntoIterator<Item = Arc<dyn Tool>>,
        enabled: &[String],
    ) -> Result<Self, ToolRegistryError> {
        let mut registry = Self {
            tools: BTreeMap::new(),
        };
        for tool in tools {
            let definition = tool.definition();
            validate_definition(&definition)?;
            let name = definition.name.clone();
            if registry
                .tools
                .insert(
                    name.clone(),
                    RegisteredTool {
                        tool,
                        definition,
                        enabled: false,
                    },
                )
                .is_some()
            {
                return Err(ToolRegistryError::Duplicate(name));
            }
        }
        for name in enabled {
            registry.set_enabled(name, true)?;
        }
        Ok(registry)
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .filter(|registered| registered.enabled)
            .map(|registered| registered.definition.clone())
            .collect()
    }

    pub fn list(&self) -> Vec<ToolStatus> {
        self.tools
            .values()
            .map(|registered| ToolStatus {
                name: registered.definition.name.clone(),
                description: registered.definition.description.clone(),
                enabled: registered.enabled,
            })
            .collect()
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<(), ToolRegistryError> {
        let registered = self
            .tools
            .get_mut(name)
            .ok_or_else(|| ToolRegistryError::Unknown(name.to_owned()))?;
        registered.enabled = enabled;
        Ok(())
    }

    pub async fn invoke(&self, name: &str, arguments: &str) -> ToolInvocation {
        let Some(registered) = self.tools.get(name) else {
            return invocation_error("unknown_tool", format!("unknown tool `{name}`"));
        };
        if !registered.enabled {
            return invocation_error("disabled_tool", format!("tool `{name}` is disabled"));
        }
        let arguments = match serde_json::from_str(arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                return invocation_error("invalid_arguments", error.to_string());
            },
        };
        match registered.tool.invoke(arguments).await {
            Ok(value) => ToolInvocation {
                output: value.to_string(),
                is_error: false,
            },
            Err(error) => invocation_error(error.kind(), error.to_string()),
        }
    }
}

fn validate_definition(definition: &ToolDefinition) -> Result<(), ToolRegistryError> {
    if definition.name.trim().is_empty() {
        return Err(ToolRegistryError::InvalidDefinition(
            "tool name must not be empty".to_owned(),
        ));
    }
    if !definition.parameters.is_object() {
        return Err(ToolRegistryError::InvalidDefinition(format!(
            "tool `{}` parameters must be a JSON object schema",
            definition.name
        )));
    }
    Ok(())
}

fn invocation_error(kind: &str, message: String) -> ToolInvocation {
    ToolInvocation {
        output: json!({"error":{"kind":kind,"message":message}}).to_string(),
        is_error: true,
    }
}

#[derive(Debug, Error)]
pub enum ToolRegistryError {
    #[error("duplicate tool `{0}`")]
    Duplicate(String),
    #[error("unknown tool `{0}`")]
    Unknown(String),
    #[error("invalid tool definition: {0}")]
    InvalidDefinition(String),
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
}

impl ToolError {
    fn kind(&self) -> &'static str {
        match self {
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::Execution(_) => "execution_error",
        }
    }
}

struct EchoTool;

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

    async fn invoke(&self, arguments: Value) -> Result<Value, ToolError> {
        let arguments: EchoArguments = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        Ok(json!({"text": arguments.text}))
    }
}

struct RandomIntegerTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RandomIntegerArguments {
    min: i64,
    max: i64,
}

#[async_trait]
impl Tool for RandomIntegerTool {
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

    async fn invoke(&self, arguments: Value) -> Result<Value, ToolError> {
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

struct CurrentDatetimeTool;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[tokio::test]
    async fn exposes_only_enabled_tools() {
        let registry = ToolRegistry::with_builtins(&enabled(&["echo"])).unwrap();
        assert_eq!(registry.definitions().len(), 1);
        assert_eq!(registry.definitions()[0].name, "echo");

        let disabled = registry
            .invoke("random_integer", r#"{"min":1,"max":1}"#)
            .await;
        assert!(disabled.is_error);
        assert!(disabled.output.contains("disabled_tool"));
    }

    #[tokio::test]
    async fn echo_returns_structured_output() {
        let registry = ToolRegistry::with_builtins(&enabled(&["echo"])).unwrap();
        let invocation = registry.invoke("echo", r#"{"text":"hello"}"#).await;
        assert!(!invocation.is_error);
        assert_eq!(
            serde_json::from_str::<Value>(&invocation.output).unwrap(),
            json!({"text":"hello"})
        );
    }

    #[tokio::test]
    async fn reports_invalid_random_ranges_to_the_model() {
        let registry = ToolRegistry::with_builtins(&enabled(&["random_integer"])).unwrap();
        let invocation = registry
            .invoke("random_integer", r#"{"min":4,"max":2}"#)
            .await;
        assert!(invocation.is_error);
        assert!(invocation.output.contains("invalid_arguments"));
    }
}
