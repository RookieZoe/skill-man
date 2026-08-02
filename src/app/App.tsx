import { useEffect, useState } from "react";

import { LibraryDesk } from "../features/library/LibraryDesk";
import type {
  ActivationPreview,
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
  const [activationError, setActivationError] = useState<string | null>(null);
  const [activationPreview, setActivationPreview] =
    useState<ActivationPreview | null>(null);
  const [activationTriggerAgentId, setActivationTriggerAgentId] = useState<
    string | null
  >(null);
  const [pendingAgentId, setPendingAgentId] = useState<string | null>(null);
  const [isApplyingActivation, setIsApplyingActivation] = useState(false);

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

  async function requestActivation(agentId: string, enabled: boolean) {
    if (!selectedId) return;
    setActivationTriggerAgentId(agentId);
    setPendingAgentId(agentId);
    setActivationError(null);
    try {
      setActivationPreview(
        await client.planActivation(selectedId, agentId, enabled),
      );
    } catch (reason) {
      setActivationError(readError(reason));
      try {
        setAgents(await client.listAgents(selectedId));
      } catch {
        // Keep the preflight error as the primary actionable message.
      }
    } finally {
      setPendingAgentId(null);
    }
  }

  async function cancelActivation() {
    if (!activationPreview) return;
    const planToken = activationPreview.planToken;
    setActivationPreview(null);
    try {
      await client.cancelActivation(planToken);
    } catch (reason) {
      setActivationError(readError(reason));
    }
  }

  async function applyActivation() {
    if (!activationPreview || !selectedId) return;
    setIsApplyingActivation(true);
    setActivationError(null);
    try {
      await client.applyActivation(activationPreview.planToken);
      const [snapshot, nextDetail, nextAgents] = await Promise.all([
        client.listSkills(filter),
        client.inspectSkill(selectedId),
        client.listAgents(selectedId),
      ]);
      setSkills(snapshot.items);
      setDetail(nextDetail);
      setAgents(nextAgents);
      setActivationPreview(null);
    } catch (reason) {
      setActivationError(readError(reason));
      setActivationPreview(null);
    } finally {
      setIsApplyingActivation(false);
    }
  }

  return (
    <LibraryDesk
      filter={filter}
      skills={skills}
      selectedId={selectedId}
      detail={detail}
      agents={agents}
      error={error}
      activationError={activationError}
      activationPreview={activationPreview}
      activationTriggerAgentId={activationTriggerAgentId}
      pendingAgentId={pendingAgentId}
      isApplyingActivation={isApplyingActivation}
      onFilter={setFilter}
      onSelect={setSelectedId}
      onRequestActivation={requestActivation}
      onApplyActivation={applyActivation}
      onCancelActivation={cancelActivation}
    />
  );
}

function readError(reason: unknown) {
  if (reason instanceof Error) return reason.message;
  if (
    typeof reason === "object" &&
    reason !== null &&
    "message" in reason &&
    typeof reason.message === "string"
  ) {
    return reason.message;
  }
  return "The catalog could not be read.";
}
