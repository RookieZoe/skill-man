import type { GitSourceCapabilityReport } from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

export interface GitSourceCapabilityFailure {
  diagnostic: string | null;
}

export function GitSourceCapabilityNotice({
  report,
  failure,
  onPromote,
}: {
  report: GitSourceCapabilityReport | null;
  failure: GitSourceCapabilityFailure | null;
  onPromote?: (remoteId: string, trigger: HTMLButtonElement) => void;
}) {
  const { t } = useLocale();
  if (failure) {
    return (
      <section
        className="git-source-capability git-source-capability--unavailable"
        aria-label={t("library.source_capability.label")}
        role="alert"
      >
        <h2>{t("library.source_capability.unavailable")}</h2>
        <p>{t("library.source_capability.unavailable_body")}</p>
        {failure.diagnostic ? (
          <details>
            <summary>{t("library.source_capability.diagnostic")}</summary>
            <pre>{failure.diagnostic}</pre>
          </details>
        ) : null}
      </section>
    );
  }
  if (!report || report.sources.length === 0) return null;

  return (
    <section
      className="git-source-capability"
      aria-label={t("library.source_capability.label")}
    >
      <h2>{t("library.source_capability.heading")}</h2>
      <ul>
        {report.sources.map((source) => {
          return (
            <li
              key={source.remoteId}
              className={`git-source-capability--${source.kind}`}
            >
              <h3>{t(`library.source_capability.${source.kind}`)}</h3>
              <p className="git-source-capability-url">{source.canonicalUrl}</p>
              <p>{t(`library.source_capability.${source.kind}_detail`)}</p>
              {source.kind === "legacy_per_skill_git_state" && onPromote ? (
                <button
                  type="button"
                  className="repair-button"
                  onClick={(event) =>
                    onPromote(source.remoteId, event.currentTarget)
                  }
                >
                  {t("library.source_capability.promote")}
                </button>
              ) : null}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
