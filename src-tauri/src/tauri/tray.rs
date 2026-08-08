//! Menu-bar tray (spec §9.4): a native quick view of recently enabled
//! Skills plus open-main-window and Quit. The tray is rebuilt whenever the
//! catalog changes (commands emit `catalog-changed`) or the main window
//! regains focus, so the recent list stays honest.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Emitter};

use crate::core::domain::{Health, SkillSummary};
use crate::seams::catalog_store::CatalogStore;

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
/// a health suffix. Pure so it is unit-testable without a Tauri runtime.
pub fn tray_skill_label(summary: &SkillSummary) -> String {
    let suffix = match summary.health {
        Health::Healthy => "",
        Health::Broken => " · broken",
        Health::Modified => " · modified",
    };
    let agents = if summary.enabled_agent_count == 1 {
        "1 Agent".to_owned()
    } else {
        format!("{} Agents", summary.enabled_agent_count)
    };
    format!("{} · {}{}", summary.directory_name, agents, suffix)
}

pub fn build_tray_menu(
    app: &AppHandle,
    recent: &[SkillSummary],
) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    let header = MenuItem::with_id(
        app,
        "recent-header",
        "Recently enabled",
        false,
        None::<&str>,
    )?;
    menu.append(&header)?;
    if recent.is_empty() {
        let empty = MenuItem::with_id(
            app,
            "recent-empty",
            "No recently enabled Skills",
            false,
            None::<&str>,
        )?;
        menu.append(&empty)?;
    } else {
        for summary in recent {
            let item = MenuItem::with_id(
                app,
                format!("open-skill:{}", summary.id.0),
                tray_skill_label(summary),
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
        "Open main window",
        true,
        None::<&str>,
    )?;
    menu.append(&open_window)?;
    let quit = MenuItem::with_id(app, MENU_ID_QUIT, "Quit", true, None::<&str>)?;
    menu.append(&quit)?;
    Ok(menu)
}

/// Rebuild the tray menu from the store's recently-enabled Skills. Failures
/// are silent: the tray keeps its last good menu.
pub fn refresh_tray(app: &AppHandle, store: &dyn CatalogStore) {
    let Ok(recent) = store.recently_enabled(TRAY_SKILL_LIMIT) else {
        return;
    };
    let Ok(menu) = build_tray_menu(app, &recent) else {
        return;
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// Handle a tray menu event: open the window, quit, or open a Skill detail.
pub fn handle_tray_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    match id {
        MENU_ID_OPEN_WINDOW => crate::tauri_adapter::lifecycle::show_main_window(app),
        MENU_ID_QUIT => app.exit(0),
        _ if id.starts_with("open-skill:") => {
            let skill_id = id.trim_start_matches("open-skill:");
            crate::tauri_adapter::lifecycle::show_main_window(app);
            let _ = app.emit(
                TRAY_SKILL_EVENT,
                TraySkillPayload {
                    skill_id: skill_id.to_owned(),
                },
            );
        }
        _ => {}
    }
}
