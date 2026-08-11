use std::io::Write;
use std::sync::Arc;

use skill_man_lib::adapters::fixture_catalog::FixtureCatalogStore;
use skill_man_lib::adapters::local_file_source::LocalFileSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::activation::ActivationService;
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::import::ImportService;
use skill_man_lib::core::maintenance::MaintenanceService;
use skill_man_lib::seams::filesystem::{
    ActivationEntrySnapshot, AdoptActivationStep, AdoptAppearanceStep, AdoptJournal,
    DirectoryFingerprint, FileImportJournal, FileImportJournalItem, FileImportJournalPhase,
    FileImportRecoveryBaseline, FileReplacement, FileSystem, FileSystemError, LinkSourceSnapshot,
    ScannedSkillEntry, SkillFingerprint, StagedTreeSnapshot,
};
use skill_man_lib::seams::recovery::RecoveryGate;
use skill_man_lib::tauri_adapter::activation_api::ActivationApi;
use skill_man_lib::tauri_adapter::catalog_api::CatalogApi;
use skill_man_lib::tauri_adapter::dto::{
    ApplyActivationRequestDto, ApplyFileImportRequestDto, ApplyFileImportSelectionRequestDto,
    CancelFileImportRequestDto, CatalogFilterDto, DiscoverFileImportCollectionRequestDto,
    DiscoverFileImportRequestDto, HealthDto, ListSkillsRequestDto, PlanActivationRequestDto,
    PlanFileImportRequestDto, PlanFileImportSelectionRequestDto, PlanFileReinstallRequestDto,
    SourceKindDto,
};
use skill_man_lib::tauri_adapter::health_api::HealthApi;
use skill_man_lib::tauri_adapter::import_api::ImportApi;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

#[test]
fn folder_file_import_installs_a_snapshot_at_the_stable_library_path() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/file-authoring");
    std::fs::create_dir_all(source.join("references")).expect("create file source");
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: file-authoring\ndescription: Install a local snapshot.\n---\n\n# File authoring\n",
    )
    .expect("write SKILL.md");
    std::fs::write(source.join("references/guide.md"), "# Original guide\n")
        .expect("write nested file");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime));

    let discovered = import
        .discover_file_import(DiscoverFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("discover folder file source");
    assert_eq!(discovered.directory_name, "file-authoring");
    assert_eq!(discovered.display_name, "file-authoring");
    assert_eq!(discovered.description, "Install a local snapshot.");

    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan folder file Import");
    assert!(preview.can_apply);
    assert!(preview.conflict.is_none());
    assert_eq!(
        preview.final_entity_path,
        library_root.join("skills/file-authoring").to_string_lossy()
    );

    let result = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply folder file Import");
    let installed = library_root.join("skills/file-authoring");
    assert_eq!(result.final_entity_path, installed.to_string_lossy());
    assert!(
        library_root
            .join("operation-history")
            .join(format!("{}.json", result.operation_id))
            .is_file(),
        "the completed operation journal is retained as history"
    );
    assert!(
        source.join("SKILL.md").is_file(),
        "file source remains in place"
    );
    assert_eq!(
        std::fs::read_to_string(installed.join("references/guide.md"))
            .expect("read installed snapshot"),
        "# Original guide\n"
    );

    let installs = catalog
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::Install,
        })
        .expect("list installed Skills");
    let imported = installs
        .items
        .iter()
        .find(|skill| skill.id == result.skill_id)
        .expect("file Install is visible in Library");
    assert_eq!(imported.source_kind, SourceKindDto::FileInstall);
    let detail = catalog
        .inspect_skill(result.skill_id)
        .expect("inspect installed Skill");
    assert!(
        detail
            .source_label
            .contains(source.to_string_lossy().as_ref())
    );
}

#[test]
fn zip_file_import_extracts_and_installs_a_single_skill_snapshot() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let archive = home.path().join("Downloads/zip-authoring.zip");
    std::fs::create_dir_all(archive.parent().unwrap()).expect("create Downloads");
    let archive_file = std::fs::File::create(&archive).expect("create ZIP archive");
    let mut writer = ZipWriter::new(archive_file);
    writer
        .start_file(
            "zip-authoring/SKILL.md",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .expect("start SKILL.md entry");
    writer
        .write_all(b"---\nname: zip-authoring\ndescription: Install a ZIP snapshot.\n---\n")
        .expect("write SKILL.md entry");
    writer
        .start_file(
            "zip-authoring/references/note.md",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .expect("start nested entry");
    writer
        .write_all(b"# Archived note\n")
        .expect("write nested entry");
    writer.finish().expect("finish ZIP archive");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime));

    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: archive.to_string_lossy().into_owned(),
        })
        .expect("plan ZIP file Import");
    assert_eq!(preview.directory_name, "zip-authoring");
    assert_eq!(preview.original_filename, "zip-authoring.zip");
    let result = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply ZIP file Import");

    let installed = library_root.join("skills/zip-authoring");
    assert!(archive.is_file(), "ZIP source remains in place");
    assert_eq!(
        std::fs::read_to_string(installed.join("references/note.md"))
            .expect("read installed archive entry"),
        "# Archived note\n"
    );
    let detail = catalog
        .inspect_skill(result.skill_id)
        .expect("inspect ZIP-installed Skill");
    assert_eq!(detail.source_kind, SourceKindDto::FileInstall);
    assert!(
        detail
            .source_label
            .contains(archive.to_string_lossy().as_ref())
    );
}

