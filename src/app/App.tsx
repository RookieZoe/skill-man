import { useEffect, useState } from "react";

import { LibraryDesk } from "../features/library/LibraryDesk";
import type {
  AgentActivation,
  CatalogClient,
  CatalogFilter,
  SkillDetail,
  SkillSummary,
} from "./catalog-client";

export interface AppProps {
  client: CatalogClient;
}

export function App({ client }: AppProps) {
  const [filter, setFilter] = useState<CatalogFilter>("all");
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [agents, setAgents] = useState<AgentActivation[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    client
      .listSkills(filter)
      .then((snapshot) => {
        if (!current) return;
        setSkills(snapshot.items);
        setSelectedId((selected) =>
          snapshot.items.some(({ id }) => id === selected)
            ? selected
            : (snapshot.items[0]?.id ?? null),
        );
        if (snapshot.items.length === 0) {
          setDetail(null);
          setAgents([]);
        }
        setError(null);
      })
      .catch((reason: unknown) => {
        if (current) setError(readError(reason));
      });
    return () => {
      current = false;
    };
  }, [client, filter]);

  useEffect(() => {
    if (!selectedId) return;

    let current = true;
    Promise.all([
      client.inspectSkill(selectedId),
      client.listAgents(selectedId),
    ])
      .then(([nextDetail, nextAgents]) => {
        if (!current) return;
        setDetail(nextDetail);
        setAgents(nextAgents);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (current) setError(readError(reason));
      });
    return () => {
      current = false;
    };
  }, [client, selectedId]);

  return (
    <LibraryDesk
      filter={filter}
      skills={skills}
      selectedId={selectedId}
      detail={detail}
      agents={agents}
      error={error}
      onFilter={setFilter}
      onSelect={setSelectedId}
    />
  );
}

function readError(reason: unknown) {
  return reason instanceof Error
    ? reason.message
    : "The catalog could not be read.";
}
