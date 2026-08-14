import type {
  AdoptEvidenceCandidate,
  AdoptEvidenceReport,
  AdoptPlan,
  AdoptResult,
  AdoptSelection,
  AdoptUndoResult,
  AdoptVerdictReason,
  ChainFault,
  LockFileFault,
  ModifiedBranch,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import type { MessageKey, PluralKey } from "../locale/messages";

/**
 * The Adopt Evidence Ledger (spec §8.1, issue #39 resolution): three
 * columns — candidate context, the full source chain with every hop, lock
 * hit and remote/ref/Verification Anchor/tree evidence (default fully
 * expanded), and the verdict with its planned operation and ownership
 * result. Viewing is never selecting: only selectable candidates carry an
 * Include control, and Modified candidates show all three branches at once.
 * Paths, URLs, refs, hashes and lock fields are Source Content and render
 * verbatim in every locale (spec §6.3).
 */

function reasonLabelKey(reason: AdoptVerdictReason): MessageKey {
  switch (reason.kind) {
    case "no_lock":
      return "library.adopt.reason.no_lock";
    case "duplicate_lock_owner":
      return "library.adopt.reason.duplicate_lock_owner";
    case "lock_file_fault":
      return "library.adopt.reason.lock_file_fault";
    case "lock_entry_fault":
      return "library.adopt.reason.lock_entry_fault";
    case "entity_not_at_installer_root":
      return "library.adopt.reason.entity_not_at_installer_root";
    case "remote_conflict":
      return "library.adopt.reason.remote_conflict";
    case "identity_conflict":
      return "library.adopt.reason.identity_conflict";
    case "library_conflict":
      return "library.adopt.reason.library_conflict";
    case "remote_unavailable":
      return "library.adopt.reason.remote_unavailable";
    case "chain_fault":
      return "library.adopt.reason.chain_fault";
    case "unreadable_entity":
      return "library.adopt.reason.unreadable_entity";
    case "fixture_entity":
      return "library.adopt.reason.fixture_entity";
  }
}

function chainFaultKey(fault: ChainFault): MessageKey {
  switch (fault.kind) {
    case "dangling":
      return "library.adopt.chain_fault.dangling";
    case "cycle":
      return "library.adopt.chain_fault.cycle";
    case "hop_limit":
      return "library.adopt.chain_fault.hop_limit";
    case "non_utf8":
      return "library.adopt.chain_fault.non_utf8";
    case "read_failed":
      return "library.adopt.chain_fault.read_failed";
    case "not_directory":
      return "library.adopt.chain_fault.not_directory";
    case "identity_replaced":
      return "library.adopt.chain_fault.identity_replaced";
  }
}

function lockFaultKey(fault: LockFileFault): MessageKey {
  switch (fault.kind) {
    case "not_utf8":
      return "library.adopt.lock_fault.not_utf8";
    case "invalid_json":
      return "library.adopt.lock_fault.invalid_json";
    case "unsupported_version":
      return "library.adopt.lock_fault.unsupported_version";
    case "duplicate_key":
      return "library.adopt.lock_fault.duplicate_key";
  }
}

function verdictKey(verdict: AdoptEvidenceCandidate["verdict"]): MessageKey {
  return `library.adopt.verdict.${verdict}` as MessageKey;
}

export function EvidenceLedger({
  report,
  selections,
  plan,
  result,
  undo,
  error,
  errorHeading,
  activity,
  onToggle,
  onSetBranch,
  onRescan,
  onPlan,
  onApply,
  onUndo,
  onClose,
}: {
  report: AdoptEvidenceReport | null;
  selections: Record<string, AdoptSelection>;
  plan: AdoptPlan | null;
  result: AdoptResult | null;
  undo: AdoptUndoResult | null;
  error: string | null;
  errorHeading: MessageKey;
  activity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onToggle: (canonicalEntity: string, checked: boolean) => void;
  onSetBranch: (canonicalEntity: string, branch: ModifiedBranch) => void;
  onRescan: () => void;
  onPlan: () => void;
  onApply: () => void;
  onUndo: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const isBusy = activity !== "idle";
  const candidates = report?.candidates ?? [];
  const step = result ? "result" : plan ? "preview" : report ? "scan" : "scan";

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isBusy) onClose();
      }}
    >
      <section
        className="activation-sheet import-sheet adopt-ledger-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.adopt.dialog_label")}
      >
        <ol className="import-progress" aria-label={t("library.adopt.progress_label")}>
          {(["scan", "preview", "result"] as const).map((stepName) => (
            <li key={stepName} aria-current={step === stepName ? "step" : undefined}>
              {t(`library.adopt.step.${stepName}` as MessageKey)}
            </li>
          ))}
        </ol>
        {result ? (
          <LedgerResult
            result={result}
            undo={undo}
            error={error}
            errorHeading={errorHeading}
            activity={activity}
            onUndo={onUndo}
            onClose={onClose}
          />
        ) : plan ? (
          <LedgerPlan
            plan={plan}
            error={error}
            errorHeading={errorHeading}
            activity={activity}
            onApply={onApply}
            onClose={onClose}
          />
        ) : (
          <LedgerScan
            report={report}
            selections={selections}
            candidates={candidates}
            error={error}
            errorHeading={errorHeading}
            activity={activity}
            isBusy={isBusy}
            onToggle={onToggle}
            onSetBranch={onSetBranch}
            onRescan={onRescan}
            onPlan={onPlan}
            onClose={onClose}
          />
        )}
      </section>
    </div>
  );
}

