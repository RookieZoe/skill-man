import { useEffect, useRef, useState } from "react";

import { listen } from "@tauri-apps/api/event";

import { LibraryDesk } from "../features/library/LibraryDesk";
import {
  OperationStatusWindow,
  type OperationStatus,
} from "../ui/OperationStatusWindow";
import {
  useLocale,
  type LocaleContextValue,
} from "../features/locale/LocaleProvider";
import {
  errorMessageKey,
  errorMessageParams,
  type MessageKey,
} from "../features/locale/messages";
import type {
  ActivationConflictDetails,
  ActivationPreview,
  ActivationReplacePreview,
  ActivationReplaceUndoResult,
  ActivationResult,
  AdoptEvidenceReport,
  AdoptPlan,
  AdoptResult,
  AdoptSelection,
  AdoptUndoResult,
  ModifiedBranch,
  AgentActivation,
  AppPreferences,
  AvailableAppUpdate,
  CatalogClient,
  CatalogFilter,
  GitImportDiscovery,
  GitImportSelectionPreview,
  GitImportSelectionResult,
  GitSourceCapabilityReport,
  LinkImportPreview,
  LinkImportResult,
  PreferenceUpdates,
  PreferencesWarning,
  RelocateLinkPreview,
  RelocateLinkResult,
  RemoveSkillPreview,
  RemoveSkillResult,
  SkillDetail,
  SkillSummary,
  StartupAgent,
} from "./catalog-client";

export interface AppProps {
  client: CatalogClient;
}

export type ImportKind = "link" | "git";

export interface RelocatePanelState {
  isOpen: boolean;
  activity: "idle" | "previewing" | "applying";
  sourcePath: string;
  preview: RelocateLinkPreview | null;
  result: RelocateLinkResult | null;
  error: string | null;
}

export interface RemovePanelState {
  isOpen: boolean;
  activity: "idle" | "planning" | "applying";
  preview: RemoveSkillPreview | null;
  result: RemoveSkillResult | null;
  error: string | null;
}

export interface AppUpdatePanelState {
  activity:
    | "idle"
    | "checking"
    | "available"
    | "downloading"
    | "ready"
    | "cancelling"
    | "installing";
  update: AvailableAppUpdate | null;
  checkStatus: "up_to_date" | "skipped" | null;
  error: string | null;
}

type OperationCopy = Readonly<{
  title: MessageKey;
  detail: MessageKey;
}>;

const ADOPT_OPERATION_COPIES = {
  scanning: {
    title: "library.adopt.rescanning",
    detail: "operation.detail.adopt.scan",
  },
  planning: {
    title: "library.adopt.planning",
    detail: "operation.detail.adopt.plan",
  },
  applying: {
    title: "library.adopt.adopting",
    detail: "operation.detail.adopt.apply",
  },
  undoing: {
    title: "library.adopt.undoing",
    detail: "operation.detail.adopt.undo",
  },
} as const satisfies Record<string, OperationCopy>;

const LINK_IMPORT_OPERATION_COPIES = {
  discovering: {
    title: "library.import.checking_source",
    detail: "operation.detail.import.link.discover",
  },
  applying: {
    title: "library.import.importing",
    detail: "operation.detail.import.link.apply",
  },
} as const satisfies Record<string, OperationCopy>;

const GIT_IMPORT_OPERATION_COPIES = {
  discovering: {
    title: "library.import.fetching",
    detail: "operation.detail.import.git.discover",
  },
  planning: {
    title: "library.adopt.planning",
    detail: "operation.detail.import.git.plan",
  },
  applying: {
    title: "library.import.importing",
    detail: "operation.detail.import.git.apply",
  },
} as const satisfies Record<string, OperationCopy>;

const RELOCATE_OPERATION_COPIES = {
  previewing: {
    title: "library.relocate.checking",
    detail: "operation.detail.relocate.check",
  },
  applying: {
    title: "library.relocate.relocating",
    detail: "operation.detail.relocate.apply",
  },
} as const satisfies Record<string, OperationCopy>;

