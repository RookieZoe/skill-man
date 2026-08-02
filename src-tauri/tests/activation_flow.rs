use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::core::activation::{ActivationError, ActivationService, SetActivation};
use skill_man_lib::core::domain::{ActivationObservedState, AgentId, AgentKind, SkillId};
use skill_man_lib::seams::activation_store::{
    ActivationContext, ActivationRecord, ActivationStore, ActivationStoreError, ConfiguredAgentPath,
};
use skill_man_lib::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileSystem, FileSystemError,
};
use skill_man_lib::tauri_adapter::activation_api::ActivationApi;
use skill_man_lib::tauri_adapter::dto::{ApplyActivationRequestDto, PlanActivationRequestDto};

#[test]
fn enable_creates_a_direct_activation_and_records_present_state() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::write(skill_root.join("SKILL.md"), "# Skill authoring\n").expect("write SKILL.md");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");

    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root.clone(),
        }],
    ));
    let service = ActivationService::new(
        store.clone(),
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );

    let preview = service
        .plan(SetActivation {
            skill_id: SkillId("skill-authoring".into()),
            agent_id: AgentId("claude-code".into()),
            enabled: true,
        })
        .expect("plan Enable");
    let result = service.apply(&preview.plan_token).expect("apply Enable");

    let activation_path = claude_root.join("skill-authoring");
    assert_eq!(
        std::fs::read_link(&activation_path).expect("Activation symlink"),
        skill_root.canonicalize().expect("canonical Skill target")
    );
    assert!(result.desired_enabled);
    assert_eq!(result.observed_state, ActivationObservedState::Present);
    assert!(store.recorded().desired_enabled);
    assert_eq!(
        store.recorded().expected_entry_path,
        claude_root
            .canonicalize()
            .expect("canonical Claude skills directory")
            .join("skill-authoring")
    );
}

#[test]
fn disable_removes_only_the_expected_activation_and_records_missing_state() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::write(skill_root.join("SKILL.md"), "# Skill authoring\n").expect("write SKILL.md");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let canonical_target = skill_root.canonicalize().expect("canonical Skill target");
    let activation_path = claude_root.join("skill-authoring");
    std::os::unix::fs::symlink(&canonical_target, &activation_path)
        .expect("seed Activation symlink");

    let store = Arc::new(TestActivationStore::new(
        activation_context(
            &skill_root,
            &claude_root,
            true,
            Some(canonical_target.clone()),
        ),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root.clone(),
        }],
    ));
    let service = ActivationService::new(
        store.clone(),
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );

    let preview = service
        .plan(SetActivation {
            skill_id: SkillId("skill-authoring".into()),
            agent_id: AgentId("claude-code".into()),
            enabled: false,
        })
        .expect("plan Disable");
    let result = service.apply(&preview.plan_token).expect("apply Disable");

    assert_eq!(
        std::fs::symlink_metadata(&activation_path)
            .expect_err("Activation is removed")
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(!result.desired_enabled);
    assert_eq!(result.observed_state, ActivationObservedState::Missing);
    assert!(!store.recorded().desired_enabled);
}

#[test]
fn disable_stops_on_target_mismatch_without_deleting_real_content() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    let occupied_directory = claude_root.join("skill-authoring");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&occupied_directory).expect("create real directory at entry");
    std::fs::write(occupied_directory.join("keep.txt"), "user content")
        .expect("write protected content");

    let store = Arc::new(TestActivationStore::new(
        activation_context(
            &skill_root,
            &claude_root,
            true,
            Some(skill_root.canonicalize().expect("canonical Skill target")),
        ),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root,
        }],
    ));
    let service = ActivationService::new(
        store.clone(),
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );

    let error = service
        .plan(SetActivation {
            skill_id: SkillId("skill-authoring".into()),
            agent_id: AgentId("claude-code".into()),
            enabled: false,
        })
        .expect_err("real directory is never removed as an Activation");

    assert!(matches!(error, ActivationError::TargetMismatch(_)));
    assert_eq!(
        std::fs::read_to_string(occupied_directory.join("keep.txt"))
            .expect("protected content remains"),
        "user content"
    );
    assert_eq!(
        store.recorded().observed_state,
        ActivationObservedState::TargetMismatch
    );
}

