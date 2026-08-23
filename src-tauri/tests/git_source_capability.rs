use std::sync::Arc;

use skill_man_lib::adapters::git_source_capability::SqliteGitSourceCapabilityReader;
use skill_man_lib::core::git_source_capability::{
    GitRepositorySourceFact, GitSourceCapabilityFacts, GitSourceCapabilityKind,
    GitSourceCapabilityReader, GitSourceCapabilityScan, GitSourceCatalogStructure, GitSourceFact,
    GitSourceManifestFact, GitSourceReleaseFact,
};
use skill_man_lib::seams::filesystem::{FileSystem, RemoteParentManifest};

mod common;
use common::{BoundTestHome, CATALOG_FILE_NAME};

#[derive(Clone)]
struct StaticReader {
    facts: GitSourceCapabilityFacts,
}

impl GitSourceCapabilityReader for StaticReader {
    fn read(&self) -> Result<GitSourceCapabilityFacts, String> {
        Ok(self.facts.clone())
    }
}

#[test]
fn legacy_per_skill_git_state_stays_readable_and_closes_source_writes() {
    let scan = GitSourceCapabilityScan::new(Arc::new(StaticReader {
        facts: GitSourceCapabilityFacts {
            catalog_structure: GitSourceCatalogStructure::unsupported(),
            sources: vec![GitSourceFact {
                remote_id: "parent-1".into(),
                canonical_url: "https://github.com/acme/skills".into(),
                catalog_aliases: vec![],
                repository: None,
                manifest: GitSourceManifestFact::Present {
                    remote_id: "parent-1".into(),
                    canonical_url: "https://github.com/acme/skills".into(),
                    aliases: vec![],
                    provider: None,
                    tracking_ref: None,
                    current_release_id: None,
                },
            }],
        },
    }));

    let report = scan.scan().expect("legacy state remains readable");
    let source = &report.sources[0];

    assert_eq!(source.kind, GitSourceCapabilityKind::LegacyPerSkillGitState);
    assert!(source.allows_read_and_maintenance());
    assert!(!source.allows_source_writes());
}

#[test]
fn sqlite_scan_reads_a_legacy_home_without_changing_catalog_or_manifest() {
    let home = BoundTestHome::new();
    home.seed_install_skill(
        "networking",
        "Networking",
        "Remote Skill retained from v6",
        "healthy",
        "2026-08-01T00:00:00Z",
    );
    home.with_sql("seed v6 remote state", |connection| {
        connection
            .execute(
                "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                 VALUES ('parent-1', 'https://github.com/acme/skills', '2026-08-01T00:00:00Z')",
                [],
            )
            .expect("insert parent");
        connection
            .execute(
                "INSERT INTO remote_bindings (
                    skill_id, remote_id, requested_ref, verification_anchor_commit,
                    original_commit_known, skill_path, remote_baseline_hash,
                    current_baseline_hash
                 ) VALUES (
                    'networking', 'parent-1', 'main', 'abcdef', 0, 'skills/networking',
                    'tree-sha256-v1:remote', 'tree-sha256-v1:current'
                 )",
                [],
            )
            .expect("insert legacy binding");
    });
    home.filesystem
        .write_remote_parent_manifest(
            &home.library_root.join("remotes"),
            &RemoteParentManifest {
                schema_version: 1,
                remote_id: "parent-1".into(),
                canonical_url: "https://github.com/acme/skills".into(),
                provider: None,
                tracking_ref: None,
                current_release_id: None,
                aliases: vec![],
                created_at: "2026-08-01T00:00:00Z".into(),
            },
        )
        .expect("write v6 source manifest");

    let catalog_before = std::fs::read(home.catalog_path()).expect("read Catalog before scan");
    let manifest_path = home.library_root.join("remotes/parent-1/source.json");
    let manifest_before = std::fs::read(&manifest_path).expect("read manifest before scan");

    let scan = GitSourceCapabilityScan::new(Arc::new(SqliteGitSourceCapabilityReader::new(
        home.write_gate.clone(),
        CATALOG_FILE_NAME,
        home.filesystem.clone(),
    )));
    let report = scan.scan().expect("scan existing Home");

    assert_eq!(report.sources.len(), 1);
    assert_eq!(
        report.sources[0].kind,
        GitSourceCapabilityKind::LegacyPerSkillGitState
    );
    assert_eq!(
        std::fs::read(home.catalog_path()).expect("read Catalog after scan"),
        catalog_before,
        "scan must not migrate or change the Catalog"
    );
    assert_eq!(
        std::fs::read(&manifest_path).expect("read manifest after scan"),
        manifest_before,
        "scan must not repair or rewrite the manifest"
    );
}

