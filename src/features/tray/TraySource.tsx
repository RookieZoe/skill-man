import { useEffect, useState } from "react";
import type { CatalogClient, SkillSummary } from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

/** Read existing detail only when names need additional disambiguation. */
export function TraySource({
  client,
  skill,
  ambiguous,
}: {
  client: CatalogClient;
  skill: SkillSummary;
  ambiguous: boolean;
}) {
  const { t } = useLocale();
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    if (ambiguous)
      void client.inspectSkill(skill.id).then(
        (detail) => {
          if (current)
            setPath(detail.fileSourceOriginalPath ?? detail.finalEntityPath);
        },
        () => {},
      );
    return () => {
      current = false;
    };
  }, [client, skill.id, ambiguous]);
  const kind =
    skill.sourceKind === "remote_install"
      ? "library.source_kind.git"
      : skill.sourceKind === "file_install"
        ? "library.source_kind.file"
        : "library.source_kind.link";
  return (
    <small>
      {t(kind)} · {skill.directoryName}
      {ambiguous && (
        <span className="tray-source-path">{path ?? skill.id}</span>
      )}
    </small>
  );
}
