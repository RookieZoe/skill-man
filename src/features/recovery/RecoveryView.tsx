import { useCallback, useEffect, useState } from "react";

import type {
  CatalogClient,
  CommandFailure,
  FixtureRecoveryPreview,
  SafetySnapshot,
} from "../../app/catalog-client";

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
  const [preview, setPreview] = useState<FixtureRecoveryPreview | null>(null);
  const [snapshots, setSnapshots] = useState<SafetySnapshot[]>([]);
  const [loadFailed, setLoadFailed] = useState(false);
  const [error, setError] = useState<CommandFailure | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<BusyAction>(null);

  const refresh = useCallback(() => {
    client.getFixtureRecoveryPreview().then(setPreview).catch(() => {
      setLoadFailed(true);
    });
    client.listSafetySnapshots().then(setSnapshots).catch(() => {
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
        <h1 id="recovery-title">Fixture Recovery</h1>
        <p>The recovery state could not be read.</p>
        <button type="button" onClick={refresh}>
          Retry
        </button>
      </section>
    );
  }
  if (!preview) {
    return (
      <section className="recovery-route" aria-labelledby="recovery-title">
        <h1 id="recovery-title">Fixture Recovery</h1>
        <p>Resolving the recovery preview…</p>
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
      <h1 id="recovery-title">Fixture Recovery</h1>
      <p className="recovery-summary">
        {preview.mode.kind === "bound_restore"
          ? "The bound Home contains fixture test data. Recovery restores the same Home identity; it is not a Relocate."
          : "The Legacy Home contains fixture test data. Recovery prepares a clean, unbound Home."}
      </p>
      <p className="recovery-path">{preview.path}</p>

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
                      message:
                        "Recovery could not prepare the clean Home and restored the original one.",
                    },
                  } satisfies CommandFailure;
                }
                setNotice(
                  "The clean Home is prepared and verified. Review the result and commit it to finish.",
                );
              })
            }
          >
            {busy === "recover" ? "Recovering…" : "Recover this Home"}
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
            {busy === "continue" ? "Continuing…" : "Continue recovery"}
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
            {busy === "commit" ? "Committing…" : "Commit recovery result"}
          </button>
        ) : null}
      </div>

      <SnapshotSection
        client={client}
        snapshots={snapshots}
        busy={busy}
        run={run}
      />
    </section>
  );
}

function ClassificationEvidence({
  preview,
}: {
  preview: FixtureRecoveryPreview;
}) {
  const classification = preview.classification;
  const heading =
    classification.kind === "pure"
      ? "Exact fixture footprint detected"
      : classification.kind === "mixed"
        ? "Modified fixture footprint detected"
        : classification.kind === "unknown"
          ? "Fixture footprint could not be read"
          : "No fixture footprint";
  const summary =
    classification.kind === "pure"
      ? "Every checked fact matches the immutable fixture fingerprint. Only this exact shape can be recovered."
      : classification.kind === "mixed"
        ? "The whole Home stays locked; a recoverable subset is never chosen."
        : classification.kind === "unknown"
          ? "The Home stays locked until every fact can be read."
          : "The Home is clean.";
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
      <h3>Catalog evidence</h3>
      <dl className="recovery-facts">
        <dt>Schema version</dt>
        <dd>{preview.catalogEvidence.schemaVersion ?? "unknown"}</dd>
        <dt>Skill rows</dt>
        <dd>{preview.catalogEvidence.skillRowCount}</dd>
        <dt>Agent rows</dt>
        <dd>{preview.catalogEvidence.agentRowCount}</dd>
        <dt>Activation rows</dt>
        <dd>{preview.catalogEvidence.activationRowCount}</dd>
        <dt>File source rows</dt>
        <dd>{preview.catalogEvidence.fileSourceRowCount}</dd>
        <dt>Remote source rows</dt>
        <dd>{preview.catalogEvidence.remoteSourceRowCount}</dd>
        <dt>Tables</dt>
        <dd>
          {preview.catalogEvidence.tables.length > 0
            ? preview.catalogEvidence.tables.join(", ")
            : "none"}
        </dd>
      </dl>
      <h3>Tree evidence</h3>
      <dl className="recovery-facts">
        <dt>fixture-entities present</dt>
        <dd>{preview.treeEvidence.fixtureEntitiesPresent ? "yes" : "no"}</dd>
        <dt>skill-authoring hash</dt>
        <dd>{hashFact(preview.treeEvidence.skillAuthoringHashMatches)}</dd>
        <dt>media-xray hash</dt>
        <dd>{hashFact(preview.treeEvidence.mediaXrayHashMatches)}</dd>
        <dt>fixture-entities root hash</dt>
        <dd>{hashFact(preview.treeEvidence.rootHashMatches)}</dd>
        <dt>legacy-audit entity present</dt>
        <dd>{preview.treeEvidence.legacyAuditEntityPresent ? "yes" : "no"}</dd>
      </dl>
    </div>
  );
}

function hashFact(matches: boolean | null): string {
  if (matches === null) {
    return "unreadable or missing";
  }
  return matches ? "matches the fixture fingerprint" : "differs";
}

function ErrorNotice({ error }: { error: CommandFailure }) {
  const message =
    error.diagnostic?.message ?? "The recovery command failed.";
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

function SnapshotSection({
  client,
  snapshots,
  busy,
  run,
}: {
  client: CatalogClient;
  snapshots: SafetySnapshot[];
  busy: BusyAction;
  run: (action: BusyAction, operation: () => Promise<unknown>) => Promise<void>;
}) {
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  return (
    <div className="recovery-snapshots">
      <h2>Safety Snapshots</h2>
      <p>
        Snapshots are never deleted automatically. Deleting one is permanent
        and cannot be undone.
      </p>
      {snapshots.length === 0 ? (
        <p>No Safety Snapshots exist.</p>
      ) : (
        <ul>
          {snapshots.map((snapshot) => (
            <li key={snapshot.snapshotId}>
              <span className="recovery-snapshot-name">
                {snapshot.snapshotId}
              </span>
              <span className="recovery-snapshot-stats">
                {snapshot.fileCount} files · {snapshot.totalBytes} bytes
              </span>
              {confirmDelete === snapshot.snapshotId ? (
                <span className="recovery-delete-confirm">
                  <button
                    type="button"
                    disabled={busy !== null}
                    onClick={() =>
                      void run("delete", async () => {
                        const preview =
                          await client.planDeleteSafetySnapshot(
                            snapshot.snapshotId,
                          );
                        await client.applyDeleteSafetySnapshot(
                          preview.snapshotId,
                        );
                        setConfirmDelete(null);
                      })
                    }
                  >
                    {busy === "delete" ? "Deleting…" : "Delete permanently"}
                  </button>
                  <button
                    type="button"
                    disabled={busy !== null}
                    onClick={() => setConfirmDelete(null)}
                  >
                    Cancel
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  disabled={busy !== null}
                  onClick={() => setConfirmDelete(snapshot.snapshotId)}
                >
                  Delete…
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
