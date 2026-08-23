import { useEffect, useLayoutEffect, useRef, useState, type Ref } from "react";

import type {
  AppUpdatePanelState,
  ImportKind,
  RelocatePanelState,
  RemovePanelState,
} from "../../app/App";
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
  CatalogFilter,
  CompatibilityWarning,
  GitRepositorySourceType,
  GitSourceCapabilityReport,
  Health,
  LinkImportPreview,
  LinkImportResult,
  OccupierNotAdoptableReason,
  OccupierKind,
  OccupierSummary,
  PreferenceUpdates,
  PreferencesWarning,
  SkillDetail,
  SkillSummary,
  SourceKind,
  SourceGroupPreviewOutcome,
  SourceTransitionResult,
  StartupAgent,
} from "../../app/catalog-client";
import { EvidenceLedger } from "../adopt/EvidenceLedger";
import { useLocale, type LocaleContextValue } from "../locale/LocaleProvider";
import { LanguageControl } from "../locale/LanguageControl";
import { IndeterminateProgress } from "../../ui/IndeterminateProgress";
import type { MessageKey } from "../locale/messages";
import { formatByteSize, formatDateTime } from "../locale/messages";
import { LockIcon, SettingsIcon } from "../../ui/icons";
import {
  GitSourceCapabilityNotice,
  type GitSourceCapabilityFailure,
} from "./GitSourceCapabilityNotice";
import { SourceGroupPreviewFlow } from "./SourceGroupPreviewFlow";

const filters: Array<{ value: CatalogFilter; labelKey: MessageKey }> = [
  { value: "all", labelKey: "library.filter.all" },
  { value: "broken", labelKey: "library.filter.broken" },
  { value: "modified", labelKey: "library.filter.modified" },
  { value: "link", labelKey: "library.filter.link" },
  { value: "install", labelKey: "library.filter.install" },
];

export type LayoutMode = "wide" | "mid" | "narrow";
type PaneKey = "library" | "detail" | "agents";

export const WIDE_BREAKPOINT = 1060;
export const MID_BREAKPOINT = 760;

export function layoutModeForWidth(width: number): LayoutMode {
  if (width >= WIDE_BREAKPOINT) return "wide";
  if (width >= MID_BREAKPOINT) return "mid";
  return "narrow";
}

interface LibraryDeskProps {
  filter: CatalogFilter;
  skills: SkillSummary[];
  libraryEmpty: boolean;
  selectedId: string | null;
  detail: SkillDetail | null;
  agents: AgentActivation[];
  error: string | null;
  gitSourceCapability: GitSourceCapabilityReport | null;
  gitSourceCapabilityFailure: GitSourceCapabilityFailure | null;
  activationError: string | null;
  activationConflict: ActivationConflictDetails | null;
  activationConflictMessage: string | null;
  replacePreview: ActivationReplacePreview | null;
  replaceResult: ActivationResult | null;
  replaceUndo: ActivationReplaceUndoResult | null;
  replaceError: string | null;
  isApplyingReplace: boolean;
  isUndoingReplace: boolean;
  activationPreview: ActivationPreview | null;
  activationTriggerControlId: string | null;
  pendingAgentId: string | null;
  isApplyingActivation: boolean;
  isCheckingActivations: boolean;
  isLinkImportOpen: boolean;
  importKind: ImportKind;
  linkImportPreview: LinkImportPreview | null;
  linkImportResult: LinkImportResult | null;
  linkImportError: string | null;
  linkImportActivity: "idle" | "discovering" | "applying";
  sourceGroupType: GitRepositorySourceType;
  sourceGroupUrl: string;
  sourceGroupRef: string;
  sourceGroupOutcome: SourceGroupPreviewOutcome | null;
  sourceTransitionResult: SourceTransitionResult | null;
  sourceGroupError: string | null;
  sourceGroupActivity: "idle" | "fetching" | "confirming" | "undoing";
  relocatePanel: RelocatePanelState;
  onOpenRelocate: () => void;
  onCloseRelocate: () => void;
  onRelocateSourcePathChange: (sourcePath: string) => void;
  onPreviewRelocate: (sourcePath: string) => void;
  onApplyRelocate: () => void;
  removePanel: RemovePanelState;
  onOpenRemove: () => void;
  onCloseRemove: () => void;
  onApplyRemove: () => void;
  lockNotice: string | null;
  onRetryRecovery: () => void;
  isAdoptOpen: boolean;
  adoptReport: AdoptEvidenceReport | null;
  adoptSelections: Record<string, AdoptSelection>;
  adoptPlan: AdoptPlan | null;
  adoptResult: AdoptResult | null;
  adoptUndo: AdoptUndoResult | null;
  adoptError: string | null;
  adoptErrorHeading: MessageKey;
  adoptActivity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onFilter: (filter: CatalogFilter) => void;
  onSelect: (skillId: string) => void;
  onRequestActivation: (agentId: string, enabled: boolean) => void;
  onRequestActivationRepair: (agentId: string) => void;
  onApplyActivation: () => void;
  onCancelActivation: () => void;
  onCloseActivationConflict: () => void;
  onAdoptFromConflict: () => void;
  onPlanReplace: () => void;
  onApplyReplace: () => void;
  onUndoReplace: () => void;
  onOpenLinkImport: () => void;
  onImportKindChange: (kind: ImportKind) => void;
  onPreviewLinkImport: (sourcePath: string) => void;
  onApplyLinkImport: () => void;
  onCloseLinkImport: () => void;
  onOpenImportedSkill: () => void;
  onSourceGroupTypeChange: (sourceType: GitRepositorySourceType) => void;
  onSourceGroupUrlChange: (sourceUrl: string) => void;
  onSourceGroupRefChange: (trackingRef: string) => void;
  onConfirmSourceTransition: () => void;
  onUndoSourceTransition: () => void;
  onFetchLatestAndManage: () => void;
  onOpenAdopt: () => void;
  onRescanAdopt: () => void;
  onToggleAdoptCandidate: (canonicalEntity: string, checked: boolean) => void;
  onSetAdoptBranch: (
    canonicalEntity: string,
    modifiedBranch: ModifiedBranch,
  ) => void;
  onPlanAdopt: () => void;
  onApplyAdopt: () => void;
  onUndoAdopt: () => void;
  onCloseAdopt: () => void;
  isPreferencesOpen: boolean;
  preferences: AppPreferences | null;
  preferencesWarning: PreferencesWarning | null;
  preferencesError: string | null;
  appUpdatePanel: AppUpdatePanelState;
  isOnboardingOpen: boolean;
  onboardingStep: number;
  onboardingAgents: StartupAgent[];
  onboardingLibraryPath: string | null;
  onboardingReport: AdoptEvidenceReport | null;
  onboardingActivity: "idle" | "checking" | "scanning";
  onboardingError: string | null;
  onOpenPreferences: () => void;
  onClosePreferences: () => void;
  onTogglePreference: (updates: PreferenceUpdates) => void;
  onCheckAppUpdate: () => void;
  onDownloadAppUpdate: () => void;
  onInstallAppUpdate: () => void;
  onCloseAppUpdate: () => void;
  onCompleteOnboarding: () => void;
  onAdvanceOnboarding: () => void;
  onCreateAgentDirectory: (agentId: string) => void;
  onFinishOnboardingWithAdopt: () => void;
}

