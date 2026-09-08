import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { CommunityUpdateControl } from "./CommunityUpdateControl";
test("checks only on request, reports failure and permits retry to a specific release", async () => {
  const check = vi
    .fn()
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValueOnce({
      version: "0.2.0",
      releaseUrl: "https://github.com/RookieZoe/skill-man/releases/tag/v0.2.0",
    });
  render(<CommunityUpdateControl check={check} />);
  expect(check).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Check now" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not check");
  await userEvent.click(screen.getByRole("button", { name: "Check now" }));
  expect(
    await screen.findByRole("link", { name: "View release" }),
  ).toHaveAttribute(
    "href",
    "https://github.com/RookieZoe/skill-man/releases/tag/v0.2.0",
  );
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});
test("reports no newer version after a successful check", async () => {
  render(<CommunityUpdateControl check={async () => null} />);
  await userEvent.click(screen.getByRole("button", { name: "Check now" }));
  expect(await screen.findByRole("status")).toHaveTextContent(
    "You’re up to date.",
  );
});
