use std::sync::Arc;

use skill_man_lib::adapters::local_file_source::LocalFileSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::import::ImportService;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::tauri_adapter::catalog_api::CatalogApi;
use skill_man_lib::tauri_adapter::dto::{
    ApplyLinkImportRequestDto, CatalogFilterDto, DiscoverLinkImportRequestDto,
    ListSkillsRequestDto, PlanLinkImportRequestDto, PublicErrorDto, SourceKindDto,
};
use skill_man_lib::tauri_adapter::import_api::ImportApi;

mod common;
use common::BoundTestHome;

#[test]
fn link_import_stays_at_its_source_and_enters_the_library_without_distribution() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let library_root = home.library_root.clone();
    let source = home.path().join("Projects/linked-authoring");
    std::fs::create_dir_all(&source).expect("create Link source");
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: linked-authoring\ndescription: Author linked Skills in place.\n---\n\n# Linked authoring\n",
    )
    .expect("write Skill document");

    let filesystem = home.filesystem.clone();
    let runtime = home.runtime.clone();
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
        home.write_gate.clone(),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime.clone()));

    let discovered = import
        .discover_link_import(DiscoverLinkImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("discover Link source");
    let canonical_source = source.canonicalize().expect("canonical Link source");
    assert_eq!(discovered.directory_name, "linked-authoring");
    assert_eq!(
        discovered.final_entity_path,
        canonical_source.to_string_lossy()
    );
    assert_eq!(discovered.display_name, "linked-authoring");
    assert_eq!(discovered.description, "Author linked Skills in place.");
    assert_eq!(discovered.source_entry_path, source.to_string_lossy());

    let preview = import
        .plan_link_import(PlanLinkImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan Link Import");
    assert!(preview.can_apply);
    assert!(preview.conflict.is_none());
    assert!(preview.library_entry_path.is_none());
    assert_eq!(preview.final_entity_path, discovered.final_entity_path);

    let result = import
        .apply_link_import(ApplyLinkImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply Link Import");
    assert!(source.join("SKILL.md").is_file());
    assert!(!library_root.join("skills/linked-authoring").exists());

    let links = catalog
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::Link,
        })
        .expect("list Link Skills");
    let imported = links
        .items
        .iter()
        .find(|skill| skill.id == result.skill_id)
        .expect("imported Link is visible in Library");
    assert_eq!(imported.directory_name, "linked-authoring");
    assert_eq!(imported.source_kind, SourceKindDto::Link);
    let detail = catalog
        .inspect_skill(result.skill_id.clone())
        .expect("inspect imported Link");
    assert_eq!(detail.final_entity_path, discovered.final_entity_path);
    assert!(detail.skill_markdown.contains("# Linked authoring"));
}

#[test]
fn library_conflict_is_visible_in_preview_and_cannot_be_applied() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let library_root = home.library_root.clone();
    let source = home.path().join("Projects/SKILL-AUTHORING");
    std::fs::create_dir_all(&source).expect("create conflicting Link source");
    std::fs::write(source.join("SKILL.md"), "# Different Skill\n")
        .expect("write conflicting Skill document");

    let filesystem = home.filesystem.clone();
    let runtime = home.runtime.clone();
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
        home.write_gate.clone(),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime));
    let before = catalog
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::All,
        })
        .expect("list Library before Conflict");

    let preview = import
        .plan_link_import(PlanLinkImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("preview Library Conflict");
    assert!(!preview.can_apply);
    let conflict = preview.conflict.expect("named Library Conflict");
    assert_eq!(conflict.directory_name, "skill-authoring");
    assert_eq!(conflict.existing_skill_id, "skill-authoring");

    let error = import
        .apply_link_import(ApplyLinkImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect_err("Conflict blocks Apply");
    assert!(matches!(error.error, PublicErrorDto::Conflict { .. }));
    let after = catalog
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::All,
        })
        .expect("list unchanged Library");
    assert_eq!(after.items.len(), before.items.len());
    assert!(source.join("SKILL.md").is_file());
    assert!(!library_root.join("skills/skill-authoring").exists());
}