export function LibraryDesk({
  filter,
  skills,
  libraryEmpty,
  selectedId,
  detail,
  agents,
  error,
  gitSourceCapability,
  gitSourceCapabilityFailure,
  activationError,
  activationConflict,
  activationConflictMessage,
  replacePreview,
  replaceResult,
  replaceUndo,
  replaceError,
  isApplyingReplace,
  isUndoingReplace,
  activationPreview,
  activationTriggerControlId,
  pendingAgentId,
  isApplyingActivation,
  isCheckingActivations,
  isLinkImportOpen,
  importKind,
  linkImportPreview,
  linkImportResult,
  linkImportError,
  linkImportActivity,
  sourceGroupType,
  sourceGroupUrl,
  sourceGroupRef,
  sourceGroupOutcome,
  sourceTransitionResult,
  sourceGroupError,
  sourceGroupActivity,
  relocatePanel,
  onOpenRelocate,
  onCloseRelocate,
  onRelocateSourcePathChange,
  onPreviewRelocate,
  onApplyRelocate,
  removePanel,
  onOpenRemove,
  onCloseRemove,
  onApplyRemove,
  lockNotice,
  onRetryRecovery,
  onFilter,
  onSelect,
  onRequestActivation,
  onRequestActivationRepair,
  onApplyActivation,
  onCancelActivation,
  onCloseActivationConflict,
  onAdoptFromConflict,
  onPlanReplace,
  onApplyReplace,
  onUndoReplace,
  onOpenLinkImport,
  onImportKindChange,
  onPreviewLinkImport,
  onApplyLinkImport,
  onCloseLinkImport,
  onOpenImportedSkill,
  onSourceGroupTypeChange,
  onSourceGroupUrlChange,
  onSourceGroupRefChange,
  onConfirmSourceTransition,
  onUndoSourceTransition,
  onFetchLatestAndManage,
  isAdoptOpen,
  adoptReport,
  adoptSelections,
  adoptPlan,
  adoptResult,
  adoptUndo,
  adoptError,
  adoptErrorHeading,
  adoptActivity,
  onOpenAdopt,
  onRescanAdopt,
  onToggleAdoptCandidate,
  onSetAdoptBranch,
  onPlanAdopt,
  onApplyAdopt,
  onUndoAdopt,
  onCloseAdopt,
  isPreferencesOpen,
  preferences,
  preferencesWarning,
  preferencesError,
  appUpdatePanel,
  isOnboardingOpen,
  onboardingStep,
  onboardingAgents,
  onboardingLibraryPath,
  onboardingReport,
  onboardingActivity,
  onboardingError,
  onOpenPreferences,
  onClosePreferences,
  onTogglePreference,
  onCheckAppUpdate,
  onDownloadAppUpdate,
  onInstallAppUpdate,
  onCloseAppUpdate,
  onCompleteOnboarding,
  onAdvanceOnboarding,
  onCreateAgentDirectory,
  onFinishOnboardingWithAdopt,
}: LibraryDeskProps) {
  const { t } = useLocale();
  const [layoutMode, setLayoutMode] = useState<LayoutMode>(() =>
    layoutModeForWidth(window.innerWidth),
  );
  const [agentDrawerOpen, setAgentDrawerOpen] = useState(false);
  const [activePane, setActivePane] = useState<PaneKey>("library");
  const agentInspectorRef = useRef<HTMLElement | null>(null);
  const pendingDrawerFocus = useRef(false);
  const lastOverlay = useRef<
    "activation" | "import" | "preferences" | "appUpdate" | null
  >(null);
  const hasActivationOverlay = Boolean(activationPreview || activationConflict);
  const hasOtherOverlay =
    hasActivationOverlay ||
    isLinkImportOpen ||
    relocatePanel.isOpen ||
    removePanel.isOpen ||
    isAdoptOpen ||
    isOnboardingOpen ||
    isPreferencesOpen;
  const hasAppUpdateOverlay =
    Boolean(appUpdatePanel.update) && !hasOtherOverlay;
  const hasOverlay = hasOtherOverlay || hasAppUpdateOverlay;
  const isAgentDrawerModal = layoutMode === "mid" && agentDrawerOpen;

  useEffect(() => {
    function onResize() {
      setLayoutMode((mode) => {
        const next = layoutModeForWidth(window.innerWidth);
        return next === mode ? mode : next;
      });
    }
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    if (!isAgentDrawerModal) return;
    function handleKeyDown(event: KeyboardEvent) {
      if (hasOverlay) return;
      if (event.key === "Escape" && !isApplyingActivation) {
        event.preventDefault();
        setAgentDrawerOpen(false);
        document.getElementById("agent-drawer-trigger")?.focus();
        return;
      }
      if (event.key !== "Tab") return;
      const inspector = agentInspectorRef.current;
      if (!inspector) return;
      const focusable = Array.from(
        inspector.querySelectorAll<HTMLElement>(
          "input, button, select, textarea, [tabindex]",
        ),
      ).filter((element) => {
        if (element.getAttribute("tabindex") === "-1") return false;
        if (
          element instanceof HTMLButtonElement ||
          element instanceof HTMLInputElement ||
          element instanceof HTMLSelectElement ||
          element instanceof HTMLTextAreaElement
        ) {
          return !element.disabled;
        }
        return true;
      });
      if (focusable.length === 0) {
        event.preventDefault();
        inspector.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable.at(-1)!;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [hasOverlay, isAgentDrawerModal, isApplyingActivation]);

  useLayoutEffect(() => {
    if (isAgentDrawerModal && pendingDrawerFocus.current) {
      pendingDrawerFocus.current = false;
      agentInspectorRef.current?.focus();
    }
  }, [isAgentDrawerModal]);

  useLayoutEffect(() => {
    if (hasActivationOverlay) {
      lastOverlay.current = "activation";
    } else if (isLinkImportOpen) {
      lastOverlay.current = "import";
    } else if (hasAppUpdateOverlay) {
      lastOverlay.current = "appUpdate";
    } else if (isPreferencesOpen) {
      lastOverlay.current = "preferences";
    } else if (lastOverlay.current) {
      if (lastOverlay.current === "activation" && activationTriggerControlId) {
        const trigger = document.getElementById(activationTriggerControlId);
        const fallback = document.getElementById(
          activationTriggerControlId.replace(
            "activation-repair-",
            "activation-",
          ),
        );
        (trigger ?? fallback)?.focus();
      } else if (lastOverlay.current === "import") {
        document.getElementById("link-import-trigger")?.focus();
      } else if (
        lastOverlay.current === "preferences" ||
        lastOverlay.current === "appUpdate"
      ) {
        document.getElementById("preferences-trigger")?.focus();
      }
      lastOverlay.current = null;
    }
  }, [
    activationTriggerControlId,
    hasActivationOverlay,
    hasAppUpdateOverlay,
    isLinkImportOpen,
    isPreferencesOpen,
  ]);

  function toggleAgentDrawer() {
    const next = !agentDrawerOpen;
    setAgentDrawerOpen(next);
    if (next) {
      pendingDrawerFocus.current = true;
    } else {
      document.getElementById("agent-drawer-trigger")?.focus();
    }
  }

  function closeAgentDrawer() {
    setAgentDrawerOpen(false);
    document.getElementById("agent-drawer-trigger")?.focus();
  }

  return (
    <div
      className="app-shell"
      data-layout-mode={layoutMode}
      data-active-pane={activePane}
    >
      <a className="skip-link" href="#skill-detail">
        {t("library.skip_to_detail")}
      </a>
      <Toolbar
        layoutMode={layoutMode}
        agentDrawerOpen={agentDrawerOpen}
        onToggleAgentDrawer={toggleAgentDrawer}
        onImport={onOpenLinkImport}
        onAdopt={onOpenAdopt}
        onOpenPreferences={onOpenPreferences}
      />
      <div className="notice-region">
        {error ? (
          <div className="global-notice" role="alert">
            <strong>{t("library.notice.unavailable")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
        {lockNotice ? (
          <div className="global-notice global-notice--locked" role="alert">
            <strong>{t("library.notice.recovery_locked")}</strong>
            <span>{lockNotice}</span>
            <span>{t("library.notice.recovery_locked_body")}</span>
            <button
              type="button"
              className="repair-button"
              onClick={onRetryRecovery}
            >
              {t("library.notice.retry_recovery")}
            </button>
          </div>
        ) : null}
        <GitSourceCapabilityNotice
          report={gitSourceCapability}
          failure={gitSourceCapabilityFailure}
        />
      </div>
      <div className="app-background" inert={hasOverlay ? true : undefined}>
        <div className="library-desk">
          {layoutMode === "narrow" ? (
            <div
              className="pane-nav"
              role="group"
              aria-label={t("library.pane.label")}
            >
              {(["library", "detail", "agents"] as const).map((pane) => (
                <button
                  type="button"
                  key={pane}
                  aria-pressed={activePane === pane}
                  onClick={() => setActivePane(pane)}
                >
                  {pane === "library"
                    ? t("library.pane.library")
                    : pane === "detail"
                      ? t("library.pane.skill")
                      : t("library.pane.agents")}
                </button>
              ))}
            </div>
          ) : null}
          <LibrarySidebar
            filter={filter}
            skills={skills}
            selectedId={selectedId}
            onFilter={onFilter}
            onSelect={(skillId) => {
              if (layoutMode === "narrow") setActivePane("detail");
              onSelect(skillId);
            }}
          />
          <SkillDetailPanel
            detail={detail}
            error={error}
            libraryEmpty={libraryEmpty}
            relocatePanel={relocatePanel}
            onOpenRelocate={onOpenRelocate}
            removePanel={removePanel}
            onOpenRemove={onOpenRemove}
          />
          <div
            className="agent-drawer"
            data-open={isAgentDrawerModal ? "true" : undefined}
          >
            <div
              className="agent-drawer-backdrop"
              onMouseDown={(event) => {
                if (
                  event.currentTarget === event.target &&
                  !isApplyingActivation
                ) {
                  closeAgentDrawer();
                }
              }}
            />
            <AgentInspector
              ref={agentInspectorRef}
              dialog={isAgentDrawerModal}
              detail={detail}
              agents={agents}
              error={activationError}
              pendingAgentId={pendingAgentId}
              isApplying={isApplyingActivation}
              isChecking={isCheckingActivations}
              onRequest={onRequestActivation}
              onRepair={onRequestActivationRepair}
            />
          </div>
        </div>
      </div>
      {activationPreview ? (
        <ActivationPreviewSheet
          preview={activationPreview}
          isApplying={isApplyingActivation}
          onApply={onApplyActivation}
          onCancel={onCancelActivation}
        />
      ) : null}
      {activationConflict ? (
        <ActivationConflictSheet
          details={activationConflict}
          message={activationConflictMessage}
          replacePreview={replacePreview}
          replaceResult={replaceResult}
          replaceUndo={replaceUndo}
          error={replaceError}
          isApplying={isApplyingReplace}
          isUndoing={isUndoingReplace}
          onAdopt={onAdoptFromConflict}
          onPlanReplace={onPlanReplace}
          onApplyReplace={onApplyReplace}
          onUndoReplace={onUndoReplace}
          onClose={onCloseActivationConflict}
        />
      ) : null}
      {relocatePanel.isOpen ? (
        <RelocateSheet
          panel={relocatePanel}
          onSourcePathChange={onRelocateSourcePathChange}
          onPreview={onPreviewRelocate}
          onApply={onApplyRelocate}
          onClose={onCloseRelocate}
        />
      ) : null}
      {removePanel.isOpen ? (
        <RemoveSheet
          panel={removePanel}
          onApply={onApplyRemove}
          onClose={onCloseRemove}
        />
      ) : null}
      {isAdoptOpen ? (
        <EvidenceLedger
          report={adoptReport}
          selections={adoptSelections}
          plan={adoptPlan}
          result={adoptResult}
          undo={adoptUndo}
          error={adoptError}
          errorHeading={adoptErrorHeading}
          activity={adoptActivity}
          onToggle={onToggleAdoptCandidate}
          onSetBranch={onSetAdoptBranch}
          onRescan={onRescanAdopt}
          onPlan={onPlanAdopt}
          onApply={onApplyAdopt}
          onUndo={onUndoAdopt}
          onClose={onCloseAdopt}
        />
      ) : null}
      {isLinkImportOpen ? (
        <LinkImportSheet
          kind={importKind}
          preview={linkImportPreview}
          result={linkImportResult}
          error={linkImportError}
          activity={linkImportActivity}
          sourceGroupType={sourceGroupType}
          sourceGroupUrl={sourceGroupUrl}
          sourceGroupRef={sourceGroupRef}
          sourceGroupOutcome={sourceGroupOutcome}
          sourceTransitionResult={sourceTransitionResult}
          sourceGroupError={sourceGroupError}
          sourceGroupActivity={sourceGroupActivity}
          onKindChange={onImportKindChange}
          onPreview={onPreviewLinkImport}
          onApply={onApplyLinkImport}
          onClose={onCloseLinkImport}
          onOpenImportedSkill={onOpenImportedSkill}
          onSourceGroupTypeChange={onSourceGroupTypeChange}
          onSourceGroupUrlChange={onSourceGroupUrlChange}
          onSourceGroupRefChange={onSourceGroupRefChange}
          onFetchLatestAndManage={onFetchLatestAndManage}
          onConfirmSourceTransition={onConfirmSourceTransition}
          onUndoSourceTransition={onUndoSourceTransition}
        />
      ) : null}
      {isOnboardingOpen ? (
        <OnboardingSheet
          step={onboardingStep}
          agents={onboardingAgents}
          libraryPath={onboardingLibraryPath}
          report={onboardingReport}
          activity={onboardingActivity}
          error={onboardingError}
          onSkip={onCompleteOnboarding}
          onAdvance={onAdvanceOnboarding}
          onCreateDirectory={onCreateAgentDirectory}
          onFinishWithAdopt={onFinishOnboardingWithAdopt}
        />
      ) : null}
      {isPreferencesOpen ? (
        <PreferencesSheet
          preferences={preferences}
          warning={preferencesWarning}
          error={preferencesError}
          appUpdatePanel={appUpdatePanel}
          onToggle={onTogglePreference}
          onCheckAppUpdate={onCheckAppUpdate}
          onClose={onClosePreferences}
        />
      ) : null}
      {hasAppUpdateOverlay ? (
        <AppUpdateSheet
          panel={appUpdatePanel}
          onDownload={onDownloadAppUpdate}
          onInstall={onInstallAppUpdate}
          onClose={onCloseAppUpdate}
        />
      ) : null}
    </div>
  );
}

function Toolbar({
  layoutMode,
  agentDrawerOpen,
  onToggleAgentDrawer,
  onImport,
  onAdopt,
  onOpenPreferences,
}: {
  layoutMode: LayoutMode;
  agentDrawerOpen: boolean;
  onToggleAgentDrawer: () => void;
  onImport: () => void;
  onAdopt: () => void;
  onOpenPreferences: () => void;
}) {
  const { t } = useLocale();
  return (
    <header className="toolbar">
      <div className="product-mark" aria-hidden="true">
        <span />
        <span />
        <span />
      </div>
      <div className="toolbar-title">
        <strong>{t("library.toolbar.brand")}</strong>
        <span>{t("library.toolbar.desk")}</span>
      </div>
      <div
        className="toolbar-actions"
        aria-label={t("library.toolbar.actions_label")}
      >
        <button type="button" className="toolbar-button" disabled>
          {t("library.toolbar.health_check")}
        </button>
        <button type="button" className="toolbar-button" onClick={onAdopt}>
          {t("library.toolbar.adopt")}
        </button>
        {layoutMode === "mid" ? (
          <button
            id="agent-drawer-trigger"
            type="button"
            className="toolbar-button"
            aria-expanded={agentDrawerOpen}
            aria-controls="agent-inspector-dialog"
            onClick={onToggleAgentDrawer}
          >
            {t("library.toolbar.agents")}
          </button>
        ) : null}
        <button
          id="link-import-trigger"
          type="button"
          className="primary-button"
          onClick={onImport}
        >
          {t("library.toolbar.import")}
        </button>
        <button
          id="preferences-trigger"
          type="button"
          className="icon-button"
          aria-label={t("library.toolbar.preferences")}
          onClick={onOpenPreferences}
        >
          <SettingsIcon />
        </button>
      </div>
    </header>
  );
}

interface LibrarySidebarProps {
  filter: CatalogFilter;
  skills: SkillSummary[];
  selectedId: string | null;
  onFilter: (filter: CatalogFilter) => void;
  onSelect: (skillId: string) => void;
}

function LibrarySidebar({
  filter,
  skills,
  selectedId,
  onFilter,
  onSelect,
}: LibrarySidebarProps) {
  const { t } = useLocale();
  return (
    <nav className="library-sidebar" aria-label={t("library.sidebar.label")}>
      <div className="panel-heading">
        <div>
          <span className="eyebrow">{t("library.sidebar.managed")}</span>
          <h1>{t("library.sidebar.label")}</h1>
        </div>
        <span
          className="count-badge"
          aria-label={t("library.sidebar.visible_count", {
            count: skills.length,
          })}
        >
          {skills.length}
        </span>
      </div>
      <div
        className="filter-strip"
        aria-label={t("library.sidebar.filter_label")}
      >
        {filters.map((item) => (
          <button
            type="button"
            className="filter-chip"
            aria-pressed={filter === item.value}
            key={item.value}
            onClick={() => onFilter(item.value)}
          >
            {t(item.labelKey)}
          </button>
        ))}
      </div>
      <div className="skill-list">
        {skills.length ? (
          skills.map((skill) => (
            <button
              type="button"
              className="skill-row"
              aria-label={skill.directoryName}
              aria-pressed={selectedId === skill.id}
              key={skill.id}
              onClick={() => onSelect(skill.id)}
            >
              <StatusDot health={skill.health} />
              <span className="skill-row-copy">
                <strong>{skill.directoryName}</strong>
                <span>{skill.description}</span>
              </span>
              <span
                className="agent-count"
                aria-label={t("library.sidebar.agent_count", {
                  count: skill.enabledAgentCount,
                })}
              >
                {skill.enabledAgentCount}
              </span>
            </button>
          ))
        ) : (
          <div className="empty-list">
            <span>{t("library.sidebar.empty")}</span>
            <small>{t("library.sidebar.empty_hint")}</small>
          </div>
        )}
      </div>
    </nav>
  );
}

function SkillDetailPanel({
  detail,
  error,
  libraryEmpty,
  relocatePanel,
  onOpenRelocate,
  removePanel,
  onOpenRemove,
}: {
  detail: SkillDetail | null;
  error: string | null;
  libraryEmpty: boolean;
  relocatePanel: RelocatePanelState;
  onOpenRelocate: () => void;
  removePanel: RemovePanelState;
  onOpenRemove: () => void;
}) {
  const { t, locale } = useLocale();
  return (
    <main
      id="skill-detail"
      className="skill-detail"
      aria-label={t("library.detail.label")}
    >
      {detail ? (
        <>
          <div className="detail-heading">
            <div className="detail-badges">
              <HealthBadge health={detail.health} />
              <span className="source-badge">
                {sourceKindLabel(detail.sourceKind, t)}
              </span>
            </div>
            <h2>{detail.directoryName}</h2>
            <p>{detail.description}</p>
          </div>
          {detail.health !== "healthy" ? (
            <HealthNotice
              detail={detail}
              relocatePanel={relocatePanel}
              onOpenRelocate={onOpenRelocate}
            />
          ) : null}
          <div className="detail-actions">
            <button
              type="button"
              className="toolbar-button danger-button"
              disabled={
                removePanel.isOpen || removePanel.activity === "planning"
              }
              onClick={onOpenRemove}
            >
              {t("library.detail.remove")}
            </button>
          </div>
          <dl className="metadata-grid">
            <div>
              <dt>{t("library.detail.source")}</dt>
              <dd>{sourceDetailLabel(t, detail)}</dd>
            </div>
            <div>
              <dt>{t("library.detail.final_entity")}</dt>
              <dd className="path-value">{detail.finalEntityPath}</dd>
            </div>
            <div>
              <dt>{t("library.detail.directory_identity")}</dt>
              <dd>{detail.directoryName}</dd>
            </div>
            <div>
              <dt>{t("library.detail.last_activity")}</dt>
              <dd>{formatDateTime(locale, detail.lastActivityAt)}</dd>
            </div>
          </dl>
          {detail.frontmatterName &&
          detail.frontmatterName !== detail.directoryName ? (
            <p className="name-notice">
              {t("library.detail.agent_visible_name")}{" "}
              <code>{detail.frontmatterName}</code>
            </p>
          ) : null}
          <section className="document-preview" aria-labelledby="preview-title">
            <div className="document-toolbar">
              <div>
                <span className="document-dot" />
                <h3 id="preview-title">SKILL.md</h3>
              </div>
              <span>{t("library.detail.read_only")}</span>
            </div>
            <pre>{detail.skillMarkdown}</pre>
          </section>
        </>
      ) : error ? (
        <div className="detail-state detail-state--error" role="alert">
          <h2>{t("library.detail.unavailable")}</h2>
          <p>{t("library.detail.unavailable_body")}</p>
        </div>
      ) : libraryEmpty ? (
        <div className="detail-state">
          <h2>{t("library.detail.empty")}</h2>
          <p>{t("library.detail.empty_body")}</p>
        </div>
      ) : (
        <LoadingPanel label={t("library.detail.loading")} />
      )}
    </main>
  );
}

function AgentInspector({
  ref,
  dialog,
  detail,
  agents,
  error,
  pendingAgentId,
  isApplying,
  isChecking,
  onRequest,
  onRepair,
}: {
  ref: Ref<HTMLElement | null>;
  dialog: boolean;
  detail: SkillDetail | null;
  agents: AgentActivation[];
  error: string | null;
  pendingAgentId: string | null;
  isApplying: boolean;
  isChecking: boolean;
  onRequest: (agentId: string, enabled: boolean) => void;
  onRepair: (agentId: string) => void;
}) {
  const { t } = useLocale();
  return (
    <aside
      ref={ref}
      id={dialog ? "agent-inspector-dialog" : undefined}
      className="agent-inspector"
      aria-label={t("library.activation.enable_by_agent")}
      role={dialog ? "dialog" : undefined}
      aria-modal={dialog ? true : undefined}
      // Constant tabindex keeps the focused element stable across breakpoint
      // changes: Chrome blurs an element whose tabindex attribute is removed.
      tabIndex={-1}
    >
      <div className="panel-heading inspector-heading">
        <div>
          <span className="eyebrow">{t("library.activation.eyebrow")}</span>
          <h2>{t("library.activation.enable_by_agent")}</h2>
        </div>
      </div>
      <p className="inspector-intro">{t("library.activation.intro")}</p>
      {error ? (
        <div className="activation-error" role="alert">
          <strong>{t("library.activation.unchanged")}</strong>
          <span>{error}</span>
        </div>
      ) : null}
      <div className="agent-list">
        {detail && isChecking ? (
          <div className="activation-checking" role="status">
            {t("library.activation.checking")}
          </div>
        ) : detail ? (
          agents.map((agent) => {
            const isPending = pendingAgentId === agent.id;
            return (
              <div
                className={`agent-row${isPending || isApplying ? " agent-row--busy" : ""}`}
                key={agent.id}
              >
                <div className="agent-row-top">
                  <span className="agent-monogram" aria-hidden="true">
                    {agent.name.slice(0, 1)}
                  </span>
                  <span className="agent-copy">
                    <strong>{agent.name}</strong>
                    <small>{agent.skillsPath}</small>
                  </span>
                  <label
                    className={`switch-control${agent.detected ? " switch-control--interactive" : ""}`}
                  >
                    <input
                      id={activationControlId(detail.id, agent.id)}
                      type="checkbox"
                      role="switch"
                      aria-label={t("library.activation.enable_label", {
                        skill: detail.directoryName,
                        agent: agent.name,
                      })}
                      checked={agent.desiredEnabled}
                      disabled={!agent.detected || isPending || isApplying}
                      onChange={(event) =>
                        onRequest(agent.id, event.currentTarget.checked)
                      }
                    />
                    <span aria-hidden="true" />
                  </label>
                </div>
                <div className="agent-status">
                  <span
                    className={`agent-state agent-state--${activationTone(agent)}`}
                  >
                    {isPending
                      ? t("library.activation.preparing")
                      : activationLabel(agent, t)}
                  </span>
                  {agent.compatibility === "unknown" ? (
                    <span className="compatibility-note">
                      {t("library.activation.compat_unknown")}
                    </span>
                  ) : null}
                  {agent.detected &&
                  agent.desiredEnabled &&
                  (agent.observedState === "missing" ||
                    agent.observedState === "occupied") ? (
                    <button
                      id={activationRepairControlId(detail.id, agent.id)}
                      type="button"
                      className="repair-button"
                      disabled={isPending || isApplying}
                      onClick={() => onRepair(agent.id)}
                    >
                      {agent.observedState === "occupied"
                        ? t("library.activation.conflict")
                        : t("library.activation.repair")}
                    </button>
                  ) : null}
                </div>
              </div>
            );
          })
        ) : (
          <div className="inspector-empty">
            <span>{t("library.activation.no_selection")}</span>
            <small>{t("library.activation.no_selection_body")}</small>
          </div>
        )}
      </div>
      <div className="inspector-footnote">
        <LockIcon />
        <span>{t("library.activation.footnote")}</span>
      </div>
    </aside>
  );
}

function LinkImportSheet({
  kind,
  preview,
  result,
  error,
  activity,
  sourceGroupType,
  sourceGroupUrl,
  sourceGroupRef,
  sourceGroupOutcome,
  sourceTransitionResult,
  sourceGroupError,
  sourceGroupActivity,
  onKindChange,
  onPreview,
  onApply,
  onClose,
  onOpenImportedSkill,
  onSourceGroupTypeChange,
  onSourceGroupUrlChange,
  onSourceGroupRefChange,
  onFetchLatestAndManage,
  onConfirmSourceTransition,
  onUndoSourceTransition,
}: {
  kind: ImportKind;
  preview: LinkImportPreview | null;
  result: LinkImportResult | null;
  error: string | null;
  activity: "idle" | "discovering" | "applying";
  sourceGroupType: GitRepositorySourceType;
  sourceGroupUrl: string;
  sourceGroupRef: string;
  sourceGroupOutcome: SourceGroupPreviewOutcome | null;
  sourceTransitionResult: SourceTransitionResult | null;
  sourceGroupError: string | null;
  sourceGroupActivity: "idle" | "fetching" | "confirming" | "undoing";
  onKindChange: (kind: ImportKind) => void;
  onPreview: (sourcePath: string) => void;
  onApply: () => void;
  onClose: () => void;
  onOpenImportedSkill: () => void;
  onSourceGroupTypeChange: (sourceType: GitRepositorySourceType) => void;
  onSourceGroupUrlChange: (sourceUrl: string) => void;
  onSourceGroupRefChange: (trackingRef: string) => void;
  onFetchLatestAndManage: () => void;
  onConfirmSourceTransition: () => void;
  onUndoSourceTransition: () => void;
}) {
  const { t } = useLocale();
  const [sourcePath, setSourcePath] = useState("");
  const sourceInput = useRef<HTMLInputElement>(null);
  const primaryButton = useRef<HTMLButtonElement>(null);
  const isDiscovering = activity === "discovering";
  const isApplying = activity === "applying";
  const isRunning = activity !== "idle" || sourceGroupActivity !== "idle";
  const isGit = kind === "git";
  const sourceGroupIsFetching = sourceGroupActivity !== "idle";
  const gitStep = sourceTransitionResult
    ? "result"
    : sourceGroupOutcome?.kind === "preview"
      ? "preview"
      : "source";
  const currentStep = isGit
    ? gitStep
    : result
      ? "result"
      : preview
        ? "preview"
        : isDiscovering
          ? "discover"
          : "source";

  useLayoutEffect(() => {
    if (
      result ||
      preview ||
      sourceTransitionResult ||
      sourceGroupOutcome?.kind === "preview"
    ) {
      primaryButton.current?.focus();
    } else {
      sourceInput.current?.focus();
    }
  }, [preview, result, sourceTransitionResult, sourceGroupOutcome]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isApplying && !sourceGroupIsFetching)
        onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isApplying, sourceGroupIsFetching, onClose]);

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (
          event.currentTarget === event.target &&
          !isApplying &&
          !sourceGroupIsFetching
        )
          onClose();
      }}
    >
      <section
        className="activation-sheet import-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={
          result
            ? t("library.import.dialog_link")
            : isGit
              ? t("library.import.dialog_from_git")
              : t("library.import.dialog_link_import")
        }
      >
        <ol
          className="import-progress"
          aria-label={t("library.import.progress_label")}
        >
          {(["source", "discover", "preview", "result"] as const).map(
            (step) => (
              <li
                key={step}
                aria-current={currentStep === step ? "step" : undefined}
              >
                {t(`library.import.step.${step}` as MessageKey)}
              </li>
            ),
          )}
        </ol>
        {isGit ? (
          <>
            <SourceKindSwitch kind={kind} onKindChange={onKindChange} />
            <SourceGroupPreviewFlow
              sourceType={sourceGroupType}
              sourceUrl={sourceGroupUrl}
              trackingRef={sourceGroupRef}
              outcome={sourceGroupOutcome}
              result={sourceTransitionResult}
              error={sourceGroupError}
              activity={sourceGroupActivity}
              onSourceTypeChange={onSourceGroupTypeChange}
              onSourceUrlChange={onSourceGroupUrlChange}
              onTrackingRefChange={onSourceGroupRefChange}
              onFetch={onFetchLatestAndManage}
              onConfirm={onConfirmSourceTransition}
              onUndo={onUndoSourceTransition}
              onClose={onClose}
            />
          </>
        ) : result ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.import.complete_eyebrow")}
              </span>
              <h2>
                {t("library.import.link_managed", {
                  name: result.directoryName,
                })}
              </h2>
              <p>{t("library.import.link_managed_body")}</p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>{t("library.import.final_entity")}</dt>
                <dd>{result.finalEntityPath}</dd>
              </div>
              <div>
                <dt>{t("library.import.storage")}</dt>
                <dd>{t("library.import.pointer_only")}</dd>
              </div>
            </dl>
            <div className="activation-sheet-actions import-result-actions">
              <button type="button" disabled={isApplying} onClick={onClose}>
                {t("library.import.close")}
              </button>
              <button type="button" onClick={onOpenImportedSkill}>
                {t("library.import.view_in_library")}
              </button>
              <button
                ref={primaryButton}
                type="button"
                className="activation-confirm-button"
                onClick={onOpenImportedSkill}
              >
                {t("library.import.enable_by_agent")}
              </button>
            </div>
          </>
        ) : preview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.import.link_preview_eyebrow")}
              </span>
              <h2>
                {t("library.import.preview_name", {
                  name: preview.directoryName,
                })}
              </h2>
              <p>{t("library.import.link_preview_body")}</p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>{t("library.import.selected_source")}</dt>
                <dd>{preview.sourceEntryPath}</dd>
              </div>
              <div>
                <dt>{t("library.import.final_entity")}</dt>
                <dd>{preview.finalEntityPath}</dd>
              </div>
              <div>
                <dt>{t("library.import.storage")}</dt>
                <dd>{t("library.import.pointer_only")}</dd>
              </div>
            </dl>
            <div className="activation-warning import-risk" role="status">
              <strong>{t("library.import.review_instructions")}</strong>
              <span>{t("library.import.review_link_body")}</span>
            </div>
            {preview.conflict ? (
              <div className="import-conflict" role="alert">
                <strong>{t("library.import.conflict_heading")}</strong>
                <span>
                  {t("library.import.conflict_body", {
                    name: preview.conflict.directoryName,
                  })}
                </span>
              </div>
            ) : null}
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.import.unchanged")}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isApplying} onClick={onClose}>
                {t("library.import.cancel")}
              </button>
              <button
                ref={primaryButton}
                type="button"
                className="activation-confirm-button"
                disabled={!preview.canApply || isRunning}
                onClick={onApply}
              >
                {isRunning
                  ? t("library.import.importing")
                  : t("library.import.import_button", {
                      name: preview.directoryName,
                    })}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.import.link_eyebrow")}
              </span>
              <h2>{t("library.import.link_title")}</h2>
              <p>{t("library.import.link_body")}</p>
            </div>
            <SourceKindSwitch kind={kind} onKindChange={onKindChange} />
            <label className="import-source-field">
              <span>{t("library.import.local_path")}</span>
              <input
                ref={sourceInput}
                type="text"
                value={sourcePath}
                disabled={isRunning}
                placeholder="~/Projects/my-skill"
                onChange={(event) => setSourcePath(event.currentTarget.value)}
              />
            </label>
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.import.source_unavailable")}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isApplying} onClick={onClose}>
                {t("library.import.cancel")}
              </button>
              <button
                ref={primaryButton}
                type="button"
                className="activation-confirm-button"
                disabled={!sourcePath.trim() || isRunning}
                onClick={() => onPreview(sourcePath)}
              >
                {isDiscovering
                  ? t("library.import.checking_source")
                  : t("library.import.preview_link")}
              </button>
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function SourceKindSwitch({
  kind,
  onKindChange,
}: {
  kind: ImportKind;
  onKindChange: (kind: ImportKind) => void;
}) {
  const { t } = useLocale();
  return (
    <div
      className="import-kind-switch"
      role="group"
      aria-label={t("library.import.source_label")}
    >
      <button
        type="button"
        aria-pressed={kind === "link"}
        onClick={() => onKindChange("link")}
      >
        {t("library.import.link_local")}
      </button>
      <button
        type="button"
        aria-pressed={kind === "git"}
        onClick={() => onKindChange("git")}
      >
        {t("library.import.install_git")}
      </button>
    </div>
  );
}

function ActivationPreviewSheet({
  preview,
  isApplying,
  onApply,
  onCancel,
}: {
  preview: ActivationPreview;
  isApplying: boolean;
  onApply: () => void;
  onCancel: () => void;
}) {
  const { t } = useLocale();
  const action = t(`library.activation.kind.${preview.kind}` as MessageKey);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const confirmButton = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    confirmButton.current?.focus();
  }, []);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isApplying) onCancel();
      if (
        event.key === "Tab" &&
        !event.shiftKey &&
        document.activeElement === confirmButton.current
      ) {
        event.preventDefault();
        cancelButton.current?.focus();
      } else if (
        event.key === "Tab" &&
        event.shiftKey &&
        document.activeElement === cancelButton.current
      ) {
        event.preventDefault();
        confirmButton.current?.focus();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isApplying, onCancel]);

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isApplying) onCancel();
      }}
    >
      <section
        className="activation-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.activation_preview.dialog", { action })}
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.activation_preview.eyebrow")}
          </span>
          <h2>
            {action} {preview.skillDirectoryName}
          </h2>
          <p>
            {preview.kind === "repair"
              ? t("library.activation_preview.recreate", {
                  agent: preview.agentName,
                })
              : t(
                  preview.enabled
                    ? "library.activation_preview.create"
                    : "library.activation_preview.remove",
                  { agent: preview.agentName },
                )}
          </p>
        </div>
        <dl className="activation-paths">
          <div>
            <dt>{t("library.activation_preview.agent_entry")}</dt>
            <dd>{preview.entryPath}</dd>
          </div>
          <div>
            <dt>{t("library.activation_preview.final_entity")}</dt>
            <dd>{preview.targetPath}</dd>
          </div>
        </dl>
        {preview.compatibilityWarning ? (
          <div className="activation-warning" role="status">
            <strong>{t("library.activation_preview.compat_confirm")}</strong>
            <span>
              {compatibilityWarningText(t, preview.compatibilityWarning)}
            </span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button
            ref={cancelButton}
            type="button"
            disabled={isApplying}
            onClick={onCancel}
          >
            {t("library.activation_preview.cancel")}
          </button>
          <button
            ref={confirmButton}
            type="button"
            className="activation-confirm-button"
            disabled={isApplying}
            onClick={onApply}
          >
            {isApplying
              ? t("library.activation_preview.applying")
              : t("library.activation_preview.apply", {
                  action,
                  agent: preview.agentName,
                })}
          </button>
        </div>
      </section>
    </div>
  );
}

