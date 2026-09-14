//! App-level appearance authority, shared by native menus and webviews.
use crate::adapters::app_state_store::write_atomic;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceSelection {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSnapshot {
    pub selection: AppearanceSelection,
    pub generation: u64,
}

pub struct AppearanceApi {
    directory: PathBuf,
    state: Mutex<AppearanceSnapshot>,
}

impl AppearanceApi {
    pub fn open(directory: PathBuf) -> Self {
        let selection = std::fs::read(directory.join("appearance.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            directory,
            state: Mutex::new(AppearanceSnapshot {
                selection,
                generation: 0,
            }),
        }
    }
    pub fn snapshot(&self) -> AppearanceSnapshot {
        self.state.lock().expect("appearance lock").clone()
    }
    pub fn migrate_legacy(&self, legacy: Option<&str>) -> Result<AppearanceSnapshot, String> {
        let mut state = self.state.lock().expect("appearance lock");
        match std::fs::read(self.directory.join("appearance.json")) {
            Ok(bytes) => {
                let _: AppearanceSelection =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                return Ok(state.clone());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        let selection = match legacy {
            Some("light") => AppearanceSelection::Light,
            Some("dark") => AppearanceSelection::Dark,
            _ => AppearanceSelection::System,
        };
        self.persist(&mut state, selection)
    }
    pub fn set_selection(
        &self,
        selection: AppearanceSelection,
    ) -> Result<AppearanceSnapshot, String> {
        let mut state = self.state.lock().expect("appearance lock");
        self.persist(&mut state, selection)
    }
    fn persist(
        &self,
        state: &mut AppearanceSnapshot,
        selection: AppearanceSelection,
    ) -> Result<AppearanceSnapshot, String> {
        write_atomic(
            &self.directory,
            "appearance.json",
            serde_json::to_string(&selection).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if state.selection != selection {
            state.generation += 1;
        }
        state.selection = selection;
        Ok(state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_survives_closing_and_reopening_app_state() {
        let dir = tempfile::tempdir().unwrap();
        {
            let api = AppearanceApi::open(dir.path().to_owned());
            api.set_selection(AppearanceSelection::Dark).unwrap();
        }
        let reopened = AppearanceApi::open(dir.path().to_owned());
        assert_eq!(reopened.snapshot().selection, AppearanceSelection::Dark);
    }
    #[test]
    fn legacy_migration_never_overwrites_an_explicit_native_choice() {
        let dir = tempfile::tempdir().unwrap();
        let api = AppearanceApi::open(dir.path().to_owned());
        api.set_selection(AppearanceSelection::Light).unwrap();
        assert_eq!(
            api.migrate_legacy(Some("dark")).unwrap().selection,
            AppearanceSelection::Light
        );
        let fresh = tempfile::tempdir().unwrap();
        let api = AppearanceApi::open(fresh.path().to_owned());
        assert_eq!(
            api.migrate_legacy(Some("dark")).unwrap().selection,
            AppearanceSelection::Dark
        );
        assert_eq!(
            api.migrate_legacy(Some("light")).unwrap().selection,
            AppearanceSelection::Dark
        );
    }

    #[test]
    fn duplicate_choice_does_not_publish_a_new_generation_and_failure_keeps_state() {
        let dir = tempfile::tempdir().unwrap();
        let api = AppearanceApi::open(dir.path().to_owned());
        let chosen = api.set_selection(AppearanceSelection::Dark).unwrap();
        assert_eq!(
            api.set_selection(AppearanceSelection::Dark).unwrap(),
            chosen
        );
        std::fs::create_dir(dir.path().join("appearance.json.tmp")).unwrap();
        assert!(api.set_selection(AppearanceSelection::Light).is_err());
        assert_eq!(api.snapshot(), chosen);
        assert_eq!(
            AppearanceApi::open(dir.path().to_owned())
                .snapshot()
                .selection,
            AppearanceSelection::Dark
        );
    }
    #[test]
    fn invalid_legacy_falls_back_and_failed_migration_can_retry() {
        let dir = tempfile::tempdir().unwrap();
        let api = AppearanceApi::open(dir.path().to_owned());
        std::fs::create_dir(dir.path().join("appearance.json.tmp")).unwrap();
        assert!(api.migrate_legacy(Some("dark")).is_err());
        assert_eq!(api.snapshot().selection, AppearanceSelection::System);
        std::fs::remove_dir(dir.path().join("appearance.json.tmp")).unwrap();
        assert_eq!(
            api.migrate_legacy(Some("unknown")).unwrap().selection,
            AppearanceSelection::System
        );
        let reopened = AppearanceApi::open(dir.path().to_owned());
        assert_eq!(
            reopened.migrate_legacy(Some("dark")).unwrap().selection,
            AppearanceSelection::System
        );
    }

    #[test]
    fn concurrent_choices_and_migration_agree_with_reopened_storage() {
        let dir = tempfile::tempdir().unwrap();
        let api = std::sync::Arc::new(AppearanceApi::open(dir.path().to_owned()));
        std::thread::scope(|scope| {
            for selection in [
                AppearanceSelection::Dark,
                AppearanceSelection::Light,
                AppearanceSelection::System,
            ] {
                let api = api.clone();
                scope.spawn(move || {
                    api.set_selection(selection).unwrap();
                });
            }
            let api = api.clone();
            scope.spawn(move || {
                api.migrate_legacy(Some("dark")).unwrap();
            });
        });
        let snapshot = api.snapshot();
        drop(api);
        assert_eq!(
            AppearanceApi::open(dir.path().to_owned())
                .snapshot()
                .selection,
            snapshot.selection
        );
    }
}

fn failure(message: String) -> crate::tauri_adapter::dto::CommandFailureDto {
    use crate::tauri_adapter::dto::{CommandFailureDto, DiagnosticDto, PublicErrorDto};
    CommandFailureDto {
        error: PublicErrorDto::StateUnavailable,
        diagnostic: Some(DiagnosticDto {
            code: "appearance_store_unavailable".into(),
            message,
        }),
    }
}

pub fn publish(app: &tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let snapshot = handle.state::<AppearanceApi>().snapshot();
        handle.set_theme(match snapshot.selection {
            AppearanceSelection::System => None,
            AppearanceSelection::Light => Some(tauri::Theme::Light),
            AppearanceSelection::Dark => Some(tauri::Theme::Dark),
        });
        let _ = handle.emit("appearance://changed", snapshot);
        crate::tauri_adapter::tray::refresh_tray(&handle);
    });
}

#[tauri::command]
pub fn get_appearance_snapshot(api: tauri::State<'_, AppearanceApi>) -> AppearanceSnapshot {
    api.snapshot()
}
#[tauri::command]
pub fn set_appearance_selection(
    app: tauri::AppHandle,
    api: tauri::State<'_, AppearanceApi>,
    selection: AppearanceSelection,
) -> Result<AppearanceSnapshot, crate::tauri_adapter::dto::CommandFailureDto> {
    let snapshot = api.set_selection(selection).map_err(failure)?;
    publish(&app);
    Ok(snapshot)
}
#[tauri::command]
pub fn migrate_appearance_legacy(
    app: tauri::AppHandle,
    api: tauri::State<'_, AppearanceApi>,
    legacy: Option<String>,
) -> Result<AppearanceSnapshot, crate::tauri_adapter::dto::CommandFailureDto> {
    let snapshot = api.migrate_legacy(legacy.as_deref()).map_err(failure)?;
    publish(&app);
    Ok(snapshot)
}
