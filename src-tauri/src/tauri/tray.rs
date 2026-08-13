//! Menu-bar tray (spec §9.4): a native quick view of recently enabled
//! Skills plus open-main-window and Quit. The tray is rebuilt whenever the
//! catalog changes (commands emit `catalog-changed`), the main window
//! regains focus, or the locale changes (`locale://changed`), so both the
//! recent list and the labels stay honest and localized (ADR-0011).

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Emitter};

use crate::core::domain::{Health, SkillSummary};
use crate::seams::catalog_store::CatalogStore;
use crate::seams::locale_store::EffectiveLocale;
use crate::tauri_adapter::native_message::{NativeMessageKey, native_message, native_plural};

pub const TRAY_ID: &str = "skill-man-tray";
pub const TRAY_SKILL_LIMIT: u32 = 5;
pub const TRAY_SKILL_EVENT: &str = "tray-open-skill";
pub const CATALOG_CHANGED_EVENT: &str = "catalog-changed";

const MENU_ID_OPEN_WINDOW: &str = "open-window";
const MENU_ID_QUIT: &str = "quit";

#[derive(Clone, serde::Serialize)]
pub struct TraySkillPayload {
    pub skill_id: String,
}

/// One tray line for a recently enabled Skill: name, enabled-Agent count and
/// a health suffix, rendered through the effective locale (ADR-0011). Pure so
/// it is unit-testable without a Tauri runtime.
pub fn tray_skill_label(summary: &SkillSummary, locale: EffectiveLocale) -> String {
    let suffix = match summary.health {
        Health::Healthy => String::new(),
        Health::Broken => native_message(locale, NativeMessageKey::TrayHealthBroken, &[]),
        Health::Modified => native_message(locale, NativeMessageKey::TrayHealthModified, &[]),
    };
    let agents = native_plural(locale, summary.enabled_agent_count as u64);
    format!("{} · {}{}", summary.directory_name, agents, suffix)
}

pub fn build_tray_menu(
    app: &AppHandle,
    recent: &[SkillSummary],
    locale: EffectiveLocale,
) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    let header = MenuItem::with_id(
        app,
        "recent-header",
        native_message(locale, NativeMessageKey::TrayRecent, &[]),
        false,
        None::<&str>,
    )?;
    menu.append(&header)?;
    if recent.is_empty() {
        let empty = MenuItem::with_id(
            app,
            "recent-empty",
            native_message(locale, NativeMessageKey::TrayEmpty, &[]),
            false,
            None::<&str>,
        )?;
        menu.append(&empty)?;
    } else {
        for summary in recent {
            let item = MenuItem::with_id(
                app,
                format!("open-skill:{}", summary.id.0),
                tray_skill_label(summary, locale),
                true,
                None::<&str>,
            )?;
            menu.append(&item)?;
        }
    }
    let separator = PredefinedMenuItem::separator(app)?;
    menu.append(&separator)?;
    let open_window = MenuItem::with_id(
        app,
        MENU_ID_OPEN_WINDOW,
        native_message(locale, NativeMessageKey::TrayOpenWindow, &[]),
        true,
        None::<&str>,
    )?;
    menu.append(&open_window)?;
    let quit = MenuItem::with_id(
        app,
        MENU_ID_QUIT,
        native_message(locale, NativeMessageKey::TrayQuit, &[]),
        true,
        None::<&str>,
    )?;
    menu.append(&quit)?;
    Ok(menu)
}

/// Rebuild the tray menu from the store's recently-enabled Skills in the
/// current effective locale. Failures are silent: the tray keeps its last
/// good menu.
pub fn refresh_tray(app: &AppHandle, store: &dyn CatalogStore, locale: EffectiveLocale) {
    let Ok(recent) = store.recently_enabled(TRAY_SKILL_LIMIT) else {
        return;
    };
    let Ok(menu) = build_tray_menu(app, &recent, locale) else {
        return;
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// Handle a tray menu event: open the window, quit, or open a Skill detail.
pub fn handle_tray_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let menu_id = event.id().as_ref();
    if menu_id == MENU_ID_OPEN_WINDOW {
        crate::tauri_adapter::lifecycle::show_main_window(app);
    } else if menu_id == MENU_ID_QUIT {
        app.exit(0);
    } else if let Some(skill_id) = menu_id.strip_prefix("open-skill:") {
        crate::tauri_adapter::lifecycle::show_main_window(app);
        let _ = app.emit(
            TRAY_SKILL_EVENT,
            TraySkillPayload {
                skill_id: skill_id.to_owned(),
            },
        );
    }
}