function ActivationConflictSheet({
  details,
  message,
  replacePreview,
  replaceResult,
  replaceUndo,
  error,
  isApplying,
  isUndoing,
  onAdopt,
  onPlanReplace,
  onApplyReplace,
  onUndoReplace,
  onClose,
}: {
  details: ActivationConflictDetails;
  message: string | null;
  replacePreview: ActivationReplacePreview | null;
  replaceResult: ActivationResult | null;
  replaceUndo: ActivationReplaceUndoResult | null;
  error: string | null;
  isApplying: boolean;
  isUndoing: boolean;
  onAdopt: (canonicalEntity: string) => void;
  onPlanReplace: () => void;
  onApplyReplace: () => void;
  onUndoReplace: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const isBusy = isApplying || isUndoing;
  const sheet = useRef<HTMLElement>(null);
  const replaceButton = useRef<HTMLButtonElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const step = replaceResult
    ? "result"
    : replacePreview
      ? "preview"
      : "conflict";
  const occupier = details.occupier;

  useLayoutEffect(() => {
    if (step === "preview") {
      replaceButton.current?.focus();
    } else if (step === "result" && !replaceUndo) {
      replaceButton.current?.focus();
    } else {
      closeButton.current?.focus();
    }
  }, [step, replaceUndo]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isBusy) onClose();
      if (event.key === "Tab") {
        const buttons = Array.from(
          sheet.current?.querySelectorAll("button") ?? [],
        );
        if (buttons.length === 0) return;
        const first = buttons[0];
        const last = buttons[buttons.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          (last as HTMLButtonElement).focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          (first as HTMLButtonElement).focus();
        }
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isBusy, onClose]);

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isBusy) onClose();
      }}
    >
      <section
        ref={sheet}
        className="activation-sheet activation-conflict-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.conflict.dialog")}
      >
        {step === "conflict" ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">{t("library.conflict.eyebrow")}</span>
              <h2>{t("library.conflict.title")}</h2>
              <p>{t("library.conflict.body")}</p>
            </div>
            {message ? (
              <p className="activation-conflict-detail">{message}</p>
            ) : null}
            <dl className="activation-paths">
              <div>
                <dt>{t("library.conflict.agent_entry")}</dt>
                <dd>{details.entryPath}</dd>
              </div>
              <div>
                <dt>{t("library.conflict.would_point_to")}</dt>
                <dd>{details.targetPath}</dd>
              </div>
            </dl>
            <div className="conflict-occupier">
              <strong>{occupierHeading(occupier, t)}</strong>
              <span>{occupierDescription(occupier, t)}</span>
              {occupier.adoptable ? null : occupier.notAdoptableReason ? (
                <small className="candidate-conflict" role="status">
                  {occupierNotAdoptableReasonText(
                    t,
                    occupier.notAdoptableReason,
                  )}
                </small>
              ) : null}
            </div>
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.conflict.replace_unchanged")}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button
                ref={closeButton}
                type="button"
                disabled={isBusy}
                onClick={onClose}
              >
                {t("library.conflict.cancel")}
              </button>
              <button
                type="button"
                disabled={!occupier.adoptable || !occupier.finalEntityPath}
                onClick={() =>
                  occupier.finalEntityPath
                    ? onAdopt(occupier.finalEntityPath)
                    : undefined
                }
              >
                {t("library.conflict.adopt_existing")}
              </button>
              <button
                ref={replaceButton}
                type="button"
                className="activation-confirm-button"
                disabled={isBusy}
                onClick={onPlanReplace}
              >
                {t("library.conflict.remove_then_replace")}
              </button>
            </div>
          </>
        ) : step === "preview" && replacePreview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.conflict.preview_eyebrow")}
              </span>
              <h2>
                {t("library.conflict.preview_title", {
                  name: replacePreview.skillDirectoryName,
                })}
              </h2>
              <p>
                {t("library.conflict.preview_body", {
                  agent: replacePreview.agentName,
                })}
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>{t("library.activation_preview.agent_entry")}</dt>
                <dd>{replacePreview.entryPath}</dd>
              </div>
              <div>
                <dt>{t("library.activation_preview.final_entity")}</dt>
                <dd>{replacePreview.targetPath}</dd>
              </div>
              <div>
                <dt>{t("library.conflict.temp_backup")}</dt>
                <dd>{replacePreview.backupPath}</dd>
              </div>
            </dl>
            <div className="activation-warning" role="status">
              <strong>{t("library.conflict.confirm_heading")}</strong>
              <span>
                {t("library.conflict.confirm_body", {
                  kind: occupierKindLabel(replacePreview.occupantKind, t),
                })}
              </span>
            </div>
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.conflict.replace_unchanged")}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button
                ref={closeButton}
                type="button"
                disabled={isBusy}
                onClick={onClose}
              >
                {t("library.conflict.cancel")}
              </button>
              <button
                ref={replaceButton}
                type="button"
                className="activation-confirm-button"
                disabled={isBusy}
                onClick={onApplyReplace}
              >
                {isApplying
                  ? t("library.conflict.replacing")
                  : t("library.conflict.replace_button")}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.conflict.result_eyebrow")}
              </span>
              <h2>{t("library.conflict.result_title")}</h2>
              <p>{t("library.conflict.result_body")}</p>
            </div>
            {replaceUndo ? (
              <div
                className={
                  replaceUndo.undone ? "update-result-ok" : "update-result-fail"
                }
                role="status"
              >
                {replaceUndo.undone
                  ? t("library.conflict.undo_restored")
                  : t("library.conflict.undo_failed", {
                      detail:
                        replaceUndo.error ?? t("library.conflict.undo_unknown"),
                    })}
              </div>
            ) : null}
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.conflict.replace_unchanged")}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button
                ref={closeButton}
                type="button"
                disabled={isBusy}
                onClick={onClose}
              >
                {t("library.conflict.close")}
              </button>
              {!replaceUndo ? (
                <button
                  ref={replaceButton}
                  type="button"
                  className="activation-confirm-button"
                  disabled={isBusy}
                  onClick={onUndoReplace}
                >
                  {isUndoing
                    ? t("library.conflict.restoring")
                    : t("library.conflict.restore_button")}
                </button>
              ) : null}
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function occupierHeading(
  occupier: OccupierSummary,
  t: LocaleContextValue["t"],
) {
  if (occupier.isSkill) {
    return t("library.conflict.occupier_skill", {
      name: occupier.directoryName,
    });
  }
  return t("library.conflict.occupier_kind", {
    name: occupier.directoryName,
    kind: occupierKindLabel(occupier.kind, t),
  });
}

