import { useEffect, useLayoutEffect, useRef, useState } from "react";

import type {
  AppUpdatePanelState,
  ImportKind,
  RelocatePanelState,
  RemovePanelState,
  UpdatePanelState,
} from "../../app/App";
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
  CatalogFilter,
  GitImportDiscovery,
  GitImportSelectionPreview,
  GitImportSelectionResult,
  Health,
  LinkImportPreview,
  LinkImportResult,
  OccupierKind,
  OccupierSummary,
  PreferenceUpdates,
  SkillDetail,
  SkillSummary,
  SourceKind,
  StartupAgent,
} from "../../app/catalog-client";
import { LockIcon, SettingsIcon } from "../../ui/icons";

const filters: Array<{ value: CatalogFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "broken", label: "Broken" },
  { value: "modified", label: "Modified" },
  { value: "link", label: "Link" },
  { value: "install", label: "Install" },
];

interface LibraryDeskProps {
  filter: CatalogFilter;
  skills: SkillSummary[];
  selectedId: string | null;
  detail: SkillDetail | null;
  agents: AgentActivation[];
  error: string | null;
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
  gitImportSource: string;
  gitImportForceFullDepth: boolean;
  gitImportDiscovery: GitImportDiscovery | null;
  gitImportSelected: string[];
  gitImportPreview: GitImportSelectionPreview | null;
  gitImportResult: GitImportSelectionResult | null;
  gitImportError: string | null;
  gitImportActivity: "idle" | "discovering" | "planning" | "applying";
  updatePanel: UpdatePanelState;
  reselectPath: string;
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
  adoptReport: AdoptScanReport | null;
  adoptSelected: string[];
  adoptPlan: AdoptPlan | null;
  adoptResult: AdoptResult | null;
  adoptUndo: AdoptUndoResult | null;
  adoptError: string | null;
  adoptErrorHeading: string;
  adoptActivity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onFilter: (filter: CatalogFilter) => void;
  onSelect: (skillId: string) => void;
  onRequestActivation: (agentId: string, enabled: boolean) => void;
  onRequestActivationRepair: (agentId: string) => void;
  onApplyActivation: () => void;
  onCancelActivation: () => void;
  onCloseActivationConflict: () => void;
  onAdoptFromConflict: (canonicalEntity: string) => void;
  onPlanReplace: () => void;
  onApplyReplace: () => void;
  onUndoReplace: () => void;
  onOpenLinkImport: () => void;
  onImportKindChange: (kind: ImportKind) => void;
  onPreviewLinkImport: (sourcePath: string) => void;
  onApplyLinkImport: () => void;
  onCloseLinkImport: () => void;
  onOpenImportedSkill: () => void;
  onGitImportSourceChange: (source: string) => void;
  onGitImportForceFullDepthChange: (force: boolean) => void;
  onDiscoverGitImport: (source: string, forceFullDepth: boolean) => void;
  onGitImportSelectionChange: (directoryNames: string[]) => void;
  onPlanGitImport: () => void;
  onApplyGitImport: () => void;
  onOpenImportedGitSkill: (skillId: string) => void;
  onCheckSkillUpdates: () => void;
  onPlanSkillUpdate: (newSkillPath: string | null) => void;
  onApplySkillUpdate: (abandonChanges: boolean) => void;
  onPinSkillUpdate: () => void;
  onReselectPathChange: (path: string) => void;
  onOpenAdopt: () => void;
  onToggleAdoptCandidate: (canonicalEntity: string, checked: boolean) => void;
  onPlanAdopt: () => void;
  onApplyAdopt: () => void;
  onUndoAdopt: () => void;
  onCloseAdopt: () => void;
  isPreferencesOpen: boolean;
  preferences: AppPreferences | null;
  preferencesWarning: string | null;
  preferencesError: string | null;
  appUpdatePanel: AppUpdatePanelState;
  isOnboardingOpen: boolean;
  onboardingStep: number;
  onboardingAgents: StartupAgent[];
  onboardingReport: AdoptScanReport | null;
  onboardingActivity: "idle" | "scanning";
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
  selectedId,
  detail,
  agents,
  error,
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
  gitImportSource,
  gitImportForceFullDepth,
  gitImportDiscovery,
  gitImportSelected,
  gitImportPreview,
  gitImportResult,
  gitImportError,
  gitImportActivity,
  updatePanel,
  reselectPath,
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
  onGitImportSourceChange,
  onGitImportForceFullDepthChange,
  onDiscoverGitImport,
  onGitImportSelectionChange,
  onPlanGitImport,
  onApplyGitImport,
  onOpenImportedGitSkill,
  onCheckSkillUpdates,
  onPlanSkillUpdate,
  onApplySkillUpdate,
  onPinSkillUpdate,
  onReselectPathChange,
  isAdoptOpen,
  adoptReport,
  adoptSelected,
  adoptPlan,
  adoptResult,
  adoptUndo,
  adoptError,
  adoptErrorHeading,
  adoptActivity,
  onOpenAdopt,
  onToggleAdoptCandidate,
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

  return (
    <div className="app-shell">
      <div className="app-background" inert={hasOverlay ? true : undefined}>
        <a className="skip-link" href="#skill-detail">
          Skip to Skill detail
        </a>
        <Toolbar
          onImport={onOpenLinkImport}
          onAdopt={onOpenAdopt}
          onOpenPreferences={onOpenPreferences}
        />
        {error ? (
          <div className="global-notice" role="alert">
            <strong>Library unavailable</strong>
            <span>{error}</span>
          </div>
        ) : null}
        {lockNotice ? (
          <div className="global-notice global-notice--locked" role="alert">
            <strong>Recovery required — writes locked</strong>
            <span>{lockNotice}</span>
            <span>
              Browsing stays available. Repair the cause (permissions or the
              interrupted operation), then retry recovery; check the app logs
              for the failing operation id.
            </span>
            <button
              type="button"
              className="repair-button"
              onClick={onRetryRecovery}
            >
              Retry recovery
            </button>
          </div>
        ) : null}
        <div className="library-desk">
          <LibrarySidebar
            filter={filter}
            skills={skills}
            selectedId={selectedId}
            onFilter={onFilter}
            onSelect={onSelect}
          />
          <SkillDetailPanel
            detail={detail}
            updatePanel={updatePanel}
            reselectPath={reselectPath}
            relocatePanel={relocatePanel}
            onCheckUpdates={onCheckSkillUpdates}
            onPlanUpdate={onPlanSkillUpdate}
            onApplyUpdate={onApplySkillUpdate}
            onPinUpdate={onPinSkillUpdate}
            onReselectPathChange={onReselectPathChange}
            onOpenRelocate={onOpenRelocate}
            removePanel={removePanel}
            onOpenRemove={onOpenRemove}
          />
          <AgentInspector
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
        <AdoptSheet
          report={adoptReport}
          selected={adoptSelected}
          plan={adoptPlan}
          result={adoptResult}
          undo={adoptUndo}
          error={adoptError}
          errorHeading={adoptErrorHeading}
          activity={adoptActivity}
          onToggle={onToggleAdoptCandidate}
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
          gitImportSource={gitImportSource}
          gitImportForceFullDepth={gitImportForceFullDepth}
          gitImportDiscovery={gitImportDiscovery}
          gitImportSelected={gitImportSelected}
          gitImportPreview={gitImportPreview}
          gitImportResult={gitImportResult}
          gitImportError={gitImportError}
          gitImportActivity={gitImportActivity}
          onKindChange={onImportKindChange}
          onPreview={onPreviewLinkImport}
          onApply={onApplyLinkImport}
          onClose={onCloseLinkImport}
          onOpenImportedSkill={onOpenImportedSkill}
          onGitImportSourceChange={onGitImportSourceChange}
          onGitImportForceFullDepthChange={onGitImportForceFullDepthChange}
          onDiscoverGitImport={onDiscoverGitImport}
          onGitImportSelectionChange={onGitImportSelectionChange}
          onPlanGitImport={onPlanGitImport}
          onApplyGitImport={onApplyGitImport}
          onOpenImportedGitSkill={onOpenImportedGitSkill}
        />
      ) : null}
      {isOnboardingOpen ? (
        <OnboardingSheet
          step={onboardingStep}
          agents={onboardingAgents}
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
  onImport,
  onAdopt,
  onOpenPreferences,
}: {
  onImport: () => void;
  onAdopt: () => void;
  onOpenPreferences: () => void;
}) {
  return (
    <header className="toolbar">
      <div className="product-mark" aria-hidden="true">
        <span />
        <span />
        <span />
      </div>
      <div className="toolbar-title">
        <strong>Skill Man</strong>
        <span>Library Desk</span>
      </div>
      <div className="toolbar-actions" aria-label="Library actions">
        <button type="button" className="toolbar-button" disabled>
          Health check
        </button>
        <button type="button" className="toolbar-button" onClick={onAdopt}>
          Adopt
        </button>
        <button
          id="link-import-trigger"
          type="button"
          className="primary-button"
          onClick={onImport}
        >
          Import
        </button>
        <button
          id="preferences-trigger"
          type="button"
          className="icon-button"
          aria-label="Preferences"
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
  return (
    <nav className="library-sidebar" aria-label="Library">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Managed</span>
          <h1>Library</h1>
        </div>
        <span
          className="count-badge"
          aria-label={`${skills.length} visible Skills`}
        >
          {skills.length}
        </span>
      </div>
      <div className="filter-strip" aria-label="Filter Library">
        {filters.map((item) => (
          <button
            type="button"
            className="filter-chip"
            aria-pressed={filter === item.value}
            key={item.value}
            onClick={() => onFilter(item.value)}
          >
            {item.label}
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
                aria-label={`${skill.enabledAgentCount} Agents`}
              >
                {skill.enabledAgentCount}
              </span>
            </button>
          ))
        ) : (
          <div className="empty-list">
            <span>No matching Skills</span>
            <small>Choose another filter to continue browsing.</small>
          </div>
        )}
      </div>
    </nav>
  );
}

