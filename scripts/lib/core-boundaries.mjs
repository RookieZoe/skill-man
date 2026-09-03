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
  ).some((imported) => {
    // Strip a trailing `as` alias; only the selected module decides whether
    // the import is filesystem access. Non-fs aliases (e.g.
    // `Ordering as AtomicOrdering` for std::sync::atomic) are not.
    const modulePart = imported.replace(/\s+as\s+[A-Za-z_]\w*\s*$/, "");
    const braceMatch = modulePart.match(/^([\s\S]*?)\{([\s\S]*)\}$/);
    const prefix = braceMatch ? braceMatch[1] : modulePart;
    const list = braceMatch ? braceMatch[2] : "";
    const filesystemModule = /(?:^|::)\s*fs\b/;
    const unixFilesystemModule = /(?:^|::)\s*os\s*::\s*unix\s*::\s*fs\b/;
    if (filesystemModule.test(prefix) || unixFilesystemModule.test(prefix)) {
      return true;
    }
    if (list) {
      return (
        /(?:^|[,\s])\s*fs\b/.test(list) ||
        /(?:^|[,\s])\s*os\s*::\s*unix\s*::\s*fs\b/.test(list)
      );
    }
    return false;
  });
}