#[test]
fn disable_without_a_recorded_target_is_rejected_without_touching_the_entry() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let activation_path = claude_root.join("skill-authoring");
    std::os::unix::fs::symlink(
        skill_root.canonicalize().expect("canonical Skill target"),
        &activation_path,
    )
    .expect("seed Activation symlink");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, true, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root,
        }],
    ));
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );

    let error = service
        .plan(SetActivation {
            skill_id: SkillId("skill-authoring".into()),
            agent_id: AgentId("claude-code".into()),
            enabled: false,
        })
        .expect_err("Disable cannot trust an Activation without its recorded target");

    assert!(matches!(error, ActivationError::Validation(_)));
    assert!(std::fs::symlink_metadata(activation_path).is_ok());
}

#[test]
fn preflight_rejects_agent_paths_that_overlap_library_or_another_agent() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let overlapping_library_path = library_root.join("agent-skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&overlapping_library_path).expect("create unsafe Agent path");

    let library_overlap_store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &overlapping_library_path, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: overlapping_library_path,
        }],
    ));
    let library_overlap_service = ActivationService::new(
        library_overlap_store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root.clone(),
    );

    let error = library_overlap_service
        .plan(enable_request())
        .expect_err("Agent path inside Library is rejected");
    assert!(matches!(error, ActivationError::PathOverlap));

    let claude_root = home.path().join(".claude/skills");
    let other_agent_root = home.path().join(".claude");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let agent_overlap_store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![
            ConfiguredAgentPath {
                agent_id: AgentId("claude-code".into()),
                skills_path: claude_root,
            },
            ConfiguredAgentPath {
                agent_id: AgentId("codex".into()),
                skills_path: other_agent_root,
            },
        ],
    ));
    let agent_overlap_service = ActivationService::new(
        agent_overlap_store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );

    let error = agent_overlap_service
        .plan(enable_request())
        .expect_err("nested Agent paths are rejected");
    assert!(matches!(error, ActivationError::PathOverlap));
}

#[test]
fn apply_stops_when_the_activation_entry_changes_after_preview() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root.clone(),
        }],
    ));
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );
    let preview = service.plan(enable_request()).expect("plan Enable");
    let occupied_path = claude_root.join("skill-authoring");
    std::fs::create_dir(&occupied_path).expect("external actor occupies entry");

    let error = service
        .apply(&preview.plan_token)
        .expect_err("stale plan cannot write");

    assert!(matches!(error, ActivationError::PlanStale));
    assert!(occupied_path.is_dir());
}

#[test]
fn apply_stops_when_the_agent_path_changes_after_preview() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    let replacement_root = home.path().join("custom-claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    std::fs::create_dir_all(&replacement_root).expect("create replacement Agent directory");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root.clone(),
        }],
    ));
    let service = ActivationService::new(
        store.clone(),
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );
    let preview = service.plan(enable_request()).expect("plan Enable");
    store.set_agent_path(replacement_root.clone());

    let error = service
        .apply(&preview.plan_token)
        .expect_err("a plan cannot write after its configured Agent path changes");

    assert!(matches!(error, ActivationError::PlanStale));
    assert!(!claude_root.join("skill-authoring").exists());
    assert!(!replacement_root.join("skill-authoring").exists());
}

#[test]
fn apply_stops_when_the_agent_directory_is_replaced_at_the_same_path() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    let previous_root = home.path().join("previous-claude-skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root.clone(),
        }],
    ));
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );
    let preview = service.plan(enable_request()).expect("plan Enable");
    std::fs::rename(&claude_root, previous_root).expect("move original Agent directory");
    std::fs::create_dir_all(&claude_root).expect("replace Agent directory at the same path");

    let error = service
        .apply(&preview.plan_token)
        .expect_err("a replaced Agent directory invalidates the plan");

    assert!(matches!(error, ActivationError::PlanStale));
    assert!(!claude_root.join("skill-authoring").exists());
}

