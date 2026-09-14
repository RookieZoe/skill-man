//! Native App-level operations. Browsing this menu never reads the Catalog.
use crate::core::locale::LocaleService;
use crate::seams::locale_store::LocaleSelection;
use crate::tauri_adapter::native_message::{NativeMessageKey as Key, native_message};
use crate::tauri_adapter::{
    appearance_api::{AppearanceApi, AppearanceSelection},
    locale_api::LocaleApi,
};
use std::sync::Arc;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager};
pub const TRAY_ID: &str = "skill-man-tray";
pub const CATALOG_CHANGED_EVENT: &str = "catalog-changed";

pub enum NativeEntry {
    Open,
    About,
    Separator,
    Language(LocaleSelection),
    Appearance(AppearanceSelection),
    CheckUpdate,
    Feedback,
    Quit,
}
pub fn menu_entries(locale: LocaleSelection, appearance: AppearanceSelection) -> Vec<NativeEntry> {
    use NativeEntry::*;
    vec![
        Open,
        About,
        Separator,
        Language(locale),
        Appearance(appearance),
        Separator,
        CheckUpdate,
        Feedback,
        Separator,
        Quit,
    ]
}

pub fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let locale = app.state::<Arc<LocaleService>>().snapshot();
    let appearance = app.state::<AppearanceApi>().snapshot();
    let text = |key| native_message(locale.effective_locale, key, &[]);
    let menu = Menu::new(app)?;
    for entry in menu_entries(locale.selection, appearance.selection) {
        match entry {
            NativeEntry::Open => menu.append(&MenuItem::with_id(
                app,
                "open-window",
                text(Key::TrayOpenWindow),
                true,
                None::<&str>,
            )?)?,
            NativeEntry::About => {
                let title = text(Key::TrayAbout);
                menu.append(&MenuItem::with_id(
                    app,
                    "tray-about",
                    title,
                    true,
                    None::<&str>,
                )?)?;
            }
            NativeEntry::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
            NativeEntry::Language(selection) => {
                let submenu = Submenu::new(app, text(Key::Language), true)?;
                for (value, id, key) in [
                    (LocaleSelection::System, "locale:system", Key::FollowSystem),
                    (LocaleSelection::ZhHans, "locale:zh-Hans", Key::Chinese),
                    (LocaleSelection::En, "locale:en", Key::English),
                ] {
                    submenu.append(&CheckMenuItem::with_id(
                        app,
                        id,
                        text(key),
                        true,
                        value == selection,
                        None::<&str>,
                    )?)?;
                }
                menu.append(&submenu)?;
            }
            NativeEntry::Appearance(selection) => {
                let submenu = Submenu::new(app, text(Key::Appearance), true)?;
                for (value, id, key) in [
                    (
                        AppearanceSelection::System,
                        "appearance:system",
                        Key::FollowSystem,
                    ),
                    (AppearanceSelection::Light, "appearance:light", Key::Light),
                    (AppearanceSelection::Dark, "appearance:dark", Key::Dark),
                ] {
                    submenu.append(&CheckMenuItem::with_id(
                        app,
                        id,
                        text(key),
                        true,
                        value == selection,
                        None::<&str>,
                    )?)?;
                }
                menu.append(&submenu)?;
            }
            NativeEntry::CheckUpdate => menu.append(&MenuItem::with_id(
                app,
                "check-app-update",
                text(Key::CheckUpdate),
                true,
                None::<&str>,
            )?)?,
            NativeEntry::Feedback => menu.append(&MenuItem::with_id(
                app,
                "feedback",
                text(Key::Feedback),
                true,
                None::<&str>,
            )?)?,
            NativeEntry::Quit => {
                menu.append(&PredefinedMenuItem::quit(app, Some(&text(Key::TrayQuit)))?)?
            }
        }
    }
    Ok(menu)
}

pub fn refresh_tray(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(menu) = build_tray_menu(app) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

pub fn handle_tray_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    #[cfg(not(target_os = "macos"))]
    use tauri_plugin_dialog::DialogExt;
    use tauri_plugin_opener::OpenerExt;
    let id = event.id().as_ref();
    let locale = app
        .state::<Arc<LocaleService>>()
        .snapshot()
        .effective_locale;
    let mut failed = None;
    match id {
        "open-window" => crate::tauri_adapter::lifecycle::show_main_window(app),
        "tray-about" => super::about::show(app),
        "feedback" => {
            if app
                .opener()
                .open_url(
                    "https://github.com/RookieZoe/skill-man/issues/new/choose",
                    None::<&str>,
                )
                .is_err()
            {
                failed = Some(Key::FeedbackFailed);
            }
        }
        "check-app-update" => {
            crate::tauri_adapter::native_app_update::show(app, true);
        }
        "locale:system" | "locale:zh-Hans" | "locale:en" => {
            let selection = match id {
                "locale:en" => LocaleSelection::En,
                "locale:zh-Hans" => LocaleSelection::ZhHans,
                _ => LocaleSelection::System,
            };
            if app
                .state::<LocaleApi>()
                .set_locale_selection(crate::tauri_adapter::dto::SetLocaleSelectionRequestDto {
                    selection,
                })
                .is_err()
            {
                failed = Some(Key::LocaleFailed);
            }
        }
        "appearance:system" | "appearance:light" | "appearance:dark" => {
            let selection = match id {
                "appearance:light" => AppearanceSelection::Light,
                "appearance:dark" => AppearanceSelection::Dark,
                _ => AppearanceSelection::System,
            };
            let api = app.state::<AppearanceApi>();
            if api.set_selection(selection).is_err() {
                failed = Some(Key::AppearanceFailed);
            } else {
                crate::tauri_adapter::appearance_api::publish(app);
            }
        }
        _ => return,
    }
    // CheckMenuItem toggles before dispatch; rebuild even on failure or reselect.
    refresh_tray(app);
    if let Some(key) = failed {
        let message = native_message(locale, key, &[]);
        #[cfg(target_os = "macos")]
        {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                super::native_update_window::prompt(
                    &app,
                    "Skill Man".into(),
                    message,
                    native_message(locale, Key::UpdateDone, &[]),
                    None,
                )
                .await;
            });
        }
        #[cfg(not(target_os = "macos"))]
        app.dialog()
            .message(message)
            .title("Skill Man")
            .show(|_| {});
    }
}

#[cfg(test)]
mod native_menu_tests {
    use super::*;
    #[test]
    fn menu_includes_feedback_after_updates_with_single_selected_choices() {
        let model = menu_entries(LocaleSelection::System, AppearanceSelection::Dark);
        assert_eq!(model.len(), 10);
        assert!(matches!(&model[0], NativeEntry::Open));
        assert!(matches!(&model[1], NativeEntry::About));
        assert!(matches!(
            &model[3],
            NativeEntry::Language(LocaleSelection::System)
        ));
        assert!(matches!(
            &model[4],
            NativeEntry::Appearance(AppearanceSelection::Dark)
        ));
        assert!(matches!(&model[6], NativeEntry::CheckUpdate));
        assert!(matches!(&model[7], NativeEntry::Feedback));
        assert!(matches!(&model[9], NativeEntry::Quit));
        for i in [2, 5, 8] {
            assert!(matches!(&model[i], NativeEntry::Separator));
        }
    }
}