#[test]
fn complete_repository_release_and_member_facts_enable_source_writes() {
    let scan = GitSourceCapabilityScan::new(Arc::new(StaticReader {
        facts: GitSourceCapabilityFacts {
            catalog_structure: supported_structure(),
            sources: vec![repository_source_fact("parent-1")],
        },
    }));

    let report = scan.scan().expect("scan complete repository source");

    assert_eq!(
        report.sources[0].kind,
        GitSourceCapabilityKind::GitRepositorySource
    );
    assert!(report.sources[0].allows_source_writes());
}

#[test]
fn missing_or_unreadable_manifest_keeps_an_otherwise_complete_source_legacy() {
    for manifest in [
        GitSourceManifestFact::Missing,
        GitSourceManifestFact::Unreadable,
    ] {
        let mut source = repository_source_fact("parent-1");
        source.manifest = manifest;
        let report = GitSourceCapabilityScan::new(Arc::new(StaticReader {
            facts: GitSourceCapabilityFacts {
                catalog_structure: supported_structure(),
                sources: vec![source],
            },
        }))
        .scan()
        .expect("a partial source remains readable");

        assert_eq!(
            report.sources[0].kind,
            GitSourceCapabilityKind::LegacyPerSkillGitState
        );
    }
}

#[test]
fn partial_manifest_keeps_an_otherwise_complete_source_legacy() {
    for manifest in [
        GitSourceManifestFact::Present {
            remote_id: "parent-1".into(),
            canonical_url: "https://github.com/acme/parent-1".into(),
            aliases: vec![],
            provider: None,
            tracking_ref: Some("main".into()),
            current_release_id: Some("release-parent-1".into()),
        },
        GitSourceManifestFact::Present {
            remote_id: "parent-1".into(),
            canonical_url: "https://github.com/acme/parent-1".into(),
            aliases: vec![],
            provider: Some("github".into()),
            tracking_ref: None,
            current_release_id: Some("release-parent-1".into()),
        },
        GitSourceManifestFact::Present {
            remote_id: "parent-1".into(),
            canonical_url: "https://github.com/acme/parent-1".into(),
            aliases: vec![],
            provider: Some("github".into()),
            tracking_ref: Some("main".into()),
            current_release_id: None,
        },
    ] {
        let mut source = repository_source_fact("parent-1");
        source.manifest = manifest;
        let report = GitSourceCapabilityScan::new(Arc::new(StaticReader {
            facts: GitSourceCapabilityFacts {
                catalog_structure: supported_structure(),
                sources: vec![source],
            },
        }))
        .scan()
        .expect("a partial source remains readable");

        assert_eq!(
            report.sources[0].kind,
            GitSourceCapabilityKind::LegacyPerSkillGitState
        );
    }
}

#[test]
fn manifest_conflict_closes_only_the_affected_source() {
    let mut conflicted = repository_source_fact("parent-conflicted");
    conflicted.catalog_aliases = vec!["https://github.com/acme/previous-name".into()];
    conflicted.manifest = GitSourceManifestFact::Present {
        remote_id: "parent-conflicted".into(),
        canonical_url: "https://github.com/acme/parent-conflicted".into(),
        aliases: vec!["https://github.com/acme/different-name".into()],
        provider: Some("github".into()),
        tracking_ref: Some("main".into()),
        current_release_id: Some("release-parent-conflicted".into()),
    };
    let scan = GitSourceCapabilityScan::new(Arc::new(StaticReader {
        facts: GitSourceCapabilityFacts {
            catalog_structure: supported_structure(),
            sources: vec![repository_source_fact("parent-healthy"), conflicted],
        },
    }));

    let report = scan.scan().expect("scan all sources");

    assert_eq!(
        report.sources[0].kind,
        GitSourceCapabilityKind::GitRepositorySource
    );
    assert_eq!(
        report.sources[1].kind,
        GitSourceCapabilityKind::RemoteSourceIdentityConflict
    );
    assert!(!report.sources[1].allows_source_writes());
    assert!(report.sources[1].allows_read_and_maintenance());
}

