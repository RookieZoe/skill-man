import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import type {
  GitSourceCapabilityReport,
  SkillSummary,
} from "../../app/catalog-client";
import { LibrarySidebar } from "./LibraryDesk";

const skills: SkillSummary[] = ["one", "two", "local"].map((id) => ({
  id,
  directoryName: id,
  displayName: id,
  description: "Example skill",
  sourceKind: id === "local" ? "link" : "remote_install",
  health: "healthy",
  enabledAgentCount: 0,
}));
const report: GitSourceCapabilityReport = {
  sources: [
    {
      remoteId: "repo-1",
      canonicalUrl: "https://github.com/acme/skills.git",
      kind: "git_repository_source",
      members: skills.slice(0, 2).map((skill) => ({
        skillId: skill.id,
        skillPath: skill.id,
        presence: true,
      })),
    },
  ],
};
const defaults = {
  filter: "all" as const,
  skills,
  selectedId: "one",
  isSelectMode: false,
  selectedSkillIds: [],
  onToggleSkillSelection: vi.fn(),
  onFilter: vi.fn(),
  onSelect: vi.fn(),
  gitSourceCapability: report,
};

test("groups by repository identity and expands without selecting a Skill", async () => {
  const user = userEvent.setup();
  const onSelect = vi.fn();
  render(<LibrarySidebar {...defaults} onSelect={onSelect} />);
  const group = screen.getByRole("button", { name: "acme/skills 2" });
  expect(group).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("button", { name: "one" })).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "local" }),
  ).not.toBeInTheDocument();
  const localGroup = screen.getByRole("button", { name: "Local sources 1" });
  expect(localGroup).toHaveAttribute("aria-expanded", "false");
  await user.click(localGroup);
  expect(screen.getByRole("button", { name: "local" })).toBeInTheDocument();
  await user.click(group);
  expect(onSelect).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "two" }));
  expect(onSelect).toHaveBeenCalledWith("two");
  await user.click(group);
  expect(screen.queryByRole("button", { name: "two" })).not.toBeInTheDocument();
});

test("keeps collapse state across filtering and retains selection behavior", async () => {
  const user = userEvent.setup();
  const onToggleSkillSelection = vi.fn();
  const props = { ...defaults, isSelectMode: true, onToggleSkillSelection };
  const view = render(<LibrarySidebar {...props} />);
  await user.click(screen.getByRole("button", { name: "acme/skills 2" }));
  view.rerender(
    <LibrarySidebar {...props} skills={[skills[0]]} filter="install" />,
  );
  expect(screen.getByRole("button", { name: "acme/skills 1" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  await user.click(screen.getByRole("button", { name: "one" }));
  expect(onToggleSkillSelection).toHaveBeenCalledWith("one");
});

test("never drops Skills while repository facts are unavailable", async () => {
  render(<LibrarySidebar {...defaults} gitSourceCapability={null} />);
  await userEvent.click(screen.getByRole("button", { name: "Git 2" }));
  await userEvent.click(
    screen.getByRole("button", { name: "Local sources 1" }),
  );
  for (const skill of skills)
    expect(screen.getByRole("button", { name: skill.id })).toBeInTheDocument();
});

test("identical repository names on different hosts remain separate", () => {
  render(
    <LibrarySidebar
      {...defaults}
      gitSourceCapability={{
        sources: [
          { ...report.sources[0], members: [report.sources[0].members[0]] },
          {
            ...report.sources[0],
            remoteId: "repo-2",
            canonicalUrl: "https://gitlab.com/acme/skills",
            members: [report.sources[0].members[1]],
          },
        ],
      }}
    />,
  );
  expect(screen.getAllByRole("button", { name: "acme/skills 1" })).toHaveLength(
    2,
  );
});
