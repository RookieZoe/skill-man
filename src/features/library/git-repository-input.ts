import type { GitRepositorySourceType } from "../../app/catalog-client";

/** Input feedback only. Core remains authoritative for validation and fetching. */
export function parseRepositoryInput(
  input: string,
): { sourceType: GitRepositorySourceType; sourceUrl: string } | null {
  const value = input.trim();
  const shorthand = /^(?!-)[A-Za-z0-9_.-]+\/(?!-)[A-Za-z0-9_.-]+$/;
  const candidate = shorthand.test(value)
    ? `https://github.com/${value}`
    : value;
  if (/\s|\\/.test(candidate) || !candidate.startsWith("https://")) return null;
  try {
    const url = new URL(candidate);
    if (url.username || url.password || url.port || url.search || url.hash)
      return null;
    const host = url.hostname.toLowerCase();
    const sourceType =
      host === "github.com" || host === "www.github.com"
        ? "github"
        : host === "gitlab.com" || host === "www.gitlab.com"
          ? "gitlab"
          : "git";
    const parts = url.pathname.replace(/\/$/, "").split("/").slice(1);
    if (
      !parts.length ||
      parts.some((part) => !part || part === "." || part === "..") ||
      (sourceType === "github" && parts.length !== 2) ||
      (sourceType === "gitlab" && (parts.length < 2 || parts.includes("-")))
    )
      return null;
    return { sourceType, sourceUrl: url.toString() };
  } catch {
    return null;
  }
}
