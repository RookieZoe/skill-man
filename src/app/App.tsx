import { useEffect, useRef, useState } from "react";

import { listen } from "@tauri-apps/api/event";

import { LibraryDesk } from "../features/library/LibraryDesk";
import type {
  ActivationConflictDetails,
  ActivationPreview,
  ActivationReplacePreview,
  ActivationReplaceUndoResult,
  ActivationResult,
  AdoptPlan,
  AdoptResult,
  AdoptScanReport,
  AdoptUndoResult,
  AgentActivation,
  AppPreferences,
  CatalogClient,
  CatalogFilter,
  GitImportDiscovery,
  GitImportSelectionPreview,
  GitImportSelectionResult,
  LinkImportPreview,
  LinkImportResult,
  PreferenceUpdates,
  RelocateLinkPreview,
  RelocateLinkResult,
  SkillDetail,
  SkillSummary,
  StartupAgent,
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

export interface RelocatePanelState {
  isOpen: boolean;
  activity: "idle" | "previewing" | "applying";
  sourcePath: string;
  preview: RelocateLinkPreview | null;
  result: RelocateLinkResult | null;
  error: string | null;
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
  const [preferences, setPreferences] = useState<AppPreferences | null>(null);
  const [preferencesWarning, setPreferencesWarning] = useState<string | null>(
    null,
  );
  const [preferencesError, setPreferencesError] = useState<string | null>(null);
  const [isPreferencesOpen, setIsPreferencesOpen] = useState(false);
  const [startupAgents, setStartupAgents] = useState<StartupAgent[]>([]);
  const [isOnboardingOpen, setIsOnboardingOpen] = useState(false);
  const [onboardingStep, setOnboardingStep] = useState(0);
  const [onboardingReport, setOnboardingReport] =
    useState<AdoptScanReport | null>(null);
  const [onboardingActivity, setOnboardingActivity] = useState<
    "idle" | "scanning"
  >("idle");
  const [onboardingError, setOnboardingError] = useState<string | null>(null);
  const [activationConflict, setActivationConflict] =
    useState<ActivationConflictDetails | null>(null);
  const [activationConflictMessage, setActivationConflictMessage] = useState<
    string | null
  >(null);
  const [replacePreview, setReplacePreview] =
    useState<ActivationReplacePreview | null>(null);
  const [replaceOperationId, setReplaceOperationId] = useState<string | null>(
    null,
  );
  const [replaceResult, setReplaceResult] = useState<ActivationResult | null>(
    null,
  );
  const [replaceUndo, setReplaceUndo] =
    useState<ActivationReplaceUndoResult | null>(null);
  const [replaceError, setReplaceError] = useState<string | null>(null);
  const [isApplyingReplace, setIsApplyingReplace] = useState(false);
  const [isUndoingReplace, setIsUndoingReplace] = useState(false);
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
  const [gitImportForceFullDepth, setGitImportForceFullDepth] = useState(false);
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
  const [relocatePanel, setRelocatePanel] = useState<RelocatePanelState>({
    isOpen: false,
    activity: "idle",
    sourcePath: "",
    preview: null,
    result: null,
    error: null,
  });
  const [reselectPath, setReselectPath] = useState("");
  const [isAdoptOpen, setIsAdoptOpen] = useState(false);
  const [adoptReport, setAdoptReport] = useState<AdoptScanReport | null>(null);
  const [adoptSelected, setAdoptSelected] = useState<string[]>([]);
  const [adoptPlan, setAdoptPlan] = useState<AdoptPlan | null>(null);
  const [adoptResult, setAdoptResult] = useState<AdoptResult | null>(null);
  const [adoptUndo, setAdoptUndo] = useState<AdoptUndoResult | null>(null);
  const [adoptError, setAdoptError] = useState<string | null>(null);
  const [adoptActivity, setAdoptActivity] = useState<
    "idle" | "scanning" | "planning" | "applying" | "undoing"
  >("idle");
  const adoptRunId = useRef(0);
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
      .startupInfo()
      .then((info) => {
        if (!current) return;
        setStartupAgents(info.agents);
        if (info.firstRun) {
          // Spec §8.7: the three-step onboarding runs on the first launch;
          // skipping still records completion so later launches light-scan.
          setIsOnboardingOpen(true);
          setOnboardingStep(0);
        }
      })
      .catch(() => {
        // Read-only locked state surfaces elsewhere; onboarding stays closed.
      });
    client
      .loadPreferences()
      .then((loaded) => {
        if (current) setPreferences(loaded);
      })
      .catch(() => {
        // Preferences stay null; the sheet shows an inline error on open.
      });
    return () => {
      current = false;
    };
  }, [client]);

  // Tray quick view: clicking a recently enabled Skill opens its detail
  // (spec §9.4). Native-only; the preview fixture has no event bus.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let unlisten: (() => void) | undefined;
    listen<{ skillId: string }>("tray-open-skill", (event) => {
      setFilter("all");
      setSelectedId(event.payload.skillId);
    }).then((dispose) => {
      unlisten = dispose;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    // Spec §10.1 + §10.2: the background Skill update check runs at startup
    // only when the preference is on (the cooldown lives in the backend).
    // Failing silently is fine; the per-Skill button always forces a check.
    if (preferences === null) return;
    if (!preferences.checkSkillUpdates) return;
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
  }, [client, preferences]);

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
    setActivationConflictMessage(null);
    try {
      setActivationPreview(
        await client.planActivation(selectedId, agentId, enabled),
      );
    } catch (reason) {
      await handleActivationPlanError(reason, selectedId, agentId);
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
    setActivationConflictMessage(null);
    try {
      setActivationPreview(
        await client.planActivationRepair(selectedId, agentId),
      );
    } catch (reason) {
      await handleActivationPlanError(reason, selectedId, agentId);
    } finally {
      setPendingAgentId(null);
    }
  }

  async function handleActivationPlanError(
    reason: unknown,
    skillId: string,
    agentId: string,
  ) {
    const failure = readCommandError(reason);
    if (failure.code === "conflict") {
      setActivationConflictMessage(failure.message);
      try {
        setActivationConflict(
          await client.activationConflictDetails(skillId, agentId),
        );
      } catch {
        // Keep the message-only Conflict sheet when details are unavailable.
      }
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
    const { skillId, agentId } = activationPreview;
    try {
      await client.applyActivation(activationPreview.planToken);
      await refreshAfterActivationChange();
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
        setActivationConflictMessage(failure.message);
        try {
          setActivationConflict(
            await client.activationConflictDetails(skillId, agentId),
          );
        } catch {
          // Keep the message-only Conflict sheet when details are unavailable.
        }
      } else {
        setActivationError(failure.message);
      }
      setActivationPreview(null);
    } finally {
      setIsApplyingActivation(false);
    }
  }

  async function refreshAfterActivationChange() {
    if (!selectedId) return;
    const [snapshot, nextDetail, nextAgents] = await Promise.all([
      client.listSkills(filter),
      client.inspectSkill(selectedId),
      client.listAgents(selectedId),
    ]);
    setSkills(snapshot.items);
    setDetail(nextDetail);
    setAgents(nextAgents);
    setAgentsReadyForSkillId(selectedId);
  }

  // -- Activation Conflict: Adopt existing item / Remove then replace / Cancel

  /** Adopt the occupying item instead of replacing it: hand off to the Adopt
   *  flow with that candidate pre-selected. */
  async function adoptFromConflict(canonicalEntity: string) {
    setActivationConflict(null);
    setActivationConflictMessage(null);
    setReplacePreview(null);
    setReplaceOperationId(null);
    setReplaceResult(null);
    setReplaceUndo(null);
    setReplaceError(null);
    setIsAdoptOpen(true);
    setAdoptReport(null);
    setAdoptSelected([]);
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    const runId = ++adoptRunId.current;
    setAdoptActivity("scanning");
    try {
      const report = await client.scanAdopt();
      if (runId !== adoptRunId.current) return;
      setAdoptReport(report);
      setAdoptSelected(
        report.candidates.some(
          (candidate) => candidate.canonicalEntity === canonicalEntity,
        )
          ? [canonicalEntity]
          : [],
      );
    } catch (reason) {
      if (runId === adoptRunId.current) setAdoptError(readError(reason));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  async function planReplace() {
    if (!activationConflict) return;
    const { skillId, agentId } = activationConflict;
    setReplaceError(null);
    setReplacePreview(null);
    try {
      setReplacePreview(await client.planActivationReplace(skillId, agentId));
    } catch (reason) {
      setReplaceError(readError(reason));
    }
  }

  async function applyReplace() {
    if (!replacePreview) return;
    setIsApplyingReplace(true);
    setReplaceError(null);
    const operationId = replacePreview.operationId;
    try {
      const result = await client.applyActivationReplace(
        replacePreview.planToken,
      );
      await refreshAfterActivationChange();
      setReplacePreview(null);
      setReplaceOperationId(operationId);
      setReplaceResult(result);
    } catch (reason) {
      setReplacePreview(null);
      setReplaceError(readError(reason));
    } finally {
      setIsApplyingReplace(false);
    }
  }

  async function undoReplace() {
    if (!replaceOperationId) return;
    setIsUndoingReplace(true);
    setReplaceError(null);
    try {
      const undo = await client.undoActivationReplace(replaceOperationId);
      await refreshAfterActivationChange();
      setReplaceUndo(undo);
    } catch (reason) {
      setReplaceError(readError(reason));
    } finally {
      setIsUndoingReplace(false);
    }
  }

  function closeActivationConflict() {
    if (isApplyingReplace || isUndoingReplace) return;
    const planToken = replacePreview?.planToken;
    // Undo that fully succeeded already finished the journal; any other
    // outcome (no undo, skipped or failed undo) leaves a committed backup
    // that must be discarded on close.
    const operationId =
      replaceOperationId && (!replaceUndo || !replaceUndo.undone)
        ? replaceOperationId
        : null;
    setActivationConflict(null);
    setActivationConflictMessage(null);
    setReplacePreview(null);
    setReplaceOperationId(null);
    setReplaceResult(null);
    setReplaceUndo(null);
    setReplaceError(null);
    if (planToken) {
      client.cancelActivationReplace(planToken).catch((reason) => {
        setActivationError(readError(reason));
      });
    }
    if (operationId) {
      client.finalizeActivationReplace(operationId).catch((reason) => {
        setActivationError(readError(reason));
      });
    }
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

  function openRelocate() {
    if (!selectedId) return;
    setRelocatePanel({
      isOpen: true,
      activity: "idle",
      sourcePath: "",
      preview: null,
      result: null,
      error: null,
    });
  }

  function closeRelocate() {
    const { preview } = relocatePanel;
    setRelocatePanel((state) => ({
      ...state,
      isOpen: false,
      preview: null,
      result: null,
    }));
    if (preview) {
      void client.cancelRelocateLink(preview.planToken).catch(() => undefined);
    }
  }

  async function previewRelocate(sourcePath: string) {
    if (!selectedId) return;
    setRelocatePanel((state) => ({
      ...state,
      activity: "previewing",
      error: null,
      preview: null,
    }));
    try {
      const preview = await client.relocateLink(selectedId, sourcePath);
      setRelocatePanel((state) => ({
        ...state,
        activity: "idle",
        sourcePath,
        preview,
      }));
    } catch (reason) {
      setRelocatePanel((state) => ({
        ...state,
        activity: "idle",
        error: readError(reason),
      }));
    }
  }

  async function applyRelocate() {
    const { preview } = relocatePanel;
    if (!selectedId || !preview) return;
    setRelocatePanel((state) => ({
      ...state,
      activity: "applying",
      error: null,
    }));
    try {
      const result = await client.applyRelocateLink(preview.planToken);
      const [snapshot, nextDetail, nextAgents] = await Promise.all([
        client.listSkills(filter),
        client.inspectSkill(selectedId),
        client.listAgents(selectedId),
      ]);
      setSkills(snapshot.items);
      setDetail(nextDetail);
      setAgents(nextAgents);
      setAgentsReadyForSkillId(selectedId);
      setRelocatePanel((state) => ({
        ...state,
        activity: "idle",
        preview: null,
        result,
      }));
    } catch (reason) {
      setRelocatePanel((state) => ({
        ...state,
        activity: "idle",
        error: readError(reason),
      }));
    }
  }

  async function openAdopt() {
    adoptRunId.current += 1;
    setIsAdoptOpen(true);
    setAdoptReport(null);
    setAdoptSelected([]);
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    const runId = adoptRunId.current;
    setAdoptActivity("scanning");
    try {
      const report = await client.scanAdopt();
      if (runId !== adoptRunId.current) return;
      setAdoptReport(report);
      setAdoptSelected(
        report.candidates
          .filter(
            (candidate) => candidate.adoptable && candidate.risk === "none",
          )
          .map((candidate) => candidate.canonicalEntity),
      );
    } catch (reason) {
      if (runId === adoptRunId.current) setAdoptError(readError(reason));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  function toggleAdoptCandidate(canonicalEntity: string, checked: boolean) {
    setAdoptSelected((selected) =>
      checked
        ? [...selected, canonicalEntity]
        : selected.filter((entity) => entity !== canonicalEntity),
    );
  }

  async function planAdopt() {
    if (adoptSelected.length === 0) return;
    const runId = ++adoptRunId.current;
    setAdoptActivity("planning");
    setAdoptError(null);
    try {
      const plan = await client.planAdopt(
        adoptSelected.map((canonicalEntity) => ({
          canonicalEntity,
          agentIds: [],
        })),
      );
      if (runId !== adoptRunId.current) {
        await client.cancelAdopt(plan.planToken).catch(() => undefined);
        return;
      }
      setAdoptPlan(plan);
    } catch (reason) {
      if (runId === adoptRunId.current) setAdoptError(readError(reason));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  async function applyAdopt() {
    if (!adoptPlan?.canApply) return;
    const runId = ++adoptRunId.current;
    setAdoptActivity("applying");
    setAdoptError(null);
    try {
      const result = await client.applyAdopt(adoptPlan.planToken);
      const snapshot = await client.listSkills(filter);
      setSkills(snapshot.items);
      setAdoptPlan(null);
      setAdoptResult(result);
    } catch (reason) {
      setAdoptPlan(null);
      setAdoptError(readError(reason));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  async function undoAdopt() {
    if (!adoptResult?.operationId) return;
    const runId = ++adoptRunId.current;
    setAdoptActivity("undoing");
    setAdoptError(null);
    try {
      const undo = await client.undoAdopt(adoptResult.operationId);
      const snapshot = await client.listSkills(filter);
      setSkills(snapshot.items);
      setAdoptUndo(undo);
    } catch (reason) {
      setAdoptError(readError(reason));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  async function closeAdopt() {
    if (adoptActivity === "applying" || adoptActivity === "undoing") return;
    adoptRunId.current += 1;
    const planToken = adoptPlan?.planToken;
    const operationId =
      adoptResult?.operationId && adoptResult.undoAvailable && !adoptUndo
        ? adoptResult.operationId
        : null;
    setIsAdoptOpen(false);
    setAdoptReport(null);
    setAdoptSelected([]);
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    setAdoptActivity("idle");
    if (planToken) {
      await client.cancelAdopt(planToken).catch(() => undefined);
    }
    if (operationId) {
      await client.finalizeAdopt(operationId).catch(() => undefined);
    }
  }

  // -- Preferences (spec §10.2, strictly four) --

  async function togglePreference(updates: PreferenceUpdates) {
    setPreferencesError(null);
    setPreferencesWarning(null);
    try {
      const result = await client.updatePreferences(updates);
      setPreferences(result.preferences);
      setPreferencesWarning(result.warning);
    } catch (reason) {
      setPreferencesError(readError(reason));
    }
  }

  // -- First-run onboarding (spec §8.7, three skippable steps) --

  async function completeOnboarding() {
    setOnboardingError(null);
    try {
      await client.completeOnboarding();
      setIsOnboardingOpen(false);
      setOnboardingStep(0);
      setOnboardingReport(null);
    } catch (reason) {
      setOnboardingError(readError(reason));
    }
  }

  async function advanceOnboarding() {
    if (onboardingStep === 1) {
      // Step 3: the first-run full scan is read-only and never adopts.
      setOnboardingActivity("scanning");
      setOnboardingError(null);
      try {
        const report = await client.scanAdopt();
        setOnboardingReport(report);
        setOnboardingStep(2);
      } catch (reason) {
        setOnboardingError(readError(reason));
      } finally {
        setOnboardingActivity("idle");
      }
      return;
    }
    setOnboardingStep((step) => Math.min(step + 1, 2));
  }

  async function createOnboardingAgentDirectory(agentId: string) {
    setOnboardingError(null);
    try {
      const info = await client.createAgentDirectory(agentId);
      setStartupAgents(info.agents);
    } catch (reason) {
      setOnboardingError(readError(reason));
    }
  }

  async function finishOnboardingWithAdopt() {
    if (!onboardingReport) return;
    const report = onboardingReport;
    setOnboardingError(null);
    try {
      await client.completeOnboarding();
    } catch (reason) {
      setOnboardingError(readError(reason));
      return;
    }
    setIsOnboardingOpen(false);
    setOnboardingStep(0);
    // Guide into Adopt with the scan results already loaded (spec §8.7).
    setAdoptReport(report);
    setAdoptSelected(
      report.candidates
        .filter((candidate) => candidate.adoptable && candidate.risk === "none")
        .map((candidate) => candidate.canonicalEntity),
    );
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    setAdoptActivity("idle");
    setIsAdoptOpen(true);
    setOnboardingReport(null);
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
      activationConflictMessage={activationConflictMessage}
      replacePreview={replacePreview}
      replaceResult={replaceResult}
      replaceUndo={replaceUndo}
      replaceError={replaceError}
      isApplyingReplace={isApplyingReplace}
      isUndoingReplace={isUndoingReplace}
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
      onCloseActivationConflict={closeActivationConflict}
      onAdoptFromConflict={adoptFromConflict}
      onPlanReplace={planReplace}
      onApplyReplace={applyReplace}
      onUndoReplace={undoReplace}
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
      relocatePanel={relocatePanel}
      onOpenRelocate={openRelocate}
      onCloseRelocate={closeRelocate}
      onRelocateSourcePathChange={(sourcePath) =>
        setRelocatePanel((state) => ({ ...state, sourcePath }))
      }
      onPreviewRelocate={previewRelocate}
      onApplyRelocate={applyRelocate}
      isAdoptOpen={isAdoptOpen}
      adoptReport={adoptReport}
      adoptSelected={adoptSelected}
      adoptPlan={adoptPlan}
      adoptResult={adoptResult}
      adoptUndo={adoptUndo}
      adoptError={adoptError}
      adoptActivity={adoptActivity}
      onOpenAdopt={openAdopt}
      onToggleAdoptCandidate={toggleAdoptCandidate}
      onPlanAdopt={planAdopt}
      onApplyAdopt={applyAdopt}
      onUndoAdopt={undoAdopt}
      onCloseAdopt={closeAdopt}
      isPreferencesOpen={isPreferencesOpen}
      preferences={preferences}
      preferencesWarning={preferencesWarning}
      preferencesError={preferencesError}
      isOnboardingOpen={isOnboardingOpen}
      onboardingStep={onboardingStep}
      onboardingAgents={startupAgents}
      onboardingReport={onboardingReport}
      onboardingActivity={onboardingActivity}
      onboardingError={onboardingError}
      onOpenPreferences={() => setIsPreferencesOpen(true)}
      onClosePreferences={() => setIsPreferencesOpen(false)}
      onTogglePreference={togglePreference}
      onCompleteOnboarding={completeOnboarding}
      onAdvanceOnboarding={advanceOnboarding}
      onCreateAgentDirectory={createOnboardingAgentDirectory}
      onFinishOnboardingWithAdopt={finishOnboardingWithAdopt}
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
