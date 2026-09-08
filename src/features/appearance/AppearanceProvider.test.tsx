import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import { AppearanceProvider, AppearanceControl } from "./AppearanceProvider";
afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
  delete document.documentElement.dataset.theme;
});
test("explicit appearance persists across remount and system mode follows changes", async () => {
  let change = () => {};
  const media = {
    matches: true,
    addEventListener: (_: string, fn: () => void) => {
      change = fn;
    },
    removeEventListener: vi.fn(),
  };
  vi.spyOn(window, "matchMedia").mockReturnValue(
    media as unknown as MediaQueryList,
  );
  const view = render(
    <AppearanceProvider>
      <AppearanceControl />
    </AppearanceProvider>,
  );
  expect(document.documentElement.dataset.theme).toBe("dark");
  await userEvent.click(screen.getByRole("radio", { name: "Light" }));
  expect(document.documentElement.dataset.theme).toBe("light");
  view.unmount();
  render(
    <AppearanceProvider>
      <AppearanceControl />
    </AppearanceProvider>,
  );
  expect(screen.getByRole("radio", { name: "Light" })).toBeChecked();
  await userEvent.click(screen.getByRole("radio", { name: "Follow system" }));
  expect(document.documentElement.dataset.theme).toBe("dark");
  act(() => {
    media.matches = false;
    change();
  });
  expect(document.documentElement.dataset.theme).toBe("light");
});
test("failed persistence keeps the active selection", async () => {
  render(
    <AppearanceProvider>
      <AppearanceControl />
    </AppearanceProvider>,
  );
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("unavailable");
  });
  await userEvent.click(screen.getByRole("radio", { name: "Dark" }));
  expect(screen.getByRole("radio", { name: "Follow system" })).toBeChecked();
  expect(screen.getByRole("alert")).toHaveTextContent(
    "Could not save appearance",
  );
});
