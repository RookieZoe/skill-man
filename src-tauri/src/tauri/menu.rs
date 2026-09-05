//! Native application menu (spec §10.3, ADR-0011): the standard macOS app
//! menu with the Window/Help submenu titles resolved through the effective
//! locale. Predefined items keep `None` text so macOS renders their
//! system-localized titles; only Skill Man's own submenu titles are App Copy.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter};

use crate::seams::locale_store::EffectiveLocale;
use crate::tauri_adapter::native_message::{NativeMessageKey, native_message};

pub const APP_MENU_AGENTS_EVENT: &str = "menu-open-agents";
const MENU_ID_OPEN_AGENTS: &str = "open-agents";

/// Builds the standard menu (same structure as `Menu::default`) with
/// locale-resolved `Window` / `Help` submenu titles.
pub fn build_app_menu(app: &AppHandle, locale: EffectiveLocale) -> tauri::Result<Menu<tauri::Wry>> {
    let window_menu = Submenu::with_id_and_items(
        app,
        "window",
        native_message(locale, NativeMessageKey::MenuWindow, &[]),
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    let help_menu = Submenu::with_id_and_items(
        app,
        "help",
        native_message(locale, NativeMessageKey::MenuHelp, &[]),
        true,
        &[],
    )?;
    let agents = MenuItem::with_id(
        app,
        MENU_ID_OPEN_AGENTS,
        native_message(locale, NativeMessageKey::MenuAgents, &[]),
        true,
        None::<&str>,
    )?;
    let pkg_info = app.package_info();
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                pkg_info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &agents,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            &window_menu,
            &help_menu,
        ],
    )
}

/// Rebuilds and installs the menu; used at startup and on `locale://changed`
/// so native surfaces follow the same generation as React.
pub fn apply_app_menu(app: &AppHandle, locale: EffectiveLocale) {
    if let Ok(menu) = build_app_menu(app, locale) {
        let _ = app.set_menu(menu);
    }
}

/// Open Agent Management from the native application menu. The webview owns
/// the surface state; this event only requests the same top-level route as
/// the toolbar and brings the main window forward.
pub fn handle_app_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    if event.id().as_ref() == MENU_ID_OPEN_AGENTS {
        crate::tauri_adapter::lifecycle::show_main_window(app);
        let _ = app.emit(APP_MENU_AGENTS_EVENT, ());
    }
}