function SkillDetailPanel({
  detail,
  updatePanel,
  reselectPath,
  relocatePanel,
  onCheckUpdates,
  onPlanUpdate,
  onApplyUpdate,
  onPinUpdate,
  onReselectPathChange,
  onOpenRelocate,
  removePanel,
  onOpenRemove,
}: {
  detail: SkillDetail | null;
  updatePanel: UpdatePanelState;
  reselectPath: string;
  relocatePanel: RelocatePanelState;
  onCheckUpdates: () => void;
  onPlanUpdate: (newSkillPath: string | null) => void;
  onApplyUpdate: (abandonChanges: boolean) => void;
  onPinUpdate: () => void;
  onReselectPathChange: (path: string) => void;
  onOpenRelocate: () => void;
  removePanel: RemovePanelState;
  onOpenRemove: () => void;
}) {
  return (
    <main id="skill-detail" className="skill-detail" aria-label="Skill detail">
      {detail ? (
        <>
          <div className="detail-heading">
            <div className="detail-badges">
              <HealthBadge health={detail.health} />
              <span className="source-badge">
                {sourceKindLabel(detail.sourceKind)}
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
              Remove…
            </button>
          </div>
          <dl className="metadata-grid">
            <div>
              <dt>Source</dt>
              <dd>{detail.sourceLabel}</dd>
            </div>
            <div>
              <dt>Final entity</dt>
              <dd className="path-value">{detail.finalEntityPath}</dd>
            </div>
            <div>
              <dt>Directory identity</dt>
              <dd>{detail.directoryName}</dd>
            </div>
            <div>
              <dt>Last activity</dt>
              <dd>{formatActivity(detail.lastActivityAt)}</dd>
            </div>
          </dl>
          {detail.frontmatterName &&
          detail.frontmatterName !== detail.directoryName ? (
            <p className="name-notice">
              Agent-visible name: <code>{detail.frontmatterName}</code>
            </p>
          ) : null}
          {detail.sourceKind === "remote_install" ? (
            <UpdateSection
              skill={detail}
              updatePanel={updatePanel}
              reselectPath={reselectPath}
              onCheckUpdates={onCheckUpdates}
              onPlanUpdate={onPlanUpdate}
              onApplyUpdate={onApplyUpdate}
              onPinUpdate={onPinUpdate}
              onReselectPathChange={onReselectPathChange}
            />
          ) : null}
          <section className="document-preview" aria-labelledby="preview-title">
            <div className="document-toolbar">
              <div>
                <span className="document-dot" />
                <h3 id="preview-title">SKILL.md</h3>
              </div>
              <span>Read only</span>
            </div>
            <pre>{detail.skillMarkdown}</pre>
          </section>
        </>
      ) : (
        <LoadingPanel label="Loading Skill detail" />
      )}
    </main>
  );
}

