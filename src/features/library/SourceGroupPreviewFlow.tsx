import type {
  ExternalOwnershipClaim,
  GitRepositorySourceType,
  SourceGroupMember,
  SourceGroupPreviewOutcome,
  SourcePromotionDraft,
  SourcePromotionDraftOutcome,
  SourceTransitionResult,
  SourceUpdateDraft,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

const PARAMETERISED_MODES = [
  "prerelease_channel",
  "fixed_tag",
  "fixed_commit",
  "branch",
];

export function SourceGroupPreviewFlow({
  sourceType,
  sourceUrl,
  policyMode,
  policyValue,
  outcome,
  promotionDraft,
  promotionOutcome,
  updateDraft,
  result,
  error,
  activity,
  onSourceTypeChange,
  onSourceUrlChange,
  onSourceGroupPolicyChange,
  onFetch,
  onConfirm,
  onConfirmPromotion,
  onUndo,
  onClose,
}: {
  sourceType: GitRepositorySourceType;
  sourceUrl: string;
  policyMode: string;
  policyValue: string;
  outcome: SourceGroupPreviewOutcome | null;
  promotionDraft: SourcePromotionDraft | null;
  promotionOutcome: SourcePromotionDraftOutcome | null;
  updateDraft: SourceUpdateDraft | null;
  result: SourceTransitionResult | null;
  error: string | null;
  activity: "idle" | "fetching" | "confirming" | "undoing";
  onSourceTypeChange: (sourceType: GitRepositorySourceType) => void;
  onSourceUrlChange: (sourceUrl: string) => void;
  onSourceGroupPolicyChange: (mode: string, value: string) => void;
  onFetch: () => void;
  onConfirm: () => void;
  onConfirmPromotion: () => void;
  onUndo: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const isBusy = activity !== "idle";

  if (result) {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.source_group.complete_eyebrow")}
          </span>
          <h2>{t("library.source_group.complete_title")}</h2>
          <p>
            {t("library.source_group.complete_body", {
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
          <button type="button" disabled={isBusy} onClick={onClose}>
            {t("library.source_group.close")}
          </button>
          {result.undoAvailable ? (
            <button
              type="button"
              className="activation-confirm-button"
              disabled={isBusy}
              onClick={onUndo}
            >
              {activity === "undoing"
                ? t("library.source_group.undoing")
                : t("library.source_group.undo")}
            </button>
          ) : null}
        </div>
      </>
    );
  }

  if (promotionDraft) {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.source_group.promotion_eyebrow")}
          </span>
          <h2>{t("library.source_group.promotion_title")}</h2>
          <p>
            {t("library.source_group.promotion_body", {
              count: promotionDraft.legacyMemberCount,
            })}
          </p>
        </div>
        <dl className="activation-paths source-group-facts">
          <div>
            <dt>{t("library.source_group.source")}</dt>
            <dd>{promotionDraft.sourceUrl}</dd>
          </div>
          <div>
            <dt>{t("library.source_group.selection_kind")}</dt>
            <dd>{promotionDraft.policy.selectionKind}</dd>
          </div>
          <div>
            <dt>{t("library.source_group.selected_ref")}</dt>
            <dd>
              <code>{promotionDraft.policy.selectedRef}</code>
            </dd>
          </div>
          <div>
            <dt>{t("library.source_group.resolved_commit")}</dt>
            <dd>
              <code>{promotionDraft.policy.resolvedCommit}</code>
            </dd>
          </div>
        </dl>
        <PromotionManifest draft={promotionDraft} />
        {error ? (
          <div className="activation-error" role="alert">
            <strong>{t("library.import.source_unavailable")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button type="button" disabled={isBusy} onClick={onClose}>
            {t("library.source_group.close")}
          </button>
          <button
            type="button"
            className="activation-confirm-button"
            disabled={isBusy}
            onClick={onConfirmPromotion}
          >
            {activity === "confirming"
              ? t("library.source_group.confirming")
              : t("library.source_group.promotion_confirm")}
          </button>
        </div>
      </>
    );
  }

  if (updateDraft) {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.source_group.update_eyebrow")}
          </span>
          <h2>{t("library.source_group.update_title")}</h2>
        </div>
        <section
          className="source-group-members"
          aria-label={t("library.source_group.members")}
        >
          <h3>{t("library.source_group.members")}</h3>
          <ul className="git-import-candidates">
            {updateDraft.members.map((member) => (
              <li key={member.skillPath || member.directoryName}>
                <div className="source-group-member-copy">
                  <strong>{member.displayName}</strong>
                  <span className="candidate-path">
                    {member.skillPath || t("library.import.repo_root")}
                  </span>
                  <span className="source-group-member-action">
                    {t(`library.source_group.member_${member.state}`)}
                  </span>
                </div>
              </li>
            ))}
          </ul>
        </section>
        <div className="activation-sheet-actions">
          <button type="button" onClick={onClose}>
            {t("library.source_group.close")}
          </button>
        </div>
      </>
    );
  }

  if (outcome?.kind === "preview") {
    const { preview } = outcome;
    const hasExternalOwnershipClaims =
      preview.externalOwnershipClaims.length > 0;
    const hasCompleteExternalClaims =
      preview.members.length > 0 &&
      preview.externalOwnershipClaims.length === preview.members.length &&
      preview.members.every((member) =>
        preview.externalOwnershipClaims.some(
          (claim) => claim.entryName === member.directoryName,
        ),
      );
    const replacementBlocked =
      hasExternalOwnershipClaims && !hasCompleteExternalClaims;
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.source_group.preview_eyebrow")}
          </span>
          <h2>{t("library.source_group.preview_title")}</h2>
          <p>{t("library.source_group.body")}</p>
        </div>
        <dl className="activation-paths source-group-facts">
          <div>
            <dt>{t("library.source_group.provider")}</dt>
            <dd>{preview.provider}</dd>
          </div>
          <div>
            <dt>{t("library.source_group.source")}</dt>
            <dd>{preview.sourceUrl}</dd>
          </div>
          <div>
            <dt>{t("library.source_group.policy_mode")}</dt>
            <dd>{preview.policy.mode}</dd>
          </div>
          <div>
            <dt>{t("library.source_group.selection_kind")}</dt>
            <dd>{preview.policy.selectionKind}</dd>
          </div>
          <div>
            <dt>{t("library.source_group.selected_ref")}</dt>
            <dd>
              <code>{preview.policy.selectedRef}</code>
            </dd>
          </div>
          <div>
            <dt>{t("library.source_group.resolved_commit")}</dt>
            <dd>
              <code>{preview.policy.resolvedCommit}</code>
            </dd>
          </div>
        </dl>
        <section
          className="source-group-members"
          aria-label={t("library.source_group.members")}
        >
          <h3>{t("library.source_group.members")}</h3>
          <ul className="git-import-candidates">
            {preview.members.map((member) => (
              <li key={member.skillPath || member.directoryName}>
                <div className="source-group-member-copy">
                  <strong>{member.displayName}</strong>
                  <span className="candidate-path">
                    {member.skillPath || t("library.import.repo_root")}
                  </span>
                  <span className="source-group-member-action">
                    {t(`library.source_group.member_${member.action}`)}
                  </span>
                  {member.description ? (
                    <small>{member.description}</small>
                  ) : null}
                </div>
                <dl className="source-group-member-tree">
                  <div>
                    <dt>{t("library.source_group.tree_summary")}</dt>
                    <dd>
                      <code>{member.treeSummary}</code>
                    </dd>
                  </div>
                </dl>
              </li>
            ))}
          </ul>
        </section>
        <ExternalClaims claims={preview.externalOwnershipClaims} />
        {replacementBlocked ? (
          <div className="activation-error" role="alert">
            <strong>{t("library.source_group.incomplete_claims_title")}</strong>
            <span>
              {t("library.source_group.incomplete_claims_body", {
                memberCount: preview.members.length,
                claimCount: preview.externalOwnershipClaims.length,
              })}
            </span>
          </div>
        ) : null}
        {error ? (
          <div className="activation-error" role="alert">
            <strong>{t("library.import.source_unavailable")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button type="button" disabled={isBusy} onClick={onClose}>
            {t("library.source_group.close")}
          </button>
          <button
            type="button"
            className="activation-confirm-button"
            disabled={isBusy || replacementBlocked}
            onClick={onConfirm}
          >
            {activity === "confirming"
              ? t("library.source_group.confirming")
              : t("library.source_group.confirm")}
          </button>
        </div>
      </>
    );
  }

  if (outcome?.kind === "repository_ref_conflict") {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.source_group.ref_conflict_eyebrow")}
          </span>
          <h2>{t("library.source_group.ref_conflict_title")}</h2>
          <p>{t("library.source_group.ref_conflict_body")}</p>
        </div>
        <label className="import-source-field">
          <span>{t("library.source_group.ref_label")}</span>
          <select
            value={policyValue}
            disabled={isBusy}
            onChange={(event) =>
              onSourceGroupPolicyChange("branch", event.currentTarget.value)
            }
          >
            <option value="">
              {t("library.source_group.ref_placeholder")}
            </option>
            {outcome.conflict.availableRefs.map((ref) => (
              <option key={ref} value={ref}>
                {ref}
              </option>
            ))}
          </select>
        </label>
        <ExternalClaims claims={outcome.conflict.externalOwnershipClaims} />
        <div className="activation-sheet-actions">
          <button type="button" disabled={isBusy} onClick={onClose}>
            {t("library.import.cancel")}
          </button>
          <button
            type="button"
            className="activation-confirm-button"
            disabled={!policyValue.trim() || isBusy}
            onClick={onFetch}
          >
            {isBusy
              ? t("library.source_group.fetching")
              : t("library.source_group.select_ref")}
          </button>
        </div>
      </>
    );
  }

  if (outcome?.kind === "repository_ownership_split") {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.source_group.ownership_split_eyebrow")}
          </span>
          <h2>{t("library.source_group.ownership_split_title")}</h2>
          <p>{t("library.source_group.ownership_split_body")}</p>
        </div>
        <section
          className="source-group-claims"
          aria-label={t("library.source_group.lock_paths")}
        >
          <h3>{t("library.source_group.lock_paths")}</h3>
          <ul>
            {outcome.split.lockPaths.map((path) => (
              <li key={path}>
                <code>{path}</code>
              </li>
            ))}
          </ul>
        </section>
        <ExternalClaims claims={outcome.split.externalOwnershipClaims} />
        <div className="activation-sheet-actions">
          <button type="button" onClick={onClose}>
            {t("library.source_group.close")}
          </button>
        </div>
      </>
    );
  }

  if (
    promotionOutcome?.kind === "repository_ref_conflict" ||
    promotionOutcome?.kind === "repository_ownership_split"
  ) {
    const conflict =
      promotionOutcome.kind === "repository_ref_conflict"
        ? promotionOutcome.conflict
        : null;
    return (
      <div className="activation-error" role="alert">
        <strong>
          {conflict
            ? t("library.source_group.ref_conflict_title")
            : t("library.source_group.ownership_split_title")}
        </strong>
        <span>
          {conflict
            ? t("library.source_group.ref_conflict_body")
            : t("library.source_group.ownership_split_body")}
        </span>
      </div>
    );
  }

  return (
    <>
      <div className="activation-sheet-heading">
        <span className="eyebrow">{t("library.source_group.eyebrow")}</span>
        <h2>{t("library.source_group.title")}</h2>
        <p>{t("library.source_group.body")}</p>
      </div>
      <label className="import-source-field">
        <span>{t("library.source_group.source_type")}</span>
        <select
          value={sourceType}
          disabled={isBusy}
          onChange={(event) =>
            onSourceTypeChange(
              event.currentTarget.value as GitRepositorySourceType,
            )
          }
        >
          <option value="github">
            {t("library.source_group.type_github")}
          </option>
          <option value="gitlab">
            {t("library.source_group.type_gitlab")}
          </option>
          <option value="git">{t("library.source_group.type_generic")}</option>
        </select>
      </label>
      <label className="import-source-field">
        <span>{t("library.source_group.repo_label")}</span>
        <input
          type="url"
          value={sourceUrl}
          disabled={isBusy}
          onChange={(event) => onSourceUrlChange(event.currentTarget.value)}
          placeholder="https://"
        />
      </label>
      <label className="import-source-field">
        <span>{t("library.source_group.policy_mode")}</span>
        <select
          value={policyMode}
          disabled={isBusy}
          onChange={(event) =>
            onSourceGroupPolicyChange(
              event.currentTarget.value,
              policyMode === event.currentTarget.value ? policyValue : "",
            )
          }
        >
          <option value="auto_release_tag_head">
            {t("library.source_group.policy_auto")}
          </option>
          <option value="prerelease_channel">
            {t("library.source_group.policy_prerelease")}
          </option>
          <option value="fixed_tag">
            {t("library.source_group.policy_fixed_tag")}
          </option>
          <option value="fixed_commit">
            {t("library.source_group.policy_fixed_commit")}
          </option>
          <option value="branch">
            {t("library.source_group.policy_branch")}
          </option>
          <option value="head">{t("library.source_group.policy_head")}</option>
        </select>
      </label>
      {PARAMETERISED_MODES.includes(policyMode) ? (
        <label className="import-source-field">
          <span>{t("library.source_group.policy_value")}</span>
          <input
            type="text"
            value={policyValue}
            disabled={isBusy}
            onChange={(event) =>
              onSourceGroupPolicyChange(policyMode, event.currentTarget.value)
            }
          />
        </label>
      ) : null}
      {error ? (
        <div className="activation-error" role="alert">
          <strong>{t("library.import.source_unavailable")}</strong>
          <span>{error}</span>
        </div>
      ) : null}
      <div className="activation-sheet-actions">
        <button type="button" disabled={isBusy} onClick={onClose}>
          {t("library.import.cancel")}
        </button>
        <button
          type="button"
          className="activation-confirm-button"
          disabled={
            isBusy ||
            (PARAMETERISED_MODES.includes(policyMode) && !policyValue.trim())
          }
          onClick={onFetch}
        >
          {isBusy
            ? t("library.source_group.fetching")
            : t("library.source_group.fetch_text")}
        </button>
      </div>
    </>
  );
}

function PromotionManifest({ draft }: { draft: SourcePromotionDraft }) {
  const { t } = useLocale();
  return (
    <section
      className="source-group-members"
      aria-label={t("library.source_group.members")}
    >
      <h3>{t("library.source_group.members")}</h3>
      <ul className="git-import-candidates">
        {draft.members.map((member) => (
          <li key={member.skillPath || member.directoryName}>
            <div className="source-group-member-copy">
              <strong>{member.displayName}</strong>
              <span className="candidate-path">
                {member.skillPath || t("library.import.repo_root")}
              </span>
              <span className="source-group-member-action">
                {t(`library.source_group.promotion_member_${member.state}`)}
              </span>
            </div>
          </li>
        ))}
        {draft.removedMembers.map((member) => (
          <li key={member.skillId}>
            <div className="source-group-member-copy">
              <strong>{member.directoryName}</strong>
              <span className="candidate-path">{member.skillPath}</span>
              <span className="source-group-member-action">
                {t("library.source_group.promotion_member_removed")}
              </span>
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}

function ExternalClaims({ claims }: { claims: ExternalOwnershipClaim[] }) {
  const { t } = useLocale();
  if (claims.length === 0) return null;
  return (
    <section
      className="source-group-claims"
      aria-label={t("library.source_group.external_claims")}
    >
      <h3>{t("library.source_group.external_claims")}</h3>
      <dl>
        {claims.map((claim) => (
          <div key={`${claim.lockPath}:${claim.entryName}`}>
            <dt>
              <code>{claim.entryName}</code>
            </dt>
            <dd>
              <code>{claim.lockPath}</code>
              {t("library.source_group.claim_ref", { ref: claim.requestedRef })}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

export function memberActionLabel(member: SourceGroupMember) {
  return member.action;
}
