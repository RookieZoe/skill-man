//! Locale Authority integration (spec §4.5, §6.1, §10.2; ADR-0011):
//! real-filesystem persistence in the app-level state directory, isolation
//! from the bootstrap locator (Restore/Abandon write only their own files),
//! and the typed DTO serialization contract — every public surface carries
//! closed codes and camelCase fields, never free App Copy.

use std::sync::Arc;

use skill_man_lib::adapters::locale_store::LocaleStoreFileSystem;
use skill_man_lib::core::locale::LocaleService;
use skill_man_lib::seams::app_state_store::{AppStateStore, HomeBindingFile};
use skill_man_lib::seams::locale_store::{
    EffectiveLocale, LocaleSelection, LocaleStore, SystemLocaleSource,
};
use skill_man_lib::tauri_adapter::dto::{
    ChainFaultDto, CommandFailureDto, CompatibilityWarningDto, DiagnosticDto, LocaleSnapshotDto,
    OccupierNotAdoptableReasonDto, PreferencesWarningDto, PublicErrorDto,
    SetLocaleSelectionRequestDto, SkillDetailDto,
};

mod common;

struct FixedSource;

impl SystemLocaleSource for FixedSource {
    fn preferred_language_tags(&self) -> Vec<String> {
        vec!["zh-Hans-CN".into(), "en-US".into()]
    }
}

fn service(state_dir: &std::path::Path) -> LocaleService {
    LocaleService::new(
        Arc::new(LocaleStoreFileSystem::new(state_dir.to_path_buf())),
        Arc::new(FixedSource),
    )
}

#[test]
fn locale_is_app_level_state_writable_without_any_home() {
    let dir = tempfile::tempdir().expect("temp dir");
    let service = service(dir.path());
    // No Home exists at all (Unconfigured): the locale stays readable and
    // writable (ADR-0011).
    assert_eq!(service.snapshot().selection, LocaleSelection::System);
    assert_eq!(
        service.snapshot().effective_locale,
        EffectiveLocale::ZhHans,
        "System mode negotiates the system preferred list"
    );
    let snapshot = service
        .set_selection(LocaleSelection::En)
        .expect("writable without a Home");
    assert_eq!(snapshot.selection, LocaleSelection::En);
    assert_eq!(snapshot.effective_locale, EffectiveLocale::En);
}

#[test]
fn selection_survives_a_restart_through_the_real_filesystem_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    {
        let service = service(dir.path());
        service
            .set_selection(LocaleSelection::ZhHans)
            .expect("persist");
    }
    // A fresh service over the same state dir reads the persisted value.
    let service = service(dir.path());
    let snapshot = service.snapshot();
    assert_eq!(snapshot.selection, LocaleSelection::ZhHans);
    assert_eq!(snapshot.effective_locale, EffectiveLocale::ZhHans);
    assert_eq!(snapshot.generation, 0, "a restart starts at generation 0");
    // Only locale.json exists: the atomic write leaves no tmp file.
    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .expect("state dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(entries, vec!["locale.json".to_string()]);
}

#[test]
fn locale_writes_never_touch_the_bootstrap_locator_and_vice_versa() {
    let dir = tempfile::tempdir().expect("temp dir");
    let state_dir = dir.path().join("state");
    let locale_store = Arc::new(LocaleStoreFileSystem::new(state_dir.clone()));
    let app_state =
        skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem::new(state_dir.clone());

    // Locale writes must not create or alter the locator.
    locale_store
        .store_selection(LocaleSelection::En)
        .expect("persist locale");
    assert!(
        !state_dir.join("home-binding.json").exists(),
        "locale writes never create the locator (Restore/Abandon isolation)"
    );

    // Locator writes must not touch the locale file.
    app_state
        .write_locator(&HomeBindingFile::empty())
        .expect("write locator");
    assert_eq!(
        locale_store.load_selection().expect("locale intact"),
        Some(LocaleSelection::En),
        "bootstrap writes never change the locale selection"
    );
}