#[test]
fn apply_stops_when_the_final_entity_is_replaced_after_preview() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let previous_skill_root = library_root.join("skills/previous-skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root.clone(),
        }],
    ));
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );
    let preview = service.plan(enable_request()).expect("plan Enable");
    std::fs::rename(&skill_root, previous_skill_root).expect("move original final entity");
    std::fs::create_dir_all(&skill_root).expect("replace final entity at the same path");

    let error = service
        .apply(&preview.plan_token)
        .expect_err("a replaced final entity invalidates the plan");

    assert!(matches!(error, ActivationError::PlanStale));
    assert!(!claude_root.join("skill-authoring").exists());
}

#[test]
fn cancelled_and_expired_plans_cannot_be_applied() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root,
        }],
    ));
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );
    let cancelled = service.plan(enable_request()).expect("plan Enable");
    assert!(service.cancel(&cancelled.plan_token).expect("cancel plan"));
    assert!(matches!(
        service
            .apply(&cancelled.plan_token)
            .expect_err("cancelled plan is consumed"),
        ActivationError::PlanNotFound
    ));

    let expiring_service = service.with_plan_ttl(Duration::ZERO);
    let expired = expiring_service
        .plan(enable_request())
        .expect("plan Enable");
    assert!(matches!(
        expiring_service
            .apply(&expired.plan_token)
            .expect_err("expired plan is unavailable"),
        ActivationError::PlanNotFound
    ));
}

#[test]
fn failed_state_write_and_failed_compensation_requires_recovery() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let context = activation_context(&skill_root, &claude_root, false, None);
    let store = Arc::new(FailingActivationStore {
        context,
        agent_paths: vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root.clone(),
        }],
    });
    let service = ActivationService::new(
        store,
        Arc::new(FailingCompensationFileSystem {
            delegate: MacOsFileSystem::new(home.path().to_path_buf()),
        }),
        library_root,
    );
    let preview = service.plan(enable_request()).expect("plan Enable");

    let error = service
        .apply(&preview.plan_token)
        .expect_err("failed compensation must be surfaced");

    assert!(matches!(error, ActivationError::RecoveryRequired { .. }));
    assert!(std::fs::symlink_metadata(claude_root.join("skill-authoring")).is_ok());
}

#[test]
fn a_missing_unrelated_agent_directory_does_not_block_claude_enable() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![
            ConfiguredAgentPath {
                agent_id: AgentId("claude-code".into()),
                skills_path: claude_root,
            },
            ConfiguredAgentPath {
                agent_id: AgentId("codex".into()),
                skills_path: home.path().join(".codex/skills"),
            },
        ],
    ));
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );

    let preview = service
        .plan(enable_request())
        .expect("missing unrelated Agent directory is only a path-policy input");

    assert!(
        preview
            .entry_path
            .ends_with(".claude/skills/skill-authoring")
    );
}

#[test]
fn tauri_adapter_exposes_typed_plan_and_apply_results() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let skill_root = library_root.join("skills/skill-authoring");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let store = Arc::new(TestActivationStore::new(
        activation_context(&skill_root, &claude_root, false, None),
        vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: claude_root,
        }],
    ));
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    );
    let api = ActivationApi::new(service);

    let preview = api
        .plan_activation(PlanActivationRequestDto {
            skill_id: "skill-authoring".into(),
            agent_id: "claude-code".into(),
            enabled: true,
        })
        .expect("typed Activation preview");
    let result = api
        .apply_activation(ApplyActivationRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("typed Activation result");

    assert_eq!(result.skill_id, "skill-authoring");
    assert_eq!(result.agent_id, "claude-code");
    assert!(result.desired_enabled);
    assert_eq!(result.snapshot_version, 8);
}

