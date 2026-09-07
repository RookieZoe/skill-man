import type { SkillSummary } from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

export function SkillSelectionStack({
  skills,
  inert = false,
}: {
  skills: SkillSummary[];
  inert?: boolean;
}) {
  const { tPlural } = useLocale();
  const top = skills.at(-1);
  if (!top) return null;
  const count = tPlural("library.selection.count", skills.length);
  return (
    <main
      id="skill-detail"
      className="skill-detail skill-selection-detail"
      aria-label={count}
      inert={inert ? true : undefined}
    >
      <div className="skill-selection-stack">
        {skills.length > 2 && (
          <div
            className="skill-stack-back skill-stack-back--second"
            aria-hidden="true"
          />
        )}
        <div className="skill-stack-back" aria-hidden="true" />
        <article className="skill-stack-front">
          <span className="count-badge" role="status">
            {count}
          </span>
          <h2>{top.directoryName}</h2>
          <p>{top.description}</p>
        </article>
      </div>
    </main>
  );
}