function UpdateSection({
  skill,
  updatePanel,
  reselectPath,
  onCheckUpdates,
  onPlanUpdate,
  onApplyUpdate,
  onPinUpdate,
  onReselectPathChange,
}: {
  skill: SkillDetail;
  updatePanel: UpdatePanelState;
  reselectPath: string;
  onCheckUpdates: () => void;
  onPlanUpdate: (newSkillPath: string | null) => void;
  onApplyUpdate: (abandonChanges: boolean) => void;
  onPinUpdate: () => void;
  onReselectPathChange: (path: string) => void;
}) {
  const isBusy = updatePanel.activity !== "idle";
  const item = updatePanel.report?.groups
    .flatMap((group) => group.items)
    .find((candidate) => candidate.skillId === skill.id);
  const planItem = updatePanel.plan?.items.find(
    (candidate) => candidate.skillId === skill.id,
  );
  const resultItem = updatePanel.result?.items.find(
    (candidate) => candidate.skillId === skill.id,
  );
  const hasChecked = updatePanel.report !== null;

  return (
    <section className="update-section" aria-labelledby="update-title">
      <div className="update-toolbar">
        <h3 id="update-title">Updates</h3>
        {!hasChecked ? (
          <button type="button" disabled={isBusy} onClick={onCheckUpdates}>
            {updatePanel.activity === "checking"
              ? "Checking"
              : "Check for updates"}
          </button>
        ) : null}
      </div>
      {updatePanel.error ? (
        <div className="activation-error" role="alert">
          <strong>Update check failed</strong>
          <span>{updatePanel.error}</span>
        </div>
      ) : null}
      {!hasChecked ? null : item ? (
        <div className="update-status">
          {item.hasUpdate ? (
            <p className="update-available" role="status">
              Update available: <code>{shortCommit(item.currentCommit)}</code> →{" "}
              <code>{shortCommit(item.resolvedCommit)}</code>
            </p>
          ) : (
            <p role="status">This Skill is up to date.</p>
          )}
          {item.upstreamPathGone ? (
            <div className="update-path-gone" role="alert">
              <strong>The upstream Skill path no longer exists</strong>
              <span>
                Choose the new location inside the repository, keep the current
                version, or Remove the Skill later.
              </span>
              <div className="reselect-row">
                <input
                  type="text"
                  value={reselectPath}
                  placeholder="packages/skills/new-name"
                  aria-label="New repository path"
                  onChange={(event) =>
                    onReselectPathChange(event.currentTarget.value)
                  }
                />
                <button
                  type="button"
                  disabled={isBusy || !reselectPath.trim()}
                  onClick={() => onPlanUpdate(reselectPath.trim())}
                >
                  {updatePanel.activity === "planning"
                    ? "Planning"
                    : "Reselect and update"}
                </button>
                <button type="button" disabled={isBusy} onClick={onPinUpdate}>
                  {updatePanel.activity === "pinning"
                    ? "Pinning"
                    : "Keep current version"}
                </button>
              </div>
            </div>
          ) : null}
          {!item.hasUpdate || item.upstreamPathGone ? null : planItem ? (
            <div className="update-plan">
              {planItem.error ? (
                <p className="update-plan-error" role="alert">
                  {planItem.error}
                </p>
              ) : (
                <>
                  {planItem.modified ? (
                    <p className="update-modified" role="alert">
                      <strong>Local changes detected</strong>
                      <span>
                        Updating will abandon the local modifications to this
                        Skill.
                      </span>
                    </p>
                  ) : null}
                  <div className="reselect-row">
                    <button
                      type="button"
                      disabled={isBusy}
                      onClick={() => onApplyUpdate(planItem.modified)}
                    >
                      {updatePanel.activity === "applying"
                        ? "Updating"
                        : planItem.modified
                          ? "Abandon changes and update"
                          : "Update"}
                    </button>
                    <button
                      type="button"
                      disabled={isBusy}
                      onClick={() => onPlanUpdate(null)}
                    >
                      Cancel
                    </button>
                  </div>
                </>
              )}
            </div>
          ) : item.hasUpdate && !item.upstreamPathGone ? (
            <div className="reselect-row">
              <button
                type="button"
                disabled={isBusy}
                onClick={() => onPlanUpdate(null)}
              >
                {updatePanel.activity === "planning" ? "Planning" : "Update"}
              </button>
              {item.modified ? null : (
                <button type="button" disabled={isBusy} onClick={onPinUpdate}>
                  {updatePanel.activity === "pinning"
                    ? "Pinning"
                    : "Keep current version"}
                </button>
              )}
            </div>
          ) : null}
          {resultItem ? (
            <p
              className={
                resultItem.updated ? "update-result-ok" : "update-result-fail"
              }
              role="status"
            >
              {resultItem.updated
                ? "Update applied."
                : `Update failed: ${resultItem.error ?? "unknown error"}`}
            </p>
          ) : null}
        </div>
      ) : (
        <p role="status">This Skill is not tracked for updates.</p>
      )}
    </section>
  );
}

function shortCommit(commit: string) {
  return commit.length > 10 ? commit.slice(0, 10) : commit;
}

function AgentInspector({
  detail,
  agents,
  error,
  pendingAgentId,
  isApplying,
  isChecking,
  onRequest,
  onRepair,
}: {
  detail: SkillDetail | null;
  agents: AgentActivation[];
  error: string | null;
  pendingAgentId: string | null;
  isApplying: boolean;
  isChecking: boolean;
  onRequest: (agentId: string, enabled: boolean) => void;
  onRepair: (agentId: string) => void;
}) {
  return (
    <aside className="agent-inspector" aria-label="Enable by Agent">
      <div className="panel-heading inspector-heading">
        <div>
          <span className="eyebrow">Activation</span>
          <h2>Enable by Agent</h2>
        </div>
      </div>
      <p className="inspector-intro">
        Preview each change before Skill Man updates the Agent directory.
      </p>
      {error ? (
        <div className="activation-error" role="alert">
          <strong>Activation unchanged</strong>
          <span>{error}</span>
        </div>
      ) : null}
      <div className="agent-list">
        {detail && isChecking ? (
          <div className="activation-checking" role="status">
            Checking desired Activations…
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
                      aria-label={`Enable ${detail.directoryName} for ${agent.name}`}
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
                    {isPending ? "Preparing preview" : activationLabel(agent)}
                  </span>
                  {agent.compatibility === "unknown" ? (
                    <span className="compatibility-note">
                      Compatibility unknown
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
                        ? "Conflict"
                        : "Repair"}
                    </button>
                  ) : null}
                </div>
              </div>
            );
          })
        ) : null}
      </div>
      <div className="inspector-footnote">
        <LockIcon />
        <span>Every Activation change requires a preview.</span>
      </div>
    </aside>
  );
}

