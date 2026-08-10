//! Preferences seam (spec §10.2): exactly four persisted switches with fixed
//! defaults. The single-row `preferences` table in SQLite is the only
//! implementation; the seam follows the store pattern of the other modules
//! and keeps the persisted surface narrow.

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPreferences {
    pub launch_at_login: bool,
    pub show_in_dock: bool,
    pub check_app_updates: bool,
    pub check_skill_updates: bool,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            show_in_dock: true,
            check_app_updates: true,
            check_skill_updates: true,
        }
    }
}

/// Partial update: `None` fields keep their persisted value. The type itself
/// has exactly the four spec'd fields, so the "strictly four" rule cannot be
/// violated through this surface.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreferenceUpdates {
    pub launch_at_login: Option<bool>,
    pub show_in_dock: Option<bool>,
    pub check_app_updates: Option<bool>,
    pub check_skill_updates: Option<bool>,
}

#[derive(Debug, Error)]
pub enum PreferencesStoreError {
    #[error("the Preferences state could not be read or written: {0}")]
    Unavailable(String),
}

pub trait PreferencesStore: Send + Sync {
    fn load_preferences(&self) -> Result<AppPreferences, PreferencesStoreError>;

    fn update_preferences(
        &self,
        updates: PreferenceUpdates,
    ) -> Result<AppPreferences, PreferencesStoreError>;

    /// Unix epoch seconds of the last successful App Update check. This is
    /// operational cooldown state, not a fifth user-visible Preference.
    fn last_app_update_check_at(&self) -> Result<Option<i64>, PreferencesStoreError>;

    fn record_app_update_check_at(&self, checked_at: i64) -> Result<(), PreferencesStoreError>;
}
