import type {
  ExternalOwnershipClaim,
  GitRepositorySourceType,
  SourceGroupPreviewOutcome,
  SourceTransitionResult,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

export function SourceGroupPreviewFlow({
  sourceType,
  sourceUrl,
  trackingRef,
  outcome,
  result,
  error,
  activity,
  onSourceTypeChange,
  onSourceUrlChange,
  onTrackingRefChange,
  onFetch,
  onConfirm,
  onUndo,
  onClose,
}: {
  sourceType: GitRepositorySourceType;
  sourceUrl: string;
  trackingRef: string;
  outcome: SourceGroupPreviewOutcome | null;
  result: SourceTransitionResult | null;
  error: string | null;
  activity: "idle" | "fetching" | "confirming" | "undoing";
  onSourceTypeChange: (sourceType: GitRepositorySourceType) => void;
  onSourceUrlChange: (sourceUrl: string) => void;
  onTrackingRefChange: (trackingRef: string) => void;
  onFetch: () => void;
  onConfirm: () => void;
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
            <dt>{t("library.source_group.tracking_ref")}</dt>
            <dd>{preview.trackingRef}</dd>
          </div>
          <div>
            <dt>{t("library.source_group.resolved_commit")}</dt>
            <dd>
              <code>{preview.resolvedCommit}</code>
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
            value={trackingRef}
            disabled={isBusy}
            onChange={(event) => onTrackingRefChange(event.currentTarget.value)}
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
            disabled={!trackingRef.trim() || isBusy}
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
          <option value="git">{t("library.source_group.type_git")}</option>
        </select>
      </label>
      <label className="import-source-field">
        <span>{t("library.source_group.repo_label")}</span>
        <input
          type="url"
          value={sourceUrl}
          disabled={isBusy}
          placeholder={t("library.source_group.repo_placeholder")}
          onChange={(event) => onSourceUrlChange(event.currentTarget.value)}
        />
      </label>
      <label className="import-source-field">
        <span>{t("library.source_group.ref_label")}</span>
        <input
          type="text"
          value={trackingRef}
          disabled={isBusy}
          placeholder={t("library.source_group.ref_placeholder")}
          onChange={(event) => onTrackingRefChange(event.currentTarget.value)}
        />
      </label>
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
          disabled={!sourceUrl.trim() || isBusy}
          onClick={onFetch}
        >
          {isBusy
            ? t("library.source_group.fetching")
            : t("library.source_group.fetch")}
        </button>
      </div>
    </>
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
      <p>{t("library.source_group.external_claims_body")}</p>
      <ul>
        {claims.map((claim) => (
          <li key={`${claim.lockPath}:${claim.entryName}`}>
            <code>{claim.lockPath}</code> · {claim.entryName} ·{" "}
            <code>{claim.requestedRef}</code>
          </li>
        ))}
      </ul>
    </section>
  );
}
