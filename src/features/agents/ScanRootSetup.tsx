import { useEffect, useState } from "react";
import type {
  AgentConfigurationPlan,
  AgentManagementSnapshot,
  AgentPreset,
  CatalogClient,
  PresetObservation,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import { agentFailureMessage, draftFromPreset } from "./AgentManagement";

// Generated IDs and shared-consumer projections may change between sequential
// creates. The paths, roles and filesystem effects approved by the user may not.
function reviewedEffects(plan: AgentConfigurationPlan) {
  const configuration = plan.configuration;
  return JSON.stringify({
    kind: plan.kind,
    configuration: configuration && {
      name: configuration.name,
      presetKey: configuration.presetKey,
      projectSkillsDir: configuration.projectSkillsDir,
      compatibility: configuration.compatibility,
      roots: configuration.roots.map(
        ({ configuredPath, role, pathIdentityKey }) => ({
          configuredPath,
          role,
          pathIdentityKey,
        }),
      ),
    },
    targetWillBeCreated: plan.targetWillBeCreated,
    blockers: plan.blockingActivationSkillIds,
    retained: plan.retainedActivationCount,
  });
}

export function ScanRootSetup({
  client,
  onReady,
  onBusyChange,
}: {
  client: CatalogClient;
  onReady: (ready: boolean) => void;
  onBusyChange: (busy: boolean) => void;
}) {
  const { t } = useLocale();
  const [snapshot, setSnapshot] = useState<AgentManagementSnapshot | null>(
    null,
  );
  const [observations, setObservations] = useState<PresetObservation[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [review, setReview] = useState<Array<{
    preset: AgentPreset;
    plan: AgentConfigurationPlan;
  }> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    Promise.all([
      client.getAgentManagementSnapshot(),
      client.refreshDetection(),
    ])
      .then(([configuration, observation]) => {
        if (!current) return;
        setSnapshot(configuration);
        setSelected((keys) =>
          keys.filter(
            (key) =>
              !configuration.configurations.some(
                (item) => item.presetKey === key,
              ),
          ),
        );
        setObservations(observation.detection.presetObservations);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (current) setError(agentFailureMessage(reason, t));
      });
    return () => {
      current = false;
    };
  }, [client, t, attempt]);

  useEffect(() => {
    onReady(
      !!snapshot?.configurations.length &&
        !busy &&
        !review &&
        selected.length === 0 &&
        !error,
    );
  }, [snapshot, busy, review, selected.length, error, onReady]);
  useEffect(() => {
    onBusyChange(busy);
    return () => onBusyChange(false);
  }, [busy, onBusyChange]);

  async function preview() {
    setBusy(true);
    setError(null);
    try {
      const plans = [];
      for (const preset of snapshot!.presets.filter((item) =>
        selected.includes(item.presetKey),
      )) {
        plans.push({
          preset,
          plan: await client.planCreateAgentConfiguration(
            draftFromPreset(preset),
          ),
        });
      }
      setReview(plans);
    } catch (reason) {
      setError(agentFailureMessage(reason, t));
    } finally {
      setBusy(false);
    }
  }

  async function confirm() {
    if (!review || busy) return;
    setBusy(true);
    setError(null);
    try {
      for (const { preset, plan } of review) {
        const latest = await client.getAgentManagementSnapshot();
        if (
          latest.configurations.some(
            (item) => item.presetKey === preset.presetKey,
          )
        )
          throw { error: { code: "plan_stale" } };
        // Each Apply changes the generation. Replan each remaining create and
        // reject changed effects, rather than applying stale batch tokens.
        const fresh = await client.planCreateAgentConfiguration(
          draftFromPreset(preset),
        );
        if (reviewedEffects(fresh) !== reviewedEffects(plan))
          throw { error: { code: "plan_stale" } };
        await client.applyAgentConfigurationPlan(fresh.planToken);
      }
    } catch (reason) {
      setError(agentFailureMessage(reason, t));
    } finally {
      setReview(null);
      try {
        const latest = await client.getAgentManagementSnapshot();
        setSnapshot(latest);
        setSelected((keys) =>
          keys.filter(
            (key) =>
              !latest.configurations.some((item) => item.presetKey === key),
          ),
        );
      } catch (reason) {
        setSnapshot(null);
        setError(agentFailureMessage(reason, t));
      }
      setBusy(false);
    }
  }

  return (
    <div className="scan-root-setup" aria-busy={busy}>
      <p>{t("scan.setup.hint")}</p>
      {snapshot?.configurations.length ? (
        <>
          <h3>{t("scan.setup.configured")}</h3>
          <ul className="scan-root-list">
            {snapshot.configurations.map((item) => (
              <li key={item.agentId}>
                <strong>{item.name}</strong>
                {item.roots.map((root) => (
                  <code key={root.rootId}>{root.configuredPath}</code>
                ))}
              </li>
            ))}
          </ul>
        </>
      ) : (
        <p>{t("scan.setup.empty")}</p>
      )}
      {review ? (
        <>
          <h3>{t("scan.setup.review")}</h3>
          <ul className="scan-root-list">
            {review.map(({ preset, plan }) => (
              <li key={preset.presetKey}>
                <strong>{plan.configuration?.name}</strong>
                {plan.configuration?.roots.map((root) => (
                  <div key={root.rootId}>
                    <span>
                      {t(
                        root.role === "activation_target"
                          ? "scan.setup.target"
                          : "scan.setup.scan_only",
                      )}
                    </span>
                    <code>{root.configuredPath}</code>
                  </div>
                ))}
                {plan.configuration?.projectSkillsDir && (
                  <small>
                    {t("scan.setup.project", {
                      path: plan.configuration.projectSkillsDir,
                    })}
                  </small>
                )}
                {plan.targetWillBeCreated && (
                  <p className="candidate-conflict">
                    {t("scan.setup.create_target")}
                  </p>
                )}
              </li>
            ))}
          </ul>
          <p>{t("scan.setup.effects")}</p>
          <div className="activation-sheet-actions">
            <button disabled={busy} onClick={() => setReview(null)}>
              {t("scan.setup.back")}
            </button>
            <button
              className="activation-confirm-button"
              disabled={busy}
              onClick={() => void confirm()}
            >
              {t("scan.setup.confirm")}
            </button>
          </div>
        </>
      ) : (
        <>
          <h3>{t("scan.setup.presets")}</h3>
          <ul className="scan-root-list">
            {snapshot?.presets
              .filter(
                (preset) =>
                  !snapshot.configurations.some(
                    (item) => item.presetKey === preset.presetKey,
                  ),
              )
              .map((preset) => {
                const state =
                  observations.find(
                    (item) => item.presetKey === preset.presetKey,
                  )?.state ?? "unknown";
                return (
                  <li key={preset.presetKey}>
                    <label>
                      <input
                        type="checkbox"
                        disabled={busy}
                        checked={selected.includes(preset.presetKey)}
                        onChange={(event) => {
                          setError(null);
                          setSelected((keys) =>
                            event.target.checked
                              ? [...keys, preset.presetKey]
                              : keys.filter((key) => key !== preset.presetKey),
                          );
                        }}
                      />
                      <span>
                        <strong>{preset.name}</strong>
                        <small>{t(`scan.setup.${state}`)}</small>
                        {preset.roots.map((path) => (
                          <code key={path}>{path}</code>
                        ))}
                      </span>
                    </label>
                  </li>
                );
              })}
          </ul>
          <button
            className="toolbar-button"
            disabled={busy || !selected.length}
            onClick={() => void preview()}
          >
            {t("scan.setup.review")}
          </button>
        </>
      )}
      {error && (
        <div role="alert">
          <p>{error}</p>
          <p>{t("scan.setup.partial")}</p>
          <button
            disabled={busy}
            onClick={() => {
              setReview(null);
              setAttempt((value) => value + 1);
            }}
          >
            {t("scan.setup.retry")}
          </button>
        </div>
      )}
    </div>
  );
}
