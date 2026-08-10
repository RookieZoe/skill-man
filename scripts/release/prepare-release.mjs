#!/usr/bin/env node

import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import process from "node:process";

try {
  const options = parseArguments(process.argv.slice(2));
  const result = await prepareRelease(options);
  process.stdout.write(`${JSON.stringify(result)}\n`);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`prepare-release: ${message}\n`);
  process.exitCode = 1;
}

async function prepareRelease(options) {
  const version = parseTag(options.tag);
  const versions = await readVersions(options.repoRoot);
  for (const [source, actual] of Object.entries(versions)) {
    if (actual !== version) {
      throw new Error(
        `${source} version ${JSON.stringify(actual)} does not match tag ${JSON.stringify(options.tag)}`,
      );
    }
  }
  if (options.checkVersionOnly) {
    return { tag: options.tag, version };
  }

  const notes = (await readFile(options.releaseNotes, "utf8")).trim();
  if (notes.length === 0) {
    throw new Error("release notes must not be empty");
  }

  const artifacts = await listFiles(options.artifactsDir);
  const updaterPath = selectOne(
    artifacts,
    (path) => path.endsWith(".app.tar.gz"),
    "aarch64 .app.tar.gz updater artifact",
  );
  const signaturePath = selectOne(
    artifacts,
    (path) => path.endsWith(".app.tar.gz.sig"),
    "aarch64 .app.tar.gz.sig updater signature",
  );
  const dmgPath = selectOne(artifacts, (path) => path.endsWith(".dmg"), "DMG");
  if (!/(?:^|[_-])aarch64(?:[_.-]|$)/i.test(basename(dmgPath))) {
    throw new Error(`expected an aarch64 DMG, found ${basename(dmgPath)}`);
  }
  if (signaturePath !== `${updaterPath}.sig`) {
    throw new Error(
      "updater signature does not belong to the updater artifact",
    );
  }

  const signature = (await readFile(signaturePath, "utf8")).trim();
  if (signature.length === 0) {
    throw new Error("updater signature must not be empty");
  }

  const updaterSize = (await stat(updaterPath)).size;
  const updaterName = basename(updaterPath);
  const latest = {
    version,
    notes,
    download_size: updaterSize,
    platforms: {
      "darwin-aarch64": {
        signature,
        url: `https://github.com/${options.repository}/releases/download/${encodeURIComponent(options.tag)}/${encodeURIComponent(updaterName)}`,
      },
    },
  };
  const latestContent = `${JSON.stringify(latest, null, 2)}\n`;

  await mkdir(options.outputDir, { recursive: true });
  const latestPath = join(options.outputDir, "latest.json");
  const checksumsPath = join(options.outputDir, "SHA256SUMS");
  await writeFile(latestPath, latestContent);

  const checksumEntries = [
    [basename(updaterPath), await sha256File(updaterPath)],
    [basename(signaturePath), await sha256File(signaturePath)],
    [basename(dmgPath), await sha256File(dmgPath)],
    ["latest.json", sha256(latestContent)],
  ].sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0));
  const checksums = `${checksumEntries
    .map(([name, digest]) => `${digest}  ${name}`)
    .join("\n")}\n`;
  await writeFile(checksumsPath, checksums);

  return {
    tag: options.tag,
    version,
    dmg: dmgPath,
    updater: updaterPath,
    signature: signaturePath,
    latestJson: latestPath,
    checksums: checksumsPath,
  };
}