struct TestActivationStore {
    context: Mutex<ActivationContext>,
    agent_paths: Vec<ConfiguredAgentPath>,
    record: Mutex<Option<ActivationRecord>>,
}

impl TestActivationStore {
    fn new(context: ActivationContext, agent_paths: Vec<ConfiguredAgentPath>) -> Self {
        Self {
            context: Mutex::new(context),
            agent_paths,
            record: Mutex::new(None),
        }
    }

    fn recorded(&self) -> ActivationRecord {
        self.record
            .lock()
            .expect("record lock")
            .clone()
            .expect("Activation record")
    }

    fn set_agent_path(&self, path: PathBuf) {
        self.context.lock().expect("context lock").agent_skills_path = path;
    }
}

impl ActivationStore for TestActivationStore {
    fn load(
        &self,
        _skill_id: &SkillId,
        _agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        Ok(Some(self.context.lock().expect("context lock").clone()))
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        Ok(self.agent_paths.clone())
    }

    fn record(&self, record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        *self.record.lock().expect("record lock") = Some(record.clone());
        let mut context = self.context.lock().expect("context lock");
        context.desired_enabled = record.desired_enabled;
        context.expected_target_path = Some(record.expected_target_path);
        Ok(8)
    }
}

fn activation_context(
    skill_root: &std::path::Path,
    agent_root: &std::path::Path,
    desired_enabled: bool,
    expected_target_path: Option<PathBuf>,
) -> ActivationContext {
    ActivationContext {
        skill_id: SkillId("skill-authoring".into()),
        directory_name: "skill-authoring".into(),
        final_entity_path: skill_root.to_path_buf(),
        agent_id: AgentId("claude-code".into()),
        agent_name: "Claude Code".into(),
        agent_kind: AgentKind::ClaudePreset,
        agent_skills_path: agent_root.to_path_buf(),
        desired_enabled,
        expected_target_path,
    }
}

fn enable_request() -> SetActivation {
    SetActivation {
        skill_id: SkillId("skill-authoring".into()),
        agent_id: AgentId("claude-code".into()),
        enabled: true,
    }
}

struct FailingActivationStore {
    context: ActivationContext,
    agent_paths: Vec<ConfiguredAgentPath>,
}

impl ActivationStore for FailingActivationStore {
    fn load(
        &self,
        _skill_id: &SkillId,
        _agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        Ok(Some(self.context.clone()))
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        Ok(self.agent_paths.clone())
    }

    fn record(&self, _record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "injected state failure".into(),
        ))
    }
}

struct FailingCompensationFileSystem {
    delegate: MacOsFileSystem,
}

impl FileSystem for FailingCompensationFileSystem {
    fn canonical_directory(&self, path: &std::path::Path) -> Result<PathBuf, FileSystemError> {
        self.delegate.canonical_directory(path)
    }

    fn normalize_configured_path(
        &self,
        path: &std::path::Path,
    ) -> Result<PathBuf, FileSystemError> {
        self.delegate.normalize_configured_path(path)
    }

    fn directory_fingerprint(
        &self,
        path: &std::path::Path,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        self.delegate.directory_fingerprint(path)
    }

    fn activation_snapshot(
        &self,
        entry_path: &std::path::Path,
    ) -> Result<ActivationEntrySnapshot, FileSystemError> {
        self.delegate.activation_snapshot(entry_path)
    }

    fn create_activation(
        &self,
        target_path: &std::path::Path,
        entry_path: &std::path::Path,
    ) -> Result<(), FileSystemError> {
        self.delegate.create_activation(target_path, entry_path)
    }

    fn remove_activation(&self, entry_path: &std::path::Path) -> Result<(), FileSystemError> {
        Err(FileSystemError::Io {
            operation: "compensate Activation",
            path: entry_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected compensation failure",
            ),
        })
    }
}