#[test]
fn sqlite_scan_recognizes_only_a_complete_repository_source() {
    let home = BoundTestHome::new();
    home.seed_install_skill(
        "networking",
        "Networking",
        "A complete source member",
        "healthy",
        "2026-08-01T00:00:00Z",
    );
    home.with_sql("seed current Git Repository Source", |connection| {
        connection
            .execute_batch(
                "CREATE TABLE git_repository_sources (
                    remote_id TEXT PRIMARY KEY REFERENCES remote_source_parents(remote_id),
                    provider TEXT NOT NULL,
                    canonical_url TEXT NOT NULL,
                    tracking_ref TEXT NOT NULL,
                    current_release_id TEXT REFERENCES git_source_releases(release_id),
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(provider, canonical_url)
                 );
                 CREATE TABLE git_source_releases (
                    release_id TEXT PRIMARY KEY,
                    remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id),
                    tracking_ref TEXT NOT NULL,
                    resolved_commit TEXT NOT NULL,
                    discovered_at TEXT NOT NULL,
                    UNIQUE(remote_id, resolved_commit)
                 );
                 CREATE TABLE git_source_release_members (
                    release_id TEXT NOT NULL REFERENCES git_source_releases(release_id),
                    skill_path TEXT NOT NULL,
                    skill_name TEXT NOT NULL,
                    tree_hash TEXT NOT NULL,
                    provider_hash TEXT,
                    PRIMARY KEY(release_id, skill_path)
                 );
                 CREATE TABLE git_source_members (
                    skill_id TEXT PRIMARY KEY REFERENCES skills(id),
                    remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id),
                    current_skill_path TEXT NOT NULL,
                    remote_baseline_hash TEXT NOT NULL,
                    current_baseline_hash TEXT NOT NULL,
                    last_checked_at INTEGER,
                    last_updated_at INTEGER
                 );
                 INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                 VALUES ('parent-1', 'https://github.com/acme/skills', '2026-08-01T00:00:00Z');
                 INSERT INTO git_source_releases (
                    release_id, remote_id, tracking_ref, resolved_commit, discovered_at
                 ) VALUES (
                    'release-1', 'parent-1', 'main',
                    '4b825dc642cb6eb9a060e54bf8d69288fbee4904', '2026-08-01T00:00:00Z'
                 );
                 INSERT INTO git_repository_sources (
                    remote_id, provider, canonical_url, tracking_ref, current_release_id,
                    created_at, updated_at
                 ) VALUES (
                    'parent-1', 'github', 'https://github.com/acme/skills', 'main', 'release-1',
                    '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z'
                 );
                 INSERT INTO git_source_release_members (
                    release_id, skill_path, skill_name, tree_hash, provider_hash
                 ) VALUES (
                    'release-1', 'skills/networking', 'networking',
                    'tree-sha256-v1:remote', NULL
                 );
                 INSERT INTO git_source_members (
                    skill_id, remote_id, current_skill_path, remote_baseline_hash,
                    current_baseline_hash, last_checked_at, last_updated_at
                 ) VALUES (
                    'networking', 'parent-1', 'skills/networking',
                    'tree-sha256-v1:remote', 'tree-sha256-v1:current', NULL, NULL
                 );
                 -- The scan must inspect the actual structure and facts,
                 -- rather than inferring eligibility from either version.
                 UPDATE catalog_meta SET schema_version = 1;
                 PRAGMA user_version = 42;",
            )
            .expect("seed source tables and rows");
    });
    home.filesystem
        .write_remote_parent_manifest(
            &home.library_root.join("remotes"),
            &RemoteParentManifest {
                schema_version: 1,
                remote_id: "parent-1".into(),
                canonical_url: "https://github.com/acme/skills".into(),
                provider: Some("github".into()),
                tracking_ref: Some("main".into()),
                current_release_id: Some("release-1".into()),
                aliases: vec![],
                created_at: "2026-08-01T00:00:00Z".into(),
            },
        )
        .expect("write current source manifest");

    let report = GitSourceCapabilityScan::new(Arc::new(SqliteGitSourceCapabilityReader::new(
        home.write_gate.clone(),
        CATALOG_FILE_NAME,
        home.filesystem.clone(),
    )))
    .scan()
    .expect("scan complete source");

    assert_eq!(
        report.sources,
        vec![
            skill_man_lib::core::git_source_capability::GitSourceCapabilitySource {
                remote_id: "parent-1".into(),
                canonical_url: "https://github.com/acme/skills".into(),
                kind: GitSourceCapabilityKind::GitRepositorySource,
            }
        ]
    );

    home.filesystem
        .write_remote_parent_manifest(
            &home.library_root.join("remotes"),
            &RemoteParentManifest {
                schema_version: 1,
                remote_id: "parent-1".into(),
                canonical_url: "https://github.com/acme/renamed-elsewhere".into(),
                provider: Some("github".into()),
                tracking_ref: Some("main".into()),
                current_release_id: Some("release-1".into()),
                aliases: vec![],
                created_at: "2026-08-01T00:00:00Z".into(),
            },
        )
        .expect("write inconsistent source manifest");
    let conflict_report =
        GitSourceCapabilityScan::new(Arc::new(SqliteGitSourceCapabilityReader::new(
            home.write_gate.clone(),
            CATALOG_FILE_NAME,
            home.filesystem.clone(),
        )))
        .scan()
        .expect("scan inconsistent manifest");
    assert_eq!(
        conflict_report.sources[0].kind,
        GitSourceCapabilityKind::RemoteSourceIdentityConflict
    );
    home.filesystem
        .write_remote_parent_manifest(
            &home.library_root.join("remotes"),
            &RemoteParentManifest {
                schema_version: 1,
                remote_id: "parent-1".into(),
                canonical_url: "https://github.com/acme/skills".into(),
                provider: Some("github".into()),
                tracking_ref: Some("main".into()),
                current_release_id: Some("release-1".into()),
                aliases: vec![],
                created_at: "2026-08-01T00:00:00Z".into(),
            },
        )
        .expect("restore matching source manifest");

    home.with_sql("remove required alias structure", |connection| {
        connection
            .execute("DROP TABLE remote_source_aliases", [])
            .expect("remove alias table");
    });
    let partial_structure_report =
        GitSourceCapabilityScan::new(Arc::new(SqliteGitSourceCapabilityReader::new(
            home.write_gate.clone(),
            CATALOG_FILE_NAME,
            home.filesystem.clone(),
        )))
        .scan()
        .expect("scan incomplete source shape");
    assert_eq!(
        partial_structure_report.sources[0].kind,
        GitSourceCapabilityKind::LegacyPerSkillGitState,
        "a missing parent/alias capability must not be inferred from empty aliases"
    );

    home.with_sql(
        "restore alias structure and introduce a foreign key violation",
        |connection| {
            connection
                .execute_batch(
                    "CREATE TABLE remote_source_aliases (
                    remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id),
                    alias_url TEXT NOT NULL UNIQUE,
                    confirmed_at TEXT NOT NULL,
                    PRIMARY KEY(remote_id, alias_url)
                 );
                 PRAGMA foreign_keys = OFF;
                 UPDATE git_source_members SET skill_id = 'missing-skill'
                 WHERE skill_id = 'networking';
                 PRAGMA foreign_keys = ON;",
                )
                .expect("create malformed source state");
        },
    );
    home.filesystem
        .write_remote_parent_manifest(
            &home.library_root.join("remotes"),
            &RemoteParentManifest {
                schema_version: 1,
                remote_id: "parent-1".into(),
                canonical_url: "https://github.com/acme/skills".into(),
                provider: Some("github".into()),
                tracking_ref: Some("main".into()),
                current_release_id: Some("release-1".into()),
                aliases: vec![],
                created_at: "2026-08-01T00:00:00Z".into(),
            },
        )
        .expect("restore matching source manifest");
    let failed_integrity_report =
        GitSourceCapabilityScan::new(Arc::new(SqliteGitSourceCapabilityReader::new(
            home.write_gate.clone(),
            CATALOG_FILE_NAME,
            home.filesystem.clone(),
        )))
        .scan()
        .expect("scan source with failed foreign key check");
    assert_eq!(
        failed_integrity_report.sources[0].kind,
        GitSourceCapabilityKind::LegacyPerSkillGitState,
        "foreign-key failure must close a partial source as Legacy"
    );
}

