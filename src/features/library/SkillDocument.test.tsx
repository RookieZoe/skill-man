import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { SkillDocument } from "./SkillDocument";

test("renders Markdown and GFM by default, preserving exact frontmatter and content in RAW", async () => {
  const markdown =
    "---\nname: example\n---\n# Instructions\n\n**Strong**\n\n| A | B |\n| - | - |\n| 1 | 2 |";
  const { container } = render(<SkillDocument markdown={markdown} />);
  expect(screen.getByRole("heading", { name: "Instructions" })).toBeVisible();
  expect(screen.getByRole("table")).toBeVisible();
  expect(container.querySelector("strong")).toHaveTextContent("Strong");
  expect(screen.queryByText(/name: example/)).not.toBeInTheDocument();
  const toggle = screen.getByRole("switch", { name: "View raw source" });
  expect(toggle).toHaveAttribute("aria-checked", "false");
  await userEvent.click(toggle);
  expect(container.querySelector("pre")?.textContent).toBe(markdown);
  await userEvent.click(toggle);
  expect(screen.getByRole("table")).toBeVisible();
});

test("source content cannot inject HTML, load images, or link to local files", () => {
  const { container } = render(
    <SkillDocument
      markdown={
        "<script>alert(1)</script>\n\n![tracking](https://example.com/pixel)\n\n[bad](javascript:alert) [local](file:///etc/passwd) [safe](https://example.com)"
      }
    />,
  );
  expect(container.querySelector("script, img, iframe")).toBeNull();
  expect(screen.getAllByRole("link")).toHaveLength(1);
  expect(screen.getByRole("link")).toHaveAttribute(
    "rel",
    "noopener noreferrer",
  );
});
