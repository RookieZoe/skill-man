//! System `LocaleStore` adapter: persists `locale.json` in the app-level
//! state directory (outside any Home, ADR-0011) with the same
//! tmp → fsync → rename → parent fsync protocol as the bootstrap locator.
//! A missing file is "never chosen" (System mode); a corrupt file or unknown
//! selection is a hard error the LocaleService turns into the safe `en`
//! baseline plus a diagnostic — never a guess.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::adapters::app_state_store::write_atomic;
use crate::seams::locale_store::{LocaleSelection, LocaleStore, LocaleStoreError};

pub const LOCALE_FILE_NAME: &str = "locale.json";
pub const LOCALE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocaleFile {
    schema_version: u32,
    #[serde(rename = "selection")]
    selection: LocaleSelectionSerde,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum LocaleSelectionSerde {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-Hans")]
    ZhHans,
}

impl From<LocaleSelection> for LocaleSelectionSerde {
    fn from(value: LocaleSelection) -> Self {
        match value {
            LocaleSelection::System => Self::System,
            LocaleSelection::En => Self::En,
            LocaleSelection::ZhHans => Self::ZhHans,
        }
    }
}

impl From<LocaleSelectionSerde> for LocaleSelection {
    fn from(value: LocaleSelectionSerde) -> Self {
        match value {
            LocaleSelectionSerde::System => Self::System,
            LocaleSelectionSerde::En => Self::En,
            LocaleSelectionSerde::ZhHans => Self::ZhHans,
        }
    }
}

pub struct LocaleStoreFileSystem {
    state_dir: PathBuf,
}

impl LocaleStoreFileSystem {
    pub fn new(state_dir: PathBuf) -> Self {
        Self { state_dir }
    }

    fn path(&self) -> PathBuf {
        self.state_dir.join(LOCALE_FILE_NAME)
    }
}

impl LocaleStore for LocaleStoreFileSystem {
    fn load_selection(&self) -> Result<Option<LocaleSelection>, LocaleStoreError> {
        let path = self.path();
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(LocaleStoreError::Unavailable(format!(
                    "{}: {error}",
                    path.display()
                )));
            }
        };
        let parsed: LocaleFile = serde_json::from_str(&content)
            .map_err(|error| LocaleStoreError::Invalid(format!("{}: {error}", path.display())))?;
        if parsed.schema_version != LOCALE_SCHEMA_VERSION {
            return Err(LocaleStoreError::Invalid(format!(
                "{}: unsupported schema version {}",
                path.display(),
                parsed.schema_version
            )));
        }
        Ok(Some(parsed.selection.into()))
    }

    fn store_selection(&self, selection: LocaleSelection) -> Result<(), LocaleStoreError> {
        let file = LocaleFile {
            schema_version: LOCALE_SCHEMA_VERSION,
            selection: selection.into(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|error| LocaleStoreError::Unavailable(error.to_string()))?;
        write_atomic(&self.state_dir, LOCALE_FILE_NAME, json)
            .map_err(|error| LocaleStoreError::Unavailable(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_never_chosen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = LocaleStoreFileSystem::new(dir.path().join("state"));
        assert_eq!(store.load_selection().expect("read"), None);
    }

    #[test]
    fn store_then_load_round_trips_all_three_selections() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = LocaleStoreFileSystem::new(dir.path().join("state"));
        for selection in [
            LocaleSelection::System,
            LocaleSelection::En,
            LocaleSelection::ZhHans,
        ] {
            store.store_selection(selection).expect("persist");
            assert_eq!(
                store.load_selection().expect("read"),
                Some(selection),
                "round trip for {selection:?}"
            );
        }
        // The tmp file is gone; only the committed name exists.
        assert!(!dir.path().join("state/locale.json.tmp").exists());
        assert!(dir.path().join("state/locale.json").is_file());
    }

    #[test]
    fn corrupt_or_unknown_values_are_hard_errors_not_guesses() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = dir.path().join("state");
        fs::create_dir_all(&state).expect("state dir");
        let store = LocaleStoreFileSystem::new(state.clone());

        fs::write(state.join(LOCALE_FILE_NAME), "{ not json").expect("corrupt");
        assert!(matches!(
            store.load_selection(),
            Err(LocaleStoreError::Invalid(_))
        ));

        fs::write(
            state.join(LOCALE_FILE_NAME),
            r#"{"schemaVersion":1,"selection":"fr"}"#,
        )
        .expect("unknown selection");
        assert!(matches!(
            store.load_selection(),
            Err(LocaleStoreError::Invalid(_))
        ));

        fs::write(
            state.join(LOCALE_FILE_NAME),
            r#"{"schemaVersion":99,"selection":"en"}"#,
        )
        .expect("bad schema");
        assert!(matches!(
            store.load_selection(),
            Err(LocaleStoreError::Invalid(_))
        ));
    }
}
