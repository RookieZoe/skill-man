import { useState } from "react";
import {
  createCatalogClient,
  type CommunityUpdate,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import { RepositoryLink } from "../../ui/RepositoryLink";
import config from "../../../src-tauri/tauri.conf.json";

export function CommunityUpdateControl({
  check = () => createCatalogClient().checkCommunityUpdate(),
}: {
  check?: () => Promise<CommunityUpdate | null>;
}) {
  const { t } = useLocale();
  const [status, setStatus] = useState<
    "idle" | "checking" | "checked" | "failed"
  >("idle");
  const [update, setUpdate] = useState<CommunityUpdate | null>(null);
  async function checkNow() {
    setStatus("checking");
    setUpdate(null);
    try {
      setUpdate(await check());
      setStatus("checked");
    } catch {
      setStatus("failed");
    }
  }
  return (
    <div className="app-update-check community-update">
      <span>
        <strong>{t("library.preferences.app_updates")}</strong>
        <small>{t("community.current", { version: config.version })}</small>
        <small>{t("community.hint")}</small>
      </span>
      <button
        type="button"
        disabled={status === "checking"}
        onClick={() => void checkNow()}
      >
        {t(
          status === "checking"
            ? "community.checking"
            : "library.preferences.check_now",
        )}
      </button>
      {status === "checked" && (
        <p className="community-update-status" role="status">
          {update ? (
            <>
              {t("community.available", { version: update.version })}{" "}
              <RepositoryLink
                url={update.releaseUrl}
                label={t("community.open")}
              />
            </>
          ) : (
            t("community.latest")
          )}
        </p>
      )}
      {status === "failed" && (
        <p className="community-update-status" role="alert">
          {t("community.failed")}
        </p>
      )}
    </div>
  );
}
