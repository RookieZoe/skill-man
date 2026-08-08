import { useEffect, useRef, useState } from "react";

import { LibraryDesk } from "../features/library/LibraryDesk";
import type {
  ActivationPreview,
  AgentActivation,
  CatalogClient,
  CatalogFilter,
  GitImportDiscovery,
  GitImportSelectionPreview,
  GitImportSelectionResult,
  LinkImportPreview,
  LinkImportResult,
  SkillDetail,
  SkillSummary,
  UpdateCheckReport,
  UpdatePlan,
  UpdateResult,
} from "./catalog-client";

export interface AppProps {
  client: CatalogClient;
}

export type ImportKind = "link" | "git";

export interface UpdatePanelState {
  activity: "idle" | "checking" | "planning" | "applying" | "pinning";
  error: string | null;
  report: UpdateCheckReport | null;
  plan: UpdatePlan | null;
  result: UpdateResult | null;
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
  const [importKind, setImportKind] = useState<ImportKind>("link");
  const [gitImportSource, setGitImportSource] = useState("");
  const [gitImportForceFullDepth, setGitImportForceFullDepth] =
    useState(false);
  const [gitImportDiscovery, setGitImportDiscovery] =
    useState<GitImportDiscovery | null>(null);
  const [gitImportSelected, setGitImportSelected] = useState<string[]>([]);
  const [gitImportPreview, setGitImportPreview] =
    useState<GitImportSelectionPreview | null>(null);
  const [gitImportResult, setGitImportResult] =
    useState<GitImportSelectionResult | null>(null);
  const [gitImportError, setGitImportError] = useState<string | null>(null);
  const [gitImportActivity, setGitImportActivity] = useState<
    "idle" | "discovering" | "planning" | "applying"
  >("idle");
  const gitImportRunId = useRef(0);
  const [updatePanel, setUpdatePanel] = useState<UpdatePanelState>({
    activity: "idle",
    error: null,
    report: null,
    plan: null,
    result: null,
  });
  const [reselectPath, setReselectPath] = useState("");
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
    // Spec §10.1: background Skill update check at startup, at most once per
    // 24h (the cooldown lives in the backend). Failing silently is fine; the
    // per-Skill "Check for updates" button always forces a fresh check.
    let current = true;
    client
      .checkSkillUpdates(false)
      .then((report) => {
        if (current) {
          setUpdatePanel((state) => ({ ...state, report }));
        }
      })
      .catch(() => undefined);
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

  useEffect(() => {
    setUpdatePanel({
      activity: "idle",
      error: null,
      report: null,
      plan: null,
      result: null,
    });
    setReselectPath("");
  }, [selectedId]);

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

  function openImport() {
    linkImportRunId.current += 1;
    gitImportRunId.current += 1;
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setGitImportDiscovery(null);
    setGitImportSelected([]);
    setGitImportPreview(null);
    setGitImportResult(null);
    setGitImportError(null);
    setImportKind("link");
    setIsLinkImportOpen(true);
  }

  async function discoverGitImport(source: string, forceFullDepth: boolean) {
    const runId = ++gitImportRunId.current;
    setGitImportActivity("discovering");
    setGitImportError(null);
    setGitImportDiscovery(null);
    setGitImportPreview(null);
    setGitImportSelected([]);
    try {
      const discovery = await client.discoverGitImport(source, forceFullDepth);
      if (runId !== gitImportRunId.current) return;
      setGitImportDiscovery(discovery);
      setGitImportSelected(
        discovery.candidates.map((candidate) => candidate.directoryName),
      );
    } catch (reason) {
      if (runId === gitImportRunId.current) {
        setGitImportError(readError(reason));
      }
    } finally {
      if (runId === gitImportRunId.current) setGitImportActivity("idle");
    }
  }

  async function planGitImport() {
    if (!gitImportDiscovery || gitImportSelected.length === 0) return;
    const runId = ++gitImportRunId.current;
    setGitImportActivity("planning");
    setGitImportError(null);
    try {
      const preview = await client.planGitImportSelection(
        gitImportSource,
        gitImportForceFullDepth,
        gitImportSelected,
      );
      if (runId !== gitImportRunId.current) {
        await client
          .cancelGitImportSelection(preview.planToken)
          .catch(() => undefined);
        return;
      }
      setGitImportPreview(preview);
    } catch (reason) {
      if (runId === gitImportRunId.current) {
        setGitImportError(readError(reason));
      }
    } finally {
      if (runId === gitImportRunId.current) setGitImportActivity("idle");
    }
  }

  async function applyGitImport() {
    if (!gitImportPreview?.canApply) return;
    const runId = ++gitImportRunId.current;
    setGitImportActivity("applying");
    setGitImportError(null);
    try {
      const result = await client.applyGitImportSelection(
        gitImportPreview.planToken,
      );
      const snapshot = await client.listSkills(filter);
      setSkills(snapshot.items);
      setGitImportPreview(null);
      setGitImportResult(result);
    } catch (reason) {
      setGitImportPreview(null);
      setGitImportError(readError(reason));
    } finally {
      if (runId === gitImportRunId.current) setGitImportActivity("idle");
    }
  }

  async function closeImport() {
    if (linkImportActivity === "applying" || gitImportActivity === "applying")
      return;
    linkImportRunId.current += 1;
    gitImportRunId.current += 1;
    const linkPlanToken = linkImportPreview?.planToken;
    const gitPlanToken = gitImportPreview?.planToken;
    setIsLinkImportOpen(false);
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setLinkImportActivity("idle");
    setGitImportDiscovery(null);
    setGitImportSelected([]);
    setGitImportPreview(null);
    setGitImportResult(null);
    setGitImportError(null);
    setGitImportActivity("idle");
    if (linkPlanToken) {
      await client.cancelLinkImport(linkPlanToken).catch(() => undefined);
    }
    if (gitPlanToken) {
      await client
        .cancelGitImportSelection(gitPlanToken)
        .catch(() => undefined);
    }
  }

