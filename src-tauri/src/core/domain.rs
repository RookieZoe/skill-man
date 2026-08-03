use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SkillId(pub String);

pub fn skill_identity_key(directory_name: &str) -> String {
    let normalized: String = directory_name.nfc().collect();
    normalized.as_str().case_fold().collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

pub fn parse_skill_metadata(skill_markdown: &str) -> SkillMetadata {
    let mut lines = skill_markdown.lines();
    if lines.next() != Some("---") {
        return SkillMetadata::default();
    }
    let mut metadata = SkillMetadata::default();
    for line in lines {
        if line == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['\'', '"']);
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => metadata.name = Some(value.to_owned()),
            "description" => metadata.description = Some(value.to_owned()),
            _ => {}
        }
    }
    metadata
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AgentId(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    Link,
    RemoteInstall,
    FileInstall,
}

impl SourceKind {
    pub fn is_install(self) -> bool {
        matches!(self, Self::RemoteInstall | Self::FileInstall)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Health {
    Healthy,
    Broken,
    Modified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentKind {
    ClaudePreset,
    CodexPreset,
    Custom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Compatibility {
    Verified,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationObservedState {
    Present,
    Missing,
    TargetMismatch,
    Dangling,
    Occupied,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CatalogFilter {
    #[default]
    All,
    Broken,
    Modified,
    Link,
    Install,
}

impl CatalogFilter {
    pub fn includes(self, skill: &SkillSummary) -> bool {
        match self {
            Self::All => true,
            Self::Broken => skill.health == Health::Broken,
            Self::Modified => skill.health == Health::Modified,
            Self::Link => skill.source_kind == SourceKind::Link,
            Self::Install => skill.source_kind.is_install(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillSummary {
    pub id: SkillId,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub source_kind: SourceKind,
    pub health: Health,
    pub enabled_agent_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDetail {
    pub summary: SkillSummary,
    pub final_entity_path: String,
    pub source_label: String,
    pub frontmatter_name: Option<String>,
    pub last_activity_at: String,
    pub skill_markdown: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentActivation {
    pub id: AgentId,
    pub name: String,
    pub kind: AgentKind,
    pub skills_path: String,
    pub detected: bool,
    pub compatibility: Compatibility,
    pub desired_enabled: bool,
    pub observed_state: ActivationObservedState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSnapshot<T> {
    pub snapshot_version: u64,
    pub items: Vec<T>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSeedAgent {
    pub id: AgentId,
    pub name: String,
    pub kind: AgentKind,
    pub skills_path: String,
    pub detected: bool,
    pub compatibility: Compatibility,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSeed {
    pub snapshot_version: u64,
    pub skills: Vec<SkillDetail>,
    pub agents: Vec<CatalogSeedAgent>,
}

#[cfg(test)]
mod tests {
    use super::skill_identity_key;

    #[test]
    fn identity_key_uses_unicode_case_folding_after_nfc() {
        assert_eq!(skill_identity_key("Straße"), skill_identity_key("STRASSE"));
        assert_eq!(
            skill_identity_key("Édit"),
            skill_identity_key("E\u{301}DIT")
        );
    }
}