function occupierDescription(
  occupier: OccupierSummary,
  t: LocaleContextValue["t"],
) {
  if (occupier.finalEntityPath) {
    return t("library.conflict.resolves_to", {
      path: occupier.finalEntityPath,
    });
  }
  if (occupier.symlinkTarget) {
    return t("library.conflict.symlink_to", {
      path: occupier.symlinkTarget,
    });
  }
  return t("library.conflict.entry_content");
}

function occupierKindLabel(kind: OccupierKind, t: LocaleContextValue["t"]) {
  if (kind === "real_directory")
    return t("library.conflict.kind.real_directory");
  if (kind === "symlink") return t("library.conflict.kind.symlink");
  return t("library.conflict.kind.file");
}

// -- First-run onboarding (spec §8.7) ---------------------------------------

const onboardingSteps: Array<{
  titleKey: MessageKey;
  bodyKey: MessageKey;
}> = [
  {
    titleKey: "library.onboarding.step1_title",
    bodyKey: "library.onboarding.step1_body",
  },
  {
    titleKey: "library.onboarding.step2_title",
    bodyKey: "library.onboarding.step2_body",
  },
  {
    titleKey: "library.onboarding.step3_title",
    bodyKey: "library.onboarding.step3_body",
  },
];

function OnboardingSheet({
  step,
  agents,
  libraryPath,
  report,
  activity,
  error,
  onSkip,
  onAdvance,
  onCreateDirectory,
  onFinishWithAdopt,
}: {
  step: number;
  agents: StartupAgent[];
  libraryPath: string | null;
  report: AdoptEvidenceReport | null;
  activity: "idle" | "checking" | "scanning";
  error: string | null;
  onSkip: () => void;
  onAdvance: () => void;
  onCreateDirectory: (agentId: string) => void;
  onFinishWithAdopt: () => void;
}) {
  const { t, tPlural } = useLocale();
  const closeButton = useRef<HTMLButtonElement>(null);
  const advanceButton = useRef<HTMLButtonElement>(null);
  const isBusy = activity !== "idle";
  const isScanning = activity === "scanning";
  const progressLabel =
    activity === "checking"
      ? t("library.onboarding.checking")
      : t("library.onboarding.scanning");
  const candidates = report?.candidates ?? [];

  useLayoutEffect(() => {
    if (step === 2 && !isBusy) advanceButton.current?.focus();
    else closeButton.current?.focus();
  }, [step, isBusy]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isBusy) onSkip();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isBusy, onSkip]);

  const current = onboardingSteps[step];
  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isBusy) onSkip();
      }}
    >
      <section
        className="activation-sheet onboarding-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.onboarding.welcome")}
      >
        <div
          className="onboarding-progress"
          aria-label={t("library.onboarding.progress")}
        >
          {onboardingSteps.map((item, index) => (
            <span
              key={item.titleKey}
              className={index <= step ? "onboarding-progress-dot--active" : ""}
            >
              {index + 1}
            </span>
          ))}
        </div>
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t("library.onboarding.first_run", { step: step + 1 })}
          </span>
          <h2>{t(current.titleKey)}</h2>
          <p>{t(current.bodyKey)}</p>
        </div>
        {step === 0 && libraryPath ? (
          <dl className="activation-paths">
            <div>
              <dt>{t("library.pane.library")}</dt>
              <dd>{libraryPath}</dd>
            </div>
          </dl>
        ) : null}
        {isBusy ? (
          <IndeterminateProgress
            className="onboarding-operation-progress"
            label={progressLabel}
          />
        ) : null}
        {step === 1 ? (
          <ul className="onboarding-agent-list">
            {agents.map((agent) => (
              <li key={agent.id}>
                <span className="agent-monogram" aria-hidden="true">
                  {agent.name.slice(0, 1)}
                </span>
                <span className="agent-copy">
                  <strong>{agent.name}</strong>
                  <small>{agent.skillsPath}</small>
                </span>
                {agent.detected ? (
                  <span className="candidate-clear">
                    {t("library.onboarding.detected")}
                  </span>
                ) : (
                  <>
                    <span className="candidate-conflict">
                      {t("library.onboarding.not_detected")}
                    </span>
                    <button
                      type="button"
                      disabled={isBusy}
                      onClick={() => onCreateDirectory(agent.id)}
                    >
                      {t("library.onboarding.create_dir")}
                    </button>
                  </>
                )}
              </li>
            ))}
          </ul>
        ) : null}
        {step === 2 ? (
          <div className="onboarding-scan">
            {!isScanning && report ? (
              <>
                <p role="status">
                  {tPlural("library.onboarding.untracked", candidates.length)}
                </p>
                <ul className="onboarding-untracked-list">
                  {candidates.map((candidate) => (
                    <li key={candidate.canonicalEntity}>
                      <span className="onboarding-untracked-copy">
                        <strong>{candidate.directoryName}</strong>
                        <span className="candidate-path">
                          {candidate.canonicalEntity}
                        </span>
                      </span>
                      <span
                        className={`onboarding-untracked-verdict adopt-verdict-${candidate.verdict}`}
                      >
                        <span>
                          {t(
                            `library.adopt.verdict.${candidate.verdict}` as MessageKey,
                          )}
                        </span>
                        {candidate.requiresRelocation ? (
                          <span className="onboarding-untracked-note">
                            {t("library.adopt.relocation_required")}
                          </span>
                        ) : (
                          ""
                        )}
                      </span>
                    </li>
                  ))}
                </ul>
                {candidates.length === 0 ? (
                  <p role="status">{t("library.onboarding.none")}</p>
                ) : null}
              </>
            ) : null}
          </div>
        ) : null}
        {error ? (
          <div className="activation-error" role="alert">
            <strong>{t("library.onboarding.unchanged")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button
            ref={closeButton}
            type="button"
            disabled={isBusy}
            onClick={onSkip}
          >
            {t("library.onboarding.skip")}
          </button>
          {step < 2 ? (
            <button
              ref={advanceButton}
              type="button"
              className="activation-confirm-button"
              disabled={isBusy}
              onClick={onAdvance}
            >
              {t("library.onboarding.continue")}
            </button>
          ) : (
            <>
              <button
                ref={advanceButton}
                type="button"
                className="activation-confirm-button"
                disabled={!report || isBusy}
                onClick={onFinishWithAdopt}
              >
                {t("library.onboarding.review_adopt")}
              </button>
            </>
          )}
        </div>
      </section>
    </div>
  );
}

// -- Preferences (spec §10.2, strictly four) --------------------------------

const preferenceRows: Array<{
  key: keyof AppPreferences;
  titleKey: MessageKey;
  noteKey: MessageKey;
}> = [
  {
    key: "launchAtLogin",
    titleKey: "library.preferences.launch_at_login",
    noteKey: "library.preferences.launch_at_login_note",
  },
  {
    key: "showInDock",
    titleKey: "library.preferences.show_in_dock",
    noteKey: "library.preferences.show_in_dock_note",
  },
  {
    key: "checkAppUpdates",
    titleKey: "library.preferences.check_app_updates",
    noteKey: "library.preferences.check_app_updates_note",
  },
  {
    key: "checkSkillUpdates",
    titleKey: "library.preferences.check_skill_updates",
    noteKey: "library.preferences.check_skill_updates_note",
  },
];

function PreferencesSheet({
  preferences,
  warning,
  error,
  appUpdatePanel,
  onToggle,
  onCheckAppUpdate,
  onClose,
}: {
  preferences: AppPreferences | null;
  warning: PreferencesWarning | null;
  error: string | null;
  appUpdatePanel: AppUpdatePanelState;
  onToggle: (updates: PreferenceUpdates) => void;
  onCheckAppUpdate: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const closeButton = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    closeButton.current?.focus();
  }, []);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onClose();
      }}
    >
      <section
        className="activation-sheet preferences-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.preferences.dialog")}
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">{t("library.preferences.eyebrow")}</span>
          <h2>{t("library.preferences.title")}</h2>
          <p>{t("library.preferences.body")}</p>
        </div>
        <LanguageControl />
        <div className="preference-list">
          {preferenceRows.map((row) => (
            <label className="preference-row" key={row.key}>
              <span>
                <strong>{t(row.titleKey)}</strong>
                <small>{t(row.noteKey)}</small>
              </span>
              <span className="switch-control switch-control--interactive">
                <input
                  type="checkbox"
                  role="switch"
                  aria-label={t(row.titleKey)}
                  checked={preferences?.[row.key] ?? false}
                  disabled={preferences === null}
                  onChange={(event) =>
                    onToggle({ [row.key]: event.currentTarget.checked })
                  }
                />
                <span aria-hidden="true" />
              </span>
            </label>
          ))}
        </div>
        <div className="app-update-check">
          <span>
            <strong>{t("library.preferences.app_updates")}</strong>
            <small>{t("library.preferences.app_updates_note")}</small>
          </span>
          <button
            id="app-update-check-trigger"
            type="button"
            disabled={appUpdatePanel.activity === "checking"}
            onClick={onCheckAppUpdate}
          >
            {appUpdatePanel.activity === "checking"
              ? t("library.preferences.checking")
              : t("library.preferences.check_now")}
          </button>
        </div>
        {appUpdatePanel.checkStatus ? (
          <div className="app-update-check-result" role="status">
            {appUpdatePanel.checkStatus === "up_to_date"
              ? t("library.preferences.up_to_date")
              : t("library.preferences.skipped")}
          </div>
        ) : null}
        {appUpdatePanel.error && appUpdatePanel.update === null ? (
          <div className="activation-error" role="alert">
            <strong>{t("library.preferences.check_failed")}</strong>
            <span>{appUpdatePanel.error}</span>
          </div>
        ) : null}
        {warning ? (
          <div className="activation-warning" role="status">
            <strong>{t("library.preferences.applied_warning")}</strong>
            <span>{preferencesWarningText(t, warning)}</span>
          </div>
        ) : null}
        {error ? (
          <div className="activation-error" role="alert">
            <strong>{t("library.preferences.unchanged")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button ref={closeButton} type="button" onClick={onClose}>
            {t("library.preferences.done")}
          </button>
        </div>
      </section>
    </div>
  );
}

function AppUpdateSheet({
  panel,
  onDownload,
  onInstall,
  onClose,
}: {
  panel: AppUpdatePanelState;
  onDownload: () => void;
  onInstall: () => void;
  onClose: () => void;
}) {
  const { t, locale } = useLocale();
  const cancelButton = useRef<HTMLButtonElement>(null);
  const primaryButton = useRef<HTMLButtonElement>(null);
  const update = panel.update;
  const isCancelling = panel.activity === "cancelling";
  const isInstalling = panel.activity === "installing";
  const blocksDismissal = isCancelling || isInstalling;
  const blocksPrimary =
    panel.activity === "downloading" || isCancelling || isInstalling;
  const isReady = panel.activity === "ready";

  useLayoutEffect(() => {
    if (panel.activity === "downloading") {
      cancelButton.current?.focus();
    } else {
      primaryButton.current?.focus();
    }
  }, [panel.activity]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !blocksDismissal) onClose();
      if (
        event.key === "Tab" &&
        !event.shiftKey &&
        document.activeElement === primaryButton.current
      ) {
        event.preventDefault();
        cancelButton.current?.focus();
      } else if (
        event.key === "Tab" &&
        event.shiftKey &&
        document.activeElement === cancelButton.current
      ) {
        event.preventDefault();
        primaryButton.current?.focus();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [blocksDismissal, onClose]);

  if (!update) return null;

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !blocksDismissal) onClose();
      }}
    >
      <section
        className="activation-sheet app-update-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.app_update.dialog")}
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">{t("library.app_update.eyebrow")}</span>
          <h2>
            {t("library.app_update.version", { version: update.version })}
          </h2>
          <p>
            {t("library.app_update.current", {
              version: update.currentVersion,
            })}
          </p>
        </div>
        <dl className="app-update-details">
          <div>
            <dt>{t("library.app_update.archive_size")}</dt>
            <dd>{formatByteSize(locale, update.downloadSizeBytes)}</dd>
          </div>
          <div>
            <dt>{t("library.app_update.release_notes")}</dt>
            <dd>{update.releaseNotes || t("library.app_update.no_notes")}</dd>
          </div>
        </dl>
        {panel.activity === "downloading" ? (
          <div className="app-update-progress" role="status">
            {t("library.app_update.downloading")}
          </div>
        ) : null}
        {isReady ? (
          <div className="app-update-ready" role="status">
            {t("library.app_update.ready")}
          </div>
        ) : null}
        {panel.error ? (
          <div className="activation-error" role="alert">
            <strong>{t("library.app_update.failed")}</strong>
            <span>{panel.error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button
            ref={cancelButton}
            type="button"
            disabled={blocksDismissal}
            onClick={onClose}
          >
            {isCancelling
              ? t("library.app_update.cancelling")
              : panel.activity === "downloading"
                ? t("library.app_update.cancel_download")
                : isReady
                  ? t("library.app_update.later")
                  : t("library.app_update.not_now")}
          </button>
          <button
            ref={primaryButton}
            type="button"
            className="activation-confirm-button"
            disabled={blocksPrimary}
            onClick={isReady ? onInstall : onDownload}
          >
            {isCancelling
              ? t("library.app_update.cancelling")
              : panel.activity === "downloading"
                ? t("library.app_update.downloading_btn")
                : panel.activity === "installing"
                  ? t("library.app_update.installing")
                  : isReady
                    ? t("library.app_update.install_restart")
                    : t("library.app_update.download_update")}
          </button>
        </div>
      </section>
    </div>
  );
}

function RemoveSheet({
  panel,
  onApply,
  onClose,
}: {
  panel: RemovePanelState;
  onApply: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const isBusy = panel.activity !== "idle";
  const confirmButton = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    if (panel.preview || panel.result) {
      confirmButton.current?.focus();
    }
  }, [panel.preview, panel.result]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isBusy) onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isBusy, onClose]);

  const preview = panel.preview;
  const isInstall =
    preview?.sourceKind === "remote_install" ||
    preview?.sourceKind === "file_install";

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isBusy) onClose();
      }}
    >
      <section
        className="activation-sheet import-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.remove.dialog")}
      >
        {panel.result ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.remove.complete_eyebrow")}
              </span>
              <h2>
                {t("library.remove.complete_title", {
                  name: panel.result.directoryName,
                })}
              </h2>
              <p>{t("library.remove.complete_body")}</p>
            </div>
            <div className="activation-sheet-actions">
              <button ref={confirmButton} type="button" onClick={onClose}>
                {t("library.remove.close")}
              </button>
            </div>
          </>
        ) : panel.activity === "planning" ? (
          <LoadingPanel label={t("library.remove.preparing")} />
        ) : preview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.remove.preview_eyebrow")}
              </span>
              <h2>
                {t("library.remove.preview_title", {
                  name: preview.directoryName,
                })}
              </h2>
              <p>{t("library.remove.preview_body")}</p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>{t("library.remove.final_entity")}</dt>
                <dd>{preview.finalEntityPath}</dd>
              </div>
              <div>
                <dt>{t("library.remove.entity_handling")}</dt>
                <dd>
                  {isInstall
                    ? t("library.remove.entity_deleted")
                    : t("library.remove.entity_kept")}
                </dd>
              </div>
              <div>
                <dt>{t("library.remove.activations")}</dt>
                <dd>{preview.activationCount}</dd>
              </div>
            </dl>
            <div className="activation-warning" role="status">
              <strong>{t("library.remove.no_undo")}</strong>
              <span>{t("library.remove.no_undo_body")}</span>
            </div>
            {panel.error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.remove.unchanged")}</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                {t("library.remove.cancel")}
              </button>
              <button
                ref={confirmButton}
                type="button"
                className="activation-confirm-button danger-button"
                disabled={isBusy}
                onClick={onApply}
              >
                {isBusy
                  ? t("library.remove.removing")
                  : t("library.remove.button", {
                      name: preview.directoryName,
                    })}
              </button>
            </div>
          </>
        ) : (
          <>
            {panel.error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.remove.unavailable")}</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button ref={confirmButton} type="button" onClick={onClose}>
                {t("library.remove.close")}
              </button>
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function HealthNotice({
  detail,
  relocatePanel,
  onOpenRelocate,
}: {
  detail: SkillDetail;
  relocatePanel: RelocatePanelState;
  onOpenRelocate: () => void;
}) {
  const { t } = useLocale();
  const broken = detail.health === "broken";
  const isBrokenLink = broken && detail.sourceKind === "link";
  return (
    <div className={`health-notice health-notice--${detail.health}`}>
      <strong>
        {broken
          ? t("library.health.source_unavailable")
          : t("library.health.local_changes")}
      </strong>
      <span>
        {broken
          ? t("library.health.broken_body")
          : t("library.health.modified_body")}
      </span>
      {isBrokenLink ? (
        <button
          type="button"
          className="repair-button"
          disabled={relocatePanel.isOpen}
          onClick={onOpenRelocate}
        >
          {t("library.health.relocate")}
        </button>
      ) : null}
    </div>
  );
}

function RelocateSheet({
  panel,
  onSourcePathChange,
  onPreview,
  onApply,
  onClose,
}: {
  panel: RelocatePanelState;
  onSourcePathChange: (sourcePath: string) => void;
  onPreview: (sourcePath: string) => void;
  onApply: () => void;
  onClose: () => void;
}) {
  const { t, tPlural } = useLocale();
  const isBusy = panel.activity !== "idle";
  const step = panel.result ? "result" : panel.preview ? "preview" : "source";
  const sourceInput = useRef<HTMLInputElement>(null);
  const confirmButton = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    if (panel.result || panel.preview) {
      confirmButton.current?.focus();
    } else {
      sourceInput.current?.focus();
    }
  }, [panel.preview, panel.result]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isBusy) onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isBusy, onClose]);

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isBusy) onClose();
      }}
    >
      <section
        className="activation-sheet import-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("library.relocate.dialog")}
      >
        <ol
          className="import-progress"
          aria-label={t("library.relocate.progress")}
        >
          {(["source", "preview", "result"] as const).map((stepName) => (
            <li
              key={stepName}
              aria-current={step === stepName ? "step" : undefined}
            >
              {t(`library.relocate.step.${stepName}` as MessageKey)}
            </li>
          ))}
        </ol>
        {panel.result ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.relocate.complete_eyebrow")}
              </span>
              <h2>
                {t("library.relocate.complete_title", {
                  name: panel.result.directoryName,
                })}
              </h2>
              <p>
                {tPlural(
                  "library.relocate.complete_body",
                  panel.result.activationCount,
                )}
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>{t("library.relocate.final_entity")}</dt>
                <dd>{panel.result.finalEntityPath}</dd>
              </div>
            </dl>
            <div className="activation-sheet-actions">
              <button type="button" onClick={onClose}>
                {t("library.relocate.close")}
              </button>
            </div>
          </>
        ) : panel.preview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.relocate.preview_eyebrow")}
              </span>
              <h2>
                {t("library.relocate.preview_title", {
                  name: panel.preview.directoryName,
                })}
              </h2>
              <p>{t("library.relocate.preview_body")}</p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>{t("library.relocate.new_source")}</dt>
                <dd>{panel.preview.sourceEntryPath}</dd>
              </div>
              <div>
                <dt>{t("library.relocate.final_entity")}</dt>
                <dd>{panel.preview.finalEntityPath}</dd>
              </div>
              <div>
                <dt>{t("library.relocate.activations")}</dt>
                <dd>{panel.preview.activationCount}</dd>
              </div>
            </dl>
            {panel.error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.relocate.unchanged")}</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                {t("library.relocate.cancel")}
              </button>
              <button
                type="button"
                className="activation-confirm-button"
                disabled={isBusy}
                onClick={onApply}
              >
                {isBusy
                  ? t("library.relocate.relocating")
                  : t("library.relocate.apply")}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">
                {t("library.relocate.source_eyebrow")}
              </span>
              <h2>{t("library.relocate.find_title")}</h2>
              <p>{t("library.relocate.find_body")}</p>
            </div>
            <label className="import-source-field">
              <span>{t("library.relocate.path_label")}</span>
              <input
                ref={sourceInput}
                type="text"
                value={panel.sourcePath}
                disabled={isBusy}
                placeholder="~/Projects/my-skill"
                onChange={(event) =>
                  onSourcePathChange(event.currentTarget.value)
                }
              />
            </label>
            {panel.error ? (
              <div className="activation-error" role="alert">
                <strong>{t("library.relocate.rejected")}</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                {t("library.relocate.cancel")}
              </button>
              <button
                ref={confirmButton}
                type="button"
                className="activation-confirm-button"
                disabled={!panel.sourcePath.trim() || isBusy}
                onClick={() => onPreview(panel.sourcePath)}
              >
                {panel.activity === "previewing"
                  ? t("library.relocate.checking")
                  : t("library.relocate.preview_button")}
              </button>
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function LoadingPanel({ label }: { label: string }) {
  return (
    <div className="loading-panel" role="status">
      <span className="loading-mark" />
      <span>{label}</span>
    </div>
  );
}

function StatusDot({ health }: { health: Health }) {
  return (
    <span className={`status-dot status-dot--${health}`} aria-label={health} />
  );
}

function HealthBadge({ health }: { health: Health }) {
  const { t } = useLocale();
  return (
    <span className={`health-badge health-badge--${health}`}>
      {t(`library.health.badge.${health}` as MessageKey)}
    </span>
  );
}

function sourceKindLabel(sourceKind: SourceKind, t: LocaleContextValue["t"]) {
  if (sourceKind === "link") return t("library.source_kind.link");
  if (sourceKind === "remote_install") return t("library.source_kind.git");
  return t("library.source_kind.file");
}

/** The detail-panel Source row: App Copy composed from the source kind and
 * raw Source Content fields (spec §4.7 — never a native-composed label). */
function sourceDetailLabel(
  t: LocaleContextValue["t"],
  detail: SkillDetail,
): string {
  if (detail.sourceKind === "link") {
    return t("library.source.link", { path: detail.finalEntityPath });
  }
  if (detail.sourceKind === "remote_install") {
    return t("library.source.git");
  }
  return detail.fileSourceOriginalPath
    ? t("library.source.file_path", { path: detail.fileSourceOriginalPath })
    : t("library.source.file");
}

function compatibilityWarningText(
  t: LocaleContextValue["t"],
  warning: CompatibilityWarning,
): string {
  switch (warning.kind) {
    case "custom_unknown":
      return t("library.activation_preview.compat_custom");
    case "frontmatter_mismatch":
      return t("library.activation_preview.compat_mismatch", {
        name: warning.frontmatterName,
        directory: warning.directoryName,
      });
  }
}

function occupierNotAdoptableReasonText(
  t: LocaleContextValue["t"],
  reason: OccupierNotAdoptableReason,
): string {
  switch (reason.kind) {
    case "regular_file":
      return t("library.conflict.reason.regular_file");
    case "points_at_managed_skill":
      return t("library.conflict.reason.points_at_managed");
    case "points_at_this_skill":
      return t("library.conflict.reason.points_at_this");
    case "no_readable_skill_md":
      return t("library.conflict.reason.no_skill_md");
    case "target_unresolvable":
      return t("library.conflict.reason.unresolvable");
    case "identity_conflict":
      return t("library.conflict.reason.identity_conflict", {
        name: reason.directoryName,
      });
  }
}

function preferencesWarningText(
  t: LocaleContextValue["t"],
  warning: PreferencesWarning,
): string {
  switch (warning.kind) {
    case "show_in_dock_failed":
      return warning.detail
        ? `${t("library.preferences.warning.show_in_dock")} ${warning.detail}`
        : t("library.preferences.warning.show_in_dock");
    case "launch_at_login_failed":
      return warning.detail
        ? `${t("library.preferences.warning.launch_at_login")} ${warning.detail}`
        : t("library.preferences.warning.launch_at_login");
  }
}

function activationTone(agent: AgentActivation) {
  if (!agent.desiredEnabled) return "neutral";
  return agent.observedState === "present" ? "healthy" : "warning";
}

function activationControlId(skillId: string, agentId: string) {
  return `activation-${skillId}-${agentId}`;
}

function activationRepairControlId(skillId: string, agentId: string) {
  return `activation-repair-${skillId}-${agentId}`;
}

function activationLabel(agent: AgentActivation, t: LocaleContextValue["t"]) {
  if (!agent.desiredEnabled) return t("library.activation.state.disabled");
  if (agent.observedState === "present") {
    return t("library.activation.state.enabled_present");
  }
  return t("library.activation.state.enabled", {
    state: t(
      `library.activation.observed.${agent.observedState}` as MessageKey,
    ),
  });
}
