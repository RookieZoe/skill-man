//! Shared bound-Home test composition (spec §9: every integration test runs
//! against a real, identity-verified Bound Home — locator, marker and v5
//! Catalog — exactly as production bootstrap resolves it). No fixture
//! seeding exists anymore; tests create real rows at temp-home paths.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, params};
use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::core::home::{BoundHome, HomeId, HomeMarker};
use skill_man_lib::core::write_gate::{WriteGate, WriteGateState};
use skill_man_lib::seams::app_state_store::{AppStateStore, HomeBindingFile, HomeBindingRecord};

pub const HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
pub const STATE_DIR_NAME: &str = "Library/Application Support/skill-man-state";
pub const LIBRARY_ROOT_NAME: &str = "Library/Application Support/skill-man";
pub const CATALOG_FILE_NAME: &str = "skill-man.sqlite3";

/// Exact fixture entity documents (spec §3.5): the byte-for-byte content the
/// production fixture seeded into `fixture-entities/`, whose
/// `tree-sha256-v1` hashes are the immutable fingerprint constants. Any
/// change here silently breaks the fingerprint tests.
pub const FIXTURE_SKILL_AUTHORING_SKILL_MD: &str = "# Skill authoring\n\nBuild focused Agent Skills with explicit triggers, boundaries, and verification.\n\n## Workflow\n\n1. Define the outcome.\n2. Keep the interface narrow.\n3. Verify the artifact.\n";
pub const FIXTURE_MEDIA_XRAY_SKILL_MD: &str =
    "# Media X-ray\n\nTranscribe local audio or video and return source-grounded notes.\n";

#[allow(dead_code)]
pub struct BoundTestHome {
    pub dir: tempfile::TempDir,
    pub home: BoundHome,
    pub state_dir: PathBuf,
    pub library_root: PathBuf,
    pub sqlite: Arc<SqliteCatalogStore>,
    pub runtime: Arc<RuntimeCatalogStore>,
    pub filesystem: Arc<MacOsFileSystem>,
    pub write_gate: Arc<WriteGate>,
}

#[allow(dead_code)]
impl BoundTestHome {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary home");
        let home_path = dir.path().join(LIBRARY_ROOT_NAME);
        let state_dir = dir.path().join(STATE_DIR_NAME);
        std::fs::create_dir_all(&home_path).expect("create Home root");
        std::fs::create_dir_all(&state_dir).expect("create app state directory");

        let home = BoundHome {
            home_id: HomeId(HOME_ID.into()),
            path: home_path.clone(),
            volume_fsid: "test-fsid".into(),
            volume_uuid: "test-uuid".into(),
            bound_at: "2026-08-01T00:00:00Z".into(),
        };

        // Bootstrap locator: the binding commit point.
        AppStateStoreFileSystem::new(state_dir.clone())
            .write_locator(&HomeBindingFile {
                schema_version: 1,
                current: Some(HomeBindingRecord {
                    home_id: home.home_id.clone(),
                    path: home_path.clone(),
                    volume_fsid: home.volume_fsid.clone(),
                    volume_uuid: home.volume_uuid.clone(),
                    bound_at: home.bound_at.clone(),
                }),
                abandoned: vec![],
            })
            .expect("write bootstrap locator");

        // Home marker.
        std::fs::write(
            home_path.join(HomeMarker::FILE_NAME),
            serde_json::to_string_pretty(&HomeMarker {
                schema_version: HomeMarker::SCHEMA_VERSION,
                home_id: home.home_id.clone(),
                volume_fsid: home.volume_fsid.clone(),
                volume_uuid: home.volume_uuid.clone(),
                created_at: "2026-08-01T00:00:00Z".into(),
            })
            .expect("marker JSON"),
        )
        .expect("write Home marker");

        // Catalog with matching identity; the bootstrap probe resolves Bound.
        let sqlite = Arc::new(
            SqliteCatalogStore::create_bound(&home, &home_path.join(CATALOG_FILE_NAME))
                .expect("create bound Catalog"),
        );
        let filesystem = Arc::new(MacOsFileSystem::new(dir.path().to_path_buf()));
        let runtime = Arc::new(RuntimeCatalogStore::new(sqlite.clone(), filesystem.clone()));
        let write_gate = Arc::new(WriteGate::new(WriteGateState::Open(home.clone())));
        Self {
            dir,
            home,
            state_dir,
            library_root: home_path,
            sqlite,
            runtime,
            filesystem,
            write_gate,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.library_root.join(CATALOG_FILE_NAME)
    }

