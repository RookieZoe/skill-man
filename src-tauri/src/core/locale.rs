//! Locale Authority module (spec §4.5, §6.1; ADR-0011): the single App-level
//! locale authority in the native composition root. It owns the persisted
//! selection, resolves the effective locale from the macOS preferred-language
//! list, and publishes a typed snapshot with a generation that increments on
//! every published change. `set_selection` is persist-then-publish: a failed
//! persist leaves selection, effective locale, generation and every visible
//! surface unchanged and returns a structured error.
//!
//! React, tray and the native menu only consume this state; they never
//! negotiate locale among themselves. The service holds no Home reference, so
//! locale stays readable and writable in Unconfigured, HomeUnavailable,
//! Catalog ReadOnly and Fixture Recovery Lock, and Restore/Abandon never
//! touch it.

use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::seams::locale_store::{
    EffectiveLocale, LocaleSelection, LocaleStore, LocaleStoreError, SystemLocaleSource,
};

/// A published locale snapshot (spec §4.5). `diagnostic` records the safe
/// `en` baseline reason when the persisted value was corrupt or the store
/// was unreadable; it is raw technical detail, never App Copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocaleSnapshot {
    pub selection: LocaleSelection,
    pub effective_locale: EffectiveLocale,
    pub generation: u64,
    pub diagnostic: Option<LocaleDiagnostic>,
}

/// Raw fallback facts; presentation never turns this into copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocaleDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum LocaleError {
    #[error("the locale selection could not be persisted: {0}")]
    StoreUnavailable(#[from] LocaleStoreError),
}

#[derive(Clone)]
struct LocaleState {
    selection: LocaleSelection,
    effective_locale: EffectiveLocale,
    generation: u64,
    diagnostic: Option<LocaleDiagnostic>,
    /// The store was unreadable at load: the safe `en` baseline stays in
    /// force until a persist proves recovery (ADR-0011) — re-negotiation
    /// must not silently flip a corrupt-store session to a system language.
    store_unavailable: bool,
}
#[derive(Clone)]
pub struct LocaleService {
    store: Arc<dyn LocaleStore>,
    system: Arc<dyn SystemLocaleSource>,
    // Shared snapshot state: every clone of the service is the SAME
    // authority (ADR-0011) — the LocaleApi, tray/menu listeners and the run
    // loop must never fork the state or native surfaces freeze at the
    // startup locale.
    state: Arc<Mutex<LocaleState>>,
}

/// ADR-0011 preferred-language matching. Returns `None` for languages that
/// must be skipped so later preferred languages get their turn (Hant variants
/// never map to 简体中文); the caller falls back to English at the end.
fn match_tag(tag: &str) -> Option<EffectiveLocale> {
    let mut parts = tag.split('-');
    let language = parts.next()?.to_ascii_lowercase();
    match language.as_str() {
        "en" => Some(EffectiveLocale::En),
        "zh" => match parts.next() {
            None => Some(EffectiveLocale::ZhHans),
            Some(qualifier) => {
                let lower = qualifier.to_ascii_lowercase();
                if lower.starts_with("hant") {
                    // Skip: keep matching later preferred languages.
                    None
                } else if lower.starts_with("hans") {
                    Some(EffectiveLocale::ZhHans)
                } else {
                    // A region without a script.
                    match lower.as_str() {
                        "tw" | "hk" | "mo" => None,
                        // `zh-CN`, `zh-SG` and any other script-less zh
                        // variant are simplified Chinese.
                        _ => Some(EffectiveLocale::ZhHans),
                    }
                }
            }
        },
        _ => None,
    }
}

/// Resolver matrix (ADR-0011): iterate the preferred list in order; Hant
/// variants are skipped, later languages may hit, and English is the final
/// fallback.
pub fn resolve_effective(preferred_language_tags: &[String]) -> EffectiveLocale {
    for tag in preferred_language_tags {
        if let Some(locale) = match_tag(tag) {
            return locale;
        }
    }
    EffectiveLocale::En
}

