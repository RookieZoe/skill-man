import { applyAppAppearance } from "../../app/catalog-client";
import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { useLocale } from "../locale/LocaleProvider";

export type Appearance = "system" | "light" | "dark";
const storageKey = "skill-man.appearance";
function readAppearance(): Appearance {
  if (
    import.meta.env.DEV &&
    document.documentElement.dataset.previewTheme === "dark"
  )
    return "dark";
  try {
    const value = localStorage.getItem(storageKey);
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}
const Context = createContext({
  selection: "system" as Appearance,
  select: (value: Appearance) => {
    void value;
  },
});
export function AppearanceProvider({ children }: { children: ReactNode }) {
  const [selection, setSelection] = useState(readAppearance);
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme =
        selection === "system" ? (media.matches ? "dark" : "light") : selection;
    };
    apply();
    void applyAppAppearance(selection).catch(() => {
      /* CSS appearance remains usable if native chrome is unavailable. */
    });
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [selection]);
  return (
    <Context.Provider
      value={{
        selection,
        select: (value) => {
          localStorage.setItem(storageKey, value);
          setSelection(value);
        },
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
                try {
                  select(value);
                  setFailed(false);
                } catch {
                  setFailed(true);
                }
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
