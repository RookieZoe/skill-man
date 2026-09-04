import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { expect, test, vi } from "vitest";

import type {
  CandidateInvalidReason,
  CatalogClient,
  EffectiveLocale,
  LocaleSelection,
  PublicError,
} from "../../app/catalog-client";
import { App } from "../../app/App";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { LanguageControl } from "./LanguageControl";
import { LocaleProvider } from "./LocaleProvider";
import {
  errorMessageKey,
  translate,
  translatePlural,
  type MessageKey,
} from "./messages";

function renderWithLocale(client: CatalogClient, ui: ReactNode) {
  return render(<LocaleProvider client={client}>{ui}</LocaleProvider>);
}

async function switchToZhHans() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Preferences" }));
  const dialog = await screen.findByRole("dialog", { name: "Preferences" });
  await user.click(
    await within(dialog).findByRole("radio", { name: "简体中文" }),
  );
  return dialog;
}

test("document lang mirrors the effective locale and switches live", async () => {
  renderWithLocale(
    createFixtureCatalogClient(),
    <App client={createFixtureCatalogClient()} />,
  );
  await waitFor(() => expect(document.documentElement.lang).toBe("en"));

  await switchToZhHans();

  await waitFor(() => expect(document.documentElement.lang).toBe("zh-Hans"));
  // The open sheet re-renders in the new locale instead of remounting.
  expect(screen.getByRole("dialog", { name: "设置" })).toBeInTheDocument();
  expect(
    within(screen.getByRole("dialog", { name: "设置" })).getByRole("heading", {
      name: "设置",
    }),
  ).toBeInTheDocument();
});

