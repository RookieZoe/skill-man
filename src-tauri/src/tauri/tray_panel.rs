//! The tray owns a short-lived webview; the main window never mounts its UI.
use crate::tauri_adapter::dto::{CommandFailureDto, DiagnosticDto, PublicErrorDto};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const LABEL: &str = "tray-panel";

pub fn dismiss(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.destroy();
    }
}

pub fn handle_icon(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        rect,
        position,
        ..
    } = event
    {
        let app = tray.app_handle();
        if app.get_webview_window(LABEL).is_some() {
            dismiss(app);
            return;
        }
        if let Err(error) = open(app, rect, position) {
            eprintln!("tray panel: {error}");
        }
    }
}

fn open(
    app: &AppHandle,
    anchor: tauri::Rect,
    pointer: tauri::PhysicalPosition<f64>,
) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App("index.html?surface=tray".into()),
    )
    .title("Skill Man")
    .inner_size(370.0, 560.0)
    .visible(false)
    .decorations(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .build()?;
    if let Some(monitor) = window.monitor_from_point(pointer.x, pointer.y)? {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let origin = anchor.position.to_physical::<f64>(scale);
        let size = anchor.size.to_physical::<f64>(scale);
        let width = (370.0 * scale).min(f64::from(area.size.width));
        let height = (560.0 * scale).min(f64::from(area.size.height));
        let left = f64::from(area.position.x);
        let top = f64::from(area.position.y);
        let x = (origin.x + size.width / 2.0 - width / 2.0)
            .clamp(left, left + f64::from(area.size.width) - width);
        let y = (origin.y + size.height).clamp(top, top + f64::from(area.size.height) - height);
        window.set_size(tauri::PhysicalSize::new(width as u32, height as u32))?;
        window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32))?;
    }
    Ok(())
}

/// Called only after the panel has subscribed and rendered its effective locale.
#[tauri::command]
pub fn tray_panel_ready(window: WebviewWindow) -> Result<(), CommandFailureDto> {
    if window.label() != LABEL {
        return Err(window_failure("invalid_window"));
    }
    window.show().map_err(window_failure)?;
    window.set_focus().map_err(window_failure)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelAction {
    Close,
    OpenMain,
    Quit,
}

#[tauri::command]
pub fn tray_panel_action(
    app: AppHandle,
    window: WebviewWindow,
    action: PanelAction,
) -> Result<(), CommandFailureDto> {
    if window.label() != LABEL {
        return Err(window_failure("invalid_window"));
    }
    dismiss(&app);
    match action {
        PanelAction::Close => {}
        PanelAction::OpenMain => crate::tauri_adapter::lifecycle::show_main_window(&app),
        PanelAction::Quit => app.exit(0),
    }
    Ok(())
}

/// A click on the status item is handled by its toggle callback, not blur.
pub fn dismiss_on_blur(app: &AppHandle) {
    if !app
        .get_webview_window(LABEL)
        .is_some_and(|window| window.is_visible().unwrap_or(false))
    {
        return;
    }
    let over_icon = (|| {
        let pointer = app.cursor_position().ok()?;
        let rect = app
            .tray_by_id(crate::tauri_adapter::tray::TRAY_ID)?
            .rect()
            .ok()??;
        let panel = app.get_webview_window(LABEL)?;
        let scale = panel
            .monitor_from_point(pointer.x, pointer.y)
            .ok()??
            .scale_factor();
        let origin = rect.position.to_physical::<f64>(scale);
        let size = rect.size.to_physical::<f64>(scale);
        Some(
            pointer.x >= origin.x
                && pointer.x <= origin.x + size.width
                && pointer.y >= origin.y
                && pointer.y <= origin.y + size.height,
        )
    })()
    .unwrap_or(false);
    if !over_icon {
        dismiss(app);
    }
}

fn window_failure(error: impl std::fmt::Display) -> CommandFailureDto {
    CommandFailureDto {
        error: PublicErrorDto::Internal,
        diagnostic: Some(DiagnosticDto {
            code: "tray_window".into(),
            message: error.to_string(),
        }),
    }
}
