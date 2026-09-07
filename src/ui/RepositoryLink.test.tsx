import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";
import { RepositoryLink } from "./RepositoryLink";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

test("opens the repository with the system browser without toggling its parent", async () => {
  const parent = vi.fn();
  render(
    <div onClick={parent}>
      <RepositoryLink url="https://github.com/tw93/kami" />
    </div>,
  );
  await userEvent.click(screen.getByRole("link"));
  expect(openUrl).toHaveBeenCalledWith("https://github.com/tw93/kami");
  expect(parent).not.toHaveBeenCalled();
});

test.each([
  "file:///etc/passwd",
  "javascript:alert(1)",
  "https://user:password@example.com",
  "not a url",
])("keeps unsafe repository content inert: %s", (url) => {
  render(<RepositoryLink url={url} />);
  expect(screen.queryByRole("link")).not.toBeInTheDocument();
});

test("reports browser launch failure", async () => {
  vi.mocked(openUrl).mockRejectedValueOnce(new Error("unavailable"));
  render(<RepositoryLink url="https://github.com/tw93/kami" />);
  await userEvent.click(screen.getByRole("link"));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not open the browser",
  );
});
