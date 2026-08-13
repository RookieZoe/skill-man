use thiserror::Error;

use crate::core::domain::AgentKind;

pub struct AgentEnableContext<'a> {
    pub agent_kind: AgentKind,
    pub directory_name: &'a str,
    pub frontmatter_name: Option<&'a str>,
}

/// Closed compatibility warnings (spec §4.7): never a free App Copy string —
/// presentation composes the message from the typed variant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompatibilityWarning {
    /// Custom Agent kinds have no declared compatibility contract.
    CustomUnknown,
    /// The frontmatter name differs from the directory identity.
    FrontmatterMismatch {
        frontmatter_name: String,
        directory_name: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentEnablePolicy {
    pub compatibility_warning: Option<CompatibilityWarning>,
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
