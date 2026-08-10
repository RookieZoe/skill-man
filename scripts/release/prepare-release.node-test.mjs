import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  readFile,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import test from "node:test";
import { URL, fileURLToPath } from "node:url";

const cliPath = fileURLToPath(
  new URL("./prepare-release.mjs", import.meta.url),
);

test("prepare-release creates deterministic updater metadata and checksums", async (t) => {
  const fixture = await createReleaseFixture(t);

  const first = runCli(fixture);
  assert.equal(first.status, 0, first.stderr);
  assert.deepEqual(JSON.parse(first.stdout), {
    tag: "v0.1.0",
    version: "0.1.0",
    dmg: fixture.dmgPath,
    updater: fixture.updaterPath,
    signature: fixture.signaturePath,
    latestJson: join(fixture.outputDir, "latest.json"),
    checksums: join(fixture.outputDir, "SHA256SUMS"),
  });

  const expectedLatest = `${JSON.stringify(
    {
      version: "0.1.0",
      notes: "First signed release.",
      download_size: 20,
      platforms: {
        "darwin-aarch64": {
          signature: "trusted-signature",
          url: "https://github.com/RookieZoe/skill-man/releases/download/v0.1.0/Skill%20Man.app.tar.gz",
        },
      },
    },
    null,
    2,
  )}\n`;
  const expectedChecksums = [
    "a6efebc25a28594915378dea13438990b454b9371de2ea8e43f8ef0ccfba0d59  Skill Man.app.tar.gz",
    "6b607348036613db98866fe3382e91f6b74129ef58cc976238e9be3383579360  Skill Man.app.tar.gz.sig",
    "64f4c937b90fbaa324986081a85fab50d886b86f6fd3a03b23926cfefe0b5c83  Skill Man_0.1.0_aarch64.dmg",
    "ef1a242eefae3a56963bc47795f982576550167c4127d4ace36ea681f6184810  latest.json",
    "",
  ].join("\n");

  assert.equal(
    await readFile(join(fixture.outputDir, "latest.json"), "utf8"),
    expectedLatest,
  );
  assert.equal(
    await readFile(join(fixture.outputDir, "SHA256SUMS"), "utf8"),
    expectedChecksums,
  );

  const second = runCli(fixture);
  assert.equal(second.status, 0, second.stderr);
  assert.equal(
    await readFile(join(fixture.outputDir, "latest.json"), "utf8"),
    expectedLatest,
  );
  assert.equal(
    await readFile(join(fixture.outputDir, "SHA256SUMS"), "utf8"),
    expectedChecksums,
  );
});

test("prepare-release validates synchronized versions before artifacts exist", async (t) => {
  const fixture = await createReleaseFixture(t);

  const result = runVersionCheck(fixture);

  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), {
    tag: "v0.1.0",
    version: "0.1.0",
  });
});

test("prepare-release rejects a non-aarch64 DMG", async (t) => {
  const fixture = await createReleaseFixture(t);
  await rename(
    fixture.dmgPath,
    join(dirname(fixture.dmgPath), "Skill Man_0.1.0_x64.dmg"),
  );

  const result = runCli(fixture);

  assert.equal(result.status, 1);
  assert.match(result.stderr, /aarch64 DMG/);
  await assert.rejects(readFile(join(fixture.outputDir, "latest.json")));
});

test("prepare-release rejects an additional architecture", async (t) => {
  const fixture = await createReleaseFixture(t);
  await writeText(
    fixture.root,
    "artifacts/dmg/Skill Man_0.1.0_x64.dmg",
    "unexpected Intel artifact\n",
  );

  const result = runCli(fixture);

  assert.equal(result.status, 1);
  assert.match(result.stderr, /exactly one DMG/);
});

test("prepare-release rejects a tag with non-SemVer numeric components", async (t) => {
  const fixture = await createReleaseFixture(t, { version: "01.2.3" });

  const result = runCli(fixture, { tag: "v01.2.3" });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /tag must match vX\.Y\.Z/);
});

test("prepare-release rejects version drift before writing metadata", async (t) => {
  const fixture = await createReleaseFixture(t);
  await writeFixture(fixture.root, "package.json", {
    name: "skill-man",
    version: "0.2.0",
  });

  const result = runCli(fixture);

  assert.equal(result.status, 1);
  assert.match(
    result.stderr,
    /package\.json version "0\.2\.0" does not match tag "v0\.1\.0"/,
  );
  await assert.rejects(readFile(join(fixture.outputDir, "latest.json")));
});

test("prepare-release rejects empty release notes", async (t) => {
  const fixture = await createReleaseFixture(t);
  await writeFile(fixture.notesPath, " \n\t");

  const result = runCli(fixture);

  assert.equal(result.status, 1);
  assert.match(result.stderr, /release notes must not be empty/);
});

test("prepare-release rejects duplicate updater artifacts", async (t) => {
  const fixture = await createReleaseFixture(t);
  await writeText(
    fixture.root,
    "artifacts/duplicate/Other.app.tar.gz",
    "unexpected updater\n",
  );

  const result = runCli(fixture);

  assert.equal(result.status, 1);
  assert.match(result.stderr, /exactly one aarch64 \.app\.tar\.gz/);
});

async function createReleaseFixture(t, { version = "0.1.0" } = {}) {
  const root = await mkdtemp(join(tmpdir(), "skill-man-release-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));

  await writeFixture(root, "package.json", {
    name: "skill-man",
    version,
  });
  await writeFixture(root, "package-lock.json", {
    name: "skill-man",
    version,
    lockfileVersion: 3,
    packages: { "": { name: "skill-man", version } },
  });
  await writeText(
    root,
    "src-tauri/Cargo.toml",
    `[package]\nname = "skill-man"\nversion = "${version}"\n`,
  );
  await writeFixture(root, "src-tauri/tauri.conf.json", {
    productName: "Skill Man",
    version,
  });
  const updaterPath = await writeText(
    root,
    "artifacts/macos/Skill Man.app.tar.gz",
    "arm64 updater bytes\n",
  );
  const signaturePath = await writeText(
    root,
    "artifacts/macos/Skill Man.app.tar.gz.sig",
    "trusted-signature\n",
  );
  const dmgPath = await writeText(
    root,
    "artifacts/dmg/Skill Man_0.1.0_aarch64.dmg",
    "signed dmg bytes\n",
  );
  const notesPath = await writeText(
    root,
    "release-notes.md",
    "First signed release.\n",
  );

  return {
    root,
    outputDir: join(root, "prepared"),
    dmgPath,
    updaterPath,
    signaturePath,
    notesPath,
  };
}

function runCli({ root, outputDir }, { tag = "v0.1.0" } = {}) {
  return spawnSync(
    process.execPath,
    [
      cliPath,
      "--repo-root",
      root,
      "--tag",
      tag,
      "--repository",
      "RookieZoe/skill-man",
      "--artifacts-dir",
      join(root, "artifacts"),
      "--release-notes",
      join(root, "release-notes.md"),
      "--output-dir",
      outputDir,
    ],
    { encoding: "utf8" },
  );
}

function runVersionCheck({ root }, { tag = "v0.1.0" } = {}) {
  return spawnSync(
    process.execPath,
    [cliPath, "--repo-root", root, "--tag", tag, "--check-version-only"],
    { encoding: "utf8" },
  );
}

async function writeFixture(root, relativePath, value) {
  await writeText(root, relativePath, `${JSON.stringify(value, null, 2)}\n`);
}

async function writeText(root, relativePath, value) {
  const path = join(root, relativePath);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, value);
  return path;
}
