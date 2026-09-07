import { useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useLocale } from "../features/locale/LocaleProvider";

export function RepositoryLink({ url }: { url: string }) {
  const [failed, setFailed] = useState(false);
  const { t } = useLocale();
  let safe = false;
  try {
    const parsed = new URL(url);
    safe =
      ["https:", "http:"].includes(parsed.protocol) &&
      !parsed.username &&
      !parsed.password;
  } catch {
    /* Invalid Source Content stays inert. */
  }
  if (!safe) return <span>{url}</span>;
  return (
    <>
      <a
        className="repository-link"
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        onClick={(event) => {
          event.stopPropagation();
          if (!isTauri()) return;
          event.preventDefault();
          setFailed(false);
          void openUrl(url).catch(() => setFailed(true));
        }}
      >
        {url}
      </a>
      {failed && <span role="alert">{t("repository.open_failed")}</span>}
    </>
  );
}