#[test]
fn zip_file_import_rejects_parent_path_traversal_without_leaving_staging() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let archive = home.path().join("Downloads/traversal.zip");
    std::fs::create_dir_all(archive.parent().unwrap()).expect("create Downloads");
    let archive_file = std::fs::File::create(&archive).expect("create ZIP archive");
    let mut writer = ZipWriter::new(archive_file);
    writer
        .start_file("../escaped.md", SimpleFileOptions::default())
        .expect("start traversal entry");
    writer
        .write_all(b"must not escape staging")
        .expect("write traversal entry");
    writer
        .start_file("safe-skill/SKILL.md", SimpleFileOptions::default())
        .expect("start valid Skill entry");
    writer
        .write_all(b"# Safe Skill\n")
        .expect("write valid Skill entry");
    writer.finish().expect("finish ZIP archive");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    let error = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: archive.to_string_lossy().into_owned(),
        })
        .expect_err("parent path traversal is rejected");
    assert_eq!(error.code, "validation");
    assert!(!library_root.join("staging/escaped.md").exists());
    let staging_is_empty = std::fs::read_dir(library_root.join("staging"))
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true);
    assert!(staging_is_empty, "rejected ZIP staging is cleaned up");
}

#[test]
fn zip_file_import_preserves_a_relative_symlink_that_resolves_inside_the_skill() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let archive = home.path().join("Downloads/safe-symlink.zip");
    std::fs::create_dir_all(archive.parent().unwrap()).expect("create Downloads");
    let archive_file = std::fs::File::create(&archive).expect("create ZIP archive");
    let mut writer = ZipWriter::new(archive_file);
    writer
        .start_file("safe-symlink/SKILL.md", SimpleFileOptions::default())
        .expect("start SKILL.md entry");
    writer
        .write_all(b"# Safe symlink\n")
        .expect("write SKILL.md entry");
    writer
        .start_file(
            "safe-symlink/references/guide.md",
            SimpleFileOptions::default(),
        )
        .expect("start symlink target");
    writer
        .write_all(b"# Linked guide\n")
        .expect("write symlink target");
    writer
        .add_symlink(
            "safe-symlink/references/latest.md",
            "guide.md",
            SimpleFileOptions::default(),
        )
        .expect("add relative symlink");
    writer.finish().expect("finish ZIP archive");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: archive.to_string_lossy().into_owned(),
        })
        .expect("plan ZIP with safe relative symlink");
    import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("install ZIP with safe relative symlink");

    let installed_link = library_root.join("skills/safe-symlink/references/latest.md");
    assert!(
        std::fs::symlink_metadata(&installed_link)
            .expect("inspect installed symlink")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(&installed_link).expect("read installed symlink"),
        std::path::PathBuf::from("guide.md")
    );
    assert_eq!(
        std::fs::read_to_string(installed_link).expect("follow installed symlink"),
        "# Linked guide\n"
    );
}

#[test]
fn folder_file_import_preserves_a_relative_symlink_that_resolves_inside_the_skill() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/folder-symlink");
    std::fs::create_dir_all(source.join("references")).expect("create file source");
    std::fs::write(source.join("SKILL.md"), "# Folder symlink\n").expect("write SKILL.md");
    std::fs::write(source.join("references/guide.md"), "# Linked guide\n")
        .expect("write symlink target");
    std::os::unix::fs::symlink("guide.md", source.join("references/latest.md"))
        .expect("create safe source symlink");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan folder with safe relative symlink");
    import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("install folder with safe relative symlink");

    let installed_link = library_root.join("skills/folder-symlink/references/latest.md");
    assert!(
        std::fs::symlink_metadata(&installed_link)
            .expect("inspect installed symlink")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(&installed_link).expect("read installed symlink"),
        std::path::PathBuf::from("guide.md")
    );
}

#[cfg(unix)]
#[test]
fn folder_file_import_accepts_a_contained_relative_skill_document_symlink() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/symlinked-skill-document");
    std::fs::create_dir_all(source.join("docs")).expect("create file source");
    std::fs::write(
        source.join("docs/actual-skill.md"),
        "---\nname: symlinked-skill-document\ndescription: Contained document.\n---\n",
    )
    .expect("write contained Skill document");
    std::os::unix::fs::symlink("docs/actual-skill.md", source.join("SKILL.md"))
        .expect("create contained SKILL.md symlink");

    let import = file_import_api(home.path(), &library_root);
    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan contained SKILL.md symlink");
    assert_eq!(preview.description, "Contained document.");
    let result = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("install contained SKILL.md symlink");

    let installed = library_root.join("skills/symlinked-skill-document/SKILL.md");
    assert!(
        std::fs::symlink_metadata(&installed)
            .expect("inspect installed SKILL.md")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(installed).expect("read installed SKILL.md through its link"),
        "---\nname: symlinked-skill-document\ndescription: Contained document.\n---\n"
    );

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("reopen SQLite"),
    );
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    HealthApi::new(MaintenanceService::new(runtime.clone(), filesystem).begin_startup())
        .run_activation_health_check()
        .expect("health scan accepts contained SKILL.md symlink");
    assert_eq!(
        CatalogApi::new(CatalogService::new(runtime))
            .inspect_skill(result.skill_id)
            .expect("inspect symlinked Skill after health scan")
            .health,
        HealthDto::Healthy
    );
}

#[test]
fn a_missing_local_file_source_is_reported_as_source_unavailable() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let import = file_import_api(home.path(), &library_root);

    let error = import
        .discover_file_import(DiscoverFileImportRequestDto {
            source_path: home
                .path()
                .join("Downloads/vanished")
                .to_string_lossy()
                .into_owned(),
        })
        .expect_err("missing source cannot be discovered");

    assert_eq!(error.code, "source_unavailable");
}

