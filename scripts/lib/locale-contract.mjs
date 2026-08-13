// Shared locale-contract facts for `check-locales.mjs` and its node tests:
// the two catalog files and the placeholder extractor.

import { readFile } from "node:fs/promises";
import { URL } from "node:url";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = fileURLToPath(new URL("../..", import.meta.url));
const localesDir = path.join(root, "resources", "locales");

export const en = JSON.parse(
  await readFile(path.join(localesDir, "en.json"), "utf8"),
);
export const zhHans = JSON.parse(
  await readFile(path.join(localesDir, "zh-Hans.json"), "utf8"),
);

const placeholderPattern = /\{([a-zA-Z][a-zA-Z0-9]*)\}/g;

export function placeholders(value) {
  return [...value.matchAll(placeholderPattern)]
    .map((match) => match[1])
    .sort();
}
