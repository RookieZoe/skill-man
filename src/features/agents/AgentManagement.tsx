import { OperationNotice } from "../../ui/OperationNotice";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";

import type {
  AgentConfiguration,
  AgentConfigurationDraft,
  AgentConfigurationPlan,
  AgentManagementSnapshot,
  AgentPreset,
  CatalogClient,
  CommandFailure,
  ObservationAndScanSnapshot,
  PresetObservation,
  PublicError,
} from "../../app/catalog-client";
import type { LayoutMode } from "../library/LibraryDesk";
import { useModalFocus } from "../../ui/useModalFocus";
import { useLocale, type LocaleContextValue } from "../locale/LocaleProvider";

interface AgentManagementProps {
  client: CatalogClient;
  layoutMode: LayoutMode;
  onOverlayChange?: (open: boolean) => void;
  onCatalogChanged?: () => Promise<void>;
}

type AgentSection = "configured" | "detected" | "presets";
type AgentSelection =
  { kind: "configuration"; id: string } | { kind: "preset"; id: string } | null;
type SheetMode = "create" | "edit" | "delete";
type NarrowPane = "list" | "detail";

interface SheetState {
  mode: SheetMode;
  agentId: string | null;
  draft: AgentConfigurationDraft;
  plan: AgentConfigurationPlan | null;
  blockerNames?: Record<string, string>;
  targetRootId?: string;
}

