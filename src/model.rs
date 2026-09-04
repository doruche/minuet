use std::fmt;

use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IdentifierError::Empty("provider"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelName(String);

impl ModelName {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IdentifierError::Empty("model"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelSelection {
    pub provider: ProviderId,
    pub model: ModelName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningEffort(String);

impl ReasoningEffort {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IdentifierError::Empty("reasoning effort"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Error)]
pub enum IdentifierError {
    #[error("{0} must not be empty")]
    Empty(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_effort_is_opaque() {
        let effort = ReasoningEffort::new("provider-specific-value").unwrap();
        assert_eq!(effort.as_str(), "provider-specific-value");
    }

    #[test]
    fn identifiers_reject_empty_values() {
        assert!(ProviderId::new("  ").is_err());
        assert!(ModelName::new("").is_err());
        assert!(ReasoningEffort::new("\t").is_err());
    }
}
