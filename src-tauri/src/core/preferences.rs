//! Preferences module (spec §10.2): exactly four persisted switches with
//! fixed defaults. The service is UI-independent; runtime side effects
//! (login item, Dock activation policy) live in the Tauri adapter layer.

use std::sync::Arc;

use thiserror::Error;

use crate::core::write_gate::{ProductWriteGuard, WriteGate};
use crate::seams::preferences_store::{
    AppPreferences, PreferenceUpdates, PreferencesStore, PreferencesStoreError,
};

#[derive(Clone)]
pub struct PreferencesService {
    store: Arc<dyn PreferencesStore>,
    write_gate: Arc<WriteGate>,
}

impl PreferencesService {
    pub fn new(store: Arc<dyn PreferencesStore>, write_gate: Arc<WriteGate>) -> Self {
        Self { store, write_gate }
    }

    pub fn load(&self) -> Result<AppPreferences, PreferencesError> {
        Ok(self.store.load_preferences()?)
    }

    pub fn update(&self, updates: PreferenceUpdates) -> Result<AppPreferences, PreferencesError> {
        let _write_guard = self.acquire_write_guard()?;
        Ok(self.store.update_preferences(updates)?)
    }

    fn acquire_write_guard(&self) -> Result<ProductWriteGuard<'_>, PreferencesError> {
        let context = self
            .write_gate
            .capture_open_context()
            .map_err(|_| PreferencesError::WriteGateClosed)?;
        self.write_gate
            .acquire_product_write(&context)
            .map_err(|_| PreferencesError::WriteGateClosed)
    }
}

#[derive(Debug, Error)]
pub enum PreferencesError {
    #[error(transparent)]
    Store(#[from] PreferencesStoreError),
    #[error("the write gate is closed for product writes")]
    WriteGateClosed,
}