#[test]
fn dto_serialization_contract_is_camel_case_and_closed() {
    // LocaleSnapshotDto on the wire (spec §4.7 isomorphism).
    let snapshot: LocaleSnapshotDto = LocaleSnapshotDto {
        selection: LocaleSelection::ZhHans,
        effective_locale: EffectiveLocale::ZhHans,
        generation: 3,
        diagnostic: Some(DiagnosticDto {
            code: "locale_store_unavailable".into(),
            message: "raw detail".into(),
        }),
    };
    let json = serde_json::to_value(&snapshot).expect("serialize");
    assert_eq!(json["selection"], "zh-Hans");
    assert_eq!(json["effectiveLocale"], "zh-Hans");
    assert_eq!(json["generation"], 3);
    assert_eq!(json["diagnostic"]["code"], "locale_store_unavailable");

    // The request parses the exact persisted spellings.
    let request: SetLocaleSelectionRequestDto = serde_json::from_value(serde_json::json!({
        "selection": "zh-Hans",
    }))
    .expect("parse zh-Hans selection");
    assert_eq!(request.selection, LocaleSelection::ZhHans);
    let request: SetLocaleSelectionRequestDto = serde_json::from_value(serde_json::json!({
        "selection": "system",
    }))
    .expect("parse system selection");
    assert_eq!(request.selection, LocaleSelection::System);

    // PublicErrorDto: closed union with typed params.
    let failure = CommandFailureDto {
        error: PublicErrorDto::Conflict {
            directory_name: "ask-matt".into(),
        },
        diagnostic: None,
    };
    let json = serde_json::to_value(&failure).expect("serialize");
    assert_eq!(json["error"]["code"], "conflict");
    assert_eq!(json["error"]["directoryName"], "ask-matt");
    let json = serde_json::to_value(CommandFailureDto {
        error: PublicErrorDto::LocaleStoreUnavailable,
        diagnostic: None,
    })
    .expect("serialize");
    assert_eq!(json["error"]["code"], "locale_store_unavailable");
    let json = serde_json::to_value(CommandFailureDto {
        error: PublicErrorDto::DiskFull {
            required_bytes: 10,
            available_bytes: 2,
        },
        diagnostic: None,
    })
    .expect("serialize");
    assert_eq!(json["error"]["requiredBytes"], 10);
    assert_eq!(json["error"]["availableBytes"], 2);

    // SkillDetailDto: the native-composed sourceLabel is gone; the raw
    // Source Content path is a standalone field (spec §4.7).
    let detail = serde_json::to_value(SkillDetailDto {
        id: "skill-authoring".into(),
        directory_name: "skill-authoring".into(),
        display_name: "Skill authoring".into(),
        description: "Build focused Agent Skills.".into(),
        source_kind: skill_man_lib::tauri_adapter::dto::SourceKindDto::FileInstall,
        health: skill_man_lib::tauri_adapter::dto::HealthDto::Healthy,
        enabled_agent_count: 1,
        final_entity_path: "/Library/skills/skill-authoring".into(),
        file_source_original_path: Some("/tmp/original-source".into()),
        frontmatter_name: None,
        last_activity_at: "2026-08-03T00:00:00Z".into(),
        skill_markdown: "# Skill authoring".into(),
    })
    .expect("serialize");
    assert_eq!(
        detail["fileSourceOriginalPath"], "/tmp/original-source",
        "the raw Source Content path stays a standalone field"
    );
    assert!(
        detail.get("sourceLabel").is_none(),
        "sourceLabel must not cross the DTO"
    );

    // Closed reason/warning unions serialize typed variants only.
    let json = serde_json::to_value(CompatibilityWarningDto::FrontmatterMismatch {
        frontmatter_name: "A".into(),
        directory_name: "b".into(),
    })
    .expect("serialize");
    assert_eq!(json["kind"], "frontmatter_mismatch");
    assert_eq!(json["frontmatterName"], "A");
    assert_eq!(json["directoryName"], "b");

    let json = serde_json::to_value(OccupierNotAdoptableReasonDto::IdentityConflict {
        directory_name: "x".into(),
    })
    .expect("serialize");
    assert_eq!(json["kind"], "identity_conflict");
    assert_eq!(json["directoryName"], "x");

    let json = serde_json::to_value(ChainFaultDto::HopLimit {
        at: "/Users/zoe/.agents/skills/loop".into(),
    })
    .expect("serialize");
    assert_eq!(json["kind"], "hop_limit");
    assert_eq!(json["at"], "/Users/zoe/.agents/skills/loop");

    let json = serde_json::to_value(ChainFaultDto::ReadFailed {
        at: "/Users/zoe/.agents/skills/broken".into(),
        detail: "Permission denied".into(),
    })
    .expect("serialize");
    assert_eq!(json["kind"], "read_failed");
    assert_eq!(json["detail"], "Permission denied");

    let json = serde_json::to_value(PreferencesWarningDto::ShowInDockFailed {
        detail: "raw".into(),
    })
    .expect("serialize");
    assert_eq!(json["kind"], "show_in_dock_failed");
    assert_eq!(json["detail"], "raw");
}

#[test]
fn locale_selection_round_trips_all_three_persisted_values() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = LocaleStoreFileSystem::new(dir.path().to_path_buf());
    for selection in [
        LocaleSelection::System,
        LocaleSelection::En,
        LocaleSelection::ZhHans,
    ] {
        store.store_selection(selection).expect("persist");
        assert_eq!(
            store.load_selection().expect("load"),
            Some(selection),
            "round trip for {selection:?}"
        );
    }
}

#[test]
fn native_message_catalog_serves_the_closed_tray_and_menu_subset() {
    use skill_man_lib::tauri_adapter::native_message::{
        NativeMessageKey, native_message, native_plural,
    };
    // English baseline values.
    assert_eq!(
        native_message(EffectiveLocale::En, NativeMessageKey::TrayRecent, &[]),
        "Recently enabled"
    );
    assert_eq!(
        native_message(EffectiveLocale::En, NativeMessageKey::MenuWindow, &[]),
        "Window"
    );
    // The zh-Hans catalog renders the same keys translated.
    assert_eq!(
        native_message(EffectiveLocale::ZhHans, NativeMessageKey::TrayRecent, &[]),
        "最近启用"
    );
    assert_eq!(
        native_message(EffectiveLocale::ZhHans, NativeMessageKey::MenuWindow, &[]),
        "窗口"
    );
    assert_eq!(native_plural(EffectiveLocale::En, 1), "1 Agent");
    assert_eq!(native_plural(EffectiveLocale::En, 5), "5 Agents");
}
