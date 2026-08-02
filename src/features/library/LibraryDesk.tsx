import { useEffect, useLayoutEffect, useRef } from "react";

import type {
  ActivationPreview,
  AgentActivation,
  CatalogFilter,
  Health,
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
  onFilter: (filter: CatalogFilter) => void;
  onSelect: (skillId: string) => void;
  onRequestActivation: (agentId: string, enabled: boolean) => void;
  onRequestActivationRepair: (agentId: string) => void;
  onApplyActivation: () => void;
  onCancelActivation: () => void;
  onCloseActivationConflict: () => void;
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
  onFilter,
  onSelect,
  onRequestActivation,
  onRequestActivationRepair,
  onApplyActivation,
  onCancelActivation,
  onCloseActivationConflict,
}: LibraryDeskProps) {
  const activationWasOpen = useRef(false);
  const hasActivationOverlay = Boolean(activationPreview || activationConflict);
  useLayoutEffect(() => {
    if (hasActivationOverlay) {
      activationWasOpen.current = true;
    } else if (activationWasOpen.current) {
      if (activationTriggerControlId) {
        const trigger = document.getElementById(activationTriggerControlId);
        const fallback = document.getElementById(
          activationTriggerControlId.replace(
            "activation-repair-",
            "activation-",
          ),
        );
        (trigger ?? fallback)?.focus();
      }
      activationWasOpen.current = false;
    }
  }, [activationTriggerControlId, hasActivationOverlay]);

  return (
    <div className="app-shell">
      <div
        className="app-background"
        inert={hasActivationOverlay ? true : undefined}
      >
        <a className="skip-link" href="#skill-detail">
          Skip to Skill detail
        </a>
        <Toolbar />
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
          <SkillDetailPanel detail={detail} />
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
    </div>
  );
}

function Toolbar() {
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
        <button type="button" className="toolbar-button" disabled>
          Adopt
        </button>
        <button type="button" className="primary-button" disabled>
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

function SkillDetailPanel({ detail }: { detail: SkillDetail | null }) {
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
            const isSupported = agent.kind === "claude_preset";
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
                    className={`switch-control${isSupported ? " switch-control--interactive" : ""}`}
                  >
                    <input
                      id={activationControlId(detail.id, agent.id)}
                      type="checkbox"
                      role="switch"
                      aria-label={`Enable ${detail.directoryName} for ${agent.name}`}
                      checked={agent.desiredEnabled}
                      disabled={
                        !isSupported ||
                        !agent.detected ||
                        isPending ||
                        isApplying
                      }
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
                  {!isSupported ? (
                    <span className="compatibility-note">Later milestone</span>
                  ) : null}
                  {agent.compatibility === "unknown" ? (
                    <span className="compatibility-note">
                      Compatibility unknown
                    </span>
                  ) : null}
                  {isSupported &&
                  agent.detected &&
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
