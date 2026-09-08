import { isTauri } from "@tauri-apps/api/core";
import config from "../../src-tauri/tauri.conf.json";

// Browser fixtures can demonstrate updates. Native builds require a real key.
export function appUpdatesAvailable(): boolean {
  const key = config.plugins.updater.pubkey.trim();
  return (
    !isTauri() ||
    (key !== "" && key !== "UPDATER_PUBLIC_KEY_REQUIRED_FOR_RELEASE")
  );
}
