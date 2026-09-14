//! Native App Update presentation. One session owns checking and both confirmations;
//! no Webview or open Home is needed for a manual request.
use super::{
    app_update_api::AppUpdateApi,
    dto::*,
    native_message::{NativeMessageKey as Key, native_message},
};
use crate::core::locale::LocaleService;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Manager};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_opener::OpenerExt;

#[derive(Default)]
struct Session {
    running: bool,
    manual: bool,
    manual_pending: bool,
    presenting: bool,
    cancelled: Arc<AtomicBool>,
}
#[derive(Default)]
pub struct NativeAppUpdate {
    session: Mutex<Session>,
}
impl NativeAppUpdate {
    pub fn request(&self, force: bool) -> bool {
        let mut state = self.session.lock().expect("native update session");
        state.manual |= force;
        state.manual_pending |=
            force && (!state.presenting || state.cancelled.load(Ordering::SeqCst));
        if state.running {
            return false;
        }
        state.running = true;
        state.presenting = false;
        state.manual_pending = false;
        state.cancelled = Arc::new(AtomicBool::new(false));
        true
    }
    fn manual(&self) -> bool {
        self.session.lock().expect("native update session").manual
    }
    fn begin_check(&self) {
        let mut state = self.session.lock().expect("native update session");
        state.manual_pending = false;
        state.presenting = false;
        state.cancelled = Arc::new(AtomicBool::new(false));
    }
    pub(super) fn cancellation(&self) -> Arc<AtomicBool> {
        self.session
            .lock()
            .expect("native update session")
            .cancelled
            .clone()
    }
    pub(super) fn cancel_progress(&self, token: &Arc<AtomicBool>) {
        let mut state = self.session.lock().expect("native update session");
        token.store(true, Ordering::SeqCst);
        if Arc::ptr_eq(token, &state.cancelled) {
            // Closing cancels all intent received before this close, but later clicks survive.
            state.manual_pending = false;
        }
    }
    pub(super) fn is_checking(&self) -> bool {
        let state = self.session.lock().expect("native update session");
        state.running && !state.presenting && !state.cancelled.load(Ordering::SeqCst)
    }
    fn presenting_result(&self) {
        let mut state = self.session.lock().expect("native update session");
        if !state.cancelled.load(Ordering::SeqCst) {
            state.presenting = true;
            state.manual_pending = false;
        }
    }
    fn finish_or_retry(&self) -> bool {
        let mut state = self.session.lock().expect("native update session");
        if state.manual_pending {
            return true;
        }
        *state = Session::default();
        false
    }
}
pub(super) fn text(app: &AppHandle, key: Key, params: &[(&str, &str)]) -> String {
    native_message(
        app.state::<Arc<LocaleService>>()
            .snapshot()
            .effective_locale,
        key,
        params,
    )
}
async fn prompt(app: &AppHandle, message: String, accept: Option<Key>) -> bool {
    #[cfg(target_os = "macos")]
    {
        super::native_update_window::prompt(
            app,
            text(app, Key::CheckUpdate, &[]),
            message,
            text(app, accept.unwrap_or(Key::UpdateDone), &[]),
            accept.map(|_| text(app, Key::UpdateLater, &[])),
        )
        .await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (tx, mut rx) = tauri::async_runtime::channel(1);
        app.dialog()
            .message(message)
            .title(text(app, Key::CheckUpdate, &[]))
            .buttons(MessageDialogButtons::OkCancelCustom(
                text(app, accept.unwrap_or(Key::UpdateDone), &[]),
                text(app, Key::UpdateLater, &[]),
            ))
            .show(move |answer| {
                let _ = tx.try_send(answer);
            });
        rx.recv().await.unwrap_or(false)
    }
}
async fn failure(app: &AppHandle, error: CommandFailureDto) {
    let key = match error.error {
        PublicErrorDto::SourceUnavailable => Key::UpdateSourceFailed,
        PublicErrorDto::StateUnavailable => Key::UpdateStateFailed,
        PublicErrorDto::StaleUpdate => Key::UpdateStale,
        PublicErrorDto::DownloadFailed => Key::UpdateDownloadFailed,
        PublicErrorDto::InstallFailed => Key::UpdateInstallFailed,
        PublicErrorDto::UpdateCancelled => return,
        _ => Key::UpdateFailed,
    };
    if prompt(app, text(app, key, &[]), Some(Key::UpdateManualDownload)).await {
        let _ = app.opener().open_url(
            "https://github.com/RookieZoe/skill-man/releases/latest",
            None::<&str>,
        );
    }
}
#[tauri::command]
pub fn show_native_app_update(app: AppHandle, force: bool) {
    show(&app, force);
}

