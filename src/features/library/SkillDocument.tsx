import { useState } from "react";
import { MarkdownContent } from "../../ui/MarkdownContent";
import { useLocale } from "../locale/LocaleProvider";

/** Only the reading view omits metadata; RAW preserves the original bytes. */
export function SkillDocument({ markdown }: { markdown: string }) {
  const [raw, setRaw] = useState(false);
  const { t } = useLocale();
  const body = markdown.replace(
    /^\uFEFF?---\r?\n[\s\S]*?\r?\n(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/,
    "",
  );
  return (
    <section className="document-preview" aria-labelledby="preview-title">
      <div className="document-toolbar">
        <div>
          <span className="document-dot" />
          <h3 id="preview-title">SKILL.md</h3>
        </div>
        <button
          type="button"
          role="switch"
          aria-label={t("library.detail.raw")}
          aria-checked={raw}
          className="raw-toggle"
          onClick={() => setRaw(!raw)}
        >
          {t("library.detail.raw_label")}
        </button>
      </div>
      {raw ? (
        <pre>{markdown}</pre>
      ) : (
        <div className="markdown-body">
          <MarkdownContent markdown={body} />
        </div>
      )}
    </section>
  );
}
