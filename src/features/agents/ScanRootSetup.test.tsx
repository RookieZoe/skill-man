import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { ScanRootSetup } from "./ScanRootSetup";

test("detection is zero-write, every preset is selectable and an empty Home is not scan-ready", async () => {
  const client = createFixtureCatalogClient({ emptyAgentConfigurations: true });
  const apply = vi.spyOn(client, "applyAgentConfigurationPlan");
  const ready = vi.fn();
  render(
    <ScanRootSetup client={client} onReady={ready} onBusyChange={() => {}} />,
  );
  expect(await screen.findAllByRole("checkbox")).toHaveLength(9);
  expect(
    screen
      .getAllByRole("checkbox")
      .every((box) => !(box as HTMLInputElement).checked),
  ).toBe(true);
  expect(ready).toHaveBeenLastCalledWith(false);
  expect(apply).not.toHaveBeenCalled();
});

test("batch confirmation replans against each new generation and retains existing configurations", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const original = (await client.getAgentManagementSnapshot()).configurations;
  const generations = new Map<string, number>();
  const plan = client.planCreateAgentConfiguration;
  client.planCreateAgentConfiguration = async (draft) => {
    const result = await plan(draft);
    generations.set(
      result.planToken,
      (await client.getAgentManagementSnapshot()).generation,
    );
    return result;
  };
  const apply = client.applyAgentConfigurationPlan;
  const writes = vi.fn(async (token: string) => {
    expect(generations.get(token)).toBe(
      (await client.getAgentManagementSnapshot()).generation,
    );
    return apply(token);
  });
  client.applyAgentConfigurationPlan = writes;
  const ready = vi.fn();
  render(
    <ScanRootSetup client={client} onReady={ready} onBusyChange={() => {}} />,
  );
  await user.click(await screen.findByRole("checkbox", { name: /Gemini CLI/ }));
  await user.click(screen.getByRole("checkbox", { name: /Cursor/ }));
  await user.click(
    screen.getByRole("button", { name: "Review selected configurations" }),
  );
  expect(writes).not.toHaveBeenCalled();
  await user.click(
    await screen.findByRole("button", { name: "Confirm and save" }),
  );
  await waitFor(() => expect(ready).toHaveBeenLastCalledWith(true));
  expect(writes).toHaveBeenCalledTimes(2);
  expect(
    (await client.getAgentManagementSnapshot()).configurations.slice(
      0,
      original.length,
    ),
  ).toEqual(original);
});

test("partial failure preserves saved configurations and retry does not duplicate them", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient({ emptyAgentConfigurations: true });
  const apply = client.applyAgentConfigurationPlan;
  let calls = 0;
  client.applyAgentConfigurationPlan = async (token) => {
    if (++calls === 2) throw { error: { code: "recovery_required" } };
    return apply(token);
  };
  render(
    <ScanRootSetup
      client={client}
      onReady={() => {}}
      onBusyChange={() => {}}
    />,
  );
  await user.click(
    await screen.findByRole("checkbox", { name: /Claude Code/ }),
  );
  await user.click(screen.getByRole("checkbox", { name: /Codex/ }));
  await user.click(
    screen.getByRole("button", { name: "Review selected configurations" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "Confirm and save" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Library changes are disabled",
  );
  expect(
    (await client.getAgentManagementSnapshot()).configurations,
  ).toHaveLength(1);
  await user.click(
    screen.getByRole("button", { name: "Review selected configurations" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "Confirm and save" }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Confirm and save" }),
    ).not.toBeInTheDocument(),
  );
  expect(
    (await client.getAgentManagementSnapshot()).configurations,
  ).toHaveLength(2);
});

test("changed filesystem effects require another review and never apply silently", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient({ emptyAgentConfigurations: true });
  const plan = client.planCreateAgentConfiguration;
  let calls = 0;
  client.planCreateAgentConfiguration = async (draft) => ({
    ...(await plan(draft)),
    targetWillBeCreated: ++calls > 1,
  });
  const apply = vi.spyOn(client, "applyAgentConfigurationPlan");
  render(
    <ScanRootSetup
      client={client}
      onReady={() => {}}
      onBusyChange={() => {}}
    />,
  );
  await user.click(
    await screen.findByRole("checkbox", { name: /Claude Code/ }),
  );
  await user.click(
    screen.getByRole("button", { name: "Review selected configurations" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "Confirm and save" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(/Review it again/);
  expect(apply).not.toHaveBeenCalled();
});

test("reload reconciles selection when the read after a successful save fails", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient({ emptyAgentConfigurations: true });
  const snapshot = client.getAgentManagementSnapshot;
  let reads = 0;
  client.getAgentManagementSnapshot = async () => {
    if (++reads === 3) throw { error: { code: "catalog_unavailable" } };
    return snapshot();
  };
  const apply = vi.spyOn(client, "applyAgentConfigurationPlan");
  const ready = vi.fn();
  render(
    <ScanRootSetup client={client} onReady={ready} onBusyChange={() => {}} />,
  );
  await user.click(
    await screen.findByRole("checkbox", { name: /Claude Code/ }),
  );
  await user.click(
    screen.getByRole("button", { name: "Review selected configurations" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "Confirm and save" }),
  );
  await screen.findByRole("alert");
  await user.click(
    screen.getByRole("button", { name: "Reload configuration" }),
  );
  await waitFor(() => expect(ready).toHaveBeenLastCalledWith(true));
  expect(apply).toHaveBeenCalledTimes(1);
  expect((await snapshot()).configurations).toHaveLength(1);
});