pub fn show(app: &AppHandle, force: bool) {
    if !app.state::<NativeAppUpdate>().request(force) {
        #[cfg(target_os = "macos")]
        super::native_update_window::focus(app);
        if force && app.state::<NativeAppUpdate>().is_checking() {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let cancellation = app.state::<NativeAppUpdate>().cancellation();
                progress(&app, Key::UpdateChecking, cancellation, None).await;
            });
        }
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut force = force;
        loop {
            run(&app, force).await;
            if !app.state::<NativeAppUpdate>().finish_or_retry() {
                break;
            }
            app.state::<NativeAppUpdate>().begin_check();
            force = true;
        }
    });
}

async fn run(app: &AppHandle, force: bool) {
    let api = app.state::<AppUpdateApi>();
    let cancelled = app.state::<NativeAppUpdate>().cancellation();
    let key = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|value| value.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if key.trim().is_empty() || key.trim() == "UPDATER_PUBLIC_KEY_REQUIRED_FOR_RELEASE" {
        app.state::<NativeAppUpdate>().presenting_result();
        if app.state::<NativeAppUpdate>().manual()
            && prompt(
                app,
                text(app, Key::UpdateUnsupported, &[]),
                Some(Key::UpdateManualDownload),
            )
            .await
        {
            let _ = app.opener().open_url(
                "https://github.com/RookieZoe/skill-man/releases/latest",
                None::<&str>,
            );
        }
        return;
    }
    if force && !progress(app, Key::UpdateChecking, cancelled.clone(), None).await {
        return;
    }
    let mut result = api
        .check_app_update(CheckAppUpdateRequestDto { force })
        .await;
    // An explicit request arriving during the automatic cooldown check must still check the network.
    if !force
        && matches!(result, Ok(AppUpdateCheckDto::Skipped))
        && app.state::<NativeAppUpdate>().manual()
    {
        if !progress(app, Key::UpdateChecking, cancelled.clone(), None).await {
            return;
        }
        result = api
            .check_app_update(CheckAppUpdateRequestDto { force: true })
            .await;
    }
    if !matches!(result, Ok(AppUpdateCheckDto::Skipped)) {
        app.state::<NativeAppUpdate>().presenting_result();
    }
    hide_progress(app).await;
    if cancelled.load(Ordering::SeqCst) {
        if let Ok(AppUpdateCheckDto::Available { update_id, .. }) = result {
            cancel(&api, &update_id);
        }
        return;
    }
    let (update_id, version) = match result {
        Ok(AppUpdateCheckDto::Available {
            update_id,
            current_version,
            version,
            release_notes,
            download_size_bytes,
        }) => {
            let mut notes = release_notes
                .lines()
                .take(8)
                .map(|line| {
                    line.trim_start_matches('#')
                        .trim()
                        .replace("**", "")
                        .replace('`', "")
                })
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(420)
                .collect::<String>();
            if release_notes.chars().count() > 420 || release_notes.lines().count() > 8 {
                notes.push_str("\n…");
            }
            let message = text(
                app,
                Key::UpdateOffer,
                &[
                    ("current", &current_version),
                    ("version", &version),
                    (
                        "size",
                        &format!("{:.1}", download_size_bytes as f64 / 1_048_576.0),
                    ),
                    ("notes", &notes),
                ],
            );
            if !prompt(app, message, Some(Key::UpdateDownload)).await {
                cancel(&api, &update_id);
                return;
            }
            (update_id, version)
        }
        Ok(AppUpdateCheckDto::UpToDate) => {
            if app.state::<NativeAppUpdate>().manual() {
                prompt(app, text(app, Key::UpdateCurrent, &[]), None).await;
            }
            return;
        }
        Ok(AppUpdateCheckDto::Skipped) => return,
        Err(error) => {
            if app.state::<NativeAppUpdate>().manual() {
                failure(app, error).await;
            }
            return;
        }
    };
    if !progress(
        app,
        Key::UpdateDownloading,
        cancelled.clone(),
        Some(update_id.clone()),
    )
    .await
    {
        cancel(&api, &update_id);
        return;
    }
    let downloaded = api
        .download_app_update(DownloadAppUpdateRequestDto {
            update_id: update_id.clone(),
        })
        .await;
    hide_progress(app).await;
    if cancelled.load(Ordering::SeqCst) {
        cancel(&api, &update_id);
        return;
    }
    if let Err(error) = downloaded {
        cancel(&api, &update_id);
        failure(app, error).await;
        return;
    }
    let ready = format!(
        "{}\n\n{}",
        text(app, Key::UpdateReady, &[]),
        text(app, Key::UpdateGatekeeper, &[])
    );
    if !prompt(
        app,
        format!("{version}\n\n{ready}"),
        Some(Key::UpdateInstall),
    )
    .await
    {
        cancel(&api, &update_id);
        return;
    }
    // The download and installation are deliberately separate user decisions.
    if let Err(error) = api.install_app_update(InstallAppUpdateRequestDto {
        update_id: update_id.clone(),
    }) {
        cancel(&api, &update_id);
        failure(app, error).await;
    }
}
fn cancel(api: &AppUpdateApi, update_id: &str) {
    let _ = api.cancel_app_update(CancelAppUpdateRequestDto {
        update_id: update_id.into(),
    });
}
async fn progress(
    app: &AppHandle,
    key: Key,
    cancelled: Arc<AtomicBool>,
    update_id: Option<String>,
) -> bool {
    #[cfg(target_os = "macos")]
    {
        super::native_update_window::show(app, key, cancelled, update_id)
            .await
            .is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, key, cancelled, update_id);
        true
    }
}
async fn hide_progress(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    super::native_update_window::hide(app).await;
}