function LedgerScan({
  report,
  selections,
  candidates,
  error,
  errorHeading,
  activity,
  isBusy,
  onToggle,
  onSetBranch,
  onRescan,
  onPlan,
  onClose,
}: {
  report: AdoptEvidenceReport | null;
  selections: Record<string, AdoptSelection>;
  candidates: AdoptEvidenceCandidate[];
  error: string | null;
  errorHeading: MessageKey;
  activity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  isBusy: boolean;
  onToggle: (canonicalEntity: string, checked: boolean) => void;
  onSetBranch: (canonicalEntity: string, branch: ModifiedBranch) => void;
  onRescan: () => void;
  onPlan: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  return (
    <>
      <div className="activation-sheet-heading adopt-ledger-heading">
        <span className="eyebrow">{t("library.adopt.scan_eyebrow")}</span>
        <h2>{t("library.adopt.scan_title")}</h2>
        <p>{t("library.adopt.scan_body")}</p>
      </div>
      <div className="adopt-ledger-body">
        {candidates.length === 0 && !isBusy ? (
          <p role="status">{t("library.adopt.none")}</p>
        ) : (
          candidates.map((candidate) => (
            <LedgerCandidate
              key={candidate.canonicalEntity}
              candidate={candidate}
              selected={selections[candidate.canonicalEntity] !== undefined}
              branch={
                selections[candidate.canonicalEntity]?.modifiedBranch ?? null
              }
              isBusy={isBusy}
              onToggle={onToggle}
              onSetBranch={onSetBranch}
            />
          ))
        )}
        {report?.truncated ? (
          <p className="candidate-conflict" role="status">
            {t("library.adopt.truncated")}
          </p>
        ) : null}
        {error ? (
          <div className="activation-error" role="alert">
            <strong>{t(errorHeading)}</strong>
            <span>{error}</span>
          </div>
        ) : null}
      </div>
      <div className="activation-sheet-actions adopt-ledger-actions">
        <button type="button" disabled={isBusy} onClick={onRescan}>
          {activity === "scanning"
            ? t("library.adopt.rescanning")
            : t("library.adopt.rescan")}
        </button>
        <button type="button" disabled={isBusy} onClick={onClose}>
          {t("library.adopt.cancel")}
        </button>
        <button
          type="button"
          className="activation-confirm-button"
          disabled={Object.keys(selections).length === 0 || isBusy}
          onClick={onPlan}
        >
          {activity === "planning"
            ? t("library.adopt.planning")
            : t("library.adopt.preview_button")}
        </button>
      </div>
    </>
  );
}

function LedgerCandidate({
  candidate,
  selected,
  branch,
  isBusy,
  onToggle,
  onSetBranch,
}: {
  candidate: AdoptEvidenceCandidate;
  selected: boolean;
  branch: ModifiedBranch | null;
  isBusy: boolean;
  onToggle: (canonicalEntity: string, checked: boolean) => void;
  onSetBranch: (canonicalEntity: string, branch: ModifiedBranch) => void;
}) {
  const { t, tPlural } = useLocale();
  const id = `adopt-candidate-${candidate.directoryName}`;
  return (
    <section
      className="adopt-ledger-candidate"
      aria-labelledby={`${id}-name`}
    >
      {/* Column 1: candidate context */}
      <div className="adopt-ledger-col adopt-ledger-context">
        <h3 id={`${id}-name`} className="adopt-candidate-name">
          {candidate.directoryName}
        </h3>
        <p className="candidate-path" title={candidate.canonicalEntity}>
          {candidate.canonicalEntity}
        </p>
        <p>
          {tPlural(
            "library.adopt.appearances" as PluralKey,
            candidate.appearances.length,
          )}
        </p>
        {candidate.requiresRelocation ? (
          <p className="candidate-warning">
            {t("library.adopt.relocation_required")}
          </p>
        ) : null}
        {candidate.selectable ? (
          <label className="adopt-include-control">
            <input
              type="checkbox"
              checked={selected}
              disabled={isBusy}
              onChange={(event) =>
                onToggle(
                  candidate.canonicalEntity,
                  event.currentTarget.checked,
                )
              }
            />
            <span>
              {selected
                ? t("library.adopt.included")
                : t("library.adopt.include")}
            </span>
          </label>
        ) : (
          <p className="candidate-blocked">{t("library.adopt.no_control")}</p>
        )}
      </div>
      {/* Column 2: full source chain / verification gates (default expanded) */}
      <div className="adopt-ledger-col adopt-ledger-chain">
        <h4>{t("library.adopt.chain.title")}</h4>
        <ul className="adopt-chain-list">
          {candidate.appearances.map((appearance) => (
            <li key={appearance.entryPath}>
              <p className="adopt-chain-entry">
                <strong>
                  {appearance.agentId
                    ? t("library.adopt.chain.appearance_agent", {
                        agent: appearance.agentId,
                      })
                    : appearance.shared
                      ? t("library.adopt.chain.appearance_shared")
                      : t("library.adopt.chain.appearance")}
                </strong>{" "}
                <span className="adopt-source-content">
                  {appearance.entryPath}
                </span>
                {appearance.originalTarget ? (
                  <span className="adopt-source-content adopt-hop-target">
                    {" "}
                    → {appearance.originalTarget}
                  </span>
                ) : null}
              </p>
              {appearance.chain.hops.map((hop, hopIndex) => (
                <p
                  key={`${hop.path}-${hopIndex}`}
                  className="adopt-hop"
                >
                  <span className="adopt-hop-index">
                    {t("library.adopt.chain.hop", {
                      number: hopIndex + 1,
                    })}
                  </span>{" "}
                  <span className="adopt-source-content">{hop.path}</span>
                  {hop.target !== null ? (
                    <span className="adopt-source-content adopt-hop-target">
                      {" "}
                      → {hop.target}
                    </span>
                  ) : null}
                  <span className="adopt-hop-identity">
                    {" "}
                    dev={hop.device} ino={hop.inode}
                  </span>
                </p>
              ))}
              {appearance.chain.fault ? (
                <p className="candidate-blocked" role="alert">
                  {t(chainFaultKey(appearance.chain.fault))}
                  {"at" in appearance.chain.fault ? (
                    <span className="adopt-source-content">
                      {" "}
                      {appearance.chain.fault.at}
                    </span>
                  ) : null}
                  {"detail" in appearance.chain.fault ? (
                    <span className="adopt-source-content">
                      {" "}
                      {appearance.chain.fault.detail}
                    </span>
                  ) : null}
                </p>
              ) : (
                <p className="adopt-hop">
                  <span className="adopt-hop-index">
                    {t("library.adopt.chain.final_entity")}
                  </span>{" "}
                  <span className="adopt-source-content">
                    {appearance.chain.finalEntity ?? ""}
                  </span>
                </p>
              )}
            </li>
          ))}
        </ul>
        {candidate.lock ? (
          <div className="adopt-evidence-block">
            <h4>{t("library.adopt.lock.title")}</h4>
            <p className="adopt-hop">
              <span className="adopt-hop-index">
                {t("library.adopt.lock.path")}
              </span>{" "}
              <span className="adopt-source-content">
                {candidate.lock.lockPath}
              </span>
            </p>
            {candidate.lock.fileFault ? (
              <p className="candidate-blocked">
                {t(lockFaultKey(candidate.lock.fileFault))}
                {"key" in candidate.lock.fileFault ? (
                  <span className="adopt-source-content">
                    {" "}
                    {candidate.lock.fileFault.key}
                  </span>
                ) : null}
                {"detail" in candidate.lock.fileFault ? (
                  <span className="adopt-source-content">
                    {" "}
                    {candidate.lock.fileFault.detail}
                  </span>
                ) : null}
              </p>
            ) : null}
            {candidate.lock.entryFault ? (
              <p className="candidate-blocked">
                {t("library.adopt.lock.entry_fault")}
                <span className="adopt-source-content">
                  {" "}
                  {candidate.lock.entryFault}
                </span>
              </p>
            ) : null}
            {candidate.lock.entry ? (
              <dl className="adopt-fact-list">
                <dt>{t("library.adopt.lock.entry_name")}</dt>
                <dd className="adopt-source-content">
                  {candidate.lock.entry.name}
                </dd>
                <dt>{t("library.adopt.lock.source_type")}</dt>
                <dd className="adopt-source-content">
                  {candidate.lock.entry.sourceType}
                </dd>
                <dt>{t("library.adopt.lock.source_url")}</dt>
                <dd className="adopt-source-content">
                  {candidate.lock.entry.sourceUrl}
                </dd>
                <dt>{t("library.adopt.lock.requested_ref")}</dt>
                <dd className="adopt-source-content">
                  {candidate.lock.entry.requestedRef ?? "HEAD"}
                </dd>
                <dt>{t("library.adopt.lock.skill_path")}</dt>
                <dd className="adopt-source-content">
                  {candidate.lock.entry.skillPath}
                </dd>
                <dt>{t("library.adopt.lock.provider_hash")}</dt>
                <dd className="adopt-source-content">
                  {candidate.lock.entry.skillFolderHash}
                </dd>
              </dl>
            ) : null}
            <p className="adopt-hop">
              <span className="adopt-hop-index">
                {t("library.adopt.lock.fingerprint")}
              </span>{" "}
              <span className="adopt-source-content">
                {candidate.lock.lockFingerprint}
              </span>
            </p>
          </div>
        ) : null}
        {candidate.remote ? (
          <div className="adopt-evidence-block">
            <h4>{t("library.adopt.remote.title")}</h4>
            <dl className="adopt-fact-list">
              <dt>{t("library.adopt.remote.canonical_url")}</dt>
              <dd className="adopt-source-content">
                {candidate.remote.canonicalUrl}
              </dd>
              <dt>{t("library.adopt.remote.requested_ref")}</dt>
              <dd className="adopt-source-content">
                {candidate.remote.requestedRef}
              </dd>
              <dt>{t("library.adopt.remote.anchor")}</dt>
              <dd className="adopt-source-content">
                {candidate.remote.anchorCommit}
              </dd>
            </dl>
            {!candidate.remote.originalInstallCommitKnown ? (
              <p className="candidate-warning">
                {t("library.adopt.remote.original_install_unknown")}
              </p>
            ) : null}
            <p className="adopt-hop">
              <span className="adopt-hop-index">
                {t("library.adopt.remote.remote_tree")}
              </span>{" "}
              <span className="adopt-source-content">
                {candidate.remote.remoteTreeHash}
              </span>
            </p>
            <p className="adopt-hop">
              <span className="adopt-hop-index">
                {t("library.adopt.remote.local_tree")}
              </span>{" "}
              <span className="adopt-source-content">
                {candidate.remote.localTreeHash}
              </span>
            </p>
            <p
              className={
                candidate.remote.treesMatch
                  ? "candidate-clear"
                  : "candidate-warning"
              }
            >
              {candidate.remote.treesMatch
                ? t("library.adopt.remote.trees_match")
                : t("library.adopt.remote.trees_differ")}
            </p>
          </div>
        ) : null}
        {candidate.localTreeHash ? (
          <p className="adopt-hop">
            <span className="adopt-hop-index">
              {t("library.adopt.chain.local_tree")}
            </span>{" "}
            <span className="adopt-source-content">
              {candidate.localTreeHash}
            </span>
          </p>
        ) : null}
      </div>
      {/* Column 3: verdict / planned operation / ownership result */}
      <div className="adopt-ledger-col adopt-ledger-verdict">
        <h4>{t("library.adopt.verdict.title")}</h4>
        <p className={`adopt-verdict adopt-verdict-${candidate.verdict}`}>
          {t(verdictKey(candidate.verdict))}
        </p>
        {candidate.reason ? (
          <p className="adopt-reason">
            {t(reasonLabelKey(candidate.reason))}
            {"detail" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {candidate.reason.detail}
              </span>
            ) : null}
            {"otherLockPath" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {candidate.reason.otherLockPath}
              </span>
            ) : null}
            {"expected" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {candidate.reason.expected}
              </span>
            ) : null}
            {"names" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {candidate.reason.names.join(", ")}
              </span>
            ) : null}
            {"directoryName" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {candidate.reason.directoryName}
              </span>
            ) : null}
            {"reason" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {candidate.reason.reason}
              </span>
            ) : null}
            {"fault" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {"at" in candidate.reason.fault
                  ? candidate.reason.fault.at
                  : ""}
                {"detail" in candidate.reason.fault
                  ? ` ${candidate.reason.fault.detail}`
                  : ""}
              </span>
            ) : null}
            {"lockPath" in candidate.reason ? (
              <span className="adopt-source-content">
                {" "}
                {candidate.reason.lockPath}
              </span>
            ) : null}
          </p>
        ) : null}
        {candidate.verdict === "modified" && candidate.selectable ? (
          <fieldset className="adopt-branch-fieldset">
            <legend>{t("library.adopt.modified.title")}</legend>
            <label>
              <input
                type="radio"
                name={`branch-${candidate.canonicalEntity}`}
                checked={branch === "keep_current"}
                disabled={isBusy || !selected}
                onChange={() =>
                  onSetBranch(candidate.canonicalEntity, "keep_current")
                }
              />
              {t("library.adopt.modified.keep_current")}
            </label>
            <label>
              <input
                type="radio"
                name={`branch-${candidate.canonicalEntity}`}
                checked={branch === "discard_to_anchor"}
                disabled={isBusy || !selected}
                onChange={() =>
                  onSetBranch(candidate.canonicalEntity, "discard_to_anchor")
                }
              />
              {t("library.adopt.modified.discard_to_anchor")}
            </label>
            <label>
              <input
                type="radio"
                name={`branch-${candidate.canonicalEntity}`}
                checked={branch === "convert_to_local_link"}
                disabled={isBusy || !selected}
                onChange={() =>
                  onSetBranch(candidate.canonicalEntity, "convert_to_local_link")
                }
              />
              {t("library.adopt.modified.convert_to_link")}
            </label>
          </fieldset>
        ) : null}
        {candidate.verdict === "modified" && !candidate.selectable ? (
          <p className="candidate-blocked">
            {t("library.adopt.modified.choose_before_plan")}
          </p>
        ) : null}
      </div>
    </section>
  );
}