#[test]
fn folder_file_import_rejects_a_lexically_collapsed_but_dangling_symlink() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/dangling-components");
    std::fs::create_dir_all(&source).expect("create file source");
    std::fs::write(source.join("SKILL.md"), "# Dangling components\n").expect("write SKILL.md");
    std::os::unix::fs::symlink("missing/../SKILL.md", source.join("bad-link.md"))
        .expect("create POSIX-dangling symlink");

    let import = file_import_api(home.path(), &library_root);
    let error = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect_err("missing path component remains dangling under POSIX resolution");
    assert_eq!(error.code, "validation");
}

#[test]
fn zip_file_import_rejects_an_absolute_archive_path() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let archive = home.path().join("Downloads/absolute-path.zip");
    std::fs::create_dir_all(archive.parent().unwrap()).expect("create Downloads");
    let archive_file = std::fs::File::create(&archive).expect("create ZIP archive");
    let mut writer = ZipWriter::new(archive_file);
    writer
        .start_file("/absolute-skill/SKILL.md", SimpleFileOptions::default())
        .expect("start absolute entry");
    writer
        .write_all(b"# Must not extract\n")
        .expect("write absolute entry");
    writer.finish().expect("finish ZIP archive");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    let error = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: archive.to_string_lossy().into_owned(),
        })
        .expect_err("absolute ZIP path is rejected");
    assert_eq!(error.code, "validation");
    assert!(!std::path::Path::new("/absolute-skill/SKILL.md").exists());
}

#[test]
fn zip_file_import_rejects_a_symlink_that_resolves_outside_the_skill() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let archive = home.path().join("Downloads/escaping-symlink.zip");
    std::fs::create_dir_all(archive.parent().unwrap()).expect("create Downloads");
    let archive_file = std::fs::File::create(&archive).expect("create ZIP archive");
    let mut writer = ZipWriter::new(archive_file);
    writer
        .start_file("escaping-symlink/SKILL.md", SimpleFileOptions::default())
        .expect("start SKILL.md entry");
    writer
        .write_all(b"# Escaping symlink\n")
        .expect("write SKILL.md entry");
    writer
        .start_file("outside.md", SimpleFileOptions::default())
        .expect("start out-of-Skill target");
    writer
        .write_all(b"outside")
        .expect("write out-of-Skill target");
    writer
        .add_symlink(
            "escaping-symlink/escape.md",
            "../outside.md",
            SimpleFileOptions::default(),
        )
        .expect("add escaping symlink");
    writer.finish().expect("finish ZIP archive");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root,
    ));

    let error = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: archive.to_string_lossy().into_owned(),
        })
        .expect_err("out-of-Skill ZIP symlink is rejected");
    assert_eq!(error.code, "validation");
    assert!(error.message.contains("symlink escapes the Skill"));
}

#[test]
fn zip_file_import_uses_the_archive_stem_for_a_skill_at_the_archive_root() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let archive = home.path().join("Downloads/root-skill.zip");
    std::fs::create_dir_all(archive.parent().unwrap()).expect("create Downloads");
    let archive_file = std::fs::File::create(&archive).expect("create ZIP archive");
    let mut writer = ZipWriter::new(archive_file);
    writer
        .start_file("SKILL.md", SimpleFileOptions::default())
        .expect("start root SKILL.md");
    writer
        .write_all(b"# Root Skill\n")
        .expect("write root SKILL.md");
    writer
        .start_file("references/guide.md", SimpleFileOptions::default())
        .expect("start root nested file");
    writer
        .write_all(b"# Root guide\n")
        .expect("write root nested file");
    writer.finish().expect("finish ZIP archive");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: archive.to_string_lossy().into_owned(),
        })
        .expect("plan archive-root file Import");
    assert_eq!(preview.directory_name, "root-skill");
    import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("install archive-root Skill");
    assert_eq!(
        std::fs::read_to_string(library_root.join("skills/root-skill/references/guide.md"))
            .expect("read installed root archive file"),
        "# Root guide\n"
    );
}

#[test]
fn health_check_marks_a_file_install_modified_when_its_entity_changes() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/hash-baseline");
    std::fs::create_dir_all(source.join("references")).expect("create file source");
    std::fs::write(source.join("SKILL.md"), "# Hash baseline\n").expect("write SKILL.md");
    std::fs::write(source.join("references/guide.md"), "before\n").expect("write baseline content");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime.clone()));
    let health = HealthApi::new(MaintenanceService::new(runtime, filesystem).begin_startup());

    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan file Import");
    let result = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply file Import");
    let installed = library_root.join("skills/hash-baseline");
    let initial = catalog
        .inspect_skill(result.skill_id.clone())
        .expect("inspect initial Install health");
    assert_eq!(initial.health, HealthDto::Healthy);

    std::fs::write(installed.join("references/guide.md"), "after\n")
        .expect("modify installed entity");
    health
        .run_activation_health_check()
        .expect("run health check");

    let modified = catalog
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::Modified,
        })
        .expect("list Modified Skills");
    assert!(
        modified
            .items
            .iter()
            .any(|skill| { skill.id == result.skill_id && skill.health == HealthDto::Modified }),
        "the recorded tree-sha256-v1 baseline detects entity changes"
    );
}