const REMOVE_OPERATION_COPIES = {
  planning: {
    title: "library.adopt.planning",
    detail: "operation.detail.remove.plan",
  },
  applying: {
    title: "library.remove.removing",
    detail: "operation.detail.remove.apply",
  },
} as const satisfies Record<string, OperationCopy>;

const APP_UPDATE_OPERATION_COPIES = {
  checking: {
    title: "library.preferences.checking",
    detail: "operation.detail.app_update.check",
  },
  downloading: {
    title: "library.app_update.downloading",
    detail: "operation.detail.app_update.download",
  },
  cancelling: {
    title: "library.app_update.cancelling",
    detail: "operation.detail.app_update.cancel",
  },
  installing: {
    title: "library.app_update.installing",
    detail: "operation.detail.app_update.install",
  },
} as const satisfies Record<string, OperationCopy>;

function operationFromActivity(
  id: string,
  activity: string,
  copies: Readonly<Record<string, OperationCopy>>,
  t: LocaleContextValue["t"],
): OperationStatus | null {
  const copy = copies[activity];
  return copy ? { id, title: t(copy.title), detail: t(copy.detail) } : null;
}

function isOperationStatus(
  operation: OperationStatus | null,
): operation is OperationStatus {
  return operation !== null;
}

export function App({ client }: AppProps) {
  const { t, tPlural } = useLocale();
  // Effects only render errors via `t`; a locale switch must not re-run
  // catalog/health effects, so the current `t` is mirrored into a ref.
  const tRef = useRef(t);
  tRef.current = t;
  const [filter, setFilter] = useState<CatalogFilter>("all");
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [gitSourceCapability, setGitSourceCapability] =
    useState<GitSourceCapabilityReport | null>(null);
  const [gitSourceCapabilityFailure, setGitSourceCapabilityFailure] = useState<{
    diagnostic: string | null;
  } | null>(null);
  const [libraryLoaded, setLibraryLoaded] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [agents, setAgents] = useState<AgentActivation[]>([]);
  const [agentsReadyForSkillId, setAgentsReadyForSkillId] = useState<
    string | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [activationError, setActivationError] = useState<string | null>(null);
  const [preferences, setPreferences] = useState<AppPreferences | null>(null);
  const [preferencesWarning, setPreferencesWarning] =
    useState<PreferencesWarning | null>(null);
  const [preferencesError, setPreferencesError] = useState<string | null>(null);
  const [isPreferencesOpen, setIsPreferencesOpen] = useState(false);
  const [appUpdatePanel, setAppUpdatePanel] = useState<AppUpdatePanelState>({
    activity: "idle",
    update: null,
    checkStatus: null,
    error: null,
  });
  const appUpdateCheckRunId = useRef(0);
  const [startupAgents, setStartupAgents] = useState<StartupAgent[]>([]);
  const [onboardingLibraryPath, setOnboardingLibraryPath] = useState<
    string | null
  >(null);
  const [isOnboardingOpen, setIsOnboardingOpen] = useState(false);
  const [onboardingStep, setOnboardingStep] = useState(0);
  const [onboardingReport, setOnboardingReport] =
    useState<AdoptEvidenceReport | null>(null);
  const [onboardingActivity, setOnboardingActivity] = useState<
    "idle" | "checking" | "scanning"
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
  const [relocatePanel, setRelocatePanel] = useState<RelocatePanelState>({
    isOpen: false,
    activity: "idle",
    sourcePath: "",
    preview: null,
    result: null,
    error: null,
  });
  const [removePanel, setRemovePanel] = useState<RemovePanelState>({
    isOpen: false,
    activity: "idle",
    preview: null,
    result: null,
    error: null,
  });
  const [lockNotice, setLockNotice] = useState<string | null>(null);
  const [isAdoptOpen, setIsAdoptOpen] = useState(false);
  const [adoptReport, setAdoptReport] = useState<AdoptEvidenceReport | null>(
    null,
  );
  const [adoptSelections, setAdoptSelections] = useState<
    Record<string, AdoptSelection>
  >({});
  const [adoptPlan, setAdoptPlan] = useState<AdoptPlan | null>(null);
  const [adoptResult, setAdoptResult] = useState<AdoptResult | null>(null);
  const [adoptUndo, setAdoptUndo] = useState<AdoptUndoResult | null>(null);
  const [adoptError, setAdoptError] = useState<string | null>(null);
  const [adoptErrorHeading, setAdoptErrorHeading] = useState<MessageKey>(
    "app.notice.scan_failed",
  );
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
      .catch((reason) => {
        // §10.4: a recovery_required startup is a read-only lock; browsing
        // stays available but every write is refused until recovery runs.
        const failure = readCommandError(reason, tRef.current);
        if (current && failure.code === "recovery_required") {
          setLockNotice(failure.message);
        }
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
        setOnboardingLibraryPath(info.libraryPath ?? null);
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
    if (preferences?.checkAppUpdates !== true) return;
    let current = true;
    const runId = ++appUpdateCheckRunId.current;
    client
      .checkAppUpdate(false)
      .then((result) => {
        if (
          current &&
          runId === appUpdateCheckRunId.current &&
          result.status === "available"
        ) {
          setAppUpdatePanel({
            activity: "available",
            update: result,
            checkStatus: null,
            error: null,
          });
        }
      })
      .catch(() => {
        // Background and offline checks are intentionally silent.
      });
    return () => {
      current = false;
    };
  }, [client, preferences?.checkAppUpdates]);

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
        if (current) setError(readError(reason, tRef.current));
      })
      .finally(() => {
        if (current) setLibraryLoaded(true);
      });
    return () => {
      current = false;
    };
  }, [client, filter]);

  useEffect(() => {
    let current = true;
    client
      .getGitSourceCapability()
      .then((report) => {
        if (!current) return;
        setGitSourceCapability(report);
        setGitSourceCapabilityFailure(null);
      })
      .catch((reason: unknown) => {
        // A Source Capability Scan failure never closes normal browsing, but
        // it must remain visible with its diagnostic collapsed by default.
        if (!current) return;
        setGitSourceCapability(null);
        setGitSourceCapabilityFailure({
          diagnostic: readDiagnostic(reason, tRef.current),
        });
      });
    return () => {
      current = false;
    };
  }, [client]);

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
        if (current) setError(readError(reason, tRef.current));
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
        setError(readError(reason, tRef.current));
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
    const failure = readCommandError(reason, t);
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
      setActivationError(readError(reason, t));
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
      const failure = readCommandError(reason, t);
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
   *  flow; the ledger opens with nothing selected (viewing is never
   *  selecting). */
  async function adoptFromConflict() {
    setActivationConflict(null);
    setActivationConflictMessage(null);
    setReplacePreview(null);
    setReplaceOperationId(null);
    setReplaceResult(null);
    setReplaceUndo(null);
    setReplaceError(null);
    setIsAdoptOpen(true);
    setAdoptReport(null);
    setAdoptSelections({});
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    setAdoptErrorHeading("app.notice.scan_failed");
    const runId = ++adoptRunId.current;
    setAdoptActivity("scanning");
    try {
      const report = await client.scanAdopt();
      if (runId !== adoptRunId.current) return;
      setAdoptReport(report);
      // Viewing is never selecting: the ledger preselects nothing, not even
      // the conflicted Skill (spec §2.1 invariant 9).
    } catch (reason) {
      if (runId === adoptRunId.current) setAdoptError(readError(reason, t));
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
      setReplaceError(readError(reason, t));
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
      setReplaceError(readError(reason, t));
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
      setReplaceError(readError(reason, t));
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
        setActivationError(readError(reason, t));
      });
    }
    if (operationId) {
      client.finalizeActivationReplace(operationId).catch((reason) => {
        setActivationError(readError(reason, t));
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
        setLinkImportError(readError(reason, t));
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
      setLinkImportError(readError(reason, t));
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
        setGitImportError(readError(reason, t));
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
        setGitImportError(readError(reason, t));
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
      setGitImportError(readError(reason, t));
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
        error: readError(reason, t),
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
        error: readError(reason, t),
      }));
    }
  }

  function openRemove() {
    if (!selectedId) return;
    setRemovePanel({
      isOpen: true,
      activity: "planning",
      preview: null,
      result: null,
      error: null,
    });
    const skillId = selectedId;
    client
      .planRemoveSkill(skillId)
      .then((preview) => {
        setRemovePanel((state) =>
          state.isOpen ? { ...state, activity: "idle", preview } : state,
        );
      })
      .catch((reason) => {
        setRemovePanel((state) =>
          state.isOpen
            ? { ...state, activity: "idle", error: readError(reason, t) }
            : state,
        );
      });
  }

  function closeRemove() {
    const { preview, result } = removePanel;
    setRemovePanel({
      isOpen: false,
      activity: "idle",
      preview: null,
      result: null,
      error: null,
    });
    if (!result && preview) {
      void client.cancelRemoveSkill(preview.planToken).catch(() => undefined);
    }
  }

  async function applyRemove() {
    const { preview } = removePanel;
    if (!selectedId || !preview) return;
    setRemovePanel((state) => ({
      ...state,
      activity: "applying",
      error: null,
    }));
    try {
      const result = await client.applyRemoveSkill(preview.planToken);
      const snapshot = await client.listSkills(filter);
      setSkills(snapshot.items);
      setSelectedId(null);
      setDetail(null);
      setAgents([]);
      setAgentsReadyForSkillId(null);
      setRemovePanel((state) => ({
        ...state,
        activity: "idle",
        preview: null,
        result,
      }));
    } catch (reason) {
      setRemovePanel((state) => ({
        ...state,
        activity: "idle",
        error: readError(reason, t),
      }));
    }
  }

  async function retryRecovery() {
    try {
      await client.runActivationHealthCheck();
      setLockNotice(null);
    } catch (reason) {
      setLockNotice(readCommandError(reason, t).message);
    }
  }

  async function openAdopt() {
    adoptRunId.current += 1;
    setIsAdoptOpen(true);
    setAdoptReport(null);
    setAdoptSelections({});
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    setAdoptErrorHeading("app.notice.scan_failed");
    const runId = adoptRunId.current;
    setAdoptActivity("scanning");
    try {
      const report = await client.scanAdopt();
      if (runId !== adoptRunId.current) return;
      setAdoptReport(report);
    } catch (reason) {
      if (runId === adoptRunId.current) setAdoptError(readError(reason, t));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  function toggleAdoptCandidate(canonicalEntity: string, checked: boolean) {
    setAdoptSelections((selections) => {
      const next = { ...selections };
      if (checked) {
        const existing = next[canonicalEntity];
        next[canonicalEntity] = {
          canonicalEntity,
          agentIds: existing?.agentIds ?? [],
          // The recommendation is to keep the current bytes; the branch
          // choice never replaces the user's Include action (spec §8.2).
          modifiedBranch: existing?.modifiedBranch ?? "keep_current",
        };
      } else {
        delete next[canonicalEntity];
      }
      return next;
    });
  }

  function setAdoptModifiedBranch(
    canonicalEntity: string,
    modifiedBranch: ModifiedBranch,
  ) {
    setAdoptSelections((selections) => {
      const existing = selections[canonicalEntity];
      if (!existing) return selections;
      return {
        ...selections,
        [canonicalEntity]: { ...existing, modifiedBranch },
      };
    });
  }

  async function rescanAdopt() {
    const runId = ++adoptRunId.current;
    setAdoptPlan(null);
    setAdoptError(null);
    setAdoptErrorHeading("app.notice.scan_failed");
    setAdoptActivity("scanning");
    try {
      const report = await client.scanAdopt();
      if (runId !== adoptRunId.current) return;
      setAdoptReport(report);
      setAdoptSelections({});
    } catch (reason) {
      if (runId === adoptRunId.current) setAdoptError(readError(reason, t));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  async function planAdopt() {
    const selections = Object.values(adoptSelections);
    if (selections.length === 0) return;
    const runId = ++adoptRunId.current;
    setAdoptActivity("planning");
    setAdoptError(null);
    setAdoptErrorHeading("app.notice.preview_failed");
    try {
      const plan = await client.planAdopt(
        adoptReport?.generation ?? 0,
        selections,
      );
      if (runId !== adoptRunId.current) {
        await client.cancelAdopt(plan.planToken).catch(() => undefined);
        return;
      }
      setAdoptPlan(plan);
    } catch (reason) {
      if (runId === adoptRunId.current) setAdoptError(readError(reason, t));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  async function applyAdopt() {
    if (!adoptPlan?.canApply) return;
    const runId = ++adoptRunId.current;
    setAdoptActivity("applying");
    setAdoptError(null);
    setAdoptErrorHeading("app.notice.adopt_failed");
    try {
      const result = await client.applyAdopt(adoptPlan.planToken);
      setAdoptPlan(null);
      setAdoptResult(result);
      try {
        const snapshot = await client.listSkills(filter);
        setSkills(snapshot.items);
      } catch (reason) {
        setAdoptErrorHeading("app.notice.refresh_failed");
        setAdoptError(
          t("app.notice.adopt_refresh_failed", {
            detail: readError(reason, t),
          }),
        );
      }
    } catch (reason) {
      setAdoptPlan(null);
      setAdoptError(readError(reason, t));
    } finally {
      if (runId === adoptRunId.current) setAdoptActivity("idle");
    }
  }

  async function undoAdopt() {
    if (!adoptResult?.operationId) return;
    const runId = ++adoptRunId.current;
    setAdoptActivity("undoing");
    setAdoptError(null);
    setAdoptErrorHeading("app.notice.undo_failed");
    try {
      const undo = await client.undoAdopt(adoptResult.operationId);
      setAdoptUndo(undo);
      try {
        const snapshot = await client.listSkills(filter);
        setSkills(snapshot.items);
      } catch (reason) {
        setAdoptErrorHeading("app.notice.refresh_failed");
        setAdoptError(
          t("app.notice.undo_refresh_failed", {
            detail: readError(reason, t),
          }),
        );
      }
    } catch (reason) {
      setAdoptError(readError(reason, t));
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
    setAdoptSelections({});
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
      setPreferencesError(readError(reason, t));
    }
  }

  async function checkAppUpdate() {
    const runId = ++appUpdateCheckRunId.current;
    setAppUpdatePanel({
      activity: "checking",
      update: null,
      checkStatus: null,
      error: null,
    });
    try {
      const result = await client.checkAppUpdate(true);
      if (runId !== appUpdateCheckRunId.current) return;
      if (result.status === "available") {
        setAppUpdatePanel({
          activity: "available",
          update: result,
          checkStatus: null,
          error: null,
        });
        setIsPreferencesOpen(false);
      } else {
        setAppUpdatePanel({
          activity: "idle",
          update: null,
          checkStatus: result.status,
          error: null,
        });
      }
    } catch (reason) {
      if (runId !== appUpdateCheckRunId.current) return;
      setAppUpdatePanel({
        activity: "idle",
        update: null,
        checkStatus: null,
        error: readAppUpdateError(reason, t),
      });
    }
  }

  async function downloadAppUpdate() {
    const update = appUpdatePanel.update;
    if (!update || appUpdatePanel.activity !== "available") return;
    setAppUpdatePanel((state) => ({
      ...state,
      activity: "downloading",
      error: null,
    }));
    try {
      await client.downloadAppUpdate(update.updateId);
      setAppUpdatePanel((state) =>
        state.update?.updateId === update.updateId
          ? { ...state, activity: "ready", error: null }
          : state,
      );
    } catch (reason) {
      setAppUpdatePanel((state) =>
        state.update?.updateId === update.updateId
          ? readCommandError(reason, t).code === "update_cancelled" ||
            state.activity === "cancelling"
            ? state
            : {
                ...state,
                activity: "available",
                error: readAppUpdateError(reason, t),
              }
          : state,
      );
    }
  }

  async function installAppUpdate() {
    const update = appUpdatePanel.update;
    if (!update || appUpdatePanel.activity !== "ready") return;
    setAppUpdatePanel((state) => ({
      ...state,
      activity: "installing",
      error: null,
    }));
    try {
      await client.installAppUpdate(update.updateId);
      setAppUpdatePanel({
        activity: "idle",
        update: null,
        checkStatus: null,
        error: null,
      });
    } catch (reason) {
      setAppUpdatePanel((state) =>
        state.update?.updateId === update.updateId
          ? {
              ...state,
              activity: "ready",
              error: readAppUpdateError(reason, t),
            }
          : state,
      );
    }
  }

  async function closeAppUpdate() {
    const update = appUpdatePanel.update;
    if (
      !update ||
      appUpdatePanel.activity === "cancelling" ||
      appUpdatePanel.activity === "installing"
    )
      return;
    const previousActivity = appUpdatePanel.activity;
    setAppUpdatePanel((state) => ({
      ...state,
      activity: "cancelling",
      error: null,
    }));
    try {
      await client.cancelAppUpdate(update.updateId);
      setAppUpdatePanel((state) =>
        state.update?.updateId === update.updateId
          ? {
              activity: "idle",
              update: null,
              checkStatus: null,
              error: null,
            }
          : state,
      );
    } catch (reason) {
      setAppUpdatePanel((state) =>
        state.update?.updateId === update.updateId
          ? {
              ...state,
              activity: previousActivity,
              error: readAppUpdateError(reason, t),
            }
          : state,
      );
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
      setOnboardingError(readError(reason, t));
    }
  }

  async function advanceOnboarding() {
    if (onboardingStep === 0) {
      setOnboardingActivity("checking");
      setOnboardingError(null);
      setOnboardingStep(1);
      try {
        const info = await client.startupInfo();
        setStartupAgents(info.agents);
        setOnboardingLibraryPath(info.libraryPath ?? null);
      } catch (reason) {
        setOnboardingStep(0);
        setOnboardingError(readError(reason, t));
      } finally {
        setOnboardingActivity("idle");
      }
      return;
    }
    if (onboardingStep === 1) {
      // Step 3: the first-run full scan is read-only and never adopts.
      setOnboardingActivity("scanning");
      setOnboardingError(null);
      setOnboardingStep(2);
      try {
        const report = await client.scanAdopt();
        setOnboardingReport(report);
      } catch (reason) {
        setOnboardingStep(1);
        setOnboardingError(readError(reason, t));
      } finally {
        setOnboardingActivity("idle");
      }
      return;
    }
    setOnboardingStep((step) => Math.min(step + 1, 2));
  }

  async function createOnboardingAgentDirectory(agentId: string) {
    setOnboardingActivity("checking");
    setOnboardingError(null);
    try {
      const info = await client.createAgentDirectory(agentId);
      setStartupAgents(info.agents);
      setOnboardingLibraryPath(info.libraryPath ?? null);
    } catch (reason) {
      setOnboardingError(readError(reason, t));
    } finally {
      setOnboardingActivity("idle");
    }
  }

  async function finishOnboardingWithAdopt() {
    if (!onboardingReport) return;
    const report = onboardingReport;
    setOnboardingError(null);
    try {
      await client.completeOnboarding();
    } catch (reason) {
      setOnboardingError(readError(reason, t));
      return;
    }
    setIsOnboardingOpen(false);
    setOnboardingStep(0);
    // Guide into Adopt with the scan results already loaded (spec §8.7);
    // viewing is never selecting, so nothing is pre-included.
    setAdoptReport(report);
    setAdoptSelections({});
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    setAdoptActivity("idle");
    setIsAdoptOpen(true);
    setOnboardingReport(null);
  }

  const activeOperations = [
    operationFromActivity("adopt", adoptActivity, ADOPT_OPERATION_COPIES, t),
    operationFromActivity(
      "link-import",
      linkImportActivity,
      LINK_IMPORT_OPERATION_COPIES,
      t,
    ),
    operationFromActivity(
      "git-import",
      gitImportActivity,
      GIT_IMPORT_OPERATION_COPIES,
      t,
    ),
    isPreferencesOpen
      ? operationFromActivity(
          "app-update",
          appUpdatePanel.activity,
          APP_UPDATE_OPERATION_COPIES,
          t,
        )
      : null,
    relocatePanel.isOpen
      ? operationFromActivity(
          "relocate",
          relocatePanel.activity,
          RELOCATE_OPERATION_COPIES,
          t,
        )
      : null,
    removePanel.isOpen
      ? operationFromActivity(
          "remove",
          removePanel.activity,
          REMOVE_OPERATION_COPIES,
          t,
        )
      : null,
    isApplyingActivation
      ? {
          id: "activation",
          title: t("library.activation_preview.applying"),
          detail: t("operation.detail.activation.apply"),
        }
      : null,
    isApplyingReplace
      ? {
          id: "activation-replace",
          title: t("library.activation_preview.applying"),
          detail: t("operation.detail.activation.replace"),
        }
      : null,
    isUndoingReplace
      ? {
          id: "activation-replace-undo",
          title: t("library.adopt.undoing"),
          detail: t("operation.detail.activation.undo"),
        }
      : null,
  ].filter(isOperationStatus);

  return (
    <>
      <LibraryDesk
        filter={filter}
        skills={skills}
        libraryEmpty={libraryLoaded && skills.length === 0}
        selectedId={selectedId}
        detail={detail}
        agents={agents}
        error={error}
        gitSourceCapability={gitSourceCapability}
        gitSourceCapabilityFailure={gitSourceCapabilityFailure}
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
        relocatePanel={relocatePanel}
        onOpenRelocate={openRelocate}
        onCloseRelocate={closeRelocate}
        onRelocateSourcePathChange={(sourcePath) =>
          setRelocatePanel((state) => ({ ...state, sourcePath }))
        }
        onPreviewRelocate={previewRelocate}
        onApplyRelocate={applyRelocate}
        removePanel={removePanel}
        onOpenRemove={openRemove}
        onCloseRemove={closeRemove}
        onApplyRemove={applyRemove}
        lockNotice={lockNotice}
        onRetryRecovery={retryRecovery}
        isAdoptOpen={isAdoptOpen}
        adoptReport={adoptReport}
        adoptSelections={adoptSelections}
        adoptPlan={adoptPlan}
        adoptResult={adoptResult}
        adoptUndo={adoptUndo}
        adoptError={adoptError}
        adoptErrorHeading={adoptErrorHeading}
        adoptActivity={adoptActivity}
        onOpenAdopt={openAdopt}
        onRescanAdopt={rescanAdopt}
        onToggleAdoptCandidate={toggleAdoptCandidate}
        onSetAdoptBranch={setAdoptModifiedBranch}
        onPlanAdopt={planAdopt}
        onApplyAdopt={applyAdopt}
        onUndoAdopt={undoAdopt}
        onCloseAdopt={closeAdopt}
        isPreferencesOpen={isPreferencesOpen}
        preferences={preferences}
        preferencesWarning={preferencesWarning}
        preferencesError={preferencesError}
        appUpdatePanel={appUpdatePanel}
        isOnboardingOpen={isOnboardingOpen}
        onboardingStep={onboardingStep}
        onboardingAgents={startupAgents}
        onboardingLibraryPath={onboardingLibraryPath}
        onboardingReport={onboardingReport}
        onboardingActivity={onboardingActivity}
        onboardingError={onboardingError}
        onOpenPreferences={() => setIsPreferencesOpen(true)}
        onClosePreferences={() => setIsPreferencesOpen(false)}
        onTogglePreference={togglePreference}
        onCheckAppUpdate={checkAppUpdate}
        onDownloadAppUpdate={downloadAppUpdate}
        onInstallAppUpdate={installAppUpdate}
        onCloseAppUpdate={closeAppUpdate}
        onCompleteOnboarding={completeOnboarding}
        onAdvanceOnboarding={advanceOnboarding}
        onCreateAgentDirectory={createOnboardingAgentDirectory}
        onFinishOnboardingWithAdopt={finishOnboardingWithAdopt}
      />
      {activeOperations.length > 0 ? (
        <OperationStatusWindow
          ariaLabel={t("operation.status.label")}
          heading={tPlural(
            "operation.status.running",
            activeOperations.length,
            {
              count: activeOperations.length,
            },
          )}
          operations={activeOperations}
        />
      ) : null}
    </>
  );
}

function readError(reason: unknown, t: LocaleContextValue["t"]) {
  if (reason instanceof Error) return reason.message;
  if (
    typeof reason === "object" &&
    reason !== null &&
    "message" in reason &&
    typeof reason.message === "string"
  ) {
    return reason.message;
  }
  // Typed command failures (spec §4.7): no free message crosses the DTO, so
  // presentation composes the localized summary from the closed code.
  if (
    typeof reason === "object" &&
    reason !== null &&
    "error" in reason &&
    typeof reason.error === "object" &&
    reason.error !== null &&
    "code" in reason.error &&
    typeof reason.error.code === "string"
  ) {
    const error = reason.error as { code: string; directoryName?: string };
    return t(errorMessageKey(error.code), errorMessageParams(error));
  }
  if (
    typeof reason === "object" &&
    reason !== null &&
    "code" in reason &&
    typeof reason.code === "string"
  ) {
    return t(errorMessageKey(reason.code as string));
  }
  return t("app.error.read_failed");
}

function readDiagnostic(
  reason: unknown,
  t: LocaleContextValue["t"],
): string | null {
  if (typeof reason !== "object" || reason === null) return null;
  const diagnostic = "diagnostic" in reason ? reason.diagnostic : null;
  if (
    typeof diagnostic !== "object" ||
    diagnostic === null ||
    !("code" in diagnostic) ||
    !("message" in diagnostic) ||
    typeof diagnostic.code !== "string" ||
    typeof diagnostic.message !== "string"
  ) {
    return null;
  }
  return t("library.source_capability.diagnostic_value", {
    code: diagnostic.code,
    message: diagnostic.message,
  });
}

function readCommandError(reason: unknown, t: LocaleContextValue["t"]) {
  return {
    code:
      typeof reason === "object" &&
      reason !== null &&
      "code" in reason &&
      typeof reason.code === "string"
        ? reason.code
        : "internal",
    message: readError(reason, t),
  };
}

function readAppUpdateError(reason: unknown, t: LocaleContextValue["t"]) {
  const { code } = readCommandError(reason, t);
  switch (code) {
    case "source_unavailable":
      return t("app.update_error.source_unavailable");
    case "state_unavailable":
      return t("app.update_error.state_unavailable");
    case "stale_update":
      return t("app.update_error.stale_update");
    case "download_failed":
      return t("app.update_error.download_failed");
    case "install_failed":
      return t("app.update_error.install_failed");
    case "update_cancelled":
      return t("app.update_error.cancelled");
    default:
      return t("app.update_error.generic");
  }
}
