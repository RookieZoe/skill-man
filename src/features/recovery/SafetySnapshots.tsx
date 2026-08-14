import { useState } from "react";

import type { CatalogClient, SafetySnapshot } from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

/**
 * The shared Safety Snapshot list with explicit, in-place deletion
 * confirmation (spec §5.2, ADR-0012 §6): never auto-deleted, deletion is
 * always an explicit user action after a two-step confirm.
 */
export function SafetySnapshots({
  client,
  snapshots,
  busy,
  run,
}: {
  client: CatalogClient;
  snapshots: SafetySnapshot[];
  busy: boolean;
  run: (operation: () => Promise<void>) => Promise<void>;
}) {
  const { t } = useLocale();
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  return (
    <div className="recovery-snapshots">
      <h2>{t("recovery.snapshots")}</h2>
      <p>{t("recovery.snapshots_body")}</p>
      {snapshots.length === 0 ? (
        <p>{t("recovery.no_snapshots")}</p>
      ) : (
        <ul>
          {snapshots.map((snapshot) => (
            <li key={snapshot.snapshotId}>
              <span className="recovery-snapshot-name">
                {snapshot.snapshotId}
              </span>
              <span className="recovery-snapshot-stats">
                {t("recovery.files_count", {
                  count: snapshot.fileCount,
                  bytes: snapshot.totalBytes,
                })}
              </span>
              {confirmDelete === snapshot.snapshotId ? (
                <span className="recovery-delete-confirm">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        const preview = await client.planDeleteSafetySnapshot(
                          snapshot.snapshotId,
                        );
                        await client.applyDeleteSafetySnapshot(
                          preview.snapshotId,
                        );
                        setConfirmDelete(null);
                      })
                    }
                  >
                    {busy
                      ? t("recovery.deleting")
                      : t("recovery.delete_permanent")}
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => setConfirmDelete(null)}
                  >
                    {t("recovery.cancel")}
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => setConfirmDelete(snapshot.snapshotId)}
                >
                  {t("recovery.delete")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