impl LocaleService {
    pub fn new(store: Arc<dyn LocaleStore>, system: Arc<dyn SystemLocaleSource>) -> Self {
        // ADR-0011: no persisted value negotiates as System; a corrupt value
        // or unreadable store falls back to the safe `en` baseline with a
        // diagnostic — never to system negotiation.
        let (selection, effective_locale, store_unavailable, diagnostic) =
            match store.load_selection() {
                Ok(Some(selection)) => (
                    selection,
                    effective_for(selection, &system.preferred_language_tags()),
                    false,
                    None,
                ),
                Ok(None) => (
                    LocaleSelection::System,
                    effective_for(LocaleSelection::System, &system.preferred_language_tags()),
                    false,
                    None,
                ),
                Err(error) => (
                    LocaleSelection::System,
                    EffectiveLocale::En,
                    true,
                    Some(LocaleDiagnostic {
                        code: "locale_store_unavailable".into(),
                        message: error.to_string(),
                    }),
                ),
            };
        Self {
            store,
            system,
            state: Arc::new(Mutex::new(LocaleState {
                selection,
                effective_locale,
                generation: 0,
                diagnostic,
                store_unavailable,
            })),
        }
    }

    pub fn snapshot(&self) -> LocaleSnapshot {
        let state = self.state.lock().expect("locale state lock");
        snapshot_from(&state)
    }

    /// Persist-then-publish: the store write is the commit point. A failed
    /// persist returns an error and changes nothing — selection, effective
    /// locale, generation and all visible surfaces keep their old values.
    pub fn set_selection(&self, selection: LocaleSelection) -> Result<LocaleSnapshot, LocaleError> {
        self.store.store_selection(selection)?;
        let mut state = self.state.lock().expect("locale state lock");
        state.selection = selection;
        state.effective_locale = effective_for(selection, &self.system.preferred_language_tags());
        state.generation += 1;
        // A successful persist proves the store is healthy; the fallback
        // diagnostic no longer applies.
        state.diagnostic = None;
        Ok(snapshot_from(&state))
    }

    /// Re-negotiate the effective locale from the current system preferred
    /// list (app activation and next startup). Only a published change bumps
    /// the generation.
    pub fn refresh_system_languages(&self) -> LocaleSnapshot {
        let mut state = self.state.lock().expect("locale state lock");
        // The safe `en` baseline is sticky while the store is unavailable;
        // only a successful persist clears it.
        if state.store_unavailable {
            return snapshot_from(&state);
        }
        let effective = effective_for(state.selection, &self.system.preferred_language_tags());
        if effective != state.effective_locale {
            state.effective_locale = effective;
            state.generation += 1;
        }
        snapshot_from(&state)
    }
}

fn snapshot_from(state: &LocaleState) -> LocaleSnapshot {
    LocaleSnapshot {
        selection: state.selection,
        effective_locale: state.effective_locale,
        generation: state.generation,
        diagnostic: state.diagnostic.clone(),
    }
}