  function openImportedGitSkill(skillId: string) {
    setFilter("all");
    setSelectedId(skillId);
    setIsLinkImportOpen(false);
    setGitImportResult(null);
    setGitImportError(null);
  }

  async function checkSkillUpdates() {
    if (!selectedId) return;
    setUpdatePanel((state) => ({
      ...state,
      activity: "checking",
      error: null,
      report: null,
      plan: null,
      result: null,
    }));
    try {
      const report = await client.checkSkillUpdates(true);
      setUpdatePanel((state) => ({ ...state, activity: "idle", report }));
    } catch (reason) {
      setUpdatePanel((state) => ({
        ...state,
        activity: "idle",
        error: readError(reason),
      }));
    }
  }

  async function planSkillUpdate(newSkillPath: string | null) {
    if (!selectedId) return;
    setUpdatePanel((state) => ({
      ...state,
      activity: "planning",
      error: null,
      plan: null,
      result: null,
    }));
    try {
      const plan = await client.planSkillUpdates([
        { skillId: selectedId, newSkillPath },
      ]);
      setUpdatePanel((state) => ({ ...state, activity: "idle", plan }));
    } catch (reason) {
      setUpdatePanel((state) => ({
        ...state,
        activity: "idle",
        error: readError(reason),
      }));
    }
  }

  async function applySkillUpdate(abandonChanges: boolean) {
    if (!selectedId || !updatePanel.plan) return;
    const item = updatePanel.plan.items[0];
    if (!item?.planToken) return;
    setUpdatePanel((state) => ({
      ...state,
      activity: "applying",
      error: null,
    }));
    try {
      const result = await client.applySkillUpdates(
        [
          {
            planToken: item.planToken,
            skillId: item.skillId,
            directoryName: item.directoryName,
          },
        ],
        abandonChanges,
      );
      const [snapshot, nextDetail, nextAgents] = await Promise.all([
        client.listSkills(filter),
        client.inspectSkill(selectedId),
        client.listAgents(selectedId),
      ]);
      setSkills(snapshot.items);
      setDetail(nextDetail);
      setAgents(nextAgents);
      setAgentsReadyForSkillId(selectedId);
      setUpdatePanel((state) => ({
        ...state,
        activity: "idle",
        plan: null,
        result,
      }));
    } catch (reason) {
      setUpdatePanel((state) => ({
        ...state,
        activity: "idle",
        error: readError(reason),
      }));
    }
  }

  async function pinSkillUpdate() {
    if (!selectedId) return;
    setUpdatePanel((state) => ({
      ...state,
      activity: "pinning",
      error: null,
    }));
    try {
      await client.pinSkillUpdates([selectedId]);
      setUpdatePanel((state) => ({
        ...state,
        activity: "idle",
        report: null,
        plan: null,
        result: null,
      }));
    } catch (reason) {
      setUpdatePanel((state) => ({
        ...state,
        activity: "idle",
        error: readError(reason),
      }));
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
      activationConflict={activationConflict}
      activationPreview={activationPreview}
      activationTriggerControlId={activationTriggerControlId}
      pendingAgentId={pendingAgentId}
      isApplyingActivation={isApplyingActivation}
      isCheckingActivations={
        !startupHealthComplete || agentsReadyForSkillId !== selectedId
      }
      isLinkImportOpen={isLinkImportOpen}
      importKind={importKind}
      linkImportPreview={linkImportPreview}
      linkImportResult={linkImportResult}
      linkImportError={linkImportError}
      linkImportActivity={linkImportActivity}
      gitImportSource={gitImportSource}
      gitImportForceFullDepth={gitImportForceFullDepth}
      gitImportDiscovery={gitImportDiscovery}
      gitImportSelected={gitImportSelected}
      gitImportPreview={gitImportPreview}
      gitImportResult={gitImportResult}
      gitImportError={gitImportError}
      gitImportActivity={gitImportActivity}
      updatePanel={updatePanel}
      reselectPath={reselectPath}
      onFilter={setFilter}
      onSelect={setSelectedId}
      onRequestActivation={requestActivation}
      onRequestActivationRepair={requestActivationRepair}
      onApplyActivation={applyActivation}
      onCancelActivation={cancelActivation}
      onCloseActivationConflict={() => setActivationConflict(null)}
      onOpenLinkImport={openImport}
      onImportKindChange={setImportKind}
      onPreviewLinkImport={previewLinkImport}
      onApplyLinkImport={applyLinkImport}
      onCloseLinkImport={closeImport}
      onOpenImportedSkill={openImportedSkill}
      onGitImportSourceChange={setGitImportSource}
      onGitImportForceFullDepthChange={setGitImportForceFullDepth}
      onDiscoverGitImport={discoverGitImport}
      onGitImportSelectionChange={setGitImportSelected}
      onPlanGitImport={planGitImport}
      onApplyGitImport={applyGitImport}
      onOpenImportedGitSkill={openImportedGitSkill}
      onCheckSkillUpdates={checkSkillUpdates}
      onPlanSkillUpdate={planSkillUpdate}
      onApplySkillUpdate={applySkillUpdate}
      onPinSkillUpdate={pinSkillUpdate}
      onReselectPathChange={setReselectPath}
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
