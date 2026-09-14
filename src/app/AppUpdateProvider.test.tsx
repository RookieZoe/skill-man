import { render, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { AppUpdateProvider, useAppUpdate } from "./AppUpdateProvider";
import { createCatalogClient } from "./catalog-client";
import { LocaleProvider } from "../features/locale/LocaleProvider";
import { createFixtureCatalogClient } from "../test-fixtures/catalog";

function Actions() {
  const { checkAppUpdate, checkAutomatically } = useAppUpdate();
  return (
    <>
      <button onClick={checkAppUpdate}>Manual check</button>
      <button onClick={checkAutomatically}>Automatic check</button>
    </>
  );
}
test("native checks use the independent native flow without mounting a web update sheet", async () => {
  const client = createCatalogClient();
  client.checkAppUpdate = vi.fn();
  const nativeUpdate = vi.fn(async () => {});
  render(
    <AppUpdateProvider client={client} nativeUpdate={nativeUpdate}>
      <span>Home unavailable</span>
      <Actions />
    </AppUpdateProvider>,
  );
  await userEvent.click(
    await screen.findByRole("button", { name: "Manual check" }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Automatic check" }),
  );
  expect(nativeUpdate.mock.calls).toEqual([[true], [false]]);
  expect(client.checkAppUpdate).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(screen.getByText("Home unavailable")).toBeInTheDocument();
});
test("a browser preview update error follows a native locale switch", async () => {
  const client = createFixtureCatalogClient();
  client.checkAppUpdate = vi
    .fn()
    .mockRejectedValue({ code: "source_unavailable" });
  let publish!: (snapshot: import("./catalog-client").LocaleSnapshot) => void;
  client.listenLocaleChanged = async (fn) => {
    publish = fn;
    return () => {};
  };
  render(
    <LocaleProvider client={client}>
      <AppUpdateProvider client={client}>
        <Actions />
      </AppUpdateProvider>
    </LocaleProvider>,
  );
  await userEvent.click(
    await screen.findByRole("button", { name: "Manual check" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Unable to reach the update service",
  );
  act(() =>
    publish({
      selection: "zh-Hans",
      effectiveLocale: "zh-Hans",
      generation: 3,
      diagnostic: null,
    }),
  );
  expect(screen.getByRole("alert")).not.toHaveTextContent(
    "Unable to reach the update service",
  );
});
