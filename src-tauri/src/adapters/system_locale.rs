//! macOS `SystemLocaleSource` adapter (spec §4.5 seam): reads the
//! `AppleLanguages` preferred-language list from the global preferences
//! plist — the same source `NSLocale.preferredLanguages` reads. A missing or
//! unreadable plist yields an empty list; the LocaleService then falls back
//! to English. Binary and XML plists are both handled by the `plist` crate.

use std::path::PathBuf;

use crate::seams::locale_store::SystemLocaleSource;

pub const GLOBAL_PREFERENCES_FILE: &str = ".GlobalPreferences.plist";
pub const APPLE_LANGUAGES_KEY: &str = "AppleLanguages";

pub struct MacOsSystemLocaleSource {
    preferences_plist: PathBuf,
}

impl MacOsSystemLocaleSource {
    pub fn new(home_directory: PathBuf) -> Self {
        Self {
            preferences_plist: home_directory
                .join("Library/Preferences")
                .join(GLOBAL_PREFERENCES_FILE),
        }
    }
}

impl SystemLocaleSource for MacOsSystemLocaleSource {
    fn preferred_language_tags(&self) -> Vec<String> {
        let Ok(value) = plist::Value::from_file(&self.preferences_plist) else {
            return Vec::new();
        };
        let Some(dictionary) = value.as_dictionary() else {
            return Vec::new();
        };
        let Some(plist::Value::Array(languages)) = dictionary.get(APPLE_LANGUAGES_KEY) else {
            return Vec::new();
        };
        languages
            .iter()
            .filter_map(|entry| match entry {
                plist::Value::String(tag) if !tag.is_empty() => Some(tag.clone()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn missing_plist_yields_empty_list() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source = MacOsSystemLocaleSource::new(dir.path().to_path_buf());
        assert_eq!(source.preferred_language_tags(), Vec::<String>::new());
    }

    #[test]
    fn reads_apple_languages_in_order_and_skips_non_strings() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prefs = dir.path().join("Library/Preferences");
        fs::create_dir_all(&prefs).expect("prefs dir");
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            APPLE_LANGUAGES_KEY.into(),
            plist::Value::Array(vec![
                plist::Value::String("zh-Hans-CN".into()),
                plist::Value::Integer(plist::Integer::from(7)),
                plist::Value::String("en-US".into()),
            ]),
        );
        plist::Value::Dictionary(dictionary)
            .to_file_xml(prefs.join(GLOBAL_PREFERENCES_FILE))
            .expect("write plist");

        let source = MacOsSystemLocaleSource::new(dir.path().to_path_buf());
        assert_eq!(
            source.preferred_language_tags(),
            vec!["zh-Hans-CN".to_string(), "en-US".to_string()]
        );
    }
}
