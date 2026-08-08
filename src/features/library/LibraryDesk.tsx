import { useEffect, useLayoutEffect, useRef, useState } from "react";

import type { ImportKind, UpdatePanelState } from "../../app/App";
import type {
  ActivationPreview,
  AdoptPlan,
  AdoptResult,
  AdoptScanReport,
  AdoptUndoResult,
  AgentActivation,
  CatalogFilter,
  GitImportDiscovery,
  GitImportSelectionPreview,
  GitImportSelectionResult,
  Health,
  LinkImportPreview,
  LinkImportResult,
  SkillDetail,
  SkillSummary,
  SourceKind,
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
  activationConflict: string | null;
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
  isAdoptOpen: boolean;
  adoptReport: AdoptScanReport | null;
  adoptSelected: string[];
  adoptPlan: AdoptPlan | null;
  adoptResult: AdoptResult | null;
  adoptUndo: AdoptUndoResult | null;
  adoptError: string | null;
  adoptActivity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onFilter: (filter: CatalogFilter) => void;
  onSelect: (skillId: string) => void;
  onRequestActivation: (agentId: string, enabled: boolean) => void;
  onRequestActivationRepair: (agentId: string) => void;
  onApplyActivation: () => void;
  onCancelActivation: () => void;
  onCloseActivationConflict: () => void;
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
  onFilter,
  onSelect,
  onRequestActivation,
  onRequestActivationRepair,
  onApplyActivation,
  onCancelActivation,
  onCloseActivationConflict,
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
  adoptActivity,
  onOpenAdopt,
  onToggleAdoptCandidate,
  onPlanAdopt,
  onApplyAdopt,
  onUndoAdopt,
  onCloseAdopt,
}: LibraryDeskProps) {
  const lastOverlay = useRef<"activation" | "import" | "adopt" | null>(null);
  const hasActivationOverlay = Boolean(activationPreview || activationConflict);
  const hasOverlay = hasActivationOverlay || isLinkImportOpen || isAdoptOpen;
  useLayoutEffect(() => {
    if (hasActivationOverlay) {
      lastOverlay.current = "activation";
    } else if (isLinkImportOpen) {
      lastOverlay.current = "import";
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
      }
      lastOverlay.current = null;
    }
  }, [activationTriggerControlId, hasActivationOverlay, isLinkImportOpen]);

  return (
    <div className="app-shell">
      <div className="app-background" inert={hasOverlay ? true : undefined}>
        <a className="skip-link" href="#skill-detail">
          Skip to Skill detail
        </a>
        <Toolbar onImport={onOpenLinkImport} onAdopt={onOpenAdopt} />
        {error ? (
          <div className="global-notice" role="alert">
            <strong>Library unavailable</strong>
            <span>{error}</span>
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
            onCheckUpdates={onCheckSkillUpdates}
            onPlanUpdate={onPlanSkillUpdate}
            onApplyUpdate={onApplySkillUpdate}
            onPinUpdate={onPinSkillUpdate}
            onReselectPathChange={onReselectPathChange}
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
          detail={activationConflict}
          onClose={onCloseActivationConflict}
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
    </div>
  );
}

function Toolbar({
  onImport,
  onAdopt,
}: {
  onImport: () => void;
  onAdopt: () => void;
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
          type="button"
          className="icon-button"
          aria-label="Preferences"
          disabled
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
  onCheckUpdates,
  onPlanUpdate,
  onApplyUpdate,
  onPinUpdate,
  onReselectPathChange,
}: {
  detail: SkillDetail | null;
  updatePanel: UpdatePanelState;
  reselectPath: string;
  onCheckUpdates: () => void;
  onPlanUpdate: (newSkillPath: string | null) => void;
  onApplyUpdate: (abandonChanges: boolean) => void;
  onPinUpdate: () => void;
  onReselectPathChange: (path: string) => void;
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
            <HealthNotice detail={detail} />
          ) : null}
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
          <button
            type="button"
            disabled={isBusy}
            onClick={onCheckUpdates}
          >
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
              Update available:{" "}
              <code>{shortCommit(item.currentCommit)}</code> →{" "}
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
                <button
                  type="button"
                  disabled={isBusy}
                  onClick={onPinUpdate}
                >
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
              <button
                type="button"
                disabled={isBusy}
                onClick={onPinUpdate}
              >
                {updatePanel.activity === "pinning"
                  ? "Pinning"
                  : "Keep current version"}
              </button>
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
  activity: "idle" | "scanning" | "planning" | "applying" | "undoing";
  onToggle: (canonicalEntity: string, checked: boolean) => void;
  onPlan: () => void;
  onApply: () => void;
  onUndo: () => void;
  onClose: () => void;
}) {
  const isBusy = activity !== "idle";
  const candidates = report?.candidates ?? [];
  const step = result
    ? "result"
    : plan
      ? "preview"
      : report
        ? "scan"
        : "scan";

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
            <li key={stepName} aria-current={step === stepName ? "step" : undefined}>
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
                <strong>Adopt unchanged</strong>
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
              <h2>Preview {plan.items.length} Skill{plan.items.length === 1 ? "" : "s"}</h2>
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
                      {item.kind === "migrate" ? "moves into Library" : "registered as Link"} ·{" "}
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
                <strong>Adopt unchanged</strong>
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
                        onToggle(candidate.canonicalEntity, event.currentTarget.checked)
                      }
                    />
                    <span>
                      <strong>{candidate.directoryName}</strong>
                      <span className="candidate-path">
                        {candidate.risk === "broken"
                          ? "Broken · target missing"
                          : candidate.risk === "external"
                            ? "External · " + (candidate.riskReason ?? "outside home")
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
                <strong>Scan failed</strong>
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
  const gitIsDiscovering = gitImportActivity === "discovering";
  const gitIsPlanning = gitImportActivity === "planning";
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
              {shortCommit(preview?.resolvedCommit ?? discovery?.resolvedCommit ?? "")}
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
          <h2>Preview {preview.items.length} Skill{preview.items.length === 1 ? "" : "s"}</h2>
          <p>
            {preview.repoUrl} · <code>{shortCommit(preview.resolvedCommit)}</code>
          </p>
        </div>
        <ul className="git-import-candidates git-import-preview-list">
          {preview.items.map((item) => (
            <li key={item.directoryName}>
              <div>
                <strong>{item.directoryName}</strong>
                <span className="candidate-path">{item.skillPath || "repo root"}</span>
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
            {isBusy ? "Importing" : `Install ${selectedCount} Skill${selectedCount === 1 ? "" : "s"}`}
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
            {discovery.repoUrl} · <code>{shortCommit(discovery.resolvedCommit)}</code>
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
          {activity === "discovering" ? "Fetching repository" : "Discover Skills"}
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
  detail,
  onClose,
}: {
  detail: string;
  onClose: () => void;
}) {
  const closeButton = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    closeButton.current?.focus();
  }, []);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
      if (event.key === "Tab") {
        event.preventDefault();
        closeButton.current?.focus();
      }
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
        className="activation-sheet activation-conflict-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Activation conflict"
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">Conflict</span>
          <h2>Activation path is occupied</h2>
          <p>
            Skill Man found existing content at the expected Agent entry and
            left it unchanged.
          </p>
        </div>
        <p className="activation-conflict-detail">{detail}</p>
        <div className="activation-sheet-actions">
          <button
            ref={closeButton}
            type="button"
            className="activation-confirm-button"
            onClick={onClose}
          >
            Keep existing content
          </button>
        </div>
      </section>
    </div>
  );
}

function HealthNotice({ detail }: { detail: SkillDetail }) {
  const broken = detail.health === "broken";
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
