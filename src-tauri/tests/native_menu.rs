//! Real AppKit menus on the process main thread, without Home or update services.
#[cfg(target_os = "macos")]
fn main() {
    use skill_man_lib::{
        adapters::locale_store::LocaleStoreFileSystem,
        core::locale::LocaleService,
        seams::locale_store::{LocaleSelection, SystemLocaleSource},
        tauri_adapter::{
            appearance_api::{AppearanceApi, AppearanceSelection},
            tray::build_tray_menu,
        },
    };
    use std::sync::Arc;
    use tauri::{Manager, menu::MenuItemKind};

    struct EnglishSystem;
    impl SystemLocaleSource for EnglishSystem {
        fn preferred_language_tags(&self) -> Vec<String> {
            vec!["en-US".into()]
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let locale = Arc::new(LocaleService::new(
        Arc::new(LocaleStoreFileSystem::new(directory.path().into())),
        Arc::new(EnglishSystem),
    ));
    let appearance = AppearanceApi::open(directory.path().into());
    let mut context = tauri::generate_context!(test = true);
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = "io.github.rookiezoe.skillman.native-menu-test".into();
    // Deliberately no Catalog, Home, updater, opener or production setup/plugins.
    let app = tauri::Builder::default()
        .manage(locale.clone())
        .manage(appearance)
        .build(context)
        .expect("build isolated native runtime");
    for (selection, language_id, update_title, feedback_title) in [
        (
            LocaleSelection::System,
            "locale:system",
            "Check for Updates",
            "Report an Issue",
        ),
        (
            LocaleSelection::En,
            "locale:en",
            "Check for Updates",
            "Report an Issue",
        ),
        (
            LocaleSelection::ZhHans,
            "locale:zh-Hans",
            "检查更新",
            "问题反馈",
        ),
    ] {
        locale.set_selection(selection).unwrap();
        for (choice, appearance_id) in [
            (AppearanceSelection::System, "appearance:system"),
            (AppearanceSelection::Light, "appearance:light"),
            (AppearanceSelection::Dark, "appearance:dark"),
        ] {
            app.state::<AppearanceApi>().set_selection(choice).unwrap();
            let before_locale = std::fs::read(directory.path().join("locale.json")).unwrap();
            let before_appearance =
                std::fs::read(directory.path().join("appearance.json")).unwrap();
            let items = build_tray_menu(app.handle()).unwrap().items().unwrap();
            assert_eq!(items.len(), 10, "seven entries and three separators");
            for (index, id) in [
                (0, "open-window"),
                (1, "tray-about"),
                (6, "check-app-update"),
                (7, "feedback"),
            ] {
                assert_eq!(items[index].id().as_ref(), id);
            }
            for index in [2, 5, 8] {
                let MenuItemKind::Predefined(separator) = &items[index] else {
                    panic!("expected native separator");
                };
                assert_eq!(separator.text().unwrap(), "");
            }
            let MenuItemKind::Predefined(quit) = &items[9] else {
                panic!("expected native Quit action");
            };
            assert_eq!(
                quit.text().unwrap(),
                if selection == LocaleSelection::ZhHans {
                    "退出 Skill Man"
                } else {
                    "Quit Skill Man"
                }
            );
            for (index, selected) in [(3, language_id), (4, appearance_id)] {
                let submenu = items[index].as_submenu().expect("native submenu");
                let children = submenu.items().unwrap();
                assert_eq!(children.len(), 3);
                let expected_ids = if index == 3 {
                    ["locale:system", "locale:zh-Hans", "locale:en"]
                } else {
                    ["appearance:system", "appearance:light", "appearance:dark"]
                };
                assert_eq!(
                    children
                        .iter()
                        .map(|item| item.id().as_ref())
                        .collect::<Vec<_>>(),
                    expected_ids
                );
                let checked: Vec<_> = children
                    .iter()
                    .filter(|item| {
                        item.as_check_menuitem()
                            .expect("check item")
                            .is_checked()
                            .unwrap()
                    })
                    .map(|item| item.id().as_ref())
                    .collect();
                assert_eq!(checked, vec![selected]);
            }
            assert_eq!(
                items[6].as_menuitem().unwrap().text().unwrap(),
                update_title
            );
            assert_eq!(
                items[7].as_menuitem().unwrap().text().unwrap(),
                feedback_title
            );
            assert!(
                app.webview_windows().is_empty(),
                "building menu must not open a window"
            );
            assert_eq!(
                std::fs::read(directory.path().join("locale.json")).unwrap(),
                before_locale
            );
            assert_eq!(
                std::fs::read(directory.path().join("appearance.json")).unwrap(),
                before_appearance
            );
        }
    }
    println!("9 real native menu combinations passed without Home, Catalog or update services");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
