import { act, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";

import { OperationStatusWindow } from "./OperationStatusWindow";

test("lists every active operation", async () => {
  vi.useFakeTimers();
  try {
    render(
      <OperationStatusWindow
        ariaLabel="Current activity"
        heading="2 operations in progress"
        operations={[
          {
            id: "adopt-scan",
            title: "Scanning",
            detail: "Reading configured Agent directories.",
          },
          {
            id: "git-import",
            title: "Fetching repository",
            detail: "Discovering importable Skills.",
          },
        ]}
      />,
    );

    expect(
      screen.queryByRole("region", { name: "Current activity" }),
    ).not.toBeInTheDocument();
    await act(async () => {
      vi.advanceTimersByTime(250);
    });

    const panel = screen.getByRole("region", { name: "Current activity" });
    expect(panel).toHaveTextContent("2 operations in progress");
    expect(
      within(panel).getByRole("heading", { name: "Scanning" }),
    ).toBeInTheDocument();
    expect(
      within(panel).getByRole("heading", { name: "Fetching repository" }),
    ).toBeInTheDocument();
    expect(within(panel).getAllByRole("progressbar")).toHaveLength(2);
  } finally {
    vi.useRealTimers();
  }
});

test("does not leave an empty activity landmark behind", () => {
  render(
    <OperationStatusWindow
      ariaLabel="Current activity"
      heading="0 operations in progress"
      operations={[]}
    />,
  );

  expect(
    screen.queryByRole("region", { name: "Current activity" }),
  ).not.toBeInTheDocument();
});
