use thiserror::Error;

use crate::core::domain::AgentKind;

pub struct AgentEnableContext<'a> {
    pub agent_kind: AgentKind,
    pub directory_name: &'a str,
    pub frontmatter_name: Option<&'a str>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentEnablePolicy {
    pub compatibility_warning: Option<String>,
}

#[derive(Debug, Error)]
#[error("Agent compatibility validation failed: {0}")]
pub struct AgentAdapterError(pub String);

pub trait AgentAdapterRegistry: Send + Sync {
    fn validate_enable(
        &self,
        context: AgentEnableContext<'_>,
    ) -> Result<AgentEnablePolicy, AgentAdapterError>;
}
