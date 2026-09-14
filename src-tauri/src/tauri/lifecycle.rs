//! App lifecycle glue (spec §10.3): window show/hide policy, Dock
//! activation policy and the login-item switch. Thin Tauri-runtime code;
//! no domain rules live here.

use tauri::{ActivationPolicy, AppHandle, Manager};

pub const MAIN_WINDOW_LABEL: &str = "main";

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Hide the main window without destroying it, so the menu-bar app stays
/// resident and the window can be reopened (spec §9.4: red close only closes
/// the window; ⌘Q / Quit terminates the app).
pub fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.hide();
    }
}

/// `show_in_dock`: `Regular` keeps the app in the Dock, `Accessory` makes it
/// a menu-bar-only app (spec §10.2).
pub fn apply_show_in_dock(app: &AppHandle, show_in_dock: bool) -> Result<(), String> {
    app.set_activation_policy(if show_in_dock {
        ActivationPolicy::Regular
    } else {
        ActivationPolicy::Accessory
    })
    .map_err(|error| error.to_string())
}

/// `launch_at_login`: registers the app as a macOS login item. Fails on
/// non-bundled development builds, where the caller should surface a warning
/// instead of rejecting the persisted preference.
pub fn apply_launch_at_login(app: &AppHandle, launch_at_login: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let autostart = app.autolaunch();
    let result = if launch_at_login {
        autostart.enable()
    } else {
        autostart.disable()
    };
    result.map_err(|error| error.to_string())
}
