//! Read-only facts for the Git Repository Source capability scan.
//!
//! The adapter owns SQLite introspection and manifest reading.  It returns
//! facts only: Core owns the classification into the user-visible source
//! states.  Neither side has a write method, so a scan cannot migrate, seed
//! or repair a Catalog.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceCatalogStructure {
    pub has_repository_sources_table: bool,
    pub has_releases_table: bool,
    pub has_release_members_table: bool,
    pub has_members_table: bool,
    pub has_required_columns: bool,
    pub has_required_foreign_keys: bool,
    pub has_required_unique_constraints: bool,
    pub has_clean_foreign_key_check: bool,
    pub has_clean_integrity_check: bool,
}

impl GitSourceCatalogStructure {
    pub fn unsupported() -> Self {
        Self {
            has_repository_sources_table: false,
            has_releases_table: false,
            has_release_members_table: false,
            has_members_table: false,
            has_required_columns: false,
            has_required_foreign_keys: false,
            has_required_unique_constraints: false,
            has_clean_foreign_key_check: false,
            has_clean_integrity_check: false,
        }
    }

    pub fn supports_repository_sources(&self) -> bool {
        self.has_repository_sources_table
            && self.has_releases_table
            && self.has_release_members_table
            && self.has_members_table
            && self.has_required_columns
            && self.has_required_foreign_keys
            && self.has_required_unique_constraints
            && self.has_clean_foreign_key_check
            && self.has_clean_integrity_check
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceReleaseFact {
    pub release_id: String,
    pub remote_id: String,
    pub selection_kind: String,
    pub selected_ref: String,
    pub resolved_commit: String,
    pub member_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceMemberFact {
    pub skill_id: String,
    pub skill_path: String,
    pub storage_relpath: String,
    pub presence: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitRepositorySourceFact {
    pub provider: Option<String>,
    pub canonical_url: String,
    pub tracking_mode: Option<String>,
    pub tracking_value: Option<String>,
    pub current_selected_ref: Option<String>,
    pub current_release_id: Option<String>,
    pub current_release: Option<GitSourceReleaseFact>,
    pub current_members: Vec<GitSourceMemberFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitSourceManifestFact {
    Missing,
    Unreadable,
    Present {
        remote_id: String,
        canonical_url: String,
        aliases: Vec<String>,
        provider: Option<String>,
        tracking_mode: Option<String>,
        tracking_value: Option<String>,
        current_selected_ref: Option<String>,
        current_release_id: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceFact {
    pub remote_id: String,
    pub canonical_url: String,
    pub catalog_aliases: Vec<String>,
    pub repository: Option<GitRepositorySourceFact>,
    pub manifest: GitSourceManifestFact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceCapabilityFacts {
    pub catalog_structure: GitSourceCatalogStructure,
    pub sources: Vec<GitSourceFact>,
}

/// Seam for the only variable external facts of a capability scan.  The
/// system adapter reads an already-existing Catalog and its Home manifest;
/// tests can supply deterministic facts without creating SQLite or files.
pub trait GitSourceCapabilityReader: Send + Sync {
    fn read(&self) -> Result<GitSourceCapabilityFacts, String>;
}
