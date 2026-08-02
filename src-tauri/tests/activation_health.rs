use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::core::domain::{ActivationObservedState, AgentId, SkillId};
use skill_man_lib::core::maintenance::MaintenanceService;
use skill_man_lib::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use skill_man_lib::seams::filesystem::FileSystem;

#[test]
fn startup_health_check_classifies_every_desired_activation() {
    let home = tempfile::tempdir().expect("temporary home");
    let agent_root = home.path().join(".claude/skills");
    let entity_root = home.path().join("entities");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    std::fs::create_dir_all(&entity_root).expect("create entity directory");

    let present_target = create_skill(&entity_root, "present");
    let missing_target = create_skill(&entity_root, "missing");
    let mismatch_target = create_skill(&entity_root, "mismatch");
    let other_target = create_skill(&entity_root, "other");
    let no_document_target = entity_root.join("no-document");
    std::fs::create_dir_all(&no_document_target).expect("create entity without SKILL.md");
    let dangling_target = entity_root.join("gone");

    std::os::unix::fs::symlink(&present_target, agent_root.join("present"))
        .expect("create present Activation");
    std::os::unix::fs::symlink(&other_target, agent_root.join("mismatch"))
        .expect("create mismatched Activation");
    std::os::unix::fs::symlink(&dangling_target, agent_root.join("dangling"))
        .expect("create dangling Activation");
    std::os::unix::fs::symlink(&no_document_target, agent_root.join("no-document"))
        .expect("create Activation without SKILL.md");
    std::fs::create_dir(agent_root.join("occupied")).expect("occupy Activation entry");

    let store = Arc::new(HealthStore::new(vec![
        desired("present", &agent_root, &present_target),
        desired("missing", &agent_root, &missing_target),
        desired("mismatch", &agent_root, &mismatch_target),
        desired("dangling", &agent_root, &dangling_target),
        desired("no-document", &agent_root, &no_document_target),
        desired("occupied", &agent_root, &other_target),
    ]));
    let service = MaintenanceService::new(
        store.clone(),
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
    );

    let maintenance = service.begin_startup();
    let report = maintenance
        .run_activation_health_check()
        .expect("check every desired Activation");
    maintenance
        .run_activation_health_check()
        .expect("manual health rerun remains available after startup");

    assert_eq!(report.checked, 6);
    assert_eq!(report.snapshot_version, 42);
    assert_eq!(store.check_count(), 2);
    assert_eq!(
        store.states(),
        HashMap::from([
            ("present".into(), ActivationObservedState::Present),
            ("missing".into(), ActivationObservedState::Missing),
            ("mismatch".into(), ActivationObservedState::TargetMismatch,),
            ("dangling".into(), ActivationObservedState::Dangling),
            ("no-document".into(), ActivationObservedState::Dangling,),
            ("occupied".into(), ActivationObservedState::Occupied),
        ])
    );
}

#[test]
fn skill_probe_rejects_non_regular_documents_without_blocking() {
    let home = tempfile::tempdir().expect("temporary home");
    let fifo_skill = home.path().join("fifo-skill");
    std::fs::create_dir(&fifo_skill).expect("create FIFO Skill directory");
    let fifo_document = fifo_skill.join("SKILL.md");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo_document)
        .status()
        .expect("run mkfifo");
    assert!(status.success());

    let symlink_skill = home.path().join("symlink-skill");
    std::fs::create_dir(&symlink_skill).expect("create symlink Skill directory");
    let shared_document = home.path().join("shared.md");
    std::fs::write(&shared_document, "# Shared\n").expect("write shared document");
    std::os::unix::fs::symlink(&shared_document, symlink_skill.join("SKILL.md"))
        .expect("create symlink SKILL.md");

    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    assert!(
        !filesystem
            .skill_directory_is_readable(&symlink_skill)
            .expect("reject symlink SKILL.md")
    );
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = filesystem.skill_directory_is_readable(&fifo_skill);
        let _ = sender.send(result);
    });

    let fifo_result = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("FIFO probe must not block")
        .expect("inspect FIFO SKILL.md");
    assert!(!fifo_result);
}

fn create_skill(root: &std::path::Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).expect("create Skill entity");
    std::fs::write(path.join("SKILL.md"), format!("# {name}\n")).expect("write SKILL.md");
    path.canonicalize().expect("canonical Skill path")
}

fn desired(
    name: &str,
    agent_root: &std::path::Path,
    expected_target_path: &std::path::Path,
) -> DesiredActivation {
    DesiredActivation {
        skill_id: SkillId(name.into()),
        agent_id: AgentId("claude-code".into()),
        expected_entry_path: agent_root.join(name),
        expected_target_path: expected_target_path.to_path_buf(),
    }
}

struct HealthStore {
    desired: Vec<DesiredActivation>,
    observations: Mutex<Vec<ActivationObservation>>,
    checks: AtomicUsize,
}

impl HealthStore {
    fn new(desired: Vec<DesiredActivation>) -> Self {
        Self {
            desired,
            observations: Mutex::new(Vec::new()),
            checks: AtomicUsize::new(0),
        }
    }

    fn states(&self) -> HashMap<String, ActivationObservedState> {
        self.observations
            .lock()
            .expect("observation lock")
            .iter()
            .map(|item| (item.skill_id.0.clone(), item.observed_state))
            .collect()
    }

    fn check_count(&self) -> usize {
        self.checks.load(Ordering::Relaxed)
    }
}

impl ActivationStore for HealthStore {
    fn load(
        &self,
        _skill_id: &SkillId,
        _agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        Ok(None)
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        Ok(Vec::new())
    }

    fn record(&self, _record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        unreachable!("health observations use their dedicated write seam")
    }

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        self.checks.fetch_add(1, Ordering::Relaxed);
        Ok(self.desired.clone())
    }

    fn record_observations(
        &self,
        observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        *self.observations.lock().expect("observation lock") = observations.to_vec();
        Ok(42)
    }
}
