import { useEffect, useRef, useState } from "react";

import { LibraryDesk } from "../features/library/LibraryDesk";
import type {
  ActivationPreview,
  AgentActivation,
  CatalogClient,
  CatalogFilter,
  LinkImportPreview,
  LinkImportResult,
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
  const [agentsReadyForSkillId, setAgentsReadyForSkillId] = useState<
    string | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [activationError, setActivationError] = useState<string | null>(null);
  const [activationConflict, setActivationConflict] = useState<string | null>(
    null,
  );
  const [activationPreview, setActivationPreview] =
    useState<ActivationPreview | null>(null);
  const [activationTriggerControlId, setActivationTriggerControlId] = useState<
    string | null
  >(null);
  const [pendingAgentId, setPendingAgentId] = useState<string | null>(null);
  const [isApplyingActivation, setIsApplyingActivation] = useState(false);
  const [startupHealthComplete, setStartupHealthComplete] = useState(false);
  const [isLinkImportOpen, setIsLinkImportOpen] = useState(false);
  const [linkImportPreview, setLinkImportPreview] =
    useState<LinkImportPreview | null>(null);
  const [linkImportResult, setLinkImportResult] =
    useState<LinkImportResult | null>(null);
  const [linkImportError, setLinkImportError] = useState<string | null>(null);
  const [linkImportActivity, setLinkImportActivity] = useState<
    "idle" | "discovering" | "applying"
  >("idle");
  const linkImportRunId = useRef(0);

  useEffect(() => {
    let current = true;
    client
      .runActivationHealthCheck()
      .catch(() => {
        // Startup maintenance is best effort and must not block Library browsing.
      })
      .finally(() => {
        if (current) setStartupHealthComplete(true);
      });
    return () => {
      current = false;
    };
  }, [client]);

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
          setAgentsReadyForSkillId(null);
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
    client
      .inspectSkill(selectedId)
      .then((nextDetail) => {
        if (!current) return;
        setDetail(nextDetail);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (current) setError(readError(reason));
      });
    return () => {
      current = false;
    };
  }, [client, selectedId]);

  useEffect(() => {
    if (!selectedId || !startupHealthComplete) return;

    let current = true;
    client
      .listAgents(selectedId)
      .then((nextAgents) => {
        if (!current) return;
        setAgents(nextAgents);
        setAgentsReadyForSkillId(selectedId);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (!current) return;
        setAgents([]);
        setAgentsReadyForSkillId(selectedId);
        setError(readError(reason));
      });
    return () => {
      current = false;
    };
  }, [client, selectedId, startupHealthComplete]);

  async function requestActivation(agentId: string, enabled: boolean) {
    if (!selectedId) return;
    setActivationTriggerControlId(`activation-${selectedId}-${agentId}`);
    setPendingAgentId(agentId);
    setActivationError(null);
    setActivationConflict(null);
    try {
      setActivationPreview(
        await client.planActivation(selectedId, agentId, enabled),
      );
    } catch (reason) {
      await handleActivationPlanError(reason, selectedId);
    } finally {
      setPendingAgentId(null);
    }
  }

  async function requestActivationRepair(agentId: string) {
    if (!selectedId) return;
    setActivationTriggerControlId(`activation-repair-${selectedId}-${agentId}`);
    setPendingAgentId(agentId);
    setActivationError(null);
    setActivationConflict(null);
    try {
      setActivationPreview(
        await client.planActivationRepair(selectedId, agentId),
      );
    } catch (reason) {
      await handleActivationPlanError(reason, selectedId);
    } finally {
      setPendingAgentId(null);
    }
  }

  async function handleActivationPlanError(reason: unknown, skillId: string) {
    const failure = readCommandError(reason);
    if (failure.code === "conflict") {
      setActivationConflict(failure.message);
    } else {
      setActivationError(failure.message);
    }
    try {
      setAgents(await client.listAgents(skillId));
      setAgentsReadyForSkillId(skillId);
    } catch {
      // Keep the preflight failure as the primary actionable message.
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
      setAgentsReadyForSkillId(selectedId);
      setActivationPreview(null);
    } catch (reason) {
      const failure = readCommandError(reason);
      try {
        setAgents(await client.listAgents(selectedId));
        setAgentsReadyForSkillId(selectedId);
      } catch {
        // Keep the Apply failure as the primary actionable message.
      }
      if (failure.code === "conflict") {
        setActivationConflict(failure.message);
      } else {
        setActivationError(failure.message);
      }
      setActivationPreview(null);
    } finally {
      setIsApplyingActivation(false);
    }
  }

  function openLinkImport() {
    linkImportRunId.current += 1;
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setIsLinkImportOpen(true);
  }

  async function previewLinkImport(sourcePath: string) {
    const runId = ++linkImportRunId.current;
    setLinkImportActivity("discovering");
    setLinkImportError(null);
    try {
      await client.discoverLinkImport(sourcePath);
      if (runId !== linkImportRunId.current) return;
      const preview = await client.planLinkImport(sourcePath);
      if (runId !== linkImportRunId.current) {
        await client.cancelLinkImport(preview.planToken).catch(() => undefined);
        return;
      }
      setLinkImportPreview(preview);
    } catch (reason) {
      if (runId === linkImportRunId.current) {
        setLinkImportPreview(null);
        setLinkImportError(readError(reason));
      }
    } finally {
      if (runId === linkImportRunId.current) setLinkImportActivity("idle");
    }
  }

  async function applyLinkImport() {
    if (!linkImportPreview?.canApply) return;
    const runId = ++linkImportRunId.current;
    setLinkImportActivity("applying");
    setLinkImportError(null);
    try {
      const result = await client.applyLinkImport(linkImportPreview.planToken);
      const snapshot = await client.listSkills(filter);
      setSkills(snapshot.items);
      setLinkImportPreview(null);
      setLinkImportResult(result);
    } catch (reason) {
      setLinkImportPreview(null);
      setLinkImportError(readError(reason));
    } finally {
      if (runId === linkImportRunId.current) setLinkImportActivity("idle");
    }
  }

  async function closeLinkImport() {
    if (linkImportActivity === "applying") return;
    linkImportRunId.current += 1;
    const planToken = linkImportPreview?.planToken;
    setIsLinkImportOpen(false);
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setLinkImportActivity("idle");
    if (!planToken) return;
    try {
      await client.cancelLinkImport(planToken);
    } catch (reason) {
      setError(readError(reason));
    }
  }

  function openImportedSkill() {
    if (!linkImportResult) return;
    setFilter("all");
    setSelectedId(linkImportResult.skillId);
    setIsLinkImportOpen(false);
    setLinkImportResult(null);
    setLinkImportError(null);
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
      activationConflict={activationConflict}
      activationPreview={activationPreview}
      activationTriggerControlId={activationTriggerControlId}
      pendingAgentId={pendingAgentId}
      isApplyingActivation={isApplyingActivation}
      isCheckingActivations={
        !startupHealthComplete || agentsReadyForSkillId !== selectedId
      }
      isLinkImportOpen={isLinkImportOpen}
      linkImportPreview={linkImportPreview}
      linkImportResult={linkImportResult}
      linkImportError={linkImportError}
      linkImportActivity={linkImportActivity}
      onFilter={setFilter}
      onSelect={setSelectedId}
      onRequestActivation={requestActivation}
      onRequestActivationRepair={requestActivationRepair}
      onApplyActivation={applyActivation}
      onCancelActivation={cancelActivation}
      onCloseActivationConflict={() => setActivationConflict(null)}
      onOpenLinkImport={openLinkImport}
      onPreviewLinkImport={previewLinkImport}
      onApplyLinkImport={applyLinkImport}
      onCloseLinkImport={closeLinkImport}
      onOpenImportedSkill={openImportedSkill}
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

function readCommandError(reason: unknown) {
  return {
    code:
      typeof reason === "object" &&
      reason !== null &&
      "code" in reason &&
      typeof reason.code === "string"
        ? reason.code
        : "internal",
    message: readError(reason),
  };
}