    /// Reopen the same Catalog after a simulated restart: identity is
    /// re-verified through `open_bound` exactly like production.
    pub fn reopen(&self) -> Arc<RuntimeCatalogStore> {
        let sqlite = Arc::new(
            SqliteCatalogStore::open_bound(&self.home, &self.catalog_path())
                .expect("reopen bound Catalog"),
        );
        Arc::new(RuntimeCatalogStore::new(sqlite, self.filesystem.clone()))
    }

    pub fn claude_root(&self) -> PathBuf {
        self.path().join(".claude/skills")
    }

    pub fn codex_root(&self) -> PathBuf {
        self.path().join(".codex/skills")
    }

    pub fn workbench_root(&self) -> PathBuf {
        self.path()
            .join("Library/Application Support/workbench/skills")
    }

    /// Run a SQL batch against the Catalog with a second connection, the way
    /// a real persistence test would observe the file.
    pub fn with_sql(&self, operation: &str, f: impl FnOnce(&Connection)) {
        let connection = Connection::open(self.catalog_path()).expect(operation);
        f(&connection);
    }

    /// The standard Library the old fixture seeded, now real rows at
    /// temp-home paths: three Agents and three Skills (no Activations — the
    /// fixture seed never inserted any).
    pub fn seed_standard_library(&self) {
        for (id, name, kind, skills_path, compatibility) in [
            (
                "claude-code",
                "Claude Code",
                "claude_preset",
                "~/.claude/skills",
                "verified",
            ),
            (
                "codex",
                "Codex",
                "codex_preset",
                "~/.codex/skills",
                "verified",
            ),
            (
                "workbench",
                "Workbench",
                "custom",
                "~/Library/Application Support/workbench/skills",
                "unknown",
            ),
        ] {
            self.seed_agent(id, name, kind, skills_path, compatibility);
        }
        self.seed_link_skill(
            "skill-authoring",
            "Skill authoring",
            "A precise workflow for building maintainable Agent Skills.",
            "2026-07-20T10:42:00Z",
        );
        self.seed_install_skill(
            "media-xray",
            "Media X-ray",
            "Transcribes and inspects local audio and video.",
            "modified",
            "2026-07-19T14:08:00Z",
        );
        self.seed_link_skill(
            "legacy-audit",
            "Legacy audit",
            "Checks an existing skills directory before Adopt.",
            "2026-07-18T03:16:00Z",
        );
    }

    pub fn seed_agent(
        &self,
        id: &str,
        name: &str,
        kind: &str,
        skills_path: &str,
        compatibility: &str,
    ) {
        self.with_sql("seed Agent", |connection| {
            connection
                .execute(
                    "INSERT INTO agents (
                        id, name, kind, skills_path, path_identity_key, detected,
                        compatibility, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?7)",
                    params![
                        id,
                        name,
                        kind,
                        skills_path,
                        skills_path.to_lowercase(),
                        compatibility,
                        "1970-01-01T00:00:00Z"
                    ],
                )
                .expect("seed Agent");
        });
    }

    /// Link Skill: entity stays outside the Library; a real directory with a
    /// SKILL.md is created at the recorded final entity.
    pub fn seed_link_skill(
        &self,
        directory_name: &str,
        display_name: &str,
        description: &str,
        activity_at: &str,
    ) {
        let final_entity = self.path().join("Projects").join(directory_name);
        std::fs::create_dir_all(&final_entity).expect("create Link entity");
        std::fs::write(
            final_entity.join("SKILL.md"),
            format!("# {display_name}\n\n{description}\n"),
        )
        .expect("write SKILL.md");
        self.with_sql("seed Link Skill", |connection| {
            connection
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path, health,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'link', NULL, ?6, 'healthy', ?7, ?7)",
                    params![
                        directory_name,
                        directory_name,
                        directory_name.to_lowercase(),
                        display_name,
                        description,
                        final_entity.to_string_lossy(),
                        activity_at,
                    ],
                )
                .expect("seed Link Skill");
        });
    }

    /// Install Skill (file/remote shape): entity inside the Library.
    pub fn seed_install_skill(
        &self,
        directory_name: &str,
        display_name: &str,
        description: &str,
        health: &str,
        activity_at: &str,
    ) {
        let final_entity = self.library_root.join("skills").join(directory_name);
        std::fs::create_dir_all(&final_entity).expect("create Install entity");
        std::fs::write(
            final_entity.join("SKILL.md"),
            format!("# {display_name}\n\n{description}\n"),
        )
        .expect("write SKILL.md");
        self.with_sql("seed Install Skill", |connection| {
            connection
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path, health,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'remote_install', ?6, ?6, ?7, ?8, ?8)",
                    params![
                        directory_name,
                        directory_name,
                        directory_name.to_lowercase(),
                        display_name,
                        description,
                        final_entity.to_string_lossy(),
                        health,
                        activity_at,
                    ],
                )
                .expect("seed Install Skill");
        });
    }

    pub fn seed_activation(&self, skill_id: &str, agent_id: &str, enabled: bool, observed: &str) {
        let entry_path = match agent_id {
            "claude-code" => self.claude_root().join(skill_id),
            "codex" => self.codex_root().join(skill_id),
            agent => self
                .path()
                .join(format!("agents/{agent}/skills/{skill_id}")),
        };
        let final_entity = self.library_root.join("skills").join(skill_id);
        self.with_sql("seed Activation", |connection| {
            connection
                .execute(
                    "INSERT INTO activations (
                        skill_id, agent_id, desired_enabled, expected_entry_path,
                        expected_target_path, observed_state, last_enabled_at, last_checked_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
                    params![
                        skill_id,
                        agent_id,
                        enabled as i64,
                        entry_path.to_string_lossy(),
                        final_entity.to_string_lossy(),
                        observed,
                        "2026-07-20T10:42:00Z",
                    ],
                )
                .expect("seed Activation");
        });
    }
}

