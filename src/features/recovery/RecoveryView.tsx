import { OperationNotice } from "../../ui/OperationNotice";
import { useCallback, useEffect, useState } from "react";

import type {
  CatalogClient,
  CommandFailure,
  FixtureRecoveryPreview,
  SafetySnapshot,
} from "../../app/catalog-client";
import { LanguageControl } from "../locale/LanguageControl";
import { useLocale, type LocaleContextValue } from "../locale/LocaleProvider";
import { SafetySnapshots } from "./SafetySnapshots";

export interface RecoveryViewProps {
  client: CatalogClient;
}

type BusyAction = "recover" | "continue" | "commit" | "delete" | null;

/**
 * The Fixture Recovery route (spec §4.4, §5.2): the read-only preview with
 * the exact evidence, the confirm → apply → commit flow and the Safety
 * Snapshot list with explicit deletion. Regular product writes stay closed
 * for the whole flow; the only writes here are the recovery module's own.
 */
export function RecoveryView({ client }: RecoveryViewProps) {
  const { t } = useLocale();
  const [preview, setPreview] = useState<FixtureRecoveryPreview | null>(null);
  const [snapshots, setSnapshots] = useState<SafetySnapshot[]>([]);
  const [loadFailed, setLoadFailed] = useState(false);
  const [error, setError] = useState<CommandFailure | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<BusyAction>(null);

  const refresh = useCallback(() => {
    client
      .getFixtureRecoveryPreview()
      .then(setPreview)
      .catch(() => {
        setLoadFailed(true);
      });
    client
      .listSafetySnapshots()
      .then(setSnapshots)
      .catch(() => {
        setSnapshots([]);
      });
  }, [client]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const run = useCallback(
    async (action: BusyAction, operation: () => Promise<unknown>) => {
      setBusy(action);
      setError(null);
      setNotice(null);
      try {
        await operation();
        refresh();
      } catch (failure) {
        setError(failure as CommandFailure);
      } finally {
        setBusy(null);
      }
    },
    [refresh],
  );

  if (loadFailed) {
    return (
      <section className="recovery-route" aria-labelledby="recovery-title">
        <LanguageControl />
        <h1 id="recovery-title">{t("recovery.title")}</h1>
        <p>{t("recovery.load_failed")}</p>
        <button type="button" onClick={refresh}>
          {t("recovery.retry")}
        </button>
      </section>
    );
  }
  if (!preview) {
    return (
      <section className="recovery-route" aria-labelledby="recovery-title">
        <LanguageControl />
        <h1 id="recovery-title">{t("recovery.title")}</h1>
        <p>{t("recovery.resolving")}</p>
      </section>
    );
  }

  const operation = preview.activeOperation;
  const awaitingCommit =
    operation?.cursor === "verified" || operation?.cursor === "awaiting_commit";
  const canCommit = awaitingCommit;
  const canContinue = operation !== null && !awaitingCommit;
  const canStart = preview.canPreview && operation === null;

  return (
    <section className="recovery-route" aria-labelledby="recovery-title">
      <LanguageControl />
      <h1 id="recovery-title">{t("recovery.title")}</h1>
      <p className="recovery-summary">
        {preview.mode.kind === "bound_restore"
          ? t("recovery.mode.bound")
          : t("recovery.mode.legacy")}
      </p>
      <p className="recovery-path">{preview.path}</p>

      <OperationNotice busy={busy !== null} />
      <ClassificationEvidence preview={preview} />

      {error ? <ErrorNotice error={error} /> : null}
      {notice ? <Notice message={notice} /> : null}

      <div className="recovery-actions" aria-live="polite">
        {canStart ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run("recover", async () => {
                const plan = await client.planFixtureRecovery();
                const result = await client.applyFixtureRecovery(
                  plan.planToken,
                );
                if (result.rolledBack) {
                  throw {
                    error: { code: "recovery_step_failed" },
                    diagnostic: {
                      code: "rolled_back",
                      message: "",
                    },
                  } satisfies CommandFailure;
                }
                setNotice(t("recovery.step.prepared"));
              })
            }
          >
            {busy === "recover"
              ? t("recovery.action.recovering")
              : t("recovery.action.recover")}
          </button>
        ) : null}
        {canContinue ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run("continue", () =>
                client.applyFixtureRecovery(operation.operationId),
              )
            }
          >
            {busy === "continue"
              ? t("recovery.action.continuing")
              : t("recovery.action.continue")}
          </button>
        ) : null}
        {canCommit ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run("commit", () =>
                client.confirmFixtureRecoveryResult(operation.operationId),
              )
            }
          >
            {busy === "commit"
              ? t("recovery.action.committing")
              : t("recovery.action.commit")}
          </button>
        ) : null}
      </div>

      <SafetySnapshots
        client={client}
        snapshots={snapshots}
        busy={busy !== null}
        run={(operation) => run("delete", operation)}
      />
    </section>
  );
}

