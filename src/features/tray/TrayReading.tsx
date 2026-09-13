import { useEffect, useState } from "react";
import type {
  CatalogClient,
  SkillDetail,
  SkillSummary,
} from "../../app/catalog-client";
import { SkillDocument } from "../library/SkillDocument";
import { useLocale } from "../locale/LocaleProvider";
import { commandErrorMessage } from "../locale/messages";

/** Each selected Skill and Catalog revision owns its request lifetime. */
export function TrayReading({
  client,
  skillId,
  summary,
  onBack,
  copyMarkdown,
}: {
  client: CatalogClient;
  skillId: string;
  summary: SkillSummary | undefined;
  onBack: () => void;
  copyMarkdown: (markdown: string) => Promise<void>;
}) {
  const { t } = useLocale();
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [failure, setFailure] = useState<unknown>(null);
  const [copied, setCopied] = useState<"success" | "failed" | null>(null);
  useEffect(() => {
    let current = true;
    if (summary)
      void client.inspectSkill(skillId).then(
        (next) => {
          if (current) {
            if (next.documentAvailable) setDetail(next);
            else setFailure({ code: "catalog_unavailable" });
          }
        },
        (reason) => {
          if (current) setFailure(reason);
        },
      );
    return () => {
      current = false;
    };
  }, [client, skillId, summary]);
  return (
    <>
      <header>
        <button autoFocus onClick={onBack}>
          {t("tray.panel.back")}
        </button>
        <strong>{summary?.displayName}</strong>
      </header>
      <div className="tray-reading">
        {detail ? (
          <SkillDocument markdown={detail.skillMarkdown} />
        ) : (
          <p role="status">
            {!summary
              ? t("tray.panel.read_failed")
              : failure
                ? commandErrorMessage(failure, t)
                : t("tray.panel.loading")}
          </p>
        )}
      </div>
      {detail && (
        <button
          onClick={() => {
            void copyMarkdown(detail.skillMarkdown).then(
              () => setCopied("success"),
              () => setCopied("failed"),
            );
          }}
        >
          {t("tray.panel.copy")}
        </button>
      )}
      {copied && (
        <p role="status">
          {t(
            copied === "success"
              ? "tray.panel.copied"
              : "tray.panel.copy_failed",
          )}
        </p>
      )}
    </>
  );
}
