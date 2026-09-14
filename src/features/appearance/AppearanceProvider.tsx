import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { useLocale } from "../locale/LocaleProvider";
import {
  appearanceApi,
  type AppearanceApi,
  type AppearanceSnapshot,
  type Appearance,
} from "./appearance-api";
export type { Appearance } from "./appearance-api";
const storageKey = "skill-man.appearance";
const Context = createContext({
  selection: "system" as Appearance,
  select: async (value: Appearance) => {
    void value;
  },
});
export function AppearanceProvider({
  children,
  api = appearanceApi,
}: {
  children: ReactNode;
  api?: AppearanceApi;
}) {
  const [snapshot, setSnapshot] = useState<AppearanceSnapshot | null>(null);
  const accept = (next: AppearanceSnapshot) =>
    setSnapshot((old) =>
      !old || next.generation >= old.generation ? next : old,
    );
  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    const receive = (next: AppearanceSnapshot) => {
      if (active) accept(next);
    };
    void (async () => {
      try {
        const unlisten = await api.listenChanged(receive);
        if (!active) {
          unlisten();
          return;
        }
        stop = unlisten;
        receive(await api.getSnapshot());
        // An unreadable legacy store is not evidence that no old choice exists.
        const legacy = localStorage.getItem(storageKey);
        const migrated = await api.migrateLegacy(legacy);
        receive(migrated);
        try {
          localStorage.removeItem(storageKey);
        } catch {
          /* retry migration on next mount */
        }
      } catch {
        /* retain native snapshot and legacy for a later retry */
      }
    })();
    return () => {
      active = false;
      stop?.();
    };
  }, [api]);
  const selection = snapshot?.selection ?? "system";
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme =
        selection === "system" ? (media.matches ? "dark" : "light") : selection;
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [selection]);
  return (
    <Context.Provider
      value={{
        selection,
        select: async (value) => accept(await api.setSelection(value)),
      }}
    >
      {children}
    </Context.Provider>
  );
}
export function AppearanceControl() {
  const { t } = useLocale();
  const { selection, select } = useContext(Context);
  const [failed, setFailed] = useState(false);
  return (
    <div className="language-control">
      <div className="language-control-label" id="appearance-label">
        {t("appearance.title")}
      </div>
      <div
        className="language-control-options"
        role="radiogroup"
        aria-labelledby="appearance-label"
      >
        {(["system", "light", "dark"] as const).map((value) => (
          <label className="language-control-option" key={value}>
            <input
              type="radio"
              name="appearance"
              value={value}
              checked={selection === value}
              onChange={() => {
                void select(value).then(
                  () => setFailed(false),
                  () => setFailed(true),
                );
              }}
            />
            <span>{t(`appearance.${value}`)}</span>
          </label>
        ))}
      </div>
      <p className="language-control-hint">{t("appearance.hint")}</p>
      {failed && <p role="alert">{t("appearance.failed")}</p>}
    </div>
  );
}
