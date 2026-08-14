import en from "../../../resources/locales/en.json";
import zhHans from "../../../resources/locales/zh-Hans.json";
import type { EffectiveLocale } from "../../app/catalog-client";

/**
 * Shared message catalog (spec §6.2): `en.json` is the complete baseline and
 * `MessageKey` is derived from it, so a key missing from English cannot be
 * referenced at all. `zh-Hans.json` must carry the identical key, placeholder
 * and plural-parameter set — `scripts/check-locales.mjs` blocks CI otherwise.
 */

/** `en.json` is the single source of the key space. */
export type MessageKey = keyof typeof en;

export type MessageParams = Readonly<Record<string, string | number>>;

/** Every `_one` / `_other` pair is a plural entry selected by count. */
export type PluralKey = {
  [K in MessageKey]: K extends `${infer Base}_one` ? Base : never;
}[MessageKey];

const catalogs: Record<EffectiveLocale, typeof en> = {
  en,
  "zh-Hans": zhHans,
};

function interpolate(template: string, params?: MessageParams): string {
  if (!params) return template;
  return template.replace(
    /\{([a-zA-Z][a-zA-Z0-9]*)\}/g,
    (match, name: string) => {
      const value = params[name];
      return value === undefined ? match : String(value);
    },
  );
}

/** Pure lookup + `{param}` interpolation. Unknown keys keep their placeholder. */
export function translate(
  locale: EffectiveLocale,
  key: MessageKey,
  params?: MessageParams,
): string {
  const catalog = catalogs[locale];
  // Runtime fallback to the English baseline keeps surfaces non-blank; CI
  // parity is enforced statically by scripts/check-locales.mjs.
  const template = catalog[key] ?? en[key];
  return interpolate(template, params);
}

function pluralSuffix(locale: EffectiveLocale, count: number): "one" | "other" {
  const rule = new Intl.PluralRules(locale).select(count);
  return rule === "one" ? "one" : "other";
}

/** Plural-aware lookup: `{base}_one` / `{base}_other` by `Intl.PluralRules`. */
export function translatePlural(
  locale: EffectiveLocale,
  base: PluralKey,
  count: number,
  params?: MessageParams,
): string {
  const suffix = pluralSuffix(locale, count);
  return translate(locale, `${base}_${suffix}` as MessageKey, {
    ...params,
    count,
  });
}

/** Locale-aware absolute date/time (spec §6.2: never hand-composed). */
export function formatDateTime(locale: EffectiveLocale, iso: string): string {
  return new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(iso));
}

/** Locale-aware byte size; units stay raw technical tokens. */
export function formatByteSize(locale: EffectiveLocale, bytes: number): string {
  if (bytes < 1024) {
    return translate(locale, "library.app_update.bytes", { count: bytes });
  }
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const precision = value >= 10 ? 0 : 1;
  return `${value.toFixed(precision)} ${units[unitIndex]}`;
}

/**
 * Closed public error code → message key (spec §4.7). Every code the native
 * union can emit maps to a bilingual presentation; the recovery-flow codes
 * fall back to the generic recovery message (RecoveryView renders its own
 * specialized notice for `recovery_step_failed`).
 */
export function errorMessageKey(code: string): MessageKey {
  switch (code) {
    case "validation":
      return "error.validation";
    case "not_found":
      return "error.not_found";
    case "conflict":
      return "error.conflict";
    case "plan_stale":
      return "error.plan_stale";
    case "permission_denied":
      return "error.permission_denied";
    case "state_unavailable":
      return "error.state_unavailable";
    case "catalog_unavailable":
      return "error.catalog_unavailable";
    case "recovery_required":
      return "error.recovery_required";
    case "source_unavailable":
      return "error.source_unavailable";
    case "target_mismatch":
      return "error.target_mismatch";
    case "disk_full":
      return "error.disk_full";
    case "modified":
      return "error.modified";
    case "stale_update":
      return "app.update_error.stale_update";
    case "update_cancelled":
      return "app.update_error.cancelled";
    case "download_failed":
      return "app.update_error.download_failed";
    case "install_failed":
      return "app.update_error.install_failed";
    case "bootstrap_unavailable":
      return "error.bootstrap_unavailable";
    case "locale_store_unavailable":
      return "error.locale_store_unavailable";
    case "recovery_not_locked":
    case "recovery_not_pure":
    case "recovery_no_active_operation":
    case "recovery_operation_already_active":
    case "recovery_writer_active":
    case "recovery_step_failed":
    case "recovery_state_ambiguous":
    case "recovery_snapshot_in_use":
    case "recovery_state_store":
    case "recovery_filesystem":
    case "recovery_probe":
      return "error.recovery";
    case "restore_not_applicable":
      return "error.restore_not_applicable";
    case "reconnect_not_available":
      return "error.reconnect_not_available";
    case "abandon_not_applicable":
      return "error.abandon_not_applicable";
    case "abandon_confirmation_mismatch":
      return "error.abandon_confirmation_mismatch";
    case "abandon_cas_conflict":
      return "error.abandon_cas_conflict";
    case "candidate_invalid":
      return "error.candidate_invalid";
    case "binding_step_failed":
      return "error.binding_step_failed";
    case "binding_state_ambiguous":
      return "error.binding_state_ambiguous";
    case "binding_not_cancellable":
      return "error.binding_not_cancellable";
    case "binding_migration_failed":
      return "error.binding_migration_failed";
    default:
      return "error.internal";
  }
}

/** Typed params for the error message key (Source Content only). */
export function errorMessageParams(error: {
  code: string;
  directoryName?: string;
}): MessageParams | undefined {
  if (error.code === "conflict" && error.directoryName) {
    return { name: error.directoryName };
  }
  return undefined;
}
