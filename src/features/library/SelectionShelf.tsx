import { useLocale } from "../locale/LocaleProvider";

/**
 * Selection shelf (spec §7.4, §7.5; ADR-0019; ADR-0021; #88):
 * Fixed action bar provided at the bottom after selecting at least one
 * Managed Skill in temporary multi-select mode.
 * Provides `Enable Globally…` and `Enable to Project…`, plus `Exit`.
 * Does not remember selections across operations; exiting clears draft.
 */
export function SelectionShelf({
  selectedCount,
  onEnableGlobally,
  onEnableToProject,
  onExit,
  inert = false,
}: {
  selectedCount: number;
  onEnableGlobally: () => void;
  onEnableToProject: () => void;
  onExit: () => void;
  inert?: boolean;
}) {
  const { t, tPlural } = useLocale();

  if (selectedCount < 1) {
    return null;
  }

  return (
    <aside
      className="selection-shelf"
      role="region"
      aria-label={tPlural("shelf.selectedCount", selectedCount)}
      inert={inert ? true : undefined}
    >
      <div className="selection-shelf-info">
        <span className="selection-shelf-count">
          {tPlural("shelf.selectedCount", selectedCount)}
        </span>
      </div>
      <div className="selection-shelf-actions">
        <button
          type="button"
          className="toolbar-button primary"
          onClick={onEnableGlobally}
        >
          {t("shelf.enableGlobally")}
        </button>
        <button
          type="button"
          className="toolbar-button"
          onClick={onEnableToProject}
        >
          {t("shelf.enableToProject")}
        </button>
        <button type="button" className="toolbar-button" onClick={onExit}>
          {t("shelf.exit")}
        </button>
      </div>
    </aside>
  );
}
