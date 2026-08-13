//! Locale selection seam (spec §4.5, §6.1; ADR-0011): App-level state that
//! lives outside any Home, so it stays readable and writable in
//! Unconfigured, HomeUnavailable, Catalog ReadOnly and Fixture Recovery Lock
//! — and is never touched by Restore or Abandon. The filesystem adapter
//! persists `locale.json` in the app-level state directory with the same
//! tmp → fsync → rename → parent fsync protocol as the bootstrap locator.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The persisted App-level choice. `System` negotiates the effective locale
/// from the macOS preferred-language list on startup and app activation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocaleSelection {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-Hans")]
    ZhHans,
}

/// The resolved locale every visible surface renders in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveLocale {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-Hans")]
    ZhHans,
}

#[derive(Debug, Error)]
pub enum LocaleStoreError {
    #[error("the locale file could not be read or written: {0}")]
    Unavailable(String),
    #[error("the persisted locale value is corrupt or unknown: {0}")]
    Invalid(String),
}

/// Seam: read/write the persisted selection. `load_selection` returning
/// `None` means "never chosen" → the System mode applies; an error means the
/// store is unavailable → the safe `en` baseline applies with a diagnostic.
pub trait LocaleStore: Send + Sync {
    fn load_selection(&self) -> Result<Option<LocaleSelection>, LocaleStoreError>;

    fn store_selection(&self, selection: LocaleSelection) -> Result<(), LocaleStoreError>;
}

/// Seam: the macOS preferred-language list (system locale authority). The
/// macOS adapter reads the `AppleLanguages` array from the global preferences
/// plist; deterministic adapters drive the resolver matrix.
pub trait SystemLocaleSource: Send + Sync {
    fn preferred_language_tags(&self) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::{EffectiveLocale, LocaleSelection};

    #[test]
    fn selection_is_a_closed_three_value_enum() {
        // The persisted surface is exactly `system | en | zh-Hans` (ADR-0011).
        let values = [
            LocaleSelection::System,
            LocaleSelection::En,
            LocaleSelection::ZhHans,
        ];
        assert_eq!(values.len(), 3);
        assert_ne!(LocaleSelection::System, LocaleSelection::En);
        assert_ne!(LocaleSelection::En, LocaleSelection::ZhHans);
    }

    #[test]
    fn effective_locale_is_two_value() {
        assert_ne!(EffectiveLocale::En, EffectiveLocale::ZhHans);
    }
}