/// A Home carrying the exact production fixture footprint (spec §3.5):
/// a v4 Catalog with the exact seed tuples and the `fixture-entities` trees
/// whose `tree-sha256-v1` hashes are the immutable fingerprint constants.
/// The Catalog is created in WAL mode exactly like the production fixture,
/// so WAL/SHM files exist and snapshot-completeness is testable.
#[allow(dead_code)]
pub struct FixtureHome {
    pub dir: tempfile::TempDir,
    pub library_root: PathBuf,
    pub state_dir: PathBuf,
}

#[allow(dead_code)]
impl FixtureHome {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary fixture Home");
        let library_root = dir.path().join(LIBRARY_ROOT_NAME);
        let state_dir = dir.path().join(STATE_DIR_NAME);
        std::fs::create_dir_all(&library_root).expect("create fixture Home root");
        std::fs::create_dir_all(&state_dir).expect("create app state directory");
        let fixture = Self {
            dir,
            library_root,
            state_dir,
        };
        fixture.seed_fixture_entities();
        fixture.seed_fixture_catalog();
        fixture
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.library_root.join(CATALOG_FILE_NAME)
    }

    /// The three fixture entity trees; `legacy-audit` must NOT be created —
    /// its absence is part of the exact fingerprint.
    fn seed_fixture_entities(&self) {
        let entities = self.library_root.join("fixture-entities");
        for (name, document) in [
            ("skill-authoring", FIXTURE_SKILL_AUTHORING_SKILL_MD),
            ("media-xray", FIXTURE_MEDIA_XRAY_SKILL_MD),
        ] {
            let directory = entities.join(name);
            std::fs::create_dir_all(&directory).expect("create fixture entity tree");
            std::fs::write(directory.join("SKILL.md"), document).expect("write fixture document");
        }
    }

    /// The exact v4 fixture Catalog: three Agents, three Skills, one
    /// Preferences row, zero relations, WAL journal mode.
    fn seed_fixture_catalog(&self) {
        let connection = Connection::open(self.catalog_path()).expect("open fixture Catalog");
        connection
            .execute_batch(
                r#"
                PRAGMA journal_mode = WAL;
                CREATE TABLE catalog_meta (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    schema_version INTEGER NOT NULL,
                    snapshot_version INTEGER NOT NULL DEFAULT 0,
                    first_run_completed_at TEXT,
                    last_startup_check_at TEXT
                );
                CREATE TABLE skills (
                    id TEXT PRIMARY KEY,
                    directory_name TEXT NOT NULL,
                    identity_key TEXT NOT NULL UNIQUE,
                    display_name TEXT NOT NULL,
                    description TEXT NOT NULL DEFAULT '',
                    source_kind TEXT NOT NULL CHECK (source_kind IN ('link', 'remote_install', 'file_install')),
                    library_entry_path TEXT,
                    final_entity_path TEXT NOT NULL,
                    recorded_content_hash TEXT,
                    health TEXT NOT NULL CHECK (health IN ('healthy', 'broken', 'modified')),
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    CHECK (
                        (source_kind = 'link' AND library_entry_path IS NULL)
                        OR (source_kind != 'link' AND library_entry_path IS NOT NULL)
                    )
                );
                CREATE TABLE agents (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    kind TEXT NOT NULL CHECK (kind IN ('claude_preset', 'codex_preset', 'custom')),
                    skills_path TEXT NOT NULL,
                    path_identity_key TEXT NOT NULL UNIQUE,
                    detected INTEGER NOT NULL CHECK (detected IN (0, 1)),
                    compatibility TEXT NOT NULL CHECK (compatibility IN ('verified', 'unknown')),
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE activations (
                    skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    desired_enabled INTEGER NOT NULL CHECK (desired_enabled IN (0, 1)),
                    expected_entry_path TEXT NOT NULL UNIQUE,
                    expected_target_path TEXT NOT NULL,
                    observed_state TEXT NOT NULL CHECK (
                        observed_state IN ('present', 'missing', 'target_mismatch', 'dangling', 'occupied')
                    ),
                    last_enabled_at TEXT,
                    last_checked_at TEXT,
                    PRIMARY KEY (skill_id, agent_id)
                );
                CREATE TABLE file_sources (
                    skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                    original_path TEXT NOT NULL,
                    original_filename TEXT NOT NULL,
                    installed_at TEXT NOT NULL
                );
                CREATE TABLE remote_sources (
                    skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                    source_url TEXT NOT NULL,
                    requested_ref TEXT NOT NULL,
                    resolved_commit TEXT NOT NULL,
                    skill_path TEXT NOT NULL,
                    last_checked_at INTEGER,
                    last_updated_at INTEGER
                );
                CREATE TABLE preferences (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    launch_at_login INTEGER NOT NULL DEFAULT 0 CHECK (launch_at_login IN (0, 1)),
                    show_in_dock INTEGER NOT NULL DEFAULT 1 CHECK (show_in_dock IN (0, 1)),
                    check_app_updates INTEGER NOT NULL DEFAULT 1 CHECK (check_app_updates IN (0, 1)),
                    check_skill_updates INTEGER NOT NULL DEFAULT 1 CHECK (check_skill_updates IN (0, 1)),
                    last_app_update_check_at TEXT,
                    last_skill_update_check_at TEXT
                );
                INSERT INTO catalog_meta (singleton, schema_version, snapshot_version)
                VALUES (1, 4, 9);
                INSERT INTO preferences (singleton) VALUES (1);
                INSERT INTO agents (
                    id, name, kind, skills_path, path_identity_key, detected,
                    compatibility, created_at, updated_at
                ) VALUES
                    ('claude-code', 'Claude Code', 'claude_preset', '~/.claude/skills', '~/.claude/skills', 1, 'verified', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z'),
                    ('codex', 'Codex', 'codex_preset', '~/.codex/skills', '~/.codex/skills', 1, 'verified', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z'),
                    ('workbench', 'Workbench', 'custom', '~/Library/Application Support/workbench/skills', '~/library/application support/workbench/skills', 1, 'unknown', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z');
                "#,
            )
            .expect("create fixture v4 schema");
        let entity = |name: &str| {
            self.library_root
                .join("fixture-entities")
                .join(name)
                .to_string_lossy()
                .into_owned()
        };
        connection
            .execute(
                "INSERT INTO skills (
                    id, directory_name, identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path, health,
                    created_at, updated_at
                 ) VALUES
                    ('skill-authoring', 'skill-authoring', 'skill-authoring', 'Skill authoring', 'A precise workflow for building maintainable Agent Skills.', 'link', NULL, ?1, 'healthy', '2026-07-20T10:42:00Z', '2026-07-20T10:42:00Z'),
                    ('media-xray', 'media-xray', 'media-xray', 'Media X-ray', 'Transcribes and inspects local audio and video.', 'remote_install', ?2, ?2, 'healthy', '2026-07-19T14:08:00Z', '2026-07-19T14:08:00Z'),
                    ('legacy-audit', 'legacy-audit', 'legacy-audit', 'Legacy audit', 'Checks an existing skills directory before Adopt.', 'link', NULL, ?3, 'broken', '2026-07-18T03:16:00Z', '2026-07-18T03:16:00Z')",
                rusqlite::params![
                    entity("skill-authoring"),
                    entity("media-xray"),
                    entity("legacy-audit"),
                ],
            )
            .expect("seed fixture Skills");
        drop(connection);
    }

    /// Run a SQL batch against the fixture Catalog with a second connection.
    pub fn with_sql(&self, operation: &str, f: impl FnOnce(&Connection)) {
        let connection = Connection::open(self.catalog_path()).expect(operation);
        f(&connection);
    }
}
