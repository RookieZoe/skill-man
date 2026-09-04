use skill_man_lib::core::git_source_capability::{
    GitSourceCapabilityKind, GitSourceCapabilityMember, GitSourceCapabilityReport,
    GitSourceCapabilitySource,
};
use skill_man_lib::tauri_adapter::dto::GitSourceCapabilityReportDto;

#[test]
fn source_capability_report_serializes_closed_source_states() {
    let dto: GitSourceCapabilityReportDto = GitSourceCapabilityReport {
        sources: vec![
            GitSourceCapabilitySource {
                remote_id: "healthy".into(),
                canonical_url: "https://github.com/acme/skills".into(),
                kind: GitSourceCapabilityKind::GitRepositorySource,
                members: vec![GitSourceCapabilityMember {
                    skill_id: "skill-a".into(),
                    skill_path: "skills/alpha".into(),
                    presence: true,
                }],
                provider: None,
                tracking_mode: None,
                tracking_value: None,
                selected_ref: None,
                resolved_commit: None,
            },
            GitSourceCapabilitySource {
                remote_id: "legacy".into(),
                canonical_url: "https://github.com/acme/legacy".into(),
                kind: GitSourceCapabilityKind::LegacyPerSkillGitState,
                members: Vec::new(),
                provider: None,
                tracking_mode: None,
                tracking_value: None,
                selected_ref: None,
                resolved_commit: None,
            },
            GitSourceCapabilitySource {
                remote_id: "conflicted".into(),
                canonical_url: "https://github.com/acme/conflicted".into(),
                kind: GitSourceCapabilityKind::RemoteSourceIdentityConflict,
                members: Vec::new(),
                provider: None,
                tracking_mode: None,
                tracking_value: None,
                selected_ref: None,
                resolved_commit: None,
            },
        ],
    }
    .into();

    assert_eq!(
        serde_json::to_string(&dto).expect("serialize typed source report"),
        r#"{"sources":[{"remoteId":"healthy","canonicalUrl":"https://github.com/acme/skills","kind":"git_repository_source","members":[{"skillId":"skill-a","skillPath":"skills/alpha","presence":true}]},{"remoteId":"legacy","canonicalUrl":"https://github.com/acme/legacy","kind":"legacy_per_skill_git_state","members":[]},{"remoteId":"conflicted","canonicalUrl":"https://github.com/acme/conflicted","kind":"remote_source_identity_conflict","members":[]}]}"#
    );
}