#[test]
fn folder_file_import_rejects_missing_skill_invalid_identity_and_oversized_files() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let downloads = home.path().join("Downloads");
    let missing_document = downloads.join("missing-document");
    std::fs::create_dir_all(&missing_document).expect("create source without SKILL.md");

    let invalid_identity = downloads.join(".hidden-skill");
    std::fs::create_dir_all(&invalid_identity).expect("create hidden source");
    std::fs::write(invalid_identity.join("SKILL.md"), "# Hidden\n").expect("write SKILL.md");

    let oversized = downloads.join("oversized-skill");
    std::fs::create_dir_all(&oversized).expect("create oversized source");
    std::fs::write(oversized.join("SKILL.md"), "# Oversized\n").expect("write SKILL.md");
    let oversized_file =
        std::fs::File::create(oversized.join("payload.bin")).expect("create oversized sparse file");
    oversized_file
        .set_len(32 * 1024 * 1024 + 1)
        .expect("size oversized sparse file");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    for source in [missing_document, invalid_identity, oversized] {
        let error = import
            .plan_file_import(PlanFileImportRequestDto {
                source_path: source.to_string_lossy().into_owned(),
            })
            .expect_err("invalid folder file source is rejected");
        assert_eq!(error.code, "validation", "source: {}", source.display());
        let staging_is_empty = std::fs::read_dir(library_root.join("staging"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true);
        assert!(staging_is_empty, "rejected staging is cleaned up");
    }
}

#[test]
fn file_import_apply_reports_plan_stale_when_the_stable_path_becomes_occupied() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/raced-skill");
    std::fs::create_dir_all(&source).expect("create file source");
    std::fs::write(source.join("SKILL.md"), "# Raced Skill\n").expect("write SKILL.md");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan file Import");
    let occupied = library_root.join("skills/raced-skill");
    std::fs::create_dir_all(&occupied).expect("occupy stable path after Preview");
    std::fs::write(occupied.join("sentinel.txt"), "external content\n")
        .expect("write occupied path sentinel");

    let error = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect_err("occupied stable path invalidates Preview");
    assert_eq!(error.code, "plan_stale");
    assert_eq!(
        std::fs::read_to_string(occupied.join("sentinel.txt"))
            .expect("occupied path remains untouched"),
        "external content\n"
    );
    let staging_is_empty = std::fs::read_dir(library_root.join("staging"))
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true);
    assert!(staging_is_empty, "stale file Import staging is cleaned up");
}

#[test]
fn zip_file_import_rejects_any_parent_component_even_when_the_path_stays_enclosed() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let archive = home.path().join("Downloads/internal-parent.zip");
    std::fs::create_dir_all(archive.parent().unwrap()).expect("create Downloads");
    let archive_file = std::fs::File::create(&archive).expect("create ZIP archive");
    let mut writer = ZipWriter::new(archive_file);
    writer
        .start_file(
            "discarded/../internal-parent/SKILL.md",
            SimpleFileOptions::default(),
        )
        .expect("start entry with internal parent component");
    writer
        .write_all(b"# Internal parent\n")
        .expect("write entry");
    writer.finish().expect("finish ZIP archive");

    let import = file_import_api(home.path(), &library_root);
    let error = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: archive.to_string_lossy().into_owned(),
        })
        .expect_err("every parent path component is rejected");
    assert_eq!(error.code, "validation");
}

#[test]
fn tree_hash_framing_detects_a_file_boundary_collision_as_modified() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/hash-framing");
    std::fs::create_dir_all(&source).expect("create file source");
    std::fs::write(source.join("SKILL.md"), "# Hash framing\n").expect("write SKILL.md");
    std::fs::write(source.join("a"), b"X").expect("write first file");
    std::fs::write(source.join("b"), b"Y").expect("write second file");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime.clone()));
    let health = HealthApi::new(MaintenanceService::new(runtime, filesystem).begin_startup());
    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan file Import");
    let result = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply file Import");

    let installed = library_root.join("skills/hash-framing");
    let mut colliding_bytes = b"X".to_vec();
    colliding_bytes.extend_from_slice(&(4_u64.to_be_bytes()));
    colliding_bytes.extend_from_slice(b"file");
    colliding_bytes.extend_from_slice(&(1_u64.to_be_bytes()));
    colliding_bytes.extend_from_slice(b"b");
    colliding_bytes.extend_from_slice(b"Y");
    std::fs::write(installed.join("a"), colliding_bytes).expect("rewrite first file");
    std::fs::remove_file(installed.join("b")).expect("remove second file");

    health
        .run_activation_health_check()
        .expect("run health check");
    assert_eq!(
        catalog
            .inspect_skill(result.skill_id)
            .expect("inspect changed Install")
            .health,
        HealthDto::Modified
    );
}

