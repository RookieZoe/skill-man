use std::sync::Arc;

use skill_man_lib::adapters::fixture_catalog::FixtureCatalogStore;
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::tauri_adapter::catalog_api::CatalogApi;
use skill_man_lib::tauri_adapter::dto::{CatalogFilterDto, ListSkillsRequestDto, SourceKindDto};

#[test]
fn the_catalog_adapter_returns_a_filtered_snapshot_of_fixture_skills() {
    let store = FixtureCatalogStore::library_desk();
    let api = CatalogApi::new(CatalogService::new(Arc::new(store)));

    let result = api
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::Modified,
        })
        .expect("fixture catalog list");

    assert_eq!(result.snapshot_version, 7);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, "media-xray");
    assert_eq!(result.items[0].directory_name, "media-xray");
    assert_eq!(result.items[0].source_kind, SourceKindDto::RemoteInstall);
}

#[test]
fn the_catalog_adapter_returns_a_read_only_skill_detail() {
    let store = FixtureCatalogStore::library_desk();
    let api = CatalogApi::new(CatalogService::new(Arc::new(store)));

    let detail = api
        .inspect_skill("skill-authoring".into())
        .expect("fixture skill detail");

    assert_eq!(detail.id, "skill-authoring");
    assert_eq!(detail.directory_name, "skill-authoring");
    assert_eq!(
        detail.final_entity_path,
        "/Users/zoe/Codes/AI/skills/skill-authoring"
    );
    assert_eq!(detail.frontmatter_name.as_deref(), Some("skill-authoring"));
    assert!(detail.skill_markdown.starts_with("# Skill authoring\n"));
    assert_eq!(detail.last_activity_at, "2026-07-20T10:42:00Z");
}
