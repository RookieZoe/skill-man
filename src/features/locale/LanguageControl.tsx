import { useState } from "react";

import type { LocaleSelection } from "../../app/catalog-client";
import { useLocale } from "./LocaleProvider";

/**
 * The App-level `System / English / 简体中文` choice (ADR-0011): lives outside
 * Home candidates and the binding forms, so it renders in the bootstrap App
 * shell and in Preferences. It is not a switch — the four boolean
 * Preferences stay untouched.
 */
export function LanguageControl() {
  const { selection, t, setSelection } = useLocale();
  const [pending, setPending] = useState(false);

  const options: Array<{ value: LocaleSelection; label: string }> = [
    { value: "system", label: t("language.system") },
    { value: "en", label: t("language.english") },
    { value: "zh-Hans", label: t("language.zh_hans") },
  ];

  return (
    <div className="language-control">
      <span className="language-control-label" id="language-control-label">
        {t("language.label")}
      </span>
      <div
        className="language-control-options"
        role="radiogroup"
        aria-labelledby="language-control-label"
      >
        {options.map((option) => (
          <label key={option.value} className="language-control-option">
            <input
              type="radio"
              name="language-selection"
              value={option.value}
              checked={selection === option.value}
              disabled={pending}
              onChange={() => {
                setPending(true);
                setSelection(option.value)
                  .catch(() => {
                    // persist-then-publish failed natively: the snapshot and
                    // every visible surface are unchanged; the control
                    // re-renders from the old snapshot.
                  })
                  .finally(() => setPending(false));
              }}
            />
            <span>{option.label}</span>
          </label>
        ))}
      </div>
      <small className="language-control-hint">
        {t("language.control_hint")}
      </small>
    </div>
  );
}
