import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
export type Appearance = "system" | "light" | "dark";
export interface AppearanceSnapshot {
  selection: Appearance;
  generation: number;
}
export interface AppearanceApi {
  getSnapshot(): Promise<AppearanceSnapshot>;
  setSelection(selection: Appearance): Promise<AppearanceSnapshot>;
  migrateLegacy(legacy: string | null): Promise<AppearanceSnapshot>;
  listenChanged(
    callback: (snapshot: AppearanceSnapshot) => void,
  ): Promise<() => void>;
}
const preview: AppearanceSnapshot = { selection: "system", generation: 0 };
export const appearanceApi: AppearanceApi = {
  getSnapshot: () =>
    isTauri() ? invoke("get_appearance_snapshot") : Promise.resolve(preview),
  setSelection: (selection) =>
    isTauri()
      ? invoke("set_appearance_selection", { selection })
      : Promise.resolve({ selection, generation: ++preview.generation }),
  migrateLegacy: (legacy) =>
    isTauri()
      ? invoke("migrate_appearance_legacy", { legacy })
      : Promise.resolve(preview),
  listenChanged: async (callback) =>
    isTauri()
      ? listen<AppearanceSnapshot>("appearance://changed", ({ payload }) =>
          callback(payload),
        )
      : () => {},
};
