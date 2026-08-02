#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SkillId(pub String);

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
