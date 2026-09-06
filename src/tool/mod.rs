use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::{Value, json};
use thiserror::Error;

mod builtins;
mod current_datetime;
mod echo;
mod output;

pub use output::ToolOutput;
mod random_integer;

/// Metadata and parameter schema exposed by a registered tool. JSON is the
/// Minuet tool protocol's heterogeneous value representation, independent of
/// any provider's HTTP format.
#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the JSON argument value accepted by `invoke`.
    /// The tool remains the authority for complete validation.
    pub parameters: Value,
}

#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the immutable registration contract. The registry snapshots it
    /// once; subsequent calls are not used for routing or presentation.
    fn definition(&self) -> ToolDefinition;

    async fn invoke(&self, arguments: Value, output: &dyn ToolOutput) -> Result<Value, ToolError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolStatus {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ToolInvocation {
    /// JSON protocol text committed to model history by the loop.
    pub output: String,
    /// Execution status for observers; the encoded output remains authoritative
    /// for the model-facing protocol.
    pub is_error: bool,
}

pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
}

/// A turn-scoped view of the registry. Definitions and invocation use the
/// same selected set, so a turn cannot observe one policy and execute another.
pub struct ToolSnapshot {
    tools: BTreeMap<String, RegisteredToolRef>,
}

struct RegisteredToolRef {
    tool: Arc<dyn Tool>,
    definition: ToolDefinition,
}

struct RegisteredTool {
    tool: Arc<dyn Tool>,
    definition: ToolDefinition,
    enabled: bool,
}

impl ToolRegistry {
    pub fn with_builtins(enabled: &[String]) -> Result<Self, ToolRegistryError> {
        Self::new(builtins::all(), enabled)
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

    pub fn snapshot(&self, enabled: &[String]) -> Result<ToolSnapshot, ToolRegistryError> {
        let mut tools = BTreeMap::new();
        let names = enabled.to_vec();
        for name in &names {
            let registered = self
                .tools
                .get(name)
                .ok_or_else(|| ToolRegistryError::Unknown(name.clone()))?;
            tools.insert(
                name.clone(),
                RegisteredToolRef {
                    tool: Arc::clone(&registered.tool),
                    definition: registered.definition.clone(),
                },
            );
        }
        Ok(ToolSnapshot { tools })
    }

    pub fn list_for(&self, enabled: &[String]) -> Vec<ToolStatus> {
        self.tools
            .values()
            .map(|registered| ToolStatus {
                name: registered.definition.name.clone(),
                description: registered.definition.description.clone(),
                enabled: enabled
                    .iter()
                    .any(|name| name == &registered.definition.name),
            })
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

    pub async fn invoke(
        &self,
        name: &str,
        arguments: &str,
        output: &dyn ToolOutput,
    ) -> ToolInvocation {
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
        match registered.tool.invoke(arguments, output).await {
            Ok(value) => ToolInvocation {
                output: encode_result(&value),
                is_error: false,
            },
            Err(error) => invocation_error(error.kind(), error.to_string()),
        }
    }
}

impl ToolSnapshot {
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition.clone()).collect()
    }
    pub async fn invoke(
        &self,
        name: &str,
        arguments: &str,
        output: &dyn ToolOutput,
    ) -> ToolInvocation {
        let Some(registered) = self.tools.get(name) else {
            return invocation_error("disabled_tool", format!("tool `{name}` is disabled"));
        };
        let arguments = match serde_json::from_str(arguments) {
            Ok(arguments) => arguments,
            Err(error) => return invocation_error("invalid_arguments", error.to_string()),
        };
        match registered.tool.invoke(arguments, output).await {
            Ok(value) => ToolInvocation {
                output: encode_result(&value),
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
    if let Some(schema_type) = definition.parameters.get("type")
        && schema_type.as_str() != Some("object")
    {
        return Err(ToolRegistryError::InvalidDefinition(format!(
            "tool `{}` parameters schema must describe an object",
            definition.name
        )));
    }
    Ok(())
}

fn invocation_error(kind: &str, message: String) -> ToolInvocation {
    ToolInvocation {
        output: encode_error(kind, message),
        is_error: true,
    }
}

/// Encodes the tool protocol result at the agent/session boundary. Keeping
/// this here prevents each caller (including skipped calls) from inventing a
/// different JSON envelope.
pub(crate) fn encode_result(value: &Value) -> String {
    value.to_string()
}

pub(crate) fn encode_error(kind: &str, message: impl Into<String>) -> String {
    json!({"error":{"kind":kind,"message":message.into()}}).to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;

    fn enabled(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[tokio::test]
    async fn exposes_only_enabled_tools() {
        let registry = ToolRegistry::with_builtins(&enabled(&["echo"])).unwrap();
        assert_eq!(registry.definitions().len(), 1);
        assert_eq!(registry.definitions()[0].name, "echo");

        let disabled = registry
            .invoke("random_integer", r#"{"min":1,"max":1}"#, &())
            .await;
        assert!(disabled.is_error);
        assert!(disabled.output.contains("disabled_tool"));
    }

    #[tokio::test]
    async fn echo_returns_structured_output() {
        let registry = ToolRegistry::with_builtins(&enabled(&["echo"])).unwrap();
        let invocation = registry.invoke("echo", r#"{"text":"hello"}"#, &()).await;
        assert!(!invocation.is_error);
        assert_eq!(
            serde_json::from_str::<Value>(&invocation.output).unwrap(),
            json!({"text":"hello"})
        );
    }

    #[test]
    fn registration_definition_is_the_routing_and_exposure_snapshot() {
        struct SnapshotTool;
        #[async_trait]
        impl Tool for SnapshotTool {
            fn definition(&self) -> ToolDefinition {
                ToolDefinition {
                    name: "snapshot".into(),
                    description: "fixed".into(),
                    parameters: json!({"type":"object"}),
                }
            }

            async fn invoke(
                &self,
                _arguments: Value,
                _output: &dyn ToolOutput,
            ) -> Result<Value, ToolError> {
                Ok(json!({"ok": true}))
            }
        }

        let registry = ToolRegistry::new(
            [Arc::new(SnapshotTool) as Arc<dyn Tool>],
            &["snapshot".into()],
        )
        .unwrap();
        assert_eq!(registry.list()[0].name, "snapshot");
        assert_eq!(registry.definitions()[0].description, "fixed");
    }

    #[tokio::test]
    async fn reports_invalid_random_ranges_to_the_model() {
        let registry = ToolRegistry::with_builtins(&enabled(&["random_integer"])).unwrap();
        let invocation = registry
            .invoke("random_integer", r#"{"min":4,"max":2}"#, &())
            .await;
        assert!(invocation.is_error);
        assert!(invocation.output.contains("invalid_arguments"));
    }

    #[test]
    fn result_and_error_encoding_are_json_protocol_values() {
        assert_eq!(encode_result(&json!({"value": 3})), r#"{"value":3}"#);
        assert_eq!(
            encode_error("execution_error", "failed"),
            r#"{"error":{"kind":"execution_error","message":"failed"}}"#
        );
    }

    #[test]
    fn rejects_non_object_parameter_schema_types() {
        let definition = ToolDefinition {
            name: "bad_schema".into(),
            description: "invalid".into(),
            parameters: json!({"type": "string"}),
        };
        assert!(matches!(
            validate_definition(&definition),
            Err(ToolRegistryError::InvalidDefinition(message))
                if message.contains("must describe an object")
        ));
    }
}
