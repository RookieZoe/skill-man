import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { AppearanceProvider, AppearanceControl } from "./AppearanceProvider";
import type { AppearanceApi, AppearanceSnapshot } from "./appearance-api";

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
  delete document.documentElement.dataset.theme;
});
function authority() {
  let changed = (snapshot: AppearanceSnapshot) => {
    void snapshot;
  };
  const api: AppearanceApi = {
    getSnapshot: async () => ({ selection: "system", generation: 0 }),
    migrateLegacy: vi.fn(async (): Promise<AppearanceSnapshot> => ({
      selection: "system",
      generation: 0,
    })),
    setSelection: vi.fn(async (selection) => ({ selection, generation: 1 })),
    listenChanged: async (callback) => {
      changed = callback;
      return () => {};
    },
  };
  return { api, publish: (snapshot: AppearanceSnapshot) => changed(snapshot) };
}
test("native changes win over late responses without remounting children", async () => {
  const { api, publish } = authority();
  let resolve!: (s: AppearanceSnapshot) => void;
  api.setSelection = () =>
    new Promise((done) => {
      resolve = done;
    });
  render(
    <AppearanceProvider api={api}>
      <AppearanceControl />
      <input aria-label="draft" />
    </AppearanceProvider>,
  );
  await screen.findByRole("radio", { name: "Dark" });
  await userEvent.type(screen.getByRole("textbox"), "keep me");
  await userEvent.click(screen.getByRole("radio", { name: "Dark" }));
  act(() => publish({ selection: "light", generation: 2 }));
  await act(async () => resolve({ selection: "dark", generation: 1 }));
  expect(screen.getByRole("radio", { name: "Light" })).toBeChecked();
  expect(screen.getByRole("textbox")).toHaveValue("keep me");
  expect(document.documentElement.dataset.theme).toBe("light");
});
test("failed migration retains legacy and failed selection retains native state", async () => {
  localStorage.setItem("skill-man.appearance", "dark");
  const { api } = authority();
  api.migrateLegacy = vi.fn().mockRejectedValue(new Error("disk full"));
  api.setSelection = vi.fn().mockRejectedValue(new Error("disk full"));
  render(
    <AppearanceProvider api={api}>
      <AppearanceControl />
    </AppearanceProvider>,
  );
  await screen.findByRole("radio", { name: "Dark" });
  await userEvent.click(screen.getByRole("radio", { name: "Dark" }));
  expect(screen.getByRole("radio", { name: "Follow system" })).toBeChecked();
  expect(screen.getByRole("alert")).toHaveTextContent(
    "Could not save appearance",
  );
  expect(localStorage.getItem("skill-man.appearance")).toBe("dark");
});
test("successful migration clears legacy only after native confirmation", async () => {
  localStorage.setItem("skill-man.appearance", "dark");
  const { api } = authority();
  render(
    <AppearanceProvider api={api}>
      <AppearanceControl />
    </AppearanceProvider>,
  );
  await waitFor(() =>
    expect(localStorage.getItem("skill-man.appearance")).toBeNull(),
  );
  expect(api.migrateLegacy).toHaveBeenCalledWith("dark");
});
