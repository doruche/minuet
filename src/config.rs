use std::{collections::BTreeMap, env, fs, io, path::PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::model::{IdentifierError, ModelName, ModelSelection, ProviderId, ReasoningEffort};

const HOME_ENV: &str = "MINUET_HOME";
const CONFIG_FILE: &str = "config.toml";

#[derive(Clone, Debug)]
pub struct Config {
    pub home: PathBuf,
    pub model: ModelSelection,
    pub default_reasoning_effort: Option<ReasoningEffort>,
    pub loop_max_steps: usize,
    pub enabled_tools: Vec<String>,
    pub provider: ProviderConfig,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key_env: String,
}

impl ProviderConfig {
    pub fn api_key(&self) -> Result<String, ConfigError> {
        env::var(&self.api_key_env).map_err(|_| ConfigError::ApiKeyNotSet(self.api_key_env.clone()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Protocol {
    OpenAiResponses,
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let configured_home = env::var_os(HOME_ENV).ok_or(ConfigError::HomeNotSet)?;
        if configured_home.is_empty() {
            return Err(ConfigError::HomeNotSet);
        }

        let configured_home = PathBuf::from(configured_home);
        let home = if configured_home.is_absolute() {
            configured_home
        } else {
            env::current_dir()
                .map_err(ConfigError::CurrentDirectory)?
                .join(configured_home)
        };

        Self::load_from_home(home)
    }

    pub fn load_from_home(home: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let home = home.into();
        let path = home.join(CONFIG_FILE);
        let contents = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let raw: RawConfig = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        Self::from_raw(home, raw)
    }

    fn from_raw(home: PathBuf, raw: RawConfig) -> Result<Self, ConfigError> {
        let provider_id = ProviderId::new(raw.model_provider)?;
        let model = ModelName::new(raw.model)?;
        let default_reasoning_effort = raw
            .model_reasoning_effort
            .map(ReasoningEffort::new)
            .transpose()?;
        if raw.loop_config.max_steps == 0 {
            return Err(ConfigError::ZeroMaxSteps);
        }

        let raw_provider = raw
            .model_providers
            .get(provider_id.as_str())
            .ok_or_else(|| ConfigError::ProviderNotFound(provider_id.to_string()))?;
        let protocol = match raw_provider.protocol.as_str() {
            "openai-responses" => Protocol::OpenAiResponses,
            other => return Err(ConfigError::UnsupportedProtocol(other.to_owned())),
        };
        ensure_non_empty("provider base_url", &raw_provider.base_url)?;
        ensure_non_empty("provider api_key_env", &raw_provider.api_key_env)?;
        for tool in &raw.tools.enabled {
            ensure_non_empty("enabled tool name", tool)?;
        }

        Ok(Self {
            home,
            model: ModelSelection {
                provider: provider_id,
                model,
            },
            default_reasoning_effort,
            loop_max_steps: raw.loop_config.max_steps,
            enabled_tools: raw.tools.enabled,
            provider: ProviderConfig {
                protocol,
                base_url: raw_provider.base_url.clone(),
                api_key_env: raw_provider.api_key_env.clone(),
            },
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.home.join(CONFIG_FILE)
    }
}

fn ensure_non_empty(field: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::EmptyField(field));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    model: String,
    model_provider: String,
    model_reasoning_effort: Option<String>,
    #[serde(rename = "loop", default)]
    loop_config: RawLoopConfig,
    #[serde(default)]
    tools: RawToolsConfig,
    model_providers: BTreeMap<String, RawProviderConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLoopConfig {
    max_steps: usize,
}

impl Default for RawLoopConfig {
    fn default() -> Self {
        Self { max_steps: 8 }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawToolsConfig {
    enabled: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProviderConfig {
    protocol: String,
    base_url: String,
    api_key_env: String,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{HOME_ENV} must point to the Minuet data directory")]
    HomeNotSet,
    #[error("failed to resolve the current directory: {0}")]
    CurrentDirectory(io::Error),
    #[error("failed to read configuration at {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to parse configuration at {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error(transparent)]
    Identifier(#[from] IdentifierError),
    #[error("model provider `{0}` has no configuration")]
    ProviderNotFound(String),
    #[error("unsupported protocol `{0}`")]
    UnsupportedProtocol(String),
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("API key environment variable `{0}` is not set")]
    ApiKeyNotSet(String),
    #[error("loop.max_steps must be greater than zero")]
    ZeroMaxSteps,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> Result<Config, ConfigError> {
        let raw = toml::from_str(contents).unwrap();
        Config::from_raw(PathBuf::from("/tmp/minuet-test"), raw)
    }

    #[test]
    fn parses_minimal_configuration() {
        let config = parse(
            r#"
                model = "MiniMax-M3"
                model_provider = "minimax"

                [model_providers.minimax]
                protocol = "openai-responses"
                base_url = "https://api.minimax.cn/v1"
                api_key_env = "MINIMAX_API_KEY"
            "#,
        )
        .unwrap();

        assert_eq!(config.model.provider.as_str(), "minimax");
        assert_eq!(config.model.model.as_str(), "MiniMax-M3");
        assert_eq!(config.loop_max_steps, 8);
        assert!(config.enabled_tools.is_empty());
        assert_eq!(config.provider.protocol, Protocol::OpenAiResponses);
    }

    #[test]
    fn keeps_reasoning_effort_opaque() {
        let config = parse(
            r#"
                model = "some-model"
                model_provider = "private"
                model_reasoning_effort = "vendor-level-7"

                [loop]
                max_steps = 3

                [tools]
                enabled = ["echo"]

                [model_providers.private]
                protocol = "openai-responses"
                base_url = "https://example.test/v1"
                api_key_env = "PRIVATE_KEY"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.default_reasoning_effort.unwrap().as_str(),
            "vendor-level-7"
        );
        assert_eq!(config.loop_max_steps, 3);
        assert_eq!(config.enabled_tools, ["echo"]);
    }

    #[test]
    fn rejects_unknown_protocols() {
        let error = parse(
            r#"
                model = "model"
                model_provider = "provider"

                [model_providers.provider]
                protocol = "mystery"
                base_url = "https://example.test/v1"
                api_key_env = "API_KEY"
            "#,
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::UnsupportedProtocol(_)));
    }
}
