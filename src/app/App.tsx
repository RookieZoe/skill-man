import { useEffect, useRef, useState } from "react";

import { listen } from "@tauri-apps/api/event";

import { LibraryDesk } from "../features/library/LibraryDesk";
import { parseRepositoryInput } from "../features/library/git-repository-input";
import {
  BackgroundOperations,
  useBackgroundOperations,
} from "../ui/BackgroundOperations";
import {
  useLocale,
  type LocaleContextValue,
} from "../features/locale/LocaleProvider";
import {
  errorMessageKey,
  errorMessageParams,
} from "../features/locale/messages";
import type {
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

export function App(props: AppProps) {
  return (
    <BackgroundOperations>
      <AppContent {...props} />
    </BackgroundOperations>
  );
}

function AppContent({ client }: AppProps) {
  const { t } = useLocale();
  const notifications = useBackgroundOperations();
  // Effects only render errors via `t`; a locale switch must not re-run
  // catalog/health effects, so the current `t` is mirrored into a ref.
  const tRef = useRef(t);
  tRef.current = t;
  const [filter, setFilter] = useState<CatalogFilter>("all");
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [catalogRefreshToken, setCatalogRefreshToken] = useState(0);
  const [gitSourceCapability, setGitSourceCapability] =
    useState<GitSourceCapabilityReport | null>(null);
  const [gitSourceCapabilityFailure, setGitSourceCapabilityFailure] = useState<{
    diagnostic: string | null;
  } | null>(null);
  const [libraryLoaded, setLibraryLoaded] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
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
  const [onboardingLibraryPath, setOnboardingLibraryPath] = useState<
    string | null
  >(null);
  const [isOnboardingOpen, setIsOnboardingOpen] = useState(false);
  const [onboardingStep, setOnboardingStep] = useState(0);
  const [onboardingScanCount, setOnboardingScanCount] = useState<number | null>(
    null,
  );
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
  const [sourceActionActivity, setSourceActionActivity] = useState(false);
  const sourceActionCount = useRef(0);
  const [backgroundRefresh, setBackgroundRefresh] = useState(0);
  const [sourceActionNotice] = useState<{
    kind: "restore" | "copy" | "remove";
    ok: boolean;
    message: string | null;
  } | null>(null);
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
        setListError(null);
      })
      .catch((reason: unknown) => {
        if (current) setListError(readError(reason, tRef.current));
      })
      .finally(() => {
        if (current) setLibraryLoaded(true);
      });
    return () => {
      current = false;
    };
  }, [client, filter, backgroundRefresh]);

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
  }, [client, backgroundRefresh]);

  useEffect(() => {
    if (!selectedId) return;

    let current = true;
    client
      .inspectSkill(selectedId)
      .then((nextDetail) => {
        if (!current) return;
        setDetail(nextDetail);
        setDetailError(null);
      })
      .catch((reason: unknown) => {
        if (current) setDetailError(readError(reason, tRef.current));
      });
    return () => {
      current = false;
    };
  }, [client, selectedId, backgroundRefresh]);

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
      await refreshLibraryAfterSourceAction(++sourceGroupRunId.current);
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
    setSourceUpdateDraft(null);
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
      sourceType:
        parseRepositoryInput(sourceGroupUrl)?.sourceType ?? sourceGroupType,
      sourceUrl:
        parseRepositoryInput(sourceGroupUrl)?.sourceUrl ?? sourceGroupUrl,
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
    setSourceUpdateDraft(null);
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

  /**
   * Git Repository Source handoff (spec §8.1): the scan candidate group is
   * handed to the source modules (#92 — Source Tracking Policy + immutable
   * Source Transition); the worktree HEAD/dirty bytes/old lock facts of the
   * report never synthesize a plan here.
   */
  function manageGitGroup(
    sourceType: GitRepositorySourceType,
    sourceUrl: string,
  ) {
    if (sourceGroupActivity !== "idle" || linkImportActivity !== "idle") {
      return;
    }
    linkImportRunId.current += 1;
    setLinkImportPreview(null);
    setLinkImportResult(null);
    setLinkImportError(null);
    setImportKind("git");
    setSourceGroupType(sourceType);
    setSourceGroupUrl(sourceUrl);
    setSourceGroupPolicyMode("auto_release_tag_head");
    setSourceGroupPolicyValue("");
    setSourceGroupOutcome(null);
    setSourceTransitionResult(null);
    setSourcePromotionRemoteId(null);
    setSourceUpdateActive(false);
    setSourcePromotionDraft(null);
    setSourcePromotionOutcome(null);
    setSourceUpdateDraft(null);
    setSourcePromotionResult(null);
    setSourceGroupError(null);
    setSourceGroupActivity("idle");
    setIsLinkImportOpen(true);

    void fetchLatestAndManage({
      sourceType,
      sourceUrl,
      trackingPolicy: null,
    });
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
    setSourceUpdateDraft(null);
    setSourcePromotionResult(null);
    setSourceGroupError(null);
    setSourceGroupActivity("idle");
    if (linkPlanToken) {
      await client.cancelLinkImport(linkPlanToken).catch(() => undefined);
    }
  }

  async function confirmSourceTransition(expectedRemovedClaims: string[] = []) {
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
        expectedRemovedClaims,
      });
      if (runId !== sourceGroupRunId.current) return;
      setSourceTransitionResult(result);
      await refreshLibraryAfterSourceAction(runId).catch(() => {
        if (runId === sourceGroupRunId.current) {
          setSourceGroupError(t("operation.background.refresh_failed"));
        }
      });
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
    setSourceUpdateDraft(null);
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
    setSourceUpdateDraft(null);
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
    const draft = sourceUpdateActive ? sourceUpdateDraft : sourcePromotionDraft;
    if (!draft) return;
    const runId = ++sourceGroupRunId.current;
    setSourceGroupActivity("confirming");
    setSourceGroupError(null);
    try {
      let result: SourcePromotionResult;
      if (sourceUpdateActive) {
        result = await client.confirmSourceUpdate({
          remoteId: draft.remoteId,
          expectedSelectedRef: draft.policy.selectedRef,
          expectedResolvedCommit: draft.policy.resolvedCommit,
        });
      } else {
        const sourceType = sourceTypeFromProvider(draft.provider);
        if (!sourceType) {
          setSourceGroupError(t("error.source_unavailable"));
          return;
        }
        result = await client.confirmSourcePromotion({
          remoteId: draft.remoteId,
          sourceType,
          sourceUrl: draft.sourceUrl,
          trackingPolicy: {
            mode: draft.policy.mode,
            value: draft.policy.value,
          },
          expectedSelectedRef: draft.policy.selectedRef,
          expectedResolvedCommit: draft.policy.resolvedCommit,
        });
      }
      if (runId !== sourceGroupRunId.current) return;
      setSourceUpdateDraft(null);
      setSourcePromotionDraft(null);
      setSourcePromotionResult(result);
      await refreshLibraryAfterSourceAction(runId).catch(() => {
        if (runId === sourceGroupRunId.current) {
          setSourceGroupError(t("operation.background.refresh_failed"));
        }
      });
    } catch (reason) {
      if (runId === sourceGroupRunId.current) {
        setSourceGroupError(readError(reason, t));
      }
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
    }
  }

  async function undoSourcePromotion() {
    if (!sourcePromotionResult) return;
    const runId = ++sourceGroupRunId.current;
    setSourceGroupActivity("undoing");
    setSourceGroupError(null);
    try {
      if (sourceUpdateActive) {
        await client.undoSourceUpdate(sourcePromotionResult.operationId);
      } else {
        await client.undoSourceTransition(sourcePromotionResult.operationId);
      }
      setSourcePromotionResult(null);
      setSourcePromotionDraft(null);
      setSourceUpdateDraft(null);
      setSourcePromotionOutcome(null);
      setSourcePromotionRemoteId(null);
      setSourceUpdateActive(false);
      await refreshLibraryAfterSourceAction(runId);
    } catch (reason) {
      if (runId === sourceGroupRunId.current) {
        setSourceGroupError(readError(reason, t));
      }
    } finally {
      if (runId === sourceGroupRunId.current) setSourceGroupActivity("idle");
    }
  }

  async function refreshLibraryAfterSourceAction(runId: number) {
    const [snapshot, nextDetail, capability] = await Promise.all([
      client.listSkills(filter),
      selectedId ? client.inspectSkill(selectedId).catch(() => null) : null,
      client.getGitSourceCapability().then(
        (report) => ({ report, failure: null }),
        (reason: unknown) => ({
          report: null,
          failure: { diagnostic: readDiagnostic(reason, t) },
        }),
      ),
    ]);
    if (runId !== sourceGroupRunId.current) return;
    setSkills(snapshot.items);
    setSelectedId((selected) =>
      snapshot.items.some(({ id }) => id === selected)
        ? selected
        : (snapshot.items[0]?.id ?? null),
    );
    setDetail(nextDetail);
    setGitSourceCapability(capability.report);
    setGitSourceCapabilityFailure(capability.failure);
    setCatalogRefreshToken((token) => token + 1);
  }

  async function refreshAfterAdopt() {
    const runId = ++sourceGroupRunId.current;
    try {
      await refreshLibraryAfterSourceAction(runId);
      if (runId === sourceGroupRunId.current) setError(null);
    } catch (reason) {
      if (runId === sourceGroupRunId.current) setError(readError(reason, t));
    }
  }

  function beginSourceAction() {
    sourceActionCount.current++;
    setSourceActionActivity(true);
  }

  function endSourceAction() {
    sourceActionCount.current--;
    setSourceActionActivity(sourceActionCount.current > 0);
  }

  // Refresh is independent of the write outcome and of any open import dialog.
  function refreshBackgroundCatalog() {
    setBackgroundRefresh((value) => value + 1);
    setCatalogRefreshToken((value) => value + 1);
  }

  async function restoreSourceRelease(remoteId: string) {
    beginSourceAction();
    const noticeId = notifications.begin({
      title: t("sourceGroup.restoreRelease"),
      detail: t("operation.background.working"),
    });
    try {
      await client.restoreCurrentSourceRelease(remoteId);
      refreshBackgroundCatalog();
      notifications.finish(noticeId, {
        title: t("sourceGroup.restoreSuccess"),
        detail: t("operation.background.next_repositories"),
      });
    } catch (reason) {
      notifications.finish(noticeId, {
        title: t("operation.background.failed"),
        detail: readError(reason, t),
        state: "failed",
      });
    } finally {
      endSourceAction();
    }
  }

  async function copySourceMember(
    remoteId: string,
    skillId: string,
    destination: string,
  ) {
    beginSourceAction();
    try {
      await client.createLocalSourceCopy(remoteId, skillId, destination);
      refreshBackgroundCatalog();
      return true;
    } catch (reason) {
      throw new Error(readError(reason, t), { cause: reason });
    } finally {
      endSourceAction();
    }
  }

  async function removeSource(remoteId: string) {
    beginSourceAction();
    const noticeId = notifications.begin({
      title: t("sourceGroup.remove"),
      detail: t("operation.background.working"),
    });
    try {
      await client.removeGitSource(remoteId);
      refreshBackgroundCatalog();
      notifications.finish(noticeId, {
        title: t("library.source_capability.remove_done"),
        detail: t("operation.background.next_repositories"),
      });
    } catch (reason) {
      notifications.finish(noticeId, {
        title: t("operation.background.failed"),
        detail: readError(reason, t),
        state: "failed",
      });
    } finally {
      endSourceAction();
    }
  }

  async function undoSourceTransition() {
    if (!sourceTransitionResult) return;
    const runId = ++sourceGroupRunId.current;
    setSourceGroupActivity("undoing");
    setSourceGroupError(null);
    try {
      await client.undoSourceTransition(sourceTransitionResult.operationId);
      await refreshLibraryAfterSourceAction(runId);
      if (runId !== sourceGroupRunId.current) return;
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
    const noticeId = notifications.begin({
      title: t("library.preferences.checking"),
      detail: t("operation.background.working"),
    });
    const runId = ++appUpdateCheckRunId.current;
    setAppUpdatePanel({
      activity: "checking",
      update: null,
      checkStatus: null,
      error: null,
    });
    try {
      const result = await client.checkAppUpdate(true);
      notifications.finish(noticeId, {
        title: t("operation.background.done"),
        detail: t(
          result.status === "available"
            ? "operation.background.update_available"
            : result.status === "up_to_date"
              ? "operation.background.update_current"
              : "operation.background.update_skipped",
        ),
      });
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
      notifications.finish(noticeId, {
        title: t("operation.background.failed"),
        detail: readAppUpdateError(reason, t),
        state: "failed",
      });
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
      setOnboardingScanCount(null);
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
      // Configuration was explicitly confirmed in step 2. Scanning remains
      // read-only and does not Adopt or Enable any Skill.
      setOnboardingActivity("scanning");
      setOnboardingError(null);
      setOnboardingStep(2);
      try {
        const configuration = await client.getAgentManagementSnapshot();
        if (!configuration.configurations.length)
          throw new Error(t("scan.setup.empty"));
        const started = await client.startRescan("onboarding");
        const runId = started.scanRun?.runId;
        if (!runId) throw new Error(t("scan.setup.scan_failed"));
        // Wait for the terminal Report: the shared evidence contract.
        let scanCount: number | null = null;
        const deadline = Date.now() + 120_000;
        while (Date.now() < deadline) {
          const snapshot = await client.getObservationSnapshot();
          const run = snapshot.scanRun;
          const report = snapshot.currentReport?.summary ?? null;
          if (
            report?.runId === runId &&
            run?.runId === runId &&
            run.state === "completed"
          ) {
            scanCount = report.counts.entities;
            break;
          }
          if (
            !run ||
            run.runId !== runId ||
            ["failed", "cancelled", "superseded"].includes(run.state)
          )
            throw new Error(t("scan.setup.scan_failed"));
          const { promise, resolve } = Promise.withResolvers<void>();
          setTimeout(resolve, 400);
          await promise;
        }
        if (scanCount === null) throw new Error(t("scan.setup.scan_timeout"));
        setOnboardingScanCount(scanCount);
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

  return (
    <>
      <LibraryDesk
        client={client}
        filter={filter}
        skills={skills}
        catalogRefreshToken={catalogRefreshToken}
        onCatalogChanged={refreshAfterAdopt}
        libraryEmpty={libraryLoaded && skills.length === 0}
        selectedId={selectedId}
        detail={detail}
        error={listError ?? detailError ?? error}
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
          const parsed = parseRepositoryInput(sourceUrl);
          if (parsed) setSourceGroupType(parsed.sourceType);
          setSourceGroupOutcome(null);
          setSourceGroupError(null);
        }}
        onSourceGroupPolicyChange={(mode, value) => {
          setSourceGroupPolicyMode(mode);
          setSourceGroupPolicyValue(value);
          setSourceGroupOutcome(null);
          setSourceGroupError(null);
        }}
        onFetchLatestAndManage={() => void fetchLatestAndManage()}
        onConfirmSourceTransition={confirmSourceTransition}
        onUndoSourceTransition={undoSourceTransition}
        onPreviewSourcePromotion={previewSourcePromotion}
        onPreviewSourceUpdate={previewSourceUpdate}
        onConfirmSourcePromotion={confirmSourcePromotion}
        onUndoSourcePromotion={undoSourcePromotion}
        sourceActionActivity={sourceActionActivity}
        sourceActionNotice={sourceActionNotice}
        onRestoreSource={(remoteId) => void restoreSourceRelease(remoteId)}
        onCopySourceMember={(remoteId, skillId, destination) =>
          copySourceMember(remoteId, skillId, destination)
        }
        onRemoveSource={(remoteId) => void removeSource(remoteId)}
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
        onManageGitGroup={manageGitGroup}
        isPreferencesOpen={isPreferencesOpen}
        preferences={preferences}
        preferencesWarning={preferencesWarning}
        preferencesError={preferencesError}
        appUpdatePanel={appUpdatePanel}
        isOnboardingOpen={isOnboardingOpen}
        onboardingStep={onboardingStep}
        onboardingLibraryPath={onboardingLibraryPath}
        onboardingScanCount={onboardingScanCount}
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
        onOpenScanSetup={() => {
          setOnboardingStep(1);
          setOnboardingScanCount(null);
          setOnboardingError(null);
          setIsOnboardingOpen(true);
        }}
      />
    </>
  );
}

function sourceTypeFromProvider(
  provider: string,
): GitRepositorySourceType | null {
  switch (provider) {
    case "gitlab":
      return "gitlab";
    case "git":
      return "git";
    case "github":
      return "github";
    default:
      return null;
  }
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