#[test]
fn apply_reports_plan_stale_when_the_link_source_changes_after_preview() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let library_root = home.library_root.clone();
    let source = home.path().join("Projects/changing-skill");
    std::fs::create_dir_all(&source).expect("create Link source");
    std::fs::write(source.join("SKILL.md"), "# Changing Skill\n").expect("write Skill document");

    let filesystem = home.filesystem.clone();
    let runtime = home.runtime.clone();
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root,
        home.write_gate.clone(),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime));

    let preview = import
        .plan_link_import(PlanLinkImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan Link Import");
    std::fs::remove_file(source.join("SKILL.md")).expect("change source after Preview");

    let error = import
        .apply_link_import(ApplyLinkImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect_err("changed source makes plan stale");
    assert!(matches!(error.error, PublicErrorDto::PlanStale));
    let skills = catalog
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::All,
        })
        .expect("list unchanged Library");
    assert!(
        !skills
            .items
            .iter()
            .any(|skill| skill.directory_name == "changing-skill")
    );
}

#[test]
fn link_identity_comes_from_the_selected_entry_and_retargeting_makes_the_plan_stale() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let library_root = home.library_root.clone();
    let projects = home.path().join("Projects");
    let first_target = projects.join("physical-one");
    let second_target = projects.join("physical-two");
    let source = projects.join("friendly-skill");
    for target in [&first_target, &second_target] {
        std::fs::create_dir_all(target).expect("create Link target");
        std::fs::write(target.join("SKILL.md"), "# Linked Skill\n").expect("write SKILL.md");
    }
    std::os::unix::fs::symlink(&first_target, &source).expect("create selected Link entry");

    let filesystem = home.filesystem.clone();
    let runtime = home.runtime.clone();
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root,
        home.write_gate.clone(),
    ));

    let discovered = import
        .discover_link_import(DiscoverLinkImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("discover selected Link entry");
    assert_eq!(discovered.directory_name, "friendly-skill");
    assert_eq!(discovered.source_entry_path, source.to_string_lossy());
    assert_eq!(
        discovered.final_entity_path,
        first_target.canonicalize().unwrap().to_string_lossy()
    );
    let preview = import
        .plan_link_import(PlanLinkImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan selected Link entry");
    assert_eq!(preview.source_entry_path, source.to_string_lossy());

    std::fs::remove_file(&source).expect("remove original Link entry");
    std::os::unix::fs::symlink(&second_target, &source).expect("retarget selected Link entry");
    let error = import
        .apply_link_import(ApplyLinkImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect_err("retargeted source invalidates Preview");
    assert!(matches!(error.error, PublicErrorDto::PlanStale));
}

#[test]
fn link_source_rejects_more_than_sixteen_intermediate_symlink_hops() {
    let home = tempfile::tempdir().expect("temporary home");
    let projects = home.path().join("Projects");
    let actual_parent = projects.join("actual");
    std::fs::create_dir_all(actual_parent.join("bounded-skill")).expect("create final Skill");
    std::fs::write(actual_parent.join("bounded-skill/SKILL.md"), "# Bounded\n")
        .expect("write SKILL.md");

    for index in (0..17).rev() {
        let link = projects.join(format!("parent-{index}"));
        let target = if index == 16 {
            actual_parent.clone()
        } else {
            projects.join(format!("parent-{}", index + 1))
        };
        std::os::unix::fs::symlink(target, link).expect("create intermediate symlink");
    }

    let filesystem = MacOsFileSystem::new(home.path().to_path_buf());
    let source = projects.join("parent-0/bounded-skill");
    let error = filesystem
        .inspect_link_source(&source)
        .expect_err("intermediate symlink chain must be bounded");
    assert!(error.to_string().contains("exceeds 16 hops"));
}