function ClassificationEvidence({
  preview,
}: {
  preview: FixtureRecoveryPreview;
}) {
  const { t } = useLocale();
  const classification = preview.classification;
  const heading =
    classification.kind === "pure"
      ? t("recovery.class.pure")
      : classification.kind === "mixed"
        ? t("recovery.class.mixed")
        : classification.kind === "unknown"
          ? t("recovery.class.unknown")
          : t("recovery.class.clean");
  const summary =
    classification.kind === "pure"
      ? t("recovery.class.pure_body")
      : classification.kind === "mixed"
        ? t("recovery.class.mixed_body")
        : classification.kind === "unknown"
          ? t("recovery.class.unknown_body")
          : t("recovery.class.clean_body");
  return (
    <div className="recovery-evidence">
      <h2>{heading}</h2>
      <p>{summary}</p>
      {"reasons" in classification && classification.reasons.length > 0 ? (
        <ul className="recovery-reasons">
          {classification.reasons.map((reason) => (
            <li key={reason}>
              <code>{reason}</code>
            </li>
          ))}
        </ul>
      ) : null}
      <h3>{t("recovery.catalog_evidence")}</h3>
      <dl className="recovery-facts">
        <dt>{t("recovery.schema_version")}</dt>
        <dd>
          {preview.catalogEvidence.schemaVersion ?? t("recovery.fact.unknown")}
        </dd>
        <dt>{t("recovery.skill_rows")}</dt>
        <dd>{preview.catalogEvidence.skillRowCount}</dd>
        <dt>{t("recovery.agent_rows")}</dt>
        <dd>{preview.catalogEvidence.agentRowCount}</dd>
        <dt>{t("recovery.activation_rows")}</dt>
        <dd>{preview.catalogEvidence.activationRowCount}</dd>
        <dt>{t("recovery.file_rows")}</dt>
        <dd>{preview.catalogEvidence.fileSourceRowCount}</dd>
        <dt>{t("recovery.remote_rows")}</dt>
        <dd>{preview.catalogEvidence.remoteSourceRowCount}</dd>
        <dt>{t("recovery.tables")}</dt>
        <dd>
          {preview.catalogEvidence.tables.length > 0
            ? preview.catalogEvidence.tables.join(", ")
            : t("recovery.fact.none")}
        </dd>
      </dl>
      <h3>{t("recovery.tree_evidence")}</h3>
      <dl className="recovery-facts">
        <dt>{t("recovery.fixture_present")}</dt>
        <dd>
          {preview.treeEvidence.fixtureEntitiesPresent
            ? t("recovery.fact.yes")
            : t("recovery.fact.no")}
        </dd>
        <dt>{t("recovery.hash_skill")}</dt>
        <dd>{hashFact(preview.treeEvidence.skillAuthoringHashMatches, t)}</dd>
        <dt>{t("recovery.hash_media")}</dt>
        <dd>{hashFact(preview.treeEvidence.mediaXrayHashMatches, t)}</dd>
        <dt>{t("recovery.hash_root")}</dt>
        <dd>{hashFact(preview.treeEvidence.rootHashMatches, t)}</dd>
        <dt>{t("recovery.legacy_present")}</dt>
        <dd>
          {preview.treeEvidence.legacyAuditEntityPresent
            ? t("recovery.fact.yes")
            : t("recovery.fact.no")}
        </dd>
      </dl>
    </div>
  );
}

function hashFact(matches: boolean | null, t: LocaleContextValue["t"]): string {
  if (matches === null) {
    return t("recovery.hash.unreadable");
  }
  return matches ? t("recovery.hash.matches") : t("recovery.hash.differs");
}

function ErrorNotice({ error }: { error: CommandFailure }) {
  const { t } = useLocale();
  const message =
    error.error.code === "recovery_step_failed"
      ? t("recovery.step.rolled_back")
      : (error.diagnostic?.message ?? t("recovery.command_failed"));
  return (
    <div className="recovery-notice recovery-notice--error" role="alert">
      <p>{message}</p>
      {error.diagnostic?.code ? (
        <p className="recovery-diagnostic">
          <code>{error.error.code}</code>
          <code>{error.diagnostic.code}</code>
        </p>
      ) : null}
    </div>
  );
}

function Notice({ message }: { message: string }) {
  return (
    <div className="recovery-notice" role="status">
      <p>{message}</p>
    </div>
  );
}
