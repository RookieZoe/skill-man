export function containsDirectFileSystemAccess(source) {
  if (
    /\bfs\s*::/.test(source) ||
    /\bstd\s*::\s*(?:fs\b|os\s*::\s*unix\s*::\s*fs\b)/.test(source)
  ) {
    return true;
  }

  if (
    /\b(?:use\s+(?:::)?\s*std|extern\s+crate\s+std)\s+as\s+[A-Za-z_]\w*\s*;/.test(
      source,
    )
  ) {
    return true;
  }

  return Array.from(
    source.matchAll(/\buse\s+(?:::)?\s*std\s*::([\s\S]*?);/g),
    ([, imported]) => imported,
  ).some(
    (imported) =>
      /\bas\s+[A-Za-z_]\w*/.test(imported) ||
      (imported.includes("{") && /\bfs\b/.test(imported)),
  );
}