#[test]
fn file_import_collection_uses_two_stage_discovery_and_installs_a_multi_selection() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let collection = home.path().join("Downloads/collection");
    for name in ["alpha", "beta"] {
        let skill = collection.join("skills").join(name);
        std::fs::create_dir_all(&skill).expect("create standard Skill directory");
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} candidate\n---\n"),
        )
        .expect("write candidate SKILL.md");
    }
    let plugin_skill = collection.join("declared-plugin/skills/plugin-candidate");
    std::fs::create_dir_all(&plugin_skill).expect("create plugin Skill directory");
    std::fs::write(plugin_skill.join("SKILL.md"), "# Plugin candidate\n")
        .expect("write plugin candidate SKILL.md");
    std::fs::create_dir_all(collection.join("deep/ignored"))
        .expect("create deep non-standard directory");
    std::fs::write(collection.join("deep/ignored/SKILL.md"), "# ignored\n")
        .expect("write deep candidate that standard discovery must ignore");

    let import = file_import_api(home.path(), &library_root);
    let discovery = import
        .discover_file_import_collection(DiscoverFileImportCollectionRequestDto {
            source_path: collection.to_string_lossy().into_owned(),
        })
        .expect("discover standard collection candidates");
    assert!(!discovery.truncated);
    assert_eq!(
        discovery
            .candidates
            .iter()
            .map(|candidate| candidate.directory_name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta", "plugin-candidate"]
    );

    let preview = import
        .plan_file_import_selection(PlanFileImportSelectionRequestDto {
            source_path: collection.to_string_lossy().into_owned(),
            selected_directory_names: vec!["alpha".into(), "beta".into()],
        })
        .expect("plan multiple selected Skills");
    assert!(preview.can_apply);
    assert_eq!(preview.items.len(), 2);
    let result = import
        .apply_file_import_selection(ApplyFileImportSelectionRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply multiple selected Skills");
    assert_eq!(result.items.len(), 2);
    assert!(library_root.join("skills/alpha/SKILL.md").is_file());
    assert!(library_root.join("skills/beta/SKILL.md").is_file());
}

#[cfg(unix)]
#[test]
fn file_import_selection_validates_only_the_selected_candidates() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let collection = home.path().join("Downloads/independent-validation");
    let good = collection.join("skills/good-candidate");
    let bad = collection.join("skills/bad-candidate");
    std::fs::create_dir_all(&good).expect("create good candidate");
    std::fs::create_dir_all(&bad).expect("create bad candidate");
    std::fs::write(good.join("SKILL.md"), "# Good candidate\n").expect("write good candidate");
    std::fs::write(bad.join("SKILL.md"), "# Bad candidate\n").expect("write bad candidate");
    symlink("../../outside", bad.join("escape")).expect("create invalid unselected symlink");

    let import = file_import_api(home.path(), &library_root);
    let preview = import
        .plan_file_import_selection(PlanFileImportSelectionRequestDto {
            source_path: collection.to_string_lossy().into_owned(),
            selected_directory_names: vec!["good-candidate".into()],
        })
        .expect("plan the valid selected candidate");
    let result = import
        .apply_file_import_selection(ApplyFileImportSelectionRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply the valid selected candidate");

    assert_eq!(result.items.len(), 1);
    assert!(
        library_root
            .join("skills/good-candidate/SKILL.md")
            .is_file()
    );
    assert!(!library_root.join("skills/bad-candidate").exists());
}

#[test]
fn file_import_collection_falls_back_to_recursive_discovery_and_reports_truncation() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let recursive = home.path().join("Downloads/recursive-collection");
    std::fs::create_dir_all(recursive.join("packages/deep-skill"))
        .expect("create recursive candidate");
    std::fs::write(
        recursive.join("packages/deep-skill/SKILL.md"),
        "# Deep Skill\n",
    )
    .expect("write recursive candidate");
    let import = file_import_api(home.path(), &library_root);
    let discovery = import
        .discover_file_import_collection(DiscoverFileImportCollectionRequestDto {
            source_path: recursive.to_string_lossy().into_owned(),
        })
        .expect("fallback to recursive discovery");
    assert_eq!(discovery.candidates.len(), 1);
    assert_eq!(discovery.candidates[0].directory_name, "deep-skill");

    let oversized_collection = home.path().join("Downloads/many-skills");
    for index in 0..101 {
        let skill = oversized_collection
            .join("skills")
            .join(format!("candidate-{index:03}"));
        std::fs::create_dir_all(&skill).expect("create many candidate directory");
        std::fs::write(skill.join("SKILL.md"), "# Candidate\n")
            .expect("write many candidate SKILL.md");
    }
    let truncated = import
        .discover_file_import_collection(DiscoverFileImportCollectionRequestDto {
            source_path: oversized_collection.to_string_lossy().into_owned(),
        })
        .expect("discover capped candidate collection");
    assert!(truncated.truncated);
    assert_eq!(truncated.candidates.len(), 100);
}

#[test]
fn cancelling_a_multi_selection_cleans_its_staging_and_operation_journal() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let collection = home.path().join("Downloads/cancel-collection");
    for name in ["cancel-alpha", "cancel-beta"] {
        let skill = collection.join("skills").join(name);
        std::fs::create_dir_all(&skill).expect("create candidate");
        std::fs::write(skill.join("SKILL.md"), "# Candidate\n").expect("write candidate");
    }
    let import = file_import_api(home.path(), &library_root);
    let preview = import
        .plan_file_import_selection(PlanFileImportSelectionRequestDto {
            source_path: collection.to_string_lossy().into_owned(),
            selected_directory_names: vec!["cancel-alpha".into(), "cancel-beta".into()],
        })
        .expect("plan cancellable selection");
    assert!(
        import
            .cancel_file_import(CancelFileImportRequestDto {
                plan_token: preview.plan_token,
            })
            .expect("cancel selection")
    );
    for directory in ["staging", "operations"] {
        assert!(
            std::fs::read_dir(library_root.join(directory))
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(true),
            "cancel leaves no {directory} residue"
        );
    }
}

