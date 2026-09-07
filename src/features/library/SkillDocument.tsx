import { useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
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
          <Markdown
            remarkPlugins={[remarkGfm]}
            skipHtml
            components={{
              // Source content must not load remote tracking images or local files.
              img: ({ alt }) => <span>{alt}</span>,
              a: ({ href, children }) =>
                href && /^https?:\/\//i.test(href) ? (
                  <a href={href} target="_blank" rel="noopener noreferrer">
                    {children}
                  </a>
                ) : (
                  <span>{children}</span>
                ),
            }}
          >
            {body}
          </Markdown>
        </div>
      )}
    </section>
  );
}
