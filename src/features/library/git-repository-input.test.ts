import { expect, test } from "vitest";
import { parseRepositoryInput } from "./git-repository-input";

test("expands GitHub owner/repo shorthand into the submitted URL", () => {
  expect(parseRepositoryInput(" tw93/kami ")).toEqual({
    sourceType: "github",
    sourceUrl: "https://github.com/tw93/kami",
  });
});

test.each([
  [" https://github.com/MengTo/Skills.git ", "github"],
  ["https://www.GITHUB.com/a/b", "github"],
  ["https://gitlab.com/group/subgroup/repo.git", "gitlab"],
  ["https://git.example.com/repo.git", "git"],
  ["owner/repo", "github"],
])("detects the provider for %s", (url, sourceType) => {
  expect(parseRepositoryInput(url)?.sourceType).toBe(sourceType);
});

test.each([
  "",
  "https://",
  "https://github.com",
  "https://github.com/owner",
  "https://github.com/a/b/blob/main/SKILL.md",
  "git@github.com:a/b.git",
  "file:///tmp/repo",
  "http://host/repo",
  "https://user:secret@host/repo",
  "https://host:8443/repo",
  "https://host/repo?token=secret",
  "https://host/repo#ref",
  "https://host/my repo",
])("rejects unsupported input %s", (url) => {
  expect(parseRepositoryInput(url)).toBeNull();
});