#[test]
fn explicit_file_reinstall_replaces_the_stable_entity_and_preserves_activation() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/reinstallable");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&source).expect("create reinstall source");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    std::fs::write(source.join("SKILL.md"), "# Reinstallable\n\nversion one\n")
        .expect("write initial source");

    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));
    let activation = ActivationApi::new(ActivationService::new(
        runtime,
        filesystem,
        library_root.clone(),
    ));

    let first_preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan initial file Install");
    let first = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: first_preview.plan_token,
        })
        .expect("apply initial file Install");
    let activation_preview = activation
        .plan_activation(PlanActivationRequestDto {
            skill_id: first.skill_id.clone(),
            agent_id: "claude-code".into(),
            enabled: true,
        })
        .expect("plan Activation");
    let activation_target = std::path::PathBuf::from(&activation_preview.target_path);
    activation
        .apply_activation(ApplyActivationRequestDto {
            plan_token: activation_preview.plan_token,
        })
        .expect("apply Activation");
    let stable_path = library_root.join("skills/reinstallable");
    let activation_path = claude_root.join("reinstallable");
    assert_eq!(
        std::fs::read_link(&activation_path).expect("read initial Activation target"),
        activation_target
    );

    std::fs::write(source.join("SKILL.md"), "# Reinstallable\n\nversion two\n")
        .expect("write replacement source");
    let reinstall_preview = import
        .plan_file_reinstall(PlanFileReinstallRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan explicit file reinstall");
    let reinstalled = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: reinstall_preview.plan_token,
        })
        .expect("apply explicit file reinstall");

    assert_eq!(reinstalled.skill_id, first.skill_id);
    assert_eq!(
        std::fs::read_link(&activation_path).expect("read preserved Activation target"),
        activation_target
    );
    assert_eq!(
        std::fs::read_to_string(stable_path.join("SKILL.md")).expect("read replacement entity"),
        "# Reinstallable\n\nversion two\n"
    );
    assert!(
        std::fs::read_dir(library_root.join("operations"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true),
        "committed reinstall leaves no backup operation"
    );

    std::fs::write(
        source.join("SKILL.md"),
        "# Reinstallable\n\nversion three\n",
    )
    .expect("write another replacement source");
    let stale_preview = import
        .plan_file_reinstall(PlanFileReinstallRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan reinstall while Activation is healthy");
    std::fs::remove_file(&activation_path).expect("remove planned Activation");
    let unrelated = home.path().join("unrelated-skill");
    std::fs::create_dir_all(&unrelated).expect("create unrelated target");
    std::fs::write(unrelated.join("SKILL.md"), "# Unrelated\n").expect("write unrelated target");
    std::os::unix::fs::symlink(&unrelated, &activation_path)
        .expect("replace Activation after reinstall preview");
    let error = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: stale_preview.plan_token,
        })
        .expect_err("changed Activation invalidates reinstall preview");
    assert_eq!(error.code, "plan_stale");
    assert_eq!(
        std::fs::read_to_string(stable_path.join("SKILL.md"))
            .expect("old stable entity survives stale reinstall"),
        "# Reinstallable\n\nversion two\n"
    );
}

#[test]
fn startup_maintenance_rolls_back_an_uncommitted_file_import_journal() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));

    let operation_id = "file-import-crash-fixture";
    let final_entity_path = library_root.join("skills/crash-fixture");
    std::fs::create_dir_all(&final_entity_path).expect("materialize interrupted final entity");
    std::fs::write(final_entity_path.join("SKILL.md"), "# Interrupted\n")
        .expect("write interrupted entity");
    let installed_fingerprint = filesystem
        .directory_fingerprint(&final_entity_path)
        .expect("fingerprint interrupted entity");
    let expected_content_hash = filesystem
        .tree_hash(&final_entity_path)
        .expect("hash interrupted entity");
    let journal = FileImportJournal {
        version: 1,
        operation_id: operation_id.into(),
        phase: FileImportJournalPhase::Planned,
        staging_operation_root: library_root.join("staging").join(operation_id),
        staging_fingerprint: installed_fingerprint.clone(),
        items: vec![FileImportJournalItem {
            skill_id: "file-crash-fixture".into(),
            final_entity_path: final_entity_path.clone(),
            expected_content_hash,
            staged_root_fingerprint: installed_fingerprint.clone(),
            replacement_planned: false,
            replacement_original_tree: None,
            installed_fingerprint: None,
            replacement: None,
        }],
    };
    filesystem
        .write_file_import_journal(&library_root, &journal)
        .expect("persist interrupted operation journal");

    let health = HealthApi::new(
        MaintenanceService::new(runtime, filesystem)
            .with_library_root(library_root.clone())
            .begin_startup(),
    );
    health
        .run_activation_health_check()
        .expect("startup recovery completes before health scan");
    assert!(!final_entity_path.exists());
    assert!(
        std::fs::read_dir(library_root.join("operations"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true),
        "recovered operation journal is archived"
    );
}

#[test]
fn startup_recovery_idempotently_finishes_an_already_rolled_back_reinstall() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/idempotent-reinstall");
    std::fs::create_dir_all(&source).expect("create source");
    std::fs::write(source.join("SKILL.md"), "# Original entity\n").expect("write source");
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));
    let preview = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan original install");
    let installed = import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("install original entity");
    let stable_path = library_root.join("skills/idempotent-reinstall");
    let original_tree = filesystem
        .staged_tree_snapshot(&stable_path)
        .expect("snapshot restored original entity");
    let normalized_library_root = filesystem
        .normalize_configured_path(&library_root)
        .expect("normalize Library root");
    let normalized_stable_path = original_tree.root.canonical_path.clone();

    let operation_id = "idempotent-reinstall-recovery";
    let staging_root = library_root.join("staging").join(operation_id);
    std::fs::create_dir_all(&staging_root).expect("create interrupted staging");
    let staging_fingerprint = filesystem
        .directory_fingerprint(&staging_root)
        .expect("fingerprint interrupted staging");
    let backup_path = normalized_library_root
        .join("operations")
        .join(operation_id)
        .join("backup/idempotent-reinstall");
    let replacement = FileReplacement {
        final_entity_path: normalized_stable_path.clone(),
        installed_fingerprint: original_tree.root.clone(),
        backup_path,
        backup_fingerprint: original_tree.root.clone(),
        original_tree_snapshot: original_tree.clone(),
    };
    filesystem
        .write_file_import_journal(
            &library_root,
            &FileImportJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: FileImportJournalPhase::FileSystemApplied,
                staging_operation_root: staging_root,
                staging_fingerprint,
                items: vec![FileImportJournalItem {
                    skill_id: installed.skill_id,
                    final_entity_path: normalized_stable_path,
                    expected_content_hash: "tree-sha256-v1:not-catalog-committed".into(),
                    staged_root_fingerprint: original_tree.root.clone(),
                    replacement_planned: true,
                    replacement_original_tree: Some(original_tree.clone()),
                    installed_fingerprint: Some(original_tree.root.clone()),
                    replacement: Some(replacement),
                }],
            },
        )
        .expect("persist post-rollback journal");

    HealthApi::new(
        MaintenanceService::new(runtime, filesystem)
            .with_library_root(library_root.clone())
            .begin_startup(),
    )
    .run_activation_health_check()
    .expect("idempotent recovery recognizes restored original entity");
    assert_eq!(
        std::fs::read_to_string(stable_path.join("SKILL.md")).expect("read restored entity"),
        "# Original entity\n"
    );
    assert!(!library_root.join("operations").join(operation_id).exists());
}