export function AgentManagement({
  client,
  layoutMode,
  onOverlayChange,
  onCatalogChanged,
}: AgentManagementProps) {
  const { t } = useLocale();
  const [snapshot, setSnapshot] = useState<AgentManagementSnapshot | null>(
    null,
  );
  const [section, setSection] = useState<AgentSection>("configured");
  const [selection, setSelection] = useState<AgentSelection>(null);
  const [narrowPane, setNarrowPane] = useState<NarrowPane>("list");
  const [detailDrawerOpen, setDetailDrawerOpen] = useState(false);
  const [sheet, setSheet] = useState<SheetState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [observation, setObservation] =
    useState<ObservationAndScanSnapshot | null>(null);
  const [detectionRefreshing, setDetectionRefreshing] = useState(false);
  const sheetOpener = useRef<HTMLElement | null>(null);
  const detailDrawerRef = useRef<HTMLElement | null>(null);
  const drawerFocusPending = useRef(false);
  const detailDrawerModal =
    layoutMode === "mid" && detailDrawerOpen && sheet === null;

  useEffect(() => {
    if (layoutMode !== "mid" || !detailDrawerOpen || sheet !== null) {
      return;
    }
    const drawer = detailDrawerRef.current;
    if (drawerFocusPending.current) {
      drawer?.querySelector<HTMLButtonElement>(".agent-detail-close")?.focus();
      drawerFocusPending.current = false;
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        closeDetailDrawer();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = Array.from(
        drawer?.querySelectorAll<HTMLElement>(
          "[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[contenteditable],[tabindex]",
        ) ?? [],
      ).filter((element) => {
        if (element.getAttribute("tabindex") === "-1") return false;
        return (
          (element instanceof HTMLButtonElement ||
            element instanceof HTMLInputElement ||
            element instanceof HTMLSelectElement ||
            element instanceof HTMLTextAreaElement) &&
          !element.disabled
        );
      });
      if (focusable.length === 0) return;
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
  }, [detailDrawerOpen, layoutMode, sheet]);

  useEffect(() => {
    onOverlayChange?.(sheet !== null || detailDrawerModal);
    return () => onOverlayChange?.(false);
  }, [detailDrawerModal, onOverlayChange, sheet]);

  useEffect(() => {
    let current = true;
    client
      .getAgentManagementSnapshot()
      .then((next) => {
        if (!current) return;
        setSnapshot(next);
        setSelection((selected) => selected ?? initialSelection(next));
      })
      .catch((reason: unknown) => {
        if (current) setError(agentFailureMessage(reason, t));
      });
    return () => {
      current = false;
    };
  }, [client, t]);

  // Detection lifecycle (ADR-0020): opening Agent Management is a trigger,
  // single-flight on the backend; the in-memory result also arrives through
  // `observation://changed`. Evidence is read-only and never configured
  // automatically.
  useEffect(() => {
    let current = true;
    let unlisten: (() => void) | null = null;
    client
      .getObservationSnapshot()
      .then((next) => {
        if (current) setObservation(next);
      })
      .catch((reason: unknown) => {
        if (current) setError(agentFailureMessage(reason, t));
      });
    client
      .listenObservationChanged((payload) => {
        if (current) setObservation(payload);
      })
      .then((stop) => {
        if (!current) stop();
        else unlisten = stop;
      })
      .catch((reason: unknown) => {
        if (current) setError(agentFailureMessage(reason, t));
      });
    client
      .refreshDetection()
      .then((next) => {
        if (current) setObservation(next);
      })
      .catch((reason: unknown) => {
        if (current) setError(agentFailureMessage(reason, t));
      });
    return () => {
      current = false;
      unlisten?.();
    };
  }, [client, t]);

  async function refreshDetection() {
    setDetectionRefreshing(true);
    try {
      setObservation(await client.refreshDetection());
    } catch (reason) {
      // Per-root probe failures already surface as Unavailable evidence; a
      // whole-command failure is surfaced like every other inline error.
      setError(agentFailureMessage(reason, t));
    } finally {
      setDetectionRefreshing(false);
    }
  }

  const configuredPresetKeys = useMemo(
    () =>
      new Set(
        snapshot?.configurations
          .map((configuration) => configuration.presetKey)
          .filter((key): key is string => key !== null) ?? [],
      ),
    [snapshot],
  );
  const detectedObservations = useMemo(
    () =>
      (observation?.detection.presetObservations ?? []).filter(
        (preset) =>
          preset.state !== "absent" &&
          preset.state !== "unknown" &&
          !configuredPresetKeys.has(preset.presetKey),
      ),
    [observation, configuredPresetKeys],
  );
  // Generation zero is the honest pre-run state: evidence is not the same
  // as absence, so the pending Run is never rendered as "no agents".
  const detectionPending =
    observation === null || observation.detection.generation === 0;
  const availablePresets = snapshot?.presets ?? [];
  const selectedConfiguration =
    selection?.kind === "configuration"
      ? (snapshot?.configurations.find(
          (configuration) => configuration.agentId === selection.id,
        ) ?? null)
      : null;
  const selectedPreset =
    selection?.kind === "preset"
      ? (snapshot?.presets.find(
          (preset) => preset.presetKey === selection.id,
        ) ?? null)
      : null;

  function selectSection(next: AgentSection) {
    setSection(next);
    setNarrowPane("list");
    if (next === "configured") {
      const first = snapshot?.configurations[0];
      setSelection(first ? { kind: "configuration", id: first.agentId } : null);
    } else if (next === "presets") {
      const first = availablePresets[0];
      setSelection(first ? { kind: "preset", id: first.presetKey } : null);
    } else {
      setSelection(null);
    }
    setDetailDrawerOpen(false);
  }

  function selectConfiguration(configuration: AgentConfiguration) {
    setSelection({ kind: "configuration", id: configuration.agentId });
    if (layoutMode === "mid") {
      drawerFocusPending.current = true;
      setDetailDrawerOpen(true);
    }
    if (layoutMode === "narrow") setNarrowPane("detail");
  }

  function selectPreset(preset: AgentPreset) {
    setSelection({ kind: "preset", id: preset.presetKey });
    if (layoutMode === "mid") {
      drawerFocusPending.current = true;
      setDetailDrawerOpen(true);
    }
    if (layoutMode === "narrow") setNarrowPane("detail");
  }

  function openCustomSheet() {
    sheetOpener.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setSheet({
      mode: "create",
      agentId: null,
      draft: {
        presetKey: null,
        name: "",
        roots: [{ configuredPath: "", role: "activation_target" }],
        projectSkillsDir: null,
      },
      plan: null,
    });
    setError(null);
  }

  function openPresetSheet(preset: AgentPreset) {
    const existing = snapshot?.configurations.find(
      (configuration) => configuration.presetKey === preset.presetKey,
    );
    if (existing) {
      openEditSheet(existing);
      return;
    }
    sheetOpener.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setSheet({
      mode: "create",
      agentId: null,
      draft: draftFromPreset(preset),
      plan: null,
    });
    setError(null);
  }

  function openEditSheet(configuration: AgentConfiguration) {
    sheetOpener.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setSheet({
      mode: "edit",
      agentId: configuration.agentId,
      draft: draftFromConfiguration(configuration),
      plan: null,
    });
    setError(null);
  }

  function openDeleteSheet(configuration: AgentConfiguration) {
    sheetOpener.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setSheet({
      mode: "delete",
      agentId: configuration.agentId,
      targetRootId: configuration.roots.find(
        (root) => root.role === "activation_target",
      )?.rootId,
      draft: draftFromConfiguration(configuration),
      plan: null,
    });
    setError(null);
  }

  async function reviewSheet() {
    if (!sheet) return;
    setBusy(true);
    setError(null);
    try {
      const plan =
        sheet.mode === "create"
          ? await client.planCreateAgentConfiguration(sheet.draft)
          : sheet.mode === "edit" && sheet.agentId
            ? await client.planEditAgentConfiguration(
                sheet.agentId,
                sheet.draft,
              )
            : sheet.agentId
              ? await client.planDeleteAgentConfiguration(sheet.agentId)
              : null;
      if (!plan) return;
      const blockerNames = await readBlockerNames(plan);
      setSheet((current) =>
        current ? { ...current, plan, blockerNames } : current,
      );
    } catch (reason) {
      setError(agentFailureMessage(reason, t));
    } finally {
      setBusy(false);
    }
  }

  async function readBlockerNames(plan: AgentConfigurationPlan) {
    if (!plan.blockingActivationSkillIds.length) return {};
    const catalog = await client.listSkills("all");
    return Object.fromEntries(
      catalog.items.map((skill) => [
        skill.id,
        skill.displayName || skill.directoryName,
      ]),
    );
  }

  async function removeTogether() {
    if (!sheet?.agentId || sheet.mode !== "delete" || !sheet.plan || busy)
      return;
    const agentId = sheet.agentId;
    const reviewed = new Set(sheet.plan.blockingActivationSkillIds);
    setBusy(true);
    setError(null);
    try {
      const fresh = await client.planDeleteAgentConfiguration(agentId);
      const current = await client.getAgentManagementSnapshot();
      const target = current.configurations
        .find((agent) => agent.agentId === agentId)
        ?.roots.find((root) => root.role === "activation_target");
      if (
        !target ||
        target.rootId !== sheet.targetRootId ||
        target.consumerAgentIds.length !== 1 ||
        target.consumerAgentIds[0] !== agentId ||
        fresh.blockingActivationSkillIds.some((id) => !reviewed.has(id))
      ) {
        throw { code: "plan_stale" };
      }
      for (const skillId of fresh.blockingActivationSkillIds) {
        const plan = await client.planGlobalLifecycle(
          skillId,
          target.rootId,
          "disable",
        );
        if (
          plan.cells.length !== 1 ||
          plan.cells[0].skillId !== skillId ||
          plan.cells[0].targetRootId !== target.rootId ||
          plan.cells[0].action !== "disable" ||
          plan.cells[0].affectedAgentIds.length !== 1 ||
          plan.cells[0].affectedAgentIds[0] !== agentId
        )
          throw { code: "plan_stale" };
        const result = await client.applyGlobalEnable(plan.planToken);
        const cell = result.cells[0];
        if (
          result.cells.length !== 1 ||
          cell.skillId !== skillId ||
          cell.targetRootId !== target.rootId ||
          !["succeeded", "no_op"].includes(cell.outcome)
        ) {
          throw { code: "disable_incomplete" };
        }
        await client.finalizeGlobalEnable(result.operationId);
      }
      const next = await client.planDeleteAgentConfiguration(agentId);
      setSheet((current) => (current ? { ...current, plan: next } : current));
      if (next.blockingActivationSkillIds.length) throw { code: "plan_stale" };
      await applySheet(next);
    } catch {
      setError(t("agents.sheet.remove_together_failed"));
      // Earlier successful Disable operations stay applied. Never delete the
      // configuration after a partial failure; show the remaining blockers.
      try {
        const plan = await client.planDeleteAgentConfiguration(agentId);
        const blockerNames = await readBlockerNames(plan);
        setSheet((current) =>
          current ? { ...current, plan, blockerNames } : current,
        );
      } catch {
        setSheet((current) => (current ? { ...current, plan: null } : current));
      }
    } finally {
      setBusy(false);
      await onCatalogChanged?.();
    }
  }

  async function applySheet(plan = sheet?.plan) {
    if (!plan || plan.blockingActivationSkillIds.length > 0) return;
    setBusy(true);
    setError(null);
    try {
      const result = await client.applyAgentConfigurationPlan(plan.planToken);
      const next = await client.getAgentManagementSnapshot();
      setSnapshot(next);
      setSheet(null);
      sheetOpener.current = null;
      if (result.deleted) {
        const first = next.configurations[0];
        setSelection(
          first ? { kind: "configuration", id: first.agentId } : null,
        );
      } else {
        setSelection({ kind: "configuration", id: result.agentId });
        setSection("configured");
      }
      closeDetailDrawer();
    } catch (reason) {
      setError(agentFailureMessage(reason, t));
    } finally {
      setBusy(false);
    }
  }

  function closeSheet() {
    if (busy) return;
    const opener = sheetOpener.current;
    sheetOpener.current = null;
    setSheet(null);
    setError(null);
    queueMicrotask(() => opener?.focus());
  }

  function closeDetailDrawer() {
    setDetailDrawerOpen(false);
    queueMicrotask(() => {
      document
        .querySelector<HTMLElement>(".agent-detail-floating-trigger")
        ?.focus();
    });
  }

  const detail = (
    <AgentDetail
      snapshot={snapshot}
      configuration={selectedConfiguration}
      preset={selectedPreset}
      onEdit={openEditSheet}
      onDelete={openDeleteSheet}
      onConfigure={openPresetSheet}
    />
  );

  return (
    <>
      <section
        id="agent-management"
        className="agent-management"
        data-layout-mode={layoutMode}
        data-narrow-pane={narrowPane}
        aria-label={t("agents.surface.label")}
      >
        {layoutMode === "narrow" ? (
          <div
            className="agent-management-pane-nav"
            role="group"
            aria-label={t("agents.nav.pane_label")}
          >
            {(["list", "detail"] as const).map((pane) => (
              <button
                key={pane}
                type="button"
                aria-pressed={narrowPane === pane}
                onClick={() => setNarrowPane(pane)}
              >
                {t(`agents.nav.pane.${pane}`)}
              </button>
            ))}
          </div>
        ) : null}

        <AgentNavigation
          section={section}
          configuredCount={snapshot?.configurations.length ?? 0}
          detectedCount={detectedObservations.length}
          presetCount={availablePresets.length}
          onSelect={selectSection}
          onNewCustom={openCustomSheet}
        />

        <AgentList
          section={section}
          snapshot={snapshot}
          detectedObservations={detectedObservations}
          detectionPending={detectionPending}
          detectionRefreshing={detectionRefreshing}
          onRefreshDetection={refreshDetection}
          availablePresets={availablePresets}
          selection={selection}
          error={error}
          onSelectConfiguration={selectConfiguration}
          onSelectPreset={selectPreset}
          onConfigureDetected={openPresetSheet}
          onNewCustom={openCustomSheet}
        />

        {layoutMode === "wide" ? (
          <aside className="agent-management-detail">{detail}</aside>
        ) : null}
        {layoutMode === "narrow" ? (
          <aside className="agent-management-detail">{detail}</aside>
        ) : null}
        {layoutMode === "mid" ? (
          <>
            {selection && !detailDrawerOpen ? (
              <button
                type="button"
                className="agent-detail-floating-trigger"
                onClick={() => {
                  drawerFocusPending.current = true;
                  setDetailDrawerOpen(true);
                }}
              >
                {t("agents.detail.open")}
              </button>
            ) : null}
            {createPortal(
              <div
                className="agent-detail-drawer"
                data-open={detailDrawerOpen}
                aria-hidden={!detailDrawerOpen}
              >
                <button
                  type="button"
                  className="agent-detail-drawer-backdrop"
                  aria-label={t("agents.detail.close")}
                  onClick={closeDetailDrawer}
                />
                <aside
                  ref={detailDrawerRef}
                  className="agent-management-detail"
                  role="dialog"
                  aria-modal="true"
                  aria-label={t("agents.detail.label")}
                >
                  <button
                    type="button"
                    className="agent-detail-close"
                    onClick={closeDetailDrawer}
                  >
                    {t("agents.detail.close")}
                  </button>
                  {detail}
                </aside>
              </div>,
              document.body,
            )}
          </>
        ) : null}
      </section>
      {sheet
        ? createPortal(
            <AgentConfigurationSheet
              state={sheet}
              busy={busy}
              error={error}
              opener={sheetOpener.current}
              onChange={(next) => {
                setSheet({ ...next, plan: null });
                setError(null);
              }}
              onReview={reviewSheet}
              onApply={applySheet}
              onRemoveTogether={removeTogether}
              onClose={closeSheet}
            />,
            document.body,
          )
        : null}
    </>
  );
}

function AgentNavigation({
  section,
  configuredCount,
  detectedCount,
  presetCount,
  onSelect,
  onNewCustom,
}: {
  section: AgentSection;
  configuredCount: number;
  detectedCount: number;
  presetCount: number;
  onSelect: (section: AgentSection) => void;
  onNewCustom: () => void;
}) {
  const { t } = useLocale();
  const rows: Array<{ section: AgentSection; count: number }> = [
    { section: "configured", count: configuredCount },
    { section: "detected", count: detectedCount },
    { section: "presets", count: presetCount },
  ];
  return (
    <nav className="agents-navigation" aria-label={t("agents.nav.label")}>
      <div className="agents-navigation-groups">
        {rows.map((row) => (
          <button
            key={row.section}
            type="button"
            aria-current={section === row.section ? "page" : undefined}
            onClick={() => onSelect(row.section)}
          >
            <span>{t(`agents.nav.${row.section}`)}</span>
            <strong>{row.count}</strong>
          </button>
        ))}
      </div>
      <button
        type="button"
        className="toolbar-button primary-button agents-new-button"
        onClick={onNewCustom}
      >
        {t("agents.nav.new_custom_agent")}
      </button>
    </nav>
  );
}

function AgentList({
  section,
  snapshot,
  detectedObservations,
  detectionPending,
  detectionRefreshing,
  onRefreshDetection,
  availablePresets,
  selection,
  error,
  onSelectConfiguration,
  onSelectPreset,
  onConfigureDetected,
  onNewCustom,
}: {
  section: AgentSection;
  snapshot: AgentManagementSnapshot | null;
  detectedObservations: PresetObservation[];
  detectionPending: boolean;
  detectionRefreshing: boolean;
  onRefreshDetection: () => void;
  availablePresets: AgentPreset[];
  selection: AgentSelection;
  error: string | null;
  onSelectConfiguration: (configuration: AgentConfiguration) => void;
  onSelectPreset: (preset: AgentPreset) => void;
  onConfigureDetected: (preset: AgentPreset) => void;
  onNewCustom: () => void;
}) {
  const { t } = useLocale();
  return (
    <main className="agents-list-pane" aria-label={t("agents.list.label")}>
      <header className="agents-list-heading">
        <h2>{t(`agents.list.${section}_title`)}</h2>
        {section === "detected" ? (
          <button
            type="button"
            className="agents-detection-refresh"
            disabled={detectionRefreshing}
            onClick={onRefreshDetection}
          >
            {t("agents.list.detected_refresh")}
          </button>
        ) : null}
      </header>
      {error ? (
        <div className="agents-inline-error" role="alert">
          {error}
        </div>
      ) : null}
      {!snapshot ? (
        <div className="agents-empty" role="status">
          {t("agents.list.loading")}
        </div>
      ) : null}
      {snapshot && section === "configured" ? (
        snapshot.configurations.length > 0 ? (
          <div className="agent-configuration-list">
            {snapshot.configurations.map((configuration) => {
              const target = configuration.roots.find(
                (root) => root.role === "activation_target",
              );
              return (
                <button
                  key={configuration.agentId}
                  type="button"
                  aria-pressed={
                    selection?.kind === "configuration" &&
                    selection.id === configuration.agentId
                  }
                  onClick={() => onSelectConfiguration(configuration)}
                >
                  <span className="agent-list-name-row">
                    <strong>{configuration.name}</strong>
                    <em>{configuration.roots.length}</em>
                  </span>
                  <span className="agent-list-target">
                    {target?.configuredPath ?? t("agents.list.target_missing")}
                  </span>
                  <span className="agent-list-meta">
                    {t("agents.list.root_count", {
                      count: configuration.roots.length,
                    })}
                  </span>
                </button>
              );
            })}
          </div>
        ) : (
          <div className="agents-empty agents-empty--fresh">
            <span className="agents-empty-mark" aria-hidden="true" />
            <h3>{t("agents.list.fresh_title")}</h3>
            <p>{t("agents.list.fresh_body")}</p>
            <button type="button" onClick={onNewCustom}>
              {t("agents.nav.new_custom_agent")}
            </button>
          </div>
        )
      ) : null}
      {snapshot && section === "detected" ? (
        detectionPending ? (
          <div className="agents-empty" role="status">
            {t("agents.list.detected_detecting")}
          </div>
        ) : detectedObservations.length > 0 ? (
          <div className="agent-detection-list">
            {detectedObservations.map((preset) => (
              <article key={preset.presetKey} className="agent-detection-card">
                <header className="agent-detection-card-heading">
                  <h3>{preset.name}</h3>
                  <span
                    className={`agent-detection-state agent-detection-state--${preset.state}`}
                  >
                    {t(`agents.list.detected_state_${preset.state}`)}
                  </span>
                </header>
                <p className="agent-detection-roots-label">
                  {t("agents.list.detectionEvidence")}
                </p>
                <ul className="agent-detection-roots">
                  {preset.roots.map((root) => (
                    <li key={root.configuredPath}>
                      <code>{root.configuredPath}</code>
                      <span
                        className={`agent-detection-root-state agent-detection-root-state--${root.state}`}
                      >
                        {t(`agents.list.detected_state_${root.state}`)}
                      </span>
                      {root.canonicalPath ? (
                        <span className="agent-detection-canonical">
                          <span className="agent-detection-canonical-label">
                            {t("agents.list.detected_canonical_label")}
                          </span>
                          <code>{root.canonicalPath}</code>
                        </span>
                      ) : null}
                      {root.diagnostic ? (
                        <details className="agent-detection-diagnostic">
                          <summary>
                            {t("agents.list.detected_diagnostic_title")}
                          </summary>
                          <code>{root.diagnostic}</code>
                        </details>
                      ) : null}
                    </li>
                  ))}
                </ul>
                {availablePresets.some(
                  (available) => available.presetKey === preset.presetKey,
                ) ? (
                  <button
                    type="button"
                    className="primary-button agent-detection-configure"
                    onClick={() => {
                      const template = availablePresets.find(
                        (available) => available.presetKey === preset.presetKey,
                      );
                      if (template) onConfigureDetected(template);
                    }}
                  >
                    {t("agents.detail.configure_template")}
                  </button>
                ) : null}
              </article>
            ))}
          </div>
        ) : (
          <div className="agents-empty">
            <h3>{t("agents.list.detected_empty_title")}</h3>
            <p>{t("agents.list.detected_empty_body")}</p>
          </div>
        )
      ) : null}
      {snapshot && section === "presets" ? (
        availablePresets.length > 0 ? (
          <div className="agent-preset-list">
            {availablePresets.map((preset) => (
              <button
                key={preset.presetKey}
                type="button"
                aria-pressed={
                  selection?.kind === "preset" &&
                  selection.id === preset.presetKey
                }
                onClick={() => onSelectPreset(preset)}
              >
                <span>
                  <strong>{preset.name}</strong>
                  <small>{preset.activationTarget}</small>
                </span>
                <em>
                  {t(
                    snapshot.configurations.some(
                      (item) => item.presetKey === preset.presetKey,
                    )
                      ? "agents.nav.configured"
                      : "agents.list.configure",
                  )}
                </em>
              </button>
            ))}
          </div>
        ) : (
          <div className="agents-empty">
            <h3>{t("agents.list.presets_empty_title")}</h3>
            <p>{t("agents.list.presets_empty_body")}</p>
          </div>
        )
      ) : null}
    </main>
  );
}

function AgentDetail({
  snapshot,
  configuration,
  preset,
  onEdit,
  onDelete,
  onConfigure,
}: {
  snapshot: AgentManagementSnapshot | null;
  configuration: AgentConfiguration | null;
  preset: AgentPreset | null;
  onEdit: (configuration: AgentConfiguration) => void;
  onDelete: (configuration: AgentConfiguration) => void;
  onConfigure: (preset: AgentPreset) => void;
}) {
  const { t } = useLocale();
  if (!configuration && !preset) {
    return (
      <div className="agent-detail-empty">
        <span aria-hidden="true" />
        <h2>{t("agents.detail.empty_title")}</h2>
        <p>{t("agents.detail.empty_body")}</p>
      </div>
    );
  }
  if (preset) {
    return (
      <div className="agent-detail-content">
        <header className="agent-detail-heading">
          <span className="eyebrow">{t("agents.detail.preset_eyebrow")}</span>
          <h2>{preset.name}</h2>
        </header>
        <CompatibilityEvidence compatibility={preset.compatibility} />
        <RootEvidenceList
          roots={preset.roots.map((configuredPath) => ({
            rootId: configuredPath,
            configuredPath,
            pathIdentityKey: configuredPath,
            role:
              configuredPath === preset.activationTarget
                ? "activation_target"
                : "scan_only",
            consumerAgentIds: [],
            activationSkillIds: [],
          }))}
        />
        <section className="agent-detail-card">
          <span className="agent-detail-card-label">
            {t("agents.detail.project_directory")}
          </span>
          <code>{preset.projectSkillsDir}</code>
        </section>
        <button
          type="button"
          className="primary-button agent-detail-primary-action"
          onClick={() => onConfigure(preset)}
        >
          {t(
            snapshot?.configurations.some(
              (item) => item.presetKey === preset.presetKey,
            )
              ? "agents.detail.edit"
              : "agents.detail.configure_template",
          )}
        </button>
      </div>
    );
  }
  if (!configuration) return null;
  const consumers = new Map(
    snapshot?.configurations.map((item) => [item.agentId, item.name]) ?? [],
  );
  const target = configuration.roots.find(
    (root) => root.role === "activation_target",
  );
  return (
    <div className="agent-detail-content">
      <header className="agent-detail-heading">
        <span className="eyebrow">{t("agents.detail.configured_eyebrow")}</span>
        <h2>{configuration.name}</h2>
      </header>
      <CompatibilityEvidence compatibility={configuration.compatibility} />
      <RootEvidenceList roots={configuration.roots} />
      <section className="agent-detail-card">
        <span className="agent-detail-card-label">
          {t("agents.detail.project_directory")}
        </span>
        {configuration.projectSkillsDir ? (
          <code>{configuration.projectSkillsDir}</code>
        ) : (
          <span className="agent-detail-muted">
            {t("agents.detail.project_directory_none")}
          </span>
        )}
      </section>
      <section className="agent-detail-card">
        <span className="agent-detail-card-label">
          {t("agents.detail.shared_consumers")}
        </span>
        <div className="shared-consumer-list">
          {(target?.consumerAgentIds ?? [configuration.agentId]).map(
            (agentId) => (
              <span key={agentId}>
                {consumers.get(agentId) ?? configuration.name}
              </span>
            ),
          )}
        </div>
      </section>
      <div className="agent-detail-actions">
        <button type="button" onClick={() => onEdit(configuration)}>
          {t("agents.detail.edit")}
        </button>
        <button
          type="button"
          className="danger-button"
          onClick={() => onDelete(configuration)}
        >
          {t("agents.detail.delete")}
        </button>
      </div>
    </div>
  );
}

function CompatibilityEvidence({
  compatibility,
}: {
  compatibility: AgentConfiguration["compatibility"];
}) {
  const { t } = useLocale();
  return (
    <section className="compatibility-evidence">
      <span aria-hidden="true" />
      <div>
        <strong>{t("agents.detail.compatibility")}</strong>
        <p>{t(`agents.detail.compatibility_${compatibility}`)}</p>
      </div>
      <em>{t(`agents.detail.compatibility_badge_${compatibility}`)}</em>
    </section>
  );
}

function RootEvidenceList({ roots }: { roots: AgentConfiguration["roots"] }) {
  const { t } = useLocale();
  return (
    <section className="agent-detail-card agent-root-evidence">
      <span className="agent-detail-card-label">
        {t("agents.detail.global_roots")}
      </span>
      <div>
        {roots.map((root) => (
          <div className="agent-root-evidence-row" key={root.rootId}>
            <span
              className={`target-radio target-radio--${root.role}`}
              aria-hidden="true"
            />
            <code>{root.configuredPath}</code>
            <small>
              {root.role === "activation_target"
                ? t("agents.detail.activation_target")
                : t("agents.detail.scan_only")}
            </small>
          </div>
        ))}
      </div>
    </section>
  );
}

function AgentConfigurationSheet({
  state,
  busy,
  error,
  opener,
  onChange,
  onReview,
  onApply,
  onRemoveTogether,
  onClose,
}: {
  state: SheetState;
  busy: boolean;
  error: string | null;
  opener?: HTMLElement | null;
  onChange: (state: SheetState) => void;
  onReview: () => void;
  onApply: () => void;
  onRemoveTogether: () => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const firstControl = useRef<HTMLInputElement | HTMLButtonElement>(null);
  const deleteMode = state.mode === "delete";
  const blockers = state.plan?.blockingActivationSkillIds ?? [];
  const modalRef = useModalFocus<HTMLFormElement>({
    opener,
    busy,
    focusKey: `${state.mode}:${state.plan?.planToken ?? "draft"}`,
    onClose,
  });

  function updateDraft(next: Partial<AgentConfigurationDraft>) {
    onChange({ ...state, draft: { ...state.draft, ...next }, plan: null });
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (state.plan) onApply();
    else onReview();
  }

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (!busy && event.target === event.currentTarget) onClose();
      }}
    >
      <form
        ref={modalRef}
        className="activation-sheet agent-configuration-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={t("agents.sheet.dialog")}
        tabIndex={-1}
        onSubmit={submit}
      >
        <div className="activation-sheet-heading">
          <span className="eyebrow">
            {t(`agents.sheet.${state.mode}_eyebrow`)}
          </span>
          <h2>{t(`agents.sheet.${state.mode}_title`)}</h2>
          <p>{t(`agents.sheet.${state.mode}_body`)}</p>
        </div>

        <OperationNotice busy={busy} />
        <div className="agent-sheet-body">
          {!deleteMode ? (
            <>
              <label className="agent-field">
                <span>{t("agents.sheet.name")}</span>
                <input
                  ref={firstControl as RefObject<HTMLInputElement>}
                  value={state.draft.name}
                  aria-label={t("agents.sheet.name")}
                  maxLength={80}
                  autoComplete="off"
                  onChange={(event) =>
                    updateDraft({ name: event.target.value })
                  }
                />
                <small>{t("agents.sheet.name_hint")}</small>
              </label>

              <fieldset className="agent-root-editor">
                <legend>{t("agents.sheet.global_roots")}</legend>
                <p>{t("agents.sheet.global_roots_hint")}</p>
                {state.draft.roots.map((root, index) => (
                  <div className="agent-root-editor-row" key={index}>
                    <input
                      type="radio"
                      name="agent-activation-target"
                      aria-label={t("agents.sheet.choose_target", {
                        index: index + 1,
                      })}
                      checked={root.role === "activation_target"}
                      onChange={() =>
                        updateDraft({
                          roots: state.draft.roots.map((item, itemIndex) => ({
                            ...item,
                            role:
                              itemIndex === index
                                ? "activation_target"
                                : "scan_only",
                          })),
                        })
                      }
                    />
                    <input
                      value={root.configuredPath}
                      aria-label={t("agents.sheet.root_path", {
                        index: index + 1,
                      })}
                      placeholder={t("agents.sheet.root_placeholder")}
                      onChange={(event) =>
                        updateDraft({
                          roots: state.draft.roots.map((item, itemIndex) =>
                            itemIndex === index
                              ? { ...item, configuredPath: event.target.value }
                              : item,
                          ),
                        })
                      }
                    />
                    <button
                      type="button"
                      aria-label={t("agents.sheet.remove_root", {
                        index: index + 1,
                      })}
                      disabled={state.draft.roots.length === 1}
                      onClick={() =>
                        updateDraft({
                          roots: state.draft.roots.filter(
                            (_, itemIndex) => itemIndex !== index,
                          ),
                        })
                      }
                    >
                      {t("agents.sheet.remove")}
                    </button>
                  </div>
                ))}
                <button
                  type="button"
                  className="agent-add-root"
                  onClick={() =>
                    updateDraft({
                      roots: [
                        ...state.draft.roots,
                        { configuredPath: "", role: "scan_only" },
                      ],
                    })
                  }
                >
                  {t("agents.sheet.add_root")}
                </button>
              </fieldset>

              <label className="agent-field">
                <span>{t("agents.sheet.project_directory")}</span>
                <input
                  value={state.draft.projectSkillsDir ?? ""}
                  aria-label={t("agents.sheet.project_directory")}
                  placeholder={t("agents.sheet.project_directory_placeholder")}
                  onChange={(event) =>
                    updateDraft({
                      projectSkillsDir: event.target.value || null,
                    })
                  }
                />
                <small>{t("agents.sheet.project_directory_hint")}</small>
              </label>
            </>
          ) : (
            <div className="agent-delete-confirmation">
              <strong>{state.draft.name}</strong>
              <p>{t("agents.sheet.delete_activation_retained")}</p>
            </div>
          )}

          {state.plan ? (
            <div className="agent-plan-review" role="status">
              <strong>{t("agents.sheet.review_title")}</strong>
              {state.plan.targetWillBeCreated ? (
                <p>{t("agents.sheet.target_will_be_created")}</p>
              ) : null}
              {state.plan.retainedActivationCount > 0 ? (
                <p>
                  {t("agents.sheet.retained_activations", {
                    count: state.plan.retainedActivationCount,
                  })}
                </p>
              ) : null}
              {blockers.length > 0 ? (
                <div className="agent-plan-blockers" role="alert">
                  <strong>{t("agents.sheet.blocked_title")}</strong>
                  <p>{t("agents.sheet.blocked_body")}</p>
                  <ul>
                    {blockers.map((skillId) => (
                      <li key={skillId}>
                        {state.blockerNames?.[skillId] ??
                          t("agents.sheet.skill_name_unavailable")}
                      </li>
                    ))}
                  </ul>
                  {deleteMode && (
                    <>
                      <p>{t("agents.sheet.remove_together_hint")}</p>
                      <button
                        type="button"
                        className="repair-button danger-button"
                        disabled={
                          busy ||
                          blockers.some((id) => !state.blockerNames?.[id])
                        }
                        onClick={onRemoveTogether}
                      >
                        {t("agents.sheet.remove_together")}
                      </button>
                    </>
                  )}
                </div>
              ) : (
                <p>{t("agents.sheet.review_ready")}</p>
              )}
            </div>
          ) : null}

          {error ? (
            <div className="agents-inline-error" role="alert">
              {error}
            </div>
          ) : null}
        </div>

        <div className="activation-sheet-actions">
          <button
            ref={
              deleteMode
                ? (firstControl as RefObject<HTMLButtonElement>)
                : undefined
            }
            type="button"
            disabled={busy}
            onClick={onClose}
          >
            {t("agents.sheet.cancel")}
          </button>
          <button
            type="submit"
            className={
              deleteMode ? "danger-button" : "activation-confirm-button"
            }
            disabled={busy || blockers.length > 0}
          >
            {busy
              ? t("agents.sheet.applying")
              : state.plan
                ? t(`agents.sheet.apply_${state.mode}`)
                : t("agents.sheet.review")}
          </button>
        </div>
      </form>
    </div>
  );
}

