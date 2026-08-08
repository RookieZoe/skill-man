//! Preferences module (spec §10.2): exactly four persisted switches with
//! fixed defaults. The service is UI-independent; runtime side effects
//! (login item, Dock activation policy) live in the Tauri adapter layer.

use std::sync::Arc;

use thiserror::Error;

use crate::seams::preferences_store::{
    AppPreferences, PreferenceUpdates, PreferencesStore, PreferencesStoreError,
};

#[derive(Clone)]
pub struct PreferencesService {
    store: Arc<dyn PreferencesStore>,
}

impl PreferencesService {
    pub fn new(store: Arc<dyn PreferencesStore>) -> Self {
        Self { store }
    }

    pub fn load(&self) -> Result<AppPreferences, PreferencesError> {
        Ok(self.store.load_preferences()?)
    }

    pub fn update(&self, updates: PreferenceUpdates) -> Result<AppPreferences, PreferencesError> {
        Ok(self.store.update_preferences(updates)?)
    }
}

#[derive(Debug, Error)]
pub enum PreferencesError {
    #[error(transparent)]
    Store(#[from] PreferencesStoreError),
}
