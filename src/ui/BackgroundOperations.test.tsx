import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import {
  BackgroundOperations,
  useBackgroundOperations,
} from "./BackgroundOperations";
import { OperationNotice } from "./OperationNotice";

test("background results survive the originating view and require explicit completion", async () => {
  let operations!: ReturnType<typeof useBackgroundOperations>;
  function Origin() {
    operations = useBackgroundOperations();
    return null;
  }
  const { rerender } = render(
    <BackgroundOperations>
      <Origin />
    </BackgroundOperations>,
  );
  let first = "",
    second = "";
  act(() => {
    first = operations.begin({
      title: "Copying alpha",
      detail: "Continue working",
    });
    second = operations.begin({
      title: "Copying beta",
      detail: "Continue working",
    });
  });
  expect(
    await screen.findByRole("region", { name: "Current activity" }),
  ).toBeInTheDocument();
  rerender(<BackgroundOperations>{null}</BackgroundOperations>);
  expect(screen.getAllByRole("progressbar")).toHaveLength(2);
  act(() =>
    operations.finish(first, {
      title: "Completed alpha",
      detail: "Open the library",
    }),
  );
  expect(screen.getAllByRole("progressbar")).toHaveLength(1);
  expect(screen.getByText("Open the library")).toBeInTheDocument();
  act(() =>
    operations.finish(second, {
      title: "Beta failed",
      detail: "Retry beta",
      state: "failed",
    }),
  );
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  const firstItem = screen.getByText("Completed alpha").closest("li")!;
  await userEvent.click(
    within(firstItem).getByRole("button", { name: "Dismiss" }),
  );
  expect(screen.queryByText("Completed alpha")).not.toBeInTheDocument();
  expect(screen.getByText("Beta failed")).toBeInTheDocument();
});

test("foreground work has one local progress surface and honest cancellation copy", () => {
  const { rerender } = render(
    <BackgroundOperations>
      <OperationNotice busy />
    </BackgroundOperations>,
  );
  expect(screen.getAllByRole("progressbar")).toHaveLength(1);
  expect(
    screen.queryByRole("region", { name: "Current activity" }),
  ).not.toBeInTheDocument();
  expect(screen.getByText(/cannot close during/)).toBeInTheDocument();
  rerender(
    <BackgroundOperations>
      <OperationNotice busy cancellable />
    </BackgroundOperations>,
  );
  expect(
    screen.getByText(/Closing this window cancels the download/),
  ).toBeInTheDocument();
  expect(screen.queryByText(/cannot close during/)).not.toBeInTheDocument();
  rerender(
    <BackgroundOperations>
      <OperationNotice busy={false} />
    </BackgroundOperations>,
  );
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
});