function AdoptSheet({
  report,
  selected,
  plan,
  result,
  undo,
  error,
  errorHeading,
  activity,
  onToggle,
  onPlan,
  onApply,
  onUndo,
  onClose,
}: {
  report: AdoptScanReport | null;
  selected: string[];
  plan: AdoptPlan | null;
  result: AdoptResult | null;
  undo: AdoptUndoResult | null;
  error: string | null;
  errorHeading: string;
  activity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onToggle: (canonicalEntity: string, checked: boolean) => void;
  onPlan: () => void;
  onApply: () => void;
  onUndo: () => void;
  onClose: () => void;
}) {
  const isBusy = activity !== "idle";
  const candidates = report?.candidates ?? [];
  const step = result ? "result" : plan ? "preview" : report ? "scan" : "scan";

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isBusy) onClose();
      }}
    >
      <section
        className="activation-sheet import-sheet adopt-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Adopt untracked Skills"
      >
        <ol className="import-progress" aria-label="Adopt progress">
          {(["scan", "preview", "result"] as const).map((stepName) => (
            <li
              key={stepName}
              aria-current={step === stepName ? "step" : undefined}
            >
              {capitalize(stepName)}
            </li>
          ))}
        </ol>
        {result ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Adopt complete</span>
              <h2>
                {result.items.filter((item) => item.adopted).length} of{" "}
                {result.items.length} Skills adopted
              </h2>
              <p>
                Adopted Skills are Managed and enabled on their target Agents.
              </p>
            </div>
            <ul className="git-import-results">
              {result.items.map((item) => (
                <li key={item.directoryName}>
                  <span>
                    <strong>{item.directoryName}</strong>{" "}
                    {item.adopted ? (
                      <span className="candidate-clear">Adopted</span>
                    ) : (
                      <span className="candidate-conflict">
                        Failed: {item.error ?? "unknown error"}
                      </span>
                    )}
                  </span>
                </li>
              ))}
            </ul>
            {undo ? (
              <div
                className={
                  undo.items.every((item) => item.undone)
                    ? "update-result-ok"
                    : "update-result-fail"
                }
                role="status"
              >
                {undo.items.every((item) => item.undone)
                  ? "Batch undone: original locations and entries restored."
                  : undo.items
                      .filter((item) => !item.undone)
                      .map(
                        (item) =>
                          `${item.directoryName}: ${item.error ?? "unknown error"}`,
                      )
                      .join(" · ")}
              </div>
            ) : null}
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{errorHeading}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              {result.undoAvailable && !undo ? (
                <button
                  type="button"
                  className="activation-confirm-button"
                  disabled={isBusy}
                  onClick={onUndo}
                >
                  {activity === "undoing" ? "Undoing" : "Undo this batch"}
                </button>
              ) : null}
              <button type="button" disabled={isBusy} onClick={onClose}>
                Close
              </button>
            </div>
          </>
        ) : plan ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Adopt preview</span>
              <h2>
                Preview {plan.items.length} Skill
                {plan.items.length === 1 ? "" : "s"}
              </h2>
              <p>
                Each Skill is its own transaction; a failure rolls back only
                that Skill.
              </p>
            </div>
            <ul className="git-import-candidates git-import-preview-list">
              {plan.items.map((item) => (
                <li key={item.directoryName}>
                  <div>
                    <strong>{item.directoryName}</strong>
                    <span className="candidate-path">
                      {item.kind === "migrate"
                        ? "moves into Library"
                        : "registered as Link"}{" "}
                      ·{" "}
                      {item.targetAgents.length > 0
                        ? `enables on ${item.targetAgents
                            .map((agent) => agent.name)
                            .join(", ")}`
                        : "one Activation"}
                    </span>
                  </div>
                  {item.error ? (
                    <span className="candidate-conflict" role="alert">
                      {item.error}
                    </span>
                  ) : (
                    <span className="candidate-clear">Ready</span>
                  )}
                </li>
              ))}
            </ul>
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{errorHeading}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                Cancel
              </button>
              <button
                type="button"
                className="activation-confirm-button"
                disabled={!plan.canApply || isBusy}
                onClick={onApply}
              >
                {isBusy ? "Adopting" : "Adopt"}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Adopt</span>
              <h2>Untracked Skills</h2>
              <p>
                Scan Agent and shared directories. Safe candidates are
                pre-selected; external, Broken and conflicting ones require
                attention.
              </p>
            </div>
            <ul className="git-import-candidates">
              {candidates.map((candidate) => (
                <li key={candidate.canonicalEntity}>
                  <label>
                    <input
                      type="checkbox"
                      checked={selected.includes(candidate.canonicalEntity)}
                      disabled={!candidate.adoptable || isBusy}
                      onChange={(event) =>
                        onToggle(
                          candidate.canonicalEntity,
                          event.currentTarget.checked,
                        )
                      }
                    />
                    <span>
                      <strong>{candidate.directoryName}</strong>
                      <span className="candidate-path">
                        {candidate.risk === "broken"
                          ? `Broken · ${candidate.riskReason ?? "target missing"}`
                          : candidate.risk === "external"
                            ? "External · " +
                              (candidate.riskReason ?? "outside home")
                            : candidate.conflict
                              ? `Conflict with "${candidate.conflict.directoryName}"`
                              : `${candidate.appearances.length} appearance${
                                  candidate.appearances.length === 1 ? "" : "s"
                                }`}
                      </span>
                    </span>
                  </label>
                </li>
              ))}
            </ul>
            {candidates.length === 0 && !isBusy ? (
              <p role="status">No untracked Skills found.</p>
            ) : null}
            {report?.truncated ? (
              <p className="candidate-conflict" role="status">
                Candidate list truncated; Rescan after adopting to reveal more.
              </p>
            ) : null}
            {error ? (
              <div className="activation-error" role="alert">
                <strong>{errorHeading}</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                Cancel
              </button>
              <button
                type="button"
                className="activation-confirm-button"
                disabled={selected.length === 0 || isBusy || report === null}
                onClick={onPlan}
              >
                {activity === "planning" ? "Preparing" : "Preview Adopt"}
              </button>
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function LinkImportSheet({
  kind,
  preview,
  result,
  error,
  activity,
  gitImportSource,
  gitImportForceFullDepth,
  gitImportDiscovery,
  gitImportSelected,
  gitImportPreview,
  gitImportResult,
  gitImportError,
  gitImportActivity,
  onKindChange,
  onPreview,
  onApply,
  onClose,
  onOpenImportedSkill,
  onGitImportSourceChange,
  onGitImportForceFullDepthChange,
  onDiscoverGitImport,
  onGitImportSelectionChange,
  onPlanGitImport,
  onApplyGitImport,
  onOpenImportedGitSkill,
}: {
  kind: ImportKind;
  preview: LinkImportPreview | null;
  result: LinkImportResult | null;
  error: string | null;
  activity: "idle" | "discovering" | "applying";
  gitImportSource: string;
  gitImportForceFullDepth: boolean;
  gitImportDiscovery: GitImportDiscovery | null;
  gitImportSelected: string[];
  gitImportPreview: GitImportSelectionPreview | null;
  gitImportResult: GitImportSelectionResult | null;
  gitImportError: string | null;
  gitImportActivity: "idle" | "discovering" | "planning" | "applying";
  onKindChange: (kind: ImportKind) => void;
  onPreview: (sourcePath: string) => void;
  onApply: () => void;
  onClose: () => void;
  onOpenImportedSkill: () => void;
  onGitImportSourceChange: (source: string) => void;
  onGitImportForceFullDepthChange: (force: boolean) => void;
  onDiscoverGitImport: (source: string, forceFullDepth: boolean) => void;
  onGitImportSelectionChange: (directoryNames: string[]) => void;
  onPlanGitImport: () => void;
  onApplyGitImport: () => void;
  onOpenImportedGitSkill: (skillId: string) => void;
}) {
  const [sourcePath, setSourcePath] = useState("");
  const sourceInput = useRef<HTMLInputElement>(null);
  const primaryButton = useRef<HTMLButtonElement>(null);
  const isDiscovering = activity === "discovering";
  const isApplying = activity === "applying";
  const isRunning = activity !== "idle" || gitImportActivity !== "idle";
  const isGit = kind === "git";
  const gitIsApplying = gitImportActivity === "applying";
  const gitStep = gitImportResult
    ? "result"
    : gitImportPreview
      ? "preview"
      : gitImportDiscovery
        ? "discover"
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
    if (result || preview || gitImportResult || gitImportPreview) {
      primaryButton.current?.focus();
    } else {
      sourceInput.current?.focus();
    }
  }, [preview, result, gitImportPreview, gitImportResult]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isApplying && !gitIsApplying) onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isApplying, gitIsApplying, onClose]);

  function toggleGitCandidate(directoryName: string, checked: boolean) {
    const next = checked
      ? [...gitImportSelected, directoryName]
      : gitImportSelected.filter((name) => name !== directoryName);
    onGitImportSelectionChange(next);
  }

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (
          event.currentTarget === event.target &&
          !isApplying &&
          !gitIsApplying
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
            ? "Link Import result"
            : gitImportResult
              ? "Import result"
              : `Import ${isGit ? "from Git" : "Link"}`
        }
      >
        <ol className="import-progress" aria-label="Import progress">
          {(["source", "discover", "preview", "result"] as const).map(
            (step) => (
              <li
                key={step}
                aria-current={currentStep === step ? "step" : undefined}
              >
                {capitalize(step)}
              </li>
            ),
          )}
        </ol>
        {isGit ? (
          <GitImportFlow
            source={gitImportSource}
            forceFullDepth={gitImportForceFullDepth}
            discovery={gitImportDiscovery}
            selected={gitImportSelected}
            preview={gitImportPreview}
            result={gitImportResult}
            error={gitImportError}
            activity={gitImportActivity}
            onSourceChange={onGitImportSourceChange}
            onForceFullDepthChange={onGitImportForceFullDepthChange}
            onDiscover={onDiscoverGitImport}
            onSelectionChange={toggleGitCandidate}
            onPlan={onPlanGitImport}
            onApply={onApplyGitImport}
            onClose={onClose}
            onKindChange={onKindChange}
            onOpenImportedSkill={onOpenImportedGitSkill}
            primaryButton={primaryButton}
            sourceInput={sourceInput}
          />
        ) : result ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Import complete</span>
              <h2>{result.directoryName} is Managed</h2>
              <p>
                The source remains in place. Library stores its Link as a SQLite
                pointer.
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>Final entity</dt>
                <dd>{result.finalEntityPath}</dd>
              </div>
              <div>
                <dt>Library storage</dt>
                <dd>SQLite pointer only</dd>
              </div>
            </dl>
            <div className="activation-sheet-actions import-result-actions">
              <button type="button" disabled={isApplying} onClick={onClose}>
                Close
              </button>
              <button type="button" onClick={onOpenImportedSkill}>
                View in Library
              </button>
              <button
                ref={primaryButton}
                type="button"
                className="activation-confirm-button"
                onClick={onOpenImportedSkill}
              >
                Enable by Agent
              </button>
            </div>
          </>
        ) : preview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Link preview</span>
              <h2>Preview {preview.directoryName}</h2>
              <p>
                Import this folder by reference. No Skill files will be copied
                into Library.
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>Selected source</dt>
                <dd>{preview.sourceEntryPath}</dd>
              </div>
              <div>
                <dt>Final entity</dt>
                <dd>{preview.finalEntityPath}</dd>
              </div>
              <div>
                <dt>Library storage</dt>
                <dd>SQLite pointer only</dd>
              </div>
            </dl>
            <div className="activation-warning import-risk" role="status">
              <strong>Review imported instructions</strong>
              <span>
                This source&apos;s SKILL.md can become instructions for every
                Agent you enable.
              </span>
            </div>
            {preview.conflict ? (
              <div className="import-conflict" role="alert">
                <strong>Library Conflict</strong>
                <span>
                  Managed Skill “{preview.conflict.directoryName}” already uses
                  this directory identity. Rename the source, Remove the
                  existing Skill, or cancel.
                </span>
              </div>
            ) : null}
            {error ? (
              <div className="activation-error" role="alert">
                <strong>Import unchanged</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isApplying} onClick={onClose}>
                Cancel
              </button>
              <button
                ref={primaryButton}
                type="button"
                className="activation-confirm-button"
                disabled={!preview.canApply || isRunning}
                onClick={onApply}
              >
                {isRunning ? "Importing" : `Import ${preview.directoryName}`}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Import · Link</span>
              <h2>Link a local Skill</h2>
              <p>
                Choose a development folder containing a readable SKILL.md. The
                folder stays at its source.
              </p>
            </div>
            <SourceKindSwitch kind={kind} onKindChange={onKindChange} />
            <label className="import-source-field">
              <span>Local folder path</span>
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
                <strong>Source unavailable</strong>
                <span>{error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isApplying} onClick={onClose}>
                Cancel
              </button>
              <button
                ref={primaryButton}
                type="button"
                className="activation-confirm-button"
                disabled={!sourcePath.trim() || isRunning}
                onClick={() => onPreview(sourcePath)}
              >
                {isDiscovering ? "Checking source" : "Preview Link"}
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
  return (
    <div className="import-kind-switch" role="group" aria-label="Import source">
      <button
        type="button"
        aria-pressed={kind === "link"}
        onClick={() => onKindChange("link")}
      >
        Link local folder
      </button>
      <button
        type="button"
        aria-pressed={kind === "git"}
        onClick={() => onKindChange("git")}
      >
        Install from Git
      </button>
    </div>
  );
}

