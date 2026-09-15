import type { Health } from "../../app/catalog-client";
import { useState, type ReactNode } from "react";
import { openSkillDirectory } from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

export interface EvidenceRailProps {
  skillId?: string;
  directoryIdentity: string;
  canonicalEntity: string;
  sourceRelease?: string | null;
  activationEvidence?: string | null;
  health: Health;
  children?: ReactNode;
}

/**
 * Evidence rail (spec §7.3; ADR-0021): four-cell signature element for
 * Library Desk Skill/Source member details ("Directory identity /
 * Canonical entity / Source release / Activation evidence").
 *
 * Source Snapshot Mismatch renders in warning tone, Broken in danger tone.
 * Directory navigation does not mutate Skill or distribution state.
 */
export function EvidenceRail({
  skillId,
  directoryIdentity,
  canonicalEntity,
  sourceRelease,
  activationEvidence,
  health,
  children,
}: EvidenceRailProps) {
  const { t } = useLocale();
  const [opening, setOpening] = useState(false);
  const [openFailed, setOpenFailed] = useState(false);
  const tone =
    health === "source_snapshot_mismatch"
      ? "warning"
      : health === "broken"
        ? "danger"
        : "healthy";

  return (
    <section
      className={`evidence-rail evidence-rail--${tone}`}
      aria-label={t("evidenceRail.label")}
      data-tone={tone}
    >
      <div className="evidence-rail-cell">
        <span className="evidence-rail-label">
          {t("evidenceRail.directoryIdentity")}
        </span>
        <span className="evidence-rail-value" title={directoryIdentity}>
          {directoryIdentity}
        </span>
      </div>
      <div className="evidence-rail-cell">
        <span className="evidence-rail-label">
          {t("evidenceRail.canonicalEntity")}
        </span>
        <span className="evidence-rail-value" title={canonicalEntity}>
          {canonicalEntity}
          {skillId && (
            <button
              type="button"
              className="skill-directory-icon"
              aria-label={t("evidenceRail.openDirectory")}
              title={t("evidenceRail.openDirectory")}
              disabled={opening}
              onClick={() => {
                setOpening(true);
                setOpenFailed(false);
                void openSkillDirectory(skillId)
                  .catch(() => setOpenFailed(true))
                  .finally(() => setOpening(false));
              }}
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <path d="M20 8V6a2 2 0 0 0-2-2h-6l-2-2H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 1.94-1.5L22 10H7l-3 10" />
              </svg>
            </button>
          )}
        </span>
        {openFailed && (
          <span role="alert">{t("evidenceRail.openDirectoryFailed")}</span>
        )}
      </div>
      <div className="evidence-rail-cell">
        <span className="evidence-rail-label">
          {t("evidenceRail.sourceRelease")}
        </span>
        <span
          className="evidence-rail-value"
          title={sourceRelease ?? t("evidenceRail.sourceReleaseNone")}
        >
          {sourceRelease ?? t("evidenceRail.sourceReleaseNone")}
        </span>
      </div>
      <div className="evidence-rail-cell">
        <span className="evidence-rail-label">
          {t("evidenceRail.activationEvidence")}
        </span>
        <span
          className="evidence-rail-value"
          title={activationEvidence ?? t("evidenceRail.activationNone")}
        >
          {activationEvidence ?? t("evidenceRail.activationNone")}
        </span>
      </div>
      {children}
    </section>
  );
}
