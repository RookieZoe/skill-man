import type { GitSourceCapabilityReport } from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

export function GitSourceCapabilityNotice({
  report,
}: {
  report: GitSourceCapabilityReport | null;
}) {
  const { t } = useLocale();
  if (!report || report.sources.length === 0) return null;

  return (
    <section
      className="git-source-capability"
      aria-label={t("library.source_capability.label")}
    >
      <h2>{t("library.source_capability.heading")}</h2>
      <ul>
        {report.sources.map((source) => {
          const closed = source.kind !== "git_repository_source";
          return (
            <li
              key={source.remoteId}
              className={`git-source-capability--${source.kind}`}
            >
              <h3>{t(`library.source_capability.${source.kind}`)}</h3>
              <p className="git-source-capability-url">{source.canonicalUrl}</p>
              <p>
                {closed
                  ? t("library.source_capability.source_actions_closed")
                  : t("library.source_capability.source_actions_available")}
              </p>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
