//! Locale Tauri API (spec §4.5, §4.7; ADR-0011): `get_locale_snapshot`,
//! `set_locale_selection` and `refresh_system_languages` plus the
//! `locale://changed` event. The payload is isomorphic to the query snapshot
//! and carries the generation. `set_locale_selection` is persist-then-publish
//! on the LocaleService: a failed persist returns a closed error and emits
//! nothing, so React, tray and the native menu keep their old generation.

use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::core::locale::{LocaleError, LocaleService};
use crate::tauri_adapter::dto::{
    CommandFailureDto, DiagnosticDto, LocaleSnapshotDto, PublicErrorDto,
    SetLocaleSelectionRequestDto,
};

pub const LOCALE_CHANGED_EVENT: &str = "locale://changed";

/// Emitter seam so API tests capture payloads without a Tauri runtime.
pub trait LocaleChangedEmitter: Send + Sync {
    fn emit_changed(&self, payload: &LocaleSnapshotDto);
}

pub struct TauriLocaleChangedEmitter {
    app: AppHandle,
}

impl TauriLocaleChangedEmitter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl LocaleChangedEmitter for TauriLocaleChangedEmitter {
    fn emit_changed(&self, payload: &LocaleSnapshotDto) {
        let _ = self.app.emit(LOCALE_CHANGED_EVENT, payload);
    }
}

pub struct LocaleApi {
    service: LocaleService,
    emitter: Arc<dyn LocaleChangedEmitter>,
}

impl LocaleApi {
    pub fn new(service: LocaleService, emitter: Arc<dyn LocaleChangedEmitter>) -> Self {
        Self { service, emitter }
    }

    pub fn get_locale_snapshot(&self) -> Result<LocaleSnapshotDto, CommandFailureDto> {
        Ok(self.service.snapshot().into())
    }

    pub fn set_locale_selection(
        &self,
        request: SetLocaleSelectionRequestDto,
    ) -> Result<LocaleSnapshotDto, CommandFailureDto> {
        let snapshot = self
            .service
            .set_selection(request.selection)
            .map_err(locale_command_error)?;
        let payload: LocaleSnapshotDto = snapshot.into();
        self.emitter.emit_changed(&payload);
        Ok(payload)
    }

    pub fn refresh_system_languages(&self) -> Result<LocaleSnapshotDto, CommandFailureDto> {
        let payload: LocaleSnapshotDto = self.service.refresh_system_languages().into();
        self.emitter.emit_changed(&payload);
        Ok(payload)
    }
}

fn locale_command_error(error: LocaleError) -> CommandFailureDto {
    CommandFailureDto {
        error: PublicErrorDto::LocaleStoreUnavailable,
        diagnostic: Some(DiagnosticDto {
            code: "locale_store_unavailable".into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use crate::core::locale::LocaleService;
    use crate::seams::locale_store::{
        EffectiveLocale, LocaleSelection, LocaleStore, LocaleStoreError, SystemLocaleSource,
    };
    use crate::tauri_adapter::dto::LocaleSnapshotDto;

    use super::{LocaleApi, LocaleChangedEmitter};

    struct CapturingEmitter(Mutex<Vec<LocaleSnapshotDto>>);

    impl LocaleChangedEmitter for CapturingEmitter {
        fn emit_changed(&self, payload: &LocaleSnapshotDto) {
            self.0.lock().expect("lock").push(payload.clone());
        }
    }

    struct MemoryStore {
        value: Mutex<Option<LocaleSelection>>,
        fail: Mutex<bool>,
    }

    impl LocaleStore for MemoryStore {
        fn load_selection(&self) -> Result<Option<LocaleSelection>, LocaleStoreError> {
            Ok(*self.value.lock().expect("lock"))
        }

        fn store_selection(&self, selection: LocaleSelection) -> Result<(), LocaleStoreError> {
            if *self.fail.lock().expect("lock") {
                return Err(LocaleStoreError::Unavailable("boom".into()));
            }
            *self.value.lock().expect("lock") = Some(selection);
            Ok(())
        }
    }

    struct FixedSource;

    impl SystemLocaleSource for FixedSource {
        fn preferred_language_tags(&self) -> Vec<String> {
            vec!["en-US".into()]
        }
    }

    fn api() -> (LocaleApi, Arc<CapturingEmitter>, Arc<MemoryStore>) {
        let store = Arc::new(MemoryStore {
            value: Mutex::new(None),
            fail: Mutex::new(false),
        });
        let emitter = Arc::new(CapturingEmitter(Mutex::new(Vec::new())));
        let service = LocaleService::new(store.clone(), Arc::new(FixedSource));
        (LocaleApi::new(service, emitter.clone()), emitter, store)
    }

    #[test]
    fn query_returns_the_published_snapshot() {
        let (api, _, _) = api();
        let snapshot = api.get_locale_snapshot().expect("query");
        assert_eq!(snapshot.selection, LocaleSelection::System);
        assert_eq!(snapshot.effective_locale, EffectiveLocale::En);
        assert_eq!(snapshot.generation, 0);
        assert!(snapshot.diagnostic.is_none());
    }

    #[test]
    fn successful_switch_emits_isomorphic_payload_with_new_generation() {
        let (api, emitter, _) = api();
        let snapshot = api
            .set_locale_selection(crate::tauri_adapter::dto::SetLocaleSelectionRequestDto {
                selection: LocaleSelection::ZhHans,
            })
            .expect("switch");
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.effective_locale, EffectiveLocale::ZhHans);
        let events = emitter.0.lock().expect("lock");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], snapshot);
    }

    #[test]
    fn failed_persist_returns_closed_error_and_emits_nothing() {
        let (api, emitter, store) = api();
        *store.fail.lock().expect("lock") = true;
        let failure = api
            .set_locale_selection(crate::tauri_adapter::dto::SetLocaleSelectionRequestDto {
                selection: LocaleSelection::En,
            })
            .expect_err("must fail");
        assert!(matches!(
            failure.error,
            super::PublicErrorDto::LocaleStoreUnavailable
        ));
        assert!(failure.diagnostic.is_some());
        assert!(emitter.0.lock().expect("lock").is_empty());
        // The snapshot and its generation are unchanged.
        let snapshot = api.get_locale_snapshot().expect("query");
        assert_eq!(snapshot.generation, 0);
        assert_eq!(snapshot.selection, LocaleSelection::System);
    }
}
