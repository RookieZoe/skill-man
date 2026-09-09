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
    dmg: join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.dmg"),
    updater: join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.app.tar.gz"),
    signature: join(
      fixture.outputDir,
      "Skill-Man-v0.1.0-aarch64.app.tar.gz.sig",
    ),
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
          signature: (await readFile(fixture.signaturePath, "utf8")).trim(),
          url: "https://github.com/RookieZoe/skill-man/releases/download/v0.1.0/Skill-Man-v0.1.0-aarch64.app.tar.gz",
        },
      },
    },
    null,
    2,
  )}\n`;

  assert.equal(
    await readFile(join(fixture.outputDir, "latest.json"), "utf8"),
    expectedLatest,
  );
  const checksums = await readFile(
    join(fixture.outputDir, "SHA256SUMS"),
    "utf8",
  );
  assert.match(
    checksums,
    /a6efebc25a28594915378dea13438990b454b9371de2ea8e43f8ef0ccfba0d59 {2}Skill-Man-v0.1.0-aarch64.app.tar.gz/,
  );
  assert.match(
    checksums,
    /64f4c937b90fbaa324986081a85fab50d886b86f6fd3a03b23926cfefe0b5c83 {2}Skill-Man-v0.1.0-aarch64.dmg/,
  );
  const checkRoot = join(fixture.root, "checksum-check");
  await mkdir(checkRoot);
  for (const path of [
    join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.app.tar.gz"),
    join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.app.tar.gz.sig"),
    join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.dmg"),
    join(fixture.outputDir, "latest.json"),
    join(fixture.outputDir, "SHA256SUMS"),
  ]) {
    await writeFile(
      join(checkRoot, path.split("/").at(-1)),
      await readFile(path),
    );
  }
  const verified = spawnSync("shasum", ["-a", "256", "-c", "SHA256SUMS"], {
    cwd: checkRoot,
    encoding: "utf8",
  });
  assert.equal(verified.status, 0, verified.stderr);

  const second = runCli(fixture);
  assert.equal(second.status, 0, second.stderr);
  assert.equal(
    await readFile(join(fixture.outputDir, "latest.json"), "utf8"),
    expectedLatest,
  );
  assert.equal(
    await readFile(join(fixture.outputDir, "SHA256SUMS"), "utf8"),
    checksums,
  );
});

test("prepare-release rejects an archive changed after Tauri signing", async (t) => {
  const fixture = await createReleaseFixture(t);
  await writeFile(fixture.updaterPath, "tampered updater");
  const result = runCli(fixture);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /signature/i);
  await assert.rejects(readFile(join(fixture.outputDir, "latest.json")));
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

  await signFixture(root, updaterPath, version);
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

async function signFixture(root, updaterPath, version) {
  const keyPath = join(root, "test.key");
  const signer = fileURLToPath(
    new URL("../../node_modules/.bin/tauri", import.meta.url),
  );
  const generated = spawnSync(
    signer,
    ["signer", "generate", "--ci", "-p", "", "-w", keyPath],
    { encoding: "utf8" },
  );
  assert.equal(generated.status, 0, "test key generation failed");
  const signed = spawnSync(
    signer,
    ["signer", "sign", "-f", keyPath, "-p", "", updaterPath],
    { encoding: "utf8" },
  );
  assert.equal(signed.status, 0, "test artifact signing failed");
  await writeFixture(root, "src-tauri/tauri.conf.json", {
    version,
    plugins: {
      updater: { pubkey: (await readFile(`${keyPath}.pub`, "utf8")).trim() },
    },
  });
}

test("release read-back validates Draft transport URLs and published metadata", async (t) => {
  const fixture = await createReleaseFixture(t);
  const prepared = runCli(fixture);
  assert.equal(prepared.status, 0, prepared.stderr);
  const files = [
    join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.app.tar.gz"),
    join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.app.tar.gz.sig"),
    join(fixture.outputDir, "Skill-Man-v0.1.0-aarch64.dmg"),
    join(fixture.outputDir, "latest.json"),
    join(fixture.outputDir, "SHA256SUMS"),
  ];
  const assetsDir = join(fixture.root, "readback");
  await mkdir(assetsDir);
  const assets = [];
  for (const file of files) {
    const name = file.split("/").at(-1);
    const bytes = await readFile(file);
    await writeFile(join(assetsDir, name), bytes);
    assets.push({
      name,
      size: bytes.length,
      browser_download_url: `https://github.com/RookieZoe/skill-man/releases/download/untagged-5c521b9040dc96aac628/${encodeURIComponent(name)}`,
    });
  }
  const releasePath = join(fixture.root, "release.json");
  const release = {
    tag_name: "v0.1.0",
    html_url:
      "https://github.com/RookieZoe/skill-man/releases/tag/untagged-5c521b9040dc96aac628",
    draft: true,
    prerelease: false,
    body: "First signed release.",
    assets,
  };
  await writeFile(releasePath, JSON.stringify(release));
  const cli = fileURLToPath(new URL("./verify-release.mjs", import.meta.url));
  const args = [
    cli,
    "--repo-root",
    fixture.root,
    "--tag",
    "v0.1.0",
    "--repository",
    "RookieZoe/skill-man",
    "--assets-dir",
    assetsDir,
    "--release-json",
    releasePath,
  ];
  const good = spawnSync(process.execPath, args, { encoding: "utf8" });
  assert.equal(good.status, 0, good.stderr);
  release.draft = false;
  await writeFile(releasePath, JSON.stringify(release));
  const unpublishedUrls = spawnSync(process.execPath, args, {
    encoding: "utf8",
  });
  assert.equal(unpublishedUrls.status, 1);
  assert.match(unpublishedUrls.stderr, /asset URL/);
  release.html_url =
    "https://github.com/RookieZoe/skill-man/releases/tag/v0.1.0";
  for (const asset of release.assets) {
    asset.browser_download_url = `https://github.com/RookieZoe/skill-man/releases/download/v0.1.0/${encodeURIComponent(asset.name)}`;
  }
  await writeFile(releasePath, JSON.stringify(release));
  const published = spawnSync(process.execPath, args, { encoding: "utf8" });
  assert.equal(published.status, 0, published.stderr);
  const metadata = JSON.parse(
    await readFile(join(assetsDir, "latest.json"), "utf8"),
  );
  metadata.platforms["darwin-aarch64"].url =
    "https://example.com/unreviewed.app.tar.gz";
  await writeFile(join(assetsDir, "latest.json"), JSON.stringify(metadata));
  const bad = spawnSync(process.execPath, args, { encoding: "utf8" });
  assert.equal(bad.status, 1);
});
