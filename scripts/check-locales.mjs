// Locale contract gate (spec §6.2, §6.3; ADR-0011): en.json is the complete
// baseline and zh-Hans.json must carry the identical key, placeholder and
// plural-parameter set. The hardcoded-copy scan blocks App Copy outside the
// message catalog, allowing only brand, technical tokens and the maintained
// Source Content allowlist. The key-usage scan blocks referencing catalog
// keys that do not exist in en.json.

import { readFile, readdir } from "node:fs/promises";
import process from "node:process";
import path from "node:path";
import { parse } from "@babel/parser";
import { en, placeholders, zhHans } from "./lib/locale-contract.mjs";

const srcDir = path.join(import.meta.dirname, "..", "src");

const failures = [];
function fail(message) {
  failures.push(message);
}

const zh = zhHans;

// -- 1. key-set and placeholder parity -------------------------------------

const enKeys = Object.keys(en);
const zhKeys = Object.keys(zh);
const enSet = new Set(enKeys);
const zhSet = new Set(zhKeys);

for (const key of enKeys) {
  if (!zhSet.has(key)) fail(`zh-Hans.json is missing key "${key}"`);
}
for (const key of zhKeys) {
  if (!enSet.has(key)) fail(`en.json is missing key "${key}"`);
}

for (const key of enKeys) {
  const enParams = placeholders(en[key]);
  const zhParams = placeholders(zh[key]);
  if (JSON.stringify(enParams) !== JSON.stringify(zhParams)) {
    fail(
      `key "${key}" placeholder mismatch: en [${enParams}] vs zh-Hans [${zhParams}]`,
    );
  }
}

// -- 2. plural pairs --------------------------------------------------------

for (const base of enKeys
  .filter((key) => key.endsWith("_one"))
  .map((key) => key.slice(0, -"_one".length))) {
  if (!enSet.has(`${base}_one`) || !enSet.has(`${base}_other`)) {
    fail(
      `plural base "${base}" must have both "${base}_one" and "${base}_other"`,
    );
  }
  if (!zhSet.has(`${base}_one`) || !zhSet.has(`${base}_other`)) {
    fail(`plural base "${base}" is missing its pair in zh-Hans.json`);
  }
}
for (const key of enKeys) {
  if (
    key.endsWith("_other") &&
    !enSet.has(`${key.slice(0, -"_other".length)}_one`)
  ) {
    fail(`plural base "${key.slice(0, -"_other".length)}" is missing "_one"`);
  }
}

// -- 3. catalog key usage ---------------------------------------------------

async function sourceFiles(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (
        entry.name === "test-fixtures" ||
        entry.name === "test" ||
        entry.name === "dev"
      ) {
        continue;
      }
      files.push(...(await sourceFiles(full)));
    } else if (
      (entry.name.endsWith(".ts") || entry.name.endsWith(".tsx")) &&
      !entry.name.includes(".test.")
    ) {
      files.push(full);
    }
  }
  return files;
}

const files = await sourceFiles(srcDir);

