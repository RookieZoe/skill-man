import { useBackgroundOperations } from "../../ui/BackgroundOperations";
import { useEffect, useLayoutEffect, useId, useRef, useState } from "react";
import { PluginGroups } from "./PluginGroups";
import { SelectionControls } from "../../ui/SelectionControls";
import { createPortal } from "react-dom";
import type {
  GitSourceCapabilitySource,
  SkillSummary,
} from "../../app/catalog-client";
import { useLocale, type LocaleContextValue } from "../locale/LocaleProvider";
import type { MessageKey } from "../locale/messages";
import type { SourceActionNotice } from "./GitSourceCapabilityNotice";
import { defaultCopyDestinationPicker } from "./GitSourceCapabilityNotice";

export interface SourceGroupCardProps {
  embedded?: boolean;
  defaultCollapsed?: boolean;
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
  ) => void | Promise<boolean>;
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
  embedded = false,
  defaultCollapsed = false,
  source,
  skills,
  actionActivity = false,
  onPromote,
  onUpdate,
  onRestore,
  onCopyMember,
  onRemove,
  onOpenBrokenDisable,
  pickDirectory = defaultCopyDestinationPicker,
}: SourceGroupCardProps) {
  const { t, tPlural } = useLocale();
  const notifications = useBackgroundOperations();
  const [expanded, setExpanded] = useState(!defaultCollapsed);
  const [removeTrigger, setRemoveTrigger] = useState<HTMLButtonElement | null>(
    null,
  );
  const [confirming, setConfirming] = useState<"restore" | "remove" | null>(
    null,
  );
  const copyableMembers = source.members.filter((m) => m.presence);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [copying, setCopying] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

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

  const selectedMembers = copyableMembers.filter((member) =>
    selectedIds.includes(member.skillId),
  );

  async function handleCopy() {
    if (!onCopyMember || copying || actionActivity || !selectedMembers.length)
      return;
    setCopying(true);
    let completed = 0;
    let noticeId: string | null = null;
    try {
      const parent = await pickDirectory();
      if (!parent || !mounted.current) return;
      noticeId = notifications.begin({
        title: t("operation.background.copy"),
        detail: t("operation.background.working"),
      });
      // Once confirmed, the batch belongs to the workspace, not this card's mount.
      for (const member of selectedMembers) {
        const name = member.skillPath.split("/").at(-1);
        if (!name || name === "." || name === ".." || name.includes("\\"))
          throw new Error();
        const ok = await onCopyMember(
          source.remoteId,
          member.skillId,
          parent.replace(/\/$/, "") + "/" + name,
        );
        if (ok === false) throw new Error();
        completed++;
        setSelectedIds((ids) => ids.filter((id) => id !== member.skillId));
      }
      notifications.finish(noticeId, {
        title: t(
          completed === selectedMembers.length
            ? "operation.background.done"
            : "operation.background.partial",
        ),
        detail:
          t("sourceGroup.copyBatchResult", {
            completed,
            total: selectedMembers.length,
          }) +
          " " +
          t("operation.background.copy_next"),
        state: completed === selectedMembers.length ? "completed" : "partial",
      });
    } catch (reason) {
      if (noticeId)
        notifications.finish(noticeId, {
          title: t(
            completed
              ? "operation.background.partial"
              : "operation.background.failed",
          ),
          detail:
            t("sourceGroup.copyBatchFailed", {
              completed,
              total: selectedMembers.length,
            }) +
            " " +
            (reason instanceof Error && reason.message
              ? reason.message + " "
              : "") +
            t("operation.background.copy_next"),
          state: completed ? "partial" : "failed",
        });
    } finally {
      setCopying(false);
    }
  }

  const overrideText =
    source.trackingMode && source.trackingMode !== "auto_release_tag_head"
      ? source.trackingValue || trackingModeLabel(source.trackingMode, t)
      : t("sourceGroup.overrideNone");

  const Container = embedded ? "article" : "details";
  const Heading = embedded ? "header" : "summary";
  return (
    <Container
      {...(!embedded
        ? {
            open: expanded,
            onToggle: (event: React.SyntheticEvent<HTMLDetailsElement>) =>
              setExpanded(event.currentTarget.open),
          }
        : {})}
      className={`source-group-card${hasMismatch ? " source-group-card--mismatch" : ""}`}
      aria-label={t("sourceGroup.label")}
    >
      <Heading
        className={`source-group-header${embedded ? "" : " source-group-summary"}`}
      >
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
      </Heading>

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
          </div>
        </section>
      ) : null}

      {/* Source-level Update and whole-source Remove */}
      <div className="source-group-actions">
        {onCopyMember && (
          <button
            type="button"
            className="repair-button source-copy-batch-button"
            disabled={actionActivity || copying || !selectedMembers.length}
            title={t("sourceGroup.copyBatchHint")}
            onClick={() => void handleCopy()}
          >
            {t(
              copying
                ? "sourceGroup.copyBatchBusy"
                : "sourceGroup.createLocalCopy",
            )}
          </button>
        )}
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
          <>
            <button
              type="button"
              className="repair-button"
              disabled={actionActivity}
              aria-haspopup="dialog"
              aria-expanded={confirming === "remove"}
              onClick={(event) => {
                setRemoveTrigger(event.currentTarget);
                setConfirming(confirming === "remove" ? null : "remove");
              }}
            >
              {t("sourceGroup.remove")}
            </button>
            {confirming === "remove" && expanded && !actionActivity && (
              <RemoveSourceConfirmation
                trigger={removeTrigger}
                sourceUrl={source.canonicalUrl}
                onClose={() => setConfirming(null)}
                onConfirm={() => {
                  setConfirming(null);
                  onRemove(source.remoteId);
                }}
              />
            )}
          </>
        ) : null}
      </div>

      {/* Read-only member rows */}
      <section
        className="source-group-members"
        aria-label={t("sourceGroup.membersLabel")}
      >
        <h4>{t("sourceGroup.membersLabel")}</h4>
        {onCopyMember && (
          <SelectionControls
            ids={copyableMembers.map((member) => member.skillId)}
            selected={selectedIds}
            onChange={setSelectedIds}
            disabled={copying || actionActivity}
          />
        )}
        {source.members.length === 0 ? (
          <p className="source-group-empty-members">
            {t("sourceGroup.emptyMembers")}
          </p>
        ) : (
          <PluginGroups
            items={source.members}
            pluginName={(member) => member.pluginName}
          >
            {(members) => (
              <ul className="source-member-list">
                {members.map((member) => {
                  const skill = skills.find((s) => s.id === member.skillId);
                  const isBroken =
                    !member.presence || skill?.health === "broken";
                  const targetCount = skill?.enabledAgentCount ?? 0;

                  return (
                    <li
                      key={member.skillId}
                      className={`source-member-row${onCopyMember ? " source-member-row--selectable" : ""}`}
                    >
                      {onCopyMember && (
                        <input
                          type="checkbox"
                          className="source-member-checkbox"
                          aria-label={member.skillPath}
                          checked={
                            member.presence &&
                            selectedIds.includes(member.skillId)
                          }
                          disabled={
                            !member.presence || copying || actionActivity
                          }
                          onChange={() =>
                            setSelectedIds((ids) =>
                              ids.includes(member.skillId)
                                ? ids.filter((id) => id !== member.skillId)
                                : [...ids, member.skillId],
                            )
                          }
                        />
                      )}
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
                          {tPlural(
                            "sourceGroup.targetGroupsCount",
                            targetCount,
                          )}
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
          </PluginGroups>
        )}
      </section>
    </Container>
  );
}

function RemoveSourceConfirmation({
  trigger,
  sourceUrl,
  onClose,
  onConfirm,
}: {
  trigger: HTMLButtonElement | null;
  sourceUrl: string;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { t } = useLocale();
  const id = useId();
  const panel = useRef<HTMLDivElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  useLayoutEffect(() => {
    const element = panel.current;
    if (!element || !trigger) return;
    const anchor = trigger.getBoundingClientRect();
    const bounds = element.getBoundingClientRect();
    const gap = 8;
    const left = Math.max(
      gap,
      Math.min(anchor.left, window.innerWidth - bounds.width - gap),
    );
    const top =
      anchor.bottom + gap + bounds.height <= window.innerHeight - gap
        ? anchor.bottom + gap
        : Math.max(gap, anchor.top - bounds.height - gap);
    element.style.left = `${left}px`;
    element.style.top = `${top}px`;
    cancel.current?.focus({ preventScroll: true });
  }, [trigger]);
  useEffect(() => {
    function outside(event: Event) {
      if (
        event.target instanceof Node &&
        !panel.current?.contains(event.target) &&
        !trigger?.contains(event.target)
      )
        onClose();
    }
    function escape(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
      trigger?.focus({ preventScroll: true });
    }
    function scroll(event: Event) {
      if (event.target instanceof Node && panel.current?.contains(event.target))
        return;
      onClose();
    }
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", outside);
    document.addEventListener("keydown", escape, true);
    document.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", onClose);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("focusin", outside);
      document.removeEventListener("keydown", escape, true);
      document.removeEventListener("scroll", scroll, true);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose, trigger]);
  return createPortal(
    <div
      ref={panel}
      className="source-remove-popover"
      role="dialog"
      aria-labelledby={`${id}-title`}
      aria-describedby={`${id}-body`}
    >
      <h4 id={`${id}-title`}>{t("sourceGroup.removeConfirmTitle")}</h4>
      <p className="source-remove-popover-url">{sourceUrl}</p>
      <p id={`${id}-body`}>{t("sourceGroup.removeConfirmBody")}</p>
      <div className="source-remove-popover-actions">
        <button
          ref={cancel}
          type="button"
          className="toolbar-button"
          onClick={() => {
            onClose();
            trigger?.focus({ preventScroll: true });
          }}
        >
          {t("sourceGroup.cancel")}
        </button>
        <button
          type="button"
          className="toolbar-button source-remove-confirm"
          onClick={onConfirm}
        >
          {t("sourceGroup.removeConfirmButton")}
        </button>
      </div>
    </div>,
    document.body,
  );
}

function memberHealthLabel(
  health: SkillSummary["health"] | undefined,
  t: LocaleContextValue["t"],
): string {
  const value = health ?? "broken";
  return t(`library.health.badge.${value}` as MessageKey);
}

function trackingModeLabel(mode: string, t: LocaleContextValue["t"]): string {
  const keys: Record<string, MessageKey> = {
    prerelease_channel: "library.source_group.policy_prerelease",
    fixed_tag: "library.source_group.policy_fixed_tag",
    fixed_commit: "library.source_group.policy_fixed_commit",
    branch: "library.source_group.policy_branch",
    head: "library.source_group.policy_head",
  };
  const key = keys[mode];
  return key ? t(key) : t("sourceGroup.override");
}