function parseArguments(args) {
  const checkVersionFlags = args.filter(
    (argument) => argument === "--check-version-only",
  );
  if (checkVersionFlags.length > 1) {
    throw new Error("duplicate argument --check-version-only");
  }
  const checkVersionOnly = checkVersionFlags.length === 1;
  const optionArguments = args.filter(
    (argument) => argument !== "--check-version-only",
  );
  const values = new Map();
  for (let index = 0; index < optionArguments.length; index += 2) {
    const name = optionArguments[index];
    const value = optionArguments[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      throw new Error("arguments must be provided as --name value pairs");
    }
    if (values.has(name)) {
      throw new Error(`duplicate argument ${name}`);
    }
    values.set(name, value);
  }

  const fullReleaseArguments = [
    "--repo-root",
    "--tag",
    "--repository",
    "--artifacts-dir",
    "--release-notes",
    "--output-dir",
  ];
  const required = checkVersionOnly
    ? ["--repo-root", "--tag"]
    : fullReleaseArguments;
  for (const name of required) {
    if (!values.get(name)) {
      throw new Error(`missing required argument ${name}`);
    }
  }
  for (const name of values.keys()) {
    if (!fullReleaseArguments.includes(name)) {
      throw new Error(`unknown argument ${name}`);
    }
  }

  const repoRoot = resolve(values.get("--repo-root"));
  const tag = values.get("--tag");
  if (checkVersionOnly) {
    return { repoRoot, tag, checkVersionOnly };
  }

  const repository = values.get("--repository");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error("--repository must be OWNER/REPO");
  }

  return {
    repoRoot,
    tag,
    checkVersionOnly,
    repository,
    artifactsDir: resolve(values.get("--artifacts-dir")),
    releaseNotes: resolve(values.get("--release-notes")),
    outputDir: resolve(values.get("--output-dir")),
  };
}

function parseTag(tag) {
  const numericComponent = "(?:0|[1-9]\\d*)";
  const match = new RegExp(
    `^v(${numericComponent}\\.${numericComponent}\\.${numericComponent})$`,
  ).exec(tag);
  if (!match) {
    throw new Error("tag must match vX.Y.Z with numeric components");
  }
  return match[1];
}

async function readVersions(repoRoot) {
  const packageJson = await readJson(join(repoRoot, "package.json"));
  const packageLock = await readJson(join(repoRoot, "package-lock.json"));
  const tauriConfig = await readJson(
    join(repoRoot, "src-tauri", "tauri.conf.json"),
  );
  const cargoManifest = await readFile(
    join(repoRoot, "src-tauri", "Cargo.toml"),
    "utf8",
  );

  return {
    "package.json": requireString(packageJson.version, "package.json version"),
    "package-lock.json": requireString(
      packageLock.version,
      "package-lock.json version",
    ),
    'package-lock.json packages[""]': requireString(
      packageLock.packages?.[""]?.version,
      'package-lock.json packages[""] version',
    ),
    "src-tauri/Cargo.toml": readCargoPackageVersion(cargoManifest),
    "src-tauri/tauri.conf.json": requireString(
      tauriConfig.version,
      "tauri.conf.json version",
    ),
  };
}

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

function readCargoPackageVersion(manifest) {
  const lines = manifest.split(/\r?\n/);
  const packageStart = lines.findIndex((line) => line.trim() === "[package]");
  if (packageStart === -1) {
    throw new Error("src-tauri/Cargo.toml is missing [package]");
  }
  const nextSection = lines.findIndex(
    (line, index) => index > packageStart && /^\s*\[/.test(line),
  );
  const packageSection = lines
    .slice(packageStart + 1, nextSection === -1 ? undefined : nextSection)
    .join("\n");
  const matches = [
    ...packageSection.matchAll(/^version\s*=\s*"([^"]+)"\s*$/gm),
  ];
  if (matches.length !== 1) {
    throw new Error("src-tauri/Cargo.toml must contain one package version");
  }
  return matches[0][1];
}

function requireString(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
  return value;
}

async function listFiles(root) {
  const entries = await readdir(root, { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((left, right) =>
    left.name.localeCompare(right.name, "en"),
  )) {
    const path = join(root, entry.name);
    if (entry.isSymbolicLink()) {
      throw new Error(`artifact tree contains a symbolic link: ${path}`);
    }
    if (entry.isDirectory()) {
      files.push(...(await listFiles(path)));
    } else if (entry.isFile()) {
      files.push(path);
    }
  }
  return files;
}

function selectOne(files, predicate, label) {
  const matches = files.filter(predicate);
  if (matches.length !== 1) {
    throw new Error(`expected exactly one ${label}, found ${matches.length}`);
  }
  return matches[0];
}

async function sha256File(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) {
    hash.update(chunk);
  }
  return hash.digest("hex");
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}
