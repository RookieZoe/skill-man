import { useState } from "react";
import type {
  GitSourceCapabilitySource,
  SkillSummary,
} from "../../app/catalog-client";
import { useLocale, type LocaleContextValue } from "../locale/LocaleProvider";
import type { MessageKey } from "../locale/messages";
import type { SourceActionNotice } from "./GitSourceCapabilityNotice";
import { defaultCopyDestinationPicker } from "./GitSourceCapabilityNotice";

export interface SourceGroupCardProps {
  source: GitSourceCapabilitySource;
  skills: SkillSummary[];
  actionActivity?: boolean;
  actionNotice?: SourceActionNotice | null;
  onPromote?: (remoteId: string, trigger: HTMLButtonElement) => void;
  onUpdate?: (remoteId: string, trigger: HTMLButtonElement) => void;
  onRestore?: (remoteId: string) => void;
  onCopyMember?: (
    remoteId: string,
    skillId: string,
    destination: string,
  ) => void;
  onRemove?: (remoteId: string) => void;
  onOpenBrokenDisable?: (
    skillId: string,
    skillPath: string,
    trigger: HTMLButtonElement,
  ) => void;
  pickDirectory?: () => Promise<string | null>;
}

/**
 * Git Repository Source Group Card (spec §7.6; ADR-0018, ADR-0021):
 * - Policy cascade and explicit override side-by-side
 * - Source-level Update and whole-source Remove
 * - Read-only member rows with `skillPath`, Activation health, and Target group count
 * - Embedded Source Snapshot Mismatch panel when snapshot mismatch is detected
 * - Fail-closed for Legacy and Remote Source Identity Conflict with typed reasons
 */
