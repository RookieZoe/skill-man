import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import type {
  CatalogClient,
  EffectiveLocale,
  LocaleSelection,
  LocaleSnapshot,
} from "../../app/catalog-client";
import {
  translate,
  translatePlural,
  type MessageKey,
  type MessageParams,
  type PluralKey,
} from "./messages";

/**
 * The single React consumer of the locale authority (ADR-0011): native is
 * the authority, this provider only renders the published snapshot, mirrors
 * it to `document.lang`, and exposes the catalog lookup. Switching never
 * remounts children — React state (filter, selection, forms, scroll, focus)
 * survives by construction and is asserted in tests.
 */
export interface LocaleContextValue {
  locale: EffectiveLocale;
  selection: LocaleSelection;
  generation: number;
  /** persist-then-publish on native; failures leave everything unchanged. */
  setSelection: (selection: LocaleSelection) => Promise<void>;
  refreshSystemLanguages: () => Promise<void>;
  t: (key: MessageKey, params?: MessageParams) => string;
  tPlural: (base: PluralKey, count: number, params?: MessageParams) => string;
}

const defaultContext: LocaleContextValue = {
  locale: "en",
  selection: "system",
  generation: 0,
  setSelection: async () => {},
  refreshSystemLanguages: async () => {},
  t: (key, params) => translate("en", key, params),
  tPlural: (base, count, params) => translatePlural("en", base, count, params),
};

const LocaleContext = createContext<LocaleContextValue>(defaultContext);

export function LocaleProvider({
  client,
  children,
}: {
  client: CatalogClient;
  children: ReactNode;
}) {
  const [snapshot, setSnapshot] = useState<LocaleSnapshot | null>(null);

  useEffect(() => {
    let current = true;
    let unlisten: (() => void) | null = null;
    client
      .getLocaleSnapshot()
      .then((next) => {
        if (current) setSnapshot(next);
      })
      .catch(() => {
        // The authority is unreachable; the English baseline keeps every
        // surface non-blank and the persisted selection is retried on the
        // next `locale://changed` event.
      });
    client
      .listenLocaleChanged((payload) => {
        setSnapshot(payload);
      })
      .then((stop) => {
        if (current) unlisten = stop;
        else stop();
      });
    return () => {
      current = false;
      unlisten?.();
    };
  }, [client]);

  const locale: EffectiveLocale = snapshot?.effectiveLocale ?? "en";

  // `document.lang` mirrors the effective locale on first frame and on every
  // runtime switch (ADR-0011).
  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  const setSelection = useCallback(
    async (selection: LocaleSelection) => {
      const next = await client.setLocaleSelection(selection);
      setSnapshot(next);
    },
    [client],
  );

  const refreshSystemLanguages = useCallback(async () => {
    const next = await client.refreshSystemLanguages();
    setSnapshot(next);
  }, [client]);

  const value = useMemo<LocaleContextValue>(
    () => ({
      locale,
      selection: snapshot?.selection ?? "system",
      generation: snapshot?.generation ?? 0,
      setSelection,
      refreshSystemLanguages,
      t: (key, params) => translate(locale, key, params),
      tPlural: (base, count, params) =>
        translatePlural(locale, base, count, params),
    }),
    [locale, snapshot, setSelection, refreshSystemLanguages],
  );

  // The effective locale must be known before the first visible frame
  // (ADR-0011): until the authority answers, no surface renders in a
  // possibly-wrong locale. Native resolves this within the startup command
  // round-trip; the first paint is a blank frame at worst, never mixed copy.
  if (snapshot === null) {
    return null;
  }

  return (
    <LocaleContext.Provider value={value}>{children}</LocaleContext.Provider>
  );
}

export function useLocale(): LocaleContextValue {
  return useContext(LocaleContext);
}