#[test]
fn startup_recovery_requires_attention_when_a_planned_reinstall_original_changed() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/stale-reinstall-recovery");
    std::fs::create_dir_all(&source).expect("create source");
    std::fs::write(source.join("SKILL.md"), "# Original\n").expect("write source");
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));
    let initial = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("plan initial install");
    import
        .apply_file_import(ApplyFileImportRequestDto {
            plan_token: initial.plan_token,
        })
        .expect("apply initial install");
    std::fs::write(source.join("SKILL.md"), "# Replacement\n").expect("write replacement source");
    import
        .plan_file_reinstall(PlanFileReinstallRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect("persist planned reinstall journal");

    let stable_document = library_root.join("skills/stale-reinstall-recovery/SKILL.md");
    std::fs::write(&stable_document, "# Externally changed original\n")
        .expect("change original after plan");
    let error = HealthApi::new(
        MaintenanceService::new(runtime, filesystem)
            .with_library_root(library_root.clone())
            .begin_startup(),
    )
    .run_activation_health_check()
    .expect_err("recovery cannot compensate from an unverified original");

    assert_eq!(error.code, "recovery_required");
    assert_eq!(
        std::fs::read_to_string(stable_document).expect("recovery leaves changed path untouched"),
        "# Externally changed original\n"
    );
    assert!(
        std::fs::read_dir(library_root.join("operations"))
            .expect("recovery journal remains")
            .next()
            .is_some()
    );
}

#[test]
fn startup_recovery_removes_staging_left_before_a_journal_was_durable() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let orphan = library_root.join("staging/file-import-pre-journal-crash");
    std::fs::create_dir_all(&orphan).expect("create orphaned staging");
    std::fs::write(orphan.join("partial"), "partial copy").expect("write partial staging");
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));

    HealthApi::new(
        MaintenanceService::new(runtime, filesystem)
            .with_library_root(library_root)
            .begin_startup(),
    )
    .run_activation_health_check()
    .expect("startup recovery cleans pre-journal staging");
    assert!(!orphan.exists());
}

#[cfg(unix)]
#[test]
fn startup_recovery_never_follows_a_symlinked_staging_root() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let victim = home.path().join("must-not-delete");
    std::fs::create_dir_all(victim.join("valuable-directory")).expect("create victim tree");
    std::fs::write(victim.join("valuable-directory/important.txt"), "keep me")
        .expect("write victim file");
    std::fs::create_dir_all(&library_root).expect("create Library root");
    std::os::unix::fs::symlink(&victim, library_root.join("staging"))
        .expect("replace staging root with external symlink");
    let filesystem = MacOsFileSystem::new(home.path().to_path_buf());

    filesystem
        .recover_file_import_journals(&library_root, &[])
        .expect_err("recovery refuses a symlinked staging root");

    assert_eq!(
        std::fs::read_to_string(victim.join("valuable-directory/important.txt"))
            .expect("victim survives recovery"),
        "keep me"
    );
}

#[test]
fn file_import_disk_preflight_returns_a_typed_error_and_cleans_staging() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/no-space");
    std::fs::create_dir_all(&source).expect("create file source");
    std::fs::write(source.join("SKILL.md"), "# No space\n").expect("write SKILL.md");
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(LowSpaceFileSystem {
        delegate: MacOsFileSystem::new(home.path().to_path_buf()),
    });
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    ));

    let error = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect_err("disk preflight rejects insufficient free space");
    assert_eq!(error.code, "disk_full");
    assert!(
        std::fs::read_dir(library_root.join("staging"))
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    );
}

#[test]
fn startup_recovery_gate_blocks_import_and_activation_writes() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let source = home.path().join("Downloads/gated-skill");
    std::fs::create_dir_all(&source).expect("create gated source");
    std::fs::write(source.join("SKILL.md"), "# Gated\n").expect("write gated source");
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let recovery_gate = Arc::new(RecoveryGate::blocked());
    let import = ImportApi::new(
        ImportService::new(
            runtime.clone(),
            filesystem.clone(),
            Arc::new(SystemClock::new()),
            Arc::new(LocalFileSource::new()),
            library_root.clone(),
        )
        .with_recovery_gate(recovery_gate.clone()),
    );
    let activation = ActivationApi::new(
        ActivationService::new(runtime, filesystem, library_root.clone())
            .with_recovery_gate(recovery_gate),
    );

    let import_error = import
        .plan_file_import(PlanFileImportRequestDto {
            source_path: source.to_string_lossy().into_owned(),
        })
        .expect_err("file Import is gated during recovery");
    assert_eq!(import_error.code, "recovery_required");
    let activation_error = activation
        .plan_activation(PlanActivationRequestDto {
            skill_id: "skill-authoring".into(),
            agent_id: "claude-code".into(),
            enabled: true,
        })
        .expect_err("Activation is gated during recovery");
    assert_eq!(activation_error.code, "recovery_required");
    assert!(!library_root.join("staging").exists());
}

