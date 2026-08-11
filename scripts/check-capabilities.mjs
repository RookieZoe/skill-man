import { readFile, readdir } from "node:fs/promises";
import process from "node:process";
import { URL } from "node:url";

import { containsDirectFileSystemAccess } from "./lib/core-boundaries.mjs";

const [
  packageJson,
  cargoToml,
  capabilityJson,
  tauriConfigJson,
  releaseConfigJson,
] = await Promise.all([
  readFile(new URL("../package.json", import.meta.url), "utf8"),
  readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8"),
  readFile(
    new URL("../src-tauri/capabilities/library-browser.json", import.meta.url),
    "utf8",
  ),
  readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
  readFile(
    new URL("../src-tauri/tauri.release.conf.json", import.meta.url),
    "utf8",
  ),
]);

const packageManifest = JSON.parse(packageJson);
const capability = JSON.parse(capabilityJson);
const tauriConfig = JSON.parse(tauriConfigJson);
const releaseConfig = JSON.parse(releaseConfigJson);
const dependencyNames = Object.keys({
  ...packageManifest.dependencies,
  ...packageManifest.devDependencies,
});
const forbiddenFrontendDependencies = [
  "@tauri-apps/plugin-fs",
  "@tauri-apps/plugin-process",
  "@tauri-apps/plugin-shell",
  "@tauri-apps/plugin-updater",
];

const coreSourceRoot = new URL("../src-tauri/src/core/", import.meta.url);
const coreSourceEntries = await readdir(coreSourceRoot, {
  recursive: true,
  withFileTypes: true,
});
for (const entry of coreSourceEntries) {
  if (!entry.isFile() || !entry.name.endsWith(".rs")) continue;
  const sourceUrl = new URL(
    entry.name,
    new URL(`${entry.parentPath}/`, coreSourceRoot),
  );
  const source = await readFile(sourceUrl, "utf8");
  if (containsDirectFileSystemAccess(source)) {
    fail(
      `Core filesystem access must go through the FileSystem seam: ${sourceUrl.pathname}`,
    );
  }
}

for (const dependency of forbiddenFrontendDependencies) {
  if (dependencyNames.includes(dependency)) {
    fail(`forbidden frontend capability dependency: ${dependency}`);
  }
}

for (const dependency of ["tauri-plugin-fs", "tauri-plugin-shell", "clap"]) {
  if (cargoToml.includes(dependency)) {
    fail(`forbidden Rust dependency or CLI surface: ${dependency}`);
  }
}

for (const permission of capability.permissions ?? []) {
  if (
    permission.startsWith("fs:") ||
    permission.startsWith("process:") ||
    permission.startsWith("shell:") ||
    permission.startsWith("updater:")
  ) {
    fail(`forbidden Library Desk permission: ${permission}`);
  }
}

const updaterEndpoint =
  "https://github.com/RookieZoe/skill-man/releases/latest/download/latest.json";
if (tauriConfig.identifier !== "io.github.rookiezoe.skillman") {
  fail("the permanent Bundle Identifier changed");
}
if (tauriConfig.bundle?.active !== true) {
  fail("local builds must produce an installable App bundle");
}
if (JSON.stringify(tauriConfig.bundle?.targets) !== JSON.stringify(["dmg"])) {
  fail("the MVP bundle target must be DMG only");
}
if (tauriConfig.bundle?.createUpdaterArtifacts !== false) {
  fail("ordinary builds must not require the updater signing private key");
}
if (tauriConfig.bundle?.macOS?.minimumSystemVersion !== "13.0") {
  fail("the MVP minimum macOS version must stay at 13.0");
}
if (tauriConfig.bundle?.macOS?.signingIdentity !== "-") {
  fail("ordinary local App bundles must receive a complete ad-hoc signature");
}
if (
  JSON.stringify(tauriConfig.plugins?.updater?.endpoints) !==
  JSON.stringify([updaterEndpoint])
) {
  fail("the App Update endpoint must stay on the single GitHub latest channel");
}
const updaterPubkey = tauriConfig.plugins?.updater?.pubkey;
if (typeof updaterPubkey !== "string" || updaterPubkey.length === 0) {
  fail("the updater public-key slot must be explicit and non-empty");
}
if (releaseConfig.bundle?.createUpdaterArtifacts !== true) {
  fail("release builds must create signed updater artifacts");
}

const releaseKeyStatus =
  updaterPubkey === "UPDATER_PUBLIC_KEY_REQUIRED_FOR_RELEASE"
    ? " Release remains fail-closed until the updater public-key sentinel is replaced."
    : " The committed updater public-key slot is populated.";
process.stdout.write(
  `Capability and build boundaries verified: Rust-owned updater, local DMG, protected release artifacts.${releaseKeyStatus}\n`,
);

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
}