function initialSelection(snapshot: AgentManagementSnapshot): AgentSelection {
  const configuration = snapshot.configurations[0];
  if (configuration) {
    return { kind: "configuration", id: configuration.agentId };
  }
  const configuredPresets = new Set(
    snapshot.configurations
      .map((item) => item.presetKey)
      .filter((key): key is string => key !== null),
  );
  const preset = snapshot.presets.find(
    (item) => !configuredPresets.has(item.presetKey),
  );
  return preset ? { kind: "preset", id: preset.presetKey } : null;
}

function draftFromConfiguration(
  configuration: AgentConfiguration,
): AgentConfigurationDraft {
  return {
    presetKey: configuration.presetKey,
    name: configuration.name,
    roots: configuration.roots.map((root) => ({
      configuredPath: root.configuredPath,
      role: root.role,
    })),
    projectSkillsDir: configuration.projectSkillsDir,
  };
}

export function draftFromPreset(preset: AgentPreset): AgentConfigurationDraft {
  return {
    presetKey: preset.presetKey,
    name: preset.name,
    roots: preset.roots.map((configuredPath) => ({
      configuredPath,
      role:
        configuredPath === preset.activationTarget
          ? "activation_target"
          : "scan_only",
    })),
    projectSkillsDir: preset.projectSkillsDir,
  };
}

