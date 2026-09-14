import { useEffect, useLayoutEffect, useRef } from "react";
import { MarkdownContent } from "../../ui/MarkdownContent";
import { OperationNotice } from "../../ui/OperationNotice";
import { RepositoryLink } from "../../ui/RepositoryLink";
import { useLocale } from "../locale/LocaleProvider";
import { formatByteSize } from "../locale/messages";
import type { AvailableAppUpdate } from "../../app/catalog-client";
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

export function AppUpdateHelp() {
  const { t } = useLocale();
  return (
    <div className="app-update-help">
      <p>{t("library.app_update.gatekeeper")}</p>
      <RepositoryLink
        url="https://github.com/RookieZoe/skill-man/releases/latest"
        label={t("library.app_update.manual_download")}
      />
    </div>
  );
}

export function AppUpdateSheet({
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
  const sheetRef = useRef<HTMLElement>(null);
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
    const focusCurrentControl = () => {
      if (panel.activity === "downloading") {
        cancelButton.current?.focus();
      } else {
        primaryButton.current?.focus();
      }
    };
    focusCurrentControl();
    // Preferences can unmount in the same commit when its Check now action
    // opens this sheet; its focus cleanup must not win over this modal.
    queueMicrotask(focusCurrentControl);
  }, [panel.activity]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !blocksDismissal) onClose();
      if (event.key === "Tab") {
        const controls = sheetRef.current?.querySelectorAll<HTMLElement>(
          'a[href],button:not([disabled]),[tabindex="0"]',
        );
        const first = controls?.[0];
        const last = controls?.[controls.length - 1];
        if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first?.focus();
        } else if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last?.focus();
        }
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [blocksDismissal, onClose]);

  if (!update)
    return (
      <div className="activation-sheet-backdrop app-update-backdrop">
        <section
          ref={sheetRef}
          className="activation-sheet app-update-sheet"
          role="dialog"
          aria-modal="true"
          aria-label={t("library.preferences.app_updates")}
        >
          <div className="activation-sheet-heading">
            <h2>{t("library.preferences.app_updates")}</h2>
          </div>
          <p role={panel.error ? "alert" : "status"}>
            {panel.error ??
              t(
                panel.activity === "checking"
                  ? "library.preferences.checking"
                  : panel.checkStatus === "up_to_date"
                    ? "library.preferences.up_to_date"
                    : "library.preferences.skipped",
              )}
          </p>
          <div className="activation-sheet-actions">
            <button ref={primaryButton} type="button" onClick={onClose}>
              {t("library.preferences.done")}
            </button>
          </div>
        </section>
      </div>
    );

  return (
    <div
      className="activation-sheet-backdrop app-update-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !blocksDismissal) onClose();
      }}
    >
      <section
        ref={sheetRef}
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
        <OperationNotice
          busy={panel.activity === "downloading" || blocksDismissal}
          cancellable={panel.activity === "downloading"}
          label={t(
            isInstalling
              ? "library.app_update.installing"
              : isCancelling
                ? "library.app_update.cancelling"
                : "library.app_update.downloading",
          )}
        />
        <div
          className="app-update-body"
          tabIndex={0}
          role="region"
          aria-label={t("library.app_update.release_notes")}
        >
          <dl className="app-update-details">
            <div>
              <dt>{t("library.app_update.archive_size")}</dt>
              <dd>{formatByteSize(locale, update.downloadSizeBytes)}</dd>
            </div>
            <div>
              <dt>{t("library.app_update.release_notes")}</dt>
              <dd className="markdown-body">
                <MarkdownContent
                  markdown={
                    update.releaseNotes || t("library.app_update.no_notes")
                  }
                />
              </dd>
            </div>
          </dl>
          <AppUpdateHelp />
        </div>
        {isReady ? (
          <div
            className="app-update-ready operation-message operation-message--success"
            role="status"
          >
            {t("library.app_update.ready")}
          </div>
        ) : null}
        {panel.error ? (
          <div
            className="activation-error operation-message operation-message--error"
            role="alert"
          >
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