fn effective_for(selection: LocaleSelection, preferred: &[String]) -> EffectiveLocale {
    match selection {
        LocaleSelection::System => resolve_effective(preferred),
        LocaleSelection::En => EffectiveLocale::En,
        LocaleSelection::ZhHans => EffectiveLocale::ZhHans,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{
        EffectiveLocale, LocaleSelection, LocaleService, LocaleSnapshot, match_tag,
        resolve_effective,
    };
    use crate::seams::locale_store::{LocaleStore, LocaleStoreError, SystemLocaleSource};

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|tag| tag.to_string()).collect()
    }

    struct FixedSource(Mutex<Vec<String>>);

    impl SystemLocaleSource for FixedSource {
        fn preferred_language_tags(&self) -> Vec<String> {
            self.0.lock().expect("lock").clone()
        }
    }

    struct MemoryStore {
        value: Mutex<Option<LocaleSelection>>,
        fail_writes: Mutex<bool>,
        fail_reads: Mutex<bool>,
    }

    impl MemoryStore {
        fn new(value: Option<LocaleSelection>) -> Self {
            Self {
                value: Mutex::new(value),
                fail_writes: Mutex::new(false),
                fail_reads: Mutex::new(false),
            }
        }
    }

    impl LocaleStore for MemoryStore {
        fn load_selection(&self) -> Result<Option<LocaleSelection>, LocaleStoreError> {
            if *self.fail_reads.lock().expect("lock") {
                return Err(LocaleStoreError::Unavailable("boom".into()));
            }
            Ok(*self.value.lock().expect("lock"))
        }

        fn store_selection(&self, selection: LocaleSelection) -> Result<(), LocaleStoreError> {
            if *self.fail_writes.lock().expect("lock") {
                return Err(LocaleStoreError::Unavailable("disk full".into()));
            }
            *self.value.lock().expect("lock") = Some(selection);
            Ok(())
        }
    }

    fn service(
        store: std::sync::Arc<MemoryStore>,
        source: std::sync::Arc<FixedSource>,
    ) -> LocaleService {
        LocaleService::new(store, source)
    }

    #[test]
    fn resolver_matrix_covers_preferred_order() {
        // First hit wins.
        assert_eq!(
            resolve_effective(&tags(&["en-US", "zh-CN"])),
            EffectiveLocale::En
        );
        assert_eq!(
            resolve_effective(&tags(&["zh-CN", "en-US"])),
            EffectiveLocale::ZhHans
        );
    }

    #[test]
    fn resolver_maps_hans_variants_and_skips_hant() {
        assert_eq!(resolve_effective(&tags(&["zh"])), EffectiveLocale::ZhHans);
        assert_eq!(
            resolve_effective(&tags(&["zh-Hans"])),
            EffectiveLocale::ZhHans
        );
        assert_eq!(
            resolve_effective(&tags(&["zh-Hans-CN"])),
            EffectiveLocale::ZhHans
        );
        assert_eq!(
            resolve_effective(&tags(&["zh-CN"])),
            EffectiveLocale::ZhHans
        );
        assert_eq!(
            resolve_effective(&tags(&["zh-SG"])),
            EffectiveLocale::ZhHans
        );
        assert_eq!(
            resolve_effective(&tags(&["zh-hans-cn"])),
            EffectiveLocale::ZhHans
        );
    }

    #[test]
    fn resolver_skips_hant_then_continues_and_falls_back_to_english() {
        // Hant never maps to simplified; later languages still get a turn.
        assert_eq!(
            resolve_effective(&tags(&["zh-Hant-TW", "en-GB"])),
            EffectiveLocale::En
        );
        assert_eq!(
            resolve_effective(&tags(&["zh-Hant", "zh-Hans"])),
            EffectiveLocale::ZhHans
        );
        for tag in ["zh-TW", "zh-HK", "zh-MO", "zh-Hant-HK"] {
            assert_eq!(match_tag(tag), None, "{tag} must be skipped");
        }
        // No match at all → English fallback.
        assert_eq!(
            resolve_effective(&tags(&["fr-FR", "de-DE"])),
            EffectiveLocale::En
        );
        assert_eq!(resolve_effective(&[]), EffectiveLocale::En);
    }

    #[test]
    fn explicit_selection_overrides_system() {
        let store = std::sync::Arc::new(MemoryStore::new(None));
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["zh-Hans-CN"]))));
        let service = service(store, source);
        let snapshot = service.set_selection(LocaleSelection::En).expect("persist");
        assert_eq!(snapshot.selection, LocaleSelection::En);
        assert_eq!(snapshot.effective_locale, EffectiveLocale::En);
        let snapshot = service
            .set_selection(LocaleSelection::ZhHans)
            .expect("persist");
        assert_eq!(snapshot.effective_locale, EffectiveLocale::ZhHans);
    }

    #[test]
    fn no_persisted_value_is_system_mode() {
        let store = std::sync::Arc::new(MemoryStore::new(None));
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["zh-Hans-CN"]))));
        let service = service(store, source);
        let snapshot = service.snapshot();
        assert_eq!(snapshot.selection, LocaleSelection::System);
        assert_eq!(snapshot.effective_locale, EffectiveLocale::ZhHans);
        assert_eq!(snapshot.generation, 0);
        assert!(snapshot.diagnostic.is_none());
    }

    #[test]
    fn unreadable_store_uses_english_baseline_with_diagnostic() {
        let store = std::sync::Arc::new(MemoryStore::new(Some(LocaleSelection::En)));
        *store.fail_reads.lock().expect("lock") = true;
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["zh-Hans"]))));
        let service = service(store, source);
        let snapshot = service.snapshot();
        assert_eq!(snapshot.effective_locale, EffectiveLocale::En);
        assert_eq!(snapshot.selection, LocaleSelection::System);
        let diagnostic = snapshot.diagnostic.expect("fallback diagnostic");
        assert_eq!(diagnostic.code, "locale_store_unavailable");
    }

    #[test]
    fn persist_failure_keeps_snapshot_and_generation_unchanged() {
        let store = std::sync::Arc::new(MemoryStore::new(Some(LocaleSelection::En)));
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["zh-Hans"]))));
        let service = service(store.clone(), source);
        let before: LocaleSnapshot = service.snapshot();
        *store.fail_writes.lock().expect("lock") = true;
        let error = service
            .set_selection(LocaleSelection::ZhHans)
            .expect_err("must fail");
        assert!(error.to_string().contains("disk full"));
        assert_eq!(service.snapshot(), before);
        // The persisted value is unchanged too.
        assert_eq!(
            *store.value.lock().expect("lock"),
            Some(LocaleSelection::En)
        );
    }

    #[test]
    fn successful_persist_publishes_new_generation() {
        let store = std::sync::Arc::new(MemoryStore::new(None));
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["fr-FR"]))));
        let service = service(store, source);
        assert_eq!(service.snapshot().generation, 0);
        let second = service
            .set_selection(LocaleSelection::ZhHans)
            .expect("persist");
        assert_eq!(second.generation, 1);
        assert_eq!(second.effective_locale, EffectiveLocale::ZhHans);
        let third = service
            .set_selection(LocaleSelection::System)
            .expect("persist");
        assert_eq!(third.generation, 2);
        // System re-negotiates from the source: fr-FR misses → English.
        assert_eq!(third.effective_locale, EffectiveLocale::En);
    }

    #[test]
    fn refresh_negotiates_system_mode_and_only_bumps_generation_on_change() {
        let store = std::sync::Arc::new(MemoryStore::new(None));
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["en-US"]))));
        let service = service(store, source.clone());
        assert_eq!(service.snapshot().effective_locale, EffectiveLocale::En);
        // Same result → same generation.
        assert_eq!(service.refresh_system_languages().generation, 0);
        // The macOS language list changed while running.
        *source.0.lock().expect("lock") = tags(&["zh-Hans-CN"]);
        let refreshed = service.refresh_system_languages();
        assert_eq!(refreshed.effective_locale, EffectiveLocale::ZhHans);
        assert_eq!(refreshed.generation, 1);
    }

    #[test]
    fn safe_english_baseline_is_sticky_until_the_store_recovers() {
        let store = std::sync::Arc::new(MemoryStore::new(None));
        *store.fail_reads.lock().expect("lock") = true;
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["zh-Hans-CN"]))));
        let service = service(store.clone(), source);
        let baseline = service.snapshot();
        assert_eq!(baseline.effective_locale, EffectiveLocale::En);
        assert!(baseline.diagnostic.is_some());

        // Re-negotiation must not flip a corrupt-store session to zh-Hans.
        let refreshed = service.refresh_system_languages();
        assert_eq!(refreshed.effective_locale, EffectiveLocale::En);
        assert_eq!(refreshed.generation, 0);

        // A successful persist proves the store recovered: negotiation and
        // the diagnostic reset apply again.
        *store.fail_reads.lock().expect("lock") = false;
        let snapshot = service
            .set_selection(LocaleSelection::System)
            .expect("persist");
        assert_eq!(snapshot.effective_locale, EffectiveLocale::ZhHans);
        assert!(snapshot.diagnostic.is_none());
        assert_eq!(snapshot.generation, 1);
    }

    #[test]
    fn every_clone_is_the_same_authority() {
        // ADR-0011: React, tray and the native menu consume ONE authority.
        // A value-forking clone would freeze native surfaces at the startup
        // locale after a manual switch — the snapshot must be shared.
        let store = std::sync::Arc::new(MemoryStore::new(None));
        let source = std::sync::Arc::new(FixedSource(Mutex::new(tags(&["en-US"]))));
        let service = service(store, source);
        let api_clone = service.clone();
        let listener_clone = service.clone();

        let switched = api_clone
            .set_selection(LocaleSelection::ZhHans)
            .expect("persist");
        assert_eq!(switched.generation, 1);
        // The listener-facing clone observes the same published state.
        let observed = listener_clone.snapshot();
        assert_eq!(observed.selection, LocaleSelection::ZhHans);
        assert_eq!(observed.effective_locale, EffectiveLocale::ZhHans);
        assert_eq!(observed.generation, 1);
    }
}