pub fn refresh_locale(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    super::native_update_window::refresh_locale(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_session_coalesces_requests_and_preserves_manual_intent_during_automatic_check() {
        let controller = NativeAppUpdate::default();
        assert!(controller.request(false));
        assert!(!controller.manual());
        assert!(!controller.request(true));
        assert!(!controller.request(true));
        assert!(controller.manual());
        controller.presenting_result();
        assert!(!controller.finish_or_retry());
        assert!(controller.request(true));
        assert!(controller.manual());
    }
    #[test]
    fn manual_request_after_automatic_skip_is_retained_until_a_real_result() {
        let controller = NativeAppUpdate::default();
        assert!(controller.request(false));
        controller.begin_check();
        assert!(!controller.request(true));
        assert!(controller.finish_or_retry());
        controller.begin_check();
        controller.presenting_result();
        assert!(!controller.request(true));
        assert!(!controller.finish_or_retry());
        assert!(controller.request(false));
    }
    #[test]
    fn a_manual_retry_after_closing_progress_survives_the_cancelled_result() {
        let controller = NativeAppUpdate::default();
        assert!(controller.request(true));
        controller.begin_check();
        controller.cancel_progress(&controller.cancellation());
        assert!(!controller.is_checking());
        assert!(!controller.request(true));
        controller.presenting_result();
        assert!(controller.finish_or_retry());
        controller.begin_check();
        assert!(controller.is_checking());
        assert!(!controller.cancellation().load(Ordering::SeqCst));
    }
    #[test]
    fn closing_progress_cancels_repeated_clicks_received_before_the_close() {
        let controller = NativeAppUpdate::default();
        assert!(controller.request(true));
        assert!(!controller.request(true));
        controller.cancel_progress(&controller.cancellation());
        controller.presenting_result();
        assert!(!controller.finish_or_retry());
        assert!(controller.request(true));
    }
}
