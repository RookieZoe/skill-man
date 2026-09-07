import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { SelectionControls } from "./SelectionControls";

test("selection actions affect only the displayed items", async () => {
  const change = vi.fn();
  render(
    <SelectionControls
      ids={["alpha", "beta"]}
      selected={["alpha", "hidden"]}
      onChange={change}
    />,
  );
  await userEvent.click(screen.getByRole("button", { name: "Select all" }));
  expect(change).toHaveBeenLastCalledWith(["hidden", "alpha", "beta"]);
  await userEvent.click(
    screen.getByRole("button", { name: "Invert selection" }),
  );
  expect(change).toHaveBeenLastCalledWith(["hidden", "beta"]);
  await userEvent.click(
    screen.getByRole("button", { name: "Clear selection" }),
  );
  expect(change).toHaveBeenLastCalledWith(["hidden"]);
});
