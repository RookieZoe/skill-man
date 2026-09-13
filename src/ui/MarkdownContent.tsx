import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** Render external Markdown without HTML, remote images, or local-file links. */
export function MarkdownContent({ markdown }: { markdown: string }) {
  return (
    <Markdown
      remarkPlugins={[remarkGfm]}
      skipHtml
      components={{
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
      {markdown}
    </Markdown>
  );
}
