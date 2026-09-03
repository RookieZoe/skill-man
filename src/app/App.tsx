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
  AdoptEvidenceReport,
  AdoptGitSource,
  AdoptPlan,
  AdoptResult,
  AdoptSelection,
  AdoptUndoResult,
  ModifiedBranch,
  AppPreferences,
  AvailableAppUpdate,
  CatalogClient,
  CatalogFilter,
  FetchLatestAndManageRequest,
  GitRepositorySourceType,
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
  SourceGroupPreviewOutcome,
  SourcePromotionDraft,
  SourcePromotionDraftOutcome,
  SourceTrackingPolicy,
  SourceUpdateDraft,
  SourcePromotionResult,
  SourceTransitionResult,
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

const SOURCE_GROUP_OPERATION_COPIES = {
  fetching: {
    title: "library.source_group.fetching",
    detail: "operation.detail.import.git.discover",
  },
  confirming: {
    title: "library.source_group.confirming",
    detail: "operation.detail.import.git.apply",
  },
  undoing: {
    title: "library.source_group.undoing",
    detail: "operation.detail.import.git.undo",
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
  const [error, setError] = useState<string | null>(null);
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
  const [isLinkImportOpen, setIsLinkImportOpen] = useState(false);
  const [importKind, setImportKind] = useState<ImportKind>("link");
  const [sourceGroupType, setSourceGroupType] =
    useState<GitRepositorySourceType>("github");
  const [sourceGroupUrl, setSourceGroupUrl] = useState("");
  const [sourceGroupPolicyMode, setSourceGroupPolicyMode] = useState(
    "auto_release_tag_head",
  );
  const [sourceGroupPolicyValue, setSourceGroupPolicyValue] = useState("");
  const [sourceGroupOutcome, setSourceGroupOutcome] =
    useState<SourceGroupPreviewOutcome | null>(null);
  const [sourceTransitionResult, setSourceTransitionResult] =
    useState<SourceTransitionResult | null>(null);
  const [sourcePromotionRemoteId, setSourcePromotionRemoteId] = useState<
    string | null
  >(null);
  const [sourcePromotionDraft, setSourcePromotionDraft] =
    useState<SourcePromotionDraft | null>(null);
  const [sourcePromotionOutcome, setSourcePromotionOutcome] =
    useState<SourcePromotionDraftOutcome | null>(null);
  const [sourceUpdateDraft, setSourceUpdateDraft] =
    useState<SourceUpdateDraft | null>(null);
  const [sourcePromotionResult, setSourcePromotionResult] =
    useState<SourcePromotionResult | null>(null);
  const [sourceUpdateActive, setSourceUpdateActive] = useState(false);
  const [sourceGroupError, setSourceGroupError] = useState<string | null>(null);
  const [sourceGroupActivity, setSourceGroupActivity] = useState<
    "idle" | "fetching" | "confirming" | "undoing"
  >("idle");
  const sourceGroupRunId = useRef(0);
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
  const adoptSourcePreviewCache = useRef<
    Map<string, Promise<SourceGroupPreviewOutcome>>
  >(new Map());
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

  // -- Activation Conflict: Adopt existing item / Remove then replace / Cancel

  /** Adopt the occupying item instead of replacing it: hand off to the Adopt
   *  flow; the ledger opens with nothing selected (viewing is never
   *  selecting). */

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
    sourceGroupRunId.current += 1;
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setSourceGroupType("github");
    setSourceGroupUrl("");
    setSourceGroupPolicyMode("auto_release_tag_head");
    setSourceGroupPolicyValue("");
    setSourceGroupOutcome(null);
    setSourceTransitionResult(null);
    setSourcePromotionRemoteId(null);
    setSourceUpdateActive(false);
    setSourcePromotionDraft(null);
    setSourcePromotionOutcome(null);
    setSourcePromotionResult(null);
    setSourceGroupError(null);
    setSourceGroupActivity("idle");
    setImportKind("link");
    setIsLinkImportOpen(true);
  }

  function sourceGroupTrackingPolicy(): SourceTrackingPolicy | null {
    const mode = sourceGroupPolicyMode;
    const value = sourceGroupPolicyValue.trim();
    const needsValue =
      mode === "prerelease_channel" ||
      mode === "fixed_tag" ||
      mode === "fixed_commit" ||
      mode === "branch";
    if (!needsValue) return { mode, value: null };
    return value ? { mode, value } : null;
  }

  async function fetchLatestAndManage(
    request: FetchLatestAndManageRequest = {
      sourceType: sourceGroupType,
      sourceUrl: sourceGroupUrl,
      trackingPolicy: sourceGroupTrackingPolicy(),
    },
    preloadedPreview?: Promise<SourceGroupPreviewOutcome>,
  ) {
    const runId = ++sourceGroupRunId.current;
    setSourceGroupActivity("fetching");
    setSourceGroupError(null);
    setSourceGroupOutcome(null);
    setSourceTransitionResult(null);
    setSourcePromotionRemoteId(null);
    setSourcePromotionDraft(null);
    setSourcePromotionOutcome(null);
    setSourcePromotionResult(null);
    try {
      const outcome = preloadedPreview
        ? await preloadedPreview
        : await client.fetchLatestAndManage(request);
      if (runId !== sourceGroupRunId.current) return;
      setSourceGroupOutcome(outcome);
    } catch (reason) {
      if (runId === sourceGroupRunId.current) {
        setSourceGroupError(readError(reason, t));
      }
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
    }
  }

  function adoptSourceRequest(
    source: AdoptGitSource,
  ): FetchLatestAndManageRequest {
    return {
      sourceType: source.sourceType,
      sourceUrl: source.sourceUrl,
      trackingPolicy:
        source.trackingRefs.length === 1
          ? { mode: "branch", value: source.trackingRefs[0] }
          : null,
    };
  }

  function sourcePreviewKey(request: FetchLatestAndManageRequest) {
    return JSON.stringify([
      request.sourceType,
      request.sourceUrl,
      request.trackingPolicy,
    ]);
  }

  function preloadAdoptSourcePreview(request: FetchLatestAndManageRequest) {
    const key = sourcePreviewKey(request);
    const cached = adoptSourcePreviewCache.current.get(key);
    if (cached) return cached;
    const preview = client.fetchLatestAndManage(request);
    adoptSourcePreviewCache.current.set(key, preview);
    void preview.catch(() => {
      // A transient background failure must not turn into a permanently
      // cached preview error. The explicit management action retries it.
      if (adoptSourcePreviewCache.current.get(key) === preview) {
        adoptSourcePreviewCache.current.delete(key);
      }
    });
    return preview;
  }

  function preloadAdoptGitSources(sources: AdoptGitSource[]) {
    for (const source of sources) {
      void preloadAdoptSourcePreview(adoptSourceRequest(source)).catch(
        () => undefined,
      );
    }
  }

  function manageAdoptGitSource(source: AdoptGitSource) {
    if (adoptActivity !== "idle") return;
    const request = adoptSourceRequest(source);
    const preloadedPreview = preloadAdoptSourcePreview(request);
    const trackingPolicy = request.trackingPolicy;

    adoptRunId.current += 1;
    setIsAdoptOpen(false);
    setAdoptPlan(null);
    setAdoptResult(null);
    setAdoptUndo(null);
    setAdoptError(null);
    setAdoptSelections({});

    linkImportRunId.current += 1;
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setImportKind("git");
    setSourceGroupType(source.sourceType);
    setSourceGroupUrl(source.sourceUrl);
    setSourceGroupPolicyMode(trackingPolicy?.mode ?? "auto_release_tag_head");
    setSourceGroupPolicyValue(trackingPolicy?.value ?? "");
    setSourceGroupOutcome(null);
    setSourceTransitionResult(null);
    setSourcePromotionRemoteId(null);
    setSourceUpdateActive(false);
    setSourcePromotionDraft(null);
    setSourcePromotionOutcome(null);
    setSourcePromotionResult(null);
    setSourceGroupError(null);
    setSourceGroupActivity("idle");
    setIsLinkImportOpen(true);

    void fetchLatestAndManage(request, preloadedPreview);
  }

  async function closeImport() {
    if (linkImportActivity === "applying" || sourceGroupActivity !== "idle")
      return;
    if (sourceTransitionResult) {
      try {
        await client.finalizeSourceTransition(
          sourceTransitionResult.operationId,
        );
      } catch (reason) {
        setSourceGroupError(readError(reason, t));
        return;
      }
    }
    if (sourcePromotionResult) {
      try {
        if (sourceUpdateActive) {
          await client.finalizeSourceUpdate(sourcePromotionResult.operationId);
        } else {
          await client.finalizeSourcePromotion(
            sourcePromotionResult.operationId,
          );
        }
      } catch (reason) {
        setSourceGroupError(readError(reason, t));
        return;
      }
    }
    linkImportRunId.current += 1;
    sourceGroupRunId.current += 1;
    const linkPlanToken = linkImportPreview?.planToken;
    setIsLinkImportOpen(false);
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setLinkImportActivity("idle");
    setSourceGroupOutcome(null);
    setSourceTransitionResult(null);
    setSourcePromotionRemoteId(null);
    setSourcePromotionDraft(null);
    setSourcePromotionOutcome(null);
    setSourcePromotionResult(null);
    setSourceGroupError(null);
    setSourceGroupActivity("idle");
    if (linkPlanToken) {
      await client.cancelLinkImport(linkPlanToken).catch(() => undefined);
    }
  }

  async function confirmSourceTransition() {
    if (sourceGroupOutcome?.kind !== "preview") return;
    const preview = sourceGroupOutcome.preview;
    const runId = ++sourceGroupRunId.current;
    setSourceGroupActivity("confirming");
    setSourceGroupError(null);
    try {
      const result = await client.confirmSourceTransition({
        sourceType: sourceGroupType,
        sourceUrl: preview.sourceUrl,
        trackingPolicy: preview.policy.mode
          ? {
              mode: preview.policy.mode,
              value: preview.policy.value,
            }
          : null,
        expectedSelectedRef: preview.policy.selectedRef,
        expectedResolvedCommit: preview.policy.resolvedCommit,
      });
      if (runId !== sourceGroupRunId.current) return;
      const snapshot = await client.listSkills(filter);
      if (runId !== sourceGroupRunId.current) return;
      setSkills(snapshot.items);
      setSourceTransitionResult(result);
    } catch (reason) {
      if (runId === sourceGroupRunId.current) {
        setSourceGroupError(readError(reason, t));
      }
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
    }
  }

  async function previewSourcePromotion(remoteId: string) {
    const runId = ++sourceGroupRunId.current;
    linkImportRunId.current += 1;
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setImportKind("git");
    setSourceGroupOutcome(null);
    setSourceTransitionResult(null);
    setSourcePromotionRemoteId(remoteId);
    setSourceUpdateActive(false);
    setSourcePromotionDraft(null);
    setSourcePromotionOutcome(null);
    setSourcePromotionResult(null);
    setSourceGroupError(null);
    setSourceGroupActivity("fetching");
    setIsLinkImportOpen(true);
    try {
      const outcome = await client.previewSourcePromotion(
        remoteId,
        sourceGroupTrackingPolicy(),
      );
      if (runId === sourceGroupRunId.current) {
        setSourcePromotionDraft(
          outcome.kind === "draft" ? outcome.draft : null,
        );
        setSourcePromotionOutcome(outcome.kind === "draft" ? null : outcome);
      }
    } catch (reason) {
      if (runId === sourceGroupRunId.current) {
        setSourceGroupError(readError(reason, t));
      }
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
    }
  }

  async function previewSourceUpdate(remoteId: string) {
    const runId = ++sourceGroupRunId.current;
    linkImportRunId.current += 1;
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setImportKind("git");
    setSourceGroupOutcome(null);
    setSourceTransitionResult(null);
    setSourcePromotionRemoteId(remoteId);
    setSourceUpdateActive(true);
    setSourcePromotionDraft(null);
    setSourcePromotionOutcome(null);
    setSourcePromotionResult(null);
    setSourceGroupError(null);
    setSourceGroupActivity("fetching");
    setIsLinkImportOpen(true);
    try {
      const draft = await client.previewSourceUpdate(remoteId);
      if (runId === sourceGroupRunId.current) {
        setSourceUpdateDraft(draft);
      }
    } catch (reason) {
      if (runId === sourceGroupRunId.current)
        setSourceGroupError(readError(reason, t));
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
    }
  }

  async function confirmSourcePromotion() {
    if (!sourcePromotionDraft) return;
    const runId = ++sourceGroupRunId.current;
    setSourceGroupActivity("confirming");
    setSourceGroupError(null);
    try {
      const result = sourceUpdateActive
        ? await client.confirmSourceUpdate({
            remoteId: sourcePromotionDraft.remoteId,
          })
        : await client.confirmSourcePromotion({
            remoteId: sourcePromotionDraft.remoteId,
            sourceType: sourceGroupType,
            sourceUrl: sourcePromotionDraft.sourceUrl,
            trackingPolicy: sourceGroupTrackingPolicy(),
            expectedSelectedRef: sourcePromotionDraft.policy.selectedRef,
            expectedResolvedCommit: sourcePromotionDraft.policy.resolvedCommit,
          });
      const snapshot = await client.listSkills(filter);
      if (runId !== sourceGroupRunId.current) return;
      setSkills(snapshot.items);
      setSourcePromotionResult({
        ...result,
        undoAvailable: !sourceUpdateActive,
      });
      void client
        .getGitSourceCapability()
        .then((report) => {
          if (runId !== sourceGroupRunId.current) return;
          setGitSourceCapability(report);
          setGitSourceCapabilityFailure(null);
        })
        .catch((reason: unknown) => {
          if (runId !== sourceGroupRunId.current) return;
          setGitSourceCapability(null);
          setGitSourceCapabilityFailure({
            diagnostic: readDiagnostic(reason, t),
          });
        });
    } catch (reason) {
      if (runId === sourceGroupRunId.current) {
        setSourceGroupError(readError(reason, t));
      }
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
    }
  }

  async function undoSourceTransition() {
    if (!sourceTransitionResult) return;
    const runId = ++sourceGroupRunId.current;
    setSourceGroupActivity("undoing");
    setSourceGroupError(null);
    try {
      await client.undoSourceTransition(sourceTransitionResult.operationId);
      const snapshot = await client.listSkills(filter);
      if (runId !== sourceGroupRunId.current) return;
      setSkills(snapshot.items);
      setSourceTransitionResult(null);
      setSourceGroupOutcome(null);
    } catch (reason) {
      if (runId === sourceGroupRunId.current) {
        setSourceGroupError(readError(reason, t));
      }
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
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
      const [snapshot, nextDetail] = await Promise.all([
        client.listSkills(filter),
        client.inspectSkill(selectedId),
      ]);
      setSkills(snapshot.items);
      setDetail(nextDetail);
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

  async function openAdopt() {
    adoptRunId.current += 1;
    setIsAdoptOpen(true);
    adoptSourcePreviewCache.current.clear();
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
      preloadAdoptGitSources(report.gitSources);
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
    adoptSourcePreviewCache.current.clear();
    setAdoptPlan(null);
    setAdoptError(null);
    setAdoptErrorHeading("app.notice.scan_failed");
    setAdoptActivity("scanning");
    try {
      const report = await client.scanAdopt();
      if (runId !== adoptRunId.current) return;
      setAdoptReport(report);
      preloadAdoptGitSources(report.gitSources);
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
    adoptSourcePreviewCache.current.clear();
    setAdoptReport(report);
    preloadAdoptGitSources(report.gitSources);
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
      "source-group-preview",
      sourceGroupActivity,
      SOURCE_GROUP_OPERATION_COPIES,
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
  ].filter(isOperationStatus);

  return (
    <>
      <LibraryDesk
        client={client}
        filter={filter}
        skills={skills}
        libraryEmpty={libraryLoaded && skills.length === 0}
        selectedId={selectedId}
        detail={detail}
        error={error}
        gitSourceCapability={gitSourceCapability}
        gitSourceCapabilityFailure={gitSourceCapabilityFailure}
        isLinkImportOpen={isLinkImportOpen}
        importKind={importKind}
        linkImportPreview={linkImportPreview}
        linkImportResult={linkImportResult}
        linkImportError={linkImportError}
        linkImportActivity={linkImportActivity}
        sourceGroupType={sourceGroupType}
        sourceGroupUrl={sourceGroupUrl}
        sourceGroupPolicyMode={sourceGroupPolicyMode}
        sourceGroupPolicyValue={sourceGroupPolicyValue}
        sourceGroupOutcome={sourceGroupOutcome}
        sourceTransitionResult={sourceTransitionResult}
        sourcePromotionActive={sourcePromotionRemoteId !== null}
        sourcePromotionDraft={sourcePromotionDraft}
        sourcePromotionOutcome={sourcePromotionOutcome}
        sourceUpdateDraft={sourceUpdateDraft}
        sourcePromotionResult={sourcePromotionResult}
        sourceGroupError={sourceGroupError}
        sourceGroupActivity={sourceGroupActivity}
        onFilter={setFilter}
        onSelect={setSelectedId}
        onOpenLinkImport={openImport}
        onImportKindChange={setImportKind}
        onPreviewLinkImport={previewLinkImport}
        onApplyLinkImport={applyLinkImport}
        onCloseLinkImport={closeImport}
        onOpenImportedSkill={openImportedSkill}
        onSourceGroupTypeChange={(sourceType) => {
          setSourceGroupType(sourceType);
          setSourceGroupOutcome(null);
          setSourceGroupError(null);
        }}
        onSourceGroupUrlChange={(sourceUrl) => {
          setSourceGroupUrl(sourceUrl);
          setSourceGroupOutcome(null);
          setSourceGroupError(null);
        }}
        onSourceGroupPolicyChange={(mode, value) => {
          setSourceGroupPolicyMode(mode);
          setSourceGroupPolicyValue(value);
          setSourceGroupOutcome(null);
          setSourceGroupError(null);
        }}
        onFetchLatestAndManage={fetchLatestAndManage}
        onConfirmSourceTransition={confirmSourceTransition}
        onUndoSourceTransition={undoSourceTransition}
        onPreviewSourcePromotion={previewSourcePromotion}
        onPreviewSourceUpdate={previewSourceUpdate}
        onConfirmSourcePromotion={confirmSourcePromotion}
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
        onManageAdoptGitSource={manageAdoptGitSource}
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
