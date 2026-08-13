use crate::core::domain::{AgentKind, skill_identity_key};
use crate::seams::agent_adapter::{
    AgentAdapterError, AgentAdapterRegistry, AgentEnableContext, AgentEnablePolicy,
    CompatibilityWarning,
};

pub struct BuiltInAgentAdapters;

impl AgentAdapterRegistry for BuiltInAgentAdapters {
    fn validate_enable(
        &self,
        context: AgentEnableContext<'_>,
    ) -> Result<AgentEnablePolicy, AgentAdapterError> {
        match context.agent_kind {
            AgentKind::ClaudePreset => Ok(AgentEnablePolicy::default()),
            AgentKind::CodexPreset => {
                if context.directory_name == ".system" {
                    return Err(AgentAdapterError(
                        "Codex reserves the .system directory".into(),
                    ));
                }
                let compatibility_warning = context
                    .frontmatter_name
                    .filter(|name| {
                        skill_identity_key(name) != skill_identity_key(context.directory_name)
                    })
                    .map(|name| CompatibilityWarning::FrontmatterMismatch {
                        frontmatter_name: name.to_owned(),
                        directory_name: context.directory_name.to_owned(),
                    });
                Ok(AgentEnablePolicy {
                    compatibility_warning,
                })
            }
            AgentKind::Custom => Ok(AgentEnablePolicy {
                compatibility_warning: Some(CompatibilityWarning::CustomUnknown),
            }),
        }
    }
}
