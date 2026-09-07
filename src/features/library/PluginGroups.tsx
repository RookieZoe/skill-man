import type { ReactNode } from "react";
import { useLocale } from "../locale/LocaleProvider";

export function groupPluginMembers<T>(
  items: T[],
  pluginName: (item: T) => string | null | undefined,
) {
  if (!items.some((item) => pluginName(item))) return null;
  const groups = new Map<string, T[]>();
  for (const item of items) {
    const name = pluginName(item) || "";
    const members = groups.get(name) ?? [];
    members.push(item);
    groups.set(name, members);
  }
  return [...groups].sort(([a], [b]) =>
    !a ? 1 : !b ? -1 : a.localeCompare(b),
  );
}

/** Plugin labels are Source Content; only the fallback label is App Copy. */
export function PluginGroups<T>({
  items,
  pluginName,
  children,
  defaultOpen = true,
  collapsible = true,
}: {
  items: T[];
  pluginName: (item: T) => string | null | undefined;
  children: (members: T[]) => ReactNode;
  defaultOpen?: boolean;
  collapsible?: boolean;
}) {
  const { t } = useLocale();
  const groups = groupPluginMembers(items, pluginName);
  if (!groups) return children(items);
  return (
    <div className="plugin-groups">
      {groups.map(([name, members]) =>
        collapsible ? (
          <details className="plugin-group" key={name} open={defaultOpen}>
            <summary className="plugin-group-heading">
              <span>
                {name
                  ? name
                      .split("-")
                      .map(
                        (part) => part.charAt(0).toUpperCase() + part.slice(1),
                      )
                      .join(" ")
                  : t("library.sidebar.other_plugins")}
              </span>
              <span className="count-badge">{members.length}</span>
            </summary>
            {children(members)}
          </details>
        ) : (
          <section className="plugin-group" key={name}>
            <h3 className="plugin-group-heading">
              <span>
                {name
                  ? name
                      .split("-")
                      .map(
                        (part) => part.charAt(0).toUpperCase() + part.slice(1),
                      )
                      .join(" ")
                  : t("library.sidebar.other_plugins")}
              </span>
              <span className="count-badge">{members.length}</span>
            </h3>
            {children(members)}
          </section>
        ),
      )}
    </div>
  );
}
