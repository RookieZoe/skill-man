import { expect, test, vi } from "vitest";
import { isTauri } from "@tauri-apps/api/core";
import config from "../../src-tauri/tauri.conf.json";
import { appUpdatesAvailable } from "./app-update-availability";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: vi.fn() }));

test.each([
  [true, "UPDATER_PUBLIC_KEY_REQUIRED_FOR_RELEASE", false],
  [true, "  ", false],
  [true, "configured-public-key", true],
  [false, "UPDATER_PUBLIC_KEY_REQUIRED_FOR_RELEASE", true],
])("updater availability for native=%s and key=%s", (native, key, expected) => {
  const original = config.plugins.updater.pubkey;
  try {
    vi.mocked(isTauri).mockReturnValue(native);
    config.plugins.updater.pubkey = key;
    expect(appUpdatesAvailable()).toBe(expected);
  } finally {
    config.plugins.updater.pubkey = original;
  }
});