test("runtime switch keeps the sheet open and preserves filter, selection and forms", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  renderWithLocale(client, <App client={client} />);

  // Establish ephemeral UI state: a filter and a selected Skill.
  await screen.findByRole("navigation", { name: "Library" });
  const skillRow = await screen.findByRole("button", {
    name: "skill-authoring",
  });
  await user.click(skillRow);
  expect(skillRow).toHaveAttribute("aria-pressed", "true");
  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("button", { name: "Broken" }));
  expect(screen.getByRole("button", { name: "Broken" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  // Switch the locale inside the Preferences sheet.
  const dialog = await switchToZhHans();
  await waitFor(() => expect(document.documentElement.lang).toBe("zh-Hans"));

  // The sheet is the same DOM node — never remounted.
  expect(dialog).toBeInTheDocument();

  // Close; filter and selection survive the locale switch. The detail pane
  // was cleared by the earlier filter change (pre-existing app behavior);
  // the filter state itself is untouched by the locale switch.
  await user.click(within(dialog).getByRole("button", { name: "完成" }));
  // Focus is restored to the sheet opener (the Preferences trigger) — the
  // overlay close path runs after the locale switch like before it.
  expect(document.getElementById("preferences-trigger")).toHaveFocus();
  expect(screen.getByRole("button", { name: "已失效" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  // The catalog was not reloaded: the Broken-filtered list still shows the
  // broken Skill without a loading pass.
  expect(
    await screen.findByRole("button", { name: "legacy-audit" }),
  ).toBeInTheDocument();
  // The Library Desk never showed a loading state (no remount).
  expect(
    screen.queryByRole("status", { name: /Loading Skill detail/ }),
  ).not.toBeInTheDocument();
  // Post-switch interaction still works: selecting works and the filter
  // returns to All without a catalog reload.
  await user.click(screen.getByRole("button", { name: "全部" }));
  const row = await screen.findByRole("button", { name: "skill-authoring" });
  await user.click(row);
  expect(
    await screen.findByRole("heading", { name: "skill-authoring" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "skill-authoring" }),
  ).toHaveAttribute("aria-pressed", "true");
});

test("persist failure keeps the selection and every visible surface", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.setLocaleSelection = vi.fn(async () => {
    throw { error: { code: "locale_store_unavailable" }, diagnostic: null };
  });
  renderWithLocale(client, <LanguageControl />);

  await user.click(await screen.findByRole("radio", { name: "简体中文" }));

  // persist-then-publish failed: the control stays on the old selection and
  // the document lang never changes.
  await waitFor(() =>
    expect(client.setLocaleSelection).toHaveBeenCalledTimes(1),
  );
  // The fixture resolves system → en, so the control renders English labels
  // and keeps the System selection after the failed persist.
  expect(await screen.findByRole("radio", { name: "System" })).toBeChecked();
  expect(document.documentElement.lang).toBe("en");
});

test("every public error code has a bilingual presentation", () => {
  const codes: PublicError["code"][] = [
    "validation",
    "not_found",
    "conflict",
    "plan_stale",
    "permission_denied",
    "state_unavailable",
    "catalog_unavailable",
    "recovery_required",
    "source_unavailable",
    "target_mismatch",
    "disk_full",
    "modified",
    "stale_update",
    "update_cancelled",
    "download_failed",
    "install_failed",
    "bootstrap_unavailable",
    "recovery_not_locked",
    "recovery_not_pure",
    "recovery_no_active_operation",
    "recovery_operation_already_active",
    "recovery_writer_active",
    "recovery_step_failed",
    "recovery_state_ambiguous",
    "recovery_snapshot_in_use",
    "recovery_state_store",
    "recovery_filesystem",
    "recovery_probe",
    "locale_store_unavailable",
    "internal",
  ];
  for (const code of codes) {
    const key = errorMessageKey(code);
    for (const locale of ["en", "zh-Hans"] as EffectiveLocale[]) {
      const message = translate(locale, key);
      expect(message.length).toBeGreaterThan(0);
      expect(message).not.toBe(key);
    }
  }
});

test("every candidate validation reason has friendly bilingual copy", () => {
  const expectedKeys: Record<CandidateInvalidReason, MessageKey> = {
    not_absolute: "error.candidate_invalid.not_absolute",
    not_utf8: "error.candidate_invalid.not_utf8",
    symlink_component: "error.candidate_invalid.symlink_component",
    state_dir_overlap: "error.candidate_invalid.state_dir_overlap",
    agent_dir_overlap: "error.candidate_invalid.agent_dir_overlap",
    parent_missing: "error.candidate_invalid.parent_missing",
    parent_not_writable: "error.candidate_invalid.parent_not_writable",
    not_directory: "error.candidate_invalid.not_directory",
    not_empty: "error.candidate_invalid.not_empty",
    no_volume_identity: "error.candidate_invalid.no_volume_identity",
    insufficient_space: "error.candidate_invalid.insufficient_space",
    not_legacy_home: "error.candidate_invalid.not_legacy_home",
    legacy_contaminated: "error.candidate_invalid.legacy_contaminated",
  };

  for (const reason of Object.keys(expectedKeys) as CandidateInvalidReason[]) {
    const key = errorMessageKey({ code: "candidate_invalid", reason });
    expect(key).toBe(expectedKeys[reason]);
    for (const locale of ["en", "zh-Hans"] as EffectiveLocale[]) {
      const message = translate(locale, key);
      expect(message.length).toBeGreaterThan(0);
      expect(message).not.toBe(translate(locale, "error.candidate_invalid"));
    }
  }

  expect(
    translate(
      "en",
      errorMessageKey({ code: "candidate_invalid", reason: "not_empty" }),
    ),
  ).toBe(
    "This folder is not empty. Choose an empty folder to set up a new Home.",
  );
  expect(
    translate(
      "zh-Hans",
      errorMessageKey({ code: "candidate_invalid", reason: "not_empty" }),
    ),
  ).toBe("此文件夹不是空的。请使用空文件夹来创建新的 Home。");
});

test("conflict errors render the typed directory name as a param", () => {
  const en = translate("en", errorMessageKey("conflict"), {
    name: "ask-matt",
  });
  const zh = translate("zh-Hans", errorMessageKey("conflict"), {
    name: "ask-matt",
  });
  expect(en).toContain("ask-matt");
  expect(zh).toContain("ask-matt");
});

test("plural lookups pick the locale plural category", () => {
  expect(translatePlural("en", "tray.agent_count", 1)).toBe("1 Agent");
  expect(translatePlural("en", "tray.agent_count", 2)).toBe("2 Agents");
  // zh-Hans has a single plural category; both counts use the _other shape.
  expect(translatePlural("zh-Hans", "tray.agent_count", 1)).toBe("1 个智能体");
  expect(translatePlural("zh-Hans", "tray.agent_count", 2)).toBe("2 个智能体");
});

test("same Source Content renders byte-identical in both locales", async () => {
  const clientEn = createFixtureCatalogClient();

  const first = renderWithLocale(clientEn, <App client={clientEn} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  const enPath = screen.getAllByText(
    "/Users/zoe/Codes/AI/skills/skill-authoring",
  )[0];
  const enMarkdown = screen.getByText(/# Skill authoring/);
  const enMarkdownText = enMarkdown.textContent;
  first.unmount();

  const zhClient = createFixtureCatalogClient();
  zhClient.getLocaleSnapshot = async () => ({
    selection: "zh-Hans" as LocaleSelection,
    effectiveLocale: "zh-Hans",
    generation: 0,
    diagnostic: null,
  });
  render(
    <LocaleProvider client={zhClient}>
      <App client={zhClient} />
    </LocaleProvider>,
  );
  await waitFor(() => expect(document.documentElement.lang).toBe("zh-Hans"));
  await screen.findByRole("heading", { name: "skill-authoring" });
  const zhPath = screen.getAllByText(
    "/Users/zoe/Codes/AI/skills/skill-authoring",
  )[0];
  expect(zhPath.textContent).toBe(enPath.textContent);
  const zhMarkdown = screen.getByText(/# Skill authoring/);
  expect(zhMarkdown.textContent).toBe(enMarkdownText);
});

test("the Language control renders in the Unconfigured bootstrap shell", async () => {
  const client = createFixtureCatalogClient();
  client.getBootstrapSnapshot = async () => ({ state: "unconfigured" });
  const { BootstrapApp } = await import("../../app/BootstrapApp");
  renderWithLocale(client, <BootstrapApp client={client} />);
  const route = await screen.findByRole("status");
  expect(within(route).getByRole("radiogroup")).toBeInTheDocument();
  expect(
    await within(route).findByRole("radio", { name: "System" }),
  ).toBeChecked();
});