export function SourceGroupCard({
  source,
  skills,
  actionActivity = false,
  actionNotice = null,
  onPromote,
  onUpdate,
  onRestore,
  onCopyMember,
  onRemove,
  onOpenBrokenDisable,
  pickDirectory = defaultCopyDestinationPicker,
}: SourceGroupCardProps) {
  const { t, tPlural } = useLocale();
  const [confirming, setConfirming] = useState<"restore" | "remove" | null>(
    null,
  );
  const copyableMembers = source.members.filter((m) => m.presence);
  const [copySkillId, setCopySkillId] = useState<string>(
    copyableMembers[0]?.skillId ?? "",
  );

  // Fail closed for Legacy and Remote Source Identity Conflict
  if (source.kind === "legacy_per_skill_git_state") {
    return (
      <article
        className="source-group-card source-group-card--legacy"
        aria-label={t("sourceGroup.label")}
      >
        <header className="source-group-header">
          <h3>{t("library.source_capability.legacy_per_skill_git_state")}</h3>
          <p className="source-group-url">{source.canonicalUrl}</p>
        </header>
        <p className="source-group-detail">
          {t("library.source_capability.legacy_per_skill_git_state_detail")}
        </p>
        <p className="source-group-blocked-reason" role="note">
          {t("sourceGroup.legacyBlockedReason")}
        </p>
        {onPromote ? (
          <div className="source-group-actions">
            <button
              type="button"
              className="repair-button"
              disabled={actionActivity}
              onClick={(event) =>
                onPromote(source.remoteId, event.currentTarget)
              }
            >
              {t("library.source_capability.promote")}
            </button>
          </div>
        ) : null}
      </article>
    );
  }

  if (source.kind === "remote_source_identity_conflict") {
    return (
      <article
        className="source-group-card source-group-card--conflict"
        aria-label={t("sourceGroup.label")}
      >
        <header className="source-group-header">
          <h3>
            {t("library.source_capability.remote_source_identity_conflict")}
          </h3>
          <p className="source-group-url">{source.canonicalUrl}</p>
        </header>
        <p className="source-group-detail">
          {t(
            "library.source_capability.remote_source_identity_conflict_detail",
          )}
        </p>
        <p className="source-group-blocked-reason" role="alert">
          {t("sourceGroup.conflictBlockedReason")}
        </p>
      </article>
    );
  }

  // Check if any member has Source Snapshot Mismatch
  const hasMismatch = source.members.some((member) => {
    const skill = skills.find((s) => s.id === member.skillId);
    return skill?.health === "source_snapshot_mismatch";
  });

  async function handleCopy() {
    if (!onCopyMember || !copySkillId) return;
    const destination = await pickDirectory();
    if (destination) {
      onCopyMember(source.remoteId, copySkillId, destination);
    }
  }

  const localCopyControls =
    onCopyMember && copyableMembers.length > 0 ? (
      <div className="local-copy-controls">
        {copyableMembers.length > 1 ? (
          <label className="copy-member-select">
            <span>{t("sourceGroup.copyMemberLabel")}</span>
            <select
              value={copySkillId}
              disabled={actionActivity}
              onChange={(e) => setCopySkillId(e.target.value)}
            >
              {copyableMembers.map((member) => (
                <option key={member.skillId} value={member.skillId}>
                  {member.skillPath}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        <button
          type="button"
          className="repair-button"
          disabled={actionActivity}
          onClick={() => void handleCopy()}
        >
          {t("sourceGroup.createLocalCopy")}
        </button>
        <small className="local-copy-note">
          {t("sourceGroup.localCopyNoSwitchNote")}
        </small>
      </div>
    ) : null;

  const overrideText =
    source.trackingMode && source.trackingMode !== "auto_release_tag_head"
      ? source.trackingValue || source.trackingMode
      : t("sourceGroup.overrideNone");

  return (
    <article
      className={`source-group-card${hasMismatch ? " source-group-card--mismatch" : ""}`}
      aria-label={t("sourceGroup.label")}
    >
      <header className="source-group-header">
        <div className="source-group-title-row">
          <h3>{source.canonicalUrl}</h3>
          {source.selectedRef ? (
            <span className="source-group-ref">
              {t("sourceGroup.selectedRef", { ref: source.selectedRef })}
            </span>
          ) : null}
        </div>
        {source.resolvedCommit ? (
          <p className="source-group-commit">
            {t("sourceGroup.resolvedCommit", {
              commit: source.resolvedCommit,
            })}
          </p>
        ) : null}
      </header>

      {/* Policy cascade and explicit override side-by-side */}
      <section
        className="source-group-policy-row"
        aria-label={t("sourceGroup.trackingPolicy")}
      >
        <div className="source-group-policy-cascade">
          <h4>{t("sourceGroup.trackingPolicy")}</h4>
          <ol className="policy-cascade-list">
            <li>{t("sourceGroup.policyRelease")}</li>
            <li>{t("sourceGroup.policySemverTag")}</li>
            <li>{t("sourceGroup.policyTag")}</li>
            <li>{t("sourceGroup.policyHead")}</li>
          </ol>
        </div>
        <div className="source-group-policy-override">
          <h4>{t("sourceGroup.override")}</h4>
          <span className="policy-override-value">{overrideText}</span>
        </div>
      </section>

      {/* Embedded Source Snapshot Mismatch Panel */}
      {hasMismatch ? (
        <section
          className="source-snapshot-mismatch-panel"
          aria-label={t("sourceGroup.mismatchTitle")}
          role="alert"
        >
          <div className="mismatch-heading">
            <strong>{t("sourceGroup.mismatchTitle")}</strong>
            <p>{t("sourceGroup.mismatchBody")}</p>
          </div>
          <div className="mismatch-actions">
            {confirming === "restore" ? (
              <div className="inline-confirmation">
                <span>{t("sourceGroup.restoreConfirmBody")}</span>
                <button
                  type="button"
                  className="repair-button"
                  disabled={actionActivity}
                  onClick={() => {
                    setConfirming(null);
                    onRestore?.(source.remoteId);
                  }}
                >
                  {t("sourceGroup.restoreConfirmButton")}
                </button>
                <button
                  type="button"
                  disabled={actionActivity}
                  onClick={() => setConfirming(null)}
                >
                  {t("sourceGroup.cancel")}
                </button>
              </div>
            ) : (
              <button
                type="button"
                className="repair-button"
                disabled={actionActivity}
                onClick={() => setConfirming("restore")}
              >
                {t("sourceGroup.restoreRelease")}
              </button>
            )}

            {localCopyControls}
          </div>
        </section>
      ) : null}
      {!hasMismatch && localCopyControls ? (
        <section
          className="source-group-local-copy-panel"
          aria-label={t("sourceGroup.copyMemberLabel")}
        >
          {localCopyControls}
        </section>
      ) : null}

      {/* Source-level Update and whole-source Remove */}
      <div className="source-group-actions">
        {onUpdate ? (
          <button
            type="button"
            className="repair-button"
            disabled={actionActivity || hasMismatch}
            onClick={(event) => {
              setConfirming(null);
              onUpdate(source.remoteId, event.currentTarget);
            }}
          >
            {t("sourceGroup.update")}
          </button>
        ) : null}

        {onRemove ? (
          confirming === "remove" ? (
            <div className="inline-confirmation">
              <span>{t("sourceGroup.removeConfirmBody")}</span>
              <button
                type="button"
                className="repair-button danger-button"
                disabled={actionActivity}
                onClick={() => {
                  setConfirming(null);
                  onRemove(source.remoteId);
                }}
              >
                {t("sourceGroup.removeConfirmButton")}
              </button>
              <button
                type="button"
                disabled={actionActivity}
                onClick={() => setConfirming(null)}
              >
                {t("sourceGroup.cancel")}
              </button>
            </div>
          ) : (
            <button
              type="button"
              className="repair-button"
              disabled={actionActivity}
              onClick={() => setConfirming("remove")}
            >
              {t("sourceGroup.remove")}
            </button>
          )
        ) : null}
      </div>

      {actionNotice && actionNotice.message !== null ? (
        <p
          className="source-group-action-notice"
          role={actionNotice.ok ? "status" : "alert"}
        >
          {actionNotice.message}
        </p>
      ) : null}

      {/* Read-only member rows */}
      <section
        className="source-group-members"
        aria-label={t("sourceGroup.membersLabel")}
      >
        <h4>{t("sourceGroup.membersLabel")}</h4>
        {source.members.length === 0 ? (
          <p className="source-group-empty-members">
            {t("sourceGroup.emptyMembers")}
          </p>
        ) : (
          <ul className="source-member-list">
            {source.members.map((member) => {
              const skill = skills.find((s) => s.id === member.skillId);
              const isBroken = !member.presence || skill?.health === "broken";
              const targetCount = skill?.enabledAgentCount ?? 0;

              return (
                <li key={member.skillId} className="source-member-row">
                  <div className="member-path-column">
                    <span className="member-skill-path">
                      {member.skillPath}
                    </span>
                    {!member.presence ? (
                      <div className="member-tombstone-info">
                        <span className="tombstone-badge">
                          {t("sourceGroup.tombstoned")}
                        </span>
                        <small className="tombstone-hint">
                          {t("sourceGroup.autoRecoverHint")}
                        </small>
                      </div>
                    ) : null}
                  </div>

                  <div className="member-health-column">
                    <span
                      className={`member-health-badge member-health-badge--${skill?.health ?? "broken"}`}
                    >
                      {memberHealthLabel(skill?.health, t)}
                    </span>
                  </div>

                  <div className="member-target-column">
                    <span className="member-target-count">
                      {tPlural("sourceGroup.targetGroupsCount", targetCount)}
                    </span>
                  </div>

                  <div className="member-action-column">
                    {isBroken && onOpenBrokenDisable ? (
                      <button
                        type="button"
                        className="repair-button danger-button"
                        disabled={actionActivity}
                        onClick={(event) =>
                          onOpenBrokenDisable(
                            member.skillId,
                            member.skillPath,
                            event.currentTarget,
                          )
                        }
                      >
                        {t("sourceGroup.memberBrokenDisable")}
                      </button>
                    ) : null}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>
    </article>
  );
}

function memberHealthLabel(
  health: SkillSummary["health"] | undefined,
  t: LocaleContextValue["t"],
): string {
  const value = health ?? "broken";
  return t(`library.health.badge.${value}` as MessageKey);
}
