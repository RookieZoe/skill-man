import { useMemo, useState, type Ref } from "react";

import type {
  ModifiedMemberResolution,
  SourcePromotionDraft,
  SourcePromotionResult,
  SourcePromotionResolution,
  UpstreamMemberRemovedResolution,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import type { MessageKey } from "../locale/messages";

type SourceFlowMode = "promotion" | "update";

function sourceCopyKey(mode: SourceFlowMode, key: string): MessageKey {
  return `library.source_${mode}.${key}` as MessageKey;
}

type DraftResolution = {
  modified: ModifiedMemberResolution | null;
  removed: UpstreamMemberRemovedResolution | null;
};

function initialResolutions(draft: SourcePromotionDraft) {
  return Object.fromEntries(
    draft.existingMembers
      .filter((member) => member.state !== "update_to_target")
      .map((member) => [member.skillId, { modified: null, removed: null }]),
  ) as Record<string, DraftResolution>;
}

export function SourcePromotionFlow({
  draft,
  result,
  error,
  activity,
  onConfirm,
  onUndo,
  onClose,
  initialFocusRef,
  mode = "promotion",
}: {
  draft: SourcePromotionDraft | null;
  result: SourcePromotionResult | null;
  error: string | null;
  activity: "idle" | "fetching" | "confirming" | "undoing";
  onConfirm: (resolutions: SourcePromotionResolution[]) => void;
  onUndo: () => void;
  onClose: () => void;
  initialFocusRef: Ref<HTMLButtonElement>;
  mode?: SourceFlowMode;
}) {
  const { t } = useLocale();
  const isBusy = activity !== "idle";

  if (result) {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t(sourceCopyKey(mode, "complete_eyebrow"))}
          </span>
          <h2>{t(sourceCopyKey(mode, "complete_title"))}</h2>
          <p>
            {t(sourceCopyKey(mode, "complete_body"), {
              count: result.memberCount,
            })}
          </p>
        </div>
        <dl className="activation-paths source-group-facts">
          <div>
            <dt>{t("library.source_group.release")}</dt>
            <dd>
              <code>{result.releaseId}</code>
            </dd>
          </div>
          <div>
            <dt>{t("library.source_group.resolved_commit")}</dt>
            <dd>
              <code>{result.resolvedCommit}</code>
            </dd>
          </div>
        </dl>
        <div className="activation-sheet-actions">
          {result.undoAvailable ? (
            <button type="button" disabled={isBusy} onClick={onUndo}>
              {t("library.source_group.undo")}
            </button>
          ) : null}
          <button ref={initialFocusRef} type="button" onClick={onClose}>
            {t("library.source_group.close")}
          </button>
        </div>
      </>
    );
  }

  if (!draft) {
    return (
      <div className="activation-sheet-heading">
        <span className="eyebrow">{t(sourceCopyKey(mode, "eyebrow"))}</span>
        <h2>{t(sourceCopyKey(mode, "loading_title"))}</h2>
        <p>{t(sourceCopyKey(mode, "loading_body"))}</p>
        {error ? (
          <div className="activation-error" role="alert">
            {error}
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button ref={initialFocusRef} type="button" onClick={onClose}>
            {t("library.source_group.close")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <SourcePromotionDraftContent
      key={`${draft.remoteId}:${draft.resolvedCommit}`}
      draft={draft}
      error={error}
      activity={activity}
      onConfirm={onConfirm}
      onClose={onClose}
      initialFocusRef={initialFocusRef}
      mode={mode}
    />
  );
}

function SourcePromotionDraftContent({
  draft,
  error,
  activity,
  onConfirm,
  onClose,
  initialFocusRef,
  mode,
}: {
  draft: SourcePromotionDraft;
  error: string | null;
  activity: "idle" | "fetching" | "confirming" | "undoing";
  onConfirm: (resolutions: SourcePromotionResolution[]) => void;
  onClose: () => void;
  initialFocusRef: Ref<HTMLButtonElement>;
  mode: SourceFlowMode;
}) {
  const { t } = useLocale();
  const [resolutions, setResolutions] = useState(() =>
    initialResolutions(draft),
  );
  const isBusy = activity !== "idle";
  const availableMappingTargets = useMemo(
    () => draft.targetMembers.filter((member) => member.legacySkillId === null),
    [draft.targetMembers],
  );

  const updateResolution = (
    skillId: string,
    update: Partial<DraftResolution>,
  ) => {
    setResolutions((current) => ({
      ...current,
      [skillId]: { ...current[skillId], ...update },
    }));
  };
  const allResolved = draft.existingMembers
    .filter((member) => member.state !== "update_to_target")
    .every((member) => {
      const resolution = resolutions[member.skillId];
      if (!resolution) return false;
      if (member.state === "modified_member_resolution_required") {
        return resolution.modified !== null;
      }
      if (!resolution.removed) return false;
      if (resolution.removed.kind === "local_link") {
        return Boolean(resolution.removed.targetDirectory.trim());
      }
      if (resolution.removed.kind === "explicit_member_mapping") {
        return (
          Boolean(resolution.removed.targetSkillPath) &&
          (!member.modified || resolution.modified !== null)
        );
      }
      return true;
    });
  const submit = () => {
    const confirmed = draft.existingMembers
      .filter((member) => member.state !== "update_to_target")
      .map((member) => ({
        skillId: member.skillId,
        modified: resolutions[member.skillId]?.modified ?? null,
        removed: resolutions[member.skillId]?.removed ?? null,
      }));
    onConfirm(confirmed);
  };

  return (
    <>
      <div className="activation-sheet-heading">
        <span className="eyebrow">{t(sourceCopyKey(mode, "eyebrow"))}</span>
        <h2>{t(sourceCopyKey(mode, "title"))}</h2>
        <p>{t(sourceCopyKey(mode, "body"))}</p>
      </div>
      <dl className="activation-paths source-group-facts">
        <div>
          <dt>{t("library.source_group.provider")}</dt>
          <dd>{draft.provider}</dd>
        </div>
        <div>
          <dt>{t("library.source_group.source")}</dt>
          <dd>{draft.canonicalUrl}</dd>
        </div>
        <div>
          <dt>{t("library.source_group.tracking_ref")}</dt>
          <dd>{draft.trackingRef}</dd>
        </div>
        <div>
          <dt>{t("library.source_group.resolved_commit")}</dt>
          <dd>
            <code>{draft.resolvedCommit}</code>
          </dd>
        </div>
      </dl>
      <section
        className="source-group-members"
        aria-label={t(sourceCopyKey(mode, "members"))}
      >
        <h3>{t(sourceCopyKey(mode, "members"))}</h3>
        <ul className="git-import-candidates">
          {draft.existingMembers.map((member) => {
            const resolution = resolutions[member.skillId];
            return (
              <li key={member.skillId}>
                <div>
                  <strong>{member.directoryName}</strong>
                  <span className="candidate-path">
                    {member.skillPath || t("library.import.repo_root")}
                  </span>
                  <small>
                    {t(`library.source_promotion.state.${member.state}`)}
                  </small>
                </div>
                {member.state === "modified_member_resolution_required" ? (
                  <fieldset
                    className="source-promotion-resolution"
                    disabled={isBusy}
                  >
                    <legend>{t("library.source_promotion.modified")}</legend>
                    <label>
                      <input
                        type="radio"
                        name={`${member.skillId}-modified`}
                        checked={resolution?.modified === "keep_modified"}
                        onChange={() =>
                          updateResolution(member.skillId, {
                            modified: "keep_modified",
                          })
                        }
                      />
                      {t("library.source_promotion.keep_modified")}
                    </label>
                    <label>
                      <input
                        type="radio"
                        name={`${member.skillId}-modified`}
                        checked={resolution?.modified === "replace_with_target"}
                        onChange={() =>
                          updateResolution(member.skillId, {
                            modified: "replace_with_target",
                          })
                        }
                      />
                      {t("library.source_promotion.replace_target")}
                    </label>
                  </fieldset>
                ) : null}
                {member.state === "upstream_member_removed" ? (
                  <fieldset
                    className="source-promotion-resolution"
                    disabled={isBusy}
                  >
                    <legend>{t("library.source_promotion.removed")}</legend>
                    <label>
                      <input
                        type="radio"
                        name={`${member.skillId}-removed`}
                        checked={resolution?.removed?.kind === "remove"}
                        onChange={() =>
                          updateResolution(member.skillId, {
                            removed: { kind: "remove" },
                            modified: null,
                          })
                        }
                      />
                      {t("library.source_promotion.remove")}
                    </label>
                    <label>
                      <input
                        type="radio"
                        name={`${member.skillId}-removed`}
                        checked={resolution?.removed?.kind === "local_link"}
                        onChange={() =>
                          updateResolution(member.skillId, {
                            removed: {
                              kind: "local_link",
                              targetDirectory: "",
                            },
                            modified: null,
                          })
                        }
                      />
                      {t("library.source_promotion.local_link")}
                    </label>
                    {resolution?.removed?.kind === "local_link" ? (
                      <input
                        aria-label={t(
                          "library.source_promotion.local_link_label",
                        )}
                        value={resolution.removed.targetDirectory}
                        onChange={(event) =>
                          updateResolution(member.skillId, {
                            removed: {
                              kind: "local_link",
                              targetDirectory: event.currentTarget.value,
                            },
                          })
                        }
                      />
                    ) : null}
                    <label>
                      <input
                        type="radio"
                        name={`${member.skillId}-removed`}
                        checked={
                          resolution?.removed?.kind ===
                          "explicit_member_mapping"
                        }
                        onChange={() =>
                          updateResolution(member.skillId, {
                            removed: {
                              kind: "explicit_member_mapping",
                              targetSkillPath: "",
                            },
                          })
                        }
                      />
                      {t("library.source_promotion.mapping")}
                    </label>
                    {resolution?.removed?.kind === "explicit_member_mapping" ? (
                      <>
                        <select
                          aria-label={t(
                            "library.source_promotion.mapping_label",
                          )}
                          value={resolution.removed.targetSkillPath}
                          onChange={(event) =>
                            updateResolution(member.skillId, {
                              removed: {
                                kind: "explicit_member_mapping",
                                targetSkillPath: event.currentTarget.value,
                              },
                            })
                          }
                        >
                          <option value="">
                            {t("library.source_promotion.select_mapping")}
                          </option>
                          {availableMappingTargets.map((target) => (
                            <option
                              key={target.member.skillPath}
                              value={target.member.skillPath}
                            >
                              {target.member.skillPath ||
                                t("library.import.repo_root")}
                            </option>
                          ))}
                        </select>
                        {member.modified ? (
                          <div>
                            <label>
                              <input
                                type="radio"
                                name={`${member.skillId}-mapped-modified`}
                                checked={
                                  resolution.modified === "keep_modified"
                                }
                                onChange={() =>
                                  updateResolution(member.skillId, {
                                    modified: "keep_modified",
                                  })
                                }
                              />
                              {t("library.source_promotion.keep_modified")}
                            </label>
                            <label>
                              <input
                                type="radio"
                                name={`${member.skillId}-mapped-modified`}
                                checked={
                                  resolution.modified === "replace_with_target"
                                }
                                onChange={() =>
                                  updateResolution(member.skillId, {
                                    modified: "replace_with_target",
                                  })
                                }
                              />
                              {t("library.source_promotion.replace_target")}
                            </label>
                          </div>
                        ) : null}
                      </>
                    ) : null}
                  </fieldset>
                ) : null}
              </li>
            );
          })}
        </ul>
      </section>
      <section
        className="source-group-members"
        aria-label={t(sourceCopyKey(mode, "target_members"))}
      >
        <h3>{t(sourceCopyKey(mode, "target_members"))}</h3>
        <ul className="git-import-candidates">
          {draft.targetMembers.map((target) => (
            <li key={target.member.skillPath || target.member.directoryName}>
              <div>
                <strong>{target.member.displayName}</strong>
                <span className="candidate-path">
                  {target.member.skillPath || t("library.import.repo_root")}
                </span>
                {target.member.description ? (
                  <small>{target.member.description}</small>
                ) : null}
              </div>
              <dl>
                <div>
                  <dt>{t("library.source_group.tree_summary")}</dt>
                  <dd>
                    <code>{target.member.treeSummary}</code>
                  </dd>
                </div>
              </dl>
            </li>
          ))}
        </ul>
      </section>
      {error ? (
        <div className="activation-error" role="alert">
          {error}
        </div>
      ) : null}
      <div className="activation-sheet-actions">
        <button
          ref={initialFocusRef}
          type="button"
          disabled={isBusy}
          onClick={onClose}
        >
          {t("library.import.cancel")}
        </button>
        <button
          type="button"
          className="activation-confirm-button"
          disabled={isBusy || !allResolved}
          onClick={submit}
        >
          {activity === "confirming"
            ? t(sourceCopyKey(mode, "confirming"))
            : t(sourceCopyKey(mode, "confirm"))}
        </button>
      </div>
    </>
  );
}
