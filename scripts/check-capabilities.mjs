import { readFile } from "node:fs/promises";
import process from "node:process";
import { URL } from "node:url";

const [packageJson, cargoToml, capabilityJson] = await Promise.all([
  readFile(new URL("../package.json", import.meta.url), "utf8"),
  readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8"),
  readFile(
    new URL("../src-tauri/capabilities/library-browser.json", import.meta.url),
    "utf8",
  ),
]);

const packageManifest = JSON.parse(packageJson);
const capability = JSON.parse(capabilityJson);
const dependencyNames = Object.keys({
  ...packageManifest.dependencies,
  ...packageManifest.devDependencies,
});
const forbiddenFrontendDependencies = [
  "@tauri-apps/plugin-fs",
  "@tauri-apps/plugin-shell",
];

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
  if (permission.startsWith("fs:") || permission.startsWith("shell:")) {
    fail(`forbidden Library Desk permission: ${permission}`);
  }
}

process.stdout.write(
  "Capability boundary verified: no frontend filesystem, shell, or CLI surface.\n",
);

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
}
