#!/usr/bin/env node

import { Buffer } from "node:buffer";
import { spawnSync } from "node:child_process";
import { createWriteStream } from "node:fs";
import {
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import process from "node:process";
import { pipeline } from "node:stream/promises";
import { URL, fileURLToPath } from "node:url";

// Local mode verifies downloaded Draft assets. --public independently fetches
// Latest and every attachment without credentials after maintainer Publish.
let temporary;
try {
  const args = process.argv.slice(2);
  const publicMode = args.includes("--public");
  const pairs = args.filter((arg) => arg !== "--public");
  const options = new Map();
  const allowed = [
    "--repo-root",
    "--tag",
    "--repository",
    "--assets-dir",
    "--release-json",
  ];
  for (let i = 0; i < pairs.length; i += 2) {
    if (!allowed.includes(pairs[i]) || !pairs[i + 1] || options.has(pairs[i]))
      throw new Error("invalid arguments");
    options.set(pairs[i], pairs[i + 1]);
  }
  for (const key of ["--repo-root", "--tag", "--repository"]) {
    if (!options.has(key)) throw new Error(`missing ${key}`);
  }
  const repo = options.get("--repository");
  const tag = options.get("--tag");
  if (
    !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo) ||
    !/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tag)
  )
    throw new Error("invalid repository or tag");
  temporary = await mkdtemp(join(tmpdir(), "skill-man-release-readback-"));
  const assetsDir = publicMode
    ? join(temporary, "assets")
    : resolve(options.get("--assets-dir") ?? "");
  const release = publicMode
    ? await (
        await request(`https://api.github.com/repos/${repo}/releases/latest`)
      ).json()
    : JSON.parse(await readFile(options.get("--release-json"), "utf8"));
  if (
    release.tag_name !== tag ||
    release.prerelease !== false ||
    typeof release.draft !== "boolean" ||
    (publicMode && release.draft) ||
    !release.body?.trim()
  )
    throw new Error("release must match the reviewed formal version and notes");
  if (!Array.isArray(release.assets) || release.assets.length !== 5)
    throw new Error("expected exactly five release assets");
  const names = release.assets.map((asset) => asset.name);
  if (
    new Set(names).size !== 5 ||
    names.some(
      (name) =>
        typeof name !== "string" ||
        basename(name) !== name ||
        /[\\\r\n]/.test(name) ||
        name === "." ||
        name === "..",
    )
  )
    throw new Error("invalid asset names");
  if (publicMode) {
    const { mkdir } = await import("node:fs/promises");
    await mkdir(assetsDir);
  }
  // GitHub gives Draft attachments a temporary ref until Publish. This is
  // only their transport location; updater metadata must still use the tag.
  let downloadRef = tag;
  if (release.draft) {
    const prefix = `https://github.com/${repo}/releases/tag/`;
    if (!release.html_url?.startsWith(prefix))
      throw new Error("invalid Draft release URL");
    downloadRef = release.html_url.slice(prefix.length);
    if (downloadRef !== tag && !/^untagged-[a-f0-9]+$/.test(downloadRef))
      throw new Error("invalid Draft release ref");
  }
  for (const asset of release.assets) {
    const expectedUrl = `https://github.com/${repo}/releases/download/${downloadRef}/${encodeURIComponent(asset.name)}`;
    if (
      asset.browser_download_url !== expectedUrl ||
      !Number.isSafeInteger(asset.size) ||
      asset.size <= 0
    )
      throw new Error("asset URL or size mismatch");
    if (publicMode) {
      const response = await request(expectedUrl);
      await pipeline(
        response.body,
        createWriteStream(join(assetsDir, asset.name), { flags: "wx" }),
      );
    }
    if ((await stat(join(assetsDir, asset.name))).size !== asset.size)
      throw new Error(`asset size mismatch: ${asset.name}`);
  }
  if (
    JSON.stringify((await readdir(assetsDir)).sort()) !==
    JSON.stringify(names.sort())
  )
    throw new Error("downloaded asset set mismatch");
  const notes = join(temporary, "notes.md");
  await writeFile(notes, release.body);
  const result = spawnSync(
    process.execPath,
    [
      fileURLToPath(new URL("./prepare-release.mjs", import.meta.url)),
      "--repo-root",
      resolve(options.get("--repo-root")),
      "--tag",
      tag,
      "--repository",
      repo,
      "--artifacts-dir",
      assetsDir,
      "--release-notes",
      notes,
      "--output-dir",
      join(temporary, "expected"),
    ],
    { encoding: "utf8" },
  );
  if (result.status !== 0)
    throw new Error(result.stderr || "release preparation failed");
  for (const name of ["latest.json", "SHA256SUMS"]) {
    const actual = await readFile(join(assetsDir, name));
    if (!actual.equals(await readFile(join(temporary, "expected", name))))
      throw new Error(
        `${name} does not match the signed assets and Release notes`,
      );
  }
  if (publicMode) {
    const latest = await request(
      `https://github.com/${repo}/releases/latest/download/latest.json`,
    );
    if (
      !Buffer.from(await latest.arrayBuffer()).equals(
        await readFile(join(assetsDir, "latest.json")),
      )
    )
      throw new Error("Latest endpoint differs from the reviewed metadata");
  }
  process.stdout.write(
    `${JSON.stringify({ tag, assets: 5, signatureVerified: true, anonymous: publicMode })}\n`,
  );
} catch (error) {
  process.stderr.write(`verify-release: ${error.message}\n`);
  process.exitCode = 1;
} finally {
  if (temporary) await rm(temporary, { recursive: true, force: true });
}

async function request(url) {
  const response = await globalThis.fetch(url, {
    headers: { "User-Agent": "Skill-Man-release-verification" },
    signal: globalThis.AbortSignal.timeout(120_000),
  });
  if (!response.ok)
    throw new Error(`HTTP ${response.status} while reading ${url}`);
  return response;
}
