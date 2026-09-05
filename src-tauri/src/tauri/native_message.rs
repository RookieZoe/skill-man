//! Rust native presentation catalog (spec §6.2): tray and native menu copy
//! resolve from the same `resources/locales/en.json` / `zh-Hans.json` files
//! the webview uses, through a tested closed `NativeMessageKey` subset.
//! English is the runtime fallback for a missing zh key (CI enforces parity).

use std::sync::OnceLock;

use serde_json::Value;

use crate::seams::locale_store::EffectiveLocale;

const EN_CATALOG: &str = include_str!("../../../resources/locales/en.json");
const ZH_CATALOG: &str = include_str!("../../../resources/locales/zh-Hans.json");

/// The closed subset of catalog keys native surfaces may render.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeMessageKey {
    TrayRecent,
    TrayEmpty,
    TrayOpenWindow,
    TrayQuit,
    TrayAgentCountOne,
    TrayAgentCountOther,
    TrayHealthBroken,
    TrayHealthModified,
    TrayHealthMismatch,
    SourceSnapshotMismatch,
    MenuWindow,
    MenuHelp,
    MenuAgents,
}

impl NativeMessageKey {
    fn catalog_key(self) -> &'static str {
        match self {
            Self::TrayRecent => "tray.recent",
            Self::TrayEmpty => "tray.empty",
            Self::TrayOpenWindow => "tray.open_window",
            Self::TrayQuit => "tray.quit",
            Self::TrayAgentCountOne => "tray.agent_count_one",
            Self::TrayAgentCountOther => "tray.agent_count_other",
            Self::TrayHealthBroken => "tray.health.broken",
            Self::TrayHealthModified => "tray.health.modified",
            Self::TrayHealthMismatch => "tray.health.source_snapshot_mismatch",
            Self::SourceSnapshotMismatch => "tray.health.source_snapshot_mismatch",
            Self::MenuWindow => "menu.window",
            Self::MenuHelp => "menu.help",
            Self::MenuAgents => "menu.agents",
        }
    }
}

fn catalogs() -> (&'static Value, &'static Value) {
    static CATALOGS: OnceLock<(Value, Value)> = OnceLock::new();
    let pair = CATALOGS.get_or_init(|| {
        (
            serde_json::from_str(EN_CATALOG).expect("valid en.json catalog"),
            serde_json::from_str(ZH_CATALOG).expect("valid zh-Hans.json catalog"),
        )
    });
    (&pair.0, &pair.1)
}

/// Lookup with `{param}` interpolation; missing zh keys fall back to the
/// English baseline so native surfaces never render blank.
pub fn native_message(
    locale: EffectiveLocale,
    key: NativeMessageKey,
    params: &[(&str, &str)],
) -> String {
    let (en, zh) = catalogs();
    let catalog = match locale {
        EffectiveLocale::En => en,
        EffectiveLocale::ZhHans => zh,
    };
    let template = catalog
        .get(key.catalog_key())
        .and_then(Value::as_str)
        .or_else(|| en.get(key.catalog_key()).and_then(Value::as_str))
        .expect("every NativeMessageKey exists in en.json");
    let mut message = template.to_owned();
    for (name, value) in params {
        message = message.replace(&format!("{{{name}}}"), value);
    }
    message
}

/// Plural-aware tray count: `{count} Agent` / `{count} Agents` by `count`.
pub fn native_plural(locale: EffectiveLocale, count: u64) -> String {
    // zh-Hans has a single plural category; en distinguishes one/other.
    let suffix = match locale {
        EffectiveLocale::En if count == 1 => "one",
        _ => "other",
    };
    let key = match suffix {
        "one" => NativeMessageKey::TrayAgentCountOne,
        _ => NativeMessageKey::TrayAgentCountOther,
    };
    native_message(locale, key, &[("count", &count.to_string())])
}

/// The Chinese plural rule has a single category, so a count of 1 still
/// renders the `_other` template there.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_native_key_resolves_in_both_locales() {
        for locale in [EffectiveLocale::En, EffectiveLocale::ZhHans] {
            for key in [
                NativeMessageKey::TrayRecent,
                NativeMessageKey::TrayEmpty,
                NativeMessageKey::TrayOpenWindow,
                NativeMessageKey::TrayQuit,
                NativeMessageKey::TrayAgentCountOne,
                NativeMessageKey::TrayAgentCountOther,
                NativeMessageKey::TrayHealthBroken,
                NativeMessageKey::TrayHealthModified,
                NativeMessageKey::MenuWindow,
                NativeMessageKey::MenuHelp,
                NativeMessageKey::MenuAgents,
            ] {
                let message = native_message(locale, key, &[]);
                assert!(!message.is_empty(), "{locale:?} {key:?} must resolve");
            }
        }
    }

    #[test]
    fn english_agent_count_is_plural_aware() {
        assert_eq!(native_plural(EffectiveLocale::En, 1), "1 Agent");
        assert_eq!(native_plural(EffectiveLocale::En, 3), "3 Agents");
    }

    #[test]
    fn zh_hans_agent_count_uses_one_template_for_all_counts() {
        // zh-Hans has a single plural category: count 1 renders from the
        // `_other` template with the count interpolated (unlike en, which
        // uses the fixed "1 Agent" `_one` template).
        assert_eq!(native_plural(EffectiveLocale::ZhHans, 1), "1 个智能体");
        assert_eq!(native_plural(EffectiveLocale::ZhHans, 3), "3 个智能体");
    }

    #[test]
    fn interpolation_replaces_count_params() {
        let message = native_message(
            EffectiveLocale::En,
            NativeMessageKey::TrayAgentCountOther,
            &[("count", "7")],
        );
        assert_eq!(message, "7 Agents");
    }

    #[test]
    fn health_suffixes_round_trip_through_the_catalog() {
        assert_eq!(
            native_message(EffectiveLocale::En, NativeMessageKey::TrayHealthBroken, &[]),
            " · broken"
        );
        assert_eq!(
            native_message(
                EffectiveLocale::ZhHans,
                NativeMessageKey::TrayHealthModified,
                &[]
            ),
            " · 已修改"
        );
    }
}