struct LowSpaceFileSystem {
    delegate: MacOsFileSystem,
}

impl FileSystem for LowSpaceFileSystem {
    fn inspect_link_source(
        &self,
        path: &std::path::Path,
    ) -> Result<LinkSourceSnapshot, FileSystemError> {
        self.delegate.inspect_link_source(path)
    }

    fn canonical_directory(
        &self,
        path: &std::path::Path,
    ) -> Result<std::path::PathBuf, FileSystemError> {
        self.delegate.canonical_directory(path)
    }

    fn normalize_configured_path(
        &self,
        path: &std::path::Path,
    ) -> Result<std::path::PathBuf, FileSystemError> {
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

    fn skill_directory_is_readable(&self, path: &std::path::Path) -> Result<bool, FileSystemError> {
        self.delegate.skill_directory_is_readable(path)
    }

    fn skill_fingerprint(
        &self,
        path: &std::path::Path,
    ) -> Result<SkillFingerprint, FileSystemError> {
        self.delegate.skill_fingerprint(path)
    }

    fn read_skill_document(&self, path: &std::path::Path) -> Result<String, FileSystemError> {
        self.delegate.read_skill_document(path)
    }

    fn tree_hash(&self, path: &std::path::Path) -> Result<String, FileSystemError> {
        self.delegate.tree_hash(path)
    }

    fn staged_tree_snapshot(
        &self,
        path: &std::path::Path,
    ) -> Result<StagedTreeSnapshot, FileSystemError> {
        self.delegate.staged_tree_snapshot(path)
    }

    fn available_space(&self, _path: &std::path::Path) -> Result<u64, FileSystemError> {
        Ok(0)
    }

    fn staged_child_directories(
        &self,
        path: &std::path::Path,
    ) -> Result<Vec<std::path::PathBuf>, FileSystemError> {
        self.delegate.staged_child_directories(path)
    }

    fn staged_has_skill_document(
        &self,
        directory: &std::path::Path,
        filename: &str,
    ) -> Result<bool, FileSystemError> {
        self.delegate.staged_has_skill_document(directory, filename)
    }

    fn canonicalize_staged_path(
        &self,
        path: &std::path::Path,
    ) -> Result<std::path::PathBuf, FileSystemError> {
        self.delegate.canonicalize_staged_path(path)
    }

    fn install_staged_skill(
        &self,
        staged_skill_path: &std::path::Path,
        final_entity_path: &std::path::Path,
        library_root: &std::path::Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        self.delegate.install_staged_skill(
            staged_skill_path,
            final_entity_path,
            library_root,
            operation_id,
            expected_staged_tree,
        )
    }

    fn discard_staging(
        &self,
        staging_operation_root: &std::path::Path,
        library_root: &std::path::Path,
        expected: Option<&DirectoryFingerprint>,
    ) -> Result<(), FileSystemError> {
        self.delegate
            .discard_staging(staging_operation_root, library_root, expected)
    }

    fn discard_installed_skill(
        &self,
        final_entity_path: &std::path::Path,
        library_root: &std::path::Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        self.delegate
            .discard_installed_skill(final_entity_path, library_root, expected)
    }

    fn create_activation(
        &self,
        target_path: &std::path::Path,
        entry_path: &std::path::Path,
    ) -> Result<(), FileSystemError> {
        self.delegate.create_activation(target_path, entry_path)
    }

    fn remove_activation(&self, entry_path: &std::path::Path) -> Result<(), FileSystemError> {
        self.delegate.remove_activation(entry_path)
    }

    fn scan_skills_directory(
        &self,
        path: &std::path::Path,
    ) -> Result<Vec<ScannedSkillEntry>, FileSystemError> {
        self.delegate.scan_skills_directory(path)
    }

    fn stage_external_directory(
        &self,
        source: &std::path::Path,
        staging_destination: &std::path::Path,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        self.delegate
            .stage_external_directory(source, staging_destination)
    }

    fn restore_external_directory(
        &self,
        source: &std::path::Path,
        destination: &std::path::Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        self.delegate
            .restore_external_directory(source, destination, expected)
    }

    fn apply_adopt_appearances(
        &self,
        appearances: &[AdoptAppearanceStep],
        activations: &[AdoptActivationStep],
    ) -> Result<(), FileSystemError> {
        self.delegate
            .apply_adopt_appearances(appearances, activations)
    }

    fn write_adopt_journal(
        &self,
        library_root: &std::path::Path,
        journal: &AdoptJournal,
    ) -> Result<(), FileSystemError> {
        self.delegate.write_adopt_journal(library_root, journal)
    }

    fn finish_adopt_journal(
        &self,
        library_root: &std::path::Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        self.delegate
            .finish_adopt_journal(library_root, operation_id)
    }

    fn recover_adopt_journals(
        &self,
        library_root: &std::path::Path,
        baselines: &[FileImportRecoveryBaseline],
        adopted_entities: &[FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        self.delegate
            .recover_adopt_journals(library_root, baselines, adopted_entities)
    }
}

fn file_import_api(home: &std::path::Path, library_root: &std::path::Path) -> ImportApi {
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    ImportApi::new(ImportService::new(
        runtime,
        filesystem,
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.to_path_buf(),
    ))
}