fn supported_structure() -> GitSourceCatalogStructure {
    GitSourceCatalogStructure {
        has_repository_sources_table: true,
        has_releases_table: true,
        has_release_members_table: true,
        has_members_table: true,
        has_required_columns: true,
        has_required_foreign_keys: true,
        has_required_unique_constraints: true,
        has_clean_foreign_key_check: true,
        has_clean_integrity_check: true,
    }
}

fn repository_source_fact(remote_id: &str) -> GitSourceFact {
    let canonical_url = format!("https://github.com/acme/{remote_id}");
    let release_id = format!("release-{remote_id}");
    GitSourceFact {
        remote_id: remote_id.into(),
        canonical_url: canonical_url.clone(),
        catalog_aliases: vec![],
        repository: Some(GitRepositorySourceFact {
            provider: Some("github".into()),
            canonical_url: canonical_url.clone(),
            tracking_ref: Some("main".into()),
            current_release_id: Some(release_id.clone()),
            current_release: Some(GitSourceReleaseFact {
                release_id: release_id.clone(),
                remote_id: remote_id.into(),
                tracking_ref: "main".into(),
                resolved_commit: "4b825dc642cb6eb9a060e54bf8d69288fbee4904".into(),
                member_paths: vec!["skills/networking".into()],
            }),
            current_member_paths: vec!["skills/networking".into()],
        }),
        manifest: GitSourceManifestFact::Present {
            remote_id: remote_id.into(),
            canonical_url,
            aliases: vec![],
            provider: Some("github".into()),
            tracking_ref: Some("main".into()),
            current_release_id: Some(release_id),
        },
    }
}