function GitImportFlow({
  source,
  forceFullDepth,
  discovery,
  selected,
  preview,
  result,
  error,
  activity,
  onSourceChange,
  onForceFullDepthChange,
  onDiscover,
  onSelectionChange,
  onPlan,
  onApply,
  onClose,
  onKindChange,
  onOpenImportedSkill,
  primaryButton,
  sourceInput,
}: {
  source: string;
  forceFullDepth: boolean;
  discovery: GitImportDiscovery | null;
  selected: string[];
  preview: GitImportSelectionPreview | null;
  result: GitImportSelectionResult | null;
  error: string | null;
  activity: "idle" | "discovering" | "planning" | "applying";
  onSourceChange: (source: string) => void;
  onForceFullDepthChange: (force: boolean) => void;
  onDiscover: (source: string, forceFullDepth: boolean) => void;
  onSelectionChange: (directoryName: string, checked: boolean) => void;
  onPlan: () => void;
  onApply: () => void;
  onClose: () => void;
  onKindChange: (kind: ImportKind) => void;
  onOpenImportedSkill: (skillId: string) => void;
  primaryButton: React.RefObject<HTMLButtonElement | null>;
  sourceInput: React.RefObject<HTMLInputElement | null>;
}) {
  const isBusy = activity !== "idle";
  const candidates = discovery?.candidates ?? [];
  const selectedCount = selected.length;

  if (result) {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">Import complete</span>
          <h2>{result.items.length} Skills are Managed</h2>
          <p>
            Installed from {preview?.repoUrl ?? discovery?.repoUrl ?? "Git"} at
            commit{" "}
            <code>
              {shortCommit(
                preview?.resolvedCommit ?? discovery?.resolvedCommit ?? "",
              )}
            </code>
            .
          </p>
        </div>
        <ul className="git-import-results">
          {result.items.map((item) => (
            <li key={item.skillId}>
              <span>{item.directoryName}</span>
              <button
                type="button"
                onClick={() => onOpenImportedSkill(item.skillId)}
              >
                View in Library
              </button>
            </li>
          ))}
        </ul>
        <div className="activation-sheet-actions">
          <button type="button" disabled={isBusy} onClick={onClose}>
            Close
          </button>
        </div>
      </>
    );
  }

  if (preview) {
    const blocked = preview.items.some((item) => !item.canApply);
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">Import preview</span>
          <h2>
            Preview {preview.items.length} Skill
            {preview.items.length === 1 ? "" : "s"}
          </h2>
          <p>
            {preview.repoUrl} ·{" "}
            <code>{shortCommit(preview.resolvedCommit)}</code>
          </p>
        </div>
        <ul className="git-import-candidates git-import-preview-list">
          {preview.items.map((item) => (
            <li key={item.directoryName}>
              <div>
                <strong>{item.directoryName}</strong>
                <span className="candidate-path">
                  {item.skillPath || "repo root"}
                </span>
              </div>
              {item.conflict ? (
                <span className="candidate-conflict" role="alert">
                  Conflict with “{item.conflict.directoryName}”
                </span>
              ) : (
                <span className="candidate-clear">Ready</span>
              )}
            </li>
          ))}
        </ul>
        {blocked ? (
          <div className="import-conflict" role="alert">
            <strong>Library Conflict</strong>
            <span>
              One or more selected Skills already exist in the Library. Rename
              or Remove the existing Skills, or cancel.
            </span>
          </div>
        ) : null}
        <div className="activation-warning import-risk" role="status">
          <strong>Review imported instructions</strong>
          <span>
            These SKILL.md files can become instructions for every Agent you
            enable.
          </span>
        </div>
        {error ? (
          <div className="activation-error" role="alert">
            <strong>Import unchanged</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button type="button" disabled={isBusy} onClick={onClose}>
            Cancel
          </button>
          <button
            ref={primaryButton}
            type="button"
            className="activation-confirm-button"
            disabled={!preview.canApply || isBusy}
            onClick={onApply}
          >
            {isBusy
              ? "Importing"
              : `Install ${selectedCount} Skill${selectedCount === 1 ? "" : "s"}`}
          </button>
        </div>
      </>
    );
  }

  if (discovery) {
    return (
      <>
        <div className="activation-sheet-heading">
          <span className="eyebrow">Discover</span>
          <h2>
            {candidates.length} Skill{candidates.length === 1 ? "" : "s"} found
          </h2>
          <p>
            {discovery.repoUrl} ·{" "}
            <code>{shortCommit(discovery.resolvedCommit)}</code>
            {discovery.truncated ? " · list truncated" : ""}
          </p>
        </div>
        <ul className="git-import-candidates">
          {candidates.map((candidate) => (
            <li key={`${candidate.directoryName}:${candidate.skillPath}`}>
              <label>
                <input
                  type="checkbox"
                  checked={selected.includes(candidate.directoryName)}
                  onChange={(event) =>
                    onSelectionChange(
                      candidate.directoryName,
                      event.currentTarget.checked,
                    )
                  }
                />
                <span>
                  <strong>{candidate.directoryName}</strong>
                  <span className="candidate-path">
                    {candidate.skillPath || "repo root"}
                  </span>
                </span>
              </label>
            </li>
          ))}
        </ul>
        {error ? (
          <div className="activation-error" role="alert">
            <strong>Discovery failed</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button type="button" disabled={isBusy} onClick={onClose}>
            Cancel
          </button>
          <button
            ref={primaryButton}
            type="button"
            className="activation-confirm-button"
            disabled={selectedCount === 0 || isBusy}
            onClick={onPlan}
          >
            {activity === "planning"
              ? "Preparing"
              : `Preview ${selectedCount} Skill${selectedCount === 1 ? "" : "s"}`}
          </button>
        </div>
      </>
    );
  }

  return (
    <>
      <div className="activation-sheet-heading">
        <span className="eyebrow">Import · Git</span>
        <h2>Install from Git</h2>
        <p>
          Public HTTPS repository. Skill Man discovers Skills, stages them, and
          tracks updates for the branch you install.
        </p>
      </div>
      <SourceKindSwitch kind="git" onKindChange={onKindChange} />
      <label className="import-source-field">
        <span>Repository URL or owner/repo</span>
        <input
          ref={sourceInput}
          type="text"
          value={source}
          disabled={isBusy}
          placeholder="vercel-labs/skills"
          onChange={(event) => onSourceChange(event.currentTarget.value)}
        />
      </label>
      <label className="import-option">
        <input
          type="checkbox"
          checked={forceFullDepth}
          disabled={isBusy}
          onChange={(event) =>
            onForceFullDepthChange(event.currentTarget.checked)
          }
        />
        <span>Force full-depth scan (slow repos)</span>
      </label>
      {error ? (
        <div className="activation-error" role="alert">
          <strong>Source unavailable</strong>
          <span>{error}</span>
        </div>
      ) : null}
      <div className="activation-sheet-actions">
        <button type="button" disabled={isBusy} onClick={onClose}>
          Cancel
        </button>
        <button
          ref={primaryButton}
          type="button"
          className="activation-confirm-button"
          disabled={!source.trim() || isBusy}
          onClick={() => onDiscover(source, forceFullDepth)}
        >
          {activity === "discovering"
            ? "Fetching repository"
            : "Discover Skills"}
        </button>
      </div>
    </>
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
  const action = capitalize(preview.kind);
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
        aria-label={`Preview ${action}`}
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">Activation preview</span>
          <h2>
            {action} {preview.skillDirectoryName}
          </h2>
          <p>
            {preview.kind === "repair"
              ? "Recreate one missing managed Activation in"
              : `${preview.enabled ? "Create" : "Remove"} one managed Activation in`}{" "}
            {preview.agentName}.
          </p>
        </div>
        <dl className="activation-paths">
          <div>
            <dt>Agent entry</dt>
            <dd>{preview.entryPath}</dd>
          </div>
          <div>
            <dt>Final entity</dt>
            <dd>{preview.targetPath}</dd>
          </div>
        </dl>
        {preview.compatibilityWarning ? (
          <div className="activation-warning" role="status">
            <strong>Compatibility confirmation</strong>
            <span>{preview.compatibilityWarning}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button
            ref={cancelButton}
            type="button"
            disabled={isApplying}
            onClick={onCancel}
          >
            Cancel
          </button>
          <button
            ref={confirmButton}
            type="button"
            className="activation-confirm-button"
            disabled={isApplying}
            onClick={onApply}
          >
            {isApplying ? "Applying" : `${action} in ${preview.agentName}`}
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
        aria-label="Activation conflict"
      >
        {step === "conflict" ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Conflict</span>
              <h2>Activation path is occupied</h2>
              <p>
                Skill Man found existing content at the expected Agent entry and
                left it unchanged. Choose how to resolve it.
              </p>
            </div>
            {message ? (
              <p className="activation-conflict-detail">{message}</p>
            ) : null}
            <dl className="activation-paths">
              <div>
                <dt>Agent entry</dt>
                <dd>{details.entryPath}</dd>
              </div>
              <div>
                <dt>Would point to</dt>
                <dd>{details.targetPath}</dd>
              </div>
            </dl>
            <div className="conflict-occupier">
              <strong>{occupierHeading(occupier)}</strong>
              <span>{occupierDescription(occupier)}</span>
              {occupier.adoptable ? null : occupier.notAdoptableReason ? (
                <small className="candidate-conflict" role="status">
                  {occupier.notAdoptableReason}
                </small>
              ) : null}
            </div>
            {error ? (
              <div className="activation-error" role="alert">
                <strong>Replace unchanged</strong>
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
                Cancel
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
                Adopt existing item
              </button>
              <button
                ref={replaceButton}
                type="button"
                className="activation-confirm-button"
                disabled={isBusy}
                onClick={onPlanReplace}
              >
                Remove then replace
              </button>
            </div>
          </>
        ) : step === "preview" && replacePreview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Replace</span>
              <h2>Remove then replace {replacePreview.skillDirectoryName}</h2>
              <p>
                The occupying item is moved to a temporary backup before the
                Activation is created in {replacePreview.agentName}.
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>Agent entry</dt>
                <dd>{replacePreview.entryPath}</dd>
              </div>
              <div>
                <dt>Final entity</dt>
                <dd>{replacePreview.targetPath}</dd>
              </div>
              <div>
                <dt>Temporary backup</dt>
                <dd>{replacePreview.backupPath}</dd>
              </div>
            </dl>
            <div className="activation-warning" role="status">
              <strong>Explicit confirmation required</strong>
              <span>
                The existing {occupierKindLabel(replacePreview.occupantKind)} is
                backed up while this window is open and is discarded when it
                closes. Undo restores it before you close.
              </span>
            </div>
            {error ? (
              <div className="activation-error" role="alert">
                <strong>Replace unchanged</strong>
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
                Cancel
              </button>
              <button
                ref={replaceButton}
                type="button"
                className="activation-confirm-button"
                disabled={isBusy}
                onClick={onApplyReplace}
              >
                {isApplying ? "Replacing" : "Remove and replace"}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Replace complete</span>
              <h2>Activation created</h2>
              <p>
                The Skill is enabled for this Agent. The previous item stays in
                its temporary backup while this window is open.
              </p>
            </div>
            {replaceUndo ? (
              <div
                className={
                  replaceUndo.undone ? "update-result-ok" : "update-result-fail"
                }
                role="status"
              >
                {replaceUndo.undone
                  ? "Previous item restored to its original location."
                  : `Previous item not restored: ${replaceUndo.error ?? "unknown reason"}`}
              </div>
            ) : null}
            {error ? (
              <div className="activation-error" role="alert">
                <strong>Replace unchanged</strong>
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
                Close
              </button>
              {!replaceUndo ? (
                <button
                  ref={replaceButton}
                  type="button"
                  className="activation-confirm-button"
                  disabled={isBusy}
                  onClick={onUndoReplace}
                >
                  {isUndoing ? "Restoring" : "Restore previous item"}
                </button>
              ) : null}
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function occupierHeading(occupier: OccupierSummary) {
  if (occupier.isSkill) {
    return `${occupier.directoryName} · untracked Skill`;
  }
  return `${occupier.directoryName} · ${occupierKindLabel(occupier.kind)}`;
}

function occupierDescription(occupier: OccupierSummary) {
  if (occupier.finalEntityPath) {
    return `Resolves to ${occupier.finalEntityPath}`;
  }
  if (occupier.symlinkTarget) {
    return `Symlink to ${occupier.symlinkTarget}`;
  }
  return "Existing content at the entry.";
}

function occupierKindLabel(kind: OccupierKind) {
  if (kind === "real_directory") return "real directory";
  if (kind === "symlink") return "symlink";
  return "file";
}

// -- First-run onboarding (spec §8.7) ---------------------------------------

const onboardingSteps = [
  {
    title: "Create the Library",
    body: "Skill Man keeps every Managed Skill in a fixed app-owned location. It never reuses Agent convention directories.",
    note: "~/Library/Application Support/skill-man",
  },
  {
    title: "Check Agent Presets",
    body: "Claude Code and Codex presets always show. Missing directories are only marked; they are never created without your explicit confirmation.",
  },
  {
    title: "Scan existing Skills",
    body: "A read-only full scan of Agent and shared directories. Nothing is Adopted, moved or overwritten by the scan.",
  },
];

function OnboardingSheet({
  step,
  agents,
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
  report: AdoptScanReport | null;
  activity: "idle" | "scanning";
  error: string | null;
  onSkip: () => void;
  onAdvance: () => void;
  onCreateDirectory: (agentId: string) => void;
  onFinishWithAdopt: () => void;
}) {
  const closeButton = useRef<HTMLButtonElement>(null);
  const advanceButton = useRef<HTMLButtonElement>(null);
  const isScanning = activity === "scanning";
  const candidates = report?.candidates ?? [];

  useLayoutEffect(() => {
    if (step === 2 && !isScanning) advanceButton.current?.focus();
    else closeButton.current?.focus();
  }, [step, isScanning]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isScanning) onSkip();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isScanning, onSkip]);

  const current = onboardingSteps[step];
  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isScanning) onSkip();
      }}
    >
      <section
        className="activation-sheet onboarding-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Welcome to Skill Man"
      >
        <div className="onboarding-progress" aria-label="First-run progress">
          {onboardingSteps.map((item, index) => (
            <span
              key={item.title}
              className={index <= step ? "onboarding-progress-dot--active" : ""}
            >
              {index + 1}
            </span>
          ))}
        </div>
        <div className="activation-sheet-heading">
          <span className="eyebrow">First run · {step + 1} of 3</span>
          <h2>{current.title}</h2>
          <p>{current.body}</p>
        </div>
        {step === 0 && current.note ? (
          <dl className="activation-paths">
            <div>
              <dt>Library</dt>
              <dd>{current.note}</dd>
            </div>
          </dl>
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
                  <span className="candidate-clear">Detected</span>
                ) : (
                  <>
                    <span className="candidate-conflict">Not detected</span>
                    <button
                      type="button"
                      disabled={isScanning}
                      onClick={() => onCreateDirectory(agent.id)}
                    >
                      Create directory
                    </button>
                  </>
                )}
              </li>
            ))}
          </ul>
        ) : null}
        {step === 2 ? (
          <div className="onboarding-scan">
            {isScanning ? (
              <p role="status">Scanning Agent and shared directories…</p>
            ) : report ? (
              <>
                <p role="status">
                  {candidates.length} Untracked Skill
                  {candidates.length === 1 ? "" : "s"} found. Nothing changed
                  yet.
                </p>
                <ul className="git-import-candidates">
                  {candidates.map((candidate) => (
                    <li key={candidate.canonicalEntity}>
                      <span>
                        <strong>{candidate.directoryName}</strong>
                        <span className="candidate-path">
                          {candidate.risk === "broken"
                            ? `Broken · ${candidate.riskReason ?? "target missing"}`
                            : candidate.risk === "external"
                              ? "External · " +
                                (candidate.riskReason ?? "outside home")
                              : candidate.conflict
                                ? `Conflict with "${candidate.conflict.directoryName}"`
                                : `${candidate.appearances.length} appearance${
                                    candidate.appearances.length === 1
                                      ? ""
                                      : "s"
                                  }`}
                        </span>
                      </span>
                    </li>
                  ))}
                </ul>
                {candidates.length === 0 ? (
                  <p role="status">
                    No untracked Skills found — nothing to adopt.
                  </p>
                ) : null}
              </>
            ) : null}
          </div>
        ) : null}
        {error ? (
          <div className="activation-error" role="alert">
            <strong>Onboarding unchanged</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button
            ref={closeButton}
            type="button"
            disabled={isScanning}
            onClick={onSkip}
          >
            Skip setup
          </button>
          {step < 2 ? (
            <button
              ref={advanceButton}
              type="button"
              className="activation-confirm-button"
              disabled={isScanning}
              onClick={onAdvance}
            >
              {isScanning ? "Scanning" : "Continue"}
            </button>
          ) : (
            <>
              <button
                ref={advanceButton}
                type="button"
                className="activation-confirm-button"
                disabled={!report || isScanning}
                onClick={onFinishWithAdopt}
              >
                Review Adopt candidates
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
  title: string;
  note: string;
}> = [
  {
    key: "launchAtLogin",
    title: "Launch at login",
    note: "Starts in the background without opening the main window.",
  },
  {
    key: "showInDock",
    title: "Show in Dock",
    note: "Turn off to run as a menu-bar accessory app.",
  },
  {
    key: "checkAppUpdates",
    title: "Check for app updates",
    note: "At most once per day; installation always asks first.",
  },
  {
    key: "checkSkillUpdates",
    title: "Check for Skill updates",
    note: "Tracked Git installs only; updates are never applied automatically.",
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
  warning: string | null;
  error: string | null;
  appUpdatePanel: AppUpdatePanelState;
  onToggle: (updates: PreferenceUpdates) => void;
  onCheckAppUpdate: () => void;
  onClose: () => void;
}) {
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
        aria-label="Preferences"
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">Preferences</span>
          <h2>Settings</h2>
          <p>
            Exactly four switches. Library path, theme, language and
            notifications are not configurable in the MVP.
          </p>
        </div>
        <div className="preference-list">
          {preferenceRows.map((row) => (
            <label className="preference-row" key={row.key}>
              <span>
                <strong>{row.title}</strong>
                <small>{row.note}</small>
              </span>
              <span className="switch-control switch-control--interactive">
                <input
                  type="checkbox"
                  role="switch"
                  aria-label={row.title}
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
            <strong>App updates</strong>
            <small>
              Check the signed latest release without changing your preference.
            </small>
          </span>
          <button
            id="app-update-check-trigger"
            type="button"
            disabled={appUpdatePanel.activity === "checking"}
            onClick={onCheckAppUpdate}
          >
            {appUpdatePanel.activity === "checking" ? "Checking…" : "Check now"}
          </button>
        </div>
        {appUpdatePanel.checkStatus ? (
          <div className="app-update-check-result" role="status">
            {appUpdatePanel.checkStatus === "up_to_date"
              ? "Skill Man is up to date."
              : "The update check was skipped."}
          </div>
        ) : null}
        {appUpdatePanel.error && appUpdatePanel.update === null ? (
          <div className="activation-error" role="alert">
            <strong>Could not check for app updates</strong>
            <span>{appUpdatePanel.error}</span>
          </div>
        ) : null}
        {warning ? (
          <div className="activation-warning" role="status">
            <strong>Applied with a warning</strong>
            <span>{warning}</span>
          </div>
        ) : null}
        {error ? (
          <div className="activation-error" role="alert">
            <strong>Preferences unchanged</strong>
            <span>{error}</span>
          </div>
        ) : null}
        <div className="activation-sheet-actions">
          <button ref={closeButton} type="button" onClick={onClose}>
            Done
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
        aria-label="App update available"
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">App update available</span>
          <h2>Version {update.version}</h2>
          <p>Current version {update.currentVersion}</p>
        </div>
        <dl className="app-update-details">
          <div>
            <dt>Archive size</dt>
            <dd>{formatDownloadSize(update.downloadSizeBytes)}</dd>
          </div>
          <div>
            <dt>Release notes</dt>
            <dd>{update.releaseNotes || "No release notes provided."}</dd>
          </div>
        </dl>
        {panel.activity === "downloading" ? (
          <div className="app-update-progress" role="status">
            Downloading and verifying the signed archive…
          </div>
        ) : null}
        {isReady ? (
          <div className="app-update-ready" role="status">
            Download verified. Install and restart Skill Man now?
          </div>
        ) : null}
        {panel.error ? (
          <div className="activation-error" role="alert">
            <strong>App update failed</strong>
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
              ? "Cancelling…"
              : panel.activity === "downloading"
                ? "Cancel download"
                : isReady
                  ? "Later"
                  : "Not now"}
          </button>
          <button
            ref={primaryButton}
            type="button"
            className="activation-confirm-button"
            disabled={blocksPrimary}
            onClick={isReady ? onInstall : onDownload}
          >
            {isCancelling
              ? "Cancelling…"
              : panel.activity === "downloading"
                ? "Downloading…"
                : panel.activity === "installing"
                  ? "Installing…"
                  : isReady
                    ? "Install and Restart"
                    : "Download update"}
          </button>
        </div>
      </section>
    </div>
  );
}

function formatDownloadSize(bytes: number) {
  if (bytes < 1024) return `${bytes} bytes`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const precision = value >= 10 ? 0 : 1;
  return `${value.toFixed(precision)} ${units[unitIndex]}`;
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
        aria-label="Remove Managed Skill"
      >
        {panel.result ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Remove complete</span>
              <h2>{panel.result.directoryName} left the Library</h2>
              <p>The catalog entry and every Activation are gone.</p>
            </div>
            <div className="activation-sheet-actions">
              <button ref={confirmButton} type="button" onClick={onClose}>
                Close
              </button>
            </div>
          </>
        ) : panel.activity === "planning" ? (
          <LoadingPanel label="Preparing Remove preview" />
        ) : preview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Remove preview</span>
              <h2>Remove {preview.directoryName}</h2>
              <p>
                Every Activation is disabled first, then the Library entry is
                deleted.
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>Final entity</dt>
                <dd>{preview.finalEntityPath}</dd>
              </div>
              <div>
                <dt>Entity handling</dt>
                <dd>
                  {isInstall
                    ? "The Install entity inside Library is deleted"
                    : "The external Link entity is kept in place"}
                </dd>
              </div>
              <div>
                <dt>Activations to disable</dt>
                <dd>{preview.activationCount}</dd>
              </div>
            </dl>
            <div className="activation-warning" role="status">
              <strong>This cannot be undone from the result window</strong>
              <span>
                The operation audit is archived, but the Skill leaves the
                Library once removed.
              </span>
            </div>
            {panel.error ? (
              <div className="activation-error" role="alert">
                <strong>Remove unchanged</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                Cancel
              </button>
              <button
                ref={confirmButton}
                type="button"
                className="activation-confirm-button danger-button"
                disabled={isBusy}
                onClick={onApply}
              >
                {isBusy ? "Removing" : `Remove ${preview.directoryName}`}
              </button>
            </div>
          </>
        ) : (
          <>
            {panel.error ? (
              <div className="activation-error" role="alert">
                <strong>Remove unavailable</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button ref={confirmButton} type="button" onClick={onClose}>
                Close
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
  const broken = detail.health === "broken";
  const isBrokenLink = broken && detail.sourceKind === "link";
  return (
    <div className={`health-notice health-notice--${detail.health}`}>
      <strong>
        {broken ? "Source unavailable" : "Local changes detected"}
      </strong>
      <span>
        {broken
          ? "The Library entry remains Managed, but its final entity cannot be read."
          : "This Install no longer matches its recorded content hash."}
      </span>
      {isBrokenLink ? (
        <button
          type="button"
          className="repair-button"
          disabled={relocatePanel.isOpen}
          onClick={onOpenRelocate}
        >
          Relocate…
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
        aria-label="Relocate Broken Link"
      >
        <ol className="import-progress" aria-label="Relocate progress">
          {(["source", "preview", "result"] as const).map((stepName) => (
            <li
              key={stepName}
              aria-current={step === stepName ? "step" : undefined}
            >
              {capitalize(stepName)}
            </li>
          ))}
        </ol>
        {panel.result ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Relocation complete</span>
              <h2>{panel.result.directoryName} is healthy again</h2>
              <p>
                The Link pointer and {panel.result.activationCount} Activation
                {panel.result.activationCount === 1 ? "" : "s"} now point at the
                relocated source.
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>Final entity</dt>
                <dd>{panel.result.finalEntityPath}</dd>
              </div>
            </dl>
            <div className="activation-sheet-actions">
              <button type="button" onClick={onClose}>
                Close
              </button>
            </div>
          </>
        ) : panel.preview ? (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Relocate preview</span>
              <h2>Relocate {panel.preview.directoryName}</h2>
              <p>
                The new source must keep the same directory identity and
                frontmatter name. Activations are repointed to the new entity.
              </p>
            </div>
            <dl className="activation-paths">
              <div>
                <dt>New source</dt>
                <dd>{panel.preview.sourceEntryPath}</dd>
              </div>
              <div>
                <dt>Final entity</dt>
                <dd>{panel.preview.finalEntityPath}</dd>
              </div>
              <div>
                <dt>Activations to update</dt>
                <dd>{panel.preview.activationCount}</dd>
              </div>
            </dl>
            {panel.error ? (
              <div className="activation-error" role="alert">
                <strong>Relocation unchanged</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                Cancel
              </button>
              <button
                type="button"
                className="activation-confirm-button"
                disabled={isBusy}
                onClick={onApply}
              >
                {isBusy ? "Relocating" : "Relocate"}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="activation-sheet-heading">
              <span className="eyebrow">Relocate · Broken Link</span>
              <h2>Find the Skill again</h2>
              <p>
                Choose the moved source directory. SKILL.md must be readable and
                the directory identity and frontmatter name must match.
              </p>
            </div>
            <label className="import-source-field">
              <span>New source path</span>
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
                <strong>Source rejected</strong>
                <span>{panel.error}</span>
              </div>
            ) : null}
            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={onClose}>
                Cancel
              </button>
              <button
                ref={confirmButton}
                type="button"
                className="activation-confirm-button"
                disabled={!panel.sourcePath.trim() || isBusy}
                onClick={() => onPreview(panel.sourcePath)}
              >
                {panel.activity === "previewing"
                  ? "Checking source"
                  : "Preview Relocate"}
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
  return (
    <span className={`health-badge health-badge--${health}`}>
      {capitalize(health)}
    </span>
  );
}

function sourceKindLabel(sourceKind: SourceKind) {
  if (sourceKind === "link") return "Link";
  if (sourceKind === "remote_install") return "Git Install";
  return "File Install";
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

function activationLabel(agent: AgentActivation) {
  if (!agent.desiredEnabled) return "Disabled";
  if (agent.observedState === "present") return "Enabled · Present";
  return `Enabled · ${capitalize(agent.observedState.replaceAll("_", " "))}`;
}

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function formatActivity(value: string) {
  return new Intl.DateTimeFormat("en", {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(value));
}
