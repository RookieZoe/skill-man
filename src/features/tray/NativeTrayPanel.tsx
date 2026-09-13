import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { CatalogClient } from "../../app/catalog-client";
import { TrayPanel } from "./TrayPanel";
import "./tray-panel.css";

export function NativeTrayPanel({ client }: { client: CatalogClient }) {
  const [failed, setFailed] = useState(false);
  const ready = useCallback(() => {
    void invoke("tray_panel_ready").catch(() => setFailed(true));
  }, []);
  useEffect(() => {
    document.body.classList.add("tray-surface");
  }, []);
  const action = (action: "close" | "open_main" | "quit") => {
    void invoke("tray_panel_action", { action }).catch(() => setFailed(true));
  };
  return (
    <TrayPanel
      client={client}
      onReady={ready}
      nativeError={failed}
      onClose={() => action("close")}
      onOpenMain={() => action("open_main")}
      onQuit={() => action("quit")}
    />
  );
}
