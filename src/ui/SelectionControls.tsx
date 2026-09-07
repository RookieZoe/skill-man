import { useLocale } from "../features/locale/LocaleProvider";

export function SelectionControls({
  ids,
  selected,
  onChange,
  disabled = false,
}: {
  ids: string[];
  selected: string[];
  onChange: (ids: string[]) => void;
  disabled?: boolean;
}) {
  const { t } = useLocale();
  const visible = new Set(ids);
  const outside = selected.filter((id) => !visible.has(id));
  return (
    <div
      className="selection-controls"
      role="group"
      aria-label={t("selection.label")}
    >
      <button
        type="button"
        disabled={
          disabled || !ids.length || ids.every((id) => selected.includes(id))
        }
        onClick={() => onChange([...outside, ...ids])}
      >
        {t("selection.all")}
      </button>
      <button
        type="button"
        disabled={disabled || !ids.length}
        onClick={() =>
          onChange([...outside, ...ids.filter((id) => !selected.includes(id))])
        }
      >
        {t("selection.invert")}
      </button>
      <button
        type="button"
        disabled={disabled || !ids.some((id) => selected.includes(id))}
        onClick={() => onChange(outside)}
      >
        {t("selection.clear")}
      </button>
    </div>
  );
}
