import type { Health } from "../../app/catalog-client";
import type { ReactNode } from "react";
import { useLocale } from "../locale/LocaleProvider";

export interface EvidenceRailProps {
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
 * The rail is strictly read-only and carries zero operation controls.
 */
export function EvidenceRail({
  directoryIdentity,
  canonicalEntity,
  sourceRelease,
  activationEvidence,
  health,
  children,
}: EvidenceRailProps) {
  const { t } = useLocale();
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
        </span>
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
