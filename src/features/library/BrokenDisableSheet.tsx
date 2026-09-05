import { useEffect, useLayoutEffect, useRef, useState } from "react";

import type {
  CatalogClient,
  GlobalTargetGroupSnapshot,
} from "../../app/catalog-client";
import { useModalFocus } from "../../ui/useModalFocus";
import { useLocale } from "../locale/LocaleProvider";
import { commandErrorMessage } from "../locale/messages";

export interface BrokenDisableSheetProps {
  client: CatalogClient;
  skillId: string;
  skillPath: string;
  opener?: HTMLElement | null;
  onClose: () => void;
}

type Step = "evidence" | "confirm" | "result";

/**
 * Dedicated Broken Member Disable three-step sheet (spec §7.4, §7.5; ADR-0018, ADR-0021):
 * - Step 1: Target group & Broken evidence (vanished from Source Release, reappear notice)
 * - Step 2: Confirm disabling shared target group and list all consumer agents
 * - Step 3: Result
 *
 * Implements §7.2 overlay/focus/drawer contract: Escape, return-focus to opener,
 * backdrop dismissal when not busy, and scrollable container on low viewport height.
 */
export function BrokenDisableSheet({
  client,
  skillId,
  skillPath,
  opener,
  onClose,
}: BrokenDisableSheetProps) {
  const { t } = useLocale();
  const [step, setStep] = useState<Step>("evidence");
  const [groupsSnapshot, setGroupsSnapshot] =
    useState<GlobalTargetGroupSnapshot | null>(null);
  const [isBusy, setIsBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const primaryButtonRef = useRef<HTMLButtonElement>(null);
  const modalRef = useModalFocus<HTMLElement>({
    opener,
    busy: isBusy,
    focusKey: `${step}:${groupsSnapshot !== null}`,
    onClose: handleClose,
  });

  // Return focus to opener on unmount / close
  function handleClose() {
    if (isBusy) return;
    opener?.focus();
    onClose();
  }

  // Load target groups for this skill
  useEffect(() => {
    let current = true;
    client
      .listTargetGroups(skillId)
      .then((next) => {
        if (!current) return;
        setGroupsSnapshot(next);
        setError(null);
      })
      .catch((cause) => {
        if (!current) return;
        setError(commandErrorMessage(cause, t));
      });
    return () => {
      current = false;
    };
  }, [client, skillId, t]);

  // Auto-focus primary action button on step transition
  useLayoutEffect(() => {
    primaryButtonRef.current?.focus();
  }, [step, groupsSnapshot]);

  // The target groups where this skill was active/desired
  const activeGroups = groupsSnapshot?.groups.filter((g) => g.desired) ?? [];

  async function handleApply() {
    setIsBusy(true);
    setError(null);
    try {
      for (const group of activeGroups) {
        const plan = await client.planGlobalLifecycle(
          skillId,
          group.targetRootId,
          "disable",
        );
        await client.applyGlobalEnable(plan.planToken);
      }
      setStep("result");
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    } finally {
      setIsBusy(false);
    }
  }

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !isBusy) {
          handleClose();
        }
      }}
    >
      <section
        ref={modalRef}
        className="activation-sheet broken-disable-sheet"
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        aria-label={t("enable.broken.dialogLabel")}
      >
        <ol
          className="import-progress"
          aria-label={t("enable.broken.progressLabel")}
        >
          <li aria-current={step === "evidence" ? "step" : undefined}>
            {t("enable.broken.stepEvidence")}
          </li>
          <li aria-current={step === "confirm" ? "step" : undefined}>
            {t("enable.broken.stepConfirm")}
          </li>
          <li aria-current={step === "result" ? "step" : undefined}>
            {t("enable.broken.stepResult")}
          </li>
        </ol>

        {error ? (
          <p className="activation-error" role="alert">
            {error}
          </p>
        ) : null}

        {step === "evidence" ? (
          <div className="broken-step-content">
            <div className="activation-sheet-heading">
              <span className="eyebrow">{t("enable.broken.dialogLabel")}</span>
              <h2>{t("enable.broken.evidenceTitle")}</h2>
              <p>{t("enable.broken.evidenceIntro", { path: skillPath })}</p>
            </div>

            <div className="broken-notice" role="status">
              <p>{t("enable.broken.reappearNotice")}</p>
            </div>

            <div className="broken-target-groups">
              {activeGroups.map((group) => {
                const consumers = group.consumers
                  .map((c) => c.agentName)
                  .join(", ");
                return (
                  <div key={group.targetRootId} className="broken-target-card">
                    <strong>
                      {t("enable.broken.targetGroupTitle", {
                        path: group.configuredPath,
                      })}
                    </strong>
                    <p>
                      {t("enable.broken.consumersTitle", {
                        consumers,
                      })}
                    </p>
                  </div>
                );
              })}
            </div>

            <div className="activation-sheet-actions">
              <button type="button" disabled={isBusy} onClick={handleClose}>
                {t("enable.broken.close")}
              </button>
              <button
                ref={primaryButtonRef}
                type="button"
                className="activation-confirm-button"
                disabled={isBusy || groupsSnapshot === null}
                onClick={() => setStep("confirm")}
              >
                {t("enable.broken.continue")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "confirm" ? (
          <div className="broken-step-content">
            <div className="activation-sheet-heading">
              <span className="eyebrow">{t("enable.broken.dialogLabel")}</span>
              <h2>{t("enable.broken.confirmTitle")}</h2>
            </div>

            <div className="broken-confirm-groups">
              {activeGroups.map((group) => {
                const consumers = group.consumers
                  .map((c) => c.agentName)
                  .join(", ");
                return (
                  <div key={group.targetRootId} className="broken-confirm-card">
                    <p>
                      {t("enable.broken.groupDisableBody", {
                        consumers,
                      })}
                    </p>
                  </div>
                );
              })}
            </div>

            <div className="activation-sheet-actions">
              <button
                type="button"
                disabled={isBusy}
                onClick={() => setStep("evidence")}
              >
                {t("enable.broken.back")}
              </button>
              <button
                ref={primaryButtonRef}
                type="button"
                className="activation-confirm-button danger-button"
                disabled={isBusy}
                onClick={() => void handleApply()}
              >
                {isBusy
                  ? t("enable.broken.applying")
                  : t("enable.broken.apply")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "result" ? (
          <div className="broken-step-content">
            <div className="activation-sheet-heading">
              <span className="eyebrow">{t("enable.broken.dialogLabel")}</span>
              <h2>{t("enable.broken.resultSuccessTitle")}</h2>
              <p>{t("enable.broken.resultSuccessBody")}</p>
            </div>

            <div className="activation-sheet-actions">
              <button
                ref={primaryButtonRef}
                type="button"
                className="activation-confirm-button"
                onClick={handleClose}
              >
                {t("enable.broken.close")}
              </button>
            </div>
          </div>
        ) : null}
      </section>
    </div>
  );
}