export function agentFailureMessage(
  reason: unknown,
  t: LocaleContextValue["t"],
): string {
  const failure = reason as Partial<CommandFailure>;
  const error = failure.error as PublicError | undefined;
  if (!error || typeof error.code !== "string") {
    return t("agents.error.generic");
  }
  switch (error.code) {
    case "recovery_required":
      return t("error.write_locked");
    case "agent_configuration_name_invalid":
      return t("agents.error.name_invalid");
    case "agent_configuration_name_conflict":
      return t("agents.error.name_conflict", { name: error.name });
    case "agent_root_required":
      return t("agents.error.root_required");
    case "agent_activation_target_required":
      return t("agents.error.target_required");
    case "agent_root_duplicate":
      return t("agents.error.root_duplicate", { path: error.path });
    case "agent_root_overlap":
      return t("agents.error.root_overlap", {
        path: error.path,
        conflictingPath: error.conflictingPath,
      });
    case "agent_root_home_overlap":
      return t("agents.error.home_overlap", { path: error.path });
    case "agent_root_invalid":
      return t("agents.error.root_invalid", { path: error.path });
    case "agent_target_not_writable":
      return t("agents.error.target_not_writable", { path: error.path });
    case "agent_target_not_allowed":
      return t("agents.error.target_not_allowed", { path: error.path });
    case "agent_project_skills_dir_invalid":
      return t("agents.error.project_directory_invalid");
    case "agent_configuration_not_found":
      return t("agents.error.not_found");
    case "agent_target_in_use":
      return t("agents.error.target_in_use");
    case "plan_stale":
      return t("agents.error.plan_stale");
    case "catalog_unavailable":
      return t("agents.error.catalog_unavailable");
    default:
      return t("agents.error.generic");
  }
}