const usedKeys = new Set();
for (const file of files) {
  const code = await readFile(file, "utf8");
  // `t("key", ...)`, `tPlural("base", ...)`, `translatePlural("base", ...)`;
  // word boundary avoids matching `params.get("matrix")`. The catalog
  // implementation itself (`src/features/locale/`) is skipped: its first
  // argument is the locale, not a key.
  if (file.startsWith(path.join(srcDir, "features", "locale"))) continue;
  for (const match of code.matchAll(
    /(?<![A-Za-z])(?:t|translatePlural|tPlural)\(\s*["'`]([a-zA-Z][a-zA-Z0-9_.-]*)["'`]/g,
  )) {
    usedKeys.add(match[1]);
  }
  // Dynamic keys (`t(\`library.health.badge.${health}\`)`) bypass the static
  // scan; verify the static prefix matches at least one real key so a typo
  // cannot silently render a raw key.
  for (const match of code.matchAll(
    /(?<![A-Za-z])(?:t|translatePlural|tPlural)\(\s*`([a-zA-Z][a-zA-Z0-9_.-]*)\$\{/g,
  )) {
    const prefix = match[1];
    if (!enKeys.some((key) => key.startsWith(prefix))) {
      fail(
        `${file}: dynamic catalog key prefix "${prefix}" matches no en.json key`,
      );
    }
  }
}
for (const key of usedKeys) {
  // Plural helpers reference the base key (`base_one`/`base_other` pairs).
  if (!enSet.has(key) && !enSet.has(`${key}_one`)) {
    fail(`catalog key "${key}" referenced in src but missing from en.json`);
  }
}

// -- 4. hardcoded App Copy gate ----------------------------------------------

// Brand and technical tokens that legitimately stay outside the catalog.
// UI words are NOT allowed here: any remaining visible copy is a violation.
const allowlist = new Set([
  "Skill Man",
  "Claude Code",
  "Codex",
  "SKILL.md",
  "SQLite",
  "Git",
  "KB",
  "MB",
  "GB",
  "Escape",
  "Tab",
  "Enter",
  "bootstrap://changed",
  "locale://changed",
  "catalog-changed",
  "tray-open-skill",
  "skill-man-tray",
  "recovery_required",
  "bootstrap_command_failed",
  "recovery_step_failed",
  "rolled_back",
  "awaiting_commit",
  "bound_restore",
  "up_to_date",
  "source_unavailable",
  "state_unavailable",
  "stale_update",
  "download_failed",
  "install_failed",
  "update_cancelled",
  "remote_install",
  "file_install",
  "real_directory",
  "symlink",
  "one",
  "other",
  "repo root",
  "unknown",
  "none",
  "packages/skills/new-name",
  "vercel-labs/skills",
  "~/Projects/my-skill",
  "~/Library/Application Support/skill-man",
  "Library/Application Support/skill-man-state",
  "2-digit",
  "input, button, select, textarea, [tabindex]",
  "#skill-detail",
  // Maintained raw diagnostic for the non-Tauri production runtime; it is
  // only ever rendered inside the explicitly labeled diagnostic block.
  "Skill Man is not running in the Tauri runtime.",
  "agent-inspector-dialog",
  "agent-drawer-trigger",
  "link-import-trigger",
  "preferences-trigger",
  "app-update-check-trigger",
  "recent-header",
  "recent-empty",
  "open-skill:",
  // Technical identity tokens of the evidence ledger (spec §8.1): raw
  // device/inode facts, never App Copy.
  "dev=",
  "ino=",
  "open-window",
  "quit",
]);

const copyLike = /[A-Za-z\u4e00-\u9fff]/;

// Plain string literals in non-JSX positions that contain a space or CJK and
// are not allowlisted are treated as suspicious App Copy.
function looksLikeCopy(value) {
  if (!copyLike.test(value) || value.length < 2) return false;
  if (allowlist.has(value)) return false;
  // Single TitleCase words are almost always copy ("Done", "Retry") unless
  // allowlisted — the main single-word escape hatch for non-JSX literals.
  if (/^[A-Z][a-z]+$/.test(value)) return true;
  if (/^[a-z][a-z0-9-]*$/.test(value)) return false; // ids, codes, class names
  if (/^(https?:\/\/|~\/|\/|\w+:)/.test(value)) return false; // URLs, paths
  if (/^[A-Za-z0-9_.-]+(\.[A-Za-z0-9_-]+)+$/.test(value)) return false; // file names
  if (/^\{[a-zA-Z][a-zA-Z0-9]*\}$/.test(value)) return false; // bare param
  if (/^[A-Za-z0-9_-]+:\s*/.test(value)) return false; // code: detail
  if (/^[0-9.,\s%·→-]+$/.test(value)) return false;
  return true;
}

// Template literals whose non-expression parts are only class-name tokens
// (`status-dot status-dot--{expr}`) or format scaffolding (`{expr} {expr}`)
// are technical, not App Copy.
function looksLikeTechnicalTemplate(value) {
  const parts = value.split("{expr}");
  if (parts.every((part) => part.trim() === "")) return true;
  return parts
    .flatMap((part) => part.split(/\s+/))
    .filter(Boolean)
    .every((token) => /^[a-z][a-z0-9-]*$/.test(token));
}

for (const file of files) {
  const code = await readFile(file, "utf8");
  let ast;
  try {
    ast = parse(code, {
      sourceType: "module",
      plugins: ["jsx", "typescript"],
    });
  } catch (error) {
    fail(`${file}: could not parse: ${error.message}`);
    continue;
  }

  const visit = (node) => {
    if (!node || typeof node.type !== "string") return;
    if (node.type === "JSXText") {
      const value = node.value.trim();
      if (
        value &&
        /[A-Za-z\u4e00-\u9fff]/.test(value) &&
        !allowlist.has(value)
      ) {
        fail(
          `${file}:${node.loc?.start.line}: hardcoded JSX copy "${value.slice(0, 60)}"`,
        );
      }
      return;
    }
    if (node.type === "JSXAttribute" && node.value?.type === "StringLiteral") {
      const name = String(node.name.name ?? node.name);
      if (
        /^(aria-label|aria-description|aria-placeholder|title|placeholder|alt|summary|label)$/.test(
          name,
        )
      ) {
        const value = node.value.value.trim();
        if (looksLikeCopy(value)) {
          fail(
            `${file}:${node.loc?.start.line}: hardcoded ${name} copy "${value.slice(0, 60)}"`,
          );
        }
      }
      return;
    }
    if (node.type === "StringLiteral" || node.type === "TemplateLiteral") {
      const value =
        node.type === "StringLiteral"
          ? node.value
          : node.quasis.map((q) => q.value.raw).join("{expr}");
      const trimmed = value.trim();
      if (
        trimmed &&
        /[A-Za-z\u4e00-\u9fff]/.test(trimmed) &&
        looksLikeCopy(trimmed) &&
        node.loc &&
        !(
          node.type === "TemplateLiteral" && looksLikeTechnicalTemplate(trimmed)
        )
      ) {
        // Only flag literals with real words; template literals need at
        // least one space or CJK to avoid flagging identifier composition.
        if (/\s/.test(trimmed) || /[\u4e00-\u9fff]/.test(trimmed)) {
          fail(
            `${file}:${node.loc?.start.line}: hardcoded copy "${trimmed.slice(0, 60)}"`,
          );
        }
      }
      return;
    }
    for (const key of Object.keys(node)) {
      if (
        key === "loc" ||
        key === "start" ||
        key === "end" ||
        key === "extra"
      ) {
        continue;
      }
      const child = node[key];
      if (Array.isArray(child)) {
        for (const item of child) {
          if (item && typeof item.type === "string") visit(item);
        }
      } else if (child && typeof child.type === "string") {
        visit(child);
      }
    }
  };
  visit(ast);
}

if (failures.length > 0) {
  process.stderr.write(
    `Locale contract violations (${failures.length}):\n${failures.map((f) => `  - ${f}`).join("\n")}\n`,
  );
  process.exit(1);
}

process.stdout.write(
  `Locale contract verified: ${enKeys.length} keys parity, ${[...usedKeys].length} catalog references, no hardcoded App Copy outside the allowlist.\n`,
);
