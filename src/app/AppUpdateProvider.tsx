import {
  createContext,
  useContext,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CatalogClient } from "./catalog-client";
import type { AppUpdatePanelState } from "../features/app-update/AppUpdateSheet";
import { useLocale } from "../features/locale/LocaleProvider";
import { AppUpdateSheet } from "../features/app-update/AppUpdateSheet";
import type { MessageKey } from "../features/locale/messages";
import { appUpdatesAvailable } from "./app-update-availability";

type NativeUpdate = (force: boolean) => Promise<void>;
const nativeUpdate: NativeUpdate = (force) =>
  invoke("show_native_app_update", { force });
interface UpdateContext {
  appUpdatePanel: AppUpdatePanelState;
  checkAppUpdate: () => Promise<void>;
  checkAutomatically: () => Promise<void>;
  downloadAppUpdate: () => Promise<void>;
  installAppUpdate: () => Promise<void>;
  closeAppUpdate: () => Promise<void>;
}
const Context = createContext<UpdateContext | null>(null);
export function useAppUpdate() {
  const value = useContext(Context);
  if (!value) throw new Error("app_update_provider_missing");
  return value;
}
export function AppUpdateProvider(props: {
  client: CatalogClient;
  children: ReactNode;
  nativeUpdate?: NativeUpdate;
}) {
  const parent = useContext(Context);
  return parent ? props.children : <UpdateAuthority {...props} />;
}
function UpdateAuthority({
  client,
  children,
  nativeUpdate: showNativeUpdate = isTauri() ? nativeUpdate : undefined,
}: {
  client: CatalogClient;
  children: ReactNode;
  nativeUpdate?: NativeUpdate;
}) {
  const { t } = useLocale();
  const [appUpdatePanel, setAppUpdatePanel] = useState<
    Omit<AppUpdatePanelState, "error"> & { error: MessageKey | null }
  >({
    activity: "idle",
    update: null,
    checkStatus: null,
    error: null,
  });
  const [visible, setVisible] = useState(false);
  const running = useRef<Promise<void> | null>(null);
  const runningForced = useRef(false);
  const lastSkipped = useRef(false);
  const panelRef = useRef(appUpdatePanel);
  panelRef.current = appUpdatePanel;
  const check = (force: boolean): Promise<void> => {
    if (showNativeUpdate) {
      return showNativeUpdate(force).catch((reason: unknown) => {
        if (force) {
          setVisible(true);
          setAppUpdatePanel({
            activity: "idle",
            update: null,
            checkStatus: null,
            error: readAppUpdateError(reason),
          });
        }
      });
    }
    if (force) setVisible(true);
    if (running.current) {
      if (force && !runningForced.current)
        return running.current.then(() =>
          lastSkipped.current ? checkRef.current(true) : undefined,
        );
      return running.current;
    }
    if (
      [
        "available",
        "downloading",
        "ready",
        "cancelling",
        "installing",
      ].includes(panelRef.current.activity)
    )
      return Promise.resolve();
    if (!appUpdatesAvailable()) {
      if (force)
        setAppUpdatePanel({
          activity: "idle",
          update: null,
          checkStatus: null,
          error: "library.app_update.unsupported",
        });
      return Promise.resolve();
    }
    setAppUpdatePanel({
      activity: "checking",
      update: null,
      checkStatus: null,
      error: null,
    });
    runningForced.current = force;
    lastSkipped.current = false;
    const task = client.checkAppUpdate(force).then(
      (result) => {
        lastSkipped.current = result.status === "skipped";
        if (result.status === "available") {
          setAppUpdatePanel({
            activity: "available",
            update: result,
            checkStatus: null,
            error: null,
          });
          setVisible(true);
        } else
          setAppUpdatePanel({
            activity: "idle",
            update: null,
            checkStatus: result.status,
            error: null,
          });
      },
      (reason: unknown) =>
        setAppUpdatePanel({
          activity: "idle",
          update: null,
          checkStatus: null,
          error: readAppUpdateError(reason),
        }),
    );
    running.current = task.finally(() => {
      running.current = null;
    });
    return running.current;
  };
  const checkRef = useRef(check);
  checkRef.current = check;
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
          ? commandCode(reason) === "update_cancelled" ||
            state.activity === "cancelling"
            ? state
            : {
                ...state,
                activity: "available",
                error: readAppUpdateError(reason),
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
              error: readAppUpdateError(reason),
            }
          : state,
      );
    }
  }

  async function closeAppUpdate() {
    const update = appUpdatePanel.update;
    if (!update) {
      setVisible(false);
      return;
    }
    if (
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
      setVisible(false);
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
              error: readAppUpdateError(reason),
            }
          : state,
      );
    }
  }

  const presentation = {
    ...appUpdatePanel,
    error: appUpdatePanel.error ? t(appUpdatePanel.error) : null,
  };
  return (
    <Context.Provider
      value={{
        appUpdatePanel: presentation,
        checkAppUpdate: () => check(true),
        checkAutomatically: () => check(false),
        downloadAppUpdate,
        installAppUpdate,
        closeAppUpdate,
      }}
    >
      <div style={{ display: "contents" }} inert={visible}>
        {children}
      </div>
      {visible && (
        <AppUpdateSheet
          panel={presentation}
          onDownload={downloadAppUpdate}
          onInstall={installAppUpdate}
          onClose={closeAppUpdate}
        />
      )}
    </Context.Provider>
  );
}
function commandCode(reason: unknown): string {
  if (typeof reason === "object" && reason !== null) {
    if ("code" in reason && typeof reason.code === "string") return reason.code;
    if ("error" in reason) return commandCode(reason.error);
  }
  return "internal";
}
function readAppUpdateError(reason: unknown): MessageKey {
  const code = commandCode(reason);
  switch (code) {
    case "source_unavailable":
      return "app.update_error.source_unavailable";
    case "state_unavailable":
      return "app.update_error.state_unavailable";
    case "stale_update":
      return "app.update_error.stale_update";
    case "download_failed":
      return "app.update_error.download_failed";
    case "install_failed":
      return "app.update_error.install_failed";
    case "update_cancelled":
      return "app.update_error.cancelled";
    default:
      return "app.update_error.generic";
  }
}