function LedgerPlan({
  plan,
  error,
  errorHeading,
  activity,
  onApply,
  onClose,
}: {
  plan: AdoptPlan;
  error: string | null;
  errorHeading: MessageKey;
  activity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onApply: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const isBusy = activity !== "idle";
  return (
    <>
      <div className="activation-sheet-heading">
        <span className="eyebrow">{t("library.adopt.preview_eyebrow")}</span>
        <h2>{t("library.adopt.preview_title")}</h2>
        <p>{t("library.adopt.preview_body")}</p>
      </div>
      <ul className="git-import-candidates git-import-preview-list adopt-plan-list">
        {plan.items.map((item) => (
          <li key={item.canonicalEntity}>
            <div>
              <strong>{item.directoryName}</strong>
              <span className="candidate-path">
                {t(`library.adopt.intent.${item.intent}` as MessageKey)}
              </span>
            </div>
            {item.applyable ? (
              <span className="candidate-clear">{t("library.adopt.ready")}</span>
            ) : (
              <span className="candidate-warning">
                {t("library.adopt.plan.handoff_pending")}
              </span>
            )}
          </li>
        ))}
      </ul>
      {error ? (
        <div className="activation-error" role="alert">
          <strong>{t(errorHeading)}</strong>
          <span>{error}</span>
        </div>
      ) : null}
      <div className="activation-sheet-actions">
        <button type="button" disabled={isBusy} onClick={onClose}>
          {t("library.adopt.cancel")}
        </button>
        <button
          type="button"
          className="activation-confirm-button"
          disabled={!plan.canApply || isBusy}
          onClick={onApply}
        >
          {isBusy ? t("library.adopt.adopting") : t("library.adopt.apply")}
        </button>
      </div>
    </>
  );
}

function LedgerResult({
  result,
  undo,
  error,
  errorHeading,
  activity,
  onUndo,
  onClose,
}: {
  result: AdoptResult;
  undo: AdoptUndoResult | null;
  error: string | null;
  errorHeading: MessageKey;
  activity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onUndo: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const isBusy = activity !== "idle";
  return (
    <>
      <div className="activation-sheet-heading">
        <span className="eyebrow">{t("library.adopt.complete_eyebrow")}</span>
        <h2>
          {t("library.adopt.complete_title", {
            adopted: result.items.filter((item) => item.adopted).length,
            total: result.items.length,
          })}
        </h2>
        <p>{t("library.adopt.complete_body")}</p>
      </div>
      <ul className="git-import-results">
        {result.items.map((item) => (
          <li key={item.directoryName}>
            <span>
              <strong>{item.directoryName}</strong>{" "}
              {item.adopted ? (
                <span className="candidate-clear">
                  {t("library.adopt.adopted")}
                </span>
              ) : (
                <span className="candidate-conflict">
                  {t("library.adopt.failed", {
                    detail:
                      item.error ?? t("library.adopt.failed_unknown"),
                  })}
                </span>
              )}
            </span>
          </li>
        ))}
      </ul>
      {undo ? (
        <div
          className={
            undo.items.every((item) => item.undone)
              ? "update-result-ok"
              : "update-result-fail"
          }
          role="status"
        >
          {undo.items.every((item) => item.undone)
            ? t("library.adopt.undone")
            : undo.items
                .filter((item) => !item.undone)
                .map((item) =>
                  t("library.adopt.undo_item", {
                    name: item.directoryName,
                    detail:
                      item.error ?? t("library.adopt.failed_unknown"),
                  }),
                )
                .join(" · ")}
        </div>
      ) : null}
      {error ? (
        <div className="activation-error" role="alert">
          <strong>{t(errorHeading)}</strong>
          <span>{error}</span>
        </div>
      ) : null}
      <div className="activation-sheet-actions">
        {result.undoAvailable && !undo ? (
          <button
            type="button"
            className="activation-confirm-button"
            disabled={isBusy}
            onClick={onUndo}
          >
            {activity === "undoing"
              ? t("library.adopt.undoing")
              : t("library.adopt.undo")}
          </button>
        ) : null}
        <button type="button" disabled={isBusy} onClick={onClose}>
          {t("library.adopt.close")}
        </button>
      </div>
    </>
  );
}
